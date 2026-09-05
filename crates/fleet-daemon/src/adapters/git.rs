//! Typed Git command-line operations matching swarm inventory section 7.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;

use crate::{
    DaemonError, DaemonResult,
    adapters::shell::{DetachedProcess, Shell, ShellCommand},
};

/// Typed Git operations used by repository and worktree services.
#[async_trait]
pub trait Git: Send + Sync {
    /// Starts `git clone --progress <url> <staging>` detached.
    async fn clone_repo(
        &self,
        url: &str,
        staging: &Path,
        log: &Path,
    ) -> DaemonResult<DetachedProcess>;
    /// Fetches `origin`, optionally pruning removed refs.
    async fn fetch(&self, cwd: &Path, prune: bool) -> DaemonResult<()>;
    /// Fetches exact refspecs from a named remote.
    async fn fetch_refs(&self, cwd: &Path, remote: &str, refs: &[String]) -> DaemonResult<()>;
    /// Reads `refs/remotes/origin/HEAD`.
    async fn origin_head(&self, cwd: &Path) -> DaemonResult<String>;
    /// Repairs origin HEAD with `git remote set-head origin --auto`.
    async fn repair_origin_head(&self, cwd: &Path) -> DaemonResult<()>;
    /// Reads the local symbolic branch checked out at `HEAD`.
    async fn symbolic_head(&self, cwd: &Path) -> DaemonResult<String>;
    /// Checks whether an origin remote-tracking branch exists.
    async fn remote_branch_exists(&self, cwd: &Path, branch: &str) -> DaemonResult<bool>;
    /// Lists origin remote-tracking refs in short form.
    async fn remote_branches(&self, cwd: &Path) -> DaemonResult<Vec<String>>;
    /// Checks out a branch forcibly from `origin/<branch>`.
    async fn checkout_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()>;
    /// Hard-resets the current branch to `origin/<branch>`.
    async fn hard_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()>;
    /// Removes untracked files with `git clean -fd`.
    async fn clean(&self, cwd: &Path) -> DaemonResult<()>;
    /// Creates and checks out a branch from a revision.
    async fn checkout_new_branch(&self, cwd: &Path, branch: &str, from: &str) -> DaemonResult<()>;
    /// Force-creates and checks out a branch from a revision.
    async fn checkout_force_branch(&self, cwd: &Path, branch: &str, from: &str)
    -> DaemonResult<()>;
    /// Checks out an existing branch.
    async fn checkout_branch(&self, cwd: &Path, branch: &str) -> DaemonResult<()>;
    /// Fetches a pull-request head into swarm's compatibility ref namespace.
    async fn fetch_pull_request(&self, cwd: &Path, number: u64) -> DaemonResult<()>;
    /// Resolves a revision with `git rev-parse --verify`.
    async fn revision(&self, cwd: &Path, revision: &str) -> DaemonResult<String>;
    /// Checks whether a revision can be resolved.
    async fn revision_exists(&self, cwd: &Path, revision: &str) -> DaemonResult<bool>;
    /// Returns the checked-out branch.
    async fn current_branch(&self, cwd: &Path) -> DaemonResult<String>;
    /// Returns raw upstream name and tracking status separated by NUL.
    async fn upstream(&self, cwd: &Path, branch: &str) -> DaemonResult<String>;
    /// Returns left/right commit divergence for `<upstream>...HEAD`.
    async fn divergence(&self, cwd: &Path, upstream: &str) -> DaemonResult<(u64, u64)>;
    /// Counts commits in `<target>..HEAD`.
    async fn unique_commits(&self, cwd: &Path, target: &str) -> DaemonResult<u64>;
    /// Counts commits in `<target>..<head>` using explicit revisions.
    async fn unique_commits_from(&self, cwd: &Path, target: &str, head: &str) -> DaemonResult<u64>;
    /// Checks whether one revision is an ancestor of another.
    async fn is_ancestor(&self, cwd: &Path, ancestor: &str, descendant: &str)
    -> DaemonResult<bool>;
    /// Returns porcelain status including normal untracked files.
    async fn status_porcelain(&self, cwd: &Path) -> DaemonResult<String>;
    /// Returns plain porcelain status for Fleet's source-update preflight.
    async fn update_status(&self, cwd: &Path) -> DaemonResult<String>;
    /// Checks whether a path is inside a Git work tree.
    async fn is_inside_work_tree(&self, cwd: &Path) -> DaemonResult<bool>;
    /// Pulls `origin main` with fast-forward only.
    async fn pull_main(&self, cwd: &Path) -> DaemonResult<()>;
    /// Returns the abbreviated HEAD used in build versions.
    async fn short_head(&self, cwd: &Path) -> DaemonResult<String>;
}

/// Git adapter implemented entirely through an injected [`Shell`].
#[derive(Clone)]
pub struct ShellGit {
    shell: Arc<dyn Shell>,
}

