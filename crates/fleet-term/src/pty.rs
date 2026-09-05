//! Portable PTY creation, process control, resize, input, and output plumbing.

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::PathBuf,
    thread,
};

use async_channel::{Receiver, TryRecvError};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

const TERM: &str = "xterm-256color";

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
    /// Creates options for the user's login shell and Fleet terminal environment.
    #[must_use]
    pub fn login_shell(
        cwd: impl Into<PathBuf>,
        session: impl AsRef<OsStr>,
        terminal: impl AsRef<OsStr>,
        _ghostty: bool,
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
            ],
            cols,
            rows,
        }
    }

    /// Creates options for an arbitrary command, primarily for controlled hosts and tests.
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
}

enum ReaderMessage {
    Data(Vec<u8>),
    Error(String),
}

/// An owned pseudo-terminal, child process, writer, and asynchronous output stream.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<ReaderMessage>,
}

impl Pty {
    /// Opens a native PTY, spawns the configured child, and starts its reader thread.
    pub fn spawn(options: PtyOptions) -> Result<Self, PtyError> {
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

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| PtyError::Setup(error.to_string()))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| PtyError::Setup(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| PtyError::Setup(error.to_string()))?;
        let (sender, output) = async_channel::unbounded();
        thread::Builder::new()
            .name("fleet-pty-reader".to_owned())
            .spawn(move || read_output(reader, sender))
            .map_err(PtyError::Io)?;

        Ok(Self {
            master: pair.master,
            writer,
            child,
            output,
        })
    }

    /// Writes bytes to the PTY and flushes them for prompt delivery.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
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
        self.child.process_id()
    }

    /// Polls the child without blocking, returning its exit code once complete.
    pub fn try_wait(&mut self) -> Result<Option<i32>, PtyError> {
        self.child.try_wait().map(exit_code).map_err(PtyError::Io)
    }

    /// Requests termination of the child process.
    pub fn kill(&mut self) -> Result<(), PtyError> {
        self.child.kill().map_err(PtyError::Io)
    }

    /// Polls the output forwarded by the blocking reader thread.
    pub fn try_read(&self) -> Result<Option<Vec<u8>>, PtyError> {
        match self.output.try_recv() {
            Ok(ReaderMessage::Data(bytes)) => Ok(Some(bytes)),
            Ok(ReaderMessage::Error(error)) => Err(PtyError::Reader(error)),
            Err(TryRecvError::Empty | TryRecvError::Closed) => Ok(None),
        }
    }

    /// Returns whether the reader has reached EOF and all output has been drained.
    #[must_use]
    pub fn output_closed(&self) -> bool {
        self.output.is_closed() && self.output.is_empty()
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

fn exit_code(status: Option<portable_pty::ExitStatus>) -> Option<i32> {
    status.map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX))
}

fn read_output(mut reader: Box<dyn Read + Send>, sender: async_channel::Sender<ReaderMessage>) {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if sender
                    .send_blocking(ReaderMessage::Data(buffer[..count].to_vec()))
                    .is_err()
                {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.send_blocking(ReaderMessage::Error(error.to_string()));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn login_shell_sets_fleet_environment() {
        let options = PtyOptions::login_shell("/tmp", "session", "editor", true, 80, 24);
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
    }

    #[test]
    fn login_shell_uses_portable_term_for_every_backend() {
        for ghostty in [false, true] {
            let options = PtyOptions::login_shell("/tmp", "session", "editor", ghostty, 80, 24);
            let term = options
                .env
                .iter()
                .find(|(key, _)| key == "TERM")
                .map(|(_, value)| value);

            assert_eq!(term, Some(&OsString::from("xterm-256color")));
        }
    }

    #[test]
    fn rejects_zero_sized_pty() {
        let options = PtyOptions::command("/bin/sh", ["-c", "true"], Path::new("/"), 0, 24);
        assert!(matches!(
            Pty::spawn(options),
            Err(PtyError::InvalidSize { .. })
        ));
    }
}
