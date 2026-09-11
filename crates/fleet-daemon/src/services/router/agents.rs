//! Native-agent routing hooks, and the seam the durable mirror hangs on.
//!
//! Classification is unchanged and stays unchanged: creation routes by worktree, every other verb
//! routes by thread, and the listing fans out and merges. What this module adds is the *mirror*
//! seam — the local daemon is a re-framing proxy that also keeps a durable read-through cache of
//! the threads other hosts own (`docs/NATIVE-AGENTS.md` §9.3), and the router is where that cache
//! meets the link.
//!
//! The cache itself lives in the agent store; [`AgentMirror`] is the two-way door between them, so
//! the router never learns any SQL and the store never learns what a link is.
//!
//! Three moments, and nothing else:
//!
//! 1. **An open.** [`open_from_mirror`] answers a warm mirrored thread from the local database and
//!    asks the owner, in the background, for everything after what it holds. Zero transcript bytes
//!    cross the link and the app paints in one frame. A cold mirror is proxied instead, and the
//!    answer is absorbed on the way back so the *next* open is warm.
//! 2. **An event.** [`ingest_remote_event`] appends what arrives on a host's link to that host's
//!    mirrored thread, before the event is republished, so a client that reacts to it by opening
//!    the thread cannot read a transcript older than the event that woke it.
//! 3. **A disconnection.** [`cached_threads`] keeps the list painted from the durable cache.
//!    Disconnection changes the status, never the data.
//!
//! Every mutation is absent from that list on purpose: a mutation for a mirrored thread is
//! forwarded and never applied locally, because the owner's reducer is the only thing that decides
//! whether it was accepted.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use fleet_core::{
    agents::{AgentThreadSummary, SeqEvent, ThreadId},
    ids::HostId,
};
use fleet_proto::event::Event;
use fleet_proto::request::RequestBody;
use fleet_proto::response::ResponseBody;

use crate::{DaemonResult, machines::RemoteEndpoint};

use super::{RemoteIds, Resolver, Target};

/// What a mirror did with one event that arrived on a host's link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorWrite {
    /// The event extended the mirrored prefix.
    Stored,
    /// Nothing to do: a duplicate, a thread this daemon does not mirror, or a refusal.
    Ignored,
    /// The mirror has fallen behind and needs the owner's window before it can take more.
    NeedsWindow,
}

/// The durable read-through mirror, as the router uses it.
///
/// Implemented by the agent service, which owns the database; named here because the router owns
/// the link and therefore owns *when* each of these is called. Every method is infallible on
/// purpose: a cache that cannot answer must degrade to asking the owner, never fail a request the
/// owner could have served.
#[async_trait]
pub trait AgentMirror: Send + Sync {
    /// Records — or refreshes — the headers of the threads `host` owns.
    async fn adopt(&self, host: &HostId, summaries: &[AgentThreadSummary]);

    /// Appends one event received on `host`'s link to that host's mirrored thread.
    async fn ingest(&self, host: &HostId, thread: ThreadId, event: &SeqEvent) -> MirrorWrite;

    /// Answers one `AgentThreadOpen` from the mirror, or `None` when the owner must be asked.
    async fn open(&self, host: &HostId, body: &RequestBody) -> Option<DaemonResult<ResponseBody>>;

    /// The request that brings this daemon's mirror up to `host`'s head, if one is needed.
    async fn delta(&self, host: &HostId, body: &RequestBody) -> Option<RequestBody>;

    /// Absorbs an owner's answer, returning whether the mirror grew.
    ///
    /// `announce` publishes the event that tells clients to re-read the thread. It is true for a
    /// background refill, whose growth nothing else would report, and false for an answer the
    /// client is already being handed — telling a client to re-read what it is reading is how a
    /// status label flickers.
    async fn absorb(
        &self,
        host: &HostId,
        body: &RequestBody,
        response: &ResponseBody,
        announce: bool,
    ) -> bool;

    /// The cached headers of one host's threads, for a list its link cannot answer.
    async fn cached_threads(&self, host: &HostId) -> Vec<AgentThreadSummary>;
}

/// Whether one endpoint's peer can answer the delta the mirror asks for.
///
/// Version skew is negotiated, never inferred, and it gates exactly one thing: the *request*
/// this daemon sends upstream. A peer that ignores `after_seq` answers with a whole projection —
/// no events, nothing to cache, and a frame the size of the transcript — so against an older
/// daemon the refill is simply never asked for and the local side stays the plain proxy it has
/// always been. Reading the mirror needs no capability from anyone, which is what keeps a
/// transcript readable while the link is down and `hello` is gone.
#[must_use]
pub fn mirrors_against(endpoint: &Arc<dyn RemoteEndpoint>) -> bool {
    endpoint.hello().is_some_and(|hello| {
        hello
            .capabilities
            .iter()
            .any(|capability| capability == fleet_proto::AGENT_WINDOW_CAPABILITY)
    })
}

