//! Environment and installation diagnostics.

use std::{path::PathBuf, sync::Arc, time::Duration};

use fleet_core::{config::Config, ids::HostId, model::HostConfigEntry};
use fleet_proto::{
    PROTOCOL_VERSION,
    response::{DoctorCheck, DoctorStatus},
    snapshot::LinkState,
};

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        files::Files,
        github::Github,
        shell::{Shell, ShellCommand},
    },
    machines::{MachineProvider, Machines, RemoteEndpoint},
    services::{
        Services,
        hosts::{HostDiagnostics, Hosts},
    },
    stores::config::ConfigStore,
};

const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Environment diagnostics service.
#[derive(Clone)]
pub struct Doctor {
    config: Arc<ConfigStore>,
    shell: Arc<dyn Shell>,
    github: Arc<dyn Github>,
    files: Arc<dyn Files>,
}

impl Doctor {
    /// Creates the diagnostics service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        shell: Arc<dyn Shell>,
        github: Arc<dyn Github>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            config,
            shell,
            github,
            files,
        }
    }

    /// Checks Fleet's external dependencies and local runtime environment in parallel.
    pub async fn check(&self) -> DaemonResult<Vec<DoctorCheck>> {
        let config = self.config.load().await?;
        let machines = Machines::from_config(&config);
        self.check_config(config, &machines).await
    }

    /// Runs diagnostics against the daemon's live machine and endpoint registry.
    pub async fn check_with_machines(&self, machines: &Machines) -> DaemonResult<Vec<DoctorCheck>> {
        let config = self.config.load().await?;
        self.check_config(config, machines).await
    }

    /// Runs only the checks belonging to one configured host.
    pub async fn check_host(
        &self,
        host: &HostId,
        machines: &Machines,
    ) -> DaemonResult<Vec<DoctorCheck>> {
        let config = self.config.load().await?;
        let entry = config
            .hosts
            .get(host)
            .ok_or_else(|| DaemonError::NotFound(format!("configured host `{host}`")))?;
        let home = self
            .config
            .path()
            .parent()
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let hosts = Hosts::new(home, Arc::clone(&self.shell));
        let diagnostics = hosts.diagnose_configured(host, entry, machines).await;
        Ok(host_checks(
            diagnostics,
            matches!(entry, HostConfigEntry::Legacy { .. }),
        ))
    }

    async fn check_config(
        &self,
        config: Config,
        machines: &Machines,
    ) -> DaemonResult<Vec<DoctorCheck>> {
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
                status: DoctorStatus::Ok,
                detail: "Zig is required only at build time".to_owned(),
            },
            DoctorCheck {
                check: "daemon socket".to_owned(),
                status: if self.files.exists(&home.join("fleetd.sock")) {
                    DoctorStatus::Ok
                } else {
                    DoctorStatus::Warn
                },
                detail: if self.files.exists(&home.join("fleetd.sock")) {
                    home.join("fleetd.sock").display().to_string()
                } else {
                    format!("missing {}", home.join("fleetd.sock").display())
                },
            },
            writable,
        ];
        let hosts = Hosts::new(home, Arc::clone(&self.shell));
        checks.extend(
            futures_util::future::join_all(config.hosts.iter().map(|(id, entry)| {
                let hosts = &hosts;
                async move {
                    let diagnostics = hosts.diagnose_configured(id, entry, machines).await;
                    host_checks(diagnostics, matches!(entry, HostConfigEntry::Legacy { .. }))
                }
            }))
            .await
            .into_iter()
            .flatten(),
        );
        Ok(checks)
    }

    /// Renders Doctor lines for an injected machine and endpoint.
    pub async fn check_machine(
        &self,
        provider: Arc<dyn MachineProvider>,
        endpoint: Option<Arc<dyn RemoteEndpoint>>,
    ) -> Vec<DoctorCheck> {
        let home = self
            .config
            .path()
            .parent()
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let hosts = Hosts::new(home, Arc::clone(&self.shell));
        host_checks(hosts.diagnose_machine(provider, endpoint).await, false)
    }
}

