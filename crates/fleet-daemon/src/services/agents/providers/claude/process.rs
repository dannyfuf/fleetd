//! Claude Code subprocess, NDJSON reader, and serialized stdin writer.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Context as _;
use fleet_core::agents::AgentEvent;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, Command},
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};

use super::{ClaudeMapper, ProviderError, ProviderEvent, ProviderSink, wire::parse_line};
use crate::services::agents::providers::exit_code;

/// How many bytes one Claude stdout frame may hold before it is abandoned.
///
/// The NDJSON reader is the daemon's only bound on what the child writes: an unterminated line —
/// a wedged child, a pathological `tool_use_result` — would otherwise grow the daemon's heap
/// until the machine gives out. 8 MiB is far past any real frame, so reaching it means the
/// framing itself is lost, exactly as the OpenCode SSE parser treats its own budget.
const MAX_FRAME: usize = 8 * 1024 * 1024;

const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug)]
enum WriteCommand {
    Json(Value, oneshot::Sender<Result<(), String>>),
    Close(oneshot::Sender<Result<(), String>>),
}

#[derive(Debug, Clone)]
pub(super) struct Writer {
    sender: mpsc::Sender<WriteCommand>,
}

impl Writer {
    pub async fn write(&self, value: Value) -> Result<(), ProviderError> {
        let (accepted, result) = oneshot::channel();
        tokio::time::timeout(
            WRITE_TIMEOUT,
            self.sender.send(WriteCommand::Json(value, accepted)),
        )
        .await
        .map_err(|_| ProviderError::Timeout {
            what: "Claude stdin writer admission".to_owned(),
        })?
        .map_err(|_| ProviderError::Exited { code: None })?;
        tokio::time::timeout(WRITE_TIMEOUT, result)
            .await
            .map_err(|_| ProviderError::Timeout {
                what: "Claude stdin write".to_owned(),
            })?
            .map_err(|_| ProviderError::Exited { code: None })?
            .map_err(|message| ProviderError::Protocol { message })
    }

    pub async fn close(&self) -> Result<(), ProviderError> {
        let (accepted, result) = oneshot::channel();
        tokio::time::timeout(
            WRITE_TIMEOUT,
            self.sender.send(WriteCommand::Close(accepted)),
        )
        .await
        .map_err(|_| ProviderError::Timeout {
            what: "Claude stdin close admission".to_owned(),
        })?
        .map_err(|_| ProviderError::Exited { code: None })?;
        tokio::time::timeout(WRITE_TIMEOUT, result)
            .await
            .map_err(|_| ProviderError::Timeout {
                what: "Claude stdin close".to_owned(),
            })?
            .map_err(|_| ProviderError::Exited { code: None })?
            .map_err(|message| ProviderError::Protocol { message })
    }
}

#[derive(Debug)]
enum SupervisorCommand {
    Stop(oneshot::Sender<Result<(), String>>),
    StdoutEof,
}

#[derive(Debug)]
pub(super) struct RunningProcess {
    pub writer: Writer,
    supervisor: mpsc::Sender<SupervisorCommand>,
    expected_stop: Arc<AtomicBool>,
    _reader: JoinHandle<()>,
    _stderr: JoinHandle<()>,
    _supervisor: JoinHandle<()>,
}

impl RunningProcess {
    pub async fn stop(&self) -> Result<(), ProviderError> {
        self.expected_stop.store(true, Ordering::Release);
        let close_result = self.writer.close().await;
        let (done, result) = oneshot::channel();
        if self
            .supervisor
            .send(SupervisorCommand::Stop(done))
            .await
            .is_err()
        {
            return close_result;
        }
        let stop_result = result
            .await
            .map_err(|_| ProviderError::Exited { code: None })?
            .map_err(|message| ProviderError::Protocol { message });
        close_result.and(stop_result)
    }
}

