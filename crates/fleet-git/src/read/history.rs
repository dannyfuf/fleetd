use crate::{CommandKind, Commit, Ref, ReflogEntry, Repository, Result, StashEntry};
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
        let mut commits = self.log_commits(&reference.0, limit).await?;
        if let Some(upstream) = upstream {
            self.mark_pushed(&mut commits, &upstream.0, &reference.0)
                .await?;
        }
        Ok(commits)
    }

    pub(super) async fn read_commits(&self, limit: usize) -> Result<Vec<Commit>> {
        let mut commits = self.log_commits("HEAD", limit).await?;
        self.mark_pushed(&mut commits, "@{upstream}", "HEAD")
            .await?;
        Ok(commits)
    }

    /// `git log <revision>`, parsed. An unresolvable revision yields an empty list.
    async fn log_commits(&self, revision: &str, limit: usize) -> Result<Vec<Commit>> {
        let format = "%x1e%H%x00%P%x00%aN%x00%ae%x00%at%x00%ct%x00%s%x00%b%x00%D%x00";
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "log",
                        "--no-show-signature",
                        "--decorate=short",
                        &format!("--max-count={limit}"),
                        &format!("--format={format}"),
                        revision,
                    ])
                    .accept_exit_code(128),
            )
            .await?;
        crate::parse::commits::commits(&output.stdout)
    }

    /// Sets `Commit::pushed` from `git rev-list <upstream>..<tip>`. A missing upstream is not an
    /// error: every commit simply keeps `pushed == false`.
    async fn mark_pushed(&self, commits: &mut [Commit], upstream: &str, tip: &str) -> Result<()> {
        let resolved = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-parse", "--verify", upstream])
                    .accept_exit_code(128),
            )
            .await?;
        if resolved.stdout.is_empty() {
            return Ok(());
        }
        let unpushed = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "rev-list",
                &format!("--max-count={}", commits.len()),
                &format!("{upstream}..{tip}"),
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

    pub(super) async fn read_reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        let format = "%x1e%H%x00%P%x00%gd%x00%gs%x00%ct%x00";
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "log",
                        "-g",
                        "--no-show-signature",
                        &format!("--max-count={limit}"),
                        &format!("--format={format}"),
                        "HEAD",
                    ])
                    .accept_exit_code(128),
            )
            .await?;
        crate::parse::commits::reflog(&output.stdout)
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
