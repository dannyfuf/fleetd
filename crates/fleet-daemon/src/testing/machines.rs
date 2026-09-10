//! Scriptable machine provider and remote endpoint test doubles.

use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
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
    io::DuplexStream,
    sync::{broadcast, watch},
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
        let (local, peer) = tokio::io::duplex(64 * 1024);
        *lock(&self.peer) = Some(peer);
        Ok(Box::new(local))
    }
    fn fleetd_binary(&self) -> &str {
        "fleetd"
    }
    fn fleet_home(&self) -> Option<&str> {
        Some("~/.fleet")
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
