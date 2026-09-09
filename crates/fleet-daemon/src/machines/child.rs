//! Child-process-backed asynchronous byte streams.

use std::{
    pin::Pin,
    process::Stdio,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use futures_util::task::AtomicWaker;

use tokio::{
    io::AsyncReadExt,
    io::{AsyncRead, AsyncWrite, ReadBuf},
    process::{ChildStdin, ChildStdout, Command},
    sync::Notify,
    task::JoinHandle,
};

use super::MachineError;

/// A child whose standard input and output form one duplex byte stream.
pub struct ChildStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    exit: Arc<ExitState>,
    monitor: JoinHandle<()>,
    stdout_closed: bool,
}

struct ExitState {
    result: Mutex<Option<Result<ExitOutcome, String>>>,
    waker: AtomicWaker,
    notify: Notify,
}

#[derive(Clone)]
struct ExitOutcome {
    code: Option<i32>,
    stderr: String,
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
            .stderr(Stdio::piped())
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
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| MachineError::Io(std::io::Error::other("child stderr unavailable")))?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            MachineError::Io(std::io::Error::other(format!(
                "child stream requires a Tokio runtime: {error}"
            )))
        })?;
        let exit = Arc::new(ExitState {
            result: Mutex::new(None),
            waker: AtomicWaker::new(),
            notify: Notify::new(),
        });
        let monitored = Arc::clone(&exit);
        let monitor = runtime.spawn(async move {
            let mut captured = Vec::new();
            let (stderr_result, status_result) =
                tokio::join!(stderr.read_to_end(&mut captured), child.wait());
            let result = match (stderr_result, status_result) {
                (Ok(_), Ok(status)) => Ok(ExitOutcome {
                    code: status.code(),
                    stderr: String::from_utf8_lossy(&captured).into_owned(),
                }),
                (Err(error), _) | (_, Err(error)) => Err(error.to_string()),
            };
            *lock(&monitored.result) = Some(result);
            monitored.waker.wake();
            monitored.notify.notify_waiters();
        });
        Ok(Self {
            stdin,
            stdout,
            exit,
            monitor,
            stdout_closed: false,
        })
    }

    /// Waits for the child and returns its numeric exit code.
    pub async fn exit_code(&self) -> Result<Option<i32>, MachineError> {
        loop {
            let notified = self.exit.notify.notified();
            if let Some(result) = lock(&self.exit.result).clone() {
                return result
                    .map(|outcome| outcome.code)
                    .map_err(|error| MachineError::Io(std::io::Error::other(error)));
            }
            notified.await;
        }
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
        if !self.stdout_closed {
            let before = buffer.filled().len();
            match Pin::new(&mut self.stdout).poll_read(context, buffer) {
                Poll::Ready(Ok(())) if buffer.filled().len() == before => {
                    self.stdout_closed = true;
                }
                result => return result,
            }
        }
        self.exit.waker.register(context.waker());
        let Some(result) = lock(&self.exit.result).clone() else {
            return Poll::Pending;
        };
        match result {
            Ok(ExitOutcome {
                code: Some(255),
                stderr,
            }) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::HostUnreachable,
                if stderr.trim().is_empty() {
                    "SSH transport exited 255".to_owned()
                } else {
                    stderr.trim().to_owned()
                },
            ))),
            Ok(ExitOutcome {
                code: Some(code),
                stderr,
            }) if code != 0 => {
                Poll::Ready(Err(std::io::Error::other(if stderr.trim().is_empty() {
                    format!("stream child exited {code}")
                } else {
                    format!("stream child exited {code}: {}", stderr.trim())
                })))
            }
            Ok(_) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(std::io::Error::other(error))),
        }
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
        self.monitor.abort();
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

    #[tokio::test]
    async fn ssh_exit_255_surfaces_captured_stderr_from_the_stream() {
        let mut stream = ChildStream::spawn(&[
            "sh".to_owned(),
            "-c".to_owned(),
            "printf 'permission denied' >&2; exit 255".to_owned(),
        ])
        .expect("child stream");
        let mut bytes = Vec::new();
        let error = stream
            .read_to_end(&mut bytes)
            .await
            .expect_err("SSH failure must not look like EOF");
        assert_eq!(error.kind(), std::io::ErrorKind::HostUnreachable);
        assert!(error.to_string().contains("permission denied"));
        assert_eq!(stream.exit_code().await.expect("exit code"), Some(255));
    }
}
