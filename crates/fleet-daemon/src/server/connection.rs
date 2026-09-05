//! Per-client protocol decoding, dispatch, responses, and subscriptions.

use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc};

use fleet_core::{ids::SessionId, sessions::SessionKind};
use fleet_proto::{
    codec::FleetCodec,
    event::{Event, EventKind},
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt, stream::FuturesUnordered};
use tokio::net::UnixStream;
use tokio_util::{codec::Framed, sync::CancellationToken};

use crate::{DaemonError, DaemonResult, server::broadcast::BroadcastBus, services::Services};

/// One Unix-socket client actor with independent subscriptions and terminal attachments.
pub struct Connection {
    stream: UnixStream,
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
}

impl Connection {
    /// Creates a connection actor for an accepted stream.
    #[must_use]
    pub fn new(
        stream: UnixStream,
        services: Arc<Services>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            stream,
            services,
            events,
            shutdown,
        }
    }

    /// Runs Hello negotiation followed by request and event multiplexing.
    pub async fn run(self) -> DaemonResult<()> {
        let mut framed = Framed::new(self.stream, FleetCodec::<serde_json::Value, Request>::new());
        let Some(first) = framed.next().await else {
            return Ok(());
        };
        let first = first.map_err(|error| DaemonError::Protocol(error.to_string()))?;
        match first.body {
            RequestBody::Hello {
                protocol: fleet_proto::PROTOCOL_VERSION,
                ..
            } => {
                send_response(
                    &mut framed,
                    Response {
                        id: first.id,
                        result: Ok(ResponseBody::Hello {
                            protocol: fleet_proto::PROTOCOL_VERSION,
                            server: Services::version(),
                        }),
                    },
                )
                .await?;
            }
            RequestBody::Hello { protocol, .. } => {
                send_response(
                    &mut framed,
                    Response {
                        id: first.id,
                        result: Err(DaemonError::Unsupported(format!(
                            "unsupported protocol {protocol}; expected {}",
                            fleet_proto::PROTOCOL_VERSION
                        ))
                        .into()),
                    },
                )
                .await?;
                return Ok(());
            }
            _ => {
                send_response(
                    &mut framed,
                    Response {
                        id: first.id,
                        result: Err(DaemonError::Protocol(
                            "Hello must be the first request".to_owned(),
                        )
                        .into()),
                    },
                )
                .await?;
                return Ok(());
            }
        }

        let mut subscriptions = HashSet::new();
        let mut attached = HashSet::new();
        let mut events = self.events.subscribe();
        let mut frames = self.services.sessions.subscribe_frames();
        let mut pending = FuturesUnordered::<DispatchFuture>::new();
        let result = loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break Ok(()),
                request = framed.next() => {
                    let Some(request) = request else { break Ok(()); };
                    let request = match request {
                        Ok(request) => request,
                        Err(error) => break Err(DaemonError::Protocol(error.to_string())),
                    };
                    let id = request.id;
                    let shutdown_request = match &request.body {
                        RequestBody::DaemonShutdown { stop_sessions } => Some(*stop_sessions),
                        _ => None,
                    };
                    let effects = RequestEffects::for_request(&request.body, &self.services);
                    let attachment = match &request.body {
                        RequestBody::AttachTerminal { terminal, .. } => Some((true, *terminal)),
                        RequestBody::DetachTerminal { terminal } => Some((false, *terminal)),
                        _ => None,
                    };
                    match request.body {
                        RequestBody::Hello { .. } => {
                            let result = Err(DaemonError::Protocol("Hello is only valid as the first request".to_owned()));
                            send_response(&mut framed, Response { id, result: result.map_err(Into::into) }).await?;
                        }
                        RequestBody::Subscribe { events } => {
                            subscriptions.extend(events);
                            send_response(&mut framed, Response { id, result: Ok(ResponseBody::Ack) }).await?;
                        }
                        RequestBody::Unsubscribe => {
                            subscriptions.clear();
                            send_response(&mut framed, Response { id, result: Ok(ResponseBody::Ack) }).await?;
                        }
                        body => {
                            let services = Arc::clone(&self.services);
                            let resize_existing = matches!(
                                &body,
                                RequestBody::AttachTerminal { terminal, .. }
                                    if attached.contains(terminal)
                            );
                            let detach_missing = matches!(
                                &body,
                                RequestBody::DetachTerminal { terminal }
                                    if !attached.contains(terminal)
                            );
                            pending.push(Box::pin(async move {
                                let result = match body {
                                    RequestBody::AttachTerminal { terminal, cols, rows }
                                        if resize_existing =>
                                    {
                                        services.sessions.resize(terminal, cols, rows).await
                                            .map(|()| ResponseBody::Ack)
                                    }
                                    RequestBody::DetachTerminal { .. } if detach_missing => {
                                        Ok(ResponseBody::Ack)
                                    }
                                    body => services.dispatch(body).await,
                                };
                                CompletedRequest { id, result, effects, attachment, shutdown_request }
                            }));
                        }
                    }
                }
                Some(completed) = pending.next(), if !pending.is_empty() => {
                    let CompletedRequest { id, result, effects, attachment, shutdown_request } = completed;
                    let succeeded = result.is_ok();
                    if succeeded
                        && let Some((attach, terminal)) = attachment
                    {
                        if attach { attached.insert(terminal); } else { attached.remove(&terminal); }
                    }
                    if succeeded {
                        effects.publish(&result, &self.services, &self.events);
                    }
                    if shutdown_request == Some(true) && succeeded {
                        self.services.stop_all_sessions().await;
                    }
                    if let Err(error) = send_response(&mut framed, Response { id, result: result.map_err(Into::into) }).await {
                        break Err(error);
                    }
                    if shutdown_request.is_some() && succeeded {
                        self.events.publish(Event::DaemonShuttingDown);
                        self.shutdown.cancel();
                        break Ok(());
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(event) if event_visible(&event, &subscriptions, &attached) => {
                            if let Err(error) = send_event(&mut framed, event).await {
                                break Err(error);
                            }
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                    }
                }
                frame = frames.recv() => {
                    match frame {
                        Ok(frame)
                            if attached.contains(&frame.terminal)
                                && subscriptions.contains(&EventKind::TerminalFrame) =>
                        {
                            if let Err(error) = send_event(&mut framed, Event::TerminalFrame(frame)).await {
                                break Err(error);
                            }
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                    }
                }
            }
        };
        let detached_any = !attached.is_empty();
        for terminal in attached {
            if let Err(error) = self.services.sessions.detach(terminal).await
                && !matches!(error, DaemonError::NotFound(_))
            {
                tracing::warn!(%error, %terminal, "failed to detach disconnected client");
            }
        }
        if detached_any {
            self.events.request_snapshot(Arc::clone(&self.services));
        }
        result
    }
}

