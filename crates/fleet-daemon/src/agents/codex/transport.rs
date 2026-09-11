//! The Codex stdio transport: id allocation, two pending maps, and one-shot termination.
//!
//! Properties that are the point of this file:
//!
//! - **Client request ids are monotonic integers from 1, per connection**, and the server has an
//!   independent id space that may collide numerically. Correlation is **by direction**, never by
//!   id alone, and responses match on the id's string form so `1` and `"1"` are the same request.
//! - **There is no request timeout in the protocol.** Every deadline is Fleet's, and each is
//!   strictly less than the `fleet-proto` timeout of the client call that triggered it.
//! - **A pending gate must not block the read loop.** Inbound requests are dispatched to their
//!   own tasks, the concurrency is capped (gates are human-latency), and the cap+1th request is
//!   answered `-32001` immediately rather than left to hang.
//! - **Termination is one-shot and fans out**: every pending request fails exactly once with one
//!   classified error, in-flight gate handlers are cancelled, and every later send fails with the
//!   stored error.
//! - **An unroutable line is a counted warning, not a session kill.**

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
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
    approvals,
    envelope::{
        self, Inbound, OutboundError, OutboundNotification, OutboundRequest, OutboundResponse,
    },
    map,
    session::CodexSession,
};
use crate::agents::harness::{
    HarnessError, HarnessEvent, HarnessResult, HarnessSink, RawRef,
    ndjson::{Line, LineSplitter},
    process::{self, PeerStreams},
};

/// The reader's hard cap. Messages can legitimately be multi-megabyte — a 4 MiB
/// `turn/diff/updated` is a real upstream test case — so the cap is high, and exceeding it is a
/// fatal transport error rather than an OOM.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// How many inbound requests may be in flight. Gates are human-latency, so the cap is small.
const MAX_INBOUND: usize = 16;

/// How long one write may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// A pending outbound request's answer.
type Answer = Result<Value, (i64, String)>;

/// The serialized writer over the child's stdin.
#[derive(Debug, Clone)]
pub(super) struct Writer {
    sender: mpsc::Sender<WriteCommand>,
}

#[derive(Debug)]
enum WriteCommand {
    Line(String, oneshot::Sender<Result<(), String>>),
    Close(oneshot::Sender<Result<(), String>>),
}

