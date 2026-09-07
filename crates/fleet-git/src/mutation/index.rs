use super::result_from_outputs;
use crate::{
    ChangeKind, CommandKind, CommandOutcome, CommitOptions, ConflictChoice, GitError,
    MutationResult, PatchAction, PatchSelection, Repository, Result,
};
use std::{
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::AsyncWriteExt;

static REPLACEMENT_ID: AtomicU64 = AtomicU64::new(0);

impl Repository {
    /// Stages literal repository-relative paths.
    pub async fn stage_paths(&self, paths: &[PathBuf]) -> Result<MutationResult> {
        self.run_commands([self.command(CommandKind::Mutation).arg("add").paths(paths)])
            .await
    }

    /// Unstages literal paths while retaining their worktree content.
    pub async fn unstage_paths(&self, paths: &[PathBuf]) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["reset", "-q"])
            .paths(paths)])
            .await
    }

    /// Stages all tracked and untracked changes.
    pub async fn stage_all(&self) -> Result<MutationResult> {
        self.run_commands([self.command(CommandKind::Mutation).args(["add", "-A"])])
            .await
    }

    /// Unstages every index change.
    pub async fn unstage_all(&self) -> Result<MutationResult> {
        self.run_commands([self.command(CommandKind::Mutation).args(["reset", "-q"])])
            .await
    }

    /// Discards all index/worktree changes for selected paths, deleting selected untracked paths.
    pub async fn discard_paths(&self, paths: &[PathBuf]) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let status_output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "status",
                        "--porcelain=v1",
                        "-z",
                        "--untracked-files=all",
                        "--find-renames",
                    ])
                    .foreground_read(),
            )
            .await?;
        let statuses = crate::parse::status::parse(&status_output.stdout)?;
        let mut records = vec![status_output.record];
        let has_head = self.has_head(&mut records).await?;
        let mut reset = Vec::new();
        let mut checkout = Vec::new();
        let mut remove = Vec::new();
        for requested in paths {
            if let Some(status) = statuses.iter().find(|status| &status.path == requested) {
                if let Some(previous) = &status.previous_path {
                    reset.push(&status.path);
                    reset.push(previous);
                    checkout.push(previous);
                    remove.push(&status.path);
                } else {
                    if status.index != ChangeKind::Unmodified {
                        reset.push(&status.path);
                    }
                    if status.worktree == ChangeKind::Untracked || status.index == ChangeKind::Added
                    {
                        remove.push(&status.path);
                    } else {
                        checkout.push(&status.path);
                    }
                }
            } else {
                checkout.push(requested);
            }
        }
        if !reset.is_empty() {
            let command = if has_head {
                self.command(CommandKind::Mutation)
                    .args(["reset", "-q", "HEAD"])
                    .paths(&reset)
            } else {
                self.command(CommandKind::Mutation)
                    .args(["rm", "-q", "--cached", "--force", "--ignore-unmatch"])
                    .paths(&reset)
            };
            records.push(self.run_one(command).await?.record);
        }
        if has_head && !checkout.is_empty() {
            let command = self
                .command(CommandKind::Mutation)
                .arg("checkout")
                .paths(&checkout);
            records.push(self.run_one(command).await?.record);
        }
        for path in remove {
            remove_worktree_path(&self.paths.worktree_root, path).await?;
        }
        Ok(MutationResult {
            records,
            warning: None,
        })
    }

    /// Discards every tracked, staged, and untracked worktree change.
    pub async fn discard_all(&self) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let mut records = Vec::new();
        let has_head = self.has_head(&mut records).await?;
        let reset = if has_head {
            self.command(CommandKind::Mutation)
                .args(["reset", "-q", "HEAD"])
        } else {
            self.command(CommandKind::Mutation)
                .args(["read-tree", "--empty"])
        };
        records.push(self.run_one(reset).await?.record);
        if has_head {
            records.push(
                self.run_one(
                    self.command(CommandKind::Mutation)
                        .arg("checkout")
                        .paths(["."]),
                )
                .await?
                .record,
            );
        }
        records.push(
            self.run_one(self.command(CommandKind::Mutation).args(["clean", "-fd"]))
                .await?
                .record,
        );
        Ok(MutationResult {
            records,
            warning: None,
        })
    }

    /// Applies selected hunks/lines to the index or worktree.
    pub async fn apply_patch_selection(
        &self,
        selection: PatchSelection,
        action: PatchAction,
    ) -> Result<MutationResult> {
        self.apply_patch_selection_inner(selection, action, None)
            .await
    }

    /// Applies a selection only when the current diff is byte-for-byte equivalent to the
    /// displayed preimage from which its positional hunk and line indexes were chosen.
    pub async fn apply_patch_selection_verified(
        &self,
        selection: PatchSelection,
        action: PatchAction,
        displayed: &crate::Diff,
    ) -> Result<MutationResult> {
        self.apply_patch_selection_inner(selection, action, Some(displayed))
            .await
    }

    async fn apply_patch_selection_inner(
        &self,
        selection: PatchSelection,
        action: PatchAction,
        displayed: Option<&crate::Diff>,
    ) -> Result<MutationResult> {
        match (selection.side, action) {
            (crate::DiffSide::Unstaged, PatchAction::Stage | PatchAction::Discard)
            | (crate::DiffSide::Staged, PatchAction::Unstage) => {}
            _ => {
                return Err(GitError::parse(
                    "patch selection",
                    "action does not match the selected diff side",
                ));
            }
        }
        let _guard = self.mutation_lock.lock().await;
        let diff = self.diff_file(&selection.path, selection.side).await?;
        if displayed.is_some_and(|displayed| displayed != &diff) {
            return Err(GitError::parse(
                "patch selection",
                "the selected diff changed; refresh and select it again",
            ));
        }
        let reverse = matches!(action, PatchAction::Unstage | PatchAction::Discard);
        let patch = crate::patch::build(&diff, &selection.hunks, reverse)?;
        let flags: &[&str] = match action {
            PatchAction::Stage => &["--cached"],
            PatchAction::Unstage => &["--reverse", "--cached"],
            PatchAction::Discard => &["--reverse"],
        };
        let output = self
            .run_one(
                self.command(CommandKind::Mutation)
                    .args(["apply", "--whitespace=nowarn"])
                    .arg_if(self.diff_context() == 0, "--unidiff-zero")
                    .args(flags)
                    .stdin(patch),
            )
            .await?;
        Ok(result_from_outputs(vec![output]))
    }

    /// Creates a commit with an explicit message.
    ///
    /// An empty message uses `--no-edit`, preserving the existing message when amending.
    pub async fn commit(&self, message: &str, options: CommitOptions) -> Result<MutationResult> {
        let message_args: &[&str] = if message.is_empty() {
            &["--no-edit"]
        } else {
            &["-m", message]
        };
        self.run_commands([self
            .command(CommandKind::Mutation)
            .arg("commit")
            .args(message_args)
            .arg_if(options.amend, "--amend")
            .arg_if(options.allow_empty, "--allow-empty")
            .arg_if(options.signoff, "--signoff")
            .arg_if(options.no_verify, "--no-verify")])
            .await
    }

    /// Replaces conflict regions according to `choice` and stages the file.
    pub async fn resolve_conflict(
        &self,
        path: &Path,
        choice: ConflictChoice,
    ) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        confined_regular_file(&self.paths.worktree_root, path).await?;
        let conflict = self.conflicted_file(path).await?;
        if conflict.conflicts.is_empty() {
            return Err(GitError::parse(
                "conflict resolution",
                "file contains no conflict markers",
            ));
        }
        let mut output = Vec::with_capacity(conflict.content.len());
        let mut cursor = 0;
        for section in &conflict.conflicts {
            output.extend_from_slice(&conflict.content[cursor..section.start]);
            match choice {
                ConflictChoice::Ours => output.extend_from_slice(&section.ours),
                ConflictChoice::Theirs => output.extend_from_slice(&section.theirs),
                ConflictChoice::Both => {
                    output.extend_from_slice(&section.ours);
                    output.extend_from_slice(&section.theirs);
                }
            }
            cursor = section.end;
        }
        output.extend_from_slice(&conflict.content[cursor..]);
        replace_verified(&self.paths.worktree_root, path, &conflict.content, &output).await?;
        let command = self.command(CommandKind::Mutation).arg("add").paths([path]);
        let applied = self.run_one(command).await?;
        Ok(result_from_outputs(vec![applied]))
    }

    async fn has_head(&self, records: &mut Vec<crate::CommandRecord>) -> Result<bool> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
                    .accept_exit_code(1)
                    .foreground_read(),
            )
            .await?;
        let has_head = matches!(
            output.record.outcome,
            CommandOutcome::Success {
                status: Some(0),
                ..
            }
        );
        records.push(output.record);
        Ok(has_head)
    }
}

