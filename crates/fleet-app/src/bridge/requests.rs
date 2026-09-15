use super::*;

pub(super) enum Request {
    Command {
        client: Option<Client>,
        body: Box<RequestBody>,
        reply: Option<Sender<Result<ResponseBody, ProtoError>>>,
        /// Released once a reply receiver closes, or transferred after a mutation answer.
        in_flight: InFlight,
    },
    Resynchronize {
        client: Client,
    },
}

struct Mutation {
    client: Option<Client>,
    body: Box<RequestBody>,
    /// Transferred to the settle counter once the daemon answers successfully.
    in_flight: InFlight,
}

/// A single owner enqueues event-backed mutations in arrival order. Response waiters remain
/// independent, preserving the existing request API while a slow daemon operation completes.
pub(super) async fn run(
    requests: Receiver<Request>,
    events: Sender<BridgeEvent>,
    resync_pending: Arc<AtomicBool>,
    settle: Arc<SettleCounter>,
) {
    let (mutations, mutation_rx) = async_channel::bounded(COMMAND_CAPACITY);
    let mutation_events = events.clone();
    let mutation_wake = idle_wake(events.clone());
    let mutation_task = tokio::spawn(async move {
        run_mutations(mutation_rx, mutation_events, settle, mutation_wake).await;
    });
    while let Ok(request) = requests.recv().await {
        match request {
            Request::Command {
                client,
                body,
                reply,
                in_flight,
            } => match reply {
                Some(reply) => dispatch(client, *body, reply, events.clone(), in_flight),
                // Admission never waits on the mutation worker: parking here backs pressure up
                // into the command loop, which also serves shutdown, reconnect and health. A
                // full lane sheds with the same policy `Bridge::send` uses at the outermost
                // hop — flag a resync so the dropped mutation is repaired from a snapshot, and
                // tell the user the write did not land.
                None => match mutations.try_send(Mutation {
                    client,
                    body,
                    in_flight,
                }) {
                    Ok(()) => {}
                    Err(async_channel::TrySendError::Full(_)) => {
                        resync_pending.store(true, Ordering::Release);
                        publish_mutation_failure(
                            &events,
                            "the Fleet daemon bridge queue was saturated",
                        );
                    }
                    Err(async_channel::TrySendError::Closed(_)) => {
                        publish_mutation_failure(&events, "the Fleet daemon bridge is closed");
                    }
                },
            },
            Request::Resynchronize { client } => resynchronize(&client, &events).await,
        }
    }
    drop(mutations);
    if let Err(error) = mutation_task.await {
        tracing::warn!(%error, "mutation request worker stopped");
    }
}

async fn run_mutations(
    mutations: Receiver<Mutation>,
    events: Sender<BridgeEvent>,
    settle: Arc<SettleCounter>,
    wake: IdleWake,
) {
    while let Ok(Mutation {
        client,
        body,
        in_flight,
    }) = mutations.recv().await
    {
        if let Some(client) = client {
            match client.request(*body).await {
                Ok(_) => {
                    let generation = settle.begin();
                    drop(in_flight);
                    let settle = Arc::clone(&settle);
                    let wake = wake.clone();
                    // The bounded grace task must outlive this mutation-worker iteration.
                    let _settle_task = tokio::spawn(expire_settle_after(
                        settle,
                        wake,
                        generation,
                        MUTATION_SETTLE_GRACE,
                    ));
                }
                Err(error) => publish_mutation_failure(&events, &error.message),
            }
        } else {
            publish_mutation_failure(&events, "the Fleet daemon is not connected");
        }
    }
}

pub(super) async fn expire_settle_after(
    settle: Arc<SettleCounter>,
    wake: IdleWake,
    generation: u64,
    grace: Duration,
) {
    tokio::time::sleep(grace).await;
    if settle.expire(generation) {
        wake.wake();
    }
}

pub(super) async fn resynchronize(client: &Client, events: &Sender<BridgeEvent>) {
    if events
        .send(BridgeEvent::EventsLagged { dropped: 1 })
        .await
        .is_err()
    {
        return;
    }
    if let Ok(snapshot) = client.get_snapshot().await {
        let _ignored = events
            .send(BridgeEvent::Daemon(Box::new(Event::SnapshotChanged(
                snapshot,
            ))))
            .await;
    }
}

