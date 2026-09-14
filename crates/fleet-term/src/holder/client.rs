//! The daemon-side [`crate::PtyBackend`] that drives a detached holder over its socket.

use std::{
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use fleet_core::ids::TerminalId;

use super::protocol::{HolderEvent, HolderRequest, ProtocolError};
use crate::pty::{OutputQueue, PtyBackend, PtyError, PtyWritePermit, ReaderMessage};

/// Ceiling on the holder's greeting, so a wedged holder fails the attach instead of the daemon.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a daemon could not take over a holder.
///
/// The two cases call for opposite remedies, which is why they are distinguished here rather than
/// flattened into one message: an unreachable holder may simply be slow and is worth retrying on
/// the next start, while one speaking another protocol will never be reachable by this build and
/// must be stopped instead of left holding a shell nothing can reach.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// The holder answered with a protocol version this build does not speak.
    #[error("{0}")]
    IncompatibleVersion(#[source] ProtocolError),
    /// The holder could not be reached, or did not greet in time.
    #[error(transparent)]
    Unreachable(#[from] PtyError),
}

/// A terminal child owned by a detached holder process.
///
/// Every operation is one framed request on the holder's socket, and output arrives on a reader
/// thread that feeds the same bounded queue a local [`crate::Pty`] uses — so the terminal host
/// cannot tell the two apart, and a daemon restart is invisible to everything above this type.
pub struct HolderPty {
    terminal: TerminalId,
    writer: Mutex<UnixStream>,
    child_pid: Option<u32>,
    /// Window size the holder reported, which is the size its replay tail was produced at.
    grid: (u16, u16),
    output: Arc<OutputQueue>,
    exit: Arc<Mutex<ExitState>>,
}

/// What the reader thread learned about the child's fate.
#[derive(Default)]
struct ExitState {
    /// The holder reported an exit; the inner option is the status code when it had one.
    reported: Option<Option<i32>>,
    /// The connection ended without an exit report, so the holder itself is gone.
    lost: bool,
    /// The daemon asked for this connection to end and wants no exit reported.
    detached: bool,
}

impl HolderPty {
    /// Connects to a holder's socket and starts relaying its output.
    ///
    /// `notify` is the terminal owner's readiness wakeup, called exactly as a local PTY's reader
    /// thread calls it.
    pub fn attach(
        socket: &Path,
        terminal: TerminalId,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, AttachError> {
        let stream = UnixStream::connect(socket).map_err(|error| {
            PtyError::Setup(format!(
                "failed to reach the holder at {}: {error}",
                socket.display()
            ))
        })?;
        let mut greeting = stream.try_clone().map_err(|error| {
            PtyError::Setup(format!("failed to split the holder socket: {error}"))
        })?;
        greeting
            .set_read_timeout(Some(HELLO_TIMEOUT))
            .map_err(|error| {
                PtyError::Setup(format!("failed to bound the holder greeting: {error}"))
            })?;
        let (child_pid, cols, rows) = match HolderEvent::read_from(&mut greeting) {
            Ok(Some(HolderEvent::Hello {
                child_pid,
                cols,
                rows,
            })) => (child_pid, cols, rows),
            // Never widened to print the frame: anything else here is terminal output.
            Ok(_unexpected) => {
                return Err(PtyError::Setup(format!(
                    "holder at {} did not greet this daemon",
                    socket.display()
                ))
                .into());
            }
            Err(error @ ProtocolError::UnsupportedVersion { .. }) => {
                return Err(AttachError::IncompatibleVersion(error));
            }
            Err(error) => {
                return Err(PtyError::Setup(format!(
                    "holder at {} failed its greeting: {error}",
                    socket.display()
                ))
                .into());
            }
        };
        // Live output must block, not time out: the greeting deadline was only for the handshake.
        greeting.set_read_timeout(None).map_err(|error| {
            PtyError::Setup(format!("failed to restore the holder socket: {error}"))
        })?;

        let output = Arc::new(OutputQueue::default());
        let exit = Arc::new(Mutex::new(ExitState::default()));
        let reader_output = Arc::clone(&output);
        let reader_exit = Arc::clone(&exit);
        thread::Builder::new()
            .name(format!("fleet-holder-reader-{terminal}"))
            .spawn(move || read_events(greeting, &reader_output, &reader_exit, notify.as_ref()))
            .map_err(PtyError::Io)?;

        Ok(Self {
            terminal,
            writer: Mutex::new(stream),
            child_pid,
            grid: (cols, rows),
            output,
            exit,
        })
    }

    /// Returns the window size the holder reported in its greeting.
    ///
    /// A reattaching daemon builds its emulator at exactly this size: the replay it is about to
    /// receive was produced for this grid, and the child is still running at it.
    #[must_use]
    pub fn grid(&self) -> (u16, u16) {
        self.grid
    }

    /// Connects to a holder only to stop it.
    ///
    /// A daemon that failed to build a terminal host around a holder it just started has no
    /// backend to kill through, and the shell behind that holder would otherwise run forever.
    pub fn stop(socket: &Path) -> Result<(), PtyError> {
        let mut stream = UnixStream::connect(socket).map_err(|error| {
            PtyError::Setup(format!(
                "failed to reach the holder at {} to stop it: {error}",
                socket.display()
            ))
        })?;
        HolderRequest::Kill
            .write_to(&mut stream)
            .map_err(|error| PtyError::Writer(format!("failed to stop a holder: {error}")))
    }

    fn exit(&self) -> std::sync::MutexGuard<'_, ExitState> {
        self.exit.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends one request, blocking only as long as the holder takes to drain its socket.
    ///
    /// Deliberately inline rather than queued on a writer thread: the holder's connection reader
    /// does nothing but forward to the PTY's own writer queue, so the only wait here is the
    /// millisecond-scale one for socket buffer space. A frame is written whole under this lock —
    /// a timeout mid-frame would desynchronise the stream, which is worse than the wait.
    fn request(&self, request: &HolderRequest) -> Result<(), PtyError> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        request.write_to(&mut *writer).map_err(|error| {
            PtyError::Writer(format!(
                "terminal {} lost its holder connection: {error}",
                self.terminal
            ))
        })
    }

    /// Closes the socket so the reader thread ends and the holder retires this connection.
    fn close(&self) {
        let writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = writer.shutdown(Shutdown::Both) {
            tracing::debug!(%error, terminal = %self.terminal, "holder connection was already closed");
        }
    }
}

impl PtyBackend for HolderPty {
    fn write(&self, bytes: &[u8]) -> Result<(), PtyError> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.request(&HolderRequest::Input(bytes.to_vec()))
    }

    fn write_permitted(&self, bytes: &[u8], permit: PtyWritePermit) -> Result<(), PtyError> {
        // The write completes inside this call, so the reservation is released by dropping it here
        // rather than by an asynchronous writer queue.
        let result = PtyBackend::write(self, bytes);
        drop(permit);
        result
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        if cols == 0 || rows == 0 {
            return Err(PtyError::InvalidSize { cols, rows });
        }
        self.request(&HolderRequest::Resize { cols, rows })
    }

    fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    fn try_wait(&mut self) -> Result<Option<i32>, PtyError> {
        let exit = self.exit();
        match (exit.reported, exit.lost) {
            (Some(Some(code)), _) => Ok(Some(code)),
            (Some(None), _) => Err(PtyError::Reader(format!(
                "holder for terminal {} reported an unknown exit status",
                self.terminal
            ))),
            (None, true) => Err(PtyError::Reader(format!(
                "holder for terminal {} stopped without reporting an exit status",
                self.terminal
            ))),
            (None, false) => Ok(None),
        }
    }

    fn kill(&mut self) -> Result<(), PtyError> {
        {
            let exit = self.exit();
            if exit.reported.is_some() || exit.lost || exit.detached {
                return Ok(());
            }
        }
        self.request(&HolderRequest::Kill)
    }

    fn detach(&mut self) -> Result<(), PtyError> {
        {
            let mut exit = self.exit();
            if exit.detached {
                return Ok(());
            }
            exit.detached = true;
        }
        // Best effort: a holder that already went away needs no notice, and the child it held is
        // gone with it either way.
        if let Err(error) = self.request(&HolderRequest::Detach) {
            tracing::debug!(%error, terminal = %self.terminal, "holder was already gone at detach");
        }
        self.close();
        Ok(())
    }

    fn try_read(&self) -> Result<Option<Vec<u8>>, PtyError> {
        self.output.pop()
    }

    fn has_output(&self) -> bool {
        !self.output.is_empty()
    }

    fn output_closed(&self) -> bool {
        self.output.is_closed_and_empty()
    }
}

impl Drop for HolderPty {
    fn drop(&mut self) {
        // Releasing the handle must never end the child: that is the whole point of a holder.
        self.close();
    }
}

fn read_events(
    mut reader: UnixStream,
    output: &Arc<OutputQueue>,
    exit: &Arc<Mutex<ExitState>>,
    notify: &(dyn Fn() + Send + Sync),
) {
    loop {
        match HolderEvent::read_from(&mut reader) {
            Ok(Some(HolderEvent::Output(bytes))) => {
                if !output.push(ReaderMessage::Data(bytes)) {
                    break;
                }
                notify();
            }
            Ok(Some(HolderEvent::Exited(code))) => {
                exit.lock().unwrap_or_else(PoisonError::into_inner).reported = Some(code);
                break;
            }
            // The greeting is consumed by `attach`; a second one is a holder bug, not a reason to
            // tear down a working terminal.
            Ok(Some(HolderEvent::Hello { .. })) => {
                tracing::warn!("holder repeated its greeting on a live connection");
            }
            Ok(None) => {
                mark_lost(exit);
                break;
            }
            Err(error) => {
                let detached = exit.lock().unwrap_or_else(PoisonError::into_inner).detached;
                if !detached {
                    tracing::warn!(%error, "holder connection failed");
                    // Recorded before the error is published, so the terminal owner that pops it
                    // already knows the holder is gone and does not try to kill through it.
                    mark_lost(exit);
                    output.push(ReaderMessage::Error(error.to_string()));
                }
                break;
            }
        }
    }
    output.close();
    notify();
}

fn mark_lost(exit: &Arc<Mutex<ExitState>>) {
    let mut exit = exit.lock().unwrap_or_else(PoisonError::into_inner);
    if exit.reported.is_none() && !exit.detached {
        exit.lost = true;
    }
}
