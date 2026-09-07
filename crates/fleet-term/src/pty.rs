//! Portable PTY creation, process control, resize, input, and output plumbing.

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use async_channel::{Receiver, TryRecvError};
use fleet_core::ids::TerminalId;
use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system,
};
use thiserror::Error;

const TERM: &str = "xterm-256color";
const IO_CHUNK_BYTES: usize = 16 * 1024;
const PTY_OUTPUT_QUEUE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const PTY_WRITE_QUEUE_BYTES: usize = 4 * 1024 * 1024;
const OUTPUT_MESSAGE_OVERHEAD: usize = 64;
const WRITE_MESSAGE_OVERHEAD: usize = 64;

/// Process and environment settings used to create a PTY.
#[derive(Debug, Clone)]
pub struct PtyOptions {
    /// Executable launched on the PTY slave.
    pub program: OsString,
    /// Arguments following the executable.
    pub args: Vec<OsString>,
    /// Initial working directory.
    pub cwd: PathBuf,
    /// Environment overrides supplied to the child.
    pub env: Vec<(OsString, OsString)>,
    /// Initial grid width.
    pub cols: u16,
    /// Initial grid height.
    pub rows: u16,
}

impl PtyOptions {
    /// Creates a login shell with Fleet's backend-independent terminal environment.
    #[must_use]
    pub fn shell(
        cwd: impl Into<PathBuf>,
        session: impl AsRef<OsStr>,
        terminal: impl AsRef<OsStr>,
        terminal_id: TerminalId,
        cols: u16,
        rows: u16,
    ) -> Self {
        let program = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/zsh"));
        Self {
            program,
            args: vec![OsString::from("-l")],
            cwd: cwd.into(),
            env: vec![
                (OsString::from("TERM"), OsString::from(TERM)),
                (OsString::from("COLORTERM"), OsString::from("truecolor")),
                (OsString::from("FLEET_SESSION"), session.as_ref().to_owned()),
                (
                    OsString::from("FLEET_TERMINAL"),
                    terminal.as_ref().to_owned(),
                ),
                (
                    OsString::from("FLEET_TERMINAL_ID"),
                    OsString::from(terminal_id.to_string()),
                ),
            ],
            cols,
            rows,
        }
    }

    /// Creates options for an arbitrary command.
    pub fn command<I, S>(
        program: impl Into<OsString>,
        args: I,
        cwd: impl Into<PathBuf>,
        cols: u16,
        rows: u16,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: cwd.into(),
            env: Vec::new(),
            cols,
            rows,
        }
    }
}

/// Failure while creating or operating a pseudo-terminal.
#[derive(Debug, Error)]
pub enum PtyError {
    /// The requested terminal size was zero.
    #[error("PTY dimensions must be non-zero (got {cols}x{rows})")]
    InvalidSize {
        /// Requested column count.
        cols: u16,
        /// Requested row count.
        rows: u16,
    },
    /// The platform PTY implementation rejected setup or spawning.
    #[error("PTY setup failed: {0}")]
    Setup(String),
    /// Reading from or writing to the PTY failed.
    #[error("PTY I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The output reader stopped after reporting an error.
    #[error("PTY reader failed: {0}")]
    Reader(String),
    /// The asynchronous writer stopped after reporting an error.
    #[error("PTY writer failed: {0}")]
    Writer(String),
    /// Enqueuing more input would exceed the bounded writer queue.
    #[error(
        "PTY writer queue is full ({queued} queued bytes, {attempted} requested, {limit} limit)"
    )]
    WriterQueueFull {
        /// Bytes already reserved by pending writes.
        queued: usize,
        /// Bytes requested by this write, including queue overhead.
        attempted: usize,
        /// Configured queue byte limit.
        limit: usize,
    },
}

enum ReaderMessage {
    Data(Vec<u8>),
    Error(String),
}

#[derive(Default)]
struct OutputState {
    messages: std::collections::VecDeque<ReaderMessage>,
    queued_bytes: usize,
    closed: bool,
    #[cfg(test)]
    waiting_pushes: usize,
}

#[derive(Default)]
struct OutputQueue {
    state: Mutex<OutputState>,
    space_available: Condvar,
    #[cfg(test)]
    backpressured: Condvar,
}

