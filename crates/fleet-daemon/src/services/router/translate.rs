//! Request, response, event, and fanout translation.

use fleet_core::{
    agents::{AgentThreadSummary, ThreadProjection},
    ids::{HostId, SessionId, TerminalId},
    model::Worktree,
    sessions::{Session, Terminal},
    watches::Watch,
};
use fleet_proto::{
    event::Event,
    job::JobRecord,
    request::RequestBody,
    response::{PruneResult, PruneSkipped, ResponseBody, WorktreeDeleteResult},
    snapshot::Snapshot,
};

use crate::{DaemonError, DaemonResult};

use super::RemoteIds;

/// Rewrites a locally addressed request into the owning daemon's id space.
pub fn to_remote(
    mut body: RequestBody,
    host: &HostId,
    ids: &RemoteIds,
) -> DaemonResult<RequestBody> {
    use RequestBody::*;
    match &mut body {
        AgentThreadCreate { worktree, .. } => ids.register_worktree(host, worktree.clone()),
        AgentThreadOpen { .. }
        | AgentItemBody { .. }
        | AgentThreadClose { .. }
        | AgentThreadReopen { .. }
        | AgentSend { .. }
        | AgentInterrupt { .. }
        | AgentRespond { .. }
        | AgentSetMode { .. }
        | AgentSetModel { .. }
        | AgentMarkSeen { .. }
        | AgentCheckpoints { .. }
        | AgentRevert { .. }
        | AgentAccountLogin { .. }
        | AgentAccountLogout { .. }
        | AgentStop { .. }
        // Delegation requests are always local, so none enters remote-id translation.
        | DelegationRun { .. }
        | DelegationComplete { .. }
        | DelegationList { .. }
        | DelegationGet { .. }
        | DelegationCancel { .. }
        | DelegationWait { .. } => {}
        CreateWorktree {
            host: placement, ..
        }
        | CreateWorktreeFromPr {
            host: placement, ..
        } => *placement = None,
        CreateWorktreeFromCard {
            host: placement, ..
        } => {
            // This request follows the card's board, which may be owned by a host the worktree is
            // not meant to land on. Clearing the placement only when it already names the owner
            // keeps "create it locally" meaning *there*, and leaves a third host's placement for
            // the owner to act on.
            if placement.as_ref() == Some(host) {
                *placement = None;
            }
        }
        DeleteWorktrees { ids: worktrees } | InspectWorktrees { ids: worktrees, .. } => {
            for worktree in worktrees.iter().cloned() {
                ids.register_worktree(host, worktree);
            }
        }
        PruneWorktrees {
            ids: Some(worktrees),
            ..
        } => {
            for worktree in worktrees.iter().cloned() {
                ids.register_worktree(host, worktree);
            }
        }
        PruneWorktrees { ids: None, .. } => {}
        KillWorktree { id }
        | SleepWorktree { id }
        | TouchWorktreeOpened { id }
        | WorktreePath { id } => ids.register_worktree(host, id.clone()),
        EnsureSession {
            worktree: Some(worktree),
            ..
        } => ids.register_worktree(host, worktree.clone()),
        EnsureSession { worktree: None, .. } => {}
        KillSession { session }
        | SleepSession { session }
        | NewTerminal { session, .. }
        | ListWatches { session } => *session = remote_session(host, session, ids)?,
        SetAgentActivity {
            session,
            terminal_id,
            ..
        } => {
            *session = remote_session(host, session, ids)?;
            *terminal_id = remote_terminal(host, *terminal_id, ids)?;
        }
        SelectTerminal { session, terminal } => {
            *session = remote_session(host, session, ids)?;
            *terminal = remote_terminal(host, *terminal, ids)?;
        }
        StartWatch { terminal, .. }
        | CloseTerminal { terminal }
        | RestartTerminal { terminal }
        | RenameTerminal { terminal, .. }
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
        | PasteTerminal { terminal, .. } => {
            *terminal = remote_terminal(host, *terminal, ids)?;
        }
        CancelJob { job } | RetryJob { job } | TailJob { job, .. } => {
            *job = remote_job(host, job, ids)?;
        }
        DismissJobs { jobs } => {
            for job in jobs {
                *job = remote_job(host, job, ids)?;
            }
        }
        AgentThreadList
        | AgentSeenCursors
        | AgentClosedThreads
        | ListBoards { .. }
        | GetBoard { .. }
        | EnsureBoard { .. }
        | EnsureWorktreeBoard { .. }
        | CreateBoard { .. }
        | CreateWorktreeBoard { .. }
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
        | ListSessions
        | CurrentSession
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
        | DaemonShutdown { .. } => {}
    }
    Ok(body)
}

