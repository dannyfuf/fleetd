//! Environment and installation diagnostics.

use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};

use fleet_core::{config::Config, ids::HostId, model::HostConfigEntry, paths::FleetHome};
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
        let mut checks = host_checks(
            diagnostics.clone(),
            matches!(entry, HostConfigEntry::Legacy { .. }),
        );
        checks.extend(ssh_identity_check(self.shell.as_ref(), entry, &diagnostics).await);
        Ok(checks)
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
        let holders = check_pty_holders(home.clone());
        let (git, github, copy, writable, holders) =
            tokio::join!(git, github, copy, writable, holders);

        let mut checks = vec![
            git,
            github,
            copy,
            holders,
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
                let shell = self.shell.as_ref();
                async move {
                    let diagnostics = hosts.diagnose_configured(id, entry, machines).await;
                    let mut checks = host_checks(
                        diagnostics.clone(),
                        matches!(entry, HostConfigEntry::Legacy { .. }),
                    );
                    checks.extend(ssh_identity_check(shell, entry, &diagnostics).await);
                    checks
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

/// Existing identities OpenSSH may offer before pinning one key is worth recommending.
///
/// sshd counts every offered key against `LoginGraceTime` and penalises the source address
/// that exceeds it — the failure mode that locked a Mac out of its devbox for minutes at a
/// time. Only keys that exist are ever offered, so this is compared against the candidates
/// that survive a stat, never against the raw `ssh -G` list: stock OpenSSH names four to
/// seven default identity files whether or not a single one of them is on disk.
const MAX_OFFERED_IDENTITIES: usize = 3;

/// Checks how this host authenticates: the configured key when there is one, and otherwise
/// how many identities OpenSSH would walk before finding the right one.
///
/// Returns nothing only when the question cannot be answered — a non-Tailscale host, an
/// unresolved address, or an `ssh -G` that fails. A diagnostic that cannot be computed is
/// not a finding.
async fn ssh_identity_check(
    shell: &dyn Shell,
    entry: &HostConfigEntry,
    diagnostics: &HostDiagnostics,
) -> Option<DoctorCheck> {
    let HostConfigEntry::Tailscale {
        user,
        ssh_options,
        identity_file,
        ssh_host,
        ..
    } = entry
    else {
        return None;
    };
    let check = format!("host {} ssh identity", diagnostics.status.id);
    if let Some(identity_file) = identity_file {
        return Some(configured_identity_check(check, identity_file).await);
    }
    let host = ssh_host
        .clone()
        .or_else(|| diagnostics.status.address.clone())?;
    let destination = user
        .as_ref()
        .map_or_else(|| host.clone(), |user| format!("{user}@{host}"));
    let command = ShellCommand::new("ssh")
        .arg("-G")
        .args(ssh_options.clone())
        .args(crate::machines::ssh::connection_defaults())
        .args(["--".to_owned(), destination])
        .timeout(CHECK_TIMEOUT);
    let result = shell.run(command).await.ok()?;
    if !result.success() {
        return None;
    }
    let offered = existing_identity_count(&result.stdout).await;
    (offered > MAX_OFFERED_IDENTITIES).then(|| DoctorCheck {
        check,
        status: DoctorStatus::Warn,
        detail: format!(
            "{offered} private keys exist and are offered before authentication; set this \
             host's identityFile to the one key it uses so sshd's login grace period is not \
             spent on the others"
        ),
    })
}

/// Counts the `ssh -G` identity candidates that are actually present on disk.
///
/// OpenSSH names every default identity path unconditionally, so the raw line count says
/// nothing about how many keys will really be offered; only the ones that exist are.
async fn existing_identity_count(ssh_config: &str) -> usize {
    let candidates = ssh_config
        .lines()
        .filter_map(|line| line.strip_prefix("identityfile "))
        .map(|path| fleet_core::paths::expand_tilde(path.trim()))
        .collect::<Vec<_>>();
    let mut existing = 0;
    for candidate in candidates {
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            existing += 1;
        }
    }
    existing
}

/// Verifies a configured `identityFile` is a key OpenSSH will actually use.
///
/// This is the one place a typo is catchable: `identityFile` also turns on
/// `IdentitiesOnly=yes`, so a path that does not resolve leaves `ssh` with no identity to
/// offer at all and every connection to the host fails authentication. `fleet-core` is
/// I/O-free by contract (ADR 0008) and its config validation stats nothing, so the check
/// belongs here rather than at config load.
async fn configured_identity_check(check: String, identity_file: &str) -> DoctorCheck {
    let path = fleet_core::paths::expand_tilde(identity_file);
    let shown = path.display().to_string();
    let failure = |detail: String| DoctorCheck {
        check: check.clone(),
        status: DoctorStatus::Fail,
        detail,
    };
    let Ok(metadata) = tokio::fs::metadata(&path).await else {
        return failure(format!(
            "identityFile {shown} does not exist; with IdentitiesOnly=yes this leaves ssh no \
             identity to offer and every connection to this host fails authentication"
        ));
    };
    if !metadata.is_file() {
        return failure(format!("identityFile {shown} is not a regular file"));
    }
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0 {
        return failure(format!(
            "identityFile {shown} is accessible to group or others (mode {:04o}); ssh refuses \
             such a key — chmod 600 it",
            mode & 0o7777
        ));
    }
    DoctorCheck {
        check,
        status: DoctorStatus::Ok,
        detail: format!("pinned to {shown}"),
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
            agent_binary_check(&prefix, "codex", binaries.map(|value| value.codex)),
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

/// Reports the detached PTY holders no daemon will ever adopt.
///
/// A holder outlives every daemon by design, which is exactly why an unreachable one is invisible:
/// nothing lists it, nothing cleans it up, and the login shell behind it runs until the machine
/// reboots. Two states qualify. A record whose holder process is gone is a leftover the next start
/// would clear anyway. A socket with no readable record is the dangerous one — a live shell with
/// nothing left pointing at it — so this is the surface that names it.
async fn check_pty_holders(home: PathBuf) -> DoctorCheck {
    use crate::services::sessions::holder;

    let layout = FleetHome::new(home);
    let records = match holder::discover_sidecars(&layout).await {
        Ok(records) => records,
        Err(error) => return failed_check("pty holders", error),
    };
    let claimed = records
        .iter()
        .filter_map(|entry| entry.sidecar.as_ref())
        .map(|sidecar| sidecar.socket.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let stale = records
        .iter()
        .filter(|entry| {
            entry
                .sidecar
                .as_ref()
                .is_none_or(|sidecar| !holder::holder_is_alive(sidecar))
        })
        .count();
    let orphaned = holder::runtime_files(&layout)
        .await
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sock")
                && !claimed.contains(path)
        })
        .collect::<Vec<_>>();

    if stale == 0 && orphaned.is_empty() {
        let live = records.len().saturating_sub(stale);
        return DoctorCheck {
            check: "pty holders".to_owned(),
            status: DoctorStatus::Ok,
            detail: format!("{live} terminal(s) held"),
        };
    }
    let mut detail = String::new();
    if stale > 0 {
        detail.push_str(&format!(
            "{stale} record(s) name a holder that is gone; the next daemon start removes them"
        ));
    }
    if let Some(first) = orphaned.first() {
        if !detail.is_empty() {
            detail.push_str("; ");
        }
        detail.push_str(&format!(
            "{} holder socket(s) have no record and will never be reattached, starting with {} — \
             stop the shell behind it or delete the socket",
            orphaned.len(),
            first.display()
        ));
    }
    DoctorCheck {
        check: "pty holders".to_owned(),
        status: DoctorStatus::Warn,
        detail,
    }
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
    use super::{
        DoctorStatus, command_failure, configured_identity_check, existing_identity_count,
    };
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn command_failure_prefers_stderr() {
        assert_eq!(command_failure(2, "bad\n", "ignored\n"), "exit 2: bad");
        assert_eq!(command_failure(1, "", "usage\n"), "exit 1: usage");
    }

    /// Stock OpenSSH names four to seven default identity files whether or not any of them
    /// exists, so counting the lines would warn at every healthy machine.
    #[tokio::test]
    async fn only_identity_candidates_that_exist_are_counted() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let present = temp.path().join("id_ed25519");
        std::fs::write(&present, b"key").unwrap_or_else(|error| panic!("{error}"));
        let ssh_config = format!(
            "user df\nidentityfile {}\nidentityfile {}\nidentityfile {}\nhostname 100.64.0.7\n",
            present.display(),
            temp.path().join("id_rsa").display(),
            temp.path().join("id_dsa").display(),
        );

        assert_eq!(existing_identity_count(&ssh_config).await, 1);
        assert_eq!(existing_identity_count("user df\nhostname h\n").await, 0);
    }

    #[tokio::test]
    async fn a_configured_identity_is_verified_rather_than_trusted() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let key = temp.path().join("id_ed25519");
        std::fs::write(&key, b"key").unwrap_or_else(|error| panic!("{error}"));
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| panic!("{error}"));
        async fn check(path: &std::path::Path) -> super::DoctorCheck {
            configured_identity_check(
                "host dev-box ssh identity".to_owned(),
                &path.to_string_lossy(),
            )
            .await
        }

        assert_eq!(check(&key).await.status, DoctorStatus::Ok);

        // A typo'd path plus IdentitiesOnly=yes leaves ssh nothing to offer, so this must be
        // loud rather than skipped.
        assert_eq!(
            check(&temp.path().join("absent")).await.status,
            DoctorStatus::Fail
        );
        assert_eq!(check(temp.path()).await.status, DoctorStatus::Fail);

        // ssh refuses a key other users can read.
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|error| panic!("{error}"));
        let loose = check(&key).await;
        assert_eq!(loose.status, DoctorStatus::Fail);
        assert!(loose.detail.contains("0644"), "{}", loose.detail);
    }
}
