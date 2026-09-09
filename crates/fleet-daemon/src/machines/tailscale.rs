//! Tailscale peer resolution and OpenSSH transport.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use fleet_core::{ids::HostId, paths::FleetHome};
use serde::Deserialize;

use crate::{
    DaemonError,
    adapters::shell::{RealShell, Shell, ShellCommand, ShellResult},
};

use super::{
    AsyncDuplex, ChildStream, ExecOutput, MachineAddress, MachineError, MachineProvider,
    ProbeReport, ssh::SshArgv,
};

const DEFAULT_RESOLVE_CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct CachedAddress {
    address: MachineAddress,
    resolved_at: Instant,
}

trait StreamSpawner: Send + Sync {
    fn spawn(&self, argv: &[String]) -> Result<Box<dyn AsyncDuplex>, MachineError>;
}

struct ChildStreamSpawner;

impl StreamSpawner for ChildStreamSpawner {
    fn spawn(&self, argv: &[String]) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        Ok(Box::new(ChildStream::spawn(argv)?))
    }
}

#[derive(Deserialize)]
struct TailscaleStatus {
    #[serde(default, rename = "Peer")]
    peers: BTreeMap<String, TailscalePeer>,
}

#[derive(Deserialize)]
struct TailscalePeer {
    #[serde(default, rename = "HostName")]
    host_name: String,
    #[serde(default, rename = "DNSName")]
    dns_name: String,
    #[serde(default, rename = "TailscaleIPs")]
    tailscale_ips: Vec<String>,
    #[serde(default, rename = "Online")]
    online: Option<bool>,
}

/// A configured Tailscale peer reached through OpenSSH.
pub struct TailscaleMachine {
    id: HostId,
    node: String,
    user: Option<String>,
    ssh_options: Vec<String>,
    fleetd: String,
    fleet_home: Option<String>,
    shell: Arc<dyn Shell>,
    control_path: PathBuf,
    resolve_cache_ttl: Duration,
    cached_address: Mutex<Option<CachedAddress>>,
    warning: Mutex<Option<String>>,
    stream_spawner: Arc<dyn StreamSpawner>,
}

impl TailscaleMachine {
    /// Creates a provider from the public host configuration fields.
    #[must_use]
    pub fn new(
        id: HostId,
        node: String,
        user: Option<String>,
        ssh_options: Vec<String>,
        fleetd: String,
        fleet_home: Option<String>,
    ) -> Self {
        let local_home = local_fleet_home();
        Self {
            id,
            node,
            user,
            ssh_options,
            fleetd,
            fleet_home,
            shell: Arc::new(RealShell),
            control_path: FleetHome::new(local_home).ssh_cache_dir().join("%C"),
            resolve_cache_ttl: DEFAULT_RESOLVE_CACHE_TTL,
            cached_address: Mutex::new(None),
            warning: Mutex::new(None),
            stream_spawner: Arc::new(ChildStreamSpawner),
        }
    }

    /// Supplies the local Fleet home, refresh interval, and process adapter.
    ///
    /// Registry composition uses this to honor the active daemon home and
    /// `ui.remoteStatusRefreshMs`; the public constructor retains contract compatibility.
    #[must_use]
    pub fn with_runtime(
        mut self,
        local_fleet_home: impl Into<PathBuf>,
        resolve_cache_ttl: Duration,
        shell: Arc<dyn Shell>,
    ) -> Self {
        self.control_path = FleetHome::new(local_fleet_home).ssh_cache_dir().join("%C");
        self.resolve_cache_ttl = resolve_cache_ttl;
        self.shell = shell;
        self
    }

    /// Returns additional OpenSSH options in configuration order.
    #[must_use]
    pub fn ssh_options(&self) -> &[String] {
        &self.ssh_options
    }

    /// Returns a non-fatal resolution warning for host doctor output.
    #[must_use]
    pub fn warning(&self) -> Option<String> {
        lock(&self.warning).clone()
    }

    fn cached_address(&self) -> Option<MachineAddress> {
        lock(&self.cached_address).as_ref().and_then(|cached| {
            (cached.resolved_at.elapsed() < self.resolve_cache_ttl).then(|| cached.address.clone())
        })
    }