/// Answers one agent open from the mirror, and refills it from the owner in the background.
///
/// `None` hands the request back to the ordinary forwarding path, and that is the answer for a
/// local thread, a cold mirror, a version-6 open, and a paged read — a "load earlier" past what
/// the mirror holds is the owner's to answer. The refill it starts is the only part gated on the
/// peer's capability, because it is the only part that leaves this machine.
pub(crate) async fn open_from_mirror(
    mirror: Option<&Arc<dyn AgentMirror>>,
    endpoint: &Arc<dyn RemoteEndpoint>,
    host: &HostId,
    body: &RequestBody,
) -> Option<DaemonResult<ResponseBody>> {
    if !matches!(body, RequestBody::AgentThreadOpen { .. }) {
        return None;
    }
    let mirror = mirror?;
    let answer = mirror.open(host, body).await?;
    if mirrors_against(endpoint)
        && let Some(delta) = mirror.delta(host, body).await
    {
        refill_in_background(
            Arc::clone(mirror),
            Arc::clone(endpoint),
            host.clone(),
            delta,
        );
    }
    Some(answer)
}

/// Absorbs one forwarded answer into the mirror, so the next open is warm.
pub(crate) async fn absorb_forwarded(
    mirror: Option<&Arc<dyn AgentMirror>>,
    host: &HostId,
    body: &RequestBody,
    response: &ResponseBody,
) {
    if let Some(mirror) = mirror {
        mirror.absorb(host, body, response, false).await;
    }
}

/// The cached headers of one host's threads, for a listing its link could not answer.
pub(crate) async fn cached_threads(
    mirror: Option<&Arc<dyn AgentMirror>>,
    host: &HostId,
) -> Vec<AgentThreadSummary> {
    match mirror {
        Some(mirror) => mirror.cached_threads(host).await,
        None => Vec::new(),
    }
}

/// Mirrors what one remote event carries, before the event is republished locally.
///
/// A summary is a header and an [`Event::Agent`] is a transcript line; a snapshot is the
/// authoritative re-sync after a link recovery and therefore re-adopts every header the owner
/// still has. A thread that needs a window says so and the refill runs in the background rather
/// than in the pump, which must never block on a round trip.
pub(crate) async fn ingest_remote_event(
    mirror: Option<&Arc<dyn AgentMirror>>,
    endpoint: &Arc<dyn RemoteEndpoint>,
    host: &HostId,
    event: &Event,
) {
    let Some(mirror) = mirror else {
        return;
    };
    match event {
        Event::AgentSummary(summary) => {
            mirror.adopt(host, std::slice::from_ref(summary)).await;
        }
        Event::SnapshotChanged(snapshot) => mirror.adopt(host, &snapshot.agent_threads).await,
        Event::Agent { thread, event } => {
            if mirror.ingest(host, *thread, event).await == MirrorWrite::NeedsWindow
                && mirrors_against(endpoint)
                && let Some(delta) = mirror.delta(host, &open_request(*thread)).await
            {
                refill_in_background(
                    Arc::clone(mirror),
                    Arc::clone(endpoint),
                    host.clone(),
                    delta,
                );
            }
        }
        _ => {}
    }
}

/// The plain open this daemon uses when it needs a thread's window for its own sake.
fn open_request(thread: ThreadId) -> RequestBody {
    RequestBody::AgentThreadOpen {
        thread,
        from_seq: None,
        after_seq: None,
        turn_limit: None,
        before_cursor: None,
        request_sync_marker: false,
    }
}

/// Asks the owner for the delta and absorbs it, off the request path.
///
/// Fire and forget by design: the answer the client already has is complete for what the mirror
/// held, and the refill only makes the *next* read cheaper. A failure is a log line, because the
/// alternative — failing an open that was already answered — would turn a cache miss into an
/// error the user can see.
fn refill_in_background(
    mirror: Arc<dyn AgentMirror>,
    endpoint: Arc<dyn RemoteEndpoint>,
    host: HostId,
    delta: RequestBody,
) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        match endpoint.request(delta.clone()).await {
            Ok(response) => {
                mirror.absorb(&host, &delta, &response, true).await;
            }
            Err(error) => tracing::debug!(
                %host,
                %error,
                "could not refill a mirrored native-agent thread from its owner"
            ),
        }
    });
}

