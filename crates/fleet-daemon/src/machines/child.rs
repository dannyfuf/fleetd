//! Child-process-backed asynchronous byte streams.

use std::{
    pin::Pin,
    process::Stdio,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

use super::MachineError;

/// A child whose standard input and output form one duplex byte stream.
pub struct ChildStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    child: Arc<Mutex<Child>>,
}

impl ChildStream {
    /// Spawns an argv without invoking a shell.
    pub fn spawn(argv: &[String]) -> Result<Self, MachineError> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| MachineError::NotFound("empty child argv".to_owned()))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MachineError::Io(std::io::Error::other("child stdin unavailable")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MachineError::Io(std::io::Error::other("child stdout unavailable")))?;
        Ok(Self {
            stdin,
            stdout,
            child: Arc::new(Mutex::new(child)),
        })
    }

    /// Waits for the child and returns its numeric exit code.
    pub async fn exit_code(&self) -> Result<Option<i32>, MachineError> {
        Ok(self.child.lock().await.wait().await?.code())
    }

    /// Converts OpenSSH's conventional transport failure into `Unreachable`.
    pub fn check_exit(status: i32, stderr: &str) -> Result<(), MachineError> {
        if status == 255 {
            Err(MachineError::Unreachable(stderr.trim().to_owned()))
        } else {
            Ok(())
        }
    }
}

impl AsyncRead for ChildStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(context, buffer)
    }
}

impl AsyncWrite for ChildStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.stdin).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stdin).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stdin).poll_shutdown(context)
    }
}

impl Drop for ChildStream {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}