/// Rewrites a remote response into the local daemon's id space.
#[must_use]
pub fn response_to_local(mut body: ResponseBody, host: &HostId, ids: &RemoteIds) -> ResponseBody {
    use ResponseBody::*;
    match &mut body {
        AgentThreads(summaries) => {
            for summary in summaries {
                translate_summary(summary, host, ids);
            }
        }
        AgentThreadCreated(summary) => translate_summary(summary, host, ids),
        AgentThreadSnapshot { projection, .. } => translate_projection(projection, host, ids),
        // A window carries the owner's own thread id inside its summary; the storage stage owns
        // rewriting its rows, and until then the id space is left as the owner sent it.
        AgentThreadWindow(window) => translate_summary(&mut window.summary, host, ids),
        // A checkpoint identity is scoped to its thread, and thread ids pass through unchanged.
        // A sign-in URL is the owner's own loopback callback and is never rewritten — but the
        // account verbs are refused on a mirror, so this arm is unreachable in practice.
        AgentItemBodyChunk { .. }
        | AgentCheckpoints(_)
        | AgentReverted(_)
        | AgentAccountLogin { .. }
        | AgentSeenCursors(_)
        | AgentClosedThreads(_)
        // Delegation responses are always local, so none enters remote-id translation.
        | DelegationStarted { .. }
        | Delegations(_)
        | Delegation(_) => {}
        Boards(summaries) => {
            // A host's context boards are not addressable from here: their ids are derived from
            // context ids every daemon shares, so a `personal` summary would shadow this
            // daemon's own board. Only worktree boards survive, and each one is registered.
            summaries.retain(|summary| summary.worktree_id.is_some());
            for summary in summaries.iter() {
                ids.register_board(host, summary.id.clone());
            }
        }
        Board(view) => {
            if view.board.worktree_id.is_some() {
                ids.register_board(host, view.board.id.clone());
                // The owner just listed the board, so its card set is authoritative: a card it
                // dropped stops resolving to this host.
                ids.replace_board_cards(
                    &view.board.id,
                    view.cards.iter().map(|card| card.id.clone()),
                );
            }
        }
        Card(card) => ids.register_card(&card.board_id, card.id.clone()),
        CardWorktree { card, worktree, .. } => {
            ids.register_card(&card.board_id, card.id.clone());
            translate_worktree(worktree, host, ids);
        }
        Watches(watches) => {
            for watch in watches {
                translate_watch(watch, host, ids);
            }
        }
        WatchTail(tail) => translate_watch(&mut tail.watch, host, ids),
        Snapshot(snapshot) => translate_snapshot(snapshot, host, ids),
        CloneStarted(job) | Job(job) => translate_job(job, host, ids),
        Worktree {
            worktree,
            post_create_job,
            ..
        } => {
            translate_worktree(worktree, host, ids);
            if let Some(job) = post_create_job {
                translate_job(job, host, ids);
            }
        }
        Path { host: owner, .. } => *owner = Some(host.clone()),
        Session(session) => translate_session(session, host, ids),
        Sessions(sessions) => {
            for session in sessions {
                translate_session(session, host, ids);
            }
        }
        CurrentSession(session) => {
            if let Some(session) = session {
                *session = local_session(host, session, ids);
            }
        }
        Terminal(terminal) => translate_terminal(terminal, host, ids),
        Jobs(jobs) => {
            for job in jobs {
                translate_job(job, host, ids);
            }
        }
        JobCancelled(job) => *job = ids.local_job(host, job),
        Inspections(inspections) => {
            for inspection in inspections {
                inspection.host = host.to_string();
            }
        }
        AgentAck
        | BoardBackendSchema(_)
        | BoardBackends(_)
        | WatchStarted(_)
        | Hello { .. }
        | Ack
        | Context(_)
        | Repo(_)
        | RemoteRepos(_)
        | BaseRefs(_)
        | WorktreesDeleted(_)
        | Pruned(_)
        | Slept(_)
        | PullRequests(_)
        | Statuses(_)
        | JobLog(_)
        | Config(_)
        | KeepAliveRuleMatches(_)
        | Doctor(_)
        | Pong
        | Version { .. }
        | ShuttingDown => {}
    }
    body
}

