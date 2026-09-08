//! Exhaustive request target classification.

use std::collections::BTreeMap;

use fleet_core::ids::HostId;
use fleet_proto::request::RequestBody;

use super::{Resolver, agents::classify_agent};

/// Execution target selected for one client request.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Local,
    Host(HostId),
    Fanout(Vec<(HostId, RequestBody)>),
    Unsupported(&'static str),
}

/// Classifies every protocol request without a wildcard arm.
#[must_use]
pub fn classify(body: &RequestBody, resolver: &dyn Resolver) -> Target {
    use RequestBody::*;
    match body {
        AgentThreadList
        | AgentThreadCreate { .. }
        | AgentThreadOpen { .. }
        | AgentThreadClose { .. }
        | AgentSend { .. }
        | AgentInterrupt { .. }
        | AgentRespond { .. }
        | AgentSetMode { .. }
        | AgentSetModel { .. }
        | AgentMarkSeen { .. }
        | AgentStop { .. } => classify_agent(body, resolver),

        CreateWorktree {
            host: Some(host), ..
        }
        | CreateWorktreeFromPr {
            host: Some(host), ..
        }
        | CreateWorktreeFromCard {
            host: Some(host), ..
        } => Target::Host(host.clone()),
        CreateWorktree { host: None, .. }
        | CreateWorktreeFromPr { host: None, .. }
        | CreateWorktreeFromCard { host: None, .. } => Target::Local,

        DeleteWorktrees { ids } | InspectWorktrees { ids, .. } => {
            fanout_worktrees(body, ids, resolver)
        }
        PruneWorktrees { ids: Some(ids), .. } => fanout_worktrees(body, ids, resolver),
        PruneWorktrees { ids: None, .. } => Target::Local,

        KillWorktree { id }
        | SleepWorktree { id }
        | TouchWorktreeOpened { id }
        | WorktreePath { id } => host_or_local(resolver.host_of_worktree(id)),

        EnsureSession {
            worktree: Some(id), ..
        } => host_or_local(resolver.host_of_worktree(id)),
        EnsureSession { worktree: None, .. } | ListSessions | CurrentSession => Target::Local,
        KillSession { session }
        | SleepSession { session }
        | NewTerminal { session, .. }
        | ListWatches { session } => host_or_local(resolver.host_of_session(session.as_ref())),
        SetAgentActivity { session, .. } => {
            host_or_local(resolver.host_of_session(session.as_ref()))
        }

        StartWatch { terminal, .. }
        | CloseTerminal { terminal }
        | RestartTerminal { terminal }
        | RenameTerminal { terminal, .. }
        | SelectTerminal { terminal, .. }
        | AttachTerminal { terminal, .. }
        | DetachTerminal { terminal }
        | TerminalInput { terminal, .. }
        | TerminalKey { terminal, .. }
        | TerminalMouse { terminal, .. }
        | ResizeTerminal { terminal, .. }
        | ScrollTerminal { terminal, .. }
        | WheelTerminal { terminal, .. }
        | ScrollOrKeyTerminal { terminal, .. }
        | RequestFullFrame { terminal }
        | PasteTerminal { terminal, .. } => host_or_local(resolver.host_of_terminal(*terminal)),

        CancelJob { job } | RetryJob { job } | TailJob { job, .. } => {
            host_or_local(resolver.host_of_job(job))
        }
        DismissJobs { jobs } => fanout_jobs(body, jobs, resolver),

        ListBoards { .. }
        | GetBoard { .. }
        | EnsureBoard { .. }
        | CreateBoard { .. }
        | UpdateBoard { .. }
        | DeleteBoard { .. }
        | CreateCard { .. }
        | UpdateCard { .. }
        | MoveCard { .. }
        | DeleteCard { .. }
        | AddCardComment { .. }
        | SyncBoard { .. }
        | ResolveCardConflict { .. }
        | DescribeBoardBackend { .. }
        | ListBoardBackends {}
        | AppendWatchOutput { .. }
        | FinishWatch { .. }
        | TailWatch { .. }
        | DismissWatch { .. }
        | Hello { .. }
        | GetSnapshot
        | Subscribe { .. }
        | Unsubscribe
        | CreateContext { .. }
        | UpdateContext { .. }
        | DeleteContext { .. }
        | SetActiveContext { .. }
        | CloneRepo { .. }
        | DeleteRepo { .. }
        | MoveRepoToContext { .. }
        | SearchRemoteRepos { .. }
        | ListRemoteRepos { .. }
        | ListBaseRefs { .. }
        | SetRepoHooks { .. }
        | DismissClone { .. }
        | RestoreTrash { .. }
        | RefreshStatuses { .. }
        | ListPullRequests { .. }
        | BootstrapHost { .. }
        | ListJobs
        | GetConfig
        | SetConfig { .. }
        | MatchKeepAliveRules
        | ImportFromSwarm
        | Doctor
        | ResetState
        | Update
        | DaemonPing
        | DaemonVersion
        | DaemonShutdown { .. } => Target::Local,
    }
}

