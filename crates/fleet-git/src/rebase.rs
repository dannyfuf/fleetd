//! Interactive-rebase operations.

use crate::{
    CommandKind, GitError, MutationResult, ObjectId, Ref, Repository, Result,
    command::{GitCommand, GitOutput},
};
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
    fn interactive_rebase_command(
        &self,
        plan: RebasePlan,
        helper_exe: &Path,
    ) -> Result<GitCommand> {
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
        Ok(command)
    }

    async fn interactive_rebase(
        &self,
        plan: RebasePlan,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let command = self.interactive_rebase_command(plan, helper_exe)?;
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
        let plan = self
            .action_rebase_plan(oid, RebaseAction::Edit, false)
            .await?;
        let rebase = self.interactive_rebase_command(plan, helper_exe)?;
        let amend = self
            .command(CommandKind::Mutation)
            .arg("commit")
            .args(if new_message.is_empty() {
                vec!["--no-edit"]
            } else {
                vec!["-m", new_message]
            })
            .arg("--amend");
        let continue_rebase = self.rebase_control_command("--continue");
        self.run_reword_transaction([rebase, amend, continue_rebase])
            .await
    }

    async fn run_reword_transaction(&self, commands: [GitCommand; 3]) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let mut outputs = Vec::with_capacity(commands.len());
        for command in commands {
            match self.run_reword_command(command).await {
                Ok(output) => outputs.push(output),
                Err(error) => {
                    let _ = self
                        .runner
                        .run(self.rebase_control_command("--abort"))
                        .await;
                    return Err(error);
                }
            }
        }
        Ok(reword_result(outputs))
    }

    async fn run_reword_command(&self, command: GitCommand) -> Result<GitOutput> {
        let may_conflict = command.may_conflict;
        match self.runner.run(command).await {
            Err(GitError::Exit {
                status,
                stdout,
                stderr,
                argv,
                message,
            }) if may_conflict => {
                if self.reword_has_unmerged_entries().await {
                    Err(GitError::Conflict {
                        status,
                        stdout,
                        stderr,
                        argv,
                        message,
                    })
                } else {
                    Err(GitError::Exit {
                        status,
                        stdout,
                        stderr,
                        argv,
                        message,
                    })
                }
            }
            result => result,
        }
    }

    async fn reword_has_unmerged_entries(&self) -> bool {
        self.runner
            .run(
                self.command(CommandKind::Read)
                    .args(["ls-files", "--unmerged", "-z"])
                    .foreground_read(),
            )
            .await
            .is_ok_and(|output| !output.stdout.is_empty())
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
        let base = self.rebase_base(oid, 2).await?;
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
        let plan = self
            .action_rebase_plan(oid, action, include_previous)
            .await?;
        self.interactive_rebase(plan, helper_exe).await
    }

    async fn action_rebase_plan(
        &self,
        oid: &ObjectId,
        action: RebaseAction,
        include_previous: bool,
    ) -> Result<RebasePlan> {
        let ancestor_count = if include_previous { 2 } else { 1 };
        let base = self.rebase_base(oid, ancestor_count).await?;
        Ok(RebasePlan {
            base,
            edits: vec![TodoEdit::Change {
                oid: oid.clone(),
                action,
            }],
            autostash: true,
            keep_empty: true,
        })
    }

    async fn rebase_base(&self, oid: &ObjectId, ancestor_count: usize) -> Result<RebaseBase> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "rev-list".to_owned(),
                        "--first-parent".to_owned(),
                        format!("--max-count={}", ancestor_count + 1),
                        oid.as_str().to_owned(),
                    ])
                    .foreground_read(),
            )
            .await?;
        let history_length = output
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count();
        if history_length == 0 {
            return Err(GitError::parse(
                "rebase base",
                format!("Git returned no history for {oid}"),
            ));
        }
        if history_length <= ancestor_count {
            Ok(RebaseBase::Root)
        } else {
            Ok(RebaseBase::Reference(format!(
                "{}~{ancestor_count}",
                oid.as_str()
            )))
        }
    }

    fn rebase_control_command(&self, action: &str) -> GitCommand {
        self.command(CommandKind::Mutation)
            .args(["rebase", action])
            .env("GIT_EDITOR", "true")
            .may_conflict()
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
        self.run_commands([self.rebase_control_command("--continue")])
            .await
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

fn reword_result(outputs: Vec<GitOutput>) -> MutationResult {
    let warnings: Vec<_> = outputs
        .iter()
        .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_owned())
        .filter(|text| !text.is_empty())
        .collect();
    MutationResult {
        records: outputs.into_iter().map(|output| output.record).collect(),
        warning: (!warnings.is_empty()).then(|| warnings.join("\n")),
    }
}
