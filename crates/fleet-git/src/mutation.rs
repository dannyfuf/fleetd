//! Mutation serialization and command-result classification.

use crate::{
    CommandKind, GitError, MutationResult, Repository, Result,
    command::{GitCommand, GitOutput},
};
use std::sync::atomic::Ordering;

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
        if command.kind != CommandKind::Read {
            self.snapshot_invalidation.fetch_add(1, Ordering::AcqRel);
        }
        match self.runner.run(command).await {
            Err(GitError::Exit {
                status,
                stdout,
                stderr,
                argv,
                message,
            }) if may_conflict => {
                if self.has_unmerged_entries().await {
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

    async fn has_unmerged_entries(&self) -> bool {
        self.runner
            .run(
                self.command(CommandKind::Read)
                    .args(["ls-files", "--unmerged", "-z"])
                    .foreground_read(),
            )
            .await
            .is_ok_and(|output| !output.stdout.is_empty())
    }
}

fn result_from_outputs(outputs: Vec<GitOutput>) -> MutationResult {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn mutation_invalidates_only_when_execution_acquires_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "--initial-branch=main"])
            .arg(temp.path())
            .status()
            .unwrap();
        assert!(status.success());
        let repository = Arc::new(Repository::discover(temp.path()).await.unwrap());
        let guard = repository.mutation_lock.lock().await;
        let task = {
            let repository = Arc::clone(&repository);
            tokio::spawn(async move { repository.stage_all().await })
        };
        tokio::task::yield_now().await;
        assert_eq!(repository.snapshot_invalidation.load(Ordering::Acquire), 0);

        drop(guard);
        task.await.unwrap().unwrap();
        assert_eq!(repository.snapshot_invalidation.load(Ordering::Acquire), 1);
    }
}
