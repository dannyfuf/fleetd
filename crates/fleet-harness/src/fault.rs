//! Runner-side daemon and socket fault injection.
//!
//! The degraded surfaces — `Daemon > Down`, the 28 px reconnect banner, the recovered state —
//! are the ones nobody tests, because nothing in a normal run breaks. This module breaks
//! things on purpose.
//!
//! Fault injection belongs to the **runner** and to fleetd's own surface, never to Fleet's
//! command socket (`docs/TESTING-HARNESS.md` §2 lists `daemon …` and `socket remove` as runner
//! lines). The application must never grow a "make the daemon die" command: it learns that its
//! daemon is gone exactly the way it does in production, when the health probe in
//! `fleet-app/src/bridge/runtime.rs` stops being answered.
//!
//! Every fault returns only once Fleet has *observed* the consequence. Nothing here sleeps for
//! a fixed time and hopes: [`inject`] asks the application for its own snapshot until the
//! key-context chain reports the new link state, so a slow machine waits longer and a fast one
//! does not wait at all.

use crate::{client::Client, env::Daemon};
use anyhow::Context as _;
use fleet_core::paths::FleetHome;
use fleet_drive::protocol::{Command, DumpArgs};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::Command as Process,
    time::{Instant, sleep},
};

/// How long a fault waits for Fleet to notice it.
///
/// The application's worst case is a 2 s health tick, a 3 s health timeout, a connection
/// attempt that can spend 10 s waiting for a daemon that will never become ready, and then a
/// reconnect backoff that tops out at 8 s. This bound sits above that sum so a genuinely slow
/// observation is still an observation rather than a spurious failure.
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(45);
/// How often Fleet is asked what it can see while waiting.
///
/// This is a probe interval, not a delay: the loop returns on the first snapshot that shows
/// the new state, and this only bounds how often the question is asked.
const OBSERVE_INTERVAL: Duration = Duration::from_millis(50);
/// How long a signalled daemon is given to leave the process table.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the process table is checked while waiting for that exit.
const EXIT_INTERVAL: Duration = Duration::from_millis(5);
/// The name `dump` is given while a fault watches the link; it never reaches `dumps/`.
const OBSERVE_DUMP: &str = "daemon-fault";

/// The daemon faults the frozen grammar spells `daemon kill|stop|cont|restart`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonFault {
    /// `SIGKILL`, and the daemon is kept from coming back. Reaches the daemon-down surfaces.
    Kill,
    /// `SIGSTOP`. The process is still there and still holds its PID lock, so nothing restarts
    /// it; it simply stops answering, which is the stalled daemon of UX-SPEC §3.12 C.
    Stop,
    /// `SIGCONT`. The stalled daemon answers again and the banner clears.
    Continue,
    /// An orderly stop and a fresh, hermetic fleetd, so the application must recover.
    Restart,
}

impl DaemonFault {
    /// The word this fault is written with in a scenario file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kill => "kill",
            Self::Stop => "stop",
            Self::Continue => "cont",
            Self::Restart => "restart",
        }
    }

    /// What Fleet must end up showing before the scenario line is allowed to return.
    const fn expects(self) -> Expectation {
        match self {
            Self::Kill | Self::Stop => Expectation::Detached,
            Self::Continue | Self::Restart => Expectation::Attached,
        }
    }
}

/// What Fleet says about its daemon link.
///
/// The snapshot's `daemon.link` is the authority. The key-context chain is the fallback, for a
/// Fleet built before that field existed, and it is blind exactly where it matters:
/// `AppState::context_chain` appends `Daemon > Banner` only *behind* a base surface, so on a
/// first-run Fleet or behind an open overlay the banner does not appear in it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    /// Cold start: neither attached nor failed yet.
    Starting,
    /// §3.12 B, the full-window "fleetd could not start" surface, which owns every key.
    Down,
    /// §3.12 C, the reconnect banner, which owns `r`, `l` and `Esc` behind the base surface.
    Banner,
    /// No daemon surface owns keys.
    Attached,
}