impl OutputQueue {
    fn push(&self, message: ReaderMessage) -> bool {
        let bytes = reader_message_bytes(&message);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !state.closed && state.queued_bytes.saturating_add(bytes) > PTY_OUTPUT_QUEUE_BYTES {
            #[cfg(test)]
            {
                state.waiting_pushes += 1;
                self.backpressured.notify_all();
            }
            state = self
                .space_available
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            #[cfg(test)]
            {
                state.waiting_pushes = state.waiting_pushes.saturating_sub(1);
            }
        }
        if state.closed {
            return false;
        }
        state.queued_bytes = state.queued_bytes.saturating_add(bytes);
        state.messages.push_back(message);
        true
    }

    fn pop(&self) -> Result<Option<Vec<u8>>, PtyError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(message) = state.messages.pop_front() {
            state.queued_bytes = state
                .queued_bytes
                .saturating_sub(reader_message_bytes(&message));
            self.space_available.notify_one();
            return match message {
                ReaderMessage::Data(bytes) => Ok(Some(bytes)),
                ReaderMessage::Error(error) => Err(PtyError::Reader(error)),
            };
        }
        Ok(None)
    }

    fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
        self.space_available.notify_all();
    }

    fn is_empty(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .messages
            .is_empty()
    }

    fn is_closed_and_empty(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed && state.messages.is_empty()
    }

    #[cfg(test)]
    fn wait_until_backpressured(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.waiting_pushes == 0 {
            state = self
                .backpressured
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

pub(super) struct PtyWritePermit {
    queued_bytes: Arc<AtomicUsize>,
    bytes: usize,
}

impl PtyWritePermit {
    pub(super) fn new(queued_bytes: Arc<AtomicUsize>, bytes: usize) -> Self {
        Self {
            queued_bytes,
            bytes,
        }
    }
}

impl Drop for PtyWritePermit {
    fn drop(&mut self) {
        self.queued_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct WriterMessage {
    bytes: Vec<u8>,
    reserved: usize,
    _permit: Option<PtyWritePermit>,
}

struct WriterState {
    queued_bytes: AtomicUsize,
    error: Mutex<Option<String>>,
}

struct PtyWriter {
    sender: mpsc::Sender<WriterMessage>,
    state: Arc<WriterState>,
}

impl PtyWriter {
    fn spawn(writer: Box<dyn Write + Send>) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let state = Arc::new(WriterState {
            queued_bytes: AtomicUsize::new(0),
            error: Mutex::new(None),
        });
        let writer_state = Arc::clone(&state);
        thread::Builder::new()
            .name("fleet-pty-writer".to_owned())
            .spawn(move || write_input(writer, receiver, writer_state))?;
        Ok(Self { sender, state })
    }

    fn write(&self, bytes: &[u8]) -> Result<(), PtyError> {
        self.enqueue(bytes, None)
    }

    fn write_permitted(&self, bytes: &[u8], permit: PtyWritePermit) -> Result<(), PtyError> {
        self.enqueue(bytes, Some(permit))
    }

    fn enqueue(&self, bytes: &[u8], permit: Option<PtyWritePermit>) -> Result<(), PtyError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if let Some(error) = self.error() {
            return Err(PtyError::Writer(error));
        }
        let reserved = if permit.is_some() {
            0
        } else {
            let reserved = bytes.len().saturating_add(WRITE_MESSAGE_OVERHEAD);
            reserve_bytes(&self.state.queued_bytes, reserved, PTY_WRITE_QUEUE_BYTES).map_err(
                |queued| PtyError::WriterQueueFull {
                    queued,
                    attempted: reserved,
                    limit: PTY_WRITE_QUEUE_BYTES,
                },
            )?;
            reserved
        };
        let message = WriterMessage {
            bytes: bytes.to_vec(),
            reserved,
            _permit: permit,
        };
        if self.sender.send(message).is_err() {
            if reserved > 0 {
                self.state
                    .queued_bytes
                    .fetch_sub(reserved, Ordering::AcqRel);
            }
            return Err(PtyError::Writer(
                self.error()
                    .unwrap_or_else(|| "writer thread stopped".to_owned()),
            ));
        }
        Ok(())
    }

    fn error(&self) -> Option<String> {
        self.state
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct SpawnedChild {
    child: Box<dyn Child + Send + Sync>,
    reaped: bool,
}

impl SpawnedChild {
    fn wait(mut self) -> io::Result<ExitStatus> {
        let result = self.child.wait();
        self.reaped = result.is_ok();
        result
    }
}

impl Drop for SpawnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            if let Err(error) = self.child.kill() {
                tracing::warn!(%error, "failed to terminate child after PTY setup failure");
            }
            if let Err(error) = self.child.wait() {
                tracing::warn!(%error, "failed to reap child after PTY setup failure");
            }
        }
    }
}

/// An owned pseudo-terminal, child process, writer, and asynchronous output stream.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: PtyWriter,
    child_pid: Option<u32>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    exit: Receiver<io::Result<ExitStatus>>,
    exit_status: Option<ExitStatus>,
    output: Arc<OutputQueue>,
}

