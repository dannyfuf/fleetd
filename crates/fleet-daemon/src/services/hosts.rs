//! Read-only remote swarm reachability probes and cached snapshot statuses.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use fleet_core::{config::Config, ids::HostId, model::HostConfigEntry, paths::FleetHome};
use fleet_proto::snapshot::{AgentBinaries, HostStatus, LinkState};
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{
    DaemonError,
    adapters::shell::{Shell, ShellCommand},
    machines::{MachineProvider, Machines, ProbeReport, RemoteEndpoint, RemoteHello},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const AGENT_BINARY_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_BINARY_SCRIPT: &str = concat!(
    "if command -v claude >/dev/null 2>&1; then printf 'claude\\n'; fi; ",
    "if command -v opencode >/dev/null 2>&1; then printf 'opencode\\n'; fi",
);

/// Probes configured hosts without importing remote inventory or operating on it.
#[derive(Clone)]
pub struct Hosts {
    shell: Arc<dyn Shell>,
    ssh_cache_dir: PathBuf,
    statuses: Arc<RwLock<BTreeMap<HostId, HostStatus>>>,
}

#[derive(Deserialize)]
struct RemoteReply {
    #[serde(default)]
    protocol: serde_json::Value,
    #[serde(default)]
    version: serde_json::Value,
}

/// Full provider observation used by host status and Doctor rendering.
#[derive(Clone)]
pub struct HostDiagnostics {
    pub status: HostStatus,
    pub resolve_error: Option<String>,
    pub stderr: Option<String>,
    pub hello: Option<RemoteHello>,
}

impl Hosts {
    /// Creates an empty cache using Fleet's SSH control socket directory.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, shell: Arc<dyn Shell>) -> Self {
        Self {
            shell,
            ssh_cache_dir: FleetHome::new(home).ssh_cache_dir(),
            statuses: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Returns cached results, pruning removed hosts and marking unprobed hosts pending.
    pub(super) async fn snapshot(&self, config: &Config, generated_at: &str) -> Vec<HostStatus> {
        let mut statuses = self.statuses.write().await;
        statuses.retain(|id, _| config.hosts.contains_key(id));
        config
            .hosts
            .keys()
            .map(|id| {
                statuses.get(id).cloned().unwrap_or_else(|| HostStatus {
                    id: id.clone(),
                    provider: provider_name(config.hosts.get(id)),
                    version: None,
                    link: link_state(config.hosts.get(id)),
                    address: None,
                    agent_binaries: None,
                    reachable: false,
                    checked_at: generated_at.to_owned(),
                    error: Some("probe pending".to_owned()),
                })
            })
            .collect()
    }

    /// Probes hosts concurrently and replaces the cache with exactly the configured hosts.
    pub async fn probe_all(
        &self,
        config: &Config,
        machines: &Machines,
    ) -> BTreeMap<HostId, HostStatus> {
        self.collect(config, machines, false).await
    }

    /// Refreshes host status while treating a Ready link as the host heartbeat.
    pub async fn refresh_all(
        &self,
        config: &Config,
        machines: &Machines,
    ) -> BTreeMap<HostId, HostStatus> {
        self.collect(config, machines, true).await
    }

    async fn collect(
        &self,
        config: &Config,
        machines: &Machines,
        skip_ready: bool,
    ) -> BTreeMap<HostId, HostStatus> {
        self.statuses
            .write()
            .await
            .retain(|id, _| config.hosts.contains_key(id));
        let cached = self.statuses.read().await.clone();
        let endpoints = machines.endpoints().into_iter().collect::<BTreeMap<_, _>>();
        let results = join_all(config.hosts.iter().map(|(id, entry)| {
            let provider = machines.get(id);
            let endpoint = endpoints.get(id).cloned();
            let cached = cached.get(id).cloned();
            async move {
                let diagnostics = if matches!(entry, HostConfigEntry::Legacy { .. }) {
                    self.diagnose_legacy(id, entry).await
                } else if let Some(provider) = provider {
                    if skip_ready
                        && endpoint
                            .as_ref()
                            .is_some_and(|endpoint| endpoint.state() == LinkState::Ready)
                    {
                        ready_diagnostics(provider, endpoint, cached).await
                    } else {
                        diagnose_provider(provider, endpoint).await
                    }
                } else {
                    unavailable_diagnostics(id, entry)
                };
                (id.clone(), diagnostics.status)
            }
        }))
        .await
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        *self.statuses.write().await = results.clone();
        results
    }

    /// Probes an injected provider, primarily for provider-neutral service callers and tests.
    pub async fn probe_machine(
        &self,
        provider: Arc<dyn MachineProvider>,
        endpoint: Option<Arc<dyn RemoteEndpoint>>,
    ) -> HostStatus {
        diagnose_provider(provider, endpoint).await.status
    }

    /// Refreshes one injected provider, skipping its probe when its link is Ready.
    pub async fn refresh_machine(
        &self,
        provider: Arc<dyn MachineProvider>,
        endpoint: Option<Arc<dyn RemoteEndpoint>>,
    ) -> HostStatus {
        if endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.state() == LinkState::Ready)
        {
            ready_diagnostics(provider, endpoint, None).await.status
        } else {
            diagnose_provider(provider, endpoint).await.status
        }
    }

    /// Returns all details needed to render one provider's Doctor checks.
    pub async fn diagnose_machine(
        &self,
        provider: Arc<dyn MachineProvider>,
        endpoint: Option<Arc<dyn RemoteEndpoint>>,
    ) -> HostDiagnostics {
        diagnose_provider(provider, endpoint).await
    }

    pub(super) async fn diagnose_configured(
        &self,
        id: &HostId,
        entry: &HostConfigEntry,
        machines: &Machines,
    ) -> HostDiagnostics {
        if matches!(entry, HostConfigEntry::Legacy { .. }) {
            return self.diagnose_legacy(id, entry).await;
        }
        let Some(provider) = machines.get(id) else {
            return unavailable_diagnostics(id, entry);
        };
        let endpoint = machines
            .endpoints()
            .into_iter()
            .find_map(|(host, endpoint)| (host == *id).then_some(endpoint));
        diagnose_provider(provider, endpoint).await
    }

    async fn diagnose_legacy(&self, id: &HostId, entry: &HostConfigEntry) -> HostDiagnostics {
        let HostConfigEntry::Legacy { ssh, swarm_command } = entry else {
            return unavailable_diagnostics(id, entry);
        };
        let result = self.remote_version(ssh, swarm_command).await;
        let (error, version, stderr) = match result {
            Ok(version) => (None, version, None),
            Err(failure) => (Some(failure.error), None, failure.stderr),
        };
        HostDiagnostics {
            status: HostStatus {
                id: id.clone(),
                provider: "legacy".to_owned(),
                version,
                link: LinkState::Legacy,
                address: Some(ssh.clone()),
                agent_binaries: None,
                reachable: error.is_none(),
                checked_at: chrono::Utc::now().to_rfc3339(),
                error,
            },
            resolve_error: None,
            stderr,
            hello: None,
        }
    }

    async fn remote_version(
        &self,
        ssh: &str,
        swarm_command: &str,
    ) -> Result<Option<String>, LegacyFailure> {
        let mut directory = tokio::fs::DirBuilder::new();
        directory.recursive(true).mode(0o700);
        directory
            .create(&self.ssh_cache_dir)
            .await
            .map_err(|error| LegacyFailure {
                error: format!("{}: {error}", self.ssh_cache_dir.display()),
                stderr: None,
            })?;
        let command = ShellCommand::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPersist=300",
                "-o",
            ])
            .arg(format!("ControlPath={}/%C", self.ssh_cache_dir.display()))
            .arg(ssh)
            .arg(swarm_command)
            .args(["list", "--json"])
            .timeout(PROBE_TIMEOUT);
        let result = self
            .shell
            .run(command)
            .await
            .map_err(|error| LegacyFailure {
                error: match error {
                    DaemonError::Timeout(_) => "timed out after 30s".to_owned(),
                    error => error.to_string(),
                },
                stderr: None,
            })?;
        if !result.success() {
            let detail = result.stderr.trim().lines().last().unwrap_or("").trim();
            let detail = if detail.is_empty() {
                format!("exited {}", result.status)
            } else {
                detail.to_owned()
            };
            return Err(LegacyFailure {
                error: if result.status == 255 {
                    detail
                } else {
                    format!("{swarm_command}: {detail}")
                },
                stderr: (!result.stderr.is_empty()).then_some(result.stderr),
            });
        }
        // Deserialize only the handshake fields; inventory is intentionally ignored.
        let reply: RemoteReply =
            serde_json::from_str(&result.stdout).map_err(|_| LegacyFailure {
                error: "remote swarm protocol mismatch".to_owned(),
                stderr: (!result.stderr.is_empty()).then_some(result.stderr.clone()),
            })?;
        if reply.protocol.as_u64() != Some(1) {
            return Err(LegacyFailure {
                error: if reply.protocol.is_null() {
                    "remote swarm protocol mismatch".to_owned()
                } else {
                    format!(
                        "remote swarm protocol mismatch (protocol {})",
                        reply.protocol
                    )
                },
                stderr: (!result.stderr.is_empty()).then_some(result.stderr),
            });
        }
        Ok(reply.version.as_str().map(str::to_owned))
    }
}

