//! The Claude stdio transport: one reader, one serialized writer, two pending maps.
//!
//! Three properties are the whole point of this file.
//!
//! **Two pending maps, keyed by direction.** Inbound control requests awaiting a Fleet answer
//! (the gates) and outbound control requests awaiting Claude's answer (`interrupt`) live in
//! separate maps. The id spaces are CLI-minted and Fleet-minted respectively and may collide;
//! direction is the discriminator, never the id alone.
//!
//! **Inbound control requests must not block the read loop.** Each is dispatched to its own task
//! and the reader keeps consuming. A human staring at an approval card while deltas queue behind
//! it is the single most common way a naive implementation freezes the transcript.
//!
//! **A decode failure produces a degraded event, never silence** — and the warning it logs
//! carries a structural fingerprint only, never the frame.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use fleet_core::agents::AgentEvent;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Child,
    sync::{Mutex, mpsc, oneshot},
    task::{AbortHandle, JoinHandle},
};

use super::{
    frames::{ControlResponseBody, Frame, ParsedFrame, parse_frame},
    map,
    session::ClaudeSession,
};
use crate::agents::harness::{
    HarnessError, HarnessEvent, HarnessResult, HarnessSink, RawRef,
    ndjson::{Line, LineSplitter},
    process::{self, PeerStreams},
};

/// The largest single stdout line the transport will buffer.
///
/// A frame is legitimately megabytes (a whole-file `Write` input), so there is no small cap. This
/// is the bound that keeps a wedged child from growing the daemon's heap until the machine gives
/// out.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// How long one stdin write may take before the transport gives up on it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a decode-failure warning may repeat.
const WARN_EVERY: Duration = Duration::from_secs(5);

/// A serialized writer over the child's stdin.
///
/// Serialized through a channel rather than a mutex so a slow write cannot interleave two frames
/// on one line, and so the reader task can write a gate response without holding the session.
#[derive(Debug, Clone)]
pub(super) struct Writer {
    sender: mpsc::Sender<WriteCommand>,
}

#[derive(Debug)]
enum WriteCommand {
    Json(Value, oneshot::Sender<Result<(), String>>),
    Close(oneshot::Sender<Result<(), String>>),
}

impl Writer {
    /// Writes one frame and waits for it to reach the child.
    pub(super) async fn write(&self, value: Value) -> HarnessResult<()> {
        let (accepted, result) = oneshot::channel();
        tokio::time::timeout(
            WRITE_TIMEOUT,
            self.sender.send(WriteCommand::Json(value, accepted)),
        )
        .await
        .map_err(|_| HarnessError::Timeout {
            what: "the Claude stdin writer",
            after: WRITE_TIMEOUT,
        })?
        .map_err(|_| HarnessError::Exited {
            code: None,
            signal: None,
        })?;
        tokio::time::timeout(WRITE_TIMEOUT, result)
            .await
            .map_err(|_| HarnessError::Timeout {
                what: "a Claude stdin write",
                after: WRITE_TIMEOUT,
            })?
            .map_err(|_| HarnessError::Exited {
                code: None,
                signal: None,
            })?
            .map_err(|detail| HarnessError::Request {
                method: "stdin".to_owned(),
                code: None,
                detail,
            })
    }

    /// Closes the child's stdin, which ends the session.
    pub(super) async fn close(&self) -> HarnessResult<()> {
        let (accepted, result) = oneshot::channel();
        if tokio::time::timeout(
            WRITE_TIMEOUT,
            self.sender.send(WriteCommand::Close(accepted)),
        )
        .await
        .map_err(|_| HarnessError::Timeout {
            what: "the Claude stdin writer",
            after: WRITE_TIMEOUT,
        })?
        .is_err()
        {
            // The writer is already gone, which is the state `close` was asking for.
            return Ok(());
        }
        match tokio::time::timeout(WRITE_TIMEOUT, result).await {
            Ok(Ok(Ok(()))) | Ok(Err(_)) => Ok(()),
            Ok(Ok(Err(detail))) => Err(HarnessError::Request {
                method: "stdin".to_owned(),
                code: None,
                detail,
            }),
            Err(_) => Err(HarnessError::Timeout {
                what: "the Claude stdin close",
                after: WRITE_TIMEOUT,
            }),
        }
    }
}

