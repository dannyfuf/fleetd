use bytes::BytesMut;
use fleet_core::{
    agents::AttentionKind,
    board::{BoardSummary, BoardView, CardDraft},
    ids::{TerminalId, WorktreeId},
    model::RepoHooks,
    sessions::AgentActivity,
    watches::{WatchChunk, WatchId, WatchStream},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::{BoardChangeReason, Event},
    request::{Request, RequestBody},
    response::{
        DaemonIdentity, HelloResponse, PongResponse, Response, ResponseBody, WorktreeDeleteResult,
    },
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
                client: fleet_proto::request::HelloClient::default(),
            },
        },
        r#"{"id":1,"body":{"type":"hello","protocol":7,"client":{"kind":"app"}}}"#,
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
    assert_frame(
        Request {
            id: 5,
            body: RequestBody::PruneWorktrees {
                dry_run: false,
                fetch: false,
                kill_sessions: false,
                repo: Some("acme/api".parse().unwrap()),
                ids: None,
            },
        },
        r#"{"id":5,"body":{"type":"prune_worktrees","dry_run":false,"fetch":false,"kill_sessions":false,"repo":"acme/api"}}"#,
    );
    assert_frame(
        Request {
            id: 6,
            body: RequestBody::PruneWorktrees {
                dry_run: false,
                fetch: false,
                kill_sessions: false,
                repo: Some("acme/api".parse().unwrap()),
                ids: Some(vec![WorktreeId::try_from("acme/api#reviewed").unwrap()]),
            },
        },
        r#"{"id":6,"body":{"type":"prune_worktrees","dry_run":false,"fetch":false,"kill_sessions":false,"repo":"acme/api","ids":["acme/api#reviewed"]}}"#,
    );
    assert_frame(
        Request {
            id: 7,
            body: RequestBody::EnsureBoard {
                context_id: "work".parse().unwrap(),
            },
        },
        r#"{"id":7,"body":{"type":"ensure_board","context_id":"work"}}"#,
    );
    assert_frame(
        Request {
            id: 8,
            body: RequestBody::CreateCard {
                board_id: "work".parse().unwrap(),
                draft: CardDraft {
                    title: "Fix login".to_owned(),
                    ..CardDraft::default()
                },
            },
        },
        r#"{"id":8,"body":{"type":"create_card","board_id":"work","draft":{"title":"Fix login","description":"","statusId":null,"priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"properties":{}}}}"#,
    );
    assert_frame(
        Request {
            id: 9,
            body: RequestBody::MoveCard {
                card_id: "card-12".parse().unwrap(),
                status_id: "doing".parse().unwrap(),
                index: Some(2),
            },
        },
        r#"{"id":9,"body":{"type":"move_card","card_id":"card-12","status_id":"doing","index":2}}"#,
    );
    assert_frame(
        Request {
            id: 10,
            body: RequestBody::SyncBoard {
                board_id: "work".parse().unwrap(),
                full: true,
            },
        },
        r#"{"id":10,"body":{"type":"sync_board","board_id":"work","full":true}}"#,
    );
    assert_frame(
        Request {
            id: 11,
            body: RequestBody::SetAgentActivity {
                session: "api/feature".parse().unwrap(),
                terminal_id: TerminalId(7),
                activity: AgentActivity::Idle,
                attention: Some(AttentionKind::Permission),
            },
        },
        r#"{"id":11,"body":{"type":"set_agent_activity","session":"api/feature","terminal_id":7,"activity":"idle","attention":"permission"}}"#,
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
        r#"{"id":1,"result":{"Ok":{"type":"hello","data":{"protocol":7,"server":"fleet-test"}}}}"#,
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
    assert_frame(
        Response {
            id: 5,
            result: Ok(ResponseBody::Boards(vec![BoardSummary {
                id: "work".parse().unwrap(),
                context_id: "work".parse().unwrap(),
                name: "Fleet".to_owned(),
                prefix: "FLT".to_owned(),
                backend_kind: "jira".to_owned(),
                card_count: 1,
                open_count: 1,
                dirty_count: 0,
                conflict_count: 0,
                last_synced_at: None,
                last_error: None,
            }])),
        },
        r#"{"id":5,"result":{"Ok":{"type":"boards","data":[{"id":"work","contextId":"work","name":"Fleet","prefix":"FLT","backendKind":"jira","cardCount":1,"openCount":1,"dirtyCount":0,"conflictCount":0,"lastSyncedAt":null,"lastError":null}]}}}"#,
    );
    assert_frame(
        Response {
            id: 6,
            result: Ok(ResponseBody::Board(board_view())),
        },
        r#"{"id":6,"result":{"Ok":{"type":"board","data":{"board":{"id":"work","contextId":"work","name":"Fleet","prefix":"FLT","nextNumber":13,"backend":{"kind":"jira","settings":{"jql":"project = SP","project":"SP"}},"statuses":[{"id":"todo","name":"To do","category":"unstarted","color":null}],"labels":[],"properties":[],"defaultRepoId":null,"settings":{"startOnWorktree":true,"branchTemplate":"{key}-{slug}","conflictPolicy":"manual","pushNewCards":false},"sync":{"lastSyncedAt":null,"cursor":null,"lastError":null,"statusMap":{"remoteToLocal":{},"localToRemote":{}},"readonlyFields":[]},"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"},"cards":[{"id":"card-12","boardId":"work","number":12,"title":"Fix login","description":"","statusId":"todo","priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"worktreeId":null,"properties":{},"comments":[],"activity":[],"remote":null,"conflict":null,"dirty":false,"archived":false,"position":0,"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"}]}}}}"#,
    );
    assert_frame(
        Response {
            id: 7,
            result: Ok(ResponseBody::Card(
                board_view().cards.pop().expect("fixture card"),
            )),
        },
        r#"{"id":7,"result":{"Ok":{"type":"card","data":{"id":"card-12","boardId":"work","number":12,"title":"Fix login","description":"","statusId":"todo","priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"worktreeId":null,"properties":{},"comments":[],"activity":[],"remote":null,"conflict":null,"dirty":false,"archived":false,"position":0,"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"}}}}"#,
    );
}