/// Sends one request on the runtime without blocking the command loop.
fn dispatch(
    client: Option<Client>,
    body: RequestBody,
    reply: Sender<Result<ResponseBody, ProtoError>>,
    events: Sender<BridgeEvent>,
    in_flight: InFlight,
) {
    tokio::spawn(async move {
        let result = match client {
            Some(client) => client.request(body).await,
            None => Err(offline("the Fleet daemon is not connected")),
        };
        if let Ok(ResponseBody::Config(config)) = &result
            && let Err(error) = events
                .send(BridgeEvent::EffectiveConfig(EffectiveConfig::from_config(
                    config,
                )))
                .await
        {
            // The event receiver is gone only during shutdown; the caller still gets its reply.
            tracing::debug!(%error, "bridge event receiver closed during shutdown");
        }
        match reply.try_send(result) {
            Ok(()) | Err(async_channel::TrySendError::Closed(_)) => {}
            Err(async_channel::TrySendError::Full(_)) => {
                tracing::warn!("bridge reply channel was unexpectedly full");
            }
        }
        reply.closed().await;
        drop(in_flight);
    });
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::state::AppState;
    use std::time::Instant;

    struct RejectingDaemon {
        home: tempfile::TempDir,
        sockets: std::sync::mpsc::Receiver<std::os::unix::net::UnixStream>,
        stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl RejectingDaemon {
        fn start(rejection: &str) -> Self {
            use std::{
                io::{Read, Write},
                os::unix::net::UnixListener,
            };

            let home = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
            let listener = UnixListener::bind(FleetHome::new(home.path()).socket_path())
                .unwrap_or_else(|error| panic!("{error}"));
            let (socket_tx, sockets) = std::sync::mpsc::channel();
            let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let thread_stopping = stopping.clone();
            let rejection = rejection.to_owned();
            let thread = std::thread::spawn(move || {
                loop {
                    let (mut stream, _) =
                        listener.accept().unwrap_or_else(|error| panic!("{error}"));
                    if thread_stopping.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    socket_tx
                        .send(stream.try_clone().unwrap_or_else(|error| panic!("{error}")))
                        .unwrap_or_else(|error| panic!("{error}"));
                    loop {
                        let mut length = [0; 4];
                        if stream.read_exact(&mut length).is_err() {
                            break;
                        }
                        let mut body = vec![0; u32::from_be_bytes(length) as usize];
                        stream
                            .read_exact(&mut body)
                            .unwrap_or_else(|error| panic!("{error}"));
                        let request: fleet_proto::request::Request =
                            serde_json::from_slice(&body).unwrap_or_else(|error| panic!("{error}"));
                        let result = match request.body {
                            RequestBody::Hello { .. } => Ok(ResponseBody::Hello {
                                protocol: fleet_proto::PROTOCOL_VERSION,
                                server: "rejecting-test-daemon".to_owned(),
                            }),
                            RequestBody::Subscribe { .. } => Ok(ResponseBody::Ack),
                            RequestBody::SetActiveContext { .. } => {
                                Err(fleet_proto::error::ProtoError {
                                    kind: fleet_proto::error::ErrorKind::Unknown,
                                    message: rejection.clone(),
                                })
                            }
                            _ => Ok(ResponseBody::Ack),
                        };
                        let bytes = serde_json::to_vec(&fleet_proto::response::Response {
                            id: request.id,
                            result,
                        })
                        .unwrap_or_else(|error| panic!("{error}"));
                        if stream
                            .write_all(&(bytes.len() as u32).to_be_bytes())
                            .is_err()
                            || stream.write_all(&bytes).is_err()
                        {
                            break;
                        }
                    }
                }
            });
            Self {
                home,
                sockets,
                stopping,
                thread: Some(thread),
            }
        }
    }

    impl Drop for RejectingDaemon {
        fn drop(&mut self) {
            self.stopping
                .store(true, std::sync::atomic::Ordering::Release);
            while let Ok(socket) = self.sockets.try_recv() {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            let _ = std::os::unix::net::UnixStream::connect(
                FleetHome::new(self.home.path()).socket_path(),
            );
            if let Some(thread) = self.thread.take() {
                thread
                    .join()
                    .unwrap_or_else(|_| panic!("rejecting test daemon panicked"));
            }
        }
    }

    #[tokio::test]
    async fn disconnected_mutation_failure_reaches_app_state() {
        let (request_tx, request_rx) = async_channel::bounded(1);
        let (event_tx, event_rx) = async_channel::bounded(1);
        request_tx
            .send(Request::Command {
                client: None,
                body: Box::new(RequestBody::SetActiveContext { id: None }),
                reply: None,
                in_flight: InFlight::untracked(),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        drop(request_tx);
        run(
            request_rx,
            event_tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(SettleCounter::default()),
        )
        .await;

        let event = event_rx
            .recv()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.apply_bridge_event(event, Instant::now());
        assert_eq!(
            state.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("the Fleet daemon is not connected")
        );
    }

    #[tokio::test]
    async fn mutation_failure_reaches_app_state() {
        let daemon = RejectingDaemon::start("context mutation rejected by daemon");
        let client = Client::connect(daemon.home.path())
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let (request_tx, request_rx) = async_channel::bounded(1);
        let (event_tx, event_rx) = async_channel::bounded(1);
        request_tx
            .send(Request::Command {
                client: Some(client),
                body: Box::new(RequestBody::SetActiveContext { id: None }),
                reply: None,
                in_flight: InFlight::untracked(),
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        drop(request_tx);

        run(
            request_rx,
            event_tx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(SettleCounter::default()),
        )
        .await;

        let event = event_rx
            .recv()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            &event,
            BridgeEvent::MutationFailed { message }
                if message == "context mutation rejected by daemon"
        ));
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_bridge_event(event, now);
        assert_eq!(
            state.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("context mutation rejected by daemon")
        );
    }
}
