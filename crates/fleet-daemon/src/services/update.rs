//! Fleet source-checkout update and release-build job.

use std::{path::PathBuf, sync::Arc};

use fleet_proto::job::{JobKind, JobRecord};

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        git::Git,
        shell::{Shell, ShellCommand},
    },
    jobs::JobManager,
};

/// Self-update service for a specific Fleet source checkout.
#[derive(Clone)]
pub struct Update {
    jobs: Arc<JobManager>,
    git: Arc<dyn Git>,
    shell: Arc<dyn Shell>,
    checkout: PathBuf,
}

impl Update {
    /// Creates an updater rooted at `checkout`.
    #[must_use]
    pub fn new(
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        shell: Arc<dyn Shell>,
        checkout: impl Into<PathBuf>,
    ) -> Self {
        Self {
            jobs,
            git,
            shell,
            checkout: checkout.into(),
        }
    }

    /// Starts `git pull --ff-only origin main` followed by `cargo build --release`.
    pub async fn start(&self) -> DaemonResult<JobRecord> {
        let git = Arc::clone(&self.git);
        let shell = Arc::clone(&self.shell);
        let checkout = self.checkout.clone();
        let id = self.jobs.submit(
            JobKind::Update,
            checkout.display().to_string(),
            "Update Fleet",
            true,
            true,
            move |context| async move {
                context.progress("checking Fleet checkout")?;
                if !git.is_inside_work_tree(&checkout).await? {
                    return Err(DaemonError::Validation(format!(
                        "not a Git work tree: {}",
                        checkout.display()
                    )));
                }
                let branch = git.current_branch(&checkout).await?;
                if branch != "main" {
                    return Err(DaemonError::Conflict(format!(
                        "Fleet update requires branch main, found {branch}"
                    )));
                }
                let status = git.update_status(&checkout).await?;
                if !status.trim().is_empty() {
                    return Err(DaemonError::Conflict(
                        "Fleet update requires a clean checkout".to_owned(),
                    ));
                }
                if context.cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }

                context.progress("pulling Fleet with fast-forward only")?;
                git.pull_main(&checkout).await?;
                if context.cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }

                context.progress("building Fleet release binaries")?;
                let result = context
                    .spawn_child(
                        shell,
                        ShellCommand::new("cargo")
                            .args(["build", "--release"])
                            .cwd(&checkout),
                    )
                    .await?;
                result.require_success("cargo build --release")?;
                context.progress("Fleet update built successfully")?;
                Ok(())
            },
        );
        self.jobs
            .list()
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        adapters::shell::{ShellCommand, ShellResult},
        testing::fakes::{FakeGit, FakeShell, FakeShellCall},
    };

    use super::Update;

    #[tokio::test]
    async fn submits_update_as_a_job() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.args == ["rev-parse", "--is-inside-work-tree"],
            ShellResult {
                status: 0,
                stdout: "true\n".to_owned(),
                stderr: String::new(),
            },
        );
        shell.when(
            |command| command.args == ["branch", "--show-current"],
            ShellResult {
                status: 0,
                stdout: "main\n".to_owned(),
                stderr: String::new(),
            },
        );
        shell.when(
            |_command| true,
            ShellResult {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let git = Arc::new(FakeGit::new(Arc::clone(&shell)));
        let jobs = Arc::new(crate::jobs::JobManager::new(temp.path()));
        let record = Update::new(jobs, git, shell.clone(), temp.path())
            .start()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(record.kind, fleet_proto::job::JobKind::Update);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(shell.calls().iter().any(|call| {
            matches!(
                call,
                FakeShellCall::Streaming(command)
                    if command == &ShellCommand::new("cargo")
                        .args(["build", "--release"])
                        .cwd(temp.path())
            )
        }));
    }
}