/// Rewrites a remote event, dropping terminal events which cannot be addressed locally.
#[must_use]
pub fn event_to_local(mut event: Event, host: &HostId, ids: &RemoteIds) -> Option<Event> {
    match &mut event {
        Event::Agent { thread, .. } => ids.register_thread(host, *thread),
        Event::AgentSummary(summary) => translate_summary(summary, host, ids),
        Event::WatchStarted(watch) | Event::WatchExited(watch) => {
            translate_watch(watch, host, ids);
        }
        Event::SnapshotChanged(snapshot) => translate_snapshot(snapshot, host, ids),
        Event::JobUpdated(job) => translate_job(job, host, ids),
        Event::SessionChanged(session) => translate_session(session, host, ids),
        Event::AgentActivityChanged {
            session,
            terminal_id,
            ..
        } => {
            *session = local_session(host, session, ids);
            *terminal_id = existing_local_terminal(host, *terminal_id, ids)?;
        }
        Event::TerminalFrame(frame) => {
            frame.terminal = existing_local_terminal(host, frame.terminal, ids)?;
        }
        Event::TerminalExited { terminal, .. } => {
            let local = existing_local_terminal(host, *terminal, ids)?;
            *terminal = local;
            ids.forget_terminal(local);
        }
        Event::TerminalTitle { terminal, .. } | Event::TerminalReattach { terminal } => {
            *terminal = existing_local_terminal(host, *terminal, ids)?;
        }
        Event::HostLinkChanged {
            host: event_host, ..
        } => *event_host = host.clone(),
        Event::AgentResync { thread, .. }
        | Event::AgentSynchronized { thread }
        | Event::AgentWindow { thread } => ids.register_thread(host, *thread),
        Event::BoardChanged { .. }
        // Delegation events are emitted by the local owner and carry globally unique ids.
        | Event::DelegationChanged(_)
        | Event::WatchOutput { .. }
        | Event::WatchDismissed(_)
        | Event::Toast { .. }
        | Event::DaemonShuttingDown
        | Event::Unknown => {}
    }
    Some(event)
}

/// Merges responses returned by independently routed hosts.
pub fn merge_fanout(
    original: &RequestBody,
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    if matches!(
        original,
        RequestBody::DeleteWorktrees { .. }
            | RequestBody::InspectWorktrees { .. }
            | RequestBody::PruneWorktrees { .. }
    ) {
        let hook_parts = parts
            .iter()
            .map(|(host, result)| {
                (
                    host.clone(),
                    match result {
                        Ok(response) => Ok(response.clone()),
                        Err(error) => Err(DaemonError::Remote(error.to_string())),
                    },
                )
            })
            .collect();
        if let Some(result) = super::lifecycle::merge_lifecycle_fanout(original, hook_parts) {
            return result;
        }
        return merge_lifecycle_vectors(parts);
    }

    match original {
        RequestBody::DismissJobs { .. } => {
            for (_, result) in parts {
                match result? {
                    ResponseBody::Ack => {}
                    other => return Err(unexpected_fanout_response("ack", &other)),
                }
            }
            Ok(ResponseBody::Ack)
        }
        RequestBody::AgentThreadList => {
            let mut merged = Vec::new();
            for (_, result) in parts {
                match result? {
                    ResponseBody::AgentThreads(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("agent threads", &other)),
                }
            }
            Ok(ResponseBody::AgentThreads(merged))
        }
        RequestBody::ListBoards { .. } => {
            let mut merged = Vec::new();
            for (_, result) in parts {
                match result? {
                    ResponseBody::Boards(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("boards", &other)),
                }
            }
            Ok(ResponseBody::Boards(merged))
        }
        _ => generic_vec_merge(parts),
    }
}