impl Writer {
    async fn write_line(&self, line: String) -> HarnessResult<()> {
        let (accepted, result) = oneshot::channel();
        tokio::time::timeout(
            WRITE_TIMEOUT,
            self.sender.send(WriteCommand::Line(line, accepted)),
        )
        .await
        .map_err(|_| HarnessError::Timeout {
            what: "the Codex stdin writer",
            after: WRITE_TIMEOUT,
        })?
        .map_err(|_| HarnessError::Exited {
            code: None,
            signal: None,
        })?;
        tokio::time::timeout(WRITE_TIMEOUT, result)
            .await
            .map_err(|_| HarnessError::Timeout {
                what: "a Codex stdin write",
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

    /// Closes stdin.
    pub(super) async fn close(&self) {
        let (accepted, result) = oneshot::channel();
        if self
            .sender
            .send(WriteCommand::Close(accepted))
            .await
            .is_err()
        {
            return;
        }
        // Fire-and-forget: the writer having gone away is the state this asked for.
        let _writer_already_gone = result.await;
    }
}

/// Everything the reader shares with the handlers it spawns.
#[derive(Clone)]
struct Shared {
    session: Arc<Mutex<CodexSession>>,
    events: HarnessSink,
    writer: Writer,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Answer>>>>,
    inbound: Arc<Mutex<HashMap<String, AbortHandle>>>,
    terminated: Arc<Mutex<Option<HarnessError>>>,
    unroutable: Arc<AtomicU64>,
}

/// One live Codex transport.
pub(super) struct Transport {
    writer: Writer,
    next_id: AtomicU64,
    shared: Shared,
    child: Arc<Mutex<Option<Child>>>,
    expected_stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
    stderr: Option<JoinHandle<()>>,
}

impl Transport {
    /// Starts the reader, writer and stderr classifier over an already-open peer.
    pub(super) fn start(
        peer: PeerStreams,
        session: Arc<Mutex<CodexSession>>,
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
        let shared = Shared {
            session,
            events: events.clone(),
            writer: writer.clone(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            inbound: Arc::new(Mutex::new(HashMap::new())),
            terminated: Arc::new(Mutex::new(None)),
            unroutable: Arc::new(AtomicU64::new(0)),
        };
        let child = Arc::new(Mutex::new(process));
        let expected_stop = Arc::new(AtomicBool::new(false));
        // The writer needs no handle: it ends when the last `Writer` is dropped and its channel
        // closes, which happens with the transport itself. The reader and the stderr drain are
        // held, because both would otherwise outlive a replaced session.
        tokio::spawn(write_loop(stdin, receiver));
        let reader = tokio::spawn(read_loop(
            stdout,
            shared.clone(),
            Arc::clone(&child),
            Arc::clone(&expected_stop),
        ));
        // stderr is drained from the moment of spawn or Codex blocks on a full pipe: there is an
        // upstream regression test for exactly this — 512 KiB of stderr before the `initialize`
        // response must not deadlock.
        let stderr = stderr.map(|stderr| tokio::spawn(classify_stderr(stderr, events)));
        Self {
            writer,
            next_id: AtomicU64::new(1),
            shared,
            child,
            expected_stop,
            reader,
            stderr,
        }
    }

    /// Sends a request and awaits its answer, within `deadline`.
    pub(super) async fn request(
        &self,
        method: &'static str,
        params: Option<Value>,
        deadline: Duration,
    ) -> HarnessResult<Value> {
        if let Some(error) = self.shared.terminated.lock().await.clone() {
            return Err(error);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .await
            .insert(id.to_string(), sender);
        let frame = OutboundRequest { id, method, params };
        let line = serde_json::to_string(&frame).map_err(|error| HarnessError::Request {
            method: method.to_owned(),
            code: None,
            detail: error.to_string(),
        })?;
        if let Err(error) = self.writer.write_line(line).await {
            self.shared.pending.lock().await.remove(&id.to_string());
            return Err(error);
        }
        match tokio::time::timeout(deadline, receiver).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err((code, detail)))) => Err(HarnessError::Request {
                method: method.to_owned(),
                code: Some(code),
                detail,
            }),
            // The sender was dropped: termination fanned out and stored the classified error.
            Ok(Err(_)) => {
                Err(self
                    .shared
                    .terminated
                    .lock()
                    .await
                    .clone()
                    .unwrap_or(HarnessError::Exited {
                        code: None,
                        signal: None,
                    }))
            }
            Err(_) => {
                self.shared.pending.lock().await.remove(&id.to_string());
                Err(HarnessError::Timeout {
                    what: method_label(method),
                    after: deadline,
                })
            }
        }
    }

    /// Sends a notification.
    pub(super) async fn notify(
        &self,
        method: &'static str,
        params: Option<Value>,
    ) -> HarnessResult<()> {
        let frame = OutboundNotification { method, params };
        let line = serde_json::to_string(&frame).map_err(|error| HarnessError::Request {
            method: method.to_owned(),
            code: None,
            detail: error.to_string(),
        })?;
        self.writer.write_line(line).await
    }

    /// Records that the next exit is Fleet's own doing.
    pub(super) fn expect_stop(&self) {
        self.expected_stop.store(true, Ordering::Release);
    }

    /// Kills the child, after every response the protocol owes has been written.
    pub(super) async fn terminate(&self) -> Option<i32> {
        self.expect_stop();
        for (_, handle) in self.shared.inbound.lock().await.drain() {
            handle.abort();
        }
        self.writer.close().await;
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

    /// The serialized writer, for answering a server request the adapter owns.
    pub(super) fn writer(&self) -> &Writer {
        &self.writer
    }

    /// How many unroutable lines this connection has seen.
    #[cfg(test)]
    pub(super) fn unroutable_lines(&self) -> u64 {
        self.shared.unroutable.load(Ordering::Relaxed)
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

/// A label for a timeout, without allocating per call.
fn method_label(method: &str) -> &'static str {
    match method {
        "initialize" => "the Codex handshake",
        "thread/start" => "starting a Codex thread",
        "thread/resume" => "resuming a Codex thread",
        "turn/start" => "sending a Codex turn",
        "turn/steer" => "steering a Codex turn",
        "turn/interrupt" => "interrupting a Codex turn",
        "thread/compact/start" => "compacting a Codex thread",
        _ => "a Codex request",
    }
}

async fn read_loop(
    mut stdout: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    shared: Shared,
    child: Arc<Mutex<Option<Child>>>,
    expected_stop: Arc<AtomicBool>,
) {
    let mut splitter = LineSplitter::new(MAX_FRAME);
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut fatal: Option<HarnessError> = None;
    loop {
        let read = match stdout.read(&mut buffer).await {
            Ok(0) => {
                if let Some(line) = splitter.finish() {
                    handle_line(&shared, line).await;
                }
                break;
            }
            Ok(read) => read,
            Err(error) => {
                fatal = Some(HarnessError::Request {
                    method: "stdout".to_owned(),
                    code: None,
                    detail: error.to_string(),
                });
                break;
            }
        };
        for line in splitter.push(&buffer[..read]) {
            handle_line(&shared, line).await;
        }
    }
    // Classify the cause once, then fan it out to every pending request exactly once.
    let code = {
        let mut child = child.lock().await;
        match child.as_mut() {
            Some(process) => match process.try_wait() {
                Ok(Some(status)) => process::exit_code(&status),
                Ok(None) => process::terminate(process, process::TERMINATE_GRACE).await,
                Err(_) => None,
            },
            None => None,
        }
    };
    // The precedence when both are available: a transport failure is the more specific
    // explanation than the exit code.
    let cause = fatal.unwrap_or(HarnessError::Exited { code, signal: None });
    {
        let mut terminated = shared.terminated.lock().await;
        if terminated.is_none() {
            *terminated = Some(cause.clone());
        }
    }
    for (_, waiter) in shared.pending.lock().await.drain() {
        // Fire-and-forget: a caller that stopped waiting needs no answer.
        let _caller_gave_up = waiter.send(Err((0, cause.to_string())));
    }
    for (_, handle) in shared.inbound.lock().await.drain() {
        handle.abort();
    }
    let expected = expected_stop.load(Ordering::Acquire);
    let mut events = Vec::new();
    {
        let mut session = shared.session.lock().await;
        // A gate nobody can answer must not stay open, or the thread can never settle.
        for (gate, pending) in session.drain_gates() {
            events.push(AgentEvent::GateResolved {
                gate,
                answer: CodexSession::closed_answer(&pending.shape),
                by: fleet_core::agents::GateResolver::ProviderClosed,
            });
        }
        if let Some(turn) = session.active_turn {
            session.close_turn_items(turn, fleet_core::agents::ItemStatus::Stopped, &mut events);
            events.push(AgentEvent::TurnAborted {
                turn,
                reason: fleet_core::agents::AbortReason::ProviderExited,
            });
            session.settle_turn(turn);
        }
    }
    if !expected {
        events.push(AgentEvent::RuntimeError {
            fatal: true,
            message: "Codex stopped answering before the turn completed".to_owned(),
        });
    }
    events.push(AgentEvent::SessionExited { code, expected });
    for event in events {
        emit(&shared.events, event, Some("transport_terminated"));
    }
}

async fn handle_line(shared: &Shared, line: Line) {
    let text = match line {
        Line::Frame(text) => text,
        Line::Oversized(bytes) => {
            tracing::error!(
                target: "fleet::agents::codex",
                bytes,
                "a Codex frame passed the transport's hard cap"
            );
            return;
        }
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            let fingerprint =
                crate::agents::harness::fingerprint::SchemaFingerprint::of_unparsable(&error);
            let seen = shared.unroutable.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                target: "fleet::agents::codex",
                unroutable = seen,
                fingerprint = %fingerprint.summary(),
                "a Codex stdout line was not JSON"
            );
            return;
        }
    };
    let Some(inbound) = envelope::classify(value) else {
        let seen = shared.unroutable.fetch_add(1, Ordering::Relaxed) + 1;
        // Counted, never fatal: the protocol shares stdout with anything that might write a
        // stray line.
        tracing::warn!(
            target: "fleet::agents::codex",
            unroutable = seen,
            "a Codex stdout line was not an envelope"
        );
        return;
    };
    match inbound {
        Inbound::Response { id, result } => {
            let key = envelope::correlation_key(&id);
            // An unknown id is dropped silently; a duplicate is dropped because the first
            // response already took the waiter.
            if let Some(waiter) = shared.pending.lock().await.remove(&key) {
                let _caller_gave_up = waiter.send(Ok(result));
            }
        }
        Inbound::Error { id, code, message } => {
            let key = envelope::correlation_key(&id);
            if let Some(waiter) = shared.pending.lock().await.remove(&key) {
                let _caller_gave_up = waiter.send(Err((code, message)));
            }
        }
        Inbound::Notification {
            method,
            params,
            emitted_at_ms,
        } => {
            let output = {
                let mut session = shared.session.lock().await;
                map::handle(&mut session, &method, &params)
            };
            let emitted = emitted_at_ms.and_then(|ms| {
                u64::try_from(ms)
                    .ok()
                    .map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms))
            });
            for event in output.events {
                let event = HarnessEvent::now(event, Some(RawRef::method(method.clone())))
                    .emitted_at(emitted);
                let _receiver_gone_at_shutdown = shared.events.send(event);
            }
            for follow_up in output.follow_up {
                tracing::debug!(
                    target: "fleet::agents::codex",
                    follow_up,
                    "a Codex notification asked for a follow-up read"
                );
            }
        }
        Inbound::Request { id, method, params } => {
            dispatch_request(shared, id, method, params).await;
        }
    }
}