    async fn resolve_uncached(&self) -> Result<MachineAddress, MachineError> {
        let command = ShellCommand::new("tailscale").args(["status", "--json"]);
        let result = match self.shell.run(command).await {
            Ok(result) if result.status == 127 => return Ok(self.missing_cli_fallback()),
            Ok(result) => result,
            Err(error) if command_is_missing(&error) => return Ok(self.missing_cli_fallback()),
            Err(error) => return Err(map_shell_error(error, "tailscale status")),
        };
        if !result.success() {
            return Err(MachineError::Unreachable(command_failure(
                "tailscale status",
                &result,
            )));
        }

        let status: TailscaleStatus = serde_json::from_str(&result.stdout)
            .map_err(|error| MachineError::Protocol(format!("tailscale status JSON: {error}")))?;
        let peer = status
            .peers
            .into_values()
            .find(|peer| peer.matches(&self.node))
            .ok_or_else(|| MachineError::NotFound(format!("Tailscale peer {}", self.node)))?;
        let dns_name = peer.dns_name.trim_end_matches('.').to_owned();
        let host = peer
            .tailscale_ips
            .iter()
            .find(|address| !address.is_empty())
            .cloned()
            .or_else(|| (!dns_name.is_empty()).then(|| dns_name.clone()))
            .ok_or_else(|| {
                MachineError::NotFound(format!(
                    "Tailscale peer {} has no IP or DNS name",
                    self.node
                ))
            })?;
        let display = if dns_name.is_empty() {
            peer.host_name
        } else {
            dns_name
        };
        *lock(&self.warning) = None;
        Ok(MachineAddress {
            host,
            user: self.user.clone(),
            display,
            online: peer.online,
        })
    }

    fn missing_cli_fallback(&self) -> MachineAddress {
        *lock(&self.warning) = Some(format!(
            "tailscale CLI unavailable; using configured SSH host {}",
            self.node
        ));
        MachineAddress {
            host: self.node.clone(),
            user: self.user.clone(),
            display: self.node.clone(),
            online: None,
        }
    }

    fn ssh_argv(&self, address: &MachineAddress) -> SshArgv {
        let destination = match &address.user {
            Some(user) => format!("{user}@{}", address.host),
            None => address.host.clone(),
        };
        SshArgv::new(destination, self.control_path.clone()).options(self.ssh_options.clone())
    }

    async fn run_remote(
        &self,
        remote: &[String],
        timeout: Duration,
    ) -> Result<ExecOutput, MachineError> {
        let address = self.resolve().await?;
        let ssh = self.ssh_argv(&address);
        ssh.ensure_control_dir().await?;
        let argv = ssh.build(remote);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| MachineError::NotFound("empty SSH argv".to_owned()))?;
        let result = self
            .shell
            .run(ShellCommand::new(program).args(args).timeout(timeout))
            .await
            .map_err(|error| map_shell_error(error, "ssh"))?;
        ChildStream::check_exit(result.status, &result.stderr)?;
        Ok(ExecOutput {
            status: result.status,
            stdout: result.stdout,
            stderr: result.stderr,
        })
    }

    #[cfg(test)]
    fn with_stream_spawner(mut self, stream_spawner: Arc<dyn StreamSpawner>) -> Self {
        self.stream_spawner = stream_spawner;
        self
    }
}

impl TailscalePeer {
    fn matches(&self, node: &str) -> bool {
        normalized_eq(&self.host_name, node) || normalized_eq(&self.dns_name, node)
    }
}

