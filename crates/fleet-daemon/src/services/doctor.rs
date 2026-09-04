//! Environment and installation diagnostics.

use std::{path::PathBuf, sync::Arc, time::Duration};

use fleet_proto::{job::JobRecord, response::DoctorCheck};

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        files::Files,
        git::Git,
        github::Github,
        shell::{Shell, ShellCommand},
    },
    jobs::JobManager,
    services::update::Update,
    stores::config::ConfigStore,
};

const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_UNSUPPORTED: &str = "remote hosts are not supported yet";

/// Environment diagnostics and self-update service.
#[derive(Clone)]
pub struct Doctor {
    jobs: Arc<JobManager>,
    config: Arc<ConfigStore>,
    shell: Arc<dyn Shell>,
    git: Arc<dyn Git>,
    github: Arc<dyn Github>,
    files: Arc<dyn Files>,
}

impl Doctor {
    /// Creates the diagnostics service.
    #[must_use]
    pub fn new(
        jobs: Arc<JobManager>,
        config: Arc<ConfigStore>,
        shell: Arc<dyn Shell>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            jobs,
            config,
            shell,
            git,
            github,
            files,
        }
    }

    /// Checks Fleet's external dependencies and local runtime environment in parallel.
    pub async fn check(&self) -> DaemonResult<Vec<DoctorCheck>> {
        let config = self.config.load().await?;
        let home = self
            .config
            .path()
            .parent()
            .ok_or_else(|| DaemonError::Validation("FLEET_HOME has no parent".to_owned()))?
            .to_path_buf();

        let git = check_git(Arc::clone(&self.shell));
        let github = check_github(Arc::clone(&self.github));
        let copy = check_copy(
            Arc::clone(&self.files),
            PathBuf::from(&config.worktrees_dir),
        );
        let writable = check_writable(Arc::clone(&self.files), home.clone());
        let (git, github, copy, writable) = tokio::join!(git, github, copy, writable);

        let mut checks = vec![
            git,
            github,
            copy,
            DoctorCheck {
                check: "runtime".to_owned(),
                ok: true,
                detail: "Zig is required only at build time".to_owned(),
            },
            DoctorCheck {
                check: "daemon socket".to_owned(),
                ok: self.files.exists(&home.join("fleetd.sock")),
                detail: if self.files.exists(&home.join("fleetd.sock")) {
                    home.join("fleetd.sock").display().to_string()
                } else {
                    format!("missing {}", home.join("fleetd.sock").display())
                },
            },
            writable,
        ];
        checks.extend(config.hosts.keys().map(|host| DoctorCheck {
            check: format!("host {host}"),
            ok: false,
            detail: REMOTE_UNSUPPORTED.to_owned(),
        }));
        Ok(checks)
    }

    /// Starts the source update and release-build workflow as a detached job.
    pub async fn update(&self) -> DaemonResult<JobRecord> {
        Update::new(
            Arc::clone(&self.jobs),
            Arc::clone(&self.git),
            Arc::clone(&self.shell),
            checkout_root(),
        )
        .start()
        .await
    }
}

async fn check_git(shell: Arc<dyn Shell>) -> DoctorCheck {
    let command = ShellCommand::new("git")
        .arg("--version")
        .timeout(CHECK_TIMEOUT);
    match shell.run(command).await {
        Ok(result) if result.success() => DoctorCheck {
            check: "git".to_owned(),
            ok: true,
            detail: result.stdout.trim().to_owned(),
        },
        Ok(result) => DoctorCheck {
            check: "git".to_owned(),
            ok: false,
            detail: command_failure(result.status, &result.stderr, &result.stdout),
        },
        Err(error) => failed_check("git", error),
    }
}

async fn check_github(github: Arc<dyn Github>) -> DoctorCheck {
    match tokio::time::timeout(CHECK_TIMEOUT, github.auth_status()).await {
        Ok(Ok(())) => DoctorCheck {
            check: "gh auth".to_owned(),
            ok: true,
            detail: "authenticated".to_owned(),
        },
        Ok(Err(error)) => failed_check("gh auth", error),
        Err(_) => DoctorCheck {
            check: "gh auth".to_owned(),
            ok: false,
            detail: "timed out after 5 seconds".to_owned(),
        },
    }
}

async fn check_copy(files: Arc<dyn Files>, worktrees_dir: PathBuf) -> DoctorCheck {
    tokio::task::spawn_blocking(move || {
        let suffix = format!(".doctor-copy-{}", uuid::Uuid::new_v4());
        let source = worktrees_dir.join(format!("{suffix}-source"));
        let destination = worktrees_dir.join(format!("{suffix}-destination"));
        let probe = source.join("probe");
        let result = (|| {
            files.create_dir_all(&source)?;
            files.atomic_write_text(&probe, "fleet doctor\n")?;
            files.clone_dir(&source, &destination)?;
            let copied = files.read_text(&destination.join("probe"))?;
            if copied != "fleet doctor\n" {
                return Err(DaemonError::Validation(
                    "copy-on-write probe content differed".to_owned(),
                ));
            }
            Ok(())
        })();
        let _source_cleanup = files.remove_detached(&source);
        let _destination_cleanup = files.remove_detached(&destination);
        match result {
            Ok(()) => DoctorCheck {
                check: "copy-on-write".to_owned(),
                ok: true,
                detail: copy_detail().to_owned(),
            },
            Err(error) => failed_check("copy-on-write", error),
        }
    })
    .await
    .unwrap_or_else(|error| DoctorCheck {
        check: "copy-on-write".to_owned(),
        ok: false,
        detail: format!("background check failed: {error}"),
    })
}

async fn check_writable(files: Arc<dyn Files>, home: PathBuf) -> DoctorCheck {
    tokio::task::spawn_blocking(move || {
        let probe = home.join(format!(".doctor-write-{}", uuid::Uuid::new_v4()));
        let result = (|| {
            files.create_dir_all(&home)?;
            files.atomic_write_text(&probe, "writable\n")?;
            files.remove_file(&probe)
        })();
        match result {
            Ok(()) => DoctorCheck {
                check: "FLEET_HOME writable".to_owned(),
                ok: true,
                detail: home.display().to_string(),
            },
            Err(error) => failed_check("FLEET_HOME writable", error),
        }
    })
    .await
    .unwrap_or_else(|error| DoctorCheck {
        check: "FLEET_HOME writable".to_owned(),
        ok: false,
        detail: format!("background check failed: {error}"),
    })
}

fn command_failure(status: i32, stderr: &str, stdout: &str) -> String {
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    format!("exit {status}: {detail}")
}

fn failed_check(check: &str, error: DaemonError) -> DoctorCheck {
    DoctorCheck {
        check: check.to_owned(),
        ok: false,
        detail: error.to_string(),
    }
}

#[cfg(target_os = "macos")]
const fn copy_detail() -> &'static str {
    "clonefile/cp -c available"
}

#[cfg(target_os = "linux")]
const fn copy_detail() -> &'static str {
    "cp --reflink=auto available"
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const fn copy_detail() -> &'static str {
    "recursive copy available"
}

fn checkout_root() -> PathBuf {
    std::env::var_os("FLEET_INSTALL_ROOT").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    )
}

#[cfg(test)]
mod tests {
    use super::command_failure;

    #[test]
    fn command_failure_prefers_stderr() {
        assert_eq!(command_failure(2, "bad\n", "ignored\n"), "exit 2: bad");
        assert_eq!(command_failure(1, "", "usage\n"), "exit 1: usage");
    }
}