struct LegacyFailure {
    error: String,
    stderr: Option<String>,
}

async fn diagnose_provider(
    provider: Arc<dyn MachineProvider>,
    endpoint: Option<Arc<dyn RemoteEndpoint>>,
) -> HostDiagnostics {
    let (address, report, agent_binaries) = tokio::join!(
        provider.resolve(),
        provider.probe(PROBE_TIMEOUT),
        check_agent_binaries(provider.as_ref())
    );
    let (address, resolve_error) = match address {
        Ok(address) => (Some(address.host), None),
        Err(error) => (None, Some(error.to_string())),
    };
    diagnostics_from_report(
        provider,
        endpoint,
        address,
        resolve_error,
        report,
        agent_binaries,
    )
}

async fn ready_diagnostics(
    provider: Arc<dyn MachineProvider>,
    endpoint: Option<Arc<dyn RemoteEndpoint>>,
    cached: Option<HostStatus>,
) -> HostDiagnostics {
    let (resolved, agent_binaries) = tokio::join!(
        async {
            if cached
                .as_ref()
                .and_then(|status| status.address.as_ref())
                .is_none()
            {
                Some(provider.resolve().await)
            } else {
                None
            }
        },
        check_agent_binaries(provider.as_ref())
    );
    let (address, resolve_error) = match resolved {
        Some(Ok(address)) => (Some(address.host), None),
        Some(Err(error)) => (None, Some(error.to_string())),
        None => (
            cached.as_ref().and_then(|status| status.address.clone()),
            None,
        ),
    };
    diagnostics_from_report(
        provider,
        endpoint,
        address,
        resolve_error,
        ProbeReport {
            reachable: true,
            latency_ms: None,
            version: cached.and_then(|status| status.version),
            error: None,
            stderr: None,
        },
        agent_binaries,
    )
}

