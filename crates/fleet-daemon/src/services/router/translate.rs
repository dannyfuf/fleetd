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
        | AgentThreadClose { .. }
        | AgentSend { .. }
        | AgentInterrupt { .. }
        | AgentRespond { .. }
        | AgentSetMode { .. }
        | AgentSetModel { .. }
        | AgentMarkSeen { .. }
        | AgentStop { .. } => {}
        CreateWorktree {
            host: placement, ..
        }
        | CreateWorktreeFromPr {
            host: placement, ..
        }
        | CreateWorktreeFromCard {
            host: placement, ..
        } => *placement = None,
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
        | ListBoards { .. }
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
        CardWorktree { worktree, .. } => translate_worktree(worktree, host, ids),
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
        | Boards(_)
        | Board(_)
        | Card(_)
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
        Event::BoardChanged { .. }
        | Event::WatchOutput { .. }
        | Event::WatchDismissed(_)
        | Event::Toast { .. }
        | Event::DaemonShuttingDown => {}
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