impl ShellGit {
    /// Creates a Git adapter backed by `shell`.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>) -> Self {
        Self { shell }
    }

    async fn checked(&self, cwd: &Path, args: &[String], operation: &str) -> DaemonResult<String> {
        let result = self
            .shell
            .run(ShellCommand::new("git").args(args.iter().cloned()).cwd(cwd))
            .await?;
        if !result.success() {
            return Err(DaemonError::Git(format!(
                "{operation} exited {}: {}",
                result.status,
                result.stderr.trim()
            )));
        }
        Ok(result.stdout.trim().to_owned())
    }
}

#[async_trait]
impl Git for ShellGit {
    async fn clone_repo(
        &self,
        url: &str,
        staging: &Path,
        log: &Path,
    ) -> DaemonResult<DetachedProcess> {
        self.shell
            .run_detached(
                ShellCommand::new("git").args([
                    "clone".to_owned(),
                    "--progress".to_owned(),
                    url.to_owned(),
                    staging.to_string_lossy().into_owned(),
                ]),
                log,
            )
            .await
    }

    async fn fetch(&self, cwd: &Path, prune: bool) -> DaemonResult<()> {
        let mut args = vec!["fetch".to_owned()];
        if prune {
            args.push("--prune".to_owned());
        }
        args.push("origin".to_owned());
        self.checked(cwd, &args, "fetch").await.map(drop)
    }

    async fn fetch_refs(&self, cwd: &Path, remote: &str, refs: &[String]) -> DaemonResult<()> {
        let args = std::iter::once("fetch".to_owned())
            .chain(std::iter::once(remote.to_owned()))
            .chain(refs.iter().cloned())
            .collect::<Vec<_>>();
        self.checked(cwd, &args, "fetch refs").await.map(drop)
    }