/// Dispatches one inbound request to its own task, or refuses it when the budget is full.
async fn dispatch_request(shared: &Shared, id: Value, method: String, params: Value) {
    let key = envelope::correlation_key(&id);
    {
        let inbound = shared.inbound.lock().await;
        if inbound.len() >= MAX_INBOUND {
            drop(inbound);
            tracing::warn!(
                target: "fleet::agents::codex",
                method = %method,
                "refusing a Codex request: the inbound handler budget is full"
            );
            write_error(shared, &id, envelope::SERVER_BUSY, "Fleet is busy").await;
            return;
        }
    }
    let shared_for_task = shared.clone();
    let key_for_task = key.clone();
    let task = tokio::spawn(async move {
        let outcome = {
            let mut session = shared_for_task.session.lock().await;
            approvals::handle(&mut session, &method, &id, &params)
        };
        for event in outcome.events {
            emit(&shared_for_task.events, event, Some(&method));
        }
        if let Some((code, message)) = outcome.error {
            write_error(&shared_for_task, &id, code, message).await;
        } else if let Some(result) = outcome.immediate {
            write_result(&shared_for_task.writer, &id, result).await;
        }
        shared_for_task.inbound.lock().await.remove(&key_for_task);
    });
    shared.inbound.lock().await.insert(key, task.abort_handle());
}

