//! Mutation serialization and command-result classification.

use crate::{
    CommandKind, GitError, MutationResult, Repository, Result,
    command::{GitCommand, GitOutput},
};
use std::path::Path;

mod index;
mod network;
mod refs;
mod stash;

impl Repository {
    pub(crate) async fn continue_command(
        &self,
        operation: &str,
        action: &str,
    ) -> Result<MutationResult> {
        self.run_commands(vec![
            self.command(CommandKind::Mutation)
                .args([operation, action])
                .env("GIT_EDITOR", "true")
                .may_conflict(),
        ])
        .await
    }

    pub(crate) async fn run_commands(
        &self,
        commands: impl IntoIterator<Item = GitCommand>,
    ) -> Result<MutationResult> {
        let _guard = self.mutation_lock.lock().await;
        let mut outputs = Vec::new();
        for command in commands {
            outputs.push(self.run_one(command).await?);
        }
        Ok(result_from_outputs(outputs))
    }

    async fn run_one(&self, command: GitCommand) -> Result<GitOutput> {
        let may_conflict = command.may_conflict;
        match self.runner.run(command).await {
            Err(GitError::Exit {
                status,
                stdout,
                stderr,
                argv,
                message,
            }) if may_conflict
                && (indicates_conflict(&stdout, &stderr)
                    || operation_sentinel_exists(&self.paths.git_dir).await) =>
            {
                Err(GitError::Conflict {
                    status,
                    stdout,
                    stderr,
                    argv,
                    message,
                })
            }
            result => result,
        }
    }
}

fn result_from_outputs(outputs: Vec<GitOutput>) -> MutationResult {
    let warning = outputs
        .iter()
        .flat_map(|output| [&output.stderr, &output.stdout])
        .find(|bytes| !bytes.is_empty())
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .filter(|text| !text.is_empty());
    MutationResult {
        records: outputs.into_iter().map(|output| output.record).collect(),
        warning,
    }
}

fn indicates_conflict(stdout: &[u8], stderr: &[u8]) -> bool {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_ascii_lowercase();
    text.contains("conflict") || text.contains("resolve all conflicts")
}

async fn operation_sentinel_exists(git_dir: &Path) -> bool {
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        if tokio::fs::metadata(git_dir.join(name)).await.is_ok() {
            return true;
        }
    }
    false
}