type DispatchFuture = Pin<Box<dyn Future<Output = CompletedRequest> + Send>>;

struct CompletedRequest {
    id: u64,
    result: DaemonResult<ResponseBody>,
    effects: RequestEffects,
    attachment: Option<(bool, fleet_core::ids::TerminalId)>,
    shutdown_request: Option<bool>,
}

struct RequestEffects {
    snapshot_changed: bool,
    session_changed: bool,
    previous_session: Option<SessionId>,
}

impl RequestEffects {
    fn for_request(body: &RequestBody, services: &Services) -> Self {
        let snapshot_changed = matches!(
            body,
            RequestBody::CreateContext { .. }
                | RequestBody::UpdateContext { .. }
                | RequestBody::DeleteContext { .. }
                | RequestBody::SetActiveContext { .. }
                | RequestBody::CloneRepo { .. }
                | RequestBody::DeleteRepo { .. }
                | RequestBody::MoveRepoToContext { .. }
                | RequestBody::SetRepoHooks { .. }
                | RequestBody::DismissClone { .. }
                | RequestBody::CreateWorktree { .. }
                | RequestBody::DeleteWorktrees { .. }
                | RequestBody::PruneWorktrees { .. }
                | RequestBody::TouchWorktreeOpened { .. }
                | RequestBody::RestoreTrash { .. }
                | RequestBody::RefreshStatuses { .. }
                | RequestBody::CreateWorktreeFromPr { .. }
                | RequestBody::EnsureSession { .. }
                | RequestBody::KillWorktree { .. }
                | RequestBody::SleepWorktree { .. }
                | RequestBody::KillSession { .. }
                | RequestBody::SleepSession { .. }
                | RequestBody::NewTerminal { .. }
                | RequestBody::CloseTerminal { .. }
                | RequestBody::RestartTerminal { .. }
                | RequestBody::RenameTerminal { .. }
                | RequestBody::SelectTerminal { .. }
                | RequestBody::AttachTerminal { .. }
                | RequestBody::DetachTerminal { .. }
                | RequestBody::DismissJobs { .. }
                | RequestBody::SetConfig { .. }
                | RequestBody::ImportFromSwarm
                | RequestBody::Update
        );
        let session_changed = matches!(
            body,
            RequestBody::EnsureSession { .. }
                | RequestBody::KillWorktree { .. }
                | RequestBody::SleepWorktree { .. }
                | RequestBody::KillSession { .. }
                | RequestBody::SleepSession { .. }
                | RequestBody::NewTerminal { .. }
                | RequestBody::CloseTerminal { .. }
                | RequestBody::RestartTerminal { .. }
                | RequestBody::RenameTerminal { .. }
                | RequestBody::SelectTerminal { .. }
        );
        let terminal = match body {
            RequestBody::CloseTerminal { terminal }
            | RequestBody::RestartTerminal { terminal }
            | RequestBody::RenameTerminal { terminal, .. } => Some(*terminal),
            _ => None,
        };
        let sessions = services.sessions.snapshot();
        let previous_session = match body {
            RequestBody::KillSession { session }
            | RequestBody::SleepSession { session }
            | RequestBody::NewTerminal { session, .. }
            | RequestBody::SelectTerminal { session, .. } => Some(session.clone()),
            RequestBody::KillWorktree { id } | RequestBody::SleepWorktree { id } => sessions
                .iter()
                .find(|session| matches!(&session.kind, SessionKind::Worktree(worktree) if worktree == id))
                .map(|session| session.id.clone()),
            _ => terminal.and_then(|terminal| {
                sessions
                    .iter()
                    .find(|session| session.terminals.iter().any(|item| item.id == terminal))
                    .map(|session| session.id.clone())
            }),
        };
        Self {
            snapshot_changed,
            session_changed,
            previous_session,
        }
    }

