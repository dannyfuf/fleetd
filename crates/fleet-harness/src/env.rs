//! Hermetic Fleet and fleetd child environment.
//!
//! One implementation of "start an isolated fleetd" serves the whole workspace: the runner
//! drives [`HarnessEnv`] plus [`Daemon`] directly, and `fleet-app`'s integration tests use the
//! same code through [`IsolatedDaemon`], which `crates/fleet-app/tests/common/mod.rs`
//! re-exports as `Daemon`.
//!
//! Nothing here may read or write the developer's real state. Every child process gets a
//! private `FLEET_HOME`, a child-only `HOME` (so `~/.gitconfig`, `~/.config` and the `gh`
//! credential store are all out of reach), a `PATH` whose first entry holds fake
//! network-facing executables, and `FLEET_DAEMON` pointed at the freshly built `fleetd`
//! rather than at whatever is installed. `docs/TESTING-HARNESS.md` §4 is the contract.

use crate::rundir::RunDirectory;
use anyhow::Context as _;
use fleet_client::Client;
use fleet_core::paths::FleetHome;
use fleet_proto::event::EventKind;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};

/// How long a run waits for fleetd to answer its first ping.
pub const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// The app command socket name inside a run directory.
pub const APP_SOCKET_NAME: &str = "fleet-harness.sock";
/// The daemon socket path relative to a run directory.
pub const DAEMON_SOCKET_RELATIVE: &str = "home/fleetd.sock";
/// Marker checked by the fixture's `fleet` shim once scenario teardown begins.
const FLEET_CLI_STOP_FILE: &str = ".fleet-harness-cli-stop";
/// How often readiness is probed while waiting for it.
const PROBE_INTERVAL: Duration = Duration::from_millis(50);
/// How long an orderly shutdown is given before the daemon is killed outright.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the socket file is given to disappear once the daemon has exited.
const SOCKET_REMOVAL_TIMEOUT: Duration = Duration::from_secs(2);
/// How long [`Daemon`]'s drop waits for a killed daemon to be reaped.
const REAP_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the drop polls while reaping.
const REAP_INTERVAL: Duration = Duration::from_millis(5);
/// The `RUST_LOG` both children inherit when the run does not choose one.
const DEFAULT_RUST_LOG: &str = "info";
/// Records Fleet's PID before replacing the wrapper with the real application process.
const APP_PID_WRAPPER: &str = "printf '%s\\n' \"$$\" > \"$1\" || exit 70\nshift\nexec \"$@\"\n";

/// The event families the app subscribes to, which a fixture client mirrors so a test sees
/// exactly what the app would see.
const APP_EVENTS: [EventKind; 4] = [
    EventKind::SnapshotChanged,
    EventKind::JobUpdated,
    EventKind::Toast,
    EventKind::DaemonShuttingDown,
];

/// A `gh` that answers every query with an empty JSON array.
///
/// The daemon shells out to the real `gh` for discovery and pull requests; a run must never
/// reach GitHub, and must never depend on whether the developer happens to be logged in.
const FAKE_GH: &str = "#!/bin/sh\n\
     # Fake gh for the Fleet harness and the fleet-app integration tests: never reaches the network.\n\
     case \"$1\" in\n\
       auth) echo 'github.com: logged in as fleet-test'; exit 0 ;;\n\
       api|pr|search|repo) echo '[]'; exit 0 ;;\n\
       --version) echo 'gh version 0.0.0-fleet-test'; exit 0 ;;\n\
       *) echo '[]'; exit 0 ;;\n\
     esac\n";