/// A remote-backed board and the one card the `board` and `card` response goldens pin.
fn board_view() -> BoardView {
    serde_json::from_value(serde_json::json!({
        "board": {
            "id": "work",
            "contextId": "work",
            "name": "Fleet",
            "prefix": "FLT",
            "nextNumber": 13,
            // Sorted keys keep this byte fixture stable with or without preserve_order.
            "backend": {"kind": "jira", "settings": {"jql": "project = SP", "project": "SP"}},
            "statuses": [{"id": "todo", "name": "To do", "category": "unstarted"}],
            "createdAt": "2026-09-06T12:00:00Z",
            "updatedAt": "2026-09-06T12:00:00Z"
        },
        "cards": [{
            "id": "card-12",
            "boardId": "work",
            "number": 12,
            "title": "Fix login",
            "statusId": "todo",
            "createdAt": "2026-09-06T12:00:00Z",
            "updatedAt": "2026-09-06T12:00:00Z"
        }]
    }))
    .expect("board view fixture")
}

#[test]
fn hello_metadata_accepts_old_and_new_ipc_v4_envelopes() {
    let old =
        r#"{"id":1,"result":{"Ok":{"type":"hello","data":{"protocol":6,"server":"fleet-test"}}}}"#;
    let old: HelloResponse = serde_json::from_str(old).expect("old Hello envelope");
    assert!(old.capabilities.is_empty());

    let new_json = r#"{"id":1,"result":{"Ok":{"type":"hello","data":{"protocol":6,"server":"fleet-test"}}},"capabilities":["prune.reviewed_ids"]}"#;
    let new: HelloResponse = serde_json::from_str(new_json).expect("new Hello envelope");
    assert_eq!(new.capabilities, ["prune.reviewed_ids"]);
    let legacy: Response = serde_json::from_str(new_json).expect("legacy Hello decoder");
    assert!(matches!(legacy.result, Ok(ResponseBody::Hello { .. })));
}

#[test]
fn pong_identity_accepts_old_and_new_ipc_v4_envelopes() {
    let old = r#"{"id":2,"result":{"Ok":{"type":"pong"}}}"#;
    let old: PongResponse = serde_json::from_str(old).expect("old Pong envelope");
    assert!(old.daemon.is_none());

    let new = PongResponse {
        response: Response {
            id: 2,
            result: Ok(ResponseBody::Pong),
        },
        daemon: Some(DaemonIdentity {
            pid: 42,
            boot_id: "boot-42".to_owned(),
        }),
    };
    let encoded = serde_json::to_string(&new).expect("new Pong envelope");
    assert_eq!(
        encoded,
        r#"{"id":2,"result":{"Ok":{"type":"pong"}},"daemon":{"pid":42,"bootId":"boot-42"}}"#
    );
    let decoded: PongResponse = serde_json::from_str(&encoded).expect("new Pong decoder");
    assert_eq!(decoded, new);
    let legacy: Response = serde_json::from_str(&encoded).expect("legacy Pong decoder");
    assert_eq!(legacy.result, Ok(ResponseBody::Pong));
}

#[test]
fn event_wire_goldens() {
    assert_frame(
        Event::AgentActivityChanged {
            session: "api/feature".parse().unwrap(),
            terminal_id: TerminalId(7),
            agent: Some("claude".to_owned()),
            activity: AgentActivity::Idle,
            attention: Some(AttentionKind::Finished),
            changed_at: "2026-09-06T12:00:00Z".to_owned(),
        },
        r#"{"type":"agent_activity_changed","data":{"session":"api/feature","terminal_id":7,"agent":"claude","activity":"idle","attention":"finished","changed_at":"2026-09-06T12:00:00Z"}}"#,
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
        Event::BoardChanged {
            board_id: "work".parse().unwrap(),
            reason: BoardChangeReason::CardChanged,
        },
        r#"{"type":"board_changed","data":{"board_id":"work","reason":"card_changed"}}"#,
    );
    assert_frame(
        Event::DaemonShuttingDown,
        r#"{"type":"daemon_shutting_down"}"#,
    );
}

#[test]
fn terminal_attention_fields_default_for_legacy_peers() {
    let request = r#"{"type":"set_agent_activity","session":"api/feature","terminal_id":7,"activity":"idle"}"#;
    let request: RequestBody = serde_json::from_str(request).expect("legacy activity request");
    assert!(matches!(
        request,
        RequestBody::SetAgentActivity {
            attention: None,
            ..
        }
    ));

    let event = r#"{"type":"agent_activity_changed","data":{"session":"api/feature","terminal_id":7,"agent":"claude","activity":"idle","changed_at":"2026-09-06T12:00:00Z"}}"#;
    let event: Event = serde_json::from_str(event).expect("legacy activity event");
    assert!(matches!(
        event,
        Event::AgentActivityChanged {
            attention: None,
            ..
        }
    ));
}
