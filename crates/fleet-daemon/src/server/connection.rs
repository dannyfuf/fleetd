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
use tokio::sync::oneshot;
use tokio_util::{codec::Framed, sync::CancellationToken};

use crate::{DaemonError, DaemonResult, server::broadcast::BroadcastBus, services::Services};

/// One Unix-socket client actor with independent subscriptions and terminal attachments.
pub struct Connection {
    stream: UnixStream,
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
    #[cfg(test)]
    before_serialized_response: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
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
            #[cfg(test)]
            before_serialized_response: None,
        }
    }

    #[cfg(test)]
    fn with_before_serialized_response(
        mut self,
        ready: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) -> Self {
        self.before_serialized_response = Some((ready, release));
        self
    }

    /// Runs Hello negotiation followed by request and event multiplexing.
    pub async fn run(self) -> DaemonResult<()> {
        #[cfg(test)]
        let mut before_serialized_response = self.before_serialized_response;
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

        let watch_owner = self.services.watches.owner();
        let owner_id = watch_owner.id;
        let mut subscriptions = HashSet::new();
        let mut attached = HashSet::new();
        let mut events = self.events.subscribe();
        let mut frames = self.services.sessions.subscribe_frames();
        let mut pending = FuturesUnordered::<DispatchFuture>::new();
        // Service requests may complete out of order, but terminal input is a byte stream. Chain
        // terminal requests in socket-read order while leaving unrelated daemon work concurrent.
        let mut terminal_order_tail: Option<oneshot::Receiver<()>> = None;
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
                            if let Err(error) = send_response(
                                &mut framed,
                                Response { id, result: result.map_err(Into::into) },
                            )
                            .await
                            {
                                break Err(error);
                            }
                        }
                        RequestBody::Subscribe { events } => {
                            subscriptions.extend(events);
                            if let Err(error) = send_response(
                                &mut framed,
                                Response { id, result: Ok(ResponseBody::Ack) },
                            )
                            .await
                            {
                                break Err(error);
                            }
                        }
                        RequestBody::Unsubscribe => {
                            subscriptions.clear();
                            if let Err(error) = send_response(
                                &mut framed,
                                Response { id, result: Ok(ResponseBody::Ack) },
                            )
                            .await
                            {
                                break Err(error);
                            }
                        }
                        body => {
                            // Attachment membership and PTY dimensions are one ordered piece of
                            // per-connection state. Running these requests in `pending` lets two
                            // Attach calls both observe a missing membership (leaking a service
                            // refcount), or lets an older resize finish after a newer one. They are
                            // infrequent and must complete here before this actor accepts the next
                            // control request.
                            if terminal_attachment_request_is_serialized(&body) {
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
                                let result = match body {
                                    RequestBody::AttachTerminal { terminal, cols, rows }
                                        if resize_existing =>
                                    {
                                        self.services.sessions.resize(terminal, cols, rows).await
                                            .map(|()| ResponseBody::Ack)
                                    }
                                    RequestBody::DetachTerminal { .. } if detach_missing => {
                                        Ok(ResponseBody::Ack)
                                    }
                                    body => self.services.dispatch_owned(body, owner_id).await,
                                };
                                let succeeded = result.is_ok();
                                if succeeded
                                    && let Some((attach, terminal)) = attachment
                                {
                                    if attach {
                                        attached.insert(terminal);
                                    } else {
                                        attached.remove(&terminal);
                                    }
                                }
                                if succeeded {
                                    effects.publish(&result, &self.services, &self.events);
                                }
                                #[cfg(test)]
                                if let Some((ready, release)) = before_serialized_response.take()
                                {
                                    let _ = ready.send(());
                                    let _ = release.await;
                                }
                                if let Err(error) = send_response(
                                    &mut framed,
                                    Response { id, result: result.map_err(Into::into) },
                                )
                                .await
                                {
                                    break Err(error);
                                }
                                continue;
                            }
                            let terminal_order = pty_input_request_is_ordered(&body).then(|| {
                                let previous = terminal_order_tail.take();
                                let (release, next) = oneshot::channel();
                                terminal_order_tail = Some(next);
                                (previous, release)
                            });
                            let services = Arc::clone(&self.services);
                            pending.push(Box::pin(async move {
                                let release_terminal_order = if let Some((previous, release)) = terminal_order {
                                    if let Some(previous) = previous {
                                        let _ = previous.await;
                                    }
                                    Some(release)
                                } else {
                                    None
                                };
                                let result = services.dispatch_owned(body, owner_id).await;
                                if let Some(release) = release_terminal_order {
                                    let _ = release.send(());
                                }
                                CompletedRequest { id, result, effects, shutdown_request }
                            }));
                        }
                    }
                }
                Some(completed) = pending.next(), if !pending.is_empty() => {
                    let CompletedRequest { id, result, effects, shutdown_request } = completed;
                    let succeeded = result.is_ok();
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
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            // A broadcast gap does not identify which terminal lost rows.
                            for terminal in &attached {
                                let _ = self.services.sessions.request_full_frame(*terminal).await;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                    }
                }
            }
        };
        drop(pending);
        drop(watch_owner);
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