    async fn origin_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            &[
                "symbolic-ref".to_owned(),
                "refs/remotes/origin/HEAD".to_owned(),
            ],
            "read origin HEAD",
        )
        .await
    }

    async fn repair_origin_head(&self, cwd: &Path) -> DaemonResult<()> {
        self.checked(
            cwd,
            &[
                "remote".to_owned(),
                "set-head".to_owned(),
                "origin".to_owned(),
                "--auto".to_owned(),
            ],
            "repair origin HEAD",
        )
        .await
        .map(drop)
    }

    async fn symbolic_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            &[
                "symbolic-ref".to_owned(),
                "--short".to_owned(),
                "HEAD".to_owned(),
            ],
            "read local symbolic HEAD",
        )
        .await
    }

    async fn remote_branch_exists(&self, cwd: &Path, branch: &str) -> DaemonResult<bool> {
        let result = self
            .shell
            .run(
                ShellCommand::new("git")
                    .args([
                        "show-ref".to_owned(),
                        "--verify".to_owned(),
                        "--quiet".to_owned(),
                        format!("refs/remotes/origin/{branch}"),
                    ])
                    .cwd(cwd),
            )
            .await?;
        match result.status {
            0 => Ok(true),
            1 => Ok(false),
            status => Err(DaemonError::Git(format!(
                "show-ref exited {status}: {}",
                result.stderr.trim()
            ))),
        }
    }

    async fn remote_branches(&self, cwd: &Path) -> DaemonResult<Vec<String>> {
        self.checked(
            cwd,
            &[
                "for-each-ref".to_owned(),
                "--format=%(refname:short)".to_owned(),
                "refs/remotes/origin".to_owned(),
            ],
            "list remote branches",
        )
        .await
        .map(|output| output.lines().map(str::to_owned).collect())
    }

    async fn checkout_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.checked(
            cwd,
            &[
                "checkout".into(),
                "-B".into(),
                branch.into(),
                format!("origin/{branch}"),
            ],
            "checkout reset",
        )
        .await
        .map(drop)
    }

    async fn hard_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.checked(
            cwd,
            &["reset".into(), "--hard".into(), format!("origin/{branch}")],
            "hard reset",
        )
        .await
        .map(drop)
    }

    async fn clean(&self, cwd: &Path) -> DaemonResult<()> {
        self.checked(cwd, &["clean".into(), "-fd".into()], "clean")
            .await
            .map(drop)
    }

    async fn checkout_new_branch(&self, cwd: &Path, branch: &str, from: &str) -> DaemonResult<()> {
        self.checked(
            cwd,
            &["checkout".into(), "-b".into(), branch.into(), from.into()],
            "checkout new branch",
        )
        .await
        .map(drop)
    }

    async fn checkout_force_branch(
        &self,
        cwd: &Path,
        branch: &str,
        from: &str,
    ) -> DaemonResult<()> {
        self.checked(
            cwd,
            &["checkout".into(), "-B".into(), branch.into(), from.into()],
            "checkout force branch",
        )
        .await
        .map(drop)
    }

    async fn checkout_branch(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.checked(cwd, &["checkout".into(), branch.into()], "checkout branch")
            .await
            .map(drop)
    }

    async fn fetch_pull_request(&self, cwd: &Path, number: u64) -> DaemonResult<()> {
        self.checked(
            cwd,
            &[
                "fetch".into(),
                "origin".into(),
                format!("+refs/pull/{number}/head:refs/swarm/pulls/{number}/head"),
            ],
            "fetch pull request",
        )
        .await
        .map(drop)
    }

    async fn revision(&self, cwd: &Path, revision: &str) -> DaemonResult<String> {
        self.checked(
            cwd,
            &["rev-parse".into(), "--verify".into(), revision.into()],
            "resolve revision",
        )
        .await
    }

    async fn revision_exists(&self, cwd: &Path, revision: &str) -> DaemonResult<bool> {
        let result = self
            .shell
            .run(
                ShellCommand::new("git")
                    .args(["rev-parse", "--verify", "--quiet", revision])
                    .cwd(cwd),
            )
            .await?;
        match result.status {
            0 => Ok(true),
            1 => Ok(false),
            status => Err(DaemonError::Git(format!(
                "rev-parse exited {status}: {}",
                result.stderr.trim()
            ))),
        }
    }

    async fn current_branch(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            &["branch".into(), "--show-current".into()],
            "current branch",
        )
        .await
    }

    async fn upstream(&self, cwd: &Path, branch: &str) -> DaemonResult<String> {
        self.checked(
            cwd,
            &[
                "for-each-ref".into(),
                "--format=%(upstream:short)%00%(upstream:track)".into(),
                format!("refs/heads/{branch}"),
            ],
            "upstream",
        )
        .await
    }

    async fn divergence(&self, cwd: &Path, upstream: &str) -> DaemonResult<(u64, u64)> {
        let output = self
            .checked(
                cwd,
                &[
                    "rev-list".into(),
                    "--left-right".into(),
                    "--count".into(),
                    format!("{upstream}...HEAD"),
                ],
                "divergence",
            )
            .await?;
        let mut counts = output.split_whitespace();
        let left = counts
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| DaemonError::Git(format!("invalid divergence output: {output}")))?;
        let right = counts
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| DaemonError::Git(format!("invalid divergence output: {output}")))?;
        Ok((left, right))
    }

    async fn unique_commits(&self, cwd: &Path, target: &str) -> DaemonResult<u64> {
        self.unique_commits_from(cwd, target, "HEAD").await
    }

    async fn unique_commits_from(&self, cwd: &Path, target: &str, head: &str) -> DaemonResult<u64> {
        let output = self
            .checked(
                cwd,
                &[
                    "rev-list".into(),
                    "--count".into(),
                    format!("{target}..{head}"),
                ],
                "count unique commits",
            )
            .await?;
        output
            .parse()
            .map_err(|_| DaemonError::Git(format!("invalid commit count: {output}")))
    }

    async fn is_ancestor(
        &self,
        cwd: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> DaemonResult<bool> {
        let result = self
            .shell
            .run(
                ShellCommand::new("git")
                    .args(["merge-base", "--is-ancestor", ancestor, descendant])
                    .cwd(cwd),
            )
            .await?;
        match result.status {
            0 => Ok(true),
            1 => Ok(false),
            status => Err(DaemonError::Git(format!(
                "merge-base exited {status}: {}",
                result.stderr.trim()
            ))),
        }
    }

    async fn status_porcelain(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            &[
                "status".into(),
                "--porcelain".into(),
                "--untracked-files=normal".into(),
            ],
            "status",
        )
        .await
    }

    async fn update_status(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            &["status".into(), "--porcelain".into()],
            "update status",
        )
        .await
    }

    async fn is_inside_work_tree(&self, cwd: &Path) -> DaemonResult<bool> {
        let result = self
            .shell
            .run(
                ShellCommand::new("git")
                    .args(["rev-parse", "--is-inside-work-tree"])
                    .cwd(cwd),
            )
            .await?;
        Ok(result.success() && result.stdout.trim() == "true")
    }

    async fn pull_main(&self, cwd: &Path) -> DaemonResult<()> {
        self.checked(
            cwd,
            &[
                "pull".into(),
                "--ff-only".into(),
                "origin".into(),
                "main".into(),
            ],
            "pull main",
        )
        .await
        .map(drop)
    }

    async fn short_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            &["rev-parse".into(), "--short".into(), "HEAD".into()],
            "short HEAD",
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        adapters::shell::{ShellCommand, ShellResult},
        testing::fakes::{FakeShell, FakeShellCall},
    };

    use super::*;

    #[tokio::test]
    async fn emits_inventory_fetch_pr_and_update_commands_exactly() {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "git",
            ShellResult {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let git = ShellGit::new(shell.clone());
        let cwd = Path::new("/repo");
        git.fetch(cwd, true)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        git.fetch_pull_request(cwd, 42)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        git.update_status(cwd)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            shell.calls(),
            vec![
                FakeShellCall::Run(
                    ShellCommand::new("git")
                        .args(["fetch", "--prune", "origin"])
                        .cwd(cwd)
                ),
                FakeShellCall::Run(
                    ShellCommand::new("git")
                        .args([
                            "fetch",
                            "origin",
                            "+refs/pull/42/head:refs/swarm/pulls/42/head"
                        ])
                        .cwd(cwd)
                ),
                FakeShellCall::Run(
                    ShellCommand::new("git")
                        .args(["status", "--porcelain"])
                        .cwd(cwd)
                ),
            ]
        );
    }
}