async fn check_agent_binaries(provider: &dyn MachineProvider) -> Option<AgentBinaries> {
    let argv = ["bash", "-lc", AGENT_BINARY_SCRIPT].map(str::to_owned);
    let output = provider.exec(&argv, AGENT_BINARY_TIMEOUT).await.ok()?;
    if output.status != 0 {
        return None;
    }
    let found = output
        .stdout
        .lines()
        .collect::<std::collections::BTreeSet<_>>();
    Some(AgentBinaries {
        claude: found.contains("claude"),
        opencode: found.contains("opencode"),
    })
}

fn diagnostics_from_report(
    provider: Arc<dyn MachineProvider>,
    endpoint: Option<Arc<dyn RemoteEndpoint>>,
    address: Option<String>,
    resolve_error: Option<String>,
    report: ProbeReport,
    agent_binaries: Option<AgentBinaries>,
) -> HostDiagnostics {
    let link = endpoint
        .as_ref()
        .map_or(LinkState::Down, |endpoint| endpoint.state());
    let hello = endpoint.as_ref().and_then(|endpoint| endpoint.hello());
    let reachable = link == LinkState::Ready || (report.reachable && resolve_error.is_none());
    let error = if reachable {
        None
    } else {
        report.error.clone().or_else(|| resolve_error.clone())
    };
    let version = hello
        .as_ref()
        .map(|hello| hello.version.clone())
        .or(report.version);
    HostDiagnostics {
        status: HostStatus {
            id: provider.id().clone(),
            provider: provider.provider_name().to_owned(),
            version,
            link,
            address,
            agent_binaries,
            reachable,
            checked_at: chrono::Utc::now().to_rfc3339(),
            error,
        },
        resolve_error,
        stderr: report.stderr,
        hello,
    }
}

