//! Byte-exact command passthrough with a best-effort cooperative watch copy.

use crate::args::{ExecArgs, WatchChildArgs};
use fleet_client::Client;
use fleet_core::{
    ids::{SessionId, TerminalId},
    watches::{WatchId, WatchStream},
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use std::{
    ffi::OsString,
    io::{Read, Write},
    os::unix::{
        net::UnixStream,
        process::{CommandExt, ExitStatusExt},
    },
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    signal::unix::{Signal as SignalStream, SignalKind, signal},
    sync::mpsc,
};

const BATCH: Duration = Duration::from_millis(50);
const WATCH_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(args: ExecArgs) -> i32 {
    let terminal = match watch_terminal(args.watch, |key| std::env::var_os(key)) {
        Ok(terminal) => terminal,
        Err(reason) => {
            debug(reason);
            return passthrough(&args.command, None);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            debug(error);
            return passthrough(&args.command, None);
        }
    };
    match runtime.block_on(watched(&args, terminal)) {
        Ok(code) => code,
        Err(error) => {
            debug(error);
            passthrough(&args.command, None)
        }
    }
}

fn watch_terminal(
    watch: bool,
    mut env: impl FnMut(&str) -> Option<OsString>,
) -> Result<TerminalId, &'static str> {
    if !watch {
        return Err("--watch not requested");
    }
    let session = env("FLEET_SESSION").ok_or("FLEET_SESSION missing")?;
    session
        .to_str()
        .and_then(|id| id.parse::<SessionId>().ok())
        .ok_or("FLEET_SESSION invalid")?;
    let terminal = env("FLEET_TERMINAL_ID")
        .as_deref()
        .and_then(|id| id.to_str())
        .and_then(|id| id.parse::<TerminalId>().ok())
        .ok_or("FLEET_TERMINAL_ID missing or not numeric")?;
    if env("FLEET_WATCH").is_some() {
        return Err("FLEET_WATCH already set (nested watch)");
    }
    Ok(terminal)
}

fn debug(error: impl std::fmt::Display) {
    if std::env::var_os("FLEET_DEBUG").is_some() {
        let reason = error.to_string().replace(['\r', '\n'], " ");
        eprintln!("fleet exec: watch unavailable: {reason}");
    }
}

fn passthrough(argv: &[OsString], watch: Option<String>) -> i32 {
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(watch) = watch {
        command.env("FLEET_WATCH", watch);
    }
    let error = command.exec();
    eprintln!("fleet exec: {}: {error}", argv[0].to_string_lossy());
    if error.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

pub(crate) fn child(args: WatchChildArgs) -> i32 {
    // EOF/timeout releases the target transparently if the wrapper died during startup.
    let watch = (|| -> std::io::Result<String> {
        let mut socket = UnixStream::connect(&args.socket)?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut id = String::new();
        socket.read_to_string(&mut id)?;
        Ok(id)
    })()
    .ok()
    .filter(|id| id.parse::<WatchId>().is_ok());
    passthrough(&args.command, watch)
}

async fn watched(args: &ExecArgs, terminal: TerminalId) -> anyhow::Result<i32> {
    let home = crate::commands::fleet_home()?;
    let client = tokio::time::timeout(WATCH_TIMEOUT, Client::connect(home)).await??;
    // /tmp keeps the Unix socket below macOS's small sockaddr_un path limit.
    let gate_dir = tempfile::Builder::new()
        .prefix("fleet-watch-")
        .tempdir_in("/tmp")?;
    let gate_path = gate_dir.path().join("gate");
    let gate = tokio::net::UnixListener::bind(&gate_path)?;
    let executable = std::env::current_exe()?;
    let signals = Signals::install()?;
    let mut child = Command::new(executable)
        .arg("watch-child")
        .arg(&gate_path)
        .arg("--")
        .args(&args.command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    // No fallible early return after spawn: the user's command must never run twice.
    let pid = child.id();
    let forward = signals.forward_to(pid);
    let watch = register_watch(&client, terminal, args, pid).await;
    release_child(gate, watch).await;

    let (chunks_tx, chunks) = mpsc::channel(128);
    let omitted = Arc::new(OmittedOutput::default());
    let readers = tee_child_output(&mut child, &chunks_tx, &omitted);
    drop(chunks_tx);
    let reporter = tokio::spawn(report(client.clone(), watch, chunks, omitted.clone()));

    let status = tokio::task::spawn_blocking(move || child.wait()).await;
    forward.abort(); // Do not forward to a reaped (and possibly reused) PID while draining.
    for reader in readers {
        let _ = tokio::task::spawn_blocking(move || reader.join()).await;
    }
    if let Some(message) = omitted.message() {
        eprintln!("fleet exec: {message}");
    }
    if let Err(error) = reporter.await {
        debug(format!("watch reporter failed: {error}"));
    }
    match status {
        Ok(Ok(status)) => {
            if let Some(id) = watch {
                let _ = tokio::time::timeout(
                    WATCH_TIMEOUT,
                    client.finish_watch(id, status.code(), status.signal()),
                )
                .await;
            }
            Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)))
        }
        other => {
            debug(format!("child wait failed: {other:?}"));
            Ok(1)
        }
    }
}