#[must_use]
pub(crate) fn classify_agent(body: &RequestBody, resolver: &dyn Resolver) -> Target {
    use RequestBody::*;
    match body {
        AgentThreadCreate { worktree, .. } => resolver
            .host_of_worktree(worktree)
            .map_or(Target::Local, Target::Host),
        AgentThreadOpen { thread, .. }
        | AgentThreadClose { thread }
        | AgentSend { thread, .. }
        | AgentInterrupt { thread }
        | AgentRespond { thread, .. }
        | AgentSetMode { thread, .. }
        | AgentSetModel { thread, .. }
        | AgentMarkSeen { thread, .. }
        | AgentCheckpoints { thread }
        | AgentRevert { thread, .. }
        | AgentStop { thread } => resolver
            .host_of_thread(thread)
            .map_or(Target::Local, Target::Host),
        AgentThreadList => agent_list_target(std::iter::empty()),
        _ => Target::Unsupported("not an agent request"),
    }
}

/// Builds the remote partitions for the federated agent-thread listing request.
///
/// The local partition is added by dispatch in the same way as other fanout requests. Keeping
/// endpoint enumeration outside [`classify_agent`] preserves the resolver-only classification
/// seam while allowing [`super::Router`] to supply the endpoints it owns.
#[must_use]
pub fn agent_list_target(hosts: impl IntoIterator<Item = HostId>) -> Target {
    Target::Fanout(
        hosts
            .into_iter()
            .map(|host| (host, RequestBody::AgentThreadList))
            .collect(),
    )
}

/// Authoritative per-host thread ownership sets observed by the remote event pumps.
///
/// A daemon restart clears the router's transient id mappings. The next snapshot re-registers
/// every surviving UUID unchanged. Replacing the set also makes a snapshot omission the deletion
/// signal: threads no longer present are forgotten without persisting a local transcript.
#[derive(Default)]
pub struct ThreadRegistrations {
    by_host: Mutex<BTreeMap<HostId, BTreeSet<ThreadId>>>,
}

impl ThreadRegistrations {
    fn register(&self, host: &HostId, thread: ThreadId, ids: &RemoteIds) {
        lock(&self.by_host)
            .entry(host.clone())
            .or_default()
            .insert(thread);
        ids.register_thread(host, thread);
    }

    fn replace(&self, host: &HostId, summaries: &[AgentThreadSummary], ids: &RemoteIds) {
        let current = summaries
            .iter()
            .map(|summary| summary.thread)
            .collect::<BTreeSet<_>>();
        let previous = lock(&self.by_host)
            .insert(host.clone(), current.clone())
            .unwrap_or_default();

        for deleted in previous.difference(&current).copied() {
            if ids.host_of_thread(&deleted).as_ref() == Some(host) {
                ids.forget_thread(deleted);
            }
        }
        for thread in current {
            ids.register_thread(host, thread);
        }
    }
}

/// Registers thread ownership carried by one remote event.
///
/// `Agent` and `AgentSummary` cover newly-created live threads. `SnapshotChanged` is the
/// authoritative mirror re-sync used after link recovery and is also where deletions are
/// detected.
pub fn register_thread_events(
    registrations: &ThreadRegistrations,
    event: &Event,
    host: &HostId,
    ids: &RemoteIds,
) {
    match event {
        Event::Agent { thread, .. } => registrations.register(host, *thread, ids),
        Event::AgentSummary(summary) => registrations.register(host, summary.thread, ids),
        Event::SnapshotChanged(snapshot) => {
            registrations.replace(host, &snapshot.agent_threads, ids);
        }
        // Ownership is registered by `Agent`/`AgentSummary`/`SnapshotChanged`; the stream's
        // control events name a thread that is already registered by the time they arrive.
        Event::AgentResync { .. }
        | Event::AgentSynchronized { .. }
        | Event::AgentWindow { .. }
        | Event::BoardChanged { .. }
        | Event::WatchStarted(_)
        | Event::WatchOutput { .. }
        | Event::WatchExited(_)
        | Event::WatchDismissed(_)
        | Event::JobUpdated(_)
        | Event::SessionChanged(_)
        | Event::AgentActivityChanged { .. }
        | Event::TerminalFrame(_)
        | Event::TerminalExited { .. }
        | Event::TerminalTitle { .. }
        | Event::HostLinkChanged { .. }
        | Event::TerminalReattach { .. }
        | Event::Toast { .. }
        | Event::DaemonShuttingDown
        | Event::Unknown => {}
    }
}

/// Seeds ownership from a mirror fragment that predates event-pump startup.
pub fn register_mirror_threads(
    registrations: &ThreadRegistrations,
    host: &HostId,
    summaries: &[AgentThreadSummary],
    ids: &RemoteIds,
) {
    registrations.replace(host, summaries, ids);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
