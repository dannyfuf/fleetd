//! The holder process: it owns one PTY and relays it over a per-terminal Unix socket.
//!
//! Everything here runs in the detached `fleetd pty-hold` process, never in the daemon. Three
//! threads share one [`Holder`]: the caller's thread pumps PTY output into the replay buffer and
//! the live connection, an accept thread installs at most one daemon connection at a time, and a
//! per-connection thread applies the daemon's requests to the PTY.

use std::{
    fs, io,
    net::Shutdown,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use super::{
    protocol::{HolderEvent, HolderRequest, MAX_FRAME_BYTES},
    replay::ReplayBuffer,
};
use crate::pty::{Pty, PtyError, PtyOptions};

/// Default byte tail a holder replays to a reattaching daemon.
///
/// One mebibyte is roughly a full screen of dense output several hundred times over, which is what
/// a restarting daemon needs to rebuild a usable viewport; the daemon's own scrollback budget
/// (`terminal.scrollbackBytes`) governs history from that point on.
pub const REPLAY_BUFFER_BYTES: usize = 1024 * 1024;

/// Backstop between PTY polls when no readiness notification arrives.
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Pause after an `accept(2)` failure so a broken listener cannot spin this thread.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
/// Bytes drained from the PTY before the pump checks the child again.
///
/// Without a ceiling a chatty child starves the exit check, and a terminal that has already
/// finished keeps a holder alive while its output drains.
const OUTPUT_BATCH_BYTES: usize = 256 * 1024;
/// How long a holder whose child exited *unasked* waits for a daemon to come back and be told.
///
/// Only the exit report is at stake. A holder that was asked to die does not linger, and one whose
/// report was delivered returns immediately.
const EXIT_LINGER: Duration = Duration::from_secs(30);
/// Poll interval while lingering.
const EXIT_LINGER_POLL: Duration = Duration::from_millis(50);
/// Mode for the socket and the directory holding it: this is a control channel into a login shell.
const PRIVATE_MODE: u32 = 0o700;
/// Mode for the socket itself; nobody but this user may connect.
const SOCKET_MODE: u32 = 0o600;

/// Construction settings for one holder process.
#[derive(Debug, Clone)]
pub struct HolderOptions {
    /// Unix socket this holder binds and removes on exit.
    pub socket: PathBuf,
    /// Sidecar record removed alongside the socket, when the daemon wrote one.
    pub sidecar: Option<PathBuf>,
    /// PTY child settings.
    pub pty: PtyOptions,
    /// Replay tail budget; clamped to the protocol's frame limit.
    pub replay_bytes: usize,
}

/// Failure while starting or running a holder process.
#[derive(Debug, Error)]
pub enum HolderError {
    /// The holder could not create, bind or clean up its socket.
    #[error("holder socket {path}: {source}")]
    Socket {
        /// Path the operation was attempted on.
        path: PathBuf,
        /// Underlying filesystem or socket failure.
        #[source]
        source: io::Error,
    },
    /// The PTY child could not be started.
    #[error(transparent)]
    Pty(#[from] PtyError),
    /// The operating system could not create a holder thread.
    #[error("failed to start holder thread: {0}")]
    Thread(#[source] io::Error),
}

impl HolderError {
    fn socket(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Socket {
            path: path.into(),
            source,
        }
    }
}

/// Runs one holder until its child exits, returning the child's status code when observed.
///
/// The socket and the sidecar are removed before returning, so the daemon's startup scan only
/// ever finds records whose holder is still alive or whose holder died without unwinding.
pub fn run(options: HolderOptions) -> Result<Option<i32>, HolderError> {
    let socket = options.socket.clone();
    if let Some(parent) = socket.parent() {
        prepare_private_directory(parent)?;
    }
    // Socket names carry a per-spawn nonce, so an existing one is another holder's, not a stale
    // copy of this terminal's. Refusing to bind is the only safe answer.
    let listener =
        UnixListener::bind(&socket).map_err(|source| HolderError::socket(&socket, source))?;
    restrict(&socket, SOCKET_MODE)?;
    // Identified through the same call teardown uses, so the two can only disagree about a file
    // that genuinely changed underneath this holder.
    let bound = FileIdentity::of_path(&socket)
        .ok_or_else(|| HolderError::socket(&socket, io::Error::from(io::ErrorKind::NotFound)))?;

    let wake = Arc::new(Wakeup::default());
    let notify = Arc::clone(&wake);
    let pty = Pty::spawn_notifying(options.pty, Arc::new(move || notify.notify()))?;
    let child_pid = pty.child_pid();
    // A replay larger than one frame could never be delivered; refuse to retain what cannot be sent.
    let replay_bytes = options.replay_bytes.min(MAX_FRAME_BYTES);
    let holder = Arc::new(Holder {
        pty: Mutex::new(pty),
        sink: Mutex::new(Sink::new(replay_bytes, child_pid)),
        kill_requested: AtomicBool::new(false),
    });

    let accepting = Arc::clone(&holder);
    thread::Builder::new()
        .name("fleet-pty-hold-accept".to_owned())
        .spawn(move || accept_connections(&listener, &accepting))
        .map_err(HolderError::Thread)?;

    tracing::info!(
        socket = %socket.display(),
        child_pid,
        replay_bytes,
        "pty holder started"
    );
    let code = holder.pump(&wake);
    holder.deliver_exit(code);
    // Only ever unlink the socket this holder is still bound to. A successor that reused the name
    // would otherwise lose its control channel to a predecessor's teardown.
    remove_if_still_ours(&socket, bound);
    if let Some(sidecar) = options.sidecar.as_deref() {
        remove_quietly(sidecar);
    }
    tracing::info!(code, "pty holder stopped");
    Ok(code)
}

/// The PTY, the replay tail and the at-most-one daemon connection, shared by the holder's threads.
struct Holder {
    pty: Mutex<Pty>,
    sink: Mutex<Sink>,
    kill_requested: AtomicBool,
}

impl Holder {
    fn pty(&self) -> MutexGuard<'_, Pty> {
        self.pty.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sink(&self) -> MutexGuard<'_, Sink> {
        self.sink.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the size the kernel holds for the PTY, which the greeting must report.
    fn window_size(&self) -> Option<(u16, u16)> {
        match self.pty().window_size() {
            Ok(size) => Some(size),
            Err(error) => {
                tracing::warn!(%error, "holder could not read its window size");
                None
            }
        }
    }

    /// Asks the terminal's foreground job to repaint for a daemon that just took over.
    fn request_repaint(&self) {
        if let Err(error) = self.pty().request_repaint() {
            tracing::warn!(%error, "holder could not ask its terminal to repaint");
        }
    }

    /// Relays PTY output until the child has exited and its output is drained.
    fn pump(&self, wake: &Wakeup) -> Option<i32> {
        loop {
            if let Err(error) = self.drain_output() {
                tracing::warn!(%error, "holder PTY reader failed");
                self.kill();
                return self.observe_exit();
            }
            let exit = self.observe_exit();
            if exit.is_some() && self.pty().output_closed() {
                return exit;
            }
            wake.wait(OUTPUT_POLL_INTERVAL);
        }
    }

    fn drain_output(&self) -> Result<(), PtyError> {
        let mut drained = 0;
        while drained < OUTPUT_BATCH_BYTES {
            // Two statements, deliberately: publishing under the PTY lock would stall daemon input
            // behind a socket write for as long as the daemon takes to read.
            let chunk = self.pty().try_read()?;
            let Some(chunk) = chunk else {
                return Ok(());
            };
            drained = drained.saturating_add(chunk.len());
            self.sink().publish(&chunk);
        }
        Ok(())
    }

    fn observe_exit(&self) -> Option<i32> {
        match self.pty().try_wait() {
            Ok(exit) => exit,
            Err(error) => {
                tracing::warn!(%error, "holder failed to observe its child's exit");
                None
            }
        }
    }

    fn kill(&self) {
        self.kill_requested.store(true, Ordering::Release);
        if let Err(error) = self.pty().kill() {
            tracing::warn!(%error, "holder failed to kill its child");
        }
    }

    /// Publishes the child's exit, waiting for a reconnecting daemon when nobody heard it.
    fn deliver_exit(&self, code: Option<i32>) {
        {
            let mut sink = self.sink();
            sink.exit = Some(code);
            let delivered = sink.send(&HolderEvent::Exited(code));
            sink.exit_delivered |= delivered;
        }
        // A daemon that asked for the kill already knows. Otherwise a daemon restarting at this
        // exact moment has no connection to be told on, so hold the socket open a little longer.
        if self.kill_requested.load(Ordering::Acquire) {
            return;
        }
        let deadline = Instant::now() + EXIT_LINGER;
        while Instant::now() < deadline {
            if self.sink().exit_delivered {
                return;
            }
            thread::sleep(EXIT_LINGER_POLL);
        }
    }
}

/// The ordered view a daemon connection has of the child's output.
///
/// One mutex covers the replay buffer and the connection together, which is what makes "the
/// replay tail, then live output, with nothing lost or reordered" true across a reattach.
struct Sink {
    replay: ReplayBuffer,
    connection: Option<UnixStream>,
    generation: u64,
    child_pid: Option<u32>,
    exit: Option<Option<i32>>,
    exit_delivered: bool,
}

impl Sink {
    fn new(replay_bytes: usize, child_pid: Option<u32>) -> Self {
        Self {
            replay: ReplayBuffer::new(replay_bytes),
            connection: None,
            generation: 0,
            child_pid,
            exit: None,
            exit_delivered: false,
        }
    }

    fn publish(&mut self, bytes: &[u8]) {
        self.replay.push(bytes);
        self.send(&HolderEvent::Output(bytes.to_vec()));
    }

    /// Installs a connection in place of any predecessor and greets it with the replay tail.
    fn attach(&mut self, stream: UnixStream, size: Option<(u16, u16)>) -> u64 {
        self.close_connection();
        self.generation = self.generation.wrapping_add(1);
        self.connection = Some(stream);
        // A size the kernel would not report leaves the greeting at zero, which the daemon reads
        // as "ask me again" rather than as a grid to build an emulator at.
        let (cols, rows) = size.unwrap_or((0, 0));
        self.send(&HolderEvent::Hello {
            child_pid: self.child_pid,
            cols,
            rows,
        });
        let replay = self.replay.snapshot();
        tracing::info!(
            generation = self.generation,
            replay_bytes = replay.len(),
            "pty holder attached a daemon"
        );
        if !replay.is_empty() {
            self.send(&HolderEvent::Output(replay));
        }
        if let Some(code) = self.exit {
            let delivered = self.send(&HolderEvent::Exited(code));
            self.exit_delivered |= delivered;
        }
        self.generation
    }

    /// Drops the connection of `generation`, leaving a newer one in place.
    fn detach(&mut self, generation: u64) {
        if self.generation == generation {
            self.close_connection();
        }
    }

    /// Writes one event to the live connection, blocking while the daemon catches up.
    ///
    /// The write has no timeout on purpose. A daemon that has fallen behind is applying
    /// backpressure, exactly as a directly owned PTY does when its reader queue fills, and
    /// mistaking that for a dead peer would end a terminal the user is still using. Only a real
    /// failure — the daemon's socket closed — retires the connection, and even then the holder and
    /// its child live on for the next daemon.
    fn send(&mut self, event: &HolderEvent) -> bool {
        let Some(stream) = self.connection.as_mut() else {
            return false;
        };
        match event.write_to(stream) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "holder lost its daemon connection while writing");
                self.close_connection();
                false
            }
        }
    }

    fn close_connection(&mut self) {
        let Some(stream) = self.connection.take() else {
            return;
        };
        // The retired connection's reader thread is blocked on a read this holder will never
        // answer; shutting the socket down is what ends it.
        if let Err(error) = stream.shutdown(Shutdown::Both) {
            tracing::debug!(%error, "holder closed an already broken daemon connection");
        }
    }
}

