//! Interactive-rebase operations.

use crate::{CommandKind, GitError, MutationResult, ObjectId, Ref, Repository, Result};
use sequence_editor::SEQUENCE_INSTRUCTION_ENV;
use std::path::Path;

mod plan;
/// Entry points used when the application executable is reinvoked by Git as a
/// sequence editor.
pub mod sequence_editor;

pub use plan::MoveDirection;
use plan::{RebaseAction, RebaseBase, RebasePlan, TodoEdit};

impl Repository {
    /// Runs an interactive rebase whose todo is rewritten by `helper_exe`.
    async fn interactive_rebase(
        &self,
        plan: RebasePlan,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let instruction = serde_json::to_string(&plan)
            .map_err(|error| GitError::parse("rebase plan", error.to_string()))?;
        let command = self
            .command(CommandKind::Mutation)
            .args(["rebase", "--interactive"])
            .env("GIT_SEQUENCE_EDITOR", shell_quote(helper_exe))
            .env("GIT_EDITOR", "true")
            .env(SEQUENCE_INSTRUCTION_ENV, instruction)
            .env("LANG", "C")
            .env("LC_MESSAGES", "C")
            .may_conflict()
            .arg_if(plan.autostash, "--autostash");
        let command = if plan.keep_empty {
            command.args(["--keep-empty", "--empty=keep"])
        } else {
            command
        };
        let command = command.arg("--no-autosquash");
        let command = match plan.base {
            RebaseBase::Root => command.arg("--root"),
            RebaseBase::Reference(base) => command.arg(base),
        };
        self.run_commands([command]).await
    }

    /// Squashes `oid` into its previous commit.
    pub async fn squash_into_previous(
        &self,
        oid: &ObjectId,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Squash, true, helper_exe)
            .await
    }

    /// Fixes `oid` into its previous commit, discarding its message.
    pub async fn fixup_into_previous(
        &self,
        oid: &ObjectId,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Fixup { flag: None }, true, helper_exe)
            .await
    }

    /// Drops `oid` from the current history.
    pub async fn drop_commit(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Drop, false, helper_exe)
            .await
    }

    /// Rewords `oid` without opening an editor.
    pub async fn reword_commit(
        &self,
        oid: &ObjectId,
        new_message: &str,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let mut result = self
            .action_rebase(oid, RebaseAction::Edit, false, helper_exe)
            .await?;
        let amended = self
            .commit(
                new_message,
                crate::CommitOptions {
                    amend: true,
                    ..crate::CommitOptions::default()
                },
            )
            .await?;
        result.records.extend(amended.records);
        if result.warning.is_none() {
            result.warning = amended.warning;
        }
        let continued = self.rebase_continue().await?;
        result.records.extend(continued.records);
        if result.warning.is_none() {
            result.warning = continued.warning;
        }
        Ok(result)
    }

    /// Moves `oid` one commit in the requested history direction.
    pub async fn move_commit(
        &self,
        oid: &ObjectId,
        direction: MoveDirection,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let offset = match direction {
            MoveDirection::Up => 1,
            MoveDirection::Down => -1,
        };
        let base = RebaseBase::Reference(format!("{}^^", oid.as_str()));
        self.interactive_rebase(
            RebasePlan {
                base,
                edits: vec![TodoEdit::Move {
                    oids: vec![oid.clone()],
                    offset,
                }],
                autostash: true,
                keep_empty: true,
            },
            helper_exe,
        )
        .await
    }

    /// Starts a rebase that pauses after applying `oid`.
    pub async fn edit_commit(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Edit, false, helper_exe)
            .await
    }

    async fn action_rebase(
        &self,
        oid: &ObjectId,
        action: RebaseAction,
        include_previous: bool,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let suffix = if include_previous { "^^" } else { "^" };
        self.interactive_rebase(
            RebasePlan {
                base: RebaseBase::Reference(format!("{}{suffix}", oid.as_str())),
                edits: vec![TodoEdit::Change {
                    oid: oid.clone(),
                    action,
                }],
                autostash: true,
                keep_empty: true,
            },
            helper_exe,
        )
        .await
    }

    /// Rebases HEAD onto a ref.
    pub async fn rebase_onto(&self, name: &Ref) -> Result<MutationResult> {
        self.run_commands([self
            .command(CommandKind::Mutation)
            .args(["rebase", "--autostash"])
            .arg(&name.0)
            .env("GIT_EDITOR", "true")
            .may_conflict()])
            .await
    }

    /// Continues an active rebase.
    pub async fn rebase_continue(&self) -> Result<MutationResult> {
        self.continue_command("rebase", "--continue").await
    }

    /// Aborts an active rebase.
    pub async fn rebase_abort(&self) -> Result<MutationResult> {
        self.continue_command("rebase", "--abort").await
    }

    /// Skips the current rebase commit.
    pub async fn rebase_skip(&self) -> Result<MutationResult> {
        self.continue_command("rebase", "--skip").await
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
