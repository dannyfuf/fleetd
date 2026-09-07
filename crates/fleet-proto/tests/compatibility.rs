use bytes::BytesMut;
use fleet_core::{
    ids::TerminalId,
    model::RepoHooks,
    sessions::AgentActivity,
    watches::{WatchChunk, WatchId, WatchStream},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::Event,
    request::{Request, RequestBody},
    response::{Response, ResponseBody, WorktreeDeleteResult},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio_util::codec::{Decoder, Encoder};

fn assert_frame<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(
    message: T,
    golden: &str,
) {
    let mut codec = FleetCodec::<&T, T>::new();
    let mut frame = BytesMut::new();
    codec.encode(&message, &mut frame).unwrap();
    assert_eq!(&frame[..4], &(golden.len() as u32).to_be_bytes());
    assert_eq!(&frame[4..], golden.as_bytes());

    // Decode the fixed fixture independently of the encoder's output.
    let mut fixture = BytesMut::new();
    fixture.extend_from_slice(&(golden.len() as u32).to_be_bytes());
    fixture.extend_from_slice(golden.as_bytes());
    assert_eq!(codec.decode(&mut fixture).unwrap(), Some(message));
    assert!(fixture.is_empty());
}

#[test]
fn request_wire_goldens() {
    assert_frame(
        Request {
            id: 1,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: "fleet-test".to_owned(),
            },
        },
        r#"{"id":1,"body":{"type":"hello","protocol":4,"client":"fleet-test"}}"#,
    );
    assert_frame(
        Request {
            id: 2,
            body: RequestBody::CreateWorktree {
                repo: "acme/api".parse().unwrap(),
                slug: "feature".to_owned(),
                branch: None,
                base: Some("origin/main".to_owned()),
                host: None,
                hooks: RepoHooks::default(),
            },
        },
        r#"{"id":2,"body":{"type":"create_worktree","repo":"acme/api","slug":"feature","branch":null,"base":"origin/main","host":null,"hooks":{"prepare":[],"postCreate":[]}}}"#,
    );
    assert_frame(
        Request {
            id: 3,
            body: RequestBody::CloneRepo {
                owner: "acme".to_owned(),
                name: "api".to_owned(),
                url: "git@github.com:acme/api.git".to_owned(),
                context: "work".parse().unwrap(),
                default_branch: None,
            },
        },
        r#"{"id":3,"body":{"type":"clone_repo","owner":"acme","name":"api","url":"git@github.com:acme/api.git","context":"work"}}"#,
    );
    assert_frame(
        Request {
            id: 4,
            body: RequestBody::SetConfig {
                // Sorted keys keep this byte fixture stable with or without preserve_order.
                patch: serde_json::json!({"sleep":{"keepAlive":[{"id":"server","kind":"listening-port","label":"server"}]}}),
            },
        },
        r#"{"id":4,"body":{"type":"set_config","patch":{"sleep":{"keepAlive":[{"id":"server","kind":"listening-port","label":"server"}]}}}}"#,
    );
}

#[test]
fn response_wire_goldens() {
    assert_frame(
        Response {
            id: 1,
            result: Ok(ResponseBody::Hello {
                protocol: PROTOCOL_VERSION,
                server: "fleet-test".to_owned(),
            }),
        },
        r#"{"id":1,"result":{"Ok":{"type":"hello","data":{"protocol":4,"server":"fleet-test"}}}}"#,
    );
    assert_frame(
        Response {
            id: 2,
            result: Err(ProtoError {
                kind: ErrorKind::Tmux,
                message: "terminal unavailable".to_owned(),
            }),
        },
        r#"{"id":2,"result":{"Err":{"kind":"tmux","message":"terminal unavailable"}}}"#,
    );
    assert_frame(
        Response {
            id: 3,
            result: Ok(ResponseBody::WorktreesDeleted(vec![WorktreeDeleteResult {
                worktree_id: "acme/api#feature".parse().unwrap(),
                ok: true,
                reason: None,
                trash_entry: Some("123-api".to_owned()),
            }])),
        },
        r#"{"id":3,"result":{"Ok":{"type":"worktrees_deleted","data":[{"worktreeId":"acme/api#feature","ok":true,"trashEntry":"123-api"}]}}}"#,
    );
    assert_frame(
        Response {
            id: 4,
            result: Ok(ResponseBody::Ack),
        },
        r#"{"id":4,"result":{"Ok":{"type":"ack"}}}"#,
    );
}

#[test]
fn event_wire_goldens() {
    assert_frame(
        Event::AgentActivityChanged {
            session: "api/feature".parse().unwrap(),
            terminal_id: TerminalId(7),
            agent: Some("claude".to_owned()),
            activity: AgentActivity::Idle,
            changed_at: "2026-09-06T12:00:00Z".to_owned(),
        },
        r#"{"type":"agent_activity_changed","data":{"session":"api/feature","terminal_id":7,"agent":"claude","activity":"idle","changed_at":"2026-09-06T12:00:00Z"}}"#,
    );
    assert_frame(
        Event::WatchOutput {
            watch: WatchId(8),
            chunks: vec![WatchChunk {
                seq: 9,
                stream: WatchStream::Stderr,
                text: "λ\n".to_owned(),
            }],
        },
        r#"{"type":"watch_output","data":{"watch":8,"chunks":[{"seq":9,"stream":"stderr","text":"λ\n"}]}}"#,
    );
    assert_frame(
        Event::TerminalExited {
            terminal: TerminalId(7),
            code: None,
        },
        r#"{"type":"terminal_exited","data":{"terminal":7,"code":null}}"#,
    );
    assert_frame(
        Event::DaemonShuttingDown,
        r#"{"type":"daemon_shutting_down"}"#,
    );
}