/// Requests whose effects append to the PTY input byte stream.
fn pty_input_request_is_ordered(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::StartWatch { .. }
            | RequestBody::AppendWatchOutput { .. }
            | RequestBody::FinishWatch { .. }
            | RequestBody::TerminalInput { .. }
            | RequestBody::TerminalKey { .. }
            | RequestBody::TerminalMouse { .. }
            | RequestBody::PasteTerminal { .. }
            | RequestBody::WheelTerminal { .. }
            | RequestBody::ScrollOrKeyTerminal { .. }
            | RequestBody::ScrollTerminal { .. }
    )
}

fn terminal_attachment_request_is_serialized(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::AttachTerminal { .. }
            | RequestBody::DetachTerminal { .. }
            | RequestBody::ResizeTerminal { .. }
    )
}

struct CompletedRequest {
    id: u64,
    result: DaemonResult<ResponseBody>,
    effects: RequestEffects,
    shutdown_request: Option<bool>,
}

struct RequestEffects {
    snapshot_changed: bool,
    session_changed: bool,
    previous_session: Option<SessionId>,
}

impl RequestEffects {
    fn for_request(body: &RequestBody, services: &Services) -> Self {
        // Watch output is a hot path and never changes session/snapshot metadata.
        if matches!(
            body,
            RequestBody::StartWatch { .. }
                | RequestBody::AppendWatchOutput { .. }
                | RequestBody::FinishWatch { .. }
                | RequestBody::ListWatches { .. }
                | RequestBody::TailWatch { .. }
                | RequestBody::DismissWatch { .. }
        ) {
            return Self {
                snapshot_changed: false,
                session_changed: false,
                previous_session: None,
            };
        }
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
        Event::WatchStarted(_) => EventKind::WatchStarted,
        Event::WatchOutput { .. } => EventKind::WatchOutput,
        Event::WatchExited(_) => EventKind::WatchExited,
        Event::WatchDismissed(_) => EventKind::WatchDismissed,
        Event::SnapshotChanged(_) => EventKind::SnapshotChanged,
        Event::JobUpdated(_) => EventKind::JobUpdated,
        Event::SessionChanged(_) => EventKind::SessionChanged,
        Event::AgentActivityChanged { .. } => EventKind::AgentActivityChanged,
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
    use std::{net::Shutdown, path::Path, time::Duration};

    use fleet_core::{config::Agent, ids::TerminalId};

    use crate::{
        adapters::{Adapters, clock::SystemClock, files::RealFiles},
        jobs::JobManager,
        stores::{config::ConfigStore, state::StateStore},
    };

    use super::*;

    async fn test_services(home: &Path) -> Arc<Services> {
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let mut effective = config.load().await.expect("load test config");
        effective.agent_commands.claude = "/bin/sleep 30".into();
        config.save(effective).await.expect("save test config");
        let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
        Services::new_with_events(
            home,
            config,
            state,
            Arc::new(JobManager::new(home)),
            Adapters::system(files),
            BroadcastBus::default(),
        )
    }

    #[tokio::test]
    async fn response_write_failure_after_attach_runs_detach_cleanup() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let services = test_services(temp.path()).await;
        let session = services
            .sessions
            .ensure(None, Some(Agent::Claude), false)
            .await
            .expect("ensure agent session");
        let terminal = session.terminals[0].id;
        let (server, client) = UnixStream::pair().expect("create socket pair");
        let (response_ready, wait_for_response) = oneshot::channel();
        let (release_response, response_released) = oneshot::channel();
        let actor = tokio::spawn(
            Connection::new(
                server,
                Arc::clone(&services),
                services.events.clone(),
                CancellationToken::new(),
            )
            .with_before_serialized_response(response_ready, response_released)
            .run(),
        );
        let mut client = Framed::new(client, FleetCodec::<Request, Response>::new());
        client
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    client: "connection-test".into(),
                },
            })
            .await
            .expect("send hello");
        client
            .next()
            .await
            .expect("receive hello response")
            .expect("decode hello response")
            .result
            .expect("hello succeeds");
        client
            .send(Request {
                id: 2,
                body: RequestBody::AttachTerminal {
                    terminal,
                    cols: 80,
                    rows: 24,
                },
            })
            .await
            .expect("send attach");

        tokio::time::timeout(Duration::from_secs(2), wait_for_response)
            .await
            .expect("actor reaches attachment response")
            .expect("attachment response barrier remains open");
        let client = client
            .into_inner()
            .into_std()
            .expect("convert client socket");
        client
            .shutdown(Shutdown::Both)
            .expect("reject response writes");
        drop(client);
        release_response
            .send(())
            .expect("release attachment response");
        let result = tokio::time::timeout(Duration::from_secs(2), actor)
            .await
            .expect("connection actor exits")
            .expect("connection task does not panic");
        assert!(result.is_err(), "the acknowledgement write must fail");
        assert_eq!(services.sessions.attachment_count(terminal), 0);

        services
            .sessions
            .kill(session.id)
            .await
            .expect("kill test session");
    }

    #[test]
    fn watch_events_are_global_and_require_their_subscription() {
        let event = Event::WatchDismissed(fleet_core::watches::WatchId(1));
        assert!(event_visible(
            &event,
            &HashSet::from([EventKind::WatchDismissed]),
            &HashSet::new()
        ));
        assert!(!event_visible(&event, &HashSet::new(), &HashSet::new()));
    }

    #[test]
    fn wheel_and_scroll_share_the_pty_ordering_channel() {
        use fleet_proto::terminal::{Modifiers, ScrollCommand, WheelEvent};
        assert!(pty_input_request_is_ordered(&RequestBody::WheelTerminal {
            terminal: TerminalId(1),
            wheel: WheelEvent {
                steps: -1,
                col: 0,
                row: 0,
                mods: Modifiers::empty()
            },
        }));
        assert!(pty_input_request_is_ordered(
            &RequestBody::ScrollOrKeyTerminal {
                terminal: TerminalId(1),
                scroll: ScrollCommand::Pages(-1),
                key: fleet_proto::terminal::KeyEvent {
                    key: fleet_proto::terminal::Key::PageUp,
                    mods: Modifiers::SHIFT,
                    text: None,
                    action: fleet_proto::terminal::KeyAction::Press,
                },
            }
        ));
        assert!(pty_input_request_is_ordered(&RequestBody::ScrollTerminal {
            terminal: TerminalId(1),
            scroll: ScrollCommand::Bottom,
        }));
    }

    #[test]
    fn attachment_and_resize_requests_are_serialized_by_the_connection_actor() {
        let terminal = TerminalId(1);
        assert!(terminal_attachment_request_is_serialized(
            &RequestBody::AttachTerminal {
                terminal,
                cols: 80,
                rows: 24,
            }
        ));
        assert!(terminal_attachment_request_is_serialized(
            &RequestBody::DetachTerminal { terminal }
        ));
        assert!(terminal_attachment_request_is_serialized(
            &RequestBody::ResizeTerminal {
                terminal,
                cols: 120,
                rows: 40,
            }
        ));
        assert!(!terminal_attachment_request_is_serialized(
            &RequestBody::RequestFullFrame { terminal }
        ));
    }

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