fn accept_connections(listener: &UnixListener, holder: &Arc<Holder>) {
    loop {
        let stream = match listener.accept() {
            Ok((stream, _address)) => stream,
            Err(error) => {
                tracing::warn!(%error, "holder failed to accept a daemon connection");
                thread::sleep(ACCEPT_BACKOFF);
                continue;
            }
        };
        let reader = match stream.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                tracing::warn!(%error, "holder could not split a daemon connection");
                continue;
            }
        };
        // Read before taking the sink lock: the greeting must carry the size, and this call locks
        // the PTY, which a blocked `send` must never be waiting behind.
        let size = holder.window_size();
        let generation = holder.sink().attach(stream, size);
        let serving = Arc::clone(holder);
        if let Err(error) = thread::Builder::new()
            .name(format!("fleet-pty-hold-{generation}"))
            .spawn(move || serve_connection(reader, &serving, generation))
        {
            tracing::warn!(%error, "holder could not start a connection thread");
            holder.sink().detach(generation);
            continue;
        }
        // After the replay, not before: the daemon has an emulator at the right size and an empty
        // screen, and a full-screen TUI only fills it when something asks.
        holder.request_repaint();
    }
}

fn serve_connection(mut reader: UnixStream, holder: &Arc<Holder>, generation: u64) {
    loop {
        match HolderRequest::read_from(&mut reader) {
            Ok(Some(HolderRequest::Input(bytes))) => {
                if let Err(error) = holder.pty().write(&bytes) {
                    tracing::warn!(%error, "holder failed to write terminal input");
                }
            }
            Ok(Some(HolderRequest::Resize { cols, rows })) => {
                if let Err(error) = holder.pty().resize(cols, rows) {
                    tracing::warn!(%error, "holder failed to resize its PTY");
                }
            }
            Ok(Some(HolderRequest::Kill)) => {
                holder.kill();
                // Deliberately not detached: the daemon that asked for the kill is still waiting
                // to be told the exit status on this very connection.
                return;
            }
            Ok(Some(HolderRequest::Detach)) | Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "holder connection ended with an error");
                break;
            }
        }
    }
    holder.sink().detach(generation);
}