pub(super) async fn login_environment(cwd: &Path) -> HashMap<OsString, OsString> {
    let shell = std::env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("/bin/zsh"));
    let output = Command::new(shell)
        .args(["-l", "-c", "/usr/bin/env -0"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = output else {
        return std::env::vars_os().collect();
    };
    if !output.status.success() {
        return std::env::vars_os().collect();
    }
    let mut environment = HashMap::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        let Some(separator) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = OsString::from(String::from_utf8_lossy(&entry[..separator]).into_owned());
        let value = OsString::from(String::from_utf8_lossy(&entry[separator + 1..]).into_owned());
        environment.insert(key, value);
    }
    if environment.is_empty() {
        std::env::vars_os().collect()
    } else {
        environment
    }
}

pub(super) async fn check_version(
    command_line: &str,
    cwd: &Path,
    environment: &HashMap<OsString, OsString>,
) -> Result<(), ProviderError> {
    let (program, base_args) = command_parts(command_line)?;
    let mut command = Command::new(resolve_program(program, environment));
    command
        .args(base_args)
        .arg("--version")
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());
    let output = command
        .output()
        .await
        .map_err(|error| ProviderError::Unavailable {
            reason: format!("could not run Claude Code version check: {error}"),
        })?;
    if !output.status.success() {
        return Err(ProviderError::Unavailable {
            reason: format!(
                "Claude Code version check failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let Some((major, minor)) = parse_version(&version) else {
        return Err(ProviderError::Unavailable {
            reason: format!(
                "could not parse Claude Code version from `{}`",
                version.trim()
            ),
        });
    };
    if (major, minor) < (2, 1) {
        return Err(ProviderError::Unavailable {
            reason: format!(
                "Claude Code {major}.{minor} is unsupported; update Claude Code to 2.1 or newer"
            ),
        });
    }
    Ok(())
}

pub(super) async fn spawn(
    command_line: &str,
    args: &[String],
    cwd: &Path,
    mut environment: HashMap<OsString, OsString>,
    fleet_session: String,
    mapper: Arc<Mutex<ClaudeMapper>>,
    events: ProviderSink,
) -> Result<RunningProcess, ProviderError> {
    let (program, base_args) = command_parts(command_line)?;
    environment.insert(
        OsString::from("FLEET_SESSION"),
        OsString::from(fleet_session),
    );
    environment.insert(OsString::from("FLEET_TERMINAL"), OsString::from("claude"));

    let mut command = Command::new(resolve_program(program, &environment));
    command
        .args(base_args)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| ProviderError::Unavailable {
            reason: format!("could not spawn Claude Code: {error}"),
        })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::Unavailable {
            reason: "Claude Code did not provide piped stdin".to_owned(),
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProviderError::Unavailable {
            reason: "Claude Code did not provide piped stdout".to_owned(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProviderError::Unavailable {
            reason: "Claude Code did not provide piped stderr".to_owned(),
        })?;

    let (writer_sender, writer_receiver) = mpsc::channel(64);
    let writer = Writer {
        sender: writer_sender,
    };
    tokio::spawn(writer_loop(stdin, writer_receiver));

    let (supervisor_sender, supervisor_receiver) = mpsc::channel(2);
    let expected_stop = Arc::new(AtomicBool::new(false));
    let eof_sender = supervisor_sender.clone();
    let reader_writer = writer.clone();
    let reader_mapper = Arc::clone(&mapper);
    let reader_events = events.clone();
    let reader = tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        loop {
            match read_frame(&mut stdout).await {
                Ok(Frame::Line(line)) if line.trim().is_empty() => continue,
                Ok(Frame::Line(line)) => {
                    let mapped = match parse_line(&line) {
                        Ok(message) => reader_mapper.lock().await.handle(message),
                        Err(error) => Err(error),
                    };
                    match mapped {
                        Ok(output) => {
                            let raw = output.raw;
                            for event in output.events {
                                let event = ProviderEvent::new(event, raw.as_deref());
                                if reader_events.send(event).is_err() {
                                    return;
                                }
                            }
                            for write in output.writes {
                                if let Err(error) = reader_writer.write(write).await {
                                    let _ = reader_events.send(
                                        AgentEvent::RuntimeError {
                                            fatal: true,
                                            message: error.to_string(),
                                        }
                                        .into(),
                                    );
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            let _ = reader_events.send(
                                AgentEvent::RuntimeError {
                                    fatal: false,
                                    message: error.to_string(),
                                }
                                .into(),
                            );
                        }
                    }
                }
                Ok(Frame::Oversized(bytes)) => {
                    tracing::warn!(
                        target: "fleet::agents::claude",
                        bytes,
                        "dropped a Claude stdout line past the frame budget"
                    );
                }
                Ok(Frame::Eof) => {
                    let _ = eof_sender.send(SupervisorCommand::StdoutEof).await;
                    return;
                }
                Err(error) => {
                    let _ = reader_events.send(
                        AgentEvent::RuntimeError {
                            fatal: false,
                            message: format!("failed reading Claude stdout: {error}"),
                        }
                        .into(),
                    );
                    let _ = eof_sender.send(SupervisorCommand::StdoutEof).await;
                    return;
                }
            }
        }
    });

    let stderr = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::warn!(target: "fleet::agents::claude", "{line}");
        }
    });

    let supervisor = tokio::spawn(supervise(
        child,
        supervisor_receiver,
        mapper,
        events,
        Arc::clone(&expected_stop),
    ));

    Ok(RunningProcess {
        writer,
        supervisor: supervisor_sender,
        expected_stop,
        _reader: reader,
        _stderr: stderr,
        _supervisor: supervisor,
    })
}

