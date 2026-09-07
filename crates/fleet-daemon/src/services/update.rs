//! Fleet source-checkout update and release-build job.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

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
            .record(&id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))
    }

    #[cfg(test)]
    pub(crate) fn checkout(&self) -> &Path {
        &self.checkout
    }
}

pub(crate) fn runtime_checkout() -> PathBuf {
    let executable = std::env::current_exe().ok();
    select_checkout(
        std::env::var_os("FLEET_INSTALL_ROOT"),
        executable.as_deref(),
    )
}

fn select_checkout(install_root: Option<OsString>, executable: Option<&Path>) -> PathBuf {
    install_root
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .or_else(|| executable.and_then(enclosing_work_tree))
        .or_else(|| executable.and_then(Path::parent).map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn enclosing_work_tree(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapters::shell::ShellResult,
        testing::fakes::{FakeGit, FakeShell, FakeShellCall},
    };
    use fleet_proto::job::JobStatus;

    #[test]
    fn runtime_root_selects_update_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("fleet");
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        let executable = checkout.join("target/release/fleetd");

        assert_eq!(
            select_checkout(Some(OsString::from("/installed/fleet")), Some(&executable)),
            Path::new("/installed/fleet")
        );
        assert_eq!(select_checkout(None, Some(&executable)), checkout);
        assert_eq!(
            select_checkout(None, Some(Path::new("/opt/fleet/bin/fleetd"))),
            Path::new("/opt/fleet/bin")
        );
    }

    #[tokio::test]
    async fn update_reports_verbose_build_success_and_failure() {
        for status in [0, 7] {
            let temp = tempfile::tempdir().unwrap();
            let shell = update_shell(status);
            let jobs = Arc::new(JobManager::new(temp.path()));
            let record = Update::new(
                jobs.clone(),
                Arc::new(FakeGit::new(shell.clone())),
                shell.clone(),
                temp.path(),
            )
            .start()
            .await
            .unwrap();
            assert_eq!(record.kind, JobKind::Update);
            let completed =
                tokio::time::timeout(std::time::Duration::from_secs(5), jobs.wait(&record.id))
                    .await
                    .unwrap()
                    .unwrap();
            if status == 0 {
                assert_eq!(completed.status, JobStatus::Succeeded);
            } else {
                assert!(
                    matches!(completed.status, JobStatus::Failed { error } if error.contains("cargo build --release"))
                );
            }
            assert!(shell.calls().iter().any(|call| matches!(call,
                FakeShellCall::Streaming(command) if command == &ShellCommand::new("cargo").args(["build", "--release"]).cwd(temp.path())
            )));
        }
    }

    #[tokio::test]
    async fn cancelled_update_never_starts_build_commands() {
        let temp = tempfile::tempdir().unwrap();
        let shell = update_shell(0);
        let jobs = Arc::new(JobManager::new(temp.path()));
        let record = Update::new(
            jobs.clone(),
            Arc::new(FakeGit::new(shell.clone())),
            shell.clone(),
            temp.path(),
        )
        .start()
        .await
        .unwrap();
        jobs.cancel(&record.id).unwrap();
        let completed =
            tokio::time::timeout(std::time::Duration::from_secs(5), jobs.wait(&record.id))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(completed.status, JobStatus::Cancelled);
        assert!(shell.calls().iter().all(|call| match call {
            FakeShellCall::Run(command) =>
                command.program == "git"
                    && command.args.first().is_some_and(|argument| matches!(
                        argument.as_str(),
                        "rev-parse" | "branch" | "status"
                    )),
            FakeShellCall::Detached { .. } | FakeShellCall::Streaming(_) => false,
        }));
    }

    fn update_shell(build_status: i32) -> Arc<FakeShell> {
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.args == ["rev-parse", "--is-inside-work-tree"],
            ShellResult {
                status: 0,
                stdout: "true\n".into(),
                stderr: String::new(),
            },
        );
        shell.when(
            |command| command.args == ["branch", "--show-current"],
            ShellResult {
                status: 0,
                stdout: "main\n".into(),
                stderr: String::new(),
            },
        );
        shell.when(
            |command| command.program == "cargo",
            ShellResult {
                status: build_status,
                stdout: "compiling dependency\n".repeat(1000),
                stderr: "build diagnostic\n".repeat(1000),
            },
        );
        shell.when(
            |_| true,
            ShellResult {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        shell
    }
}