/// Coalesced readiness: PTY output and child exit both wake the pump.
#[derive(Default)]
struct Wakeup {
    ready: Mutex<bool>,
    signal: Condvar,
}

impl Wakeup {
    fn notify(&self) {
        let mut ready = self.ready.lock().unwrap_or_else(PoisonError::into_inner);
        *ready = true;
        self.signal.notify_all();
    }

    fn wait(&self, timeout: Duration) {
        let mut ready = self.ready.lock().unwrap_or_else(PoisonError::into_inner);
        if !*ready {
            let (guard, _elapsed) = self
                .signal
                .wait_timeout(ready, timeout)
                .unwrap_or_else(PoisonError::into_inner);
            ready = guard;
        }
        *ready = false;
    }
}

/// Device and inode of a file, so a path can be checked against the object it named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of_path(path: &Path) -> Option<Self> {
        let metadata = fs::symlink_metadata(path).ok()?;
        Some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

/// Creates or validates the directory a holder binds inside, refusing anything world-reachable.
///
/// The socket is a control channel into a login shell, and under a shared temporary directory the
/// parent is the only thing standing between that shell and another local user.
fn prepare_private_directory(directory: &Path) -> Result<(), HolderError> {
    match fs::create_dir_all(directory) {
        Ok(()) => {}
        Err(error) => return Err(HolderError::socket(directory, error)),
    }
    restrict(directory, PRIVATE_MODE)?;
    let metadata =
        fs::metadata(directory).map_err(|error| HolderError::socket(directory, error))?;
    // SAFETY: `geteuid` reads this process's own identity and cannot fail.
    let us = unsafe { libc::geteuid() };
    if metadata.uid() != us || metadata.permissions().mode() & 0o777 != PRIVATE_MODE {
        return Err(HolderError::socket(
            directory,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "holder directory must be owned by this user with mode 0700",
            ),
        ));
    }
    Ok(())
}

fn restrict(path: &Path, mode: u32) -> Result<(), HolderError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| HolderError::socket(path, error))
}

/// Unlinks a socket only while the path still names the object this holder bound.
fn remove_if_still_ours(path: &Path, bound: FileIdentity) {
    if FileIdentity::of_path(path) == Some(bound) {
        remove_quietly(path);
        return;
    }
    tracing::warn!(
        path = %path.display(),
        "leaving a holder socket that now belongs to another holder"
    );
}

fn remove_quietly(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "holder could not remove its runtime file");
        }
    }
}