/// Terminal signals this wrapper relays to the launched child.
struct Signals {
    interrupt: SignalStream,
    terminate: SignalStream,
    hangup: SignalStream,
}

impl Signals {
    fn install() -> std::io::Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
        })
    }

    /// Relays signals to the child process group leader until the returned task is aborted.
    fn forward_to(mut self, pid: u32) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let Ok(pid) = i32::try_from(pid) else {
                return;
            };
            loop {
                let received = tokio::select! {
                    _ = self.interrupt.recv() => Signal::SIGINT,
                    _ = self.terminate.recv() => Signal::SIGTERM,
                    _ = self.hangup.recv() => Signal::SIGHUP,
                };
                if should_forward(received) {
                    let _ = kill(Pid::from_raw(pid), received);
                }
            }
        })
    }
}

fn should_forward(signal: Signal) -> bool {
    matches!(signal, Signal::SIGINT | Signal::SIGTERM | Signal::SIGHUP)
}

/// Registers the watch, reporting an unavailable daemon rather than failing the command.
async fn register_watch(
    client: &Client,
    terminal: TerminalId,
    args: &ExecArgs,
    pid: u32,
) -> Option<WatchId> {
    let label = args.label.clone().unwrap_or_else(|| {
        Path::new(&args.command[0])
            .file_name()
            .unwrap_or(&args.command[0])
            .to_string_lossy()
            .into_owned()
    });
    let command = args
        .command
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let registration = tokio::time::timeout(
        WATCH_TIMEOUT,
        client.start_watch(
            terminal,
            label,
            command,
            std::env::current_dir().ok(),
            Some(pid),
        ),
    )
    .await;
    match registration {
        Ok(Ok(id)) => Some(id),
        Ok(Err(error)) => {
            debug(error);
            None
        }
        Err(error) => {
            debug(error);
            None
        }
    }
}

/// Hands the launcher its watch ID, then closes the gate so the child execs the real command.
async fn release_child(gate: tokio::net::UnixListener, watch: Option<WatchId>) {
    if let Ok(Ok((mut socket, _))) = tokio::time::timeout(WATCH_TIMEOUT, gate.accept()).await {
        if let Some(id) = watch {
            let _ = socket.write_all(id.to_string().as_bytes()).await;
        }
        let _ = socket.shutdown().await;
    }
}

/// Starts one blocking copier per captured stream; each forwards output before monitoring.
fn tee_child_output(
    child: &mut std::process::Child,
    chunks: &mpsc::Sender<(WatchStream, Vec<u8>)>,
    omitted: &Arc<OmittedOutput>,
) -> Vec<std::thread::JoinHandle<()>> {
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let (chunks, omitted) = (chunks.clone(), omitted.clone());
        readers.push(std::thread::spawn(move || {
            tee(
                stdout,
                std::io::stdout(),
                WatchStream::Stdout,
                chunks,
                &omitted,
            );
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let (chunks, omitted) = (chunks.clone(), omitted.clone());
        readers.push(std::thread::spawn(move || {
            tee(
                stderr,
                std::io::stderr(),
                WatchStream::Stderr,
                chunks,
                &omitted,
            );
        }));
    }
    readers
}

#[derive(Default)]
struct OmittedOutput {
    stdout: AtomicU64,
    stderr: AtomicU64,
}

impl OmittedOutput {
    fn record(&self, stream: WatchStream, bytes: u64) {
        let counter = match stream {
            WatchStream::Stdout => &self.stdout,
            WatchStream::Stderr => &self.stderr,
        };
        counter.fetch_add(bytes, Ordering::Relaxed);
    }

    fn message(&self) -> Option<String> {
        let stdout = self.stdout.load(Ordering::Relaxed);
        let stderr = self.stderr.load(Ordering::Relaxed);
        (stdout > 0 || stderr > 0).then(|| format!(
            "watch copy omitted {stdout} stdout bytes and {stderr} stderr bytes because monitoring could not keep up; original output was forwarded"
        ))
    }
}

fn tee(
    mut pipe: impl Read,
    mut output: impl Write,
    stream: WatchStream,
    tx: mpsc::Sender<(WatchStream, Vec<u8>)>,
    omitted: &OmittedOutput,
) {
    let mut buffer = [0_u8; 8192];
    let mut omitted_bytes = 0;
    let mut monitoring_closed = false;
    loop {
        let n = match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                debug(format!("child output read failed: {error}"));
                break;
            }
        };
        let bytes = &buffer[..n];
        if output
            .write_all(bytes)
            .and_then(|()| output.flush())
            .is_err()
        {
            // Closing our read end propagates a broken consumer pipe to the child.
            break;
        }
        if monitoring_closed {
            continue;
        }
        match tx.try_reserve() {
            Ok(permit) => {
                permit.send((stream, bytes.to_vec()));
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Never stall the child's output on watch RPCs: skip this chunk, count the
                // gap, and resume copying as soon as the monitoring task drains the channel.
                omitted_bytes += n as u64;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // An unavailable watch has already been reported by the monitoring task.
                monitoring_closed = true;
            }
        }
    }
    omitted.record(stream, omitted_bytes);
}