/// Everything a child process needs to run inside the run's own world and nothing else.
#[derive(Debug, Clone)]
pub struct HarnessEnv {
    /// `FLEET_HOME`: the private Fleet home this run reads and writes.
    pub fleet_home: PathBuf,
    /// `HOME`: a child-only home, so no child can reach the developer's dotfiles.
    pub child_home: PathBuf,
    /// `FLEET_HARNESS_SOCK`: the app's command socket.
    pub socket: PathBuf,
    /// `PATH`, with [`HarnessEnv::fake_bin`] first.
    pub path: OsString,
    /// The directory whose executables shadow the developer's network-facing tools.
    pub fake_bin: PathBuf,
    /// `FLEET_DAEMON`: the `fleetd` every child must use.
    pub daemon: PathBuf,
    /// `RUST_LOG` for both children.
    pub rust_log: String,
    /// The PID of the hermetic Fleet process, written before its binary executes.
    app_pid: PathBuf,
}

impl HarnessEnv {
    /// Lays a hermetic environment out inside a run directory, using its private `home/`.
    pub fn new(run_dir: &RunDirectory) -> anyhow::Result<Self> {
        Self::lay_out(&run_dir.root, run_dir.home.clone())
    }

    /// Lays the same environment out inside any directory the caller owns and will delete.
    pub fn rooted(root: &Path) -> anyhow::Result<Self> {
        Self::lay_out(root, root.join("home"))
    }

    fn lay_out(root: &Path, fleet_home: PathBuf) -> anyhow::Result<Self> {
        let child_home = root.join("child-home");
        let app_pid = child_home.join("fleet-harness-app.pid");
        let fake_bin = root.join("bin");
        let directories = [
            fleet_home.clone(),
            fake_bin.clone(),
            child_home.join(".config"),
            child_home.join(".local").join("share"),
            child_home.join(".local").join("state"),
            child_home.join(".cache"),
        ];
        for directory in &directories {
            fs::create_dir_all(directory)
                .with_context(|| format!("create {}", directory.display()))?;
        }
        let mut entries = vec![fake_bin.clone()];
        entries.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let environment = Self {
            fleet_home,
            child_home,
            socket: root.join(APP_SOCKET_NAME),
            path: std::env::join_paths(entries).context("build the child PATH")?,
            fake_bin,
            // A missing fleetd is reported when it is spawned, where the message can say how
            // to build it, rather than when the directories are laid out.
            daemon: fleetd_path().unwrap_or_else(|| PathBuf::from("fleetd")),
            rust_log: std::env::var("FLEET_HARNESS_RUST_LOG")
                .or_else(|_| std::env::var("RUST_LOG"))
                .unwrap_or_else(|_| DEFAULT_RUST_LOG.to_owned()),
            app_pid,
        };
        environment.install_fake("gh", FAKE_GH)?;
        Ok(environment)
    }

    /// Overrides `RUST_LOG` for both children.
    #[must_use]
    pub fn with_rust_log(mut self, filter: impl Into<String>) -> Self {
        self.rust_log = filter.into();
        self
    }