async fn replace_verified(
    root: &Path,
    relative: &Path,
    expected: &[u8],
    replacement: &[u8],
) -> Result<()> {
    let absolute = confined_regular_file(root, relative).await?;
    verify_preimage(&absolute, expected).await?;
    let metadata = tokio::fs::symlink_metadata(&absolute)
        .await
        .map_err(|source| io_error("inspect", &absolute, source))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| GitError::parse("conflict resolution", "path has no parent directory"))?;
    let temporary = replacement_path(parent);
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|source| io_error("create", &temporary, source))?;
    let write_result = async {
        file.set_permissions(metadata.permissions()).await?;
        file.write_all(replacement).await?;
        file.sync_all().await
    }
    .await;
    drop(file);
    if let Err(source) = write_result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(io_error("write", &temporary, source));
    }
    let verification = async {
        let verified = confined_regular_file(root, relative).await?;
        verify_preimage(&verified, expected).await?;
        tokio::fs::rename(&temporary, &verified)
            .await
            .map_err(|source| io_error("replace", &verified, source))
    }
    .await;
    if verification.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    verification
}

async fn confined_regular_file(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(GitError::parse(
            "conflict resolution",
            "path must stay within the worktree",
        ));
    }

    let components: Vec<_> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            Component::CurDir => None,
            _ => None,
        })
        .collect();
    if components.is_empty() {
        return Err(GitError::parse(
            "conflict resolution",
            "path must name a regular file",
        ));
    }

    let mut absolute = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        absolute.push(component);
        let metadata = tokio::fs::symlink_metadata(&absolute)
            .await
            .map_err(|source| io_error("inspect", &absolute, source))?;
        if metadata.file_type().is_symlink() {
            return Err(GitError::parse(
                "conflict resolution",
                "path traverses a symbolic link",
            ));
        }
        let is_last = index + 1 == components.len();
        if (is_last && !metadata.is_file()) || (!is_last && !metadata.is_dir()) {
            return Err(GitError::parse(
                "conflict resolution",
                "path must name a regular file beneath regular directories",
            ));
        }
    }
    Ok(absolute)
}