impl Pty {
    /// Opens a native PTY, spawns the configured child, and starts its reader thread.
    pub fn spawn(options: PtyOptions) -> Result<Self, PtyError> {
        Self::spawn_inner(options, None)
    }

    #[cfg(feature = "ghostty")]
    pub(crate) fn spawn_notifying(
        options: PtyOptions,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, PtyError> {
        Self::spawn_inner(options, Some(notify))
    }

    fn spawn_inner(
        options: PtyOptions,
        notify: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Self, PtyError> {
        validate_size(options.cols, options.rows)?;
        if !options.cwd.is_dir() {
            return Err(PtyError::Setup(format!(
                "working directory does not exist: {}",
                options.cwd.display()
            )));
        }

        let pair = native_pty_system()
            .openpty(pty_size(options.cols, options.rows))
            .map_err(|error| PtyError::Setup(error.to_string()))?;
        let mut command = CommandBuilder::new(&options.program);
        command.args(&options.args);
        command.cwd(&options.cwd);
        for (key, value) in options.env {
            command.env(key, value);
        }

        let child = SpawnedChild {
            child: pair
                .slave
                .spawn_command(command)
                .map_err(|error| PtyError::Setup(error.to_string()))?,
            reaped: false,
        };
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| PtyError::Setup(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| PtyError::Setup(error.to_string()))?;
        let writer = PtyWriter::spawn(writer)?;
        // Keep draining while the host writes: an echoing child can otherwise deadlock
        // a large paste. The host separately bounds parsing work per iteration.
        let output = Arc::new(OutputQueue::default());
        let reader_output = Arc::clone(&output);
        let reader_notify = notify.clone();
        thread::Builder::new()
            .name("fleet-pty-reader".to_owned())
            .spawn(move || read_output(reader, reader_output, reader_notify))
            .map_err(PtyError::Io)?;

        let child_pid = child.child.process_id();
        let killer = child.child.clone_killer();
        let (exit_sender, exit) = async_channel::bounded(1);
        thread::Builder::new()
            .name("fleet-pty-wait".to_owned())
            .spawn(move || {
                let _ = exit_sender.send_blocking(child.wait());
                if let Some(notify) = notify {
                    notify();
                }
            })
            .map_err(PtyError::Io)?;

        Ok(Self {
            master: pair.master,
            writer,
            child_pid,
            killer,
            exit,
            exit_status: None,
            output,
        })
    }

    /// Enqueues bytes for ordered delivery by the dedicated PTY writer.
    pub fn write(&self, bytes: &[u8]) -> Result<(), PtyError> {
        self.writer.write(bytes)
    }

    pub(super) fn write_permitted(
        &self,
        bytes: &[u8],
        permit: PtyWritePermit,
    ) -> Result<(), PtyError> {
        self.writer.write_permitted(bytes, permit)
    }

    /// Updates the PTY's kernel window size.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        validate_size(cols, rows)?;
        self.master
            .resize(pty_size(cols, rows))
            .map_err(|error| PtyError::Setup(error.to_string()))
    }

    /// Returns the shell or child process identifier when the platform exposes it.
    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// Polls the child without blocking, returning its exit code once complete.
    pub fn try_wait(&mut self) -> Result<Option<i32>, PtyError> {
        if self.exit_status.is_none() {
            match self.exit.try_recv() {
                Ok(status) => self.exit_status = Some(status?),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Closed) => {
                    return Err(PtyError::Io(io::Error::other("PTY child waiter closed")));
                }
            }
        }
        Ok(self
            .exit_status
            .as_ref()
            .map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX)))
    }

    /// Requests termination of the child process.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        self.killer.kill()?;
        // portable-pty's split Unix killer only sends SIGHUP; its owning Child::kill
        // also escalates after 200 ms. Keep that contract with the independent waiter.
        #[cfg(unix)]
        {
            for attempt in 0..5 {
                if attempt > 0 {
                    thread::sleep(std::time::Duration::from_millis(50));
                }
                if self.try_wait()?.is_some() {
                    return Ok(());
                }
            }
            if let Some(pid) = self.child_pid {
                terminate_child(pid)?;
            }
        }
        Ok(())
    }

    /// Polls the output forwarded by the blocking reader thread.
    pub fn try_read(&self) -> Result<Option<Vec<u8>>, PtyError> {
        self.output.pop()
    }

    #[cfg(feature = "ghostty")]
    pub(crate) fn has_output(&self) -> bool {
        !self.output.is_empty()
    }

    /// Returns whether the reader has reached EOF and all output has been drained.
    #[must_use]
    pub fn output_closed(&self) -> bool {
        self.output.is_closed_and_empty()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.output.close();
    }
}