#[async_trait]
impl MachineProvider for TailscaleMachine {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn provider_name(&self) -> &'static str {
        "tailscale"
    }

    async fn resolve(&self) -> Result<MachineAddress, MachineError> {
        if let Some(address) = self.cached_address() {
            return Ok(address);
        }
        let address = self.resolve_uncached().await?;
        *lock(&self.cached_address) = Some(CachedAddress {
            address: address.clone(),
            resolved_at: Instant::now(),
        });
        Ok(address)
    }

    async fn probe(&self, timeout: Duration) -> ProbeReport {
        let started = Instant::now();
        match self.run_remote(&["true".to_owned()], timeout).await {
            Ok(output) if output.status == 0 => {}
            Ok(output) => {
                return failed_probe(
                    started,
                    format!("ssh true exited {}", output.status),
                    output.stderr,
                );
            }
            Err(error) => return failed_probe_error(started, error),
        }

        match self
            .run_remote(&[self.fleetd.clone(), "--version".to_owned()], timeout)
            .await
        {
            Ok(output) if output.status == 0 => ProbeReport {
                reachable: true,
                latency_ms: Some(elapsed_ms(started)),
                version: Some(output.stdout.trim().to_owned()),
                error: None,
                stderr: (!output.stderr.is_empty()).then_some(output.stderr),
            },
            Ok(output) => failed_probe(
                started,
                format!("{} --version exited {}", self.fleetd, output.status),
                output.stderr,
            ),
            Err(error) => failed_probe_error(started, error),
        }
    }

    async fn exec(&self, argv: &[String], timeout: Duration) -> Result<ExecOutput, MachineError> {
        self.run_remote(argv, timeout).await
    }

    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        let address = self.resolve().await?;
        let ssh = self.ssh_argv(&address);
        ssh.ensure_control_dir().await?;
        let mut remote = vec![self.fleetd.clone(), "connect".to_owned()];
        if let Some(home) = &self.fleet_home {
            remote.extend(["--home".to_owned(), home.clone()]);
        }
        self.stream_spawner.spawn(&ssh.build(&remote))
    }

    fn fleetd_binary(&self) -> &str {
        &self.fleetd
    }
    fn fleet_home(&self) -> Option<&str> {
        self.fleet_home.as_deref()
    }

    fn warning(&self) -> Option<String> {
        TailscaleMachine::warning(self)
    }
}

