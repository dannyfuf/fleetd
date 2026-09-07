use super::result_from_outputs;
use crate::{
    ChangeKind, CommandKind, CommitOptions, ConflictChoice, GitError, MutationResult, PatchAction,
    PatchSelection, Repository, Result,
};
use std::path::{Path, PathBuf};

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
            let command = self
                .command(CommandKind::Mutation)
                .args(["reset", "-q", "HEAD"])
                .paths(&reset);
            records.push(self.run_one(command).await?.record);
        }
        if !checkout.is_empty() {
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
        self.run_commands([
            self.command(CommandKind::Mutation)
                .args(["reset", "-q", "HEAD"]),
            self.command(CommandKind::Mutation)
                .arg("checkout")
                .paths(["."]),
            self.command(CommandKind::Mutation).args(["clean", "-fd"]),
        ])
        .await
    }

    /// Applies selected hunks/lines to the index or worktree.
    pub async fn apply_patch_selection(
        &self,
        selection: PatchSelection,
        action: PatchAction,
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
        let absolute = self.paths.worktree_root.join(path);
        tokio::fs::write(&absolute, output)
            .await
            .map_err(|source| GitError::Spawn {
                argv: vec![format!("write {}", absolute.display())],
                source,
            })?;
        let command = self.command(CommandKind::Mutation).arg("add").paths([path]);
        let applied = self.run_one(command).await?;
        Ok(result_from_outputs(vec![applied]))
    }
}

async fn remove_worktree_path(root: &Path, relative: &Path) -> Result<()> {
    let absolute = root.join(relative);
    let result = if tokio::fs::metadata(&absolute)
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