/// One newline-terminated frame of the child's stdout, or the reason there is none.
#[derive(Debug)]
enum Frame {
    /// A complete line, with its terminator stripped.
    Line(String),
    /// A line past [`MAX_FRAME`]; its bytes were dropped through the next newline.
    Oversized(usize),
    /// The child closed stdout.
    Eof,
}

/// Reads one line without ever buffering more than [`MAX_FRAME`] bytes of it.
///
/// `AsyncBufReadExt::lines` has no such budget, which is why it is not used here.
async fn read_frame<R>(reader: &mut R) -> std::io::Result<Frame>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let mut seen = 0usize;
    let mut oversized = false;
    loop {
        let (chunk, consume, terminated) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                if seen == 0 {
                    return Ok(Frame::Eof);
                }
                return Ok(settle(line, seen, oversized));
            }
            match available.iter().position(|byte| *byte == b'\n') {
                Some(newline) => (available[..newline].to_vec(), newline + 1, true),
                None => (available.to_vec(), available.len(), false),
            }
        };
        reader.consume(consume);
        seen = seen.saturating_add(chunk.len());
        if oversized || seen > MAX_FRAME {
            oversized = true;
            line = Vec::new();
        } else {
            line.extend_from_slice(&chunk);
        }
        if terminated {
            return Ok(settle(line, seen, oversized));
        }
    }
}