/// Outbound control requests awaiting Claude's `control_response`.
type Outbound = Arc<Mutex<HashMap<String, oneshot::Sender<ControlResponseBody>>>>;

/// Inbound control requests Fleet is answering, so a cancel can stop the handler.
type Inbound = Arc<Mutex<HashMap<String, AbortHandle>>>;

/// One live Claude transport.
pub(super) struct Transport {
    /// The serialized stdin writer.
    pub(super) writer: Writer,
    outbound: Outbound,
    inbound: Inbound,
    child: Arc<Mutex<Option<Child>>>,
    expected_stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
    stderr: Option<JoinHandle<()>>,
}

impl Transport {
    /// Starts the reader, the writer and the stderr drain over an already-open peer.
    pub(super) fn start(
        peer: PeerStreams,
        session: Arc<Mutex<ClaudeSession>>,
        events: HarnessSink,
    ) -> Self {
        let PeerStreams {
            stdin,
            stdout,
            stderr,
            process,
        } = peer;
        let (sender, receiver) = mpsc::channel(64);
        let writer = Writer { sender };
        let outbound: Outbound = Arc::new(Mutex::new(HashMap::new()));
        let inbound: Inbound = Arc::new(Mutex::new(HashMap::new()));
        let child = Arc::new(Mutex::new(process));
        let expected_stop = Arc::new(AtomicBool::new(false));

        // The writer needs no handle: it ends when the last `Writer` is dropped and its channel
        // closes, which happens with the transport itself. The reader and the stderr drain are
        // held, because both would otherwise outlive a replaced session.
        tokio::spawn(write_loop(stdin, receiver));
        let shared = Shared {
            session: Arc::clone(&session),
            events: events.clone(),
            writer: writer.clone(),
            outbound: Arc::clone(&outbound),
            inbound: Arc::clone(&inbound),
            child: Arc::clone(&child),
            expected_stop: Arc::clone(&expected_stop),
        };
        let reader = tokio::spawn(read_loop(stdout, shared));
        // stderr is drained from the moment of spawn: an undrained pipe is a deadlock.
        let stderr = stderr.map(|stderr| tokio::spawn(drain_stderr(stderr)));

        Self {
            writer,
            outbound,
            inbound,
            child,
            expected_stop,
            reader,
            stderr,
        }
    }

    /// Writes an outbound control request and returns the receiver for its receipt.
    pub(super) async fn control_request(
        &self,
        request_id: String,
        frame: Value,
    ) -> HarnessResult<oneshot::Receiver<ControlResponseBody>> {
        let (sender, receiver) = oneshot::channel();
        self.outbound.lock().await.insert(request_id, sender);
        self.writer.write(frame).await?;
        Ok(receiver)
    }

    /// Records that the next exit is Fleet's own doing, so it is reported as expected.
    pub(super) fn expect_stop(&self) {
        self.expected_stop.store(true, Ordering::Release);
    }

    /// Kills the child, after everything the protocol owed it has been written.
    pub(super) async fn terminate(&self) -> Option<i32> {
        self.expect_stop();
        // In-flight gate handlers cannot write to a socket that is about to die.
        for (_, handle) in self.inbound.lock().await.drain() {
            handle.abort();
        }
        let mut child = self.child.lock().await;
        match child.as_mut() {
            Some(process) => process::terminate(process, process::TERMINATE_GRACE).await,
            None => None,
        }
    }

    /// Whether the child is still running.
    pub(super) async fn is_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        match child.as_mut() {
            Some(process) => matches!(process.try_wait(), Ok(None)),
            None => false,
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
        if let Some(stderr) = &self.stderr {
            stderr.abort();
        }
    }
}

/// Everything the reader task shares with the handlers it spawns.
///
/// The child's stdout is deliberately **not** in here: a `Box<dyn AsyncRead + Send>` is not
/// `Sync`, so holding a reference to it across an `await` would make the whole reader future
/// non-`Send` and unspawnable.
#[derive(Clone)]
struct Shared {
    session: Arc<Mutex<ClaudeSession>>,
    events: HarnessSink,
    writer: Writer,
    outbound: Outbound,
    inbound: Inbound,
    child: Arc<Mutex<Option<Child>>>,
    expected_stop: Arc<AtomicBool>,
}

