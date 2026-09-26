//! Scriptable machine provider and remote endpoint test doubles.

use std::{
    collections::VecDeque,
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use fleet_proto::{
    event::Event,
    request::RequestBody,
    response::ResponseBody,
    snapshot::{LinkState, Snapshot},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf},
    sync::{broadcast, watch},
    time::{Instant, Sleep},
};

use crate::{
    DaemonError, DaemonResult,
    machines::{
        AsyncDuplex, ExecOutput, MachineAddress, MachineError, MachineProvider, ProbeReport,
        RemoteEndpoint, RemoteHello,
    },
};

/// Scriptable provider with argv recording and an in-memory duplex peer.
pub struct FakeMachine {
    id: HostId,
    resolves: Mutex<VecDeque<Result<MachineAddress, MachineError>>>,
    probes: Mutex<VecDeque<ProbeReport>>,
    execs: Mutex<VecDeque<Result<ExecOutput, MachineError>>>,
    calls: Mutex<Vec<Vec<String>>>,
    peer: Mutex<Option<DuplexStream>>,
    write_bytes_per_second: Mutex<Option<NonZeroUsize>>,
    stream_opens: AtomicUsize,
}

impl FakeMachine {
    #[must_use]
    pub fn new(id: HostId) -> Self {
        Self {
            id,
            resolves: Mutex::new(VecDeque::new()),
            probes: Mutex::new(VecDeque::new()),
            execs: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            peer: Mutex::new(None),
            write_bytes_per_second: Mutex::new(None),
            stream_opens: AtomicUsize::new(0),
        }
    }

    pub fn push_resolve(&self, result: Result<MachineAddress, MachineError>) {
        lock(&self.resolves).push_back(result);
    }
    pub fn push_probe(&self, report: ProbeReport) {
        lock(&self.probes).push_back(report);
    }
    pub fn push_exec(&self, result: Result<ExecOutput, MachineError>) {
        lock(&self.execs).push_back(result);
    }
    #[must_use]
    pub fn exec_calls(&self) -> Vec<Vec<String>> {
        lock(&self.calls).clone()
    }
    pub fn take_stream_peer(&self) -> Option<DuplexStream> {
        lock(&self.peer).take()
    }
    /// Throttles writes from the link side of subsequently opened streams.
    pub fn throttle_stream_writes(&self, bytes_per_second: NonZeroUsize) {
        *lock(&self.write_bytes_per_second) = Some(bytes_per_second);
    }
    /// Number of [`MachineProvider::open_stream`] calls observed so far.
    #[must_use]
    pub fn stream_opens(&self) -> usize {
        self.stream_opens.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl MachineProvider for FakeMachine {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn provider_name(&self) -> &'static str {
        "command"
    }
    async fn resolve(&self) -> Result<MachineAddress, MachineError> {
        lock(&self.resolves).pop_front().unwrap_or_else(|| {
            Ok(MachineAddress {
                host: self.id.to_string(),
                user: None,
                display: self.id.to_string(),
                online: Some(true),
            })
        })
    }
    async fn probe(&self, _timeout: Duration) -> ProbeReport {
        lock(&self.probes).pop_front().unwrap_or(ProbeReport {
            reachable: true,
            latency_ms: Some(0),
            version: Some("fleetd test".to_owned()),
            error: None,
            stderr: None,
        })
    }
    async fn exec(&self, argv: &[String], _timeout: Duration) -> Result<ExecOutput, MachineError> {
        lock(&self.calls).push(argv.to_vec());
        lock(&self.execs).pop_front().unwrap_or_else(|| {
            Ok(ExecOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        })
    }
    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        self.stream_opens.fetch_add(1, Ordering::Relaxed);
        let (local, peer) = tokio::io::duplex(64 * 1024);
        *lock(&self.peer) = Some(peer);
        match *lock(&self.write_bytes_per_second) {
            Some(bytes_per_second) => Ok(Box::new(ThrottledWrites::new(local, bytes_per_second))),
            None => Ok(Box::new(local)),
        }
    }
    fn fleetd_binary(&self) -> &str {
        "fleetd"
    }
    fn fleet_home(&self) -> Option<&str> {
        Some("~/.fleet")
    }
}

const THROTTLED_WRITE_QUANTUM: usize = 16 * 1024;

struct ThrottledWrites {
    inner: DuplexStream,
    bytes_per_second: NonZeroUsize,
    next_write: Pin<Box<Sleep>>,
}

impl ThrottledWrites {
    fn new(inner: DuplexStream, bytes_per_second: NonZeroUsize) -> Self {
        Self {
            inner,
            bytes_per_second,
            next_write: Box::pin(tokio::time::sleep(Duration::ZERO)),
        }
    }