    /// Writes an executable that shadows `name` for every child, and returns its path.
    ///
    /// This is how a fixture preset installs a tool-specific fake — `acli` for the board
    /// preset, for instance — without a second notion of what "hermetic" means.
    pub fn install_fake(&self, name: &str, script: &str) -> anyhow::Result<PathBuf> {
        let path = self.fake_bin.join(name);
        fs::write(&path, script).with_context(|| format!("write the fake {name}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .with_context(|| format!("make the fake {name} executable"))?;
        }
        Ok(path)
    }

    /// The daemon's Unix socket inside the private Fleet home.
    ///
    /// Within a run directory this is [`DAEMON_SOCKET_RELATIVE`].
    #[must_use]
    pub fn daemon_socket(&self) -> PathBuf {
        FleetHome::new(self.fleet_home.clone()).socket_path()
    }

    /// Marker the fixture's `fleet` shim checks before it may auto-start a daemon.
    #[must_use]
    pub fn fleet_cli_stop_path(&self) -> PathBuf {
        self.fake_bin.join(FLEET_CLI_STOP_FILE)
    }

    /// Prevents a provider command that outlives daemon shutdown from starting a replacement.
    pub fn stop_fleet_cli(&self) -> anyhow::Result<()> {
        let path = self.fleet_cli_stop_path();
        fs::write(&path, b"teardown\n")
            .with_context(|| format!("write Fleet CLI teardown marker {}", path.display()))
    }

    /// Points one child at this environment and at nothing outside it, without wrapping it.
    pub fn apply(&self, command: &mut Command) {
        command
            .env("FLEET_HOME", &self.fleet_home)
            .env("HOME", &self.child_home)
            .env("FLEET_HARNESS_SOCK", &self.socket)
            .env("PATH", &self.path)
            .env("FLEET_DAEMON", &self.daemon)
            .env("RUST_LOG", &self.rust_log)
            .env("XDG_CONFIG_HOME", self.child_home.join(".config"))
            .env(
                "XDG_DATA_HOME",
                self.child_home.join(".local").join("share"),
            )
            .env(
                "XDG_STATE_HOME",
                self.child_home.join(".local").join("state"),
            )
            .env("XDG_CACHE_HOME", self.child_home.join(".cache"));
    }

    /// Points Fleet at this environment through an `exec` wrapper that records its PID.
    ///
    /// The wrapper is the process the runner owns and `exec` preserves its PID, so daemon fault
    /// injection can stop exactly this run's Fleet during a singleton hand-off without inspecting
    /// other processes.
    pub fn apply_to_app(&self, command: &mut Command) {
        let (program, arguments) = {
            let command = command.as_std();
            (
                command.get_program().to_owned(),
                command.get_args().map(OsString::from).collect::<Vec<_>>(),
            )
        };
        *command = Command::new("sh");
        command
            .arg("-c")
            .arg(APP_PID_WRAPPER)
            .arg("fleet-harness-app")
            .arg(&self.app_pid)
            .arg(program)
            .args(arguments);
        self.apply(command);
    }

    /// Reads the PID recorded by Fleet's launch wrapper.
    pub fn app_pid(&self) -> anyhow::Result<u32> {
        read_pid_file(&self.app_pid, "Fleet")
    }
}

/// A running `fleetd` owned by the run that started it.
#[derive(Debug)]
pub struct Daemon {
    child: Child,
    /// The daemon's Unix socket inside the private Fleet home.
    pub socket: PathBuf,
    environment: HarnessEnv,
    binary: PathBuf,
    log: Option<PathBuf>,
    exited: bool,
}

impl Daemon {
    /// Starts fleetd against `environment` and returns once it answers `daemon_ping`.
    pub async fn start(
        binary: &Path,
        environment: &HarnessEnv,
        stdout: Stdio,
        stderr: Stdio,
    ) -> anyhow::Result<Self> {
        let daemon = Self::spawn(binary, environment, stdout, stderr)?;
        daemon.wait_until_ready().await?;
        Ok(daemon)
    }

    fn spawn(
        binary: &Path,
        environment: &HarnessEnv,
        stdout: Stdio,
        stderr: Stdio,
    ) -> anyhow::Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg("--home")
            .arg(&environment.fleet_home)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(true);
        environment.apply(&mut command);
        let child = command.spawn().with_context(|| {
            format!(
                "start {}; build it with `cargo build -p fleet-daemon` or set FLEET_DAEMON",
                binary.display()
            )
        })?;
        Ok(Self {
            child,
            socket: environment.daemon_socket(),
            environment: environment.clone(),
            binary: binary.to_owned(),
            log: None,
            exited: false,
        })
    }

    fn with_log(mut self, path: PathBuf) -> Self {
        self.log = Some(path);
        self
    }

    /// The process handle, for the runner's fault injection.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Takes ownership of a freshly started replacement `fleetd` in this same home.
    ///
    /// `daemon restart` stops the daemon and spawns another one in the same hermetic home.
    /// Without this the handle keeps pointing at the reaped original and `exited` stays true,
    /// so `Drop` returns immediately: a run that is then interrupted — a stage timing out, an
    /// OOM, a `kill -9` — leaves the replacement running, holding the socket and the pid lock
    /// under a `home/` nothing reclaims. Adopting it restores the guarantee the first daemon
    /// had, that the process cannot outlive the runner whatever happens next.
    pub fn adopt(&mut self, child: Child) {
        self.child = child;
        self.exited = false;
    }

