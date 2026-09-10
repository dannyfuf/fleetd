//! Native-agent routing hooks.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use fleet_core::{
    agents::{AgentThreadSummary, ThreadId},
    ids::HostId,
};
use fleet_proto::event::Event;
use fleet_proto::request::RequestBody;

use super::{RemoteIds, Resolver, Target};

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
        Event::BoardChanged { .. }
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
        | Event::DaemonShuttingDown => {}
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