fn local_fleet_home() -> PathBuf {
    std::env::var_os("FLEET_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".fleet")))
        .unwrap_or_else(|| PathBuf::from(".fleet"))
}

fn normalized_eq(left: &str, right: &str) -> bool {
    left.trim_end_matches('.')
        .eq_ignore_ascii_case(right.trim_end_matches('.'))
}

fn command_is_missing(error: &DaemonError) -> bool {
    matches!(error, DaemonError::Shell(message) if message.contains("No such file or directory") || message.contains("not found"))
}

fn map_shell_error(error: DaemonError, operation: &str) -> MachineError {
    match error {
        DaemonError::Timeout(_) => MachineError::Timeout(operation.to_owned()),
        error if command_is_missing(&error) => MachineError::NotFound(error.to_string()),
        error => MachineError::Unreachable(error.to_string()),
    }
}

fn command_failure(operation: &str, result: &ShellResult) -> String {
    let stderr = result.stderr.trim();
    if stderr.is_empty() {
        format!("{operation} exited {}", result.status)
    } else {
        format!("{operation} exited {}: {stderr}", result.status)
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn failed_probe(started: Instant, error: String, stderr: String) -> ProbeReport {
    ProbeReport {
        reachable: false,
        latency_ms: Some(elapsed_ms(started)),
        version: None,
        error: Some(error),
        stderr: (!stderr.is_empty()).then_some(stderr),
    }
}

fn failed_probe_error(started: Instant, error: MachineError) -> ProbeReport {
    let stderr = match &error {
        MachineError::Unreachable(stderr) if !stderr.is_empty() => Some(stderr.clone()),
        _ => None,
    };
    ProbeReport {
        reachable: false,
        latency_ms: Some(elapsed_ms(started)),
        version: None,
        error: Some(error.to_string()),
        stderr,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fakes::{FakeShell, FakeShellCall};

    fn shell_result(status: i32, stdout: &str, stderr: &str) -> ShellResult {
        ShellResult {
            status,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    fn status_json() -> &'static str {
        r#"{"Peer":{"peer-key":{"HostName":"DEV-BOX","DNSName":"Dev-Box.tailnet.ts.net.","TailscaleIPs":["100.64.0.7","fd7a::7"],"Online":true}}}"#
    }

    fn machine(local_home: &Path, shell: Arc<dyn Shell>, cache_ttl: Duration) -> TailscaleMachine {
        TailscaleMachine::new(
            HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}")),
            "dev-box".to_owned(),
            Some("df".to_owned()),
            vec![
                "-o".to_owned(),
                "StrictHostKeyChecking=accept-new".to_owned(),
            ],
            "fleetd".to_owned(),
            Some("~/.fleet".to_owned()),
        )
        .with_runtime(local_home, cache_ttl, shell)
    }

    fn resolve_command() -> ShellCommand {
        ShellCommand::new("tailscale").args(["status", "--json"])
    }

    fn ssh_command(local_home: &Path, remote: &str, timeout: Duration) -> ShellCommand {
        ShellCommand::new("ssh")
            .args([
                "-o".to_owned(),
                "BatchMode=yes".to_owned(),
                "-o".to_owned(),
                "ConnectTimeout=10".to_owned(),
                "-o".to_owned(),
                "ControlMaster=auto".to_owned(),
                "-o".to_owned(),
                "ControlPersist=300".to_owned(),
                "-o".to_owned(),
                format!(
                    "ControlPath={}",
                    FleetHome::new(local_home)
                        .ssh_cache_dir()
                        .join("%C")
                        .display()
                ),
                "-o".to_owned(),
                "StrictHostKeyChecking=accept-new".to_owned(),
                "--".to_owned(),
                "df@100.64.0.7".to_owned(),
                remote.to_owned(),
            ])
            .timeout(timeout)
    }

    #[tokio::test]
    async fn resolve_matches_hostname_and_returns_first_ip_with_exact_argv() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command == &resolve_command(),
            shell_result(0, status_json(), ""),
        );
        let provider = machine(temp.path(), shell.clone(), Duration::from_secs(10));

        let address = provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            address,
            MachineAddress {
                host: "100.64.0.7".to_owned(),
                user: Some("df".to_owned()),
                display: "Dev-Box.tailnet.ts.net".to_owned(),
                online: Some(true),
            }
        );
        assert_eq!(shell.calls(), vec![FakeShellCall::Run(resolve_command())]);
        assert_eq!(provider.warning(), None);
    }

    #[tokio::test]
    async fn resolve_matches_dns_case_insensitively_and_falls_back_to_dns() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command == &resolve_command(),
            shell_result(
                0,
                r#"{"Peer":{"key":{"HostName":"other","DNSName":"DEV-BOX.","TailscaleIPs":[],"Online":false}}}"#,
                "",
            ),
        );
        let provider = machine(temp.path(), shell, Duration::from_secs(10));

        let address = provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(address.host, "DEV-BOX");
        assert_eq!(address.online, Some(false));
    }

    #[tokio::test]
    async fn missing_tailscale_cli_falls_back_and_exposes_warning() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command == &resolve_command(),
            shell_result(127, "", "tailscale: command not found"),
        );
        let provider = machine(temp.path(), shell, Duration::from_secs(10));

        let address = provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(address.host, "dev-box");
        assert_eq!(address.online, None);
        assert_eq!(
            provider.warning().as_deref(),
            Some("tailscale CLI unavailable; using configured SSH host dev-box")
        );
    }

    #[tokio::test]
    async fn resolve_cache_expires_at_the_configured_refresh_interval() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command == &resolve_command(),
            shell_result(0, status_json(), ""),
        );
        let provider = machine(temp.path(), shell.clone(), Duration::from_millis(10));

        provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(shell.calls().len(), 1);

        tokio::time::sleep(Duration::from_millis(20)).await;
        provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(shell.calls().len(), 2);
    }

    #[tokio::test]
    async fn probe_runs_true_then_version_with_exact_argv() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        let timeout = Duration::from_secs(7);
        shell.when(
            |command| command == &resolve_command(),
            shell_result(0, status_json(), ""),
        );
        let true_command = ssh_command(temp.path(), "'true'", timeout);
        let version_command = ssh_command(temp.path(), "'fleetd' '--version'", timeout);
        shell.when(
            {
                let expected = true_command.clone();
                move |command| command == &expected
            },
            shell_result(0, "", ""),
        );
        shell.when(
            {
                let expected = version_command.clone();
                move |command| command == &expected
            },
            shell_result(0, "fleetd 0.1.0\n", ""),
        );
        let provider = machine(temp.path(), shell.clone(), Duration::from_secs(10));

        let report = provider.probe(timeout).await;

        assert!(report.reachable);
        assert_eq!(report.version.as_deref(), Some("fleetd 0.1.0"));
        assert_eq!(
            shell.calls(),
            vec![
                FakeShellCall::Run(resolve_command()),
                FakeShellCall::Run(true_command),
                FakeShellCall::Run(version_command),
            ]
        );
    }

    #[tokio::test]
    async fn exec_captures_output_and_uses_exact_argv() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        let timeout = Duration::from_secs(9);
        shell.when(
            |command| command == &resolve_command(),
            shell_result(0, status_json(), ""),
        );
        let expected = ssh_command(temp.path(), "'printf' 'it'\"'\"'s ready'", timeout);
        shell.when(
            {
                let expected = expected.clone();
                move |command| command == &expected
            },
            shell_result(23, "partial", "remote warning\n"),
        );
        let provider = machine(temp.path(), shell.clone(), Duration::from_secs(10));

        let output = provider
            .exec(&["printf".to_owned(), "it's ready".to_owned()], timeout)
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(output.status, 23);
        assert_eq!(output.stdout, "partial");
        assert_eq!(output.stderr, "remote warning\n");
        assert_eq!(
            shell.calls(),
            vec![
                FakeShellCall::Run(resolve_command()),
                FakeShellCall::Run(expected),
            ]
        );
    }

    #[tokio::test]
    async fn exec_maps_ssh_255_to_unreachable() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        let timeout = Duration::from_secs(9);
        shell.when(
            |command| command == &resolve_command(),
            shell_result(0, status_json(), ""),
        );
        let expected = ssh_command(temp.path(), "'true'", timeout);
        shell.when(
            {
                let expected = expected.clone();
                move |command| command == &expected
            },
            shell_result(255, "", "No route to host\n"),
        );
        let provider = machine(temp.path(), shell, Duration::from_secs(10));

        let error = provider
            .exec(&["true".to_owned()], timeout)
            .await
            .expect_err("SSH 255 must be unreachable");

        assert!(
            matches!(error, MachineError::Unreachable(message) if message == "No route to host")
        );
    }

    #[test]
    fn shell_timeouts_map_to_machine_timeouts() {
        let error = map_shell_error(DaemonError::Timeout("ssh".to_owned()), "ssh");

        assert!(matches!(error, MachineError::Timeout(operation) if operation == "ssh"));
    }

    #[derive(Default)]
    struct CapturingSpawner {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl StreamSpawner for CapturingSpawner {
        fn spawn(&self, argv: &[String]) -> Result<Box<dyn AsyncDuplex>, MachineError> {
            lock(&self.calls).push(argv.to_vec());
            let (stream, _peer) = tokio::io::duplex(64);
            Ok(Box::new(stream))
        }
    }

    #[tokio::test]
    async fn open_stream_spawns_child_stream_argv_exactly() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command == &resolve_command(),
            shell_result(0, status_json(), ""),
        );
        let spawner = Arc::new(CapturingSpawner::default());
        let provider = machine(temp.path(), shell.clone(), Duration::from_secs(10))
            .with_stream_spawner(spawner.clone());

        let _stream = provider
            .open_stream()
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        let control_path = FleetHome::new(temp.path()).ssh_cache_dir().join("%C");
        assert_eq!(
            lock(&spawner.calls).as_slice(),
            [SshArgv::new("df@100.64.0.7", control_path)
                .options([
                    "-o".to_owned(),
                    "StrictHostKeyChecking=accept-new".to_owned(),
                ])
                .build(&[
                    "fleetd".to_owned(),
                    "connect".to_owned(),
                    "--home".to_owned(),
                    "~/.fleet".to_owned(),
                ])]
        );
        assert_eq!(shell.calls(), vec![FakeShellCall::Run(resolve_command())]);
    }

    #[tokio::test]
    #[ignore = "requires a real Tailscale peer named by FLEET_TAILSCALE_HOST"]
    async fn resolves_and_probes_real_tailscale_peer() {
        let Ok(node) = std::env::var("FLEET_TAILSCALE_HOST") else {
            return;
        };
        let user = std::env::var("FLEET_TAILSCALE_USER").ok();
        let provider = TailscaleMachine::new(
            HostId::try_from("tailscale-test").unwrap_or_else(|error| panic!("{error}")),
            node,
            user,
            Vec::new(),
            "fleetd".to_owned(),
            Some("~/.fleet".to_owned()),
        );

        let address = provider
            .resolve()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!address.host.is_empty());
        let report = provider.probe(Duration::from_secs(30)).await;
        assert!(report.reachable, "{report:?}");
        assert!(report.version.is_some());
    }
}