fn host_checks(diagnostics: HostDiagnostics, legacy: bool) -> Vec<DoctorCheck> {
    let HostDiagnostics {
        status,
        resolve_error,
        stderr,
        hello,
    } = diagnostics;
    let prefix = format!("host {}", status.id);
    let local_version = Services::version();
    let summary_detail = if legacy {
        status.error.clone().unwrap_or_else(|| {
            format!(
                "{} · {}",
                status.address.as_deref().unwrap_or("address unknown"),
                status
                    .version
                    .as_deref()
                    .unwrap_or("swarm (version unknown)")
            )
        })
    } else {
        format!(
            "provider {} · address {} · version {} · link {}",
            status.provider,
            status.address.as_deref().unwrap_or("unknown"),
            status.version.as_deref().unwrap_or("unknown"),
            link_name(status.link)
        )
    };
    let mut checks = vec![
        DoctorCheck {
            check: prefix.clone(),
            status: if status.reachable {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Fail
            },
            detail: summary_detail,
        },
        DoctorCheck {
            check: format!("{prefix} provider"),
            status: DoctorStatus::Ok,
            detail: status.provider.clone(),
        },
        DoctorCheck {
            check: format!("{prefix} address"),
            status: if status.address.is_some() {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Fail
            },
            detail: status
                .address
                .clone()
                .or(resolve_error)
                .unwrap_or_else(|| "unresolved".to_owned()),
        },
        DoctorCheck {
            check: format!("{prefix} ssh/probe"),
            status: if status.reachable {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Fail
            },
            detail: if status.reachable {
                "reachable".to_owned()
            } else {
                stderr
                    .filter(|value| !value.is_empty())
                    .or_else(|| status.error.clone())
                    .unwrap_or_else(|| "unreachable".to_owned())
            },
        },
        DoctorCheck {
            check: format!("{prefix} fleetd version"),
            status: match status.version.as_deref() {
                Some(version) if version == local_version => DoctorStatus::Ok,
                Some(_) => DoctorStatus::Warn,
                None => DoctorStatus::Fail,
            },
            detail: status.version.as_ref().map_or_else(
                || format!("remote unknown · local {local_version}"),
                |version| format!("remote {version} · local {local_version}"),
            ),
        },
        DoctorCheck {
            check: format!("{prefix} link"),
            status: match status.link {
                LinkState::Ready => DoctorStatus::Ok,
                LinkState::Connecting | LinkState::Legacy => DoctorStatus::Warn,
                LinkState::Down => DoctorStatus::Fail,
            },
            detail: link_name(status.link).to_owned(),
        },
        DoctorCheck {
            check: format!("{prefix} protocol"),
            status: if status.link == LinkState::Ready && hello.is_some() {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Warn
            },
            detail: if status.link == LinkState::Ready && hello.is_some() {
                format!("remote protocol {PROTOCOL_VERSION} matches local {PROTOCOL_VERSION}")
            } else if legacy {
                "legacy swarm protocol 1; Fleet protocol not negotiated".to_owned()
            } else {
                format!("Fleet protocol {PROTOCOL_VERSION} not negotiated")
            },
        },
    ];
    if !legacy {
        let binaries = status.agent_binaries.as_ref();
        checks.extend([
            agent_binary_check(&prefix, "claude", binaries.map(|value| value.claude)),
            agent_binary_check(&prefix, "opencode", binaries.map(|value| value.opencode)),
        ]);
    }
    if legacy {
        checks.push(DoctorCheck {
            check: format!("{prefix} migration"),
            status: DoctorStatus::Warn,
            detail: concat!(
                "migrate this legacy {ssh, swarmCommand} entry to the new schema: ",
                r#"{"provider":"tailscale","node":"<node>","fleetd":"fleetd","fleetHome":"~/.fleet"}"#
            )
            .to_owned(),
        });
    }
    checks
}

fn agent_binary_check(prefix: &str, binary: &str, available: Option<bool>) -> DoctorCheck {
    let (status, detail) = match available {
        Some(true) => (DoctorStatus::Ok, "available through login shell"),
        Some(false) => (DoctorStatus::Fail, "not found through login shell"),
        None => (DoctorStatus::Warn, "login-shell availability check failed"),
    };
    DoctorCheck {
        check: format!("{prefix} {binary}"),
        status,
        detail: detail.to_owned(),
    }
}

const fn link_name(link: LinkState) -> &'static str {
    match link {
        LinkState::Connecting => "connecting",
        LinkState::Ready => "ready",
        LinkState::Down => "down",
        LinkState::Legacy => "legacy",
    }
}

async fn check_git(shell: Arc<dyn Shell>) -> DoctorCheck {
    let command = ShellCommand::new("git")
        .arg("--version")
        .timeout(CHECK_TIMEOUT);
    match shell.run(command).await {
        Ok(result) if result.success() => DoctorCheck {
            check: "git".to_owned(),
            status: DoctorStatus::Ok,
            detail: result.stdout.trim().to_owned(),
        },
        Ok(result) => DoctorCheck {
            check: "git".to_owned(),
            status: DoctorStatus::Fail,
            detail: command_failure(result.status, &result.stderr, &result.stdout),
        },
        Err(error) => failed_check("git", error),
    }
}

async fn check_github(github: Arc<dyn Github>) -> DoctorCheck {
    match tokio::time::timeout(CHECK_TIMEOUT, github.auth_status()).await {
        Ok(Ok(())) => DoctorCheck {
            check: "gh auth".to_owned(),
            status: DoctorStatus::Ok,
            detail: "authenticated".to_owned(),
        },
        Ok(Err(error)) => failed_check("gh auth", error),
        Err(_) => DoctorCheck {
            check: "gh auth".to_owned(),
            status: DoctorStatus::Fail,
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
                status: DoctorStatus::Ok,
                detail: copy_detail().to_owned(),
            },
            Err(error) => failed_check("copy-on-write", error),
        }
    })
    .await
    .unwrap_or_else(|error| DoctorCheck {
        check: "copy-on-write".to_owned(),
        status: DoctorStatus::Fail,
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
                status: DoctorStatus::Ok,
                detail: home.display().to_string(),
            },
            Err(error) => failed_check("FLEET_HOME writable", error),
        }
    })
    .await
    .unwrap_or_else(|error| DoctorCheck {
        check: "FLEET_HOME writable".to_owned(),
        status: DoctorStatus::Fail,
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
        status: DoctorStatus::Fail,
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

#[cfg(test)]
mod tests {
    use super::command_failure;

    #[test]
    fn command_failure_prefers_stderr() {
        assert_eq!(command_failure(2, "bad\n", "ignored\n"), "exit 2: bad");
        assert_eq!(command_failure(1, "", "usage\n"), "exit 1: usage");
    }
}