    /// The daemon's process id while it is running.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// The private `FLEET_HOME` this daemon runs in.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.environment.fleet_home
    }

    /// The environment every child of this run shares.
    #[must_use]
    pub fn environment(&self) -> &HarnessEnv {
        &self.environment
    }

    /// The `fleetd` this daemon was started from.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Connects once the daemon answers `daemon_ping`, subscribed to the app's families.
    ///
    /// Probes instead of sleeping: a run is never timed against the wall clock.
    pub async fn client(&self) -> anyhow::Result<Client> {
        let adopted_pid = self
            .pid()
            .context("the harness-owned fleetd has no process id")?;
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(client) = Client::connect(self.home()).await
                && client.daemon_ping().await.is_ok()
            {
                let recorded_pid =
                    read_pid_file(&FleetHome::new(self.home().to_owned()).pid_path(), "fleetd")?;
                ensure_owned_daemon(adopted_pid, recorded_pid, self.home())?;
                client
                    .subscribe(APP_EVENTS.to_vec())
                    .await
                    .context("subscribe to the app's event families")?;
                return Ok(client);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "fleetd did not become ready within {READY_TIMEOUT:?} in {}{}",
                    self.home().display(),
                    self.log_tail()
                );
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    }

    /// Waits for readiness without keeping the client that proved it.
    pub async fn wait_until_ready(&self) -> anyhow::Result<()> {
        self.client().await.map(drop)
    }

    /// Asks the daemon to exit with its terminals, waits for the process, and leaves no socket
    /// behind. This is teardown: nothing the run started may outlive it.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.stop(true).await
    }

    /// Asks the daemon to exit the way `fleet daemon restart` does: the PTY holders stay up so
    /// the replacement adopts the same sessions, which is what a restart scenario is proving.
    pub async fn shutdown_keeping_sessions(&mut self) -> anyhow::Result<()> {
        self.stop(false).await
    }

    async fn stop(&mut self, stop_sessions: bool) -> anyhow::Result<()> {
        let refusal = self.request_shutdown(stop_sessions).await;
        let killed = match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(status) => {
                status.context("wait for fleetd to exit")?;
                false
            }
            Err(_elapsed) => {
                self.child
                    .kill()
                    .await
                    .context("kill an unresponsive fleetd")?;
                true
            }
        };
        self.exited = true;
        let removed = self.wait_for_socket_removal().await;
        if killed || !removed {
            // A leftover socket makes the next run look like a daemon is already running.
            remove_if_present(&self.socket)?;
            anyhow::bail!(
                "fleetd did not shut down cleanly{}",
                refusal.map_or_else(String::new, |refusal| format!(": {refusal}"))
            );
        }
        Ok(())
    }

    /// Asks a reachable daemon to exit, and reports why it could not be asked.
    async fn request_shutdown(&self, stop_sessions: bool) -> Option<String> {
        match Client::connect(self.home()).await {
            Ok(client) => client
                .daemon_shutdown(stop_sessions)
                .await
                .err()
                .map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        }
    }

    async fn wait_for_socket_removal(&self) -> bool {
        let deadline = tokio::time::Instant::now() + SOCKET_REMOVAL_TIMEOUT;
        loop {
            if !self.socket.exists() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        }
    }

    fn log_tail(&self) -> String {
        match self
            .log
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
        {
            Some(log) if !log.is_empty() => format!("; log:\n{log}"),
            _ => String::new(),
        }
    }
}