/// Turns the bytes of one frame into its outcome; a mangled byte costs its character, not the
/// frame, exactly as the OpenCode parser decodes.
fn settle(mut line: Vec<u8>, seen: usize, oversized: bool) -> Frame {
    if oversized {
        return Frame::Oversized(seen);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Frame::Line(String::from_utf8_lossy(&line).into_owned())
}

async fn writer_loop(
    mut stdin: tokio::process::ChildStdin,
    mut receiver: mpsc::Receiver<WriteCommand>,
) {
    while let Some(command) = receiver.recv().await {
        match command {
            WriteCommand::Json(value, accepted) => {
                let result = async {
                    let bytes = serde_json::to_vec(&value).context("serialize Claude stdin")?;
                    stdin
                        .write_all(&bytes)
                        .await
                        .context("write Claude stdin")?;
                    stdin
                        .write_all(b"\n")
                        .await
                        .context("write Claude newline")?;
                    stdin.flush().await.context("flush Claude stdin")?;
                    anyhow::Ok(())
                }
                .await
                .map_err(|error| error.to_string());
                let failed = result.is_err();
                let _ = accepted.send(result);
                if failed {
                    return;
                }
            }
            WriteCommand::Close(accepted) => {
                let result = stdin.shutdown().await.map_err(|error| error.to_string());
                let _ = accepted.send(result);
                return;
            }
        }
    }
}

/// Watches the child until it exits or the provider asks for a stop.
async fn supervise(
    mut child: Child,
    mut commands: mpsc::Receiver<SupervisorCommand>,
    mapper: Arc<Mutex<ClaudeMapper>>,
    events: ProviderSink,
    expected_stop: Arc<AtomicBool>,
) {
    tokio::select! {
        // Checklist 24: the control branch first. A stop pressed on a child that is already
        // exiting queues its `Stop` and then loses the coin flip to `child.wait()`, and the
        // dropped `done` reaches `RunningProcess::stop` as `ProviderError::Exited`.
        biased;
        command = commands.recv() => {
            match command {
                Some(SupervisorCommand::Stop(done)) => {
                    let result = terminate(&mut child).await;
                    let code = result.as_ref().ok().copied().flatten();
                    emit_exit(&mapper, &events, code, true).await;
                    let _ = done.send(result.map(|_| ()).map_err(|error| error.to_string()));
                }
                Some(SupervisorCommand::StdoutEof) => {
                    let code = match child.try_wait() {
                        Ok(Some(status)) => exit_code(&status),
                        Ok(None) => {
                            let _ = child.start_kill();
                            child.wait().await.ok().as_ref().and_then(exit_code)
                        }
                        Err(_) => None,
                    };
                    let expected = expected_stop.load(Ordering::Acquire);
                    emit_exit(&mapper, &events, code, expected).await;
                }
                None => {}
            }
        }
        status = child.wait() => {
            let code = status.ok().as_ref().and_then(exit_code);
            let expected = expected_stop.load(Ordering::Acquire);
            emit_exit(&mapper, &events, code, expected).await;
            // The child is provably gone, so a stop that arrives from here on has already
            // happened. Answering it is what keeps `AgentSessionManager::stop` on its
            // settlement path instead of reporting a conflict.
            while let Ok(command) = commands.try_recv() {
                if let SupervisorCommand::Stop(done) = command {
                    // Fire-and-forget: the receiver is gone only when the caller's `stop`
                    // future was already dropped, which needs no answer.
                    let _ignored = done.send(Ok(()));
                }
            }
        }
    }
}

async fn terminate(child: &mut Child) -> anyhow::Result<Option<i32>> {
    // Stdin is already closed: give Claude Code the same budget to exit on its own and persist
    // its session before escalating to signals.
    if let Ok(status) = tokio::time::timeout(STOP_TIMEOUT, child.wait()).await {
        return Ok(exit_code(
            &status.context("wait for Claude Code after closing stdin")?,
        ));
    }
    if let Some(pid) = child.id() {
        #[allow(clippy::cast_possible_wrap)]
        let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("send SIGTERM to Claude Code");
            }
        }
    }
    let status = match tokio::time::timeout(STOP_TIMEOUT, child.wait()).await {
        Ok(status) => status.context("wait for Claude Code after SIGTERM")?,
        Err(_) => {
            child.start_kill().context("send SIGKILL to Claude Code")?;
            child
                .wait()
                .await
                .context("wait for Claude Code after SIGKILL")?
        }
    };
    Ok(exit_code(&status))
}

async fn emit_exit(
    mapper: &Arc<Mutex<ClaudeMapper>>,
    events: &ProviderSink,
    code: Option<i32>,
    expected: bool,
) {
    let mapped = mapper.lock().await.process_exit(code, expected);
    for event in mapped {
        if events
            .send(ProviderEvent::new(event, Some("process_exit")))
            .is_err()
        {
            break;
        }
    }
}

