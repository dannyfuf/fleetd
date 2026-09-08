//! Request classification and remote forwarding facade.

use std::sync::Arc;

use fleet_core::{
    agents::ThreadId,
    ids::{HostId, JobId, TerminalId, WorktreeId},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

use crate::{DaemonError, DaemonResult, machines::Machines};

use super::mirror::Mirror;

pub mod agents;
pub mod classify;
pub(crate) mod create;
pub mod ids;
pub(crate) mod lifecycle;
pub(crate) mod sessions;
pub mod translate;

pub use classify::Target;
pub use ids::{ClearedIds, RemoteIds};

/// Resolves protocol identifiers to the machine that owns them.
pub trait Resolver {
    fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId>;
    fn host_of_session(&self, id: &str) -> Option<HostId>;
    fn host_of_terminal(&self, id: TerminalId) -> Option<HostId>;
    fn host_of_job(&self, id: &JobId) -> Option<HostId>;
    fn host_of_thread(&self, id: &ThreadId) -> Option<HostId>;
}

/// Federating request router shared by every local client connection.
pub struct Router {
    pub ids: RemoteIds,
    pub mirror: Arc<Mirror>,
    pub machines: Arc<Machines>,
}

impl Router {
    #[must_use]
    pub fn new(machines: Arc<Machines>, mirror: Arc<Mirror>) -> Self {
        let _ = agents::register_thread_events;
        let _ = create::ensure_repo_then_create;
        let _ = lifecycle::merge_lifecycle_fanout;
        let _ = sessions::on_attach;
        let _ = sessions::on_detach;
        Self {
            ids: RemoteIds::default(),
            mirror,
            machines,
        }
    }

    #[must_use]
    pub fn route(&self, body: &RequestBody) -> Target {
        classify::classify(body, self)
    }

    pub async fn forward(&self, _host: &HostId, _body: RequestBody) -> DaemonResult<ResponseBody> {
        Err(DaemonError::Unsupported(
            "Router::forward: not implemented".to_owned(),
        ))
    }

    pub async fn fanout(
        &self,
        parts: Vec<(HostId, RequestBody)>,
    ) -> Vec<(HostId, DaemonResult<ResponseBody>)> {
        parts
            .into_iter()
            .map(|(host, _)| {
                (
                    host,
                    Err(DaemonError::Unsupported(
                        "Router::fanout: not implemented".to_owned(),
                    )),
                )
            })
            .collect()
    }
}

impl Resolver for Router {
    fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
        self.ids
            .host_of_worktree(id)
            .or_else(|| self.mirror.host_of_worktree(id))
    }
    fn host_of_session(&self, id: &str) -> Option<HostId> {
        self.ids.host_of_session(id)
    }
    fn host_of_terminal(&self, id: TerminalId) -> Option<HostId> {
        self.ids.host_of_terminal(id)
    }
    fn host_of_job(&self, id: &JobId) -> Option<HostId> {
        self.ids.host_of_job(id)
    }
    fn host_of_thread(&self, id: &ThreadId) -> Option<HostId> {
        self.ids.host_of_thread(id)
    }
}