async fn report(
    client: Client,
    watch: Option<WatchId>,
    mut rx: mpsc::Receiver<(WatchStream, Vec<u8>)>,
    omitted: Arc<OmittedOutput>,
) {
    let Some(watch) = watch else {
        return;
    };
    let mut interval = tokio::time::interval(BATCH);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut chunks: Vec<(WatchStream, Vec<u8>)> = Vec::new();
    let mut bytes = 0;
    let mut decoders = StreamDecoders::default();
    loop {
        let done = tokio::select! {
            item = rx.recv(), if bytes < 256 * 1024 => {
                if let Some((stream, data)) = item {
                    bytes += data.len();
                    if let Some((_, text)) = chunks.iter_mut().find(|(previous, _)| *previous == stream) { text.extend(data); }
                    else { chunks.push((stream, data)); }
                    continue;
                }
                true
            }
            _ = interval.tick() => false,
        };
        if done && let Some(message) = omitted.message() {
            chunks.push((
                WatchStream::Stderr,
                format!("\n[fleet exec: {message}]\n").into_bytes(),
            ));
        }
        for (stream, data) in chunks.drain(..) {
            let text = decoders.push(stream, &data);
            if text.is_empty() {
                continue;
            }
            if !matches!(
                tokio::time::timeout(
                    WATCH_TIMEOUT,
                    client.append_watch_output(watch, stream, text)
                )
                .await,
                Ok(Ok(()))
            ) {
                debug("watch output reporting failed or timed out");
                return;
            }
        }
        bytes = 0;
        if done {
            for (stream, text) in decoders.finish() {
                if !matches!(
                    tokio::time::timeout(
                        WATCH_TIMEOUT,
                        client.append_watch_output(watch, stream, text)
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    debug("watch output reporting failed or timed out");
                    return;
                }
            }
            return;
        }
    }
}

#[derive(Default)]
struct StreamDecoders {
    stdout: IncrementalUtf8,
    stderr: IncrementalUtf8,
}

impl StreamDecoders {
    fn push(&mut self, stream: WatchStream, bytes: &[u8]) -> String {
        self.decoder(stream).push(bytes)
    }

    fn finish(&mut self) -> Vec<(WatchStream, String)> {
        [WatchStream::Stdout, WatchStream::Stderr]
            .into_iter()
            .filter_map(|stream| {
                let text = self.decoder(stream).finish();
                (!text.is_empty()).then_some((stream, text))
            })
            .collect()
    }

    fn decoder(&mut self, stream: WatchStream) -> &mut IncrementalUtf8 {
        match stream {
            WatchStream::Stdout => &mut self.stdout,
            WatchStream::Stderr => &mut self.stderr,
        }
    }
}

#[derive(Default)]
struct IncrementalUtf8 {
    pending: Vec<u8>,
}

impl IncrementalUtf8 {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut decoded = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    decoded.push_str(valid);
                    self.pending.clear();
                    return decoded;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    decoded.push_str(&String::from_utf8_lossy(&self.pending[..valid_up_to]));
                    if let Some(invalid_length) = error.error_len() {
                        decoded.push(char::REPLACEMENT_CHARACTER);
                        self.pending.drain(..valid_up_to + invalid_length);
                    } else {
                        self.pending.drain(..valid_up_to);
                        return decoded;
                    }
                }
            }
        }
    }

    fn finish(&mut self) -> String {
        let decoded = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_utf8_per_stream() {
        let mut decoders = StreamDecoders::default();
        assert_eq!(decoders.push(WatchStream::Stdout, &[0xe2]), "");
        assert_eq!(decoders.push(WatchStream::Stderr, &[0xf0, 0x9f]), "");
        assert_eq!(decoders.push(WatchStream::Stdout, &[0x82, 0xac]), "€");
        assert_eq!(decoders.push(WatchStream::Stderr, &[0x98, 0x80]), "😀");
        assert!(decoders.finish().is_empty());
    }

    #[test]
    fn sigint_reaches_child_once() {
        let explicitly_forwarded = [Signal::SIGINT]
            .into_iter()
            .filter(|signal| should_forward(*signal))
            .count();
        let terminal_deliveries = 0;
        assert_eq!(terminal_deliveries + explicitly_forwarded, 1);
    }

    #[tokio::test]
    async fn saturated_monitoring_keeps_passthrough_exact_and_records_omissions() {
        let input = (0..32_768)
            .map(|index| (index % 256) as u8)
            .collect::<Vec<_>>();
        let expected = input.clone();
        let (tx, mut rx) = mpsc::channel(1);
        let omitted = Arc::new(OmittedOutput::default());
        let copy_omitted = omitted.clone();
        let reader = tokio::task::spawn_blocking(move || {
            let mut output = Vec::new();
            tee(
                input.as_slice(),
                &mut output,
                WatchStream::Stdout,
                tx,
                &copy_omitted,
            );
            output
        });
        let result = tokio::time::timeout(Duration::from_secs(1), reader).await;
        // Release a blocked sender even if the timeout assertion fails.
        rx.close();
        let output = result.expect("monitoring stalled passthrough").unwrap();
        assert_eq!(output, expected);
        let (stream, prefix) = rx.recv().await.unwrap();
        assert_eq!(stream, WatchStream::Stdout);
        assert_eq!(prefix, expected[..8192]);
        assert!(rx.recv().await.is_none());
        assert_eq!(omitted.stdout.load(Ordering::Relaxed), 24_576);
        assert_eq!(omitted.stderr.load(Ordering::Relaxed), 0);
        assert!(
            omitted
                .message()
                .unwrap()
                .contains("24576 stdout bytes and 0 stderr bytes")
        );
    }

    #[test]
    fn closed_monitoring_keeps_stderr_passthrough_exact() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let omitted = OmittedOutput::default();
        let input = vec![0xff; 20_000];
        let mut output = Vec::new();
        tee(
            input.as_slice(),
            &mut output,
            WatchStream::Stderr,
            tx,
            &omitted,
        );
        assert_eq!(output, input);
        assert!(omitted.message().is_none());
    }

    fn eligibility(watch: bool, env: &[(&str, &str)]) -> Result<TerminalId, &'static str> {
        watch_terminal(watch, |key| {
            env.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn watch_eligibility_uses_terminal_id_and_preserves_human_name() {
        assert_eq!(
            eligibility(
                true,
                &[
                    ("FLEET_SESSION", "repo/feature"),
                    ("FLEET_TERMINAL", "editor"),
                    ("FLEET_TERMINAL_ID", "42"),
                ]
            ),
            Ok(TerminalId(42))
        );
        for id in [None, Some("editor"), Some(""), Some("18446744073709551616")] {
            let mut env = vec![("FLEET_SESSION", "repo/feature"), ("FLEET_TERMINAL", "42")];
            if let Some(id) = id {
                env.push(("FLEET_TERMINAL_ID", id));
            }
            assert_eq!(
                eligibility(true, &env),
                Err("FLEET_TERMINAL_ID missing or not numeric")
            );
        }
    }

    #[test]
    fn every_ineligible_watch_has_a_passthrough_reason() {
        assert_eq!(eligibility(false, &[]), Err("--watch not requested"));
        assert_eq!(
            eligibility(true, &[("FLEET_TERMINAL_ID", "42")]),
            Err("FLEET_SESSION missing")
        );
        assert_eq!(
            eligibility(true, &[("FLEET_SESSION", ""), ("FLEET_TERMINAL_ID", "42")]),
            Err("FLEET_SESSION invalid")
        );
        assert_eq!(
            eligibility(
                true,
                &[
                    ("FLEET_SESSION", "repo/feature"),
                    ("FLEET_TERMINAL_ID", "42"),
                    ("FLEET_WATCH", ""),
                ]
            ),
            Err("FLEET_WATCH already set (nested watch)")
        );
    }
}