/// The link states a fault is allowed to settle on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// Either daemon-failure surface: which one appears depends on whether the application was
    /// attached when the daemon went away.
    Detached,
    /// Neither: the daemon is back and Fleet is usable again.
    Attached,
}

impl Expectation {
    const fn satisfied_by(self, link: Link) -> bool {
        match self {
            Self::Detached => matches!(link, Link::Down | Link::Banner),
            Self::Attached => matches!(link, Link::Attached),
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            Self::Detached => "`Daemon > Down` or `Daemon > Banner`",
            Self::Attached => "a working daemon link",
        }
    }
}

/// Applies a daemon fault and returns once Fleet has seen its consequence.
///
/// This is the operation a `daemon …` scenario line performs. [`daemon`] is the runner half on
/// its own, for a caller that has no application to ask.
pub async fn inject(
    client: &mut Client,
    process: &mut Daemon,
    fault: DaemonFault,
) -> anyhow::Result<()> {
    daemon(process, fault).await?;
    if fault.expects() == Expectation::Attached {
        // Recovery is proved from both ends. A context chain that cannot show a daemon surface
        // at all — first run, or an open overlay — reports `Attached` even while the daemon is
        // dead, so the runner also insists that fleetd answers a ping of its own.
        process.wait_until_ready().await.with_context(|| {
            format!(
                "fleetd never answered again after `daemon {}`",
                fault.as_str()
            )
        })?;
    }
    observe(client, fault).await
}

/// Applies a daemon fault without waiting for the application to notice.
pub async fn daemon(process: &mut Daemon, fault: DaemonFault) -> anyhow::Result<()> {
    match fault {
        DaemonFault::Kill => kill(process).await,
        DaemonFault::Stop => signal(running_pid(process, "stop").await?, "STOP").await,
        DaemonFault::Continue => resume(process).await,
        DaemonFault::Restart => restart(process).await,
    }
}

/// Unlinks the daemon's socket file, which is the whole of `socket remove`.
///
/// Fleet observes nothing when the file goes: it is already connected through a socket it
/// opened, and the loss only surfaces the next time something tries to *reach* a daemon
/// through that path. So this returns as soon as the path is gone. A `socket remove` with no
/// socket to remove is a failure, not a no-op — a scenario must never believe it broke
/// something it did not.
pub async fn remove_socket(process: &Daemon) -> anyhow::Result<()> {
    let socket = &process.socket;
    anyhow::ensure!(
        fs::symlink_metadata(socket).is_ok(),
        "there is no daemon socket at {} to remove",
        socket.display()
    );
    tokio::fs::remove_file(socket)
        .await
        .with_context(|| format!("remove the daemon socket {}", socket.display()))
}

/// Lets a `SIGSTOP`ped daemon run again, whether or not one was ever stopped.
///
/// Teardown must call this before it stops anything: a scenario that ran `daemon stop` without
/// a matching `daemon cont` would otherwise leave a frozen fleetd behind, and an orderly
/// shutdown request cannot be answered by a process that is not running.
pub async fn resume(process: &Daemon) -> anyhow::Result<()> {
    // Nothing to continue is the same outcome as continuing something, so teardown can call
    // this unconditionally without having to know what the scenario did.
    match live_pid(process).await? {
        Some(pid) => signal(pid, "CONT").await,
        None => Ok(()),
    }
}

/// `SIGKILL`, then make sure the daemon stays dead.
async fn kill(process: &mut Daemon) -> anyhow::Result<()> {
    let pid = running_pid(process, "kill").await?;
    if process.pid() == Some(pid) {
        // The runner's own child is killed through its handle, which also reaps it. A zombie
        // still answers `kill -0`, and `fleet_client::live_process` would read it as a daemon
        // that is alive but unreachable.
        process
            .child_mut()
            .kill()
            .await
            .context("SIGKILL the private fleetd")?;
    } else {
        signal(pid, "KILL").await?;
        await_exit(pid).await?;
    }
    seal(process)?;
    // A killed daemon never reaches its singleton guard's `Drop`, so the socket it bound is
    // still on disk. Clearing it here is what a crashed daemon looks like by the time anything
    // else runs, and it keeps the runner's own teardown from reporting a socket it cannot
    // account for.
    remove_if_present(&process.socket)
}