    fn delay_for(&self, bytes: usize) -> Duration {
        let nanos = (bytes as u128 * 1_000_000_000_u128)
            .div_ceil(self.bytes_per_second.get() as u128)
            .min(u64::MAX as u128) as u64;
        Duration::from_nanos(nanos)
    }
}

impl AsyncRead for ThrottledWrites {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

impl AsyncWrite for ThrottledWrites {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        if self.next_write.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        let quantum = buffer.len().min(THROTTLED_WRITE_QUANTUM);
        match Pin::new(&mut self.inner).poll_write(cx, &buffer[..quantum]) {
            Poll::Ready(Ok(written)) => {
                let delay = self.delay_for(written);
                self.next_write.as_mut().reset(Instant::now() + delay);
                Poll::Ready(Ok(written))
            }
            result => result,
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Scripted remote endpoint with controllable events and link state.
pub struct FakeRemote {
    host: HostId,
    responses: Mutex<VecDeque<DaemonResult<ResponseBody>>>,
    requests: Mutex<Vec<RequestBody>>,
    events_tx: broadcast::Sender<Event>,
    state_tx: watch::Sender<LinkState>,
    hello: Mutex<Option<RemoteHello>>,
    last_snapshot: Mutex<Option<Snapshot>>,
    nudges: AtomicUsize,
}

impl FakeRemote {
    #[must_use]
    pub fn new(host: HostId) -> Self {
        let (events_tx, _) = broadcast::channel(256);
        let (state_tx, _) = watch::channel(LinkState::Ready);
        Self {
            host,
            responses: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            events_tx,
            state_tx,
            hello: Mutex::new(None),
            last_snapshot: Mutex::new(None),
            nudges: AtomicUsize::new(0),
        }
    }

    pub fn push_response(&self, response: DaemonResult<ResponseBody>) {
        lock(&self.responses).push_back(response);
    }
    #[must_use]
    pub fn requests(&self) -> Vec<RequestBody> {
        lock(&self.requests).clone()
    }
    pub fn emit(&self, event: Event) {
        let _ = self.events_tx.send(event);
    }
    /// Records the new link state even when no observer is subscribed to the watch channel.
    pub fn set_state(&self, state: LinkState) {
        self.state_tx.send_replace(state);
    }
    /// Number of [`RemoteEndpoint::nudge_reconnect`] calls observed so far.
    #[must_use]
    pub fn nudges(&self) -> usize {
        self.nudges.load(Ordering::Relaxed)
    }
    pub fn set_hello(&self, hello: RemoteHello) {
        *lock(&self.hello) = Some(hello);
    }
    pub fn set_last_snapshot(&self, snapshot: Snapshot) {
        *lock(&self.last_snapshot) = Some(snapshot);
    }
}

#[async_trait]
impl RemoteEndpoint for FakeRemote {
    fn host(&self) -> &HostId {
        &self.host
    }
    fn state(&self) -> LinkState {
        *self.state_tx.borrow()
    }
    fn hello(&self) -> Option<RemoteHello> {
        lock(&self.hello).clone()
    }
    fn last_snapshot_seen(&self) -> Option<Snapshot> {
        lock(&self.last_snapshot).clone()
    }
    async fn request(&self, body: RequestBody) -> DaemonResult<ResponseBody> {
        lock(&self.requests).push(body);
        lock(&self.responses).pop_front().unwrap_or_else(|| {
            Err(DaemonError::Unsupported(
                "FakeRemote response queue is empty".to_owned(),
            ))
        })
    }
    fn nudge_reconnect(&self) {
        self.nudges.fetch_add(1, Ordering::Relaxed);
    }
    fn events(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }
    fn state_changes(&self) -> watch::Receiver<LinkState> {
        self.state_tx.subscribe()
    }
    async fn close(&self) {
        self.set_state(LinkState::Down);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