fn host_or_local(host: Option<HostId>) -> Target {
    host.map_or(Target::Local, Target::Host)
}

fn fanout_worktrees(
    body: &RequestBody,
    ids: &[fleet_core::ids::WorktreeId],
    resolver: &dyn Resolver,
) -> Target {
    let mut parts = BTreeMap::<HostId, Vec<fleet_core::ids::WorktreeId>>::new();
    for id in ids {
        if let Some(host) = resolver.host_of_worktree(id) {
            parts.entry(host).or_default().push(id.clone());
        }
    }
    if parts.is_empty() {
        return Target::Local;
    }
    Target::Fanout(
        parts
            .into_iter()
            .map(|(host, ids)| {
                let part = match body {
                    RequestBody::DeleteWorktrees { .. } => RequestBody::DeleteWorktrees { ids },
                    RequestBody::InspectWorktrees { repo, fetch, .. } => {
                        RequestBody::InspectWorktrees {
                            ids,
                            repo: repo.clone(),
                            fetch: *fetch,
                        }
                    }
                    RequestBody::PruneWorktrees {
                        dry_run,
                        fetch,
                        kill_sessions,
                        repo,
                        ..
                    } => RequestBody::PruneWorktrees {
                        dry_run: *dry_run,
                        fetch: *fetch,
                        kill_sessions: *kill_sessions,
                        repo: repo.clone(),
                        ids: Some(ids),
                    },
                    _ => body.clone(),
                };
                (host, part)
            })
            .collect(),
    )
}

fn fanout_jobs(
    body: &RequestBody,
    jobs: &[fleet_core::ids::JobId],
    resolver: &dyn Resolver,
) -> Target {
    let mut parts = BTreeMap::<HostId, Vec<fleet_core::ids::JobId>>::new();
    for job in jobs {
        if let Some(host) = resolver.host_of_job(job) {
            parts.entry(host).or_default().push(job.clone());
        }
    }
    if parts.is_empty() {
        Target::Local
    } else {
        Target::Fanout(
            parts
                .into_iter()
                .map(|(host, jobs)| {
                    (
                        host,
                        match body {
                            RequestBody::DismissJobs { .. } => RequestBody::DismissJobs { jobs },
                            _ => body.clone(),
                        },
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        agents::ThreadId,
        ids::{JobId, TerminalId, WorktreeId},
    };

    use super::*;

    struct TestResolver {
        remote: WorktreeId,
        host: HostId,
    }

    impl Resolver for TestResolver {
        fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
            (id == &self.remote).then(|| self.host.clone())
        }
        fn host_of_session(&self, id: &str) -> Option<HostId> {
            id.starts_with("dev-box/").then(|| self.host.clone())
        }
        fn host_of_terminal(&self, id: TerminalId) -> Option<HostId> {
            (id == TerminalId(9)).then(|| self.host.clone())
        }
        fn host_of_job(&self, _id: &JobId) -> Option<HostId> {
            None
        }
        fn host_of_thread(&self, _id: &ThreadId) -> Option<HostId> {
            None
        }
    }

    #[test]
    fn representative_requests_are_local_host_or_fanout() {
        let remote = WorktreeId::try_from("acme/api#remote").expect("worktree");
        let local = WorktreeId::try_from("acme/api#local").expect("worktree");
        let host = HostId::try_from("dev-box").expect("host");
        let resolver = TestResolver {
            remote: remote.clone(),
            host: host.clone(),
        };
        assert_eq!(classify(&RequestBody::GetConfig, &resolver), Target::Local);
        assert_eq!(
            classify(&RequestBody::WorktreePath { id: remote.clone() }, &resolver),
            Target::Host(host.clone())
        );
        assert!(
            matches!(classify(&RequestBody::DeleteWorktrees { ids: vec![local, remote] }, &resolver), Target::Fanout(parts) if parts.len() == 1 && parts[0].0 == host)
        );
        assert_eq!(
            classify(
                &RequestBody::RequestFullFrame {
                    terminal: TerminalId(9)
                },
                &resolver
            ),
            Target::Host(host)
        );
    }
}