async fn read_loop(mut stdout: Box<dyn tokio::io::AsyncRead + Send + Unpin>, shared: Shared) {
    let mut splitter = LineSplitter::new(MAX_FRAME);
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut warnings = Warnings::default();
    loop {
        let read = match stdout.read(&mut buffer).await {
            Ok(0) => {
                // A final unterminated line at EOF is processed **before** termination.
                if let Some(line) = splitter.finish() {
                    handle_line(&shared, line, &mut warnings).await;
                }
                break;
            }
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(
                    target: "fleet::agents::claude",
                    %error,
                    "the Claude stdout stream failed"
                );
                break;
            }
        };
        for line in splitter.push(&buffer[..read]) {
            handle_line(&shared, line, &mut warnings).await;
        }
    }
    // stdout EOF is **process lifetime, never a turn boundary**: the exit is what settles the
    // session, and the turn it interrupted is settled by the teardown below.
    let code = {
        let mut child = shared.child.lock().await;
        match child.as_mut() {
            Some(process) => match process.try_wait() {
                Ok(Some(status)) => process::exit_code(&status),
                Ok(None) => process::terminate(process, process::TERMINATE_GRACE).await,
                Err(_) => None,
            },
            None => None,
        }
    };
    let expected = shared.expected_stop.load(Ordering::Acquire);
    let mut events = {
        let mut session = shared.session.lock().await;
        let mut events =
            session.settle_open_gates(fleet_core::agents::GateResolver::ProviderClosed);
        events.extend(session.process_exit(code, expected));
        events
    };
    // `SessionExited` is last. Nothing follows it.
    events.sort_by_key(|event| u8::from(matches!(event, AgentEvent::SessionExited { .. })));
    for event in events {
        emit(&shared.events, event, Some("process_exit"));
    }
}

/// Rate-limited decode-failure accounting.
#[derive(Debug, Default)]
struct Warnings {
    count: u64,
    last: Option<Instant>,
}

impl Warnings {
    /// Whether this failure should be logged, given how recently one was.
    fn should_log(&mut self) -> bool {
        self.count = self.count.saturating_add(1);
        let due = self.last.is_none_or(|last| last.elapsed() >= WARN_EVERY);
        if due {
            self.last = Some(Instant::now());
        }
        due
    }
}

async fn handle_line(shared: &Shared, line: Line, warnings: &mut Warnings) {
    let text = match line {
        Line::Frame(text) => text,
        Line::Oversized(bytes) => {
            tracing::warn!(
                target: "fleet::agents::claude",
                bytes,
                "dropped a Claude stdout line past the frame budget"
            );
            return;
        }
    };
    match parse_frame(&text) {
        ParsedFrame::Undecodable { kind, fingerprint } => {
            if warnings.should_log() {
                // The fingerprint is counts and field *names*. Never the frame.
                tracing::warn!(
                    target: "fleet::agents::claude",
                    frame = %kind,
                    failures = warnings.count,
                    fingerprint = %fingerprint.summary(),
                    "could not decode a Claude frame"
                );
            }
            emit(
                &shared.events,
                AgentEvent::Unknown {
                    method: kind.clone(),
                },
                Some(&kind),
            );
        }
        ParsedFrame::Frame(boxed) if matches!(*boxed, Frame::ControlResponse(_)) => {
            let Frame::ControlResponse(frame) = *boxed else {
                return;
            };
            let Some(request_id) = frame.response.request_id.clone() else {
                return;
            };
            let waiter = shared.outbound.lock().await.remove(&request_id);
            match waiter {
                // Fire-and-forget: the receiver is gone only when the caller stopped waiting,
                // which needs no answer.
                Some(waiter) => {
                    let _unwatched = waiter.send(frame.response);
                }
                None => {
                    let output = {
                        let mut session = shared.session.lock().await;
                        map::handle(&mut session, Frame::ControlResponse(frame))
                    };
                    deliver(shared, output).await;
                }
            }
        }
        ParsedFrame::Frame(boxed) if matches!(*boxed, Frame::ControlRequest(_)) => {
            let Frame::ControlRequest(frame) = *boxed else {
                return;
            };
            // Its own task, so a human deciding cannot freeze the transcript.
            let request_id = frame.request_id.clone();
            let session = Arc::clone(&shared.session);
            let events = shared.events.clone();
            let writer = shared.writer.clone();
            let inbound = Arc::clone(&shared.inbound);
            let task = tokio::spawn(async move {
                let output = {
                    let mut session = session.lock().await;
                    map::handle(&mut session, Frame::ControlRequest(frame))
                };
                let raw = output.raw.clone();
                for event in output.events {
                    emit(&events, event, raw.as_deref());
                }
                for write in output.writes {
                    if let Err(error) = writer.write(write).await {
                        tracing::warn!(
                            target: "fleet::agents::claude",
                            %error,
                            "could not answer a Claude control request"
                        );
                        break;
                    }
                }
                inbound.lock().await.remove(&request_id);
            });
            shared
                .inbound
                .lock()
                .await
                .insert(frame_id(&text), task.abort_handle());
        }
        ParsedFrame::Frame(frame) => {
            let frame = *frame;
            // A withdrawn request's handler must stop before it writes a response to a request
            // the CLI is no longer waiting for.
            if let Frame::ControlCancel(cancel) = &frame
                && let Some(handle) = shared.inbound.lock().await.remove(&cancel.request_id)
            {
                handle.abort();
            }
            let output = {
                let mut session = shared.session.lock().await;
                map::handle(&mut session, frame)
            };
            deliver(shared, output).await;
        }
    }
}