    fn publish(
        &self,
        result: &DaemonResult<ResponseBody>,
        services: &Arc<Services>,
        events: &BroadcastBus,
    ) {
        if self.snapshot_changed {
            events.request_snapshot(Arc::clone(services));
        }
        if !self.session_changed {
            return;
        }
        if let Ok(ResponseBody::Session(session)) = result {
            events.publish(Event::SessionChanged(session.clone()));
            return;
        }
        let sessions = services.sessions.snapshot();
        if let Ok(ResponseBody::Terminal(terminal)) = result
            && let Some(session) = sessions
                .iter()
                .find(|session| session.terminals.iter().any(|item| item.id == terminal.id))
        {
            events.publish(Event::SessionChanged(session.clone()));
            return;
        }
        if let Some(id) = &self.previous_session
            && let Some(session) = sessions.iter().find(|session| &session.id == id)
        {
            events.publish(Event::SessionChanged(session.clone()));
        }
    }
}

fn event_visible(
    event: &Event,
    subscriptions: &HashSet<EventKind>,
    attached: &HashSet<fleet_core::ids::TerminalId>,
) -> bool {
    if !subscriptions.contains(&event_kind(event)) {
        return false;
    }
    match event {
        Event::TerminalFrame(frame) => attached.contains(&frame.terminal),
        Event::TerminalExited { terminal, .. } | Event::TerminalTitle { terminal, .. } => {
            attached.contains(terminal)
        }
        _ => true,
    }
}

fn event_kind(event: &Event) -> EventKind {
    match event {
        Event::SnapshotChanged(_) => EventKind::SnapshotChanged,
        Event::JobUpdated(_) => EventKind::JobUpdated,
        Event::SessionChanged(_) => EventKind::SessionChanged,
        Event::TerminalFrame(_) => EventKind::TerminalFrame,
        Event::TerminalExited { .. } => EventKind::TerminalExited,
        Event::TerminalTitle { .. } => EventKind::TerminalTitle,
        Event::Toast { .. } => EventKind::Toast,
        Event::DaemonShuttingDown => EventKind::DaemonShuttingDown,
    }
}

async fn send_response(
    framed: &mut Framed<UnixStream, FleetCodec<serde_json::Value, Request>>,
    response: Response,
) -> DaemonResult<()> {
    let value = serde_json::to_value(response)?;
    framed
        .send(value)
        .await
        .map_err(|error| DaemonError::Protocol(error.to_string()))
}

async fn send_event(
    framed: &mut Framed<UnixStream, FleetCodec<serde_json::Value, Request>>,
    event: Event,
) -> DaemonResult<()> {
    let value = serde_json::to_value(event)?;
    framed
        .send(value)
        .await
        .map_err(|error| DaemonError::Protocol(error.to_string()))
}

#[cfg(test)]
mod tests {
    use fleet_core::ids::TerminalId;

    use super::*;

    #[test]
    fn terminal_events_are_visible_only_to_attached_subscribers() {
        let terminal = TerminalId(9);
        let event = Event::TerminalExited {
            terminal,
            code: Some(0),
        };
        let subscriptions = HashSet::from([EventKind::TerminalExited]);

        assert!(!event_visible(&event, &subscriptions, &HashSet::new()));
        assert!(event_visible(
            &event,
            &subscriptions,
            &HashSet::from([terminal])
        ));
        assert!(!event_visible(
            &event,
            &HashSet::new(),
            &HashSet::from([terminal])
        ));
    }
}
