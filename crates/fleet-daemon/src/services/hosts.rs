//! Read-only remote swarm reachability probes and cached snapshot statuses.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use fleet_core::{config::Config, ids::HostId, model::HostConfigEntry, paths::FleetHome};
use fleet_proto::snapshot::HostStatus;
use futures_util::future::join_all;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{
    DaemonError,
    adapters::shell::{Shell, ShellCommand},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub async fn snapshot(&self, config: &Config, generated_at: &str) -> Vec<HostStatus> {
        let mut statuses = self.statuses.write().await;
        statuses.retain(|id, _| config.hosts.contains_key(id));
        config
            .hosts
            .keys()
            .map(|id| {
                statuses.get(id).cloned().unwrap_or_else(|| HostStatus {
                    id: id.clone(),
                    reachable: false,
                    checked_at: generated_at.to_owned(),
                    error: Some("probe pending".to_owned()),
                })
            })
            .collect()
    }

    /// Probes hosts concurrently and replaces the cache with exactly the configured hosts.
    pub async fn probe_all(&self, config: &Config) -> BTreeMap<HostId, HostStatus> {
        self.statuses
            .write()
            .await
            .retain(|id, _| config.hosts.contains_key(id));
        let results = join_all(
            config
                .hosts
                .iter()
                .map(|(id, entry)| async move { (id.clone(), self.probe(id, entry).await) }),
        )
        .await
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        *self.statuses.write().await = results.clone();
        results
    }

    /// Checks SSH connectivity and remote swarm protocol compatibility.
    pub async fn probe(&self, id: &HostId, entry: &HostConfigEntry) -> HostStatus {
        self.probe_with_version(id, entry).await.0
    }

    pub(super) async fn probe_with_version(
        &self,
        id: &HostId,
        entry: &HostConfigEntry,
    ) -> (HostStatus, Option<String>) {
        let result = self.remote_version(entry).await;
        let (error, version) = match result {
            Ok(version) => (None, version),
            Err(error) => (Some(error), None),
        };
        (
            HostStatus {
                id: id.clone(),
                reachable: error.is_none(),
                checked_at: chrono::Utc::now().to_rfc3339(),
                error,
            },
            version,
        )
    }

    async fn remote_version(&self, entry: &HostConfigEntry) -> Result<Option<String>, String> {
        let mut directory = tokio::fs::DirBuilder::new();
        directory.recursive(true).mode(0o700);
        directory
            .create(&self.ssh_cache_dir)
            .await
            .map_err(|error| format!("{}: {error}", self.ssh_cache_dir.display()))?;
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
            .arg(&entry.ssh)
            .arg(&entry.swarm_command)
            .args(["list", "--json"])
            .timeout(PROBE_TIMEOUT);
        let result = self.shell.run(command).await.map_err(|error| match error {
            DaemonError::Timeout(_) => "timed out after 30s".to_owned(),
            error => error.to_string(),
        })?;
        if !result.success() {
            let detail = result.stderr.trim().lines().last().unwrap_or("").trim();
            let detail = if detail.is_empty() {
                format!("exited {}", result.status)
            } else {
                detail.to_owned()
            };
            return Err(if result.status == 255 {
                detail
            } else {
                format!("{}: {detail}", entry.swarm_command)
            });
        }
        // Deserialize only the handshake fields; inventory is intentionally ignored.
        let reply: RemoteReply = serde_json::from_str(&result.stdout)
            .map_err(|_| "remote swarm protocol mismatch".to_owned())?;
        if reply.protocol.as_u64() != Some(1) {
            return Err(if reply.protocol.is_null() {
                "remote swarm protocol mismatch".to_owned()
            } else {
                format!(
                    "remote swarm protocol mismatch (protocol {})",
                    reply.protocol
                )
            });
        }
        Ok(reply.version.as_str().map(str::to_owned))
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
            HostConfigEntry {
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
        Hosts::new(temp.path(), shell).probe(&id, &entry).await
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
        let (status, version) = hosts.probe_with_version(&id, &entry).await;
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
        let status = hosts.probe(&id, &entry).await;
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
        let results = hosts.probe_all(&config).await;
        assert_eq!(
            hosts.snapshot(&config, "later").await,
            vec![results[&id].clone()]
        );
        config.hosts.clear();
        assert!(hosts.snapshot(&config, "later").await.is_empty());
        assert!(hosts.statuses.read().await.is_empty());
        config.hosts.insert(id, entry);
        hosts.probe_all(&config).await;
        config.hosts.clear();
        assert!(hosts.probe_all(&config).await.is_empty());
        assert!(hosts.statuses.read().await.is_empty());
    }
}
