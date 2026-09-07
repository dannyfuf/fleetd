//! Bounded repository snapshots and operation detection.

use self::refs::order_by_checkout_recency;
use crate::{
    CommandKind, Head, ObjectId, OperationState, RepoSnapshot, Repository, Result, SnapshotOptions,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

mod conflict;
mod diff;
mod history;
mod refs;

impl Repository {
    /// Loads a complete, bounded snapshot; independent reads execute concurrently.
    pub async fn snapshot(&self, options: SnapshotOptions) -> Result<RepoSnapshot> {
        let head = self.read_head();
        let files = self.read_status();
        let branches = self.read_local_branches();
        let remote_branches = self.read_remote_branches(options.include_remotes);
        let remotes = self.read_remotes(options.include_remotes);
        let tags = self.read_tags(options.include_tags);
        let commits = self.read_commits(options.commit_limit);
        let reflog = self.read_reflog(options.reflog_limit);
        let stashes = self.read_stashes();
        let (head, files, local_branches, remote_branches, remotes, tags, commits, reflog, stashes) = tokio::join!(
            head,
            files,
            branches,
            remote_branches,
            remotes,
            tags,
            commits,
            reflog,
            stashes
        );
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let reflog = reflog?;
        let mut local_branches = local_branches?;
        order_by_checkout_recency(&mut local_branches, &reflog);
        Ok(RepoSnapshot {
            root: self.paths.worktree_root.clone(),
            head: head?,
            operation: detect_operation(&self.paths.git_dir).await,
            files: files?,
            local_branches,
            remote_branches: remote_branches?,
            remotes: remotes?,
            tags: tags?,
            commits: commits?,
            reflog,
            stashes: stashes?,
            generation,
        })
    }

    async fn read_head(&self) -> Result<Head> {
        let branch_output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
                    .accept_exit_code(1),
            )
            .await?;
        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_owned();
        let oid_output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-parse", "--verify", "HEAD"])
                    .accept_exit_code(128),
            )
            .await?;
        let oid_text = String::from_utf8_lossy(&oid_output.stdout)
            .trim()
            .to_owned();
        if oid_text.is_empty() {
            return Ok(Head::Unborn {
                name: if branch.is_empty() {
                    "HEAD".to_owned()
                } else {
                    branch
                },
            });
        }
        let description = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["show", "-s", "--format=%s", "HEAD"]),
            )
            .await?;
        let description = String::from_utf8_lossy(&description.stdout)
            .trim()
            .to_owned();
        let oid = ObjectId(oid_text);
        if branch.is_empty() {
            Ok(Head::Detached { oid, description })
        } else {
            Ok(Head::Branch {
                name: branch,
                oid: Some(oid),
                description,
            })
        }
    }

    async fn read_status(&self) -> Result<Vec<crate::FileStatus>> {
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--find-renames",
            ]))
            .await?;
        crate::parse::status::parse(&output.stdout)
    }
}

async fn detect_operation(git_dir: &Path) -> OperationState {
    let merge = git_dir.join("rebase-merge");
    let apply = git_dir.join("rebase-apply");
    let merge_exists = tokio::fs::metadata(&merge)
        .await
        .is_ok_and(|metadata| metadata.is_dir());
    let apply_exists = tokio::fs::metadata(&apply)
        .await
        .is_ok_and(|metadata| metadata.is_dir());
    if merge_exists || apply_exists {
        let directory = if merge_exists { &merge } else { &apply };
        let done = read_count(directory.join("done")).await;
        let remaining = read_count(directory.join("git-rebase-todo")).await;
        return OperationState::Rebasing {
            interactive: merge_exists
                || tokio::fs::metadata(directory.join("interactive"))
                    .await
                    .is_ok(),
            onto: read_trimmed(directory.join("onto")).await,
            head_name: read_trimmed(directory.join("head-name")).await,
            done,
            total: match (done, remaining) {
                (Some(done), Some(remaining)) => Some(done + remaining),
                _ => None,
            },
        };
    }
    if tokio::fs::metadata(git_dir.join("MERGE_HEAD"))
        .await
        .is_ok()
    {
        return OperationState::Merging;
    }
    if tokio::fs::metadata(git_dir.join("CHERRY_PICK_HEAD"))
        .await
        .is_ok()
    {
        return OperationState::CherryPicking;
    }
    if tokio::fs::metadata(git_dir.join("REVERT_HEAD"))
        .await
        .is_ok()
    {
        return OperationState::Reverting;
    }
    if tokio::fs::metadata(git_dir.join("BISECT_START"))
        .await
        .is_ok()
    {
        return OperationState::Bisecting;
    }
    OperationState::None
}

async fn read_trimmed(path: PathBuf) -> Option<String> {
    tokio::fs::read(path)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn read_count(path: PathBuf) -> Option<usize> {
    tokio::fs::read(path).await.ok().map(|bytes| {
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| {
                let line = line.trim_ascii();
                !line.is_empty() && !line.starts_with(b"#")
            })
            .count()
    })
}
