//! Persistent remote-daemon endpoint contract and reconnecting link skeleton.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use fleet_proto::{
    event::Event, request::RequestBody, response::ResponseBody, snapshot::LinkState,
};
use tokio::sync::{broadcast, watch};

use crate::{DaemonError, DaemonResult};

use super::MachineProvider;

/// Remote Hello metadata retained by an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHello {
    pub version: String,
    pub daemon_id: String,
    pub build_commit: Option<String>,
    pub capabilities: Vec<String>,
}

/// Backoff and handshake bounds for a remote link.
#[derive(Debug, Clone, Copy)]
pub struct LinkOptions {
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    pub hello_timeout: Duration,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            hello_timeout: Duration::from_secs(10),
        }
    }
}

/// Request and event surface exposed by a remote daemon connection.
#[async_trait]
pub trait RemoteEndpoint: Send + Sync {
    fn host(&self) -> &HostId;
    fn state(&self) -> LinkState;
    fn hello(&self) -> Option<RemoteHello>;
    async fn request(&self, body: RequestBody) -> DaemonResult<ResponseBody>;
    fn events(&self) -> broadcast::Receiver<Event>;
    fn state_changes(&self) -> watch::Receiver<LinkState>;
    async fn close(&self);
}

/// Reconnecting framed protocol link. The link stage replaces the skeleton operations.
pub struct RemoteLink {
    provider: Arc<dyn MachineProvider>,
    options: LinkOptions,
    events_tx: broadcast::Sender<Event>,
    state_tx: watch::Sender<LinkState>,
}

impl RemoteLink {
    #[must_use]
    pub fn new(provider: Arc<dyn MachineProvider>, options: LinkOptions) -> Arc<Self> {
        let (events_tx, _) = broadcast::channel(256);
        let (state_tx, _) = watch::channel(LinkState::Down);
        Arc::new(Self {
            provider,
            options,
            events_tx,
            state_tx,
        })
    }

    pub async fn connect(&self) -> DaemonResult<()> {
        let _ = self.options;
        Err(DaemonError::Unsupported(
            "RemoteLink::connect: not implemented".to_owned(),
        ))
    }
}

#[async_trait]
impl RemoteEndpoint for RemoteLink {
    fn host(&self) -> &HostId {
        self.provider.id()
    }
    fn state(&self) -> LinkState {
        *self.state_tx.borrow()
    }
    fn hello(&self) -> Option<RemoteHello> {
        None
    }
    async fn request(&self, _body: RequestBody) -> DaemonResult<ResponseBody> {
        Err(DaemonError::Unsupported(
            "RemoteLink::request: not implemented".to_owned(),
        ))
    }
    fn events(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }
    fn state_changes(&self) -> watch::Receiver<LinkState> {
        self.state_tx.subscribe()
    }
    async fn close(&self) {}
}
