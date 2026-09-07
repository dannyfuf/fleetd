//! Typed Git command-line operations for repository and worktree services.

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
        pid_file: &Path,
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
pub struct ShellGit<S: ?Sized = dyn Shell> {
    shell: Arc<S>,
}

impl<S: ?Sized> Clone for ShellGit<S> {
    fn clone(&self) -> Self {
        Self {
            shell: Arc::clone(&self.shell),
        }
    }
}

impl<S: Shell + ?Sized> ShellGit<S> {
    /// Creates a Git adapter backed by `shell`.
    #[must_use]
    pub fn new(shell: Arc<S>) -> Self {
        Self { shell }
    }

    async fn checked<A: Into<String>>(
        &self,
        cwd: &Path,
        args: impl IntoIterator<Item = A> + Send,
        operation: &str,
    ) -> DaemonResult<String> {
        let result = self
            .shell
            .run(ShellCommand::new("git").args(args).cwd(cwd))
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
impl<S: Shell + ?Sized> Git for ShellGit<S> {
    async fn clone_repo(
        &self,
        url: &str,
        staging: &Path,
        log: &Path,
        pid_file: &Path,
    ) -> DaemonResult<DetachedProcess> {
        let pid_file_text = pid_file.to_string_lossy().into_owned();
        let process = self
            .shell
            .run_detached(
                ShellCommand::new("sh").args([
                    "-c".to_owned(),
                    concat!(
                        "set -eu\n",
                        "pid_file=$1\n",
                        "temporary=${pid_file}.tmp\n",
                        "umask 077\n",
                        "printf '%s\\n' \"$$\" > \"$temporary\"\n",
                        "mv \"$temporary\" \"$pid_file\"\n",
                        "exec git clone --progress -- \"$2\" \"$3\"",
                    )
                    .to_owned(),
                    "fleet-clone".to_owned(),
                    pid_file_text,
                    url.to_owned(),
                    staging.to_string_lossy().into_owned(),
                ]),
                log,
            )
            .await?;
        if let Some(parent) = pid_file.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| DaemonError::fs(parent, error))?;
        }
        if !pid_file.exists() {
            tokio::fs::write(pid_file, format!("{}\n", process.pid))
                .await
                .map_err(|error| DaemonError::fs(pid_file, error))?;
        }
        Ok(process)
    }

    async fn fetch(&self, cwd: &Path, prune: bool) -> DaemonResult<()> {
        let mut args = vec!["fetch"];
        if prune {
            args.push("--prune");
        }
        args.push("origin");
        self.checked(cwd, args, "fetch").await.map(drop)
    }

    async fn fetch_refs(&self, cwd: &Path, remote: &str, refs: &[String]) -> DaemonResult<()> {
        let args = ["fetch", remote]
            .into_iter()
            .chain(refs.iter().map(String::as_str));
        self.checked(cwd, args, "fetch refs").await.map(drop)
    }

    async fn origin_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            ["symbolic-ref", "refs/remotes/origin/HEAD"],
            "read origin HEAD",
        )
        .await
    }

    async fn repair_origin_head(&self, cwd: &Path) -> DaemonResult<()> {
        self.checked(
            cwd,
            ["remote", "set-head", "origin", "--auto"],
            "repair origin HEAD",
        )
        .await
        .map(drop)
    }

    async fn symbolic_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(
            cwd,
            ["symbolic-ref", "--short", "HEAD"],
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
                        "show-ref",
                        "--verify",
                        "--quiet",
                        &format!("refs/remotes/origin/{branch}"),
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
            [
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/remotes/origin",
            ],
            "list remote branches",
        )
        .await
        .map(|output| output.lines().map(str::to_owned).collect())
    }

    async fn checkout_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.checked(
            cwd,
            ["checkout", "-B", branch, &format!("origin/{branch}")],
            "checkout reset",
        )
        .await
        .map(drop)
    }

    async fn hard_reset(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.checked(
            cwd,
            ["reset", "--hard", &format!("origin/{branch}")],
            "hard reset",
        )
        .await
        .map(drop)
    }

    async fn clean(&self, cwd: &Path) -> DaemonResult<()> {
        self.checked(cwd, ["clean", "-fd"], "clean").await.map(drop)
    }

    async fn checkout_new_branch(&self, cwd: &Path, branch: &str, from: &str) -> DaemonResult<()> {
        self.checked(cwd, ["checkout", "-b", branch, from], "checkout new branch")
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
            ["checkout", "-B", branch, from],
            "checkout force branch",
        )
        .await
        .map(drop)
    }

    async fn checkout_branch(&self, cwd: &Path, branch: &str) -> DaemonResult<()> {
        self.checked(cwd, ["checkout", branch], "checkout branch")
            .await
            .map(drop)
    }

    async fn fetch_pull_request(&self, cwd: &Path, number: u64) -> DaemonResult<()> {
        self.checked(
            cwd,
            [
                "fetch",
                "origin",
                &format!("+refs/pull/{number}/head:refs/swarm/pulls/{number}/head"),
            ],
            "fetch pull request",
        )
        .await
        .map(drop)
    }

    async fn revision(&self, cwd: &Path, revision: &str) -> DaemonResult<String> {
        self.checked(cwd, ["rev-parse", "--verify", revision], "resolve revision")
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
        self.checked(cwd, ["branch", "--show-current"], "current branch")
            .await
    }

    async fn upstream(&self, cwd: &Path, branch: &str) -> DaemonResult<String> {
        self.checked(
            cwd,
            [
                "for-each-ref",
                "--format=%(upstream:short)%00%(upstream:track)",
                &format!("refs/heads/{branch}"),
            ],
            "upstream",
        )
        .await
    }

    async fn divergence(&self, cwd: &Path, upstream: &str) -> DaemonResult<(u64, u64)> {
        let output = self
            .checked(
                cwd,
                [
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{upstream}...HEAD"),
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

    async fn unique_commits_from(&self, cwd: &Path, target: &str, head: &str) -> DaemonResult<u64> {
        let output = self
            .checked(
                cwd,
                ["rev-list", "--count", &format!("{target}..{head}")],
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
            ["status", "--porcelain", "--untracked-files=normal"],
            "status",
        )
        .await
    }

    async fn update_status(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(cwd, ["status", "--porcelain"], "update status")
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
        self.checked(cwd, ["pull", "--ff-only", "origin", "main"], "pull main")
            .await
            .map(drop)
    }

    async fn short_head(&self, cwd: &Path) -> DaemonResult<String> {
        self.checked(cwd, ["rev-parse", "--short", "HEAD"], "short HEAD")
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