async fn verify_preimage(path: &Path, expected: &[u8]) -> Result<()> {
    let current = tokio::fs::read(path)
        .await
        .map_err(|source| io_error("read", path, source))?;
    if current != expected {
        return Err(GitError::parse(
            "conflict resolution",
            "file changed after conflict inspection",
        ));
    }
    Ok(())
}

fn replacement_path(parent: &Path) -> PathBuf {
    let id = REPLACEMENT_ID.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".fleet-conflict-{}-{id}.tmp", std::process::id()))
}

fn io_error(action: &str, path: &Path, source: std::io::Error) -> GitError {
    GitError::Spawn {
        argv: vec![format!("{action} {}", path.display())],
        source,
    }
}

async fn remove_worktree_path(root: &Path, relative: &Path) -> Result<()> {
    let absolute = root.join(relative);
    let result = if tokio::fs::symlink_metadata(&absolute)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        tokio::fs::remove_dir_all(&absolute).await
    } else {
        tokio::fs::remove_file(&absolute).await
    };
    match result {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(GitError::Spawn {
            argv: vec![format!("remove {}", absolute.display())],
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::replace_verified;

    #[tokio::test]
    async fn changed_preimage_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("conflict.txt");
        std::fs::write(&path, b"changed\n").unwrap();

        let error = replace_verified(
            root.path(),
            std::path::Path::new("conflict.txt"),
            b"original\n",
            b"replacement\n",
        )
        .await
        .unwrap_err();

        assert!(matches!(error, crate::GitError::Parse { .. }));
        assert_eq!(std::fs::read(path).unwrap(), b"changed\n");
    }
}
