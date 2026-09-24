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
        | AgentThreadReopen { .. }
        | AgentSend { .. }
        | AgentInterrupt { .. }
        | AgentRespond { .. }
        | AgentSetMode { .. }
        | AgentSetModel { .. }
        | AgentMarkSeen { .. }
        | AgentItemBody { .. }
        | AgentCheckpoints { .. }
        | AgentRevert { .. }
        | AgentAccountLogin { .. }
        | AgentAccountLogout { .. }
        | AgentStop { .. } => classify_agent(body, resolver),

        // Delegation is served by the daemon that owns the caller's thread. A mirrored caller is
        // refused by the service itself with the specific locality reason.
        DelegationRun { .. }
        | DelegationComplete { .. }
        | DelegationList { .. }
        | DelegationGet { .. }
        | DelegationCancel { .. }
        | DelegationWait { .. } => Target::Local,

        AgentSeenCursors | AgentClosedThreads => Target::Local,

        CreateWorktree {
            host: Some(host), ..
        }
        | CreateWorktreeFromPr {
            host: Some(host), ..
        } => Target::Host(host.clone()),
        CreateWorktree { host: None, .. } | CreateWorktreeFromPr { host: None, .. } => {
            Target::Local
        }

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

        // A worktree board lives on the daemon that owns the worktree, so the scope request is
        // routed by the worktree and every later request by the board or card it names
        // (`docs/decisions/0021-hosted-worktree-boards-route-to-owner.md`).
        EnsureWorktreeBoard { worktree_id } | CreateWorktreeBoard { worktree_id, .. } => {
            host_or_local(resolver.host_of_worktree(worktree_id))
        }
        GetBoard { board_id }
        | UpdateBoard { board_id, .. }
        | DeleteBoard { board_id }
        | CreateCard { board_id, .. }
        | SyncBoard { board_id, .. }
        | DescribeBoardBackend { board_id } => host_or_local(resolver.host_of_board(board_id)),
        UpdateCard { card_id, .. }
        | MoveCard { card_id, .. }
        // A card's run is served where the card lives, exactly as a move is: the board document
        // and the delegation that backs the run are both the owner's (ADR 0021).
        | CardRunStart { card_id }
        | CardRunCancel { card_id }
        | CardRunWait { card_id, .. }
        | DeleteCard { card_id }
        | AddCardComment { card_id, .. }
        | ResolveCardConflict { card_id, .. }
        | CreateWorktreeFromCard { card_id, .. } => host_or_local(resolver.host_of_card(card_id)),

        // `ListBoards` is federated by [`super::Router::route`], which needs the endpoint list
        // this resolver-only seam does not have; reaching classify means there are no hosts.
        // `EnsureBoard`, `CreateBoard` and the backend registry are context-scoped or
        // daemon-scoped, and stay here.
        ListBoards { .. }
        | EnsureBoard { .. }
        | CreateBoard { .. }
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
        | DoctorHost { .. }
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
                    RequestBody::InspectWorktrees {
                        repo,
                        fetch,
                        background,
                        ..
                    } => RequestBody::InspectWorktrees {
                        ids,
                        repo: repo.clone(),
                        fetch: *fetch,
                        background: *background,
                    },
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
        RequestBody::InspectWorktrees {
            ids,
            repo,
            fetch,
            background,
        } => {
            if ids.is_empty() {
                Some(body.clone())
            } else {
                nonempty_worktree_part(ids, resolver).map(|ids| RequestBody::InspectWorktrees {
                    ids,
                    repo: repo.clone(),
                    fetch: *fetch,
                    background: *background,
                })
            }
        }
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
        // The local list always runs: a host being enumerated never hides this daemon's own
        // boards, and the merge puts the local rows first.
        RequestBody::ListBoards { .. }
        | RequestBody::AgentItemBody { .. }
        | RequestBody::PruneWorktrees { ids: None, .. }
        | RequestBody::AgentThreadList
        | RequestBody::AgentSeenCursors
        | RequestBody::AgentClosedThreads => Some(body.clone()),
        RequestBody::AgentThreadCreate { .. }
        | RequestBody::AgentThreadOpen { .. }
        | RequestBody::AgentCheckpoints { .. }
        | RequestBody::AgentRevert { .. }
        | RequestBody::AgentThreadClose { .. }
        | RequestBody::AgentThreadReopen { .. }
        | RequestBody::AgentSend { .. }
        | RequestBody::AgentInterrupt { .. }
        | RequestBody::AgentRespond { .. }
        | RequestBody::AgentSetMode { .. }
        | RequestBody::AgentSetModel { .. }
        | RequestBody::AgentMarkSeen { .. }
        | RequestBody::AgentAccountLogin { .. }
        | RequestBody::AgentAccountLogout { .. }
        | RequestBody::AgentStop { .. }
        | RequestBody::DelegationRun { .. }
        | RequestBody::DelegationComplete { .. }
        | RequestBody::DelegationList { .. }
        | RequestBody::DelegationGet { .. }
        | RequestBody::DelegationCancel { .. }
        | RequestBody::DelegationWait { .. }
        | RequestBody::GetBoard { .. }
        | RequestBody::EnsureBoard { .. }
        | RequestBody::EnsureWorktreeBoard { .. }
        | RequestBody::CreateBoard { .. }
        | RequestBody::CreateWorktreeBoard { .. }
        | RequestBody::UpdateBoard { .. }
        | RequestBody::DeleteBoard { .. }
        | RequestBody::CreateCard { .. }
        | RequestBody::UpdateCard { .. }
        | RequestBody::MoveCard { .. }
        | RequestBody::CardRunStart { .. }
        | RequestBody::CardRunCancel { .. }
        | RequestBody::CardRunWait { .. }
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
        | RequestBody::DoctorHost { .. }
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
        agents::{AgentKind, DelegationId, ItemId, StreamKind, ThreadId, UserInput},
        board::{BoardPatch, CardDraft, CardPatch, ConflictResolution},
        ids::{BoardId, CardId, JobId, StatusId, TerminalId, WorktreeId},
    };

    use super::*;

    struct TestResolver {
        remote: WorktreeId,
        host: HostId,
        remote_board: BoardId,
        remote_card: CardId,
    }

    impl TestResolver {
        fn new(remote: WorktreeId, host: HostId) -> Self {
            Self {
                remote,
                host,
                remote_board: board("wt-acme-api-remote"),
                remote_card: card("card-remote"),
            }
        }
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
            Some(self.host.clone())
        }
        fn host_of_board(&self, id: &BoardId) -> Option<HostId> {
            (id == &self.remote_board).then(|| self.host.clone())
        }
        fn host_of_card(&self, id: &CardId) -> Option<HostId> {
            (id == &self.remote_card).then(|| self.host.clone())
        }
    }

    fn board(value: &str) -> BoardId {
        value.parse().expect("board id")
    }

    fn card(value: &str) -> CardId {
        value.parse().expect("card id")
    }

    #[test]
    fn representative_requests_are_local_host_or_fanout() {
        let remote = WorktreeId::try_from("acme/api#remote").expect("worktree");
        let local = WorktreeId::try_from("acme/api#local").expect("worktree");
        let host = HostId::try_from("dev-box").expect("host");
        let resolver = TestResolver::new(remote.clone(), host.clone());
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
            Target::Host(host.clone())
        );
        // Every other thread mutation follows its owner. The two account verbs do not: the
        // sign-in Codex starts is a loopback callback on whichever daemon runs the harness, so
        // forwarding one would hand this machine's browser a URL only the owner's loopback can
        // answer. They stay local and the manager refuses a mirrored thread by name.
        let thread = ThreadId::new();
        // Every per-thread mutation the resolver claims is forwarded to the owner. `AgentSend` is
        // the reported regression: after a restart the ids map is empty and only the mirror knows
        // the owner, so the resolver must still answer with the host (verified end to end against a
        // real `Router` in `super::super::create::tests`).
        for forwarded in [
            RequestBody::AgentSend {
                thread,
                input: UserInput::default(),
            },
            RequestBody::AgentStop { thread },
            RequestBody::AgentItemBody {
                thread,
                item: ItemId::new(),
                stream: StreamKind::AssistantText,
                offset: 0,
                limit: 4_096,
            },
        ] {
            assert_eq!(
                classify(&forwarded, &resolver),
                Target::Host(host.clone()),
                "{forwarded:?}"
            );
        }
        for local in [
            RequestBody::AgentAccountLogin { thread },
            RequestBody::AgentAccountLogout { thread },
        ] {
            assert_eq!(classify(&local, &resolver), Target::Local, "{local:?}");
        }
    }

    /// The three run verbs are card-addressed like `MoveCard`, and for the same reason: the board
    /// document they write and the delegation they drive both live on the card's owner (ADR 0021).
    #[test]
    fn a_card_run_request_classifies_exactly_like_a_move_of_the_same_card() {
        let remote = WorktreeId::try_from("acme/api#remote").expect("worktree");
        let host = HostId::try_from("dev-box").expect("host");
        let resolver = TestResolver::new(remote, host.clone());
        let hosted = resolver.remote_card.clone();
        let local = card("card-local");

        for (hosted, local) in [
            (
                RequestBody::CardRunStart {
                    card_id: hosted.clone(),
                },
                RequestBody::CardRunStart {
                    card_id: local.clone(),
                },
            ),
            (
                RequestBody::CardRunCancel {
                    card_id: hosted.clone(),
                },
                RequestBody::CardRunCancel {
                    card_id: local.clone(),
                },
            ),
            (
                RequestBody::CardRunWait {
                    card_id: hosted.clone(),
                    timeout_ms: 30_000,
                },
                RequestBody::CardRunWait {
                    card_id: local.clone(),
                    timeout_ms: 30_000,
                },
            ),
        ] {
            let moved = RequestBody::MoveCard {
                card_id: match &hosted {
                    RequestBody::CardRunStart { card_id }
                    | RequestBody::CardRunCancel { card_id }
                    | RequestBody::CardRunWait { card_id, .. } => card_id.clone(),
                    other => panic!("{other:?}"),
                },
                status_id: status(),
                index: None,
                cancel_run: false,
            };
            assert_eq!(
                classify(&hosted, &resolver),
                classify(&moved, &resolver),
                "{hosted:?}"
            );
            assert_eq!(
                classify(&hosted, &resolver),
                Target::Host(host.clone()),
                "{hosted:?}"
            );
            assert_eq!(classify(&local, &resolver), Target::Local, "{local:?}");
        }
    }

    #[test]
    fn every_board_and_card_request_follows_the_daemon_that_owns_it() {
        let remote = WorktreeId::try_from("acme/api#remote").expect("worktree");
        let local_worktree = WorktreeId::try_from("acme/api#local").expect("worktree");
        let host = HostId::try_from("dev-box").expect("host");
        let resolver = TestResolver::new(remote.clone(), host.clone());
        let hosted_board = resolver.remote_board.clone();
        let local_board = board("wt-acme-api-local");
        let hosted_card = resolver.remote_card.clone();
        let local_card = card("card-local");

        for (hosted, local) in [
            (
                RequestBody::GetBoard {
                    board_id: hosted_board.clone(),
                },
                RequestBody::GetBoard {
                    board_id: local_board.clone(),
                },
            ),
            (
                RequestBody::UpdateBoard {
                    board_id: hosted_board.clone(),
                    patch: BoardPatch::default(),
                },
                RequestBody::UpdateBoard {
                    board_id: local_board.clone(),
                    patch: BoardPatch::default(),
                },
            ),
            (
                RequestBody::DeleteBoard {
                    board_id: hosted_board.clone(),
                },
                RequestBody::DeleteBoard {
                    board_id: local_board.clone(),
                },
            ),
            (
                RequestBody::CreateCard {
                    board_id: hosted_board.clone(),
                    draft: CardDraft::default(),
                },
                RequestBody::CreateCard {
                    board_id: local_board.clone(),
                    draft: CardDraft::default(),
                },
            ),
            (
                RequestBody::SyncBoard {
                    board_id: hosted_board.clone(),
                    full: false,
                },
                RequestBody::SyncBoard {
                    board_id: local_board.clone(),
                    full: false,
                },
            ),
            (
                RequestBody::DescribeBoardBackend {
                    board_id: hosted_board,
                },
                RequestBody::DescribeBoardBackend {
                    board_id: local_board,
                },
            ),
            (
                RequestBody::UpdateCard {
                    card_id: hosted_card.clone(),
                    patch: CardPatch::default(),
                },
                RequestBody::UpdateCard {
                    card_id: local_card.clone(),
                    patch: CardPatch::default(),
                },
            ),
            (
                RequestBody::MoveCard {
                    card_id: hosted_card.clone(),
                    status_id: status(),
                    index: None,
                    cancel_run: false,
                },
                RequestBody::MoveCard {
                    card_id: local_card.clone(),
                    status_id: status(),
                    index: None,
                    cancel_run: false,
                },
            ),
            (
                RequestBody::DeleteCard {
                    card_id: hosted_card.clone(),
                },
                RequestBody::DeleteCard {
                    card_id: local_card.clone(),
                },
            ),
            (
                RequestBody::AddCardComment {
                    card_id: hosted_card.clone(),
                    body: "note".to_owned(),
                },
                RequestBody::AddCardComment {
                    card_id: local_card.clone(),
                    body: "note".to_owned(),
                },
            ),
            (
                RequestBody::CardRunStart {
                    card_id: hosted_card.clone(),
                },
                RequestBody::CardRunStart {
                    card_id: local_card.clone(),
                },
            ),
            (
                RequestBody::CardRunCancel {
                    card_id: hosted_card.clone(),
                },
                RequestBody::CardRunCancel {
                    card_id: local_card.clone(),
                },
            ),
            (
                RequestBody::CardRunWait {
                    card_id: hosted_card.clone(),
                    timeout_ms: 30_000,
                },
                RequestBody::CardRunWait {
                    card_id: local_card.clone(),
                    timeout_ms: 30_000,
                },
            ),
            (
                RequestBody::ResolveCardConflict {
                    card_id: hosted_card.clone(),
                    resolution: ConflictResolution::KeepLocal,
                },
                RequestBody::ResolveCardConflict {
                    card_id: local_card.clone(),
                    resolution: ConflictResolution::KeepLocal,
                },
            ),
            (
                RequestBody::CreateWorktreeFromCard {
                    card_id: hosted_card,
                    repo_id: None,
                    base: None,
                    host: None,
                },
                RequestBody::CreateWorktreeFromCard {
                    card_id: local_card,
                    repo_id: None,
                    base: None,
                    host: None,
                },
            ),
            (
                RequestBody::EnsureWorktreeBoard {
                    worktree_id: remote.clone(),
                },
                RequestBody::EnsureWorktreeBoard {
                    worktree_id: local_worktree.clone(),
                },
            ),
            (
                RequestBody::CreateWorktreeBoard {
                    worktree_id: remote,
                    name: None,
                    prefix: None,
                    backend: None,
                },
                RequestBody::CreateWorktreeBoard {
                    worktree_id: local_worktree,
                    name: None,
                    prefix: None,
                    backend: None,
                },
            ),
        ] {
            assert_eq!(
                classify(&hosted, &resolver),
                Target::Host(host.clone()),
                "{hosted:?}"
            );
            assert_eq!(classify(&local, &resolver), Target::Local, "{local:?}");
        }

        // A context board and the backend registry are this daemon's own whatever the resolver
        // says, and `ListBoards` is federated by the router, not here.
        for always_local in [
            RequestBody::EnsureBoard {
                context_id: "personal".parse().expect("context id"),
            },
            RequestBody::CreateBoard {
                context_id: "personal".parse().expect("context id"),
                name: None,
                prefix: None,
                backend: None,
            },
            RequestBody::ListBoardBackends {},
        ] {
            assert_eq!(
                classify(&always_local, &resolver),
                Target::Local,
                "{always_local:?}"
            );
        }
    }

    fn status() -> StatusId {
        "todo".parse().expect("status id")
    }

    #[test]
    fn every_delegation_request_is_served_locally() {
        let remote = WorktreeId::try_from("acme/api#remote").expect("worktree");
        let resolver = TestResolver::new(remote, HostId::try_from("dev-box").expect("host"));
        let caller = ThreadId::new();
        let child = ThreadId::new();
        let delegation = DelegationId::new();
        let requests = [
            RequestBody::DelegationRun {
                caller,
                provider: AgentKind::Codex,
                brief: "inspect the router".to_owned(),
                expectation: "report local routing".to_owned(),
                worktree: None,
                mode: None,
                model: None,
                title: None,
                fleet_path: None,
                env: std::collections::BTreeMap::new(),
                eager: false,
            },
            RequestBody::DelegationComplete {
                delegation,
                child,
                token: "secret".to_owned(),
                result: "done".to_owned(),
                blocked: false,
            },
            RequestBody::DelegationList {
                caller: Some(caller),
            },
            RequestBody::DelegationGet { delegation },
            RequestBody::DelegationCancel { delegation },
            RequestBody::DelegationWait {
                delegation,
                timeout_ms: 1,
                caller: None,
            },
        ];

        for request in requests {
            assert_eq!(classify(&request, &resolver), Target::Local, "{request:?}");
        }
    }
}
