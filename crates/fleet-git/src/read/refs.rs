use crate::{
    Branch, CommandKind, ReflogEntry, Remote, RemoteBranchGroup, Repository, Result, Tag,
    command::GitOutput,
};

impl Repository {
    /// Runs one `for-each-ref` query over `prefix`.
    async fn for_each_ref(&self, sort: &str, format: &str, prefix: &str) -> Result<GitOutput> {
        self.runner
            .run(self.command(CommandKind::Read).args([
                "for-each-ref",
                sort,
                "--format",
                format,
                prefix,
            ]))
            .await
    }

    pub(super) async fn read_local_branches(&self) -> Result<Vec<Branch>> {
        let format = "%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(subject)%00%(objectname)%00%(committerdate:unix)%00";
        let output = self
            .for_each_ref("--sort=-committerdate", format, "refs/heads")
            .await?;
        crate::parse::refs::local_branches(&output.stdout)
    }

    pub(super) async fn read_remote_branches(
        &self,
        enabled: bool,
    ) -> Result<Vec<RemoteBranchGroup>> {
        if !enabled {
            return Ok(Vec::new());
        }
        let format =
            "%(refname:short)%00%(objectname)%00%(subject)%00%(committerdate:unix)%00%(symref)%00";
        let output = self
            .for_each_ref("--sort=-committerdate", format, "refs/remotes")
            .await?;
        crate::parse::refs::remote_branches(&output.stdout)
    }

    pub(super) async fn read_tags(&self, enabled: bool) -> Result<Vec<Tag>> {
        if !enabled {
            return Ok(Vec::new());
        }
        let format = "%(refname:short)%00%(if)%(*objectname)%(then)%(*objectname)%(else)%(objectname)%(end)%00%(creatordate:unix)%00%(subject)%00";
        let output = self
            .for_each_ref("--sort=-creatordate", format, "refs/tags")
            .await?;
        crate::parse::refs::tags(&output.stdout)
    }

    pub(super) async fn read_remotes(&self, enabled: bool) -> Result<Vec<Remote>> {
        if !enabled {
            return Ok(Vec::new());
        }
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["remote", "--verbose"]),
            )
            .await?;
        if let Ok(remotes) = parse_remotes(&output.stdout)
            && remotes
                .iter()
                .all(|remote| remote.fetch_url.is_some() && remote.push_url.is_some())
        {
            return Ok(remotes);
        }
        // Unusual URL framing or incomplete configuration needs get-url's original
        // output and error classification, rather than an error from the batch parser.
        let names = self
            .runner
            .run(self.command(CommandKind::Read).arg("remote"))
            .await?;
        let mut remotes = Vec::new();
        for name in String::from_utf8_lossy(&names.stdout).lines() {
            let fetch = self
                .runner
                .run(
                    self.command(CommandKind::Read)
                        .args(["remote", "get-url", name]),
                )
                .await?;
            let push = self
                .runner
                .run(
                    self.command(CommandKind::Read)
                        .args(["remote", "get-url", "--push", name])
                        .accept_exit_code(2),
                )
                .await?;
            remotes.push(Remote {
                name: name.to_owned(),
                fetch_url: nonempty_text(&fetch.stdout),
                push_url: nonempty_text(&push.stdout),
            });
        }
        Ok(remotes)
    }
}

// Checkout recency uses the branch just left (the `from` side of the reflog
// entry); the currently checked-out branch is always placed first separately.
fn checkout_recency(reflog: &[ReflogEntry]) -> Vec<(String, i64)> {
    const PREFIX: &str = "checkout: moving from ";
    let mut seen: Vec<(String, i64)> = Vec::new();
    for entry in reflog {
        let Some(rest) = entry.subject.strip_prefix(PREFIX) else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        if name.is_empty() || seen.iter().any(|(known, _)| known == name) {
            continue;
        }
        seen.push((name.to_owned(), entry.committed_at));
    }
    seen
}