/// Writes a result for one server request.
pub(super) async fn write_result(shared_writer: &Writer, id: &Value, result: Value) {
    let frame = OutboundResponse {
        id: id.clone(),
        result,
    };
    match serde_json::to_string(&frame) {
        Ok(line) => {
            if let Err(error) = shared_writer.write_line(line).await {
                tracing::warn!(
                    target: "fleet::agents::codex",
                    %error,
                    "could not answer a Codex request"
                );
            }
        }
        Err(error) => tracing::warn!(
            target: "fleet::agents::codex",
            %error,
            "could not encode a Codex response"
        ),
    }
}

async fn write_error(shared: &Shared, id: &Value, code: i64, message: &'static str) {
    let frame = OutboundError {
        id: id.clone(),
        error: envelope::ErrorBody { code, message },
    };
    match serde_json::to_string(&frame) {
        Ok(line) => {
            if let Err(error) = shared.writer.write_line(line).await {
                tracing::warn!(
                    target: "fleet::agents::codex",
                    %error,
                    "could not refuse a Codex request"
                );
            }
        }
        Err(error) => tracing::warn!(
            target: "fleet::agents::codex",
            %error,
            "could not encode a Codex error response"
        ),
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
            WriteCommand::Line(line, accepted) => {
                let result = async {
                    stdin
                        .write_all(line.as_bytes())
                        .await
                        .map_err(|error| error.to_string())?;
                    stdin
                        .write_all(b"\n")
                        .await
                        .map_err(|error| error.to_string())?;
                    stdin.flush().await.map_err(|error| error.to_string())
                }
                .await;
                let failed = result.is_err();
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

/// Codex writes routine tracing to stderr, so it is classified rather than surfaced.
///
/// ANSI is stripped, structured lines below ERROR are dropped, the two known-benign ERROR lines
/// are dropped (Codex's own SQLite recovering, logged at ERROR), and
/// `"failed to connect to websocket"` is fatal.
async fn classify_stderr(
    stderr: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    events: HarnessSink,
) {
    use tokio::io::AsyncBufReadExt as _;

    let structured =
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\S+\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+\S+:\s+(.*)$");
    let benign = [
        "state db missing rollout path for thread",
        "state db record_discrepancy: find_thread_path_by_id_str_in_subdir, falling_back",
    ];
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = strip_ansi(&line);
        if line.trim().is_empty() {
            continue;
        }
        let (level, message) = match structured
            .as_ref()
            .ok()
            .and_then(|regex| regex.captures(&line))
        {
            Some(captures) => (
                captures
                    .get(1)
                    .map_or("", |level| level.as_str())
                    .to_owned(),
                captures.get(2).map_or("", |body| body.as_str()).to_owned(),
            ),
            // An unstructured line passes through as a notice.
            None => (String::from("ERROR"), line.clone()),
        };
        if level != "ERROR" {
            tracing::debug!(target: "fleet::agents::codex", "{message}");
            continue;
        }
        if benign.iter().any(|known| message.contains(known)) {
            tracing::debug!(target: "fleet::agents::codex", "{message}");
            continue;
        }
        if message.contains("failed to connect to websocket") {
            emit(
                &events,
                AgentEvent::RuntimeError {
                    fatal: true,
                    message: message.clone(),
                },
                Some("stderr"),
            );
            continue;
        }
        tracing::warn!(target: "fleet::agents::codex", "{message}");
        emit(&events, AgentEvent::Notice(message), Some("stderr"));
    }
}

/// Removes ANSI SGR sequences.
fn strip_ansi(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            clean.push(character);
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_classification_drops_everything_below_error() {
        assert_eq!(strip_ansi("\u{1b}[31mboom\u{1b}[0m"), "boom");
    }

    #[test]
    fn a_timeout_names_the_operation_and_not_the_method_string() {
        assert_eq!(method_label("turn/start"), "sending a Codex turn");
        assert_eq!(method_label("brand/new"), "a Codex request");
    }
}