fn command_parts(command_line: &str) -> Result<(OsString, Vec<OsString>), ProviderError> {
    let parts = shell_words::split(command_line).map_err(|error| ProviderError::Unavailable {
        reason: format!("invalid configured Claude command: {error}"),
    })?;
    let mut parts = parts.into_iter();
    let program = parts.next().ok_or_else(|| ProviderError::Unavailable {
        reason: "configured Claude command is empty".to_owned(),
    })?;
    Ok((OsString::from(program), parts.map(OsString::from).collect()))
}

fn resolve_program(program: OsString, environment: &HashMap<OsString, OsString>) -> OsString {
    let path = std::path::Path::new(&program);
    if path.components().count() > 1 {
        return program;
    }
    environment
        .get(&OsString::from("PATH"))
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(path))
        .find(|candidate| candidate.is_file())
        .map_or(program, std::path::PathBuf::into_os_string)
}

fn parse_version(output: &str) -> Option<(u64, u64)> {
    output.split_whitespace().find_map(|word| {
        let word = word.trim_matches(|character: char| !character.is_ascii_digit());
        let mut parts = word.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        Some((major, minor))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unterminated_stdout_line_is_dropped_instead_of_buffered() {
        let oversized = vec![b'x'; MAX_FRAME + 1024];
        let mut stdin = Vec::new();
        stdin.extend_from_slice(&oversized);
        stdin.extend_from_slice(b"\n{\"type\":\"result\"}\r\n");
        let mut reader = BufReader::new(std::io::Cursor::new(stdin));

        match read_frame(&mut reader).await.expect("oversized frame") {
            Frame::Oversized(bytes) => assert!(bytes > MAX_FRAME),
            other => panic!("expected the frame to be dropped, got {other:?}"),
        }
        // The frame after it still parses, and its `\r` is gone.
        match read_frame(&mut reader).await.expect("next frame") {
            Frame::Line(line) => assert_eq!(line, r#"{"type":"result"}"#),
            other => panic!("expected the next frame, got {other:?}"),
        }
        assert!(matches!(
            read_frame(&mut reader).await.expect("eof"),
            Frame::Eof
        ));
    }

    /// A stop pressed on a child that has just died must still be answered: the manager treats
    /// an error from `stop` as a conflict and returns before it settles the turn, so losing the
    /// queued `Stop` leaves the transcript claiming a dead process is still running.
    #[tokio::test]
    async fn a_stop_queued_behind_the_childs_exit_is_still_answered() {
        // The race is a coin flip per attempt while both `select!` arms are ready, so one
        // attempt proves nothing; thirty make the unfixed supervisor lose with certainty.
        for attempt in 0..30 {
            let mut child = Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap_or_else(|error| panic!("spawn the test child: {error}"));
            // Reap it here so the supervisor's exit arm is ready on its very first poll — the
            // stop is then queued behind a child that has provably already gone.
            child
                .wait()
                .await
                .unwrap_or_else(|error| panic!("reap the test child: {error}"));

            let (commands_sender, commands) = mpsc::channel(2);
            let (done, answered) = oneshot::channel();
            commands_sender
                .send(SupervisorCommand::Stop(done))
                .await
                .unwrap_or_else(|error| panic!("queue the stop: {error}"));

            let (events, _sink) = tokio::sync::mpsc::unbounded_channel();
            let expected_stop = Arc::new(AtomicBool::new(true));
            supervise(
                child,
                commands,
                Arc::new(Mutex::new(ClaudeMapper::default())),
                events,
                expected_stop,
            )
            .await;

            let answer = answered
                .await
                .unwrap_or_else(|_| panic!("attempt {attempt}: the queued stop was dropped"));
            assert_eq!(answer, Ok(()), "attempt {attempt}");
        }
    }

    #[test]
    fn parses_claude_code_version() {
        assert_eq!(parse_version("2.1.263 (Claude Code)"), Some((2, 1)));
        assert_eq!(parse_version("Claude Code v3.7.0"), Some((3, 7)));
        assert_eq!(parse_version("unknown"), None);
    }
}
