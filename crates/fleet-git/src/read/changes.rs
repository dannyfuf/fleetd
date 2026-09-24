//! What the checked-out branch changed against its base: the Workspace's Changes panel.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    BranchChanges, BranchFile, CommandKind, DiffKind, ObjectId, Ref, Repository, Result,
    parse::changes::{ahead_commits, numstat},
};

/// An untracked file larger than this is listed without line counts: counting means reading it.
const MAX_COUNTED_UNTRACKED_BYTES: u64 = 1024 * 1024;

impl Repository {
    /// Everything this worktree changed against `base`: the files, committed or not, compared
    /// with the merge base of `HEAD` and `base`, and the commits `HEAD` has that `base` lacks.
    ///
    /// `Ok(None)` when there is nothing to compare: `base` names no commit here, or `HEAD` is
    /// unborn. At most `commit_limit` commits are listed; [`BranchChanges::ahead`] counts all.
    pub async fn branch_changes(
        &self,
        base: &Ref,
        commit_limit: usize,
    ) -> Result<Option<BranchChanges>> {
        let Some(merge_base) = self.branch_merge_base(base).await? else {
            return Ok(None);
        };
        let names = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "diff",
                        "--no-color",
                        "--no-ext-diff",
                        "--find-renames",
                        "--name-status",
                        "-z",
                    ])
                    .arg(merge_base.as_str()),
            )
            .await?;
        let stats = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "diff",
                        "--no-color",
                        "--no-ext-diff",
                        "--find-renames",
                        "--numstat",
                        "-z",
                    ])
                    .arg(merge_base.as_str()),
            )
            .await?;
        let untracked = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
            ]))
            .await?;

        let counts: HashMap<PathBuf, (Option<u64>, Option<u64>)> = numstat(&stats.stdout)?
            .into_iter()
            .map(|(path, added, removed)| (path, (added, removed)))
            .collect();
        let mut files: Vec<BranchFile> = crate::parse::diff::commit_files(&names.stdout)?
            .into_iter()
            .map(|file| {
                let (added, removed) = counts.get(&file.path).copied().unwrap_or((None, None));
                BranchFile {
                    path: file.path,
                    previous_path: file.previous_path,
                    kind: file.kind,
                    added,
                    removed,
                }
            })
            .collect();
        for path in untracked
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = crate::parse::status::bytes_to_path(path);
            let added = self.count_lines(&path).await;
            files.push(BranchFile {
                path,
                previous_path: None,
                kind: DiffKind::Added,
                added,
                removed: added.map(|_| 0),
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let range = format!("{}..HEAD", merge_base.as_str());
        let log = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "log",
                "--no-show-signature",
                "-z",
                "--format=%H%x1f%s",
                &format!("--max-count={commit_limit}"),
                &range,
            ]))
            .await?;
        let ahead = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-list", "--count", &range]),
            )
            .await?;
        let ahead = crate::parse::text(&ahead.stdout)
            .trim()
            .parse()
            .map_err(|_| crate::GitError::parse("ahead count", "not a number"))?;

        Ok(Some(BranchChanges {
            base: base.clone(),
            merge_base,
            files,
            commits: ahead_commits(&log.stdout)?,
            ahead,
        }))
    }

    /// One path's unified diff against the merge base of `HEAD` and `base`, working tree
    /// included, as text for an inline diff view.
    ///
    /// An untracked path is diffed against `/dev/null`. `Ok(None)` on the same terms as
    /// [`Repository::branch_changes`].
    pub async fn branch_file_diff(&self, base: &Ref, path: &Path) -> Result<Option<String>> {
        let Some(merge_base) = self.branch_merge_base(base).await? else {
            return Ok(None);
        };
        let tracked = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["ls-files", "--error-unmatch", "-z"])
                    .paths([path])
                    .accept_exit_code(1),
            )
            .await?;
        let command = self
            .command(CommandKind::Read)
            .arg("diff")
            .args(self.diff_args());
        let command = if tracked.stdout.is_empty() && !self.in_tree(&merge_base, path).await? {
            command
                .arg("--no-index")
                .paths([Path::new("/dev/null"), path])
                .accept_exit_code(1)
        } else {
            command.arg(merge_base.as_str()).paths([path])
        };
        let output = self.runner.run(command).await?;
        Ok(Some(crate::parse::text(&output.stdout).into_owned()))
    }

    /// The merge base of `HEAD` and `base`, or `None` when either names no commit.
    async fn branch_merge_base(&self, base: &Ref) -> Result<Option<ObjectId>> {
        let Some(base) = self.resolve_commit(&base.0).await? else {
            return Ok(None);
        };
        if self.resolve_commit("HEAD").await?.is_none() {
            return Ok(None);
        }
        let output = self
            .run_optional(
                self.command(CommandKind::Read)
                    .args(["merge-base", "HEAD"])
                    .arg(base.as_str()),
            )
            .await?;
        // Unrelated histories have no merge base; comparing with the base itself is the most
        // useful answer left.
        Ok(Some(output.map_or(base, |output| {
            ObjectId(crate::parse::text(&output.stdout).trim().to_owned())
        })))
    }

    /// Whether `path` exists in `commit`'s tree, which a deleted path does and an untracked
    /// one does not.
    async fn in_tree(&self, commit: &ObjectId, path: &Path) -> Result<bool> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["ls-tree", "--name-only", "-z"])
                    .arg(commit.as_str())
                    .paths([path]),
            )
            .await?;
        Ok(!output.stdout.is_empty())
    }

    /// Lines in an untracked text file, or `None` for a binary, large or unreadable one.
    async fn count_lines(&self, path: &Path) -> Option<u64> {
        let full = self.paths.worktree_root.join(path);
        let metadata = tokio::fs::metadata(&full).await.ok()?;
        if !metadata.is_file() || metadata.len() > MAX_COUNTED_UNTRACKED_BYTES {
            return None;
        }
        let bytes = tokio::fs::read(&full).await.ok()?;
        if bytes.contains(&0) {
            return None;
        }
        let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
        let unterminated = usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
        u64::try_from(newlines + unterminated).ok()
    }
}