/// An orderly stop followed by a fresh hermetic fleetd.
async fn restart(process: &mut Daemon) -> anyhow::Result<()> {
    // A frozen daemon cannot answer a shutdown request; continuing it first is what makes the
    // orderly path available instead of a second kill.
    resume(process).await?;
    // `Daemon::shutdown` is also how the runner's handle learns its process is gone. Only a
    // handle that knows it has been reaped leaves the socket path alone when it is dropped,
    // and the replacement below binds that same path — so the restart stops here if the
    // orderly stop did not complete.
    process
        .shutdown()
        .await
        .context("stop fleetd before restarting it")?;
    unseal(process)?;
    start_replacement(process).await
}

/// Starts the replacement daemon and hands it to the runner's handle.
///
/// Its output is appended to the hermetic home's own `logs/fleetd.out` rather than to the run
/// directory's `fleetd.log`, which belongs to the daemon that was stopped. The handle adopts the
/// child with [`Daemon::adopt`], so teardown and `Drop` cover the replacement exactly as they
/// covered the original — the socket is not the only way back to it.
async fn start_replacement(process: &mut Daemon) -> anyhow::Result<()> {
    let logs = process.home().join("logs");
    fs::create_dir_all(&logs).with_context(|| format!("create {}", logs.display()))?;
    let path = logs.join("fleetd.out");
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let errors = log.try_clone().context("clone the daemon log handle")?;
    let mut command = Process::new(process.binary());
    command
        .arg("--home")
        .arg(process.home())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    // The replacement is as hermetic as the daemon it replaces: private FLEET_HOME, child-only
    // HOME, and the fake network-facing executables first on PATH.
    process.environment().apply(&mut command);
    // `kill_on_drop` is the backstop under `Daemon::drop`'s own kill: a runner that is torn
    // down between the spawn and the adopt must not leave a daemon behind either.
    command.kill_on_drop(true);
    let child = command
        .spawn()
        .with_context(|| format!("restart {}", process.binary().display()))?;
    process.adopt(child);
    process
        .wait_until_ready()
        .await
        .context("the restarted fleetd never became ready")
}

/// Waits until Fleet's own snapshot reports the link state this fault asked for.
async fn observe(client: &mut Client, fault: DaemonFault) -> anyhow::Result<()> {
    let expected = fault.expects();
    let deadline = Instant::now() + OBSERVE_TIMEOUT;
    loop {
        let (link, evidence) = read_link(client).await?;
        if expected.satisfied_by(link) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "`daemon {}` was applied, but Fleet never showed {} within {OBSERVE_TIMEOUT:?}; \
                 it reports {evidence}",
                fault.as_str(),
                expected.describe(),
            );
        }
        sleep(OBSERVE_INTERVAL).await;
    }
}