/// Adds the local partition of a mixed-host bulk request to its merged remote response.
pub(crate) fn merge_local_and_remote(
    original: &RequestBody,
    local: DaemonResult<ResponseBody>,
    remote: DaemonResult<ResponseBody>,
) -> DaemonResult<ResponseBody> {
    let local = local?;
    let remote = remote?;
    if matches!(
        original,
        RequestBody::DeleteWorktrees { .. }
            | RequestBody::InspectWorktrees { .. }
            | RequestBody::PruneWorktrees { .. }
    ) {
        let local_host = HostId::try_from("local-part").expect("static host id is valid");
        let remote_host = HostId::try_from("remote-part").expect("static host id is valid");
        return super::lifecycle::merge_lifecycle_fanout(
            original,
            vec![(local_host, Ok(local)), (remote_host, Ok(remote))],
        )
        .unwrap_or_else(|| {
            Err(DaemonError::Protocol(
                "lifecycle response was not mergeable".to_owned(),
            ))
        });
    }
    match (local, remote) {
        (ResponseBody::WorktreesDeleted(mut local), ResponseBody::WorktreesDeleted(mut remote)) => {
            local.append(&mut remote);
            Ok(ResponseBody::WorktreesDeleted(local))
        }
        (ResponseBody::Inspections(mut local), ResponseBody::Inspections(mut remote)) => {
            local.append(&mut remote);
            Ok(ResponseBody::Inspections(local))
        }
        (ResponseBody::Pruned(mut local), ResponseBody::Pruned(mut remote)) => {
            local.deleted.append(&mut remote.deleted);
            local.skipped.append(&mut remote.skipped);
            Ok(ResponseBody::Pruned(local))
        }
        (ResponseBody::AgentThreads(mut local), ResponseBody::AgentThreads(mut remote)) => {
            local.append(&mut remote);
            Ok(ResponseBody::AgentThreads(local))
        }
        (ResponseBody::Boards(mut local), ResponseBody::Boards(remote)) => {
            // The fanout part asked every host for all of its contexts, because a host scopes
            // boards by *its* context ids. The caller's filter is therefore applied here, against
            // the id the host sent, which is never rewritten.
            let wanted = match original {
                RequestBody::ListBoards { context_id } => context_id.clone(),
                _ => None,
            };
            for summary in remote {
                if wanted
                    .as_ref()
                    .is_some_and(|context| &summary.context_id != context)
                {
                    continue;
                }
                // The owner is authoritative about a board it owns: its summary replaces a
                // leftover local document for the same id instead of listing it twice.
                if let Some(existing) = local.iter_mut().find(|existing| existing.id == summary.id)
                {
                    *existing = summary;
                } else {
                    local.push(summary);
                }
            }
            Ok(ResponseBody::Boards(local))
        }
        (ResponseBody::Ack, ResponseBody::Ack) => Ok(ResponseBody::Ack),
        (local, remote) => Err(DaemonError::Protocol(format!(
            "cannot merge local and remote responses for {original:?}: {local:?}, {remote:?}"
        ))),
    }
}

fn merge_lifecycle_vectors(
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    let mut responses = parts.into_iter();
    let Some((_, first)) = responses.next() else {
        return Ok(ResponseBody::Ack);
    };
    match first? {
        ResponseBody::WorktreesDeleted(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::WorktreesDeleted(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("delete results", &other)),
                }
            }
            Ok(ResponseBody::WorktreesDeleted(merged))
        }
        ResponseBody::Inspections(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::Inspections(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("inspections", &other)),
                }
            }
            Ok(ResponseBody::Inspections(merged))
        }
        ResponseBody::Pruned(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::Pruned(mut value) => {
                        merged.deleted.append(&mut value.deleted);
                        merged.skipped.append(&mut value.skipped);
                    }
                    other => return Err(unexpected_fanout_response("prune results", &other)),
                }
            }
            Ok(ResponseBody::Pruned(merged))
        }
        other => Err(unexpected_fanout_response("lifecycle vector", &other)),
    }
}

fn generic_vec_merge(
    parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    let mut responses = parts.into_iter();
    let Some((_, first)) = responses.next() else {
        return Ok(ResponseBody::Ack);
    };
    match first? {
        ResponseBody::Sessions(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::Sessions(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("sessions", &other)),
                }
            }
            Ok(ResponseBody::Sessions(merged))
        }
        ResponseBody::Jobs(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::Jobs(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("jobs", &other)),
                }
            }
            Ok(ResponseBody::Jobs(merged))
        }
        ResponseBody::Statuses(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::Statuses(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("statuses", &other)),
                }
            }
            Ok(ResponseBody::Statuses(merged))
        }
        ResponseBody::WorktreesDeleted(mut merged) => {
            for (_, response) in responses {
                match response? {
                    ResponseBody::WorktreesDeleted(mut values) => merged.append(&mut values),
                    other => return Err(unexpected_fanout_response("delete results", &other)),
                }
            }
            Ok(ResponseBody::WorktreesDeleted(merged))
        }
        other => Err(unexpected_fanout_response("vector", &other)),
    }
}