impl Drop for Daemon {
    /// Reaps the daemon however the run ended, a panic included, and removes its socket.
    ///
    /// Nothing here can await, so the kill is signalled and the exit polled; `fleetd` dies in
    /// milliseconds under `SIGKILL`, and the bound means a wedged one cannot hang a test run.
    fn drop(&mut self) {
        if self.exited {
            return;
        }
        if let Err(error) = self.child.start_kill() {
            eprintln!("fleet-harness: could not signal fleetd: {error}");
        }
        let deadline = std::time::Instant::now() + REAP_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(REAP_INTERVAL);
                }
                Ok(None) => {
                    eprintln!(
                        "fleet-harness: fleetd {:?} outlived its kill",
                        self.child.id()
                    );
                    break;
                }
                Err(error) => {
                    eprintln!("fleet-harness: could not reap fleetd: {error}");
                    break;
                }
            }
        }
        if let Err(error) = remove_if_present(&self.socket) {
            eprintln!("fleet-harness: {error:?}");
        }
    }
}

/// A running fleetd plus the temporary directory that holds its whole world.
///
/// `crates/fleet-app/tests/common/mod.rs` re-exports this as `Daemon`, so the app's
/// integration tests and the harness runner start fleetd through the same code.
#[derive(Debug)]
pub struct IsolatedDaemon {
    // Declaration order is drop order: the daemon dies before its home is deleted.
    daemon: Daemon,
    directory: tempfile::TempDir,
}

impl IsolatedDaemon {
    /// Starts fleetd in a fresh temporary home labelled for the test that owns it.
    ///
    /// Spawning is synchronous and readiness is awaited by [`IsolatedDaemon::connect`], so a
    /// caller can arrange its fixtures while the daemon boots. Must be called from inside a
    /// tokio runtime, as every caller is an async test.
    pub fn start(label: &str) -> anyhow::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix(&format!("fleet-harness-{label}-"))
            .tempdir()
            .context("create an isolated Fleet home")?;
        let environment = HarnessEnv::rooted(directory.path())?;
        let logs = environment.fleet_home.join("logs");
        fs::create_dir_all(&logs).context("create the daemon log directory")?;
        let log_path = logs.join("fleetd.out");
        let log = fs::File::create(&log_path).context("create the daemon log")?;
        let errors = log.try_clone().context("clone the daemon log handle")?;
        let binary = environment.daemon.clone();
        let daemon = Daemon::spawn(&binary, &environment, Stdio::from(log), Stdio::from(errors))?
            .with_log(log_path);
        Ok(Self { daemon, directory })
    }

    /// The temporary `FLEET_HOME`.
    #[must_use]
    pub fn home(&self) -> &Path {
        self.daemon.home()
    }

    /// The temporary directory holding the home, the fake executables and the logs.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// The environment every child process shares.
    #[must_use]
    pub fn environment(&self) -> &HarnessEnv {
        self.daemon.environment()
    }

    /// The daemon itself.
    #[must_use]
    pub fn daemon(&self) -> &Daemon {
        &self.daemon
    }

    /// The daemon itself, mutably, for fault injection.
    pub fn daemon_mut(&mut self) -> &mut Daemon {
        &mut self.daemon
    }

    /// Connects once the daemon is ready, subscribed to the families the app subscribes to.
    ///
    /// An error rather than a panic. `IsolatedDaemon` is the public seam other crates build
    /// fixtures on, and a helper that aborts the process takes the caller's own diagnostics with
    /// it — `docs/TESTING-HARNESS.md` §8 asks for a failure that can be read rather than a
    /// backtrace of the runner.
    pub async fn connect(&self) -> anyhow::Result<Client> {
        self.daemon
            .client()
            .await
            .context("connect to the run's isolated fleetd")
    }

    /// Shuts the daemon down and reports whether it left anything behind.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.daemon.shutdown().await
    }
}

/// Where `fleetd` is: `FLEET_DAEMON`, then beside the current executable, then up to three
/// directories above it (`target/debug/deps/<test>` → `target/debug/fleetd`).
#[must_use]
pub fn fleetd_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FLEET_DAEMON") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let current = std::env::current_exe().ok()?;
    let mut directory = current.parent()?;
    for _ in 0..3 {
        let candidate = directory.join("fleetd");
        if candidate.is_file() {
            return Some(candidate);
        }
        directory = directory.parent()?;
    }
    None
}