/// Asks Fleet for one snapshot and classifies the daemon link it reports.
///
/// Returns the link and the evidence it was read from, so a timeout says what Fleet was
/// actually showing rather than only that it was not the expected thing.
async fn read_link(client: &mut Client) -> anyhow::Result<(Link, String)> {
    let response = client
        .send(Command::Dump(DumpArgs {
            name: OBSERVE_DUMP.to_owned(),
        }))
        .await
        .context("ask Fleet what it can see")?;
    anyhow::ensure!(
        response.ok,
        "Fleet could not report its state: {}",
        response
            .error
            .unwrap_or_else(|| "no reason given".to_owned())
    );
    let snapshot = response
        .data
        .get("snapshot")
        .context("the dump carried no snapshot")?;
    if let Some(reported) = snapshot
        .get("daemon")
        .and_then(|daemon| daemon.get("link"))
        .and_then(serde_json::Value::as_str)
    {
        return Ok((link_named(reported), format!("daemon.link = {reported:?}")));
    }
    // A Fleet older than the `daemon` field. The chain is all there is, and it cannot show a
    // banner on a first-run or overlay surface, so say so when it is the thing being read.
    let contexts = snapshot
        .get("key_contexts")
        .and_then(serde_json::Value::as_array)
        .context("the dump carried neither snapshot.daemon nor snapshot.key_contexts")?;
    let contexts: Vec<String> = contexts
        .iter()
        .map(|value| match value.as_str() {
            Some(word) => word.to_owned(),
            None => value.to_string(),
        })
        .collect();
    Ok((
        link_of(&contexts),
        format!(
            "no snapshot.daemon field, and key contexts [{}] — an older Fleet whose chain \
             shadows the reconnect banner on a first-run or overlay surface",
            contexts.join(", ")
        ),
    ))
}

/// Classifies the snapshot's own `daemon.link` word.
fn link_named(word: &str) -> Link {
    match word {
        "failed" => Link::Down,
        "lost" => Link::Banner,
        "connected" | "reconnected" => Link::Attached,
        // "starting", and anything a newer Fleet grows: neither attached nor detached, so a
        // fault keeps waiting rather than passing on a state it does not recognise.
        _ => Link::Starting,
    }
}

/// Classifies a key-context chain.
fn link_of(contexts: &[String]) -> Link {
    // `Daemon > Down` replaces the whole chain; the banner is appended behind the base surface.
    if contexts.iter().map(String::as_str).eq(["Daemon", "Down"]) {
        return Link::Down;
    }
    match contexts.split_last() {
        Some((last, rest))
            if last == "Banner" && rest.last().is_some_and(|word| word == "Daemon") =>
        {
            Link::Banner
        }
        _ => Link::Attached,
    }
}

/// Stops fleetd from being restarted behind the runner's back.
///
/// Fleet auto-starts a daemon whenever its socket is dead (`fleet_client::ensure_daemon`), so a
/// killed daemon comes straight back — as a process the runner never spawned, cannot signal
/// again and does not tear down, which would make `daemon kill` both invisible and leaky. A
/// directory where fleetd must open `fleetd.pid` makes every start attempt fail inside
/// `SingletonGuard::acquire` before it binds anything, which is the real "fleetd will not
/// start" of UX-SPEC §3.12 B. `daemon restart` lifts it; nothing else does.
fn seal(process: &Daemon) -> anyhow::Result<()> {
    let path = pid_path(process);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Ok(_) => remove_if_present(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!("inspect {}", path.display())));
        }
    }
    fs::create_dir(&path)
        .with_context(|| format!("keep fleetd from restarting by holding {}", path.display()))
}

/// Lifts [`seal`], so a daemon may start in this home again.
fn unseal(process: &Daemon) -> anyhow::Result<()> {
    let path = pid_path(process);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path)
            .with_context(|| format!("let fleetd start again by clearing {}", path.display())),
        Ok(_) | Err(_) => Ok(()),
    }
}

/// `<FLEET_HOME>/fleetd.pid`, the file `SingletonGuard` locks for a daemon's whole lifetime.
fn pid_path(process: &Daemon) -> PathBuf {
    FleetHome::new(process.home().to_owned()).pid_path()
}

