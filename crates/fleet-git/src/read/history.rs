use crate::{
    CommandKind, Commit, GitError, ObjectId, Ref, ReflogEntry, Repository, Result, StashEntry,
};
use std::collections::HashSet;

impl Repository {
    /// Lists a ref's commits, newest first — lazygit's sub-commits view.
    ///
    /// `upstream` decides `Commit::pushed`: every commit reachable from `reference` but not from
    /// `upstream` is unpushed. Pass `None` to leave the flag `false` for every commit.
    pub async fn commits_for_ref(
        &self,
        reference: &Ref,
        upstream: Option<&Ref>,
        limit: usize,
    ) -> Result<Vec<Commit>> {
        let Some(revision) = self.resolve_commit(&reference.0).await? else {
            return Ok(Vec::new());
        };
        let mut commits = self.log_commits(&revision, limit).await?;
        if let Some(upstream) = upstream {
            self.mark_pushed(&mut commits, &upstream.0, revision.as_str())
                .await?;
        }
        Ok(commits)
    }

    pub(super) async fn read_commits(
        &self,
        revision: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<Commit>> {
        let Some(revision) = revision else {
            return Ok(Vec::new());
        };
        let mut commits = self.log_commits(revision, limit).await?;
        self.mark_pushed(&mut commits, "@{upstream}", revision.as_str())
            .await?;
        Ok(commits)
    }

    async fn log_commits(&self, revision: &ObjectId, limit: usize) -> Result<Vec<Commit>> {
        let format = "%x1e%H%x00%P%x00%aN%x00%ae%x00%at%x00%ct%x00%s%x00%b%x00%D%x00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "log",
                "--no-show-signature",
                "--decorate=short",
                &format!("--max-count={limit}"),
                &format!("--format={format}"),
                revision.as_str(),
            ]))
            .await?;
        crate::parse::commits::commits(&output.stdout)
    }

    /// Sets `Commit::pushed` from `git rev-list <upstream>..<tip>`. A missing upstream is not an
    /// error: every commit simply keeps `pushed == false`.
    async fn mark_pushed(&self, commits: &mut [Commit], upstream: &str, tip: &str) -> Result<()> {
        let resolved = self.resolve_commit(upstream).await?;
        let Some(resolved) = resolved else {
            return Ok(());
        };
        let unpushed = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "rev-list",
                &format!("--max-count={}", commits.len()),
                &format!("{}..{tip}", resolved.as_str()),
            ]))
            .await?;
        let unpushed: HashSet<&str> = std::str::from_utf8(&unpushed.stdout)
            .unwrap_or_default()
            .lines()
            .collect();
        for commit in commits.iter_mut() {
            commit.pushed = !unpushed.contains(commit.oid.as_str());
        }
        Ok(())
    }

    async fn resolve_commit(&self, revision: &str) -> Result<Option<ObjectId>> {
        let revision = format!("{revision}^{{commit}}");
        let output = self
            .run_optional(self.command(CommandKind::Read).args([
                "rev-parse",
                "--verify",
                "--quiet",
                &revision,
            ]))
            .await?;
        let oid = output
            .as_ref()
            .map(|output| crate::parse::text(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        Ok((!oid.is_empty()).then_some(ObjectId(oid)))
    }

    pub(super) async fn read_reflog(
        &self,
        head_exists: bool,
        limit: usize,
    ) -> Result<Vec<ReflogEntry>> {
        if !head_exists {
            return Ok(Vec::new());
        }
        let format = "%x1e%H%x00%P%x00%gd%x00%gs%x00%gd%x00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "log",
                "-g",
                "--no-show-signature",
                "--date=unix",
                &format!("--max-count={limit}"),
                &format!("--format={format}"),
                "HEAD",
            ]))
            .await?;
        let normalized = normalize_reflog_event_times(&output.stdout)?;
        crate::parse::commits::reflog(&normalized)
    }

    pub(super) async fn read_stashes(&self) -> Result<Vec<StashEntry>> {
        let format = "%x1e%gd%x00%H%x00%ct%x00%gs%x00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "stash",
                "list",
                "-z",
                &format!("--format={format}"),
            ]))
            .await?;
        crate::parse::commits::stashes(&output.stdout)
    }
}

fn normalize_reflog_event_times(input: &[u8]) -> Result<Vec<u8>> {
    let records = split_reflog_records(input)?;
    let mut normalized = Vec::with_capacity(input.len());
    for (index, fields) in records.into_iter().enumerate() {
        let dated_selector = crate::parse::text(fields[4]);
        let event_time = dated_selector
            .strip_prefix("HEAD@{")
            .and_then(|value| value.strip_suffix('}'))
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| {
                GitError::parse(
                    "reflog timestamp",
                    format!("invalid dated selector {dated_selector}"),
                )
            })?;
        let selector = format!("HEAD@{{{index}}}");
        let timestamp = event_time.to_string();
        normalized.push(0x1e);
        for field in [
            fields[0],
            fields[1],
            selector.as_bytes(),
            fields[3],
            timestamp.as_bytes(),
        ] {
            normalized.extend_from_slice(field);
            normalized.push(0);
        }
        normalized.push(b'\n');
    }
    Ok(normalized)
}

fn split_reflog_records(input: &[u8]) -> Result<Vec<[&[u8]; 5]>> {
    let mut cursor = 0;
    let mut records = Vec::new();
    while cursor < input.len() {
        while input
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b'\n' | 0))
        {
            cursor += 1;
        }
        if cursor == input.len() {
            break;
        }
        if input[cursor] != 0x1e {
            return Err(GitError::parse("reflog", "record is missing its prefix"));
        }
        cursor += 1;
        let mut fields = [&[][..]; 5];
        for field in &mut fields {
            let end = input[cursor..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|offset| cursor + offset)
                .ok_or_else(|| GitError::parse("reflog", "record is missing a field"))?;
            *field = &input[cursor..end];
            cursor = end + 1;
        }
        records.push(fields);
    }
    Ok(records)
}