/// Converts an unavailable host into the per-item response shape of a bulk request.
#[must_use]
pub(crate) fn unavailable_fanout_response(
    body: &RequestBody,
    host: &HostId,
    error: &DaemonError,
) -> Option<ResponseBody> {
    let reason = error.to_string();
    match body {
        RequestBody::AgentItemBody { .. }
        | RequestBody::AgentCheckpoints { .. }
        | RequestBody::AgentRevert { .. } => None,
        RequestBody::DeleteWorktrees { ids } => Some(ResponseBody::WorktreesDeleted(
            ids.iter()
                .cloned()
                .map(|worktree_id| WorktreeDeleteResult {
                    worktree_id,
                    ok: false,
                    reason: Some(reason.clone()),
                    trash_entry: None,
                })
                .collect(),
        )),
        RequestBody::InspectWorktrees { ids, .. } => {
            let mut results = Vec::with_capacity(ids.len());
            for worktree_id in ids.iter().cloned() {
                let Ok(repo_id) = fleet_core::ids::RepoId::try_from(worktree_id.repo()) else {
                    return None;
                };
                results.push(fleet_core::inspection::WorktreeInspection {
                    repo_id,
                    worktree_id,
                    host: host.to_string(),
                    path: String::new(),
                    branch: String::new(),
                    base_ref: String::new(),
                    head: None,
                    target_branch: String::new(),
                    upstream: None,
                    ahead: None,
                    behind: None,
                    upstream_gone: false,
                    dirty: false,
                    dirty_files: None,
                    merged_into_target: false,
                    unique_commits: None,
                    published: false,
                    merged: false,
                    pr: None,
                    session: fleet_core::sessions::SessionState::Unknown,
                    running: Vec::new(),
                    inspected_at: chrono::Utc::now().to_rfc3339(),
                    warnings: Vec::new(),
                    error: Some(reason.clone()),
                });
            }
            Some(ResponseBody::Inspections(results))
        }
        RequestBody::PruneWorktrees {
            dry_run,
            ids: Some(ids),
            ..
        } => Some(ResponseBody::Pruned(PruneResult {
            dry_run: *dry_run,
            deleted: Vec::new(),
            skipped: ids
                .iter()
                .cloned()
                .map(|worktree_id| PruneSkipped {
                    worktree_id,
                    reason: reason.clone(),
                    merged: false,
                    dirty: false,
                    unique_commits: None,
                    running: Vec::new(),
                })
                .collect(),
        })),
        // An unreachable host must never hide this daemon's own boards, so its partition is an
        // empty listing rather than a failure; `Router::fanout` logs the error it replaces.
        RequestBody::ListBoards { .. } => Some(ResponseBody::Boards(Vec::new())),
        RequestBody::PruneWorktrees { ids: None, .. }
        | RequestBody::AgentThreadList
        | RequestBody::AgentSeenCursors
        | RequestBody::AgentClosedThreads
        | RequestBody::AgentThreadCreate { .. }
        | RequestBody::AgentThreadOpen { .. }
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
        | RequestBody::DismissJobs { .. }
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

fn remote_terminal(host: &HostId, local: TerminalId, ids: &RemoteIds) -> DaemonResult<TerminalId> {
    match ids.remote_terminal(local) {
        Some((owner, remote)) if &owner == host => Ok(remote),
        Some((owner, _)) => Err(DaemonError::Conflict(format!(
            "terminal {local} belongs to host {owner}, not {host}"
        ))),
        None => Err(DaemonError::NotFound(format!("terminal {local}"))),
    }
}

fn existing_local_terminal(
    host: &HostId,
    remote: TerminalId,
    ids: &RemoteIds,
) -> Option<TerminalId> {
    let local = ids.existing_local_terminal(host, remote);
    if local.is_none() {
        tracing::debug!(%host, terminal = %remote, "dropping remote event for unknown terminal");
    }
    local
}

fn remote_job(
    host: &HostId,
    local: &fleet_core::ids::JobId,
    ids: &RemoteIds,
) -> DaemonResult<fleet_core::ids::JobId> {
    match ids.remote_job(local) {
        Some((owner, remote)) if &owner == host => Ok(remote),
        Some((owner, _)) => Err(DaemonError::Conflict(format!(
            "job {local} belongs to host {owner}, not {host}"
        ))),
        None => Err(DaemonError::NotFound(format!("job {local}"))),
    }
}

fn remote_session(host: &HostId, local: &SessionId, ids: &RemoteIds) -> DaemonResult<SessionId> {
    match ids.remote_session(local.as_ref()) {
        Some((owner, remote)) if &owner == host => {
            SessionId::try_from(remote).map_err(|error| DaemonError::Protocol(error.to_string()))
        }
        Some((owner, _)) => Err(DaemonError::Conflict(format!(
            "session {local} belongs to host {owner}, not {host}"
        ))),
        None => Err(DaemonError::NotFound(format!("session {local}"))),
    }
}

fn local_session(host: &HostId, remote: &SessionId, ids: &RemoteIds) -> SessionId {
    let local = ids.local_session(host, remote.as_ref());
    SessionId::try_from(local).unwrap_or_else(|_| remote.clone())
}

fn translate_terminal(terminal: &mut Terminal, host: &HostId, ids: &RemoteIds) {
    terminal.id = ids.local_terminal(host, terminal.id);
}

fn translate_session(session: &mut Session, host: &HostId, ids: &RemoteIds) {
    session.host = Some(host.clone());
    session.id = local_session(host, &session.id, ids);
    for terminal in &mut session.terminals {
        translate_terminal(terminal, host, ids);
    }
    session.active_terminal = session
        .active_terminal
        .map(|terminal| ids.local_terminal(host, terminal));
}

fn translate_worktree(worktree: &mut Worktree, host: &HostId, ids: &RemoteIds) {
    ids.register_worktree(host, worktree.id.clone());
    worktree.host = Some(host.clone());
    worktree.session = ids.local_session(host, &worktree.session);
}

fn translate_job(job: &mut JobRecord, host: &HostId, ids: &RemoteIds) {
    job.id = ids.local_job(host, &job.id);
}

fn translate_watch(watch: &mut Watch, host: &HostId, ids: &RemoteIds) {
    watch.session = local_session(host, &watch.session, ids);
    watch.terminal = ids.local_terminal(host, watch.terminal);
}

fn translate_summary(summary: &mut AgentThreadSummary, host: &HostId, ids: &RemoteIds) {
    ids.register_thread(host, summary.thread);
    ids.register_worktree(host, summary.worktree.clone());
    summary.host = Some(host.clone());
}

fn translate_projection(projection: &mut ThreadProjection, host: &HostId, ids: &RemoteIds) {
    ids.register_thread(host, projection.thread);
    ids.register_worktree(host, projection.worktree.clone());
}

pub(crate) fn translate_snapshot(snapshot: &mut Snapshot, host: &HostId, ids: &RemoteIds) {
    for worktree in &mut snapshot.worktrees {
        translate_worktree(worktree, host, ids);
    }
    for session in &mut snapshot.sessions {
        translate_session(session, host, ids);
    }
    for summary in &mut snapshot.agent_threads {
        translate_summary(summary, host, ids);
    }
    for status in &snapshot.statuses {
        ids.register_worktree(host, status.worktree_id.clone());
    }
    for job in &mut snapshot.jobs {
        translate_job(job, host, ids);
    }
}

fn unexpected_fanout_response(expected: &str, actual: &ResponseBody) -> DaemonError {
    DaemonError::Protocol(format!(
        "fanout expected {expected} response, received {actual:?}"
    ))
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        board::{Board, BoardSettings, BoardSummary, BoardView, Card, Priority, SyncState},
        ids::{BoardId, CardId, ContextId, StatusId, WorktreeId},
    };

    use super::*;

    #[test]
    fn a_card_worktree_placement_is_cleared_only_for_the_board_owner() {
        let owner = host("dev-box");
        let elsewhere = host("build-box");
        let ids = RemoteIds::default();

        let translated = to_remote(create_from_card(Some(owner.clone())), &owner, &ids)
            .expect("translate a placement naming the owner");
        assert!(matches!(
            translated,
            RequestBody::CreateWorktreeFromCard { host: None, .. }
        ));

        let translated = to_remote(create_from_card(Some(elsewhere.clone())), &owner, &ids)
            .expect("translate a placement naming a third host");
        assert!(matches!(
            translated,
            RequestBody::CreateWorktreeFromCard { host: Some(placement), .. }
                if placement == elsewhere
        ));

        let translated =
            to_remote(create_from_card(None), &owner, &ids).expect("translate an unplaced request");
        assert!(matches!(
            translated,
            RequestBody::CreateWorktreeFromCard { host: None, .. }
        ));
    }

    #[test]
    fn a_hosted_board_listing_keeps_only_worktree_boards_and_registers_them() {
        let owner = host("dev-box");
        let ids = RemoteIds::default();
        let scoped = board("wt-acme-api-feature");
        let context = board("personal");

        let response = response_to_local(
            ResponseBody::Boards(vec![
                summary(scoped.clone(), Some("acme/api#feature")),
                summary(context.clone(), None),
            ]),
            &owner,
            &ids,
        );

        let ResponseBody::Boards(summaries) = response else {
            panic!("expected a board listing");
        };
        assert_eq!(
            summaries
                .iter()
                .map(|value| value.id.clone())
                .collect::<Vec<_>>(),
            vec![scoped.clone()]
        );
        assert_eq!(ids.host_of_board(&scoped), Some(owner));
        assert_eq!(ids.host_of_board(&context), None);
    }

    #[test]
    fn a_forwarded_board_view_registers_the_board_and_replaces_its_cards() {
        let owner = host("dev-box");
        let ids = RemoteIds::default();
        let scoped = board("wt-acme-api-feature");
        let first = card("card-one");
        let second = card("card-two");
        ids.register_card(&scoped, second.clone());

        let response = response_to_local(
            ResponseBody::Board(view(
                &scoped,
                Some("acme/api#feature"),
                std::slice::from_ref(&first),
            )),
            &owner,
            &ids,
        );
        assert!(matches!(response, ResponseBody::Board(_)));
        assert_eq!(ids.host_of_board(&scoped), Some(owner.clone()));
        assert_eq!(ids.board_of_card(&first), Some(scoped.clone()));
        assert_eq!(ids.board_of_card(&second), None);

        // A context board arrives with the same id as this daemon's own; nothing is claimed.
        let context = board("personal");
        let passed_through = response_to_local(
            ResponseBody::Board(view(&context, None, &[card("card-three")])),
            &owner,
            &ids,
        );
        assert!(matches!(passed_through, ResponseBody::Board(_)));
        assert_eq!(ids.host_of_board(&context), None);
        assert_eq!(ids.board_of_card(&card("card-three")), None);
    }

    #[test]
    fn a_forwarded_card_registers_the_board_it_names() {
        let owner = host("dev-box");
        let ids = RemoteIds::default();
        let scoped = board("wt-acme-api-feature");

        let answer = response_to_local(
            ResponseBody::Card(card_payload(&scoped, card("card-one"))),
            &owner,
            &ids,
        );
        assert!(matches!(answer, ResponseBody::Card(_)));
        assert_eq!(ids.board_of_card(&card("card-one")), Some(scoped.clone()));

        let answer = response_to_local(
            ResponseBody::CardWorktree {
                card: card_payload(&scoped, card("card-two")),
                worktree: worktree_record(),
                created: true,
            },
            &owner,
            &ids,
        );
        assert!(matches!(answer, ResponseBody::CardWorktree { .. }));
        assert_eq!(ids.board_of_card(&card("card-two")), Some(scoped));
        assert_eq!(
            ids.host_of_worktree(&WorktreeId::try_from("acme/api#feature").expect("worktree")),
            Some(owner)
        );
    }

    #[test]
    fn listing_boards_concatenates_hosts_and_lets_the_owner_replace_a_local_row() {
        let first = host("dev-box");
        let second = host("build-box");
        let shared = board("wt-acme-api-feature");
        let original = RequestBody::ListBoards { context_id: None };

        let merged = merge_fanout(
            &original,
            vec![
                (
                    first,
                    Ok(ResponseBody::Boards(vec![summary(
                        shared.clone(),
                        Some("acme/api#feature"),
                    )])),
                ),
                (
                    second,
                    Ok(ResponseBody::Boards(vec![summary(
                        board("wt-acme-api-other"),
                        Some("acme/api#other"),
                    )])),
                ),
            ],
        )
        .expect("merge a board fanout");

        let with_local = merge_local_and_remote(
            &original,
            Ok(ResponseBody::Boards(vec![
                stale(&shared),
                summary(board("personal"), None),
            ])),
            Ok(merged),
        )
        .expect("merge local and remote boards");

        let ResponseBody::Boards(summaries) = with_local else {
            panic!("expected a board listing");
        };
        assert_eq!(summaries.len(), 3);
        // Local rows keep their position and the owner's summary replaces the stale one in place.
        assert_eq!(summaries[0].id, shared);
        assert_eq!(summaries[0].card_count, 11);
        assert_eq!(summaries[1].id, board("personal"));
        assert_eq!(summaries[2].id, board("wt-acme-api-other"));
    }

    #[test]
    fn listing_boards_for_one_context_keeps_only_that_contexts_hosted_summaries() {
        let original = RequestBody::ListBoards {
            context_id: Some(context("personal")),
        };
        let mut other = summary(board("wt-acme-api-other"), Some("acme/api#other"));
        other.context_id = context("work");

        let merged = merge_local_and_remote(
            &original,
            Ok(ResponseBody::Boards(Vec::new())),
            Ok(ResponseBody::Boards(vec![
                summary(board("wt-acme-api-feature"), Some("acme/api#feature")),
                other,
            ])),
        )
        .expect("merge a context-scoped listing");

        let ResponseBody::Boards(summaries) = merged else {
            panic!("expected a board listing");
        };
        assert_eq!(
            summaries
                .iter()
                .map(|value| value.id.clone())
                .collect::<Vec<_>>(),
            vec![board("wt-acme-api-feature")]
        );
    }

    #[test]
    fn an_unreachable_host_contributes_an_empty_board_listing() {
        let unreachable = DaemonError::Remote("host dev-box is unreachable".to_owned());
        assert_eq!(
            unavailable_fanout_response(
                &RequestBody::ListBoards { context_id: None },
                &host("dev-box"),
                &unreachable,
            ),
            Some(ResponseBody::Boards(Vec::new()))
        );
    }

    fn host(value: &str) -> HostId {
        value.parse().expect("host id")
    }

    fn board(value: &str) -> BoardId {
        value.parse().expect("board id")
    }

    fn card(value: &str) -> CardId {
        value.parse().expect("card id")
    }

    fn context(value: &str) -> ContextId {
        value.parse().expect("context id")
    }

    fn create_from_card(placement: Option<HostId>) -> RequestBody {
        RequestBody::CreateWorktreeFromCard {
            card_id: card("card-one"),
            repo_id: None,
            base: None,
            host: placement,
        }
    }

    fn summary(id: BoardId, worktree: Option<&str>) -> BoardSummary {
        BoardSummary {
            id,
            context_id: context("personal"),
            worktree_id: worktree.map(|id| WorktreeId::try_from(id).expect("worktree id")),
            name: "board".to_owned(),
            prefix: "FLT".to_owned(),
            backend_kind: "local".to_owned(),
            card_count: 11,
            open_count: 0,
            dirty_count: 0,
            conflict_count: 0,
            last_synced_at: None,
            last_error: None,
        }
    }

    /// The empty document this daemon created for a worktree another host owns.
    fn stale(id: &BoardId) -> BoardSummary {
        BoardSummary {
            card_count: 0,
            ..summary(id.clone(), Some("acme/api#feature"))
        }
    }

    fn view(id: &BoardId, worktree: Option<&str>, cards: &[CardId]) -> BoardView {
        BoardView {
            board: Board {
                id: id.clone(),
                context_id: context("personal"),
                worktree_id: worktree.map(|id| WorktreeId::try_from(id).expect("worktree id")),
                name: "board".to_owned(),
                prefix: "FLT".to_owned(),
                next_number: 1,
                backend: fleet_core::board::BackendRef::default(),
                statuses: Vec::new(),
                labels: Vec::new(),
                properties: Vec::new(),
                default_repo_id: None,
                settings: BoardSettings::default(),
                sync: SyncState::default(),
                created_at: "2026-09-20T12:00:00Z".to_owned(),
                updated_at: "2026-09-20T12:00:00Z".to_owned(),
            },
            cards: cards
                .iter()
                .cloned()
                .map(|card| card_payload(id, card))
                .collect(),
        }
    }

    fn card_payload(board_id: &BoardId, id: CardId) -> Card {
        Card {
            id,
            board_id: board_id.clone(),
            number: 1,
            title: "card".to_owned(),
            description: String::new(),
            status_id: StatusId::try_from("todo").expect("status id"),
            priority: Priority::default(),
            labels: Vec::new(),
            assignee: None,
            estimate: None,
            due_date: None,
            parent_id: None,
            properties: Default::default(),
            repo_id: None,
            worktree_id: None,
            activity: Vec::new(),
            comments: Vec::new(),
            remote: None,
            conflict: None,
            dirty: false,
            archived: false,
            position: 0,
            created_at: "2026-09-20T12:00:00Z".to_owned(),
            updated_at: "2026-09-20T12:00:00Z".to_owned(),
        }
    }

    fn worktree_record() -> Worktree {
        Worktree {
            id: WorktreeId::try_from("acme/api#feature").expect("worktree id"),
            repo_id: "acme/api".parse().expect("repo id"),
            slug: "feature".to_owned(),
            branch: "feature".to_owned(),
            base_ref: "origin/main".to_owned(),
            path: "/tmp/acme/api/feature".to_owned(),
            session: "acme/api/feature".to_owned(),
            host: None,
            created_at: "2026-09-20T12:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }
    }
}