/// The process id of the daemon that is actually serving this home.
///
/// The PID file is authoritative and the runner's own child is the fallback, because a
/// `daemon restart` leaves the handle holding a reaped child while a live daemon serves the
/// same home.
async fn live_pid(process: &Daemon) -> anyhow::Result<Option<u32>> {
    for pid in recorded_pid(process).into_iter().chain(process.pid()) {
        if is_alive(pid).await? {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

/// The daemon a breaking fault is aimed at, or a failure naming the fault that found none.
async fn running_pid(process: &Daemon, fault: &str) -> anyhow::Result<u32> {
    live_pid(process)
        .await
        .with_context(|| format!("look for the fleetd `daemon {fault}` must signal"))?
        .with_context(|| {
            format!(
                "`daemon {fault}` needs a running fleetd; none is serving {}",
                process.home().display()
            )
        })
}

/// Reads the PID `SingletonGuard::acquire` wrote, if the file is readable and holds one.
///
/// The format is the one `crates/fleet-daemon/src/server/listener.rs` writes: the decimal
/// process id and a newline. A sealed home has a directory here and reads as no PID at all.
fn recorded_pid(process: &Daemon) -> Option<u32> {
    fs::read_to_string(pid_path(process))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether a process id still names a live process.
async fn is_alive(pid: u32) -> anyhow::Result<bool> {
    Ok(run_kill(pid, "0").await?.success())
}

/// Sends one signal, failing when the process is not there to receive it.
async fn signal(pid: u32, name: &str) -> anyhow::Result<()> {
    let status = run_kill(pid, name).await?;
    anyhow::ensure!(status.success(), "kill -{name} {pid} failed with {status}");
    Ok(())
}

/// Runs `kill(1)`.
///
/// `fleet-harness` does not depend on `libc` and its manifest belongs to another stage, so
/// signalling goes through the POSIX utility every platform this harness runs on already
/// ships. The runner's own `PATH` is used deliberately: the hermetic `PATH` exists to shadow
/// *network-facing* tools from child processes, and this call is the runner's own.
async fn run_kill(pid: u32, signal: &str) -> anyhow::Result<std::process::ExitStatus> {
    Process::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| format!("send -{signal} to {pid} with kill(1)"))
}

/// Waits for a signalled process to leave the process table.
async fn await_exit(pid: u32) -> anyhow::Result<()> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if !is_alive(pid).await? {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "fleetd {pid} was still alive {EXIT_TIMEOUT:?} after SIGKILL"
        );
        sleep(EXIT_INTERVAL).await;
    }
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

    fn chain(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_snapshot_s_own_link_word_decides_the_surface() {
        assert_eq!(link_named("failed"), Link::Down);
        assert_eq!(link_named("lost"), Link::Banner);
        assert_eq!(link_named("connected"), Link::Attached);
        assert_eq!(link_named("reconnected"), Link::Attached);
        // Cold start is neither, so a fault waits instead of passing on it.
        assert_eq!(link_named("starting"), Link::Starting);
        assert!(!Expectation::Detached.satisfied_by(Link::Starting));
        assert!(!Expectation::Attached.satisfied_by(Link::Starting));
        // A word this build does not know is treated the same way, never as success.
        assert_eq!(link_named("quiesced"), Link::Starting);
    }

    #[test]
    fn the_context_chain_names_both_daemon_failure_surfaces() {
        assert_eq!(link_of(&chain(&["Daemon", "Down"])), Link::Down);
        assert_eq!(
            link_of(&chain(&["Hub", "Worktrees", "Daemon", "Banner"])),
            Link::Banner
        );
        assert_eq!(
            link_of(&chain(&["Workspace", "Terminal", "Daemon", "Banner"])),
            Link::Banner,
            "the banner is appended behind whatever base surface owns the screen"
        );
        assert_eq!(
            link_of(&chain(&["Hub", "Daemon", "Banner", "Worktrees"])),
            Link::Attached,
            "only a `Daemon > Banner` pair at the tail of the chain is the banner"
        );
        assert_eq!(
            link_of(&chain(&["Daemon", "Down", "Doctor"])),
            Link::Attached,
            "`Daemon > Down` replaces the whole chain, so a longer chain is a different surface"
        );
    }

    #[test]
    fn a_chain_with_no_daemon_surface_reads_as_attached() {
        for words in [
            &["Hub", "Worktrees"][..],
            &["FirstRun"][..],
            &["Palette"][..],
            &["Dialog", "Quit"][..],
            &[][..],
        ] {
            assert_eq!(
                link_of(&chain(words)),
                Link::Attached,
                "{words:?} shows no daemon surface"
            );
        }
    }

    #[test]
    fn a_banner_at_the_tail_is_the_banner_wherever_the_base_surface_ends() {
        assert_eq!(
            link_of(&chain(&["Agent", "Terminal", "Daemon", "Banner"])),
            Link::Banner
        );
        assert_eq!(
            link_of(&chain(&["Banner"])),
            Link::Attached,
            "`Banner` alone is not a daemon context"
        );
    }

    #[test]
    fn breaking_faults_wait_for_a_failure_surface_and_recovering_ones_for_a_working_link() {
        for fault in [DaemonFault::Kill, DaemonFault::Stop] {
            assert_eq!(fault.expects(), Expectation::Detached, "{fault:?}");
            assert!(fault.expects().satisfied_by(Link::Down));
            assert!(fault.expects().satisfied_by(Link::Banner));
            assert!(!fault.expects().satisfied_by(Link::Attached));
        }
        for fault in [DaemonFault::Continue, DaemonFault::Restart] {
            assert_eq!(fault.expects(), Expectation::Attached, "{fault:?}");
            assert!(fault.expects().satisfied_by(Link::Attached));
            assert!(!fault.expects().satisfied_by(Link::Down));
        }
    }

    #[test]
    fn every_fault_is_spelled_the_way_the_grammar_writes_it() {
        assert_eq!(
            [
                DaemonFault::Kill,
                DaemonFault::Stop,
                DaemonFault::Continue,
                DaemonFault::Restart,
            ]
            .map(DaemonFault::as_str),
            ["kill", "stop", "cont", "restart"]
        );
    }

    /// The seal must survive the PID file fleetd leaves behind, and must be reversible, or a
    /// `daemon kill` would make its own home permanently unusable.
    #[test]
    fn sealing_a_home_replaces_its_pid_file_and_unsealing_gives_it_back() {
        let home = tempfile::tempdir().expect("temporary home");
        let pid_file = FleetHome::new(home.path().to_owned()).pid_path();
        fs::write(&pid_file, "12345\n").expect("write a PID file");

        let path = pid_file.clone();
        let seal_once = || {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => return,
                Ok(_) => fs::remove_file(&path).expect("clear the PID file"),
                Err(_) => {}
            }
            fs::create_dir(&path).expect("seal the home");
        };
        seal_once();
        assert!(
            pid_file.is_dir(),
            "a sealed home holds a directory where fleetd must open its PID file"
        );
        seal_once();
        assert!(pid_file.is_dir(), "sealing twice is not an error");

        fs::remove_dir_all(&pid_file).expect("unseal");
        assert!(!pid_file.exists(), "an unsealed home is writable again");
        assert!(
            fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&pid_file)
                .is_ok(),
            "fleetd's singleton guard must be able to open the PID file again"
        );
    }

    /// Stop, continue and kill are the three signals every daemon fault is built from, so they
    /// are tested against a real process rather than against a daemon.
    #[tokio::test]
    async fn signals_stop_continue_and_kill_a_real_process() {
        let mut child = Process::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn a process to signal");
        let pid = child.id().expect("a freshly spawned process has an id");

        assert!(is_alive(pid).await.expect("probe"), "it is running");
        signal(pid, "STOP").await.expect("stop it");
        assert!(
            is_alive(pid).await.expect("probe"),
            "a stopped process is still in the process table"
        );
        signal(pid, "CONT").await.expect("continue it");

        child.kill().await.expect("kill it");
        await_exit(pid).await.expect("it left the process table");
        assert!(
            signal(pid, "CONT").await.is_err(),
            "signalling a process that is gone is a failure, not a silent success"
        );
    }
}
