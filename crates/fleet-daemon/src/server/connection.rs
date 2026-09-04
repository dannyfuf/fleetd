//! Per-client protocol decoding, dispatch, responses, and subscriptions.

use std::{collections::HashSet, sync::Arc};

use fleet_proto::{
    codec::FleetCodec,
    event::{Event, EventKind},
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
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
            RequestBody::Hello { protocol: 1, .. } => {
                send_response(
                    &mut framed,
                    Response {
                        id: first.id,
                        result: Ok(ResponseBody::Hello {
                            protocol: 1,
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
                            "unsupported protocol {protocol}; expected 1"
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
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                request = framed.next() => {
                    let Some(request) = request else { break; };
                    let request = request.map_err(|error| DaemonError::Protocol(error.to_string()))?;
                    let id = request.id;
                    let shutdown_requested = matches!(request.body, RequestBody::DaemonShutdown { .. });
                    let attachment = match &request.body {
                        RequestBody::AttachTerminal { terminal, .. } => Some((true, *terminal)),
                        RequestBody::DetachTerminal { terminal } => Some((false, *terminal)),
                        _ => None,
                    };
                    let result = match request.body {
                        RequestBody::Hello { .. } => Err(DaemonError::Protocol("Hello is only valid as the first request".to_owned())),
                        RequestBody::Subscribe { events } => {
                            subscriptions.extend(events);
                            Ok(ResponseBody::Ack)
                        }
                        RequestBody::Unsubscribe => {
                            subscriptions.clear();
                            Ok(ResponseBody::Ack)
                        }
                        body => self.services.dispatch(body).await,
                    };
                    if result.is_ok()
                        && let Some((attach, terminal)) = attachment
                    {
                        if attach { attached.insert(terminal); } else { attached.remove(&terminal); }
                    }
                    send_response(&mut framed, Response { id, result: result.map_err(Into::into) }).await?;
                    if shutdown_requested {
                        self.shutdown.cancel();
                        break;
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(event) if subscriptions.contains(&event_kind(&event)) => send_event(&mut framed, event).await?,
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                frame = frames.recv() => {
                    match frame {
                        Ok(frame)
                            if attached.contains(&frame.terminal)
                                && subscriptions.contains(&EventKind::TerminalFrame) =>
                        {
                            send_event(&mut framed, Event::TerminalFrame(frame)).await?;
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        Ok(())
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