fn unavailable_diagnostics(id: &HostId, entry: &HostConfigEntry) -> HostDiagnostics {
    let error = "machine provider unavailable".to_owned();
    HostDiagnostics {
        status: HostStatus {
            id: id.clone(),
            provider: provider_name(Some(entry)),
            version: None,
            link: link_state(Some(entry)),
            address: None,
            agent_binaries: None,
            reachable: false,
            checked_at: chrono::Utc::now().to_rfc3339(),
            error: Some(error.clone()),
        },
        resolve_error: Some(error),
        stderr: None,
        hello: None,
    }
}

fn provider_name(entry: Option<&HostConfigEntry>) -> String {
    match entry {
        Some(HostConfigEntry::Tailscale { .. }) => "tailscale",
        Some(HostConfigEntry::Command { .. }) => "command",
        Some(HostConfigEntry::Legacy { .. }) => "legacy",
        None => "unknown",
    }
    .to_owned()
}

fn link_state(entry: Option<&HostConfigEntry>) -> fleet_proto::snapshot::LinkState {
    if matches!(entry, Some(HostConfigEntry::Legacy { .. })) {
        fleet_proto::snapshot::LinkState::Legacy
    } else {
        fleet_proto::snapshot::LinkState::Down
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DaemonResult,
        adapters::shell::{DetachedProcess, LineCallback, ShellResult},
        testing::fakes::{FakeShell, FakeShellCall},
    };
    use std::{os::unix::fs::PermissionsExt, path::Path};
    use tokio_util::sync::CancellationToken;

    fn host() -> (HostId, HostConfigEntry) {
        (
            HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}")),
            HostConfigEntry::Legacy {
                ssh: "arch-dev".to_owned(),
                swarm_command: "swarm".to_owned(),
            },
        )
    }

    fn shell_result(status: i32, stdout: &str, stderr: &str) -> ShellResult {
        ShellResult {
            status,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    async fn probe_result(result: ShellResult) -> HostStatus {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(|command| command.program == "ssh", result);
        let (id, entry) = host();
        Hosts::new(temp.path(), shell)
            .diagnose_legacy(&id, &entry)
            .await
            .status
    }

    #[tokio::test]
    async fn success_checks_protocol_and_uses_private_control_socket_directory() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "ssh",
            shell_result(
                0,
                r#"{"protocol":1,"version":"swarm 0.1.0+a5a11f0","worktrees":[{"ignored":true}]}"#,
                "",
            ),
        );
        let hosts = Hosts::new(temp.path(), shell.clone());
        let (id, entry) = host();
        let diagnostics = hosts.diagnose_legacy(&id, &entry).await;
        let status = diagnostics.status;
        let version = status.version.clone();
        assert!(status.reachable);
        assert_eq!(status.error, None);
        assert_eq!(status.id, id);
        assert!(chrono::DateTime::parse_from_rfc3339(&status.checked_at).is_ok());
        assert_eq!(version.as_deref(), Some("swarm 0.1.0+a5a11f0"));
        assert_eq!(
            shell.calls(),
            vec![FakeShellCall::Run(
                ShellCommand::new("ssh")
                    .args([
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=10",
                        "-o",
                        "ControlMaster=auto",
                        "-o",
                        "ControlPersist=300",
                        "-o"
                    ])
                    .arg(format!("ControlPath={}/%C", hosts.ssh_cache_dir.display()))
                    .args(["arch-dev", "swarm", "list", "--json"])
                    .timeout(Duration::from_secs(30))
            )]
        );
        let permissions = std::fs::metadata(&hosts.ssh_cache_dir)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions();
        assert_eq!(permissions.mode() & 0o777, 0o700);
    }

    #[tokio::test]
    async fn ssh_failure_reports_last_trimmed_stderr_line() {
        let status = probe_result(shell_result(
            255,
            "",
            "warning\n  Permission denied (publickey)  \n",
        ))
        .await;
        assert!(!status.reachable);
        assert_eq!(
            status.error.as_deref(),
            Some("Permission denied (publickey)")
        );
    }

    #[tokio::test]
    async fn remote_command_failure_includes_command_prefix() {
        let status = probe_result(shell_result(
            127,
            "",
            "warning\n swarm: command not found\n",
        ))
        .await;
        assert!(!status.reachable);
        assert_eq!(
            status.error.as_deref(),
            Some("swarm: swarm: command not found")
        );
    }

    #[tokio::test]
    async fn non_json_output_is_a_protocol_mismatch() {
        let status = probe_result(shell_result(0, "Welcome to the devbox!", "")).await;
        assert!(!status.reachable);
        assert_eq!(
            status.error.as_deref(),
            Some("remote swarm protocol mismatch")
        );
    }

    #[tokio::test]
    async fn wrong_protocol_includes_parseable_protocol() {
        for protocol in ["2", "\"1\"", "null"] {
            let status = probe_result(shell_result(
                0,
                &format!(r#"{{"protocol":{protocol}}}"#),
                "",
            ))
            .await;
            assert!(!status.reachable);
            let error = status.error.unwrap_or_default();
            assert!(error.starts_with("remote swarm protocol mismatch"));
            if protocol != "null" {
                assert!(error.contains(protocol));
            }
        }
    }

    // The Shell boundary owns deadline enforcement and returns this distinct error.
    struct TimeoutShell(FakeShell);

    #[async_trait::async_trait]
    impl Shell for TimeoutShell {
        async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
            assert_eq!(command.timeout, Some(Duration::from_secs(30)));
            self.0.run(command).await?;
            Err(DaemonError::Timeout("ssh".to_owned()))
        }

        async fn run_detached(&self, _: ShellCommand, _: &Path) -> DaemonResult<DetachedProcess> {
            panic!("probe must not detach");
        }

        async fn run_streaming(
            &self,
            _: ShellCommand,
            _: CancellationToken,
            _: LineCallback,
        ) -> DaemonResult<ShellResult> {
            panic!("probe must capture output");
        }
    }

    #[tokio::test]
    async fn timeout_reports_thirty_second_deadline() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let hosts = Hosts::new(temp.path(), Arc::new(TimeoutShell(FakeShell::new())));
        let (id, entry) = host();
        let status = hosts.diagnose_legacy(&id, &entry).await.status;
        assert!(!status.reachable);
        assert_eq!(status.error.as_deref(), Some("timed out after 30s"));
    }

    #[tokio::test]
    async fn snapshots_are_pending_until_probed_and_prune_removed_hosts() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(|_| true, shell_result(0, r#"{"protocol":1}"#, ""));
        let hosts = Hosts::new(temp.path(), shell.clone());
        let (id, entry) = host();
        let mut config = fleet_core::config::default_config(temp.path());
        config.hosts.insert(id.clone(), entry.clone());
        let pending = hosts.snapshot(&config, "generated").await;
        assert!(!pending[0].reachable);
        assert_eq!(pending[0].checked_at, "generated");
        assert_eq!(pending[0].error.as_deref(), Some("probe pending"));
        assert!(shell.calls().is_empty());
        let machines = Machines::from_config(&config);
        let results = hosts.probe_all(&config, &machines).await;
        assert_eq!(
            hosts.snapshot(&config, "later").await,
            vec![results[&id].clone()]
        );
        config.hosts.clear();
        assert!(hosts.snapshot(&config, "later").await.is_empty());
        assert!(hosts.statuses.read().await.is_empty());
        config.hosts.insert(id, entry);
        let machines = Machines::from_config(&config);
        hosts.probe_all(&config, &machines).await;
        config.hosts.clear();
        assert!(hosts.probe_all(&config, &machines).await.is_empty());
        assert!(hosts.statuses.read().await.is_empty());
    }
}