// Preserve committer-date order for branches absent from the loaded reflog.
pub(super) fn order_by_checkout_recency(branches: &mut Vec<Branch>, reflog: &[ReflogEntry]) {
    let recency = checkout_recency(reflog);
    let mut ordered: Vec<Branch> = Vec::with_capacity(branches.len());
    for (name, at) in recency {
        // The head branch is placed first below, so it never joins the recency run.
        if let Some(index) = branches
            .iter()
            .position(|branch| !branch.is_head && branch.name.eq_ignore_ascii_case(&name))
        {
            let mut branch = branches.remove(index);
            branch.checked_out_at = Some(at);
            ordered.push(branch);
        }
    }
    ordered.append(branches);
    if let Some(index) = ordered.iter().position(|branch| branch.is_head) {
        let head = ordered.remove(index);
        ordered.insert(0, head);
    }
    *branches = ordered;
}

fn nonempty_text(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn parse_remotes(bytes: &[u8]) -> Result<Vec<Remote>> {
    let mut remotes = std::collections::BTreeMap::<&str, Remote>::new();
    let output = String::from_utf8_lossy(bytes);
    for line in output.lines() {
        let (name, url) = line
            .split_once('\t')
            .ok_or_else(|| crate::GitError::parse("remotes", "missing URL separator"))?;
        let remote = remotes.entry(name).or_insert_with(|| Remote {
            name: name.to_owned(),
            fetch_url: None,
            push_url: None,
        });
        // Git expands insteadOf/pushInsteadOf and may list several push URLs.
        // get-url, used by the original single-remote reader, returns the first.
        let (slot, url) = if let Some(url) = url.strip_suffix(" (fetch)") {
            (&mut remote.fetch_url, url)
        } else if let Some(url) = url.strip_suffix(" (push)") {
            (&mut remote.push_url, url)
        } else {
            return Err(crate::GitError::parse("remotes", "missing URL direction"));
        };
        if slot.is_none() {
            let url = url.trim();
            if !url.is_empty() {
                *slot = Some(url.to_owned());
            }
        }
    }
    Ok(remotes.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::{checkout_recency, order_by_checkout_recency};
    use crate::{Branch, ObjectId, ReflogEntry};

    fn entry(subject: &str, at: i64) -> ReflogEntry {
        ReflogEntry {
            oid: ObjectId::from("a"),
            parents: Vec::new(),
            selector: String::new(),
            subject: subject.to_owned(),
            committed_at: at,
        }
    }

    fn branch(name: &str, is_head: bool, committed_at: i64) -> Branch {
        Branch {
            name: name.to_owned(),
            oid: ObjectId::from(name),
            is_head,
            upstream: None,
            subject: String::new(),
            committed_at,
            checked_out_at: None,
        }
    }

    #[test]
    fn checkout_recency_reads_the_from_side_once_per_branch() {
        let reflog = vec![
            entry("checkout: moving from feature to main", 300),
            entry("commit: work", 250),
            entry("checkout: moving from main to feature", 200),
            entry("checkout: moving from topic to main", 100),
        ];
        assert_eq!(
            checkout_recency(&reflog),
            vec![
                ("feature".to_owned(), 300),
                ("main".to_owned(), 200),
                ("topic".to_owned(), 100),
            ]
        );
        assert!(checkout_recency(&[entry("commit: only", 1)]).is_empty());
    }

    #[test]
    fn branches_are_ordered_head_first_then_by_checkout_recency() {
        // `for-each-ref --sort=-committerdate` order on the way in.
        let mut branches = vec![
            branch("feature", false, 400),
            branch("main", true, 300),
            branch("topic", false, 200),
            branch("stale", false, 100),
        ];
        let reflog = vec![
            entry("checkout: moving from topic to main", 300),
            entry("checkout: moving from feature to topic", 200),
        ];
        order_by_checkout_recency(&mut branches, &reflog);
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "topic", "feature", "stale"]);
        assert_eq!(branches[1].checked_out_at, Some(300));
        assert_eq!(branches[2].checked_out_at, Some(200));
        assert_eq!(branches[3].checked_out_at, None);
    }

    #[test]
    fn an_empty_reflog_keeps_committerdate_order_with_the_head_first() {
        let mut branches = vec![
            branch("feature", false, 400),
            branch("main", true, 300),
            branch("topic", false, 200),
        ];
        order_by_checkout_recency(&mut branches, &[]);
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "feature", "topic"]);
    }
}
