//! Watch request contract, terminal cleanup, and socket-owner interruption.

use fleet_core::{
    config::Agent,
    ids::TerminalId,
    watches::{WatchId, WatchStatus, WatchStream},
};
use fleet_daemon::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::{BroadcastBus, connection::Connection},
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::{
    codec::FleetCodec,
    event::Event,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::net::UnixStream;
use tokio_util::{codec::Framed, sync::CancellationToken};

async fn services(home: &Path) -> Arc<Services> {
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(home, files.clone()));
    let mut effective = config.load().await.unwrap();
    effective.agent_commands.claude = "/bin/sleep 30".into();
    config.save(effective).await.unwrap();
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
fn start(terminal: TerminalId) -> RequestBody {
    RequestBody::StartWatch {
        terminal,
        label: "test".into(),
        command: vec!["sh".into()],
        cwd: None,
        pid: Some(123),
    }
}
async fn start_id(services: &Services, terminal: TerminalId) -> WatchId {
    let ResponseBody::WatchStarted(id) = services.dispatch(start(terminal)).await.unwrap() else {
        panic!("expected watch id");
    };
    id
}

#[tokio::test]
async fn request_lifecycle_and_terminal_session_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path()).await;
    assert!(services.dispatch(start(TerminalId(999))).await.is_err());
    let session = services
        .sessions
        .ensure(None, Some(Agent::Claude), false)
        .await
        .unwrap();
    let terminal = session.terminals[0].id;
    let id = start_id(&services, terminal).await;
    for (stream, text) in [
        (WatchStream::Stdout, "out\n"),
        (WatchStream::Stderr, "err\n"),
    ] {
        assert_eq!(
            services
                .dispatch(RequestBody::AppendWatchOutput {
                    watch: id,
                    stream,
                    text: text.into()
                })
                .await
                .unwrap(),
            ResponseBody::Ack
        );
    }
    let ResponseBody::WatchTail(tail) = services
        .dispatch(RequestBody::TailWatch {
            watch: id,
            from_seq: Some(1),
        })
        .await
        .unwrap()
    else {
        panic!("expected tail");
    };
    assert_eq!(tail.watch.session, session.id);
    assert_eq!(tail.chunks.len(), 1);
    assert_eq!(tail.chunks[0].text, "err\n");
    assert_eq!((tail.first_retained_seq, tail.next_seq), (0, 2));
    assert!(
        services
            .dispatch(RequestBody::DismissWatch { watch: id })
            .await
            .is_err()
    );
    services
        .dispatch(RequestBody::FinishWatch {
            watch: id,
            code: Some(3),
            signal: None,
        })
        .await
        .unwrap();
    let ResponseBody::Watches(watches) = services
        .dispatch(RequestBody::ListWatches {
            session: session.id.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("expected list");
    };
    assert_eq!(
        watches[0].status,
        WatchStatus::Exited {
            code: Some(3),
            signal: None
        }
    );
    services
        .dispatch(RequestBody::DismissWatch { watch: id })
        .await
        .unwrap();
    assert!(services.watches.tail(id, None).is_err());
    let id = start_id(&services, terminal).await;
    services
        .dispatch(RequestBody::CloseTerminal { terminal })
        .await
        .unwrap();
    assert!(services.watches.tail(id, None).is_err());
    let session = services
        .sessions
        .ensure(None, Some(Agent::Claude), false)
        .await
        .unwrap();
    let id = start_id(&services, session.terminals[0].id).await;
    services
        .dispatch(RequestBody::KillSession {
            session: session.id,
        })
        .await
        .unwrap();
    assert!(services.watches.tail(id, None).is_err());
}

#[tokio::test]
async fn dropping_starter_socket_interrupts_running_watch_and_flushes_output() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path()).await;
    let session = services
        .sessions
        .ensure(None, Some(Agent::Claude), false)
        .await
        .unwrap();
    let (server, client) = UnixStream::pair().unwrap();
    let connection = Connection::new(
        server,
        services.clone(),
        services.events.clone(),
        CancellationToken::new(),
    );
    let actor = tokio::spawn(connection.run());
    let mut client = Framed::new(client, FleetCodec::<Request, Response>::new());
    client
        .send(Request {
            id: 1,
            body: RequestBody::Hello {
                protocol: 1,
                client: "watch-test".into(),
            },
        })
        .await
        .unwrap();
    client.next().await.unwrap().unwrap().result.unwrap();
    client
        .send(Request {
            id: 2,
            body: start(session.terminals[0].id),
        })
        .await
        .unwrap();
    let ResponseBody::WatchStarted(id) = client.next().await.unwrap().unwrap().result.unwrap()
    else {
        panic!("expected watch id");
    };
    // An unrelated dispatcher/connection must not mutate an ID it does not own.
    assert!(
        services
            .dispatch(RequestBody::AppendWatchOutput {
                watch: id,
                stream: WatchStream::Stdout,
                text: "foreign".into()
            })
            .await
            .is_err()
    );
    assert!(
        services
            .dispatch(RequestBody::FinishWatch {
                watch: id,
                code: Some(0),
                signal: None
            })
            .await
            .is_err()
    );
    let mut events = services.events.subscribe();
    client
        .send(Request {
            id: 3,
            body: RequestBody::AppendWatchOutput {
                watch: id,
                stream: WatchStream::Stdout,
                text: "partial".into(),
            },
        })
        .await
        .unwrap();
    client.next().await.unwrap().unwrap().result.unwrap();
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), actor)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        services.watches.tail(id, None).unwrap().watch.status,
        WatchStatus::Exited {
            code: None,
            signal: Some(9)
        }
    );
    assert!(matches!(
        events.try_recv().unwrap(),
        Event::WatchOutput { .. }
    ));
    assert!(matches!(events.try_recv().unwrap(), Event::WatchExited(_)));
    services.sessions.kill(session.id).await.unwrap();
}