fn read_pid_file(path: &Path, process: &str) -> anyhow::Result<u32> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read the {process} PID from {}", path.display()))?;
    contents
        .trim()
        .parse()
        .with_context(|| format!("parse the {process} PID from {}", path.display()))
}

fn ensure_owned_daemon(adopted_pid: u32, recorded_pid: u32, home: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        adopted_pid == recorded_pid,
        "fleetd answered from {} as PID {recorded_pid}, but the harness adopted PID {adopted_pid}",
        home.display()
    );
    Ok(())
}

fn remove_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::Error::new(error).context(format!("remove {}", path.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_can_name_nothing_outside_the_root_it_was_given() {
        let root = tempfile::tempdir().expect("temporary root");
        let environment = HarnessEnv::rooted(root.path()).expect("lay the environment out");

        for path in [
            &environment.fleet_home,
            &environment.child_home,
            &environment.socket,
            &environment.fake_bin,
        ] {
            assert!(
                path.starts_with(root.path()),
                "{} escapes the run root",
                path.display()
            );
        }
        assert_eq!(
            std::env::split_paths(&environment.path).next().as_deref(),
            Some(environment.fake_bin.as_path()),
            "the fake executables must shadow the developer's"
        );
        assert!(
            environment.fake_bin.join("gh").is_file(),
            "gh must be faked, so no run can reach GitHub"
        );
        assert_ne!(
            Some(environment.child_home.clone()),
            std::env::var_os("HOME").map(PathBuf::from),
            "HOME must not be the developer's"
        );
    }

    #[tokio::test]
    async fn the_fleet_launch_wrapper_records_the_execed_process_pid() {
        let root = tempfile::tempdir().expect("temporary root");
        let environment = HarnessEnv::rooted(root.path()).expect("lay the environment out");
        let mut command = Command::new("sh");
        command.arg("-c").arg("printf launched");
        environment.apply_to_app(&mut command);
        command.stdout(Stdio::piped());

        let child = command.spawn().expect("spawn wrapped Fleet");
        let child_pid = child.id().expect("wrapped Fleet has a process id");
        let output = child
            .wait_with_output()
            .await
            .expect("wait for wrapped Fleet");

        assert!(
            output.status.success(),
            "wrapped Fleet exited with {}",
            output.status
        );
        assert_eq!(output.stdout, b"launched");
        assert_eq!(environment.app_pid().expect("read Fleet PID"), child_pid);
    }

    #[test]
    fn readiness_rejects_a_daemon_the_harness_did_not_adopt() {
        let home = Path::new("/tmp/fleet-harness-readiness");
        let error = ensure_owned_daemon(41, 42, home).expect_err("the PIDs differ");
        let message = error.to_string();

        assert!(
            message.contains("PID 42"),
            "the serving PID is named: {message}"
        );
        assert!(
            message.contains("PID 41"),
            "the adopted PID is named: {message}"
        );
    }

    #[tokio::test]
    async fn an_isolated_daemon_runs_in_its_own_home_and_leaves_no_socket_behind() {
        let mut daemon = IsolatedDaemon::start("env")
            .expect("build fleet-daemon before running the hermetic environment test");
        let socket = daemon.daemon().socket.clone();

        let client = daemon
            .connect()
            .await
            .unwrap_or_else(|error| panic!("connect to the isolated daemon: {error:#}"));
        let snapshot = client
            .get_snapshot()
            .await
            .unwrap_or_else(|error| panic!("get_snapshot failed: {error}"));
        assert_eq!(
            snapshot.daemon.home,
            daemon.home().to_string_lossy(),
            "fleetd must run in the temporary home, never in ~/.fleet"
        );
        assert!(socket.exists(), "a ready daemon has bound its socket");
        drop(client);

        daemon
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shutdown failed: {error}"));
        assert!(
            !socket.exists(),
            "an orderly shutdown must leave no socket behind"
        );
    }
}
