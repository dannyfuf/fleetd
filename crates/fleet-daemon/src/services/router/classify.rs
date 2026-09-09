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

/// Returns the local partition omitted from [`Target::Fanout`]'s remote-host parts.
#[must_use]
pub(crate) fn local_fanout_part(
    body: &RequestBody,
    resolver: &dyn Resolver,
) -> Option<RequestBody> {
    match body {
        RequestBody::DeleteWorktrees { ids } => {
            nonempty_worktree_part(ids, resolver).map(|ids| RequestBody::DeleteWorktrees { ids })
        }
        RequestBody::InspectWorktrees { ids, repo, fetch } => nonempty_worktree_part(ids, resolver)
            .map(|ids| RequestBody::InspectWorktrees {
                ids,
                repo: repo.clone(),
                fetch: *fetch,
            }),
        RequestBody::PruneWorktrees {
            dry_run,
            fetch,
            kill_sessions,
            repo,
            ids: Some(ids),
        } => nonempty_worktree_part(ids, resolver).map(|ids| RequestBody::PruneWorktrees {
            dry_run: *dry_run,
            fetch: *fetch,
            kill_sessions: *kill_sessions,
            repo: repo.clone(),
            ids: Some(ids),
        }),
        RequestBody::DismissJobs { jobs } => {
            let jobs = jobs
                .iter()
                .filter(|job| resolver.host_of_job(job).is_none())
                .cloned()
                .collect::<Vec<_>>();
            (!jobs.is_empty()).then_some(RequestBody::DismissJobs { jobs })
        }
        RequestBody::PruneWorktrees { ids: None, .. }
        | RequestBody::AgentThreadList
        | RequestBody::AgentThreadCreate { .. }
        | RequestBody::AgentThreadOpen { .. }
        | RequestBody::AgentThreadClose { .. }
        | RequestBody::AgentSend { .. }
        | RequestBody::AgentInterrupt { .. }
        | RequestBody::AgentRespond { .. }
        | RequestBody::AgentSetMode { .. }
        | RequestBody::AgentSetModel { .. }
        | RequestBody::AgentMarkSeen { .. }
        | RequestBody::AgentStop { .. }
        | RequestBody::ListBoards { .. }
        | RequestBody::GetBoard { .. }
        | RequestBody::EnsureBoard { .. }
        | RequestBody::CreateBoard { .. }
        | RequestBody::UpdateBoard { .. }
        | RequestBody::DeleteBoard { .. }
        | RequestBody::CreateCard { .. }
        | RequestBody::UpdateCard { .. }
        | RequestBody::MoveCard { .. }
        | RequestBody::DeleteCard { .. }
        | RequestBody::AddCardComment { .. }
        | RequestBody::CreateWorktreeFromCard { .. }
        | RequestBody::SyncBoard { .. }
        | RequestBody::ResolveCardConflict { .. }
        | RequestBody::DescribeBoardBackend { .. }
        | RequestBody::ListBoardBackends {}
        | RequestBody::StartWatch { .. }
        | RequestBody::AppendWatchOutput { .. }
        | RequestBody::FinishWatch { .. }
        | RequestBody::ListWatches { .. }
        | RequestBody::TailWatch { .. }
        | RequestBody::DismissWatch { .. }
        | RequestBody::Hello { .. }
        | RequestBody::GetSnapshot
        | RequestBody::Subscribe { .. }
        | RequestBody::Unsubscribe
        | RequestBody::CreateContext { .. }
        | RequestBody::UpdateContext { .. }
        | RequestBody::DeleteContext { .. }
        | RequestBody::SetActiveContext { .. }
        | RequestBody::CloneRepo { .. }
        | RequestBody::DeleteRepo { .. }
        | RequestBody::MoveRepoToContext { .. }
        | RequestBody::SearchRemoteRepos { .. }
        | RequestBody::ListRemoteRepos { .. }
        | RequestBody::ListBaseRefs { .. }
        | RequestBody::SetRepoHooks { .. }
        | RequestBody::DismissClone { .. }
        | RequestBody::CreateWorktree { .. }
        | RequestBody::KillWorktree { .. }
        | RequestBody::SleepWorktree { .. }
        | RequestBody::TouchWorktreeOpened { .. }
        | RequestBody::WorktreePath { .. }
        | RequestBody::RestoreTrash { .. }
        | RequestBody::RefreshStatuses { .. }
        | RequestBody::SetAgentActivity { .. }
        | RequestBody::ListPullRequests { .. }
        | RequestBody::CreateWorktreeFromPr { .. }
        | RequestBody::BootstrapHost { .. }
        | RequestBody::EnsureSession { .. }
        | RequestBody::ListSessions
        | RequestBody::CurrentSession
        | RequestBody::KillSession { .. }
        | RequestBody::SleepSession { .. }
        | RequestBody::NewTerminal { .. }
        | RequestBody::CloseTerminal { .. }
        | RequestBody::RestartTerminal { .. }
        | RequestBody::RenameTerminal { .. }
        | RequestBody::SelectTerminal { .. }
        | RequestBody::AttachTerminal { .. }
        | RequestBody::DetachTerminal { .. }
        | RequestBody::TerminalInput { .. }
        | RequestBody::TerminalKey { .. }
        | RequestBody::TerminalMouse { .. }
        | RequestBody::ResizeTerminal { .. }
        | RequestBody::ScrollTerminal { .. }
        | RequestBody::WheelTerminal { .. }
        | RequestBody::ScrollOrKeyTerminal { .. }
        | RequestBody::RequestFullFrame { .. }
        | RequestBody::PasteTerminal { .. }
        | RequestBody::ListJobs
        | RequestBody::CancelJob { .. }
        | RequestBody::RetryJob { .. }
        | RequestBody::TailJob { .. }
        | RequestBody::GetConfig
        | RequestBody::SetConfig { .. }
        | RequestBody::MatchKeepAliveRules
        | RequestBody::ImportFromSwarm
        | RequestBody::Doctor
        | RequestBody::ResetState
        | RequestBody::Update
        | RequestBody::DaemonPing
        | RequestBody::DaemonVersion
        | RequestBody::DaemonShutdown { .. } => None,
    }
}

fn nonempty_worktree_part(
    ids: &[fleet_core::ids::WorktreeId],
    resolver: &dyn Resolver,
) -> Option<Vec<fleet_core::ids::WorktreeId>> {
    let ids = ids
        .iter()
        .filter(|id| resolver.host_of_worktree(id).is_none())
        .cloned()
        .collect::<Vec<_>>();
    (!ids.is_empty()).then_some(ids)
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