/// The `request_id` of a raw control-request line, for the in-flight map.
fn frame_id(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default()
}

async fn deliver(shared: &Shared, output: map::MapOutput) {
    let raw = output.raw.clone();
    for event in output.events {
        emit(&shared.events, event, raw.as_deref());
    }
    for write in output.writes {
        if let Err(error) = shared.writer.write(write).await {
            tracing::warn!(
                target: "fleet::agents::claude",
                %error,
                "could not write a Claude reply"
            );
            break;
        }
    }
}

/// Sends one event, or notices that the thread runtime is gone.
pub(super) fn emit(events: &HarnessSink, event: AgentEvent, raw: Option<&str>) {
    // Fire-and-forget: the receiver is dropped only when the thread runtime has gone away, which
    // happens during shutdown and needs no answer here.
    let _receiver_gone_at_shutdown = events.send(HarnessEvent::now(event, raw.map(RawRef::method)));
}

async fn write_loop(
    mut stdin: Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    mut receiver: mpsc::Receiver<WriteCommand>,
) {
    while let Some(command) = receiver.recv().await {
        match command {
            WriteCommand::Json(value, accepted) => {
                let result = async {
                    let mut bytes =
                        serde_json::to_vec(&value).map_err(|error| error.to_string())?;
                    bytes.push(b'\n');
                    stdin
                        .write_all(&bytes)
                        .await
                        .map_err(|error| error.to_string())?;
                    stdin.flush().await.map_err(|error| error.to_string())
                }
                .await;
                let failed = result.is_err();
                // Fire-and-forget: a caller that stopped waiting needs no answer.
                let _caller_gave_up = accepted.send(result);
                if failed {
                    return;
                }
            }
            WriteCommand::Close(accepted) => {
                let result = stdin.shutdown().await.map_err(|error| error.to_string());
                let _caller_gave_up = accepted.send(result);
                return;
            }
        }
    }
}

/// Drains stderr into the log, one line at a time, and never blocks the protocol.
async fn drain_stderr(stderr: Box<dyn tokio::io::AsyncRead + Send + Unpin>) {
    use tokio::io::AsyncBufReadExt as _;

    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        tracing::warn!(target: "fleet::agents::claude", "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decode_warning_is_counted_every_time_and_logged_rarely() {
        let mut warnings = Warnings::default();
        assert!(warnings.should_log(), "the first failure is always logged");
        assert!(!warnings.should_log());
        assert!(!warnings.should_log());
        assert_eq!(warnings.count, 3, "every failure is counted");
    }

    #[test]
    fn the_in_flight_key_comes_from_the_frames_own_request_id() {
        assert_eq!(
            frame_id(r#"{"type":"control_request","request_id":"abc","request":{}}"#),
            "abc"
        );
        assert_eq!(frame_id("not json"), "");
    }
}