#[cfg(unix)]
fn terminate_child(pid: u32) -> io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: kill takes scalar arguments, and this PID came from our spawned child.
    const SIGKILL: i32 = 9;
    if unsafe { kill(pid, SIGKILL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn validate_size(cols: u16, rows: u16) -> Result<(), PtyError> {
    if cols == 0 || rows == 0 {
        Err(PtyError::InvalidSize { cols, rows })
    } else {
        Ok(())
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn read_output(
    mut reader: Box<dyn Read + Send>,
    output: Arc<OutputQueue>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let mut buffer = vec![0_u8; IO_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if !output.push(ReaderMessage::Data(buffer[..count].to_vec())) {
                    break;
                }
                if let Some(notify) = &notify {
                    notify();
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                output.push(ReaderMessage::Error(error.to_string()));
                break;
            }
        }
    }
    output.close();
    if let Some(notify) = notify {
        notify();
    }
}

fn reader_message_bytes(message: &ReaderMessage) -> usize {
    let payload = match message {
        ReaderMessage::Data(bytes) => bytes.len(),
        ReaderMessage::Error(error) => error.len(),
    };
    payload.saturating_add(OUTPUT_MESSAGE_OVERHEAD)
}

fn reserve_bytes(counter: &AtomicUsize, amount: usize, limit: usize) -> Result<usize, usize> {
    let mut queued = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = queued.checked_add(amount).filter(|next| *next <= limit) else {
            return Err(queued);
        };
        match counter.compare_exchange_weak(queued, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(queued),
            Err(actual) => queued = actual,
        }
    }
}

fn write_input(
    mut writer: Box<dyn Write + Send>,
    receiver: mpsc::Receiver<WriterMessage>,
    state: Arc<WriterState>,
) {
    while let Ok(message) = receiver.recv() {
        let result = writer
            .write_all(&message.bytes)
            .and_then(|()| writer.flush());
        if message.reserved > 0 {
            state
                .queued_bytes
                .fetch_sub(message.reserved, Ordering::AcqRel);
        }
        if let Err(error) = result {
            *state
                .error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn shell_sets_fleet_environment() {
        let options = PtyOptions::shell("/tmp", "session", "editor", TerminalId(42), 80, 24);
        assert!(
            options
                .env
                .contains(&(OsString::from("TERM"), OsString::from("xterm-256color")))
        );
        assert!(
            options
                .env
                .contains(&(OsString::from("FLEET_SESSION"), OsString::from("session")))
        );
        assert!(
            options
                .env
                .contains(&(OsString::from("FLEET_TERMINAL"), OsString::from("editor")))
        );
        assert!(
            options
                .env
                .contains(&(OsString::from("FLEET_TERMINAL_ID"), OsString::from("42")))
        );
    }

    #[test]
    fn rejects_zero_sized_pty() {
        let options = PtyOptions::command("/bin/sh", ["-c", "true"], Path::new("/"), 0, 24);
        assert!(matches!(
            Pty::spawn(options),
            Err(PtyError::InvalidSize { .. })
        ));
    }

    #[test]
    fn output_queue_backpressures_without_closing() {
        let queue = Arc::new(OutputQueue::default());
        let first = vec![b'a'; PTY_OUTPUT_QUEUE_BYTES - OUTPUT_MESSAGE_OVERHEAD];
        assert!(queue.push(ReaderMessage::Data(first.clone())));

        let producer_queue = Arc::clone(&queue);
        let (completed, completion) = mpsc::channel();
        let producer = thread::spawn(move || {
            let pushed = producer_queue.push(ReaderMessage::Data(b"next".to_vec()));
            completed.send(pushed).unwrap();
        });
        queue.wait_until_backpressured();
        assert!(matches!(
            completion.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        assert_eq!(queue.pop().unwrap(), Some(first));
        assert!(completion.recv().unwrap());
        producer.join().unwrap();
        assert_eq!(queue.pop().unwrap(), Some(b"next".to_vec()));
        assert!(!queue.is_closed_and_empty());
    }
}
