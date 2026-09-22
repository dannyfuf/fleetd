mod support;

use fleet_core::{
    agents::AttentionKind,
    board::{Board, BoardSummary, BoardView, CardDraft},
    ids::{TerminalId, WorktreeId},
    model::RepoHooks,
    sessions::AgentActivity,
    watches::{WatchChunk, WatchId, WatchStream},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    error::{ErrorKind, ProtoError},
    event::{BoardChangeReason, Event},
    request::{Request, RequestBody},
    response::{
        DaemonIdentity, HelloResponse, PongResponse, Response, ResponseBody, StampedResponse,
        WorktreeDeleteResult,
    },
    snapshot::{DaemonInfo, Snapshot},
};
use support::assert_frame;

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
        r#"{"id":1,"body":{"type":"hello","protocol":8,"client":{"kind":"app"}}}"#,
    );
    assert_frame(
        Request {
            id: 101,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: fleet_proto::request::HelloClient {
                    client_id: Some("11111111-2222-4333-8444-555555555555".to_owned()),
                    capabilities: vec![fleet_proto::AGENT_SEEN_CAPABILITY.to_owned()],
                    ..fleet_proto::request::HelloClient::default()
                },
            },
        },
        r#"{"id":101,"body":{"type":"hello","protocol":8,"client":{"kind":"app","clientId":"11111111-2222-4333-8444-555555555555","capabilities":["agent.seen"]}}}"#,
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
            id: 71,
            body: RequestBody::EnsureWorktreeBoard {
                worktree_id: WorktreeId::try_from("acme/api#feature").unwrap(),
            },
        },
        r#"{"id":71,"body":{"type":"ensure_worktree_board","worktree_id":"acme/api#feature"}}"#,
    );
    assert_frame(
        Request {
            id: 72,
            body: RequestBody::CreateWorktreeBoard {
                worktree_id: WorktreeId::try_from("acme/api#feature").unwrap(),
                name: Some("Feature board".to_owned()),
                prefix: Some("FEA".to_owned()),
                backend: None,
            },
        },
        r#"{"id":72,"body":{"type":"create_worktree_board","worktree_id":"acme/api#feature","name":"Feature board","prefix":"FEA","backend":null}}"#,
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
                cancel_run: false,
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
        r#"{"id":1,"result":{"Ok":{"type":"hello","data":{"protocol":8,"server":"fleet-test"}}}}"#,
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
                worktree_id: None,
                name: "Fleet".to_owned(),
                prefix: "FLT".to_owned(),
                backend_kind: "jira".to_owned(),
                card_count: 1,
                open_count: 1,
                dirty_count: 0,
                conflict_count: 0,
                working_count: 0,
                attention_count: 0,
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
    assert_frame(
        StampedResponse {
            response: Response {
                id: 8,
                result: Ok(ResponseBody::Ack),
            },
            snapshot_revision: Some(42),
        },
        r#"{"id":8,"result":{"Ok":{"type":"ack"}},"snapshotRevision":42}"#,
    );
}

#[test]
fn stamped_hello_and_pong_wire_goldens() {
    assert_frame(
        HelloResponse {
            response: Response {
                id: 9,
                result: Ok(ResponseBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    server: "fleet-test".to_owned(),
                }),
            },
            snapshot_revision: Some(42),
            capabilities: vec!["snapshot.revision".to_owned(), "board.worktree".to_owned()],
            daemon_id: "daemon-test".to_owned(),
            build_commit: None,
        },
        r#"{"id":9,"result":{"Ok":{"type":"hello","data":{"protocol":8,"server":"fleet-test"}}},"snapshotRevision":42,"capabilities":["snapshot.revision","board.worktree"],"daemonId":"daemon-test"}"#,
    );
    assert_frame(
        PongResponse {
            response: Response {
                id: 10,
                result: Ok(ResponseBody::Pong),
            },
            snapshot_revision: Some(43),
            daemon: Some(DaemonIdentity {
                pid: 42,
                boot_id: "boot-42".to_owned(),
            }),
        },
        r#"{"id":10,"result":{"Ok":{"type":"pong"}},"snapshotRevision":43,"daemon":{"pid":42,"bootId":"boot-42"}}"#,
    );
}

#[test]
fn stamped_snapshot_wire_golden() {
    assert_frame(
        Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-15T12:00:00Z".to_owned(),
            revision: Some(42),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: DaemonInfo {
                version: "fleetd test".to_owned(),
                pid: 42,
                started_at: "2026-09-15T11:00:00Z".to_owned(),
                home: "/tmp/fleet".to_owned(),
            },
        },
        r#"{"boards":[],"generatedAt":"2026-09-15T12:00:00Z","revision":42,"contexts":[],"repos":[],"clones":[],"worktrees":[],"activeContext":null,"sessions":[],"agentThreads":[],"statuses":[],"pools":[],"hosts":[],"jobs":[],"daemon":{"version":"fleetd test","pid":42,"startedAt":"2026-09-15T11:00:00Z","home":"/tmp/fleet"}}"#,
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
fn legacy_board_shapes_without_worktree_id_decode_as_unscoped() {
    let board: Board = serde_json::from_value(serde_json::json!({
        "id": "work",
        "contextId": "work",
        "name": "Fleet",
        "prefix": "FLT",
        "nextNumber": 1,
        "statuses": [{"id": "todo", "name": "To do", "category": "unstarted"}],
        "createdAt": "2026-09-06T12:00:00Z",
        "updatedAt": "2026-09-06T12:00:00Z"
    }))
    .expect("legacy board fixture");
    assert!(board.worktree_id.is_none());

    let summary: BoardSummary = serde_json::from_value(serde_json::json!({
        "id": "work",
        "contextId": "work",
        "name": "Fleet",
        "prefix": "FLT",
        "backendKind": "local",
        "cardCount": 0,
        "openCount": 0,
        "dirtyCount": 0,
        "conflictCount": 0,
        "lastSyncedAt": null,
        "lastError": null
    }))
    .expect("legacy board summary fixture");
    assert!(summary.worktree_id.is_none());
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
        snapshot_revision: None,
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
fn snapshot_revision_metadata_defaults_for_legacy_peers() {
    let response = r#"{"id":4,"result":{"Ok":{"type":"ack"}}}"#;
    let response: StampedResponse =
        serde_json::from_str(response).expect("legacy response envelope");
    assert!(response.snapshot_revision.is_none());

    let stamped = r#"{"id":4,"result":{"Ok":{"type":"ack"}},"snapshotRevision":12}"#;
    let legacy: Response = serde_json::from_str(stamped).expect("legacy response decoder");
    assert_eq!(legacy.result, Ok(ResponseBody::Ack));

    let snapshot = r#"{"boards":[],"generatedAt":"2026-09-15T12:00:00Z","contexts":[],"repos":[],"clones":[],"worktrees":[],"activeContext":null,"sessions":[],"agentThreads":[],"statuses":[],"pools":[],"hosts":[],"jobs":[],"daemon":{"version":"fleetd test","pid":42,"startedAt":"2026-09-15T11:00:00Z","home":"/tmp/fleet"}}"#;
    let snapshot: Snapshot = serde_json::from_str(snapshot).expect("legacy snapshot");
    assert!(snapshot.revision.is_none());
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

/// The board view the automation goldens pin: a two-column board whose first column runs a
/// skill, and one card carrying agent preferences, a blocker, a pending run and a finished run.
fn automated_board_view() -> BoardView {
    let mut view = board_view();
    view.board.statuses = serde_json::from_value(serde_json::json!([
        {
            "id": "todo",
            "name": "To do",
            "category": "unstarted",
            "automation": {
                "onEnter": {
                    "kind": {"kind": "skill", "name": "deep-review", "args": "--fast"},
                    "instructions": "Review {key}.",
                    "expect": "the review finds no blocking issue",
                    "agent": {
                        "provider": "claude",
                        "model": "opus",
                        "effort": "high",
                        "mode": "full_access"
                    },
                    "env": ["CARD={key}"]
                },
                "onSuccess": "done",
                "advanceWhenUnblocked": "done"
            }
        },
        {"id": "done", "name": "Done", "category": "completed"}
    ]))
    .expect("automated statuses fixture");
    view.board.settings.max_live_runs = Some(2);
    view.cards = serde_json::from_value(serde_json::json!([{
        "id": "card-12",
        "boardId": "work",
        "number": 12,
        "title": "Fix login",
        "statusId": "todo",
        "comments": [{
            "id": "comment-1",
            "body": "report",
            "createdAt": "2026-09-06T12:00:00Z",
            "runId": "11111111-2222-4333-8444-555555555555"
        }],
        "agent": {"provider": "codex", "model": "gpt-5", "effort": "high"},
        "blockedBy": ["card-11"],
        "pendingRun": {"statusId": "todo", "since": "2026-09-06T12:00:00Z"},
        "runs": [{
            "id": "11111111-2222-4333-8444-555555555555",
            "threadId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "statusId": "todo",
            "action": {"kind": "prompt"},
            "provider": "claude",
            "model": "opus",
            "effort": "high",
            "startedAt": "2026-09-06T12:00:00Z",
            "endedAt": "2026-09-06T12:30:00Z",
            "outcome": "needs_you",
            "detail": "reported blocked",
            "reportCommentId": "comment-1",
            "filesChanged": 3,
            "costUsd": 0.42,
            "tokens": 1200
        }],
        "createdAt": "2026-09-06T12:00:00Z",
        "updatedAt": "2026-09-06T12:00:00Z"
    }]))
    .expect("automated cards fixture");
    view
}

#[test]
fn board_automation_wire_goldens() {
    assert_frame(
        Response {
            id: 7,
            result: Ok(ResponseBody::Card(
                automated_board_view().cards.pop().expect("fixture card"),
            )),
        },
        r#"{"id":7,"result":{"Ok":{"type":"card","data":{"id":"card-12","boardId":"work","number":12,"title":"Fix login","description":"","statusId":"todo","priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"worktreeId":null,"properties":{},"comments":[{"id":"comment-1","author":null,"body":"report","createdAt":"2026-09-06T12:00:00Z","remoteId":null,"runId":"11111111-2222-4333-8444-555555555555"}],"activity":[],"remote":null,"conflict":null,"dirty":false,"archived":false,"position":0,"agent":{"provider":"codex","model":"gpt-5","effort":"high"},"blockedBy":["card-11"],"pendingRun":{"statusId":"todo","since":"2026-09-06T12:00:00Z"},"runs":[{"id":"11111111-2222-4333-8444-555555555555","threadId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","statusId":"todo","action":{"kind":"prompt"},"provider":"claude","model":"opus","effort":"high","startedAt":"2026-09-06T12:00:00Z","endedAt":"2026-09-06T12:30:00Z","outcome":"needs_you","detail":"reported blocked","reportCommentId":"comment-1","filesChanged":3,"costUsd":0.42,"tokens":1200}],"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"}}}}"#,
    );
    assert_frame(
        Response {
            id: 6,
            result: Ok(ResponseBody::Board(automated_board_view())),
        },
        r#"{"id":6,"result":{"Ok":{"type":"board","data":{"board":{"id":"work","contextId":"work","name":"Fleet","prefix":"FLT","nextNumber":13,"backend":{"kind":"jira","settings":{"jql":"project = SP","project":"SP"}},"statuses":[{"id":"todo","name":"To do","category":"unstarted","color":null,"automation":{"onEnter":{"kind":{"kind":"skill","name":"deep-review","args":"--fast"},"instructions":"Review {key}.","expect":"the review finds no blocking issue","agent":{"provider":"claude","model":"opus","effort":"high","mode":"full_access"},"env":["CARD={key}"]},"onSuccess":"done","advanceWhenUnblocked":"done"}},{"id":"done","name":"Done","category":"completed","color":null}],"labels":[],"properties":[],"defaultRepoId":null,"settings":{"startOnWorktree":true,"branchTemplate":"{key}-{slug}","conflictPolicy":"manual","pushNewCards":false,"maxLiveRuns":2},"sync":{"lastSyncedAt":null,"cursor":null,"lastError":null,"statusMap":{"remoteToLocal":{},"localToRemote":{}},"readonlyFields":[]},"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"},"cards":[{"id":"card-12","boardId":"work","number":12,"title":"Fix login","description":"","statusId":"todo","priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"worktreeId":null,"properties":{},"comments":[{"id":"comment-1","author":null,"body":"report","createdAt":"2026-09-06T12:00:00Z","remoteId":null,"runId":"11111111-2222-4333-8444-555555555555"}],"activity":[],"remote":null,"conflict":null,"dirty":false,"archived":false,"position":0,"agent":{"provider":"codex","model":"gpt-5","effort":"high"},"blockedBy":["card-11"],"pendingRun":{"statusId":"todo","since":"2026-09-06T12:00:00Z"},"runs":[{"id":"11111111-2222-4333-8444-555555555555","threadId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","statusId":"todo","action":{"kind":"prompt"},"provider":"claude","model":"opus","effort":"high","startedAt":"2026-09-06T12:00:00Z","endedAt":"2026-09-06T12:30:00Z","outcome":"needs_you","detail":"reported blocked","reportCommentId":"comment-1","filesChanged":3,"costUsd":0.42,"tokens":1200}],"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"}]}}}}"#,
    );
}

/// A board view carrying one live run, joined onto the automated fixture above.
///
/// `liveRuns` is joined from the delegation store on read and skipped when it is empty, so
/// every board golden written before automation existed is unchanged by its arrival.
const BOARD_VIEW_WITH_A_LIVE_RUN: &str = r#"{"id":15,"result":{"Ok":{"type":"board","data":{"board":{"id":"work","contextId":"work","name":"Fleet","prefix":"FLT","nextNumber":13,"backend":{"kind":"jira","settings":{"jql":"project = SP","project":"SP"}},"statuses":[{"id":"todo","name":"To do","category":"unstarted","color":null,"automation":{"onEnter":{"kind":{"kind":"skill","name":"deep-review","args":"--fast"},"instructions":"Review {key}.","expect":"the review finds no blocking issue","agent":{"provider":"claude","model":"opus","effort":"high","mode":"full_access"},"env":["CARD={key}"]},"onSuccess":"done","advanceWhenUnblocked":"done"}},{"id":"done","name":"Done","category":"completed","color":null}],"labels":[],"properties":[],"defaultRepoId":null,"settings":{"startOnWorktree":true,"branchTemplate":"{key}-{slug}","conflictPolicy":"manual","pushNewCards":false,"maxLiveRuns":2},"sync":{"lastSyncedAt":null,"cursor":null,"lastError":null,"statusMap":{"remoteToLocal":{},"localToRemote":{}},"readonlyFields":[]},"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"},"cards":[{"id":"card-12","boardId":"work","number":12,"title":"Fix login","description":"","statusId":"todo","priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"worktreeId":null,"properties":{},"comments":[{"id":"comment-1","author":null,"body":"report","createdAt":"2026-09-06T12:00:00Z","remoteId":null,"runId":"11111111-2222-4333-8444-555555555555"}],"activity":[],"remote":null,"conflict":null,"dirty":false,"archived":false,"position":0,"agent":{"provider":"codex","model":"gpt-5","effort":"high"},"blockedBy":["card-11"],"pendingRun":{"statusId":"todo","since":"2026-09-06T12:00:00Z"},"runs":[{"id":"11111111-2222-4333-8444-555555555555","threadId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","statusId":"todo","action":{"kind":"prompt"},"provider":"claude","model":"opus","effort":"high","startedAt":"2026-09-06T12:00:00Z","endedAt":"2026-09-06T12:30:00Z","outcome":"needs_you","detail":"reported blocked","reportCommentId":"comment-1","filesChanged":3,"costUsd":0.42,"tokens":1200}],"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"}],"liveRuns":[{"cardId":"card-12","run":"dddddddd-2222-4333-8444-555555555555","status":"running","headline":"Reading the login handler","started":"2026-09-06T12:05:00Z"}]}}}}"#;

/// The three run verbs, the flag that rides on `MoveCard`, and a board view carrying a live run.
///
/// `cancel_run` is skipped when it is false, so the `MoveCard` golden above — written before the
/// flag existed — is byte for byte what it always was, and only a move that really cancels a run
/// puts the key on the wire.
#[test]
fn board_automation_request_and_live_run_wire_goldens() {
    assert_frame(
        Request {
            id: 11,
            body: RequestBody::CardRunStart {
                card_id: "card-12".parse().unwrap(),
            },
        },
        r#"{"id":11,"body":{"type":"card_run_start","card_id":"card-12"}}"#,
    );
    assert_frame(
        Request {
            id: 12,
            body: RequestBody::CardRunCancel {
                card_id: "card-12".parse().unwrap(),
            },
        },
        r#"{"id":12,"body":{"type":"card_run_cancel","card_id":"card-12"}}"#,
    );
    assert_frame(
        Request {
            id: 13,
            body: RequestBody::CardRunWait {
                card_id: "card-12".parse().unwrap(),
                timeout_ms: 30_000,
            },
        },
        r#"{"id":13,"body":{"type":"card_run_wait","card_id":"card-12","timeout_ms":30000}}"#,
    );
    assert_frame(
        Request {
            id: 14,
            body: RequestBody::MoveCard {
                card_id: "card-12".parse().unwrap(),
                status_id: "doing".parse().unwrap(),
                index: None,
                cancel_run: true,
            },
        },
        r#"{"id":14,"body":{"type":"move_card","card_id":"card-12","status_id":"doing","index":null,"cancel_run":true}}"#,
    );

    let mut view = automated_board_view();
    view.live_runs = serde_json::from_value(serde_json::json!([{
        "cardId": "card-12",
        "run": "dddddddd-2222-4333-8444-555555555555",
        "status": "running",
        "headline": "Reading the login handler",
        "started": "2026-09-06T12:05:00Z"
    }]))
    .expect("live runs fixture");
    assert_frame(
        Response {
            id: 15,
            result: Ok(ResponseBody::Board(view)),
        },
        BOARD_VIEW_WITH_A_LIVE_RUN,
    );
}

#[test]
fn a_card_without_automation_fields_encodes_as_it_did_before() {
    // Every automation field a card gained is skipped when it is unset, so a board nobody
    // automated puts the same bytes on the wire as the build before this feature. A diff here
    // is a missing `skip_serializing_if` on the new field, never a fixture to refresh.
    assert_frame(
        board_view().cards.pop().expect("fixture card"),
        r#"{"id":"card-12","boardId":"work","number":12,"title":"Fix login","description":"","statusId":"todo","priority":"none","labels":[],"assignee":null,"estimate":null,"dueDate":null,"parentId":null,"repoId":null,"worktreeId":null,"properties":{},"comments":[],"activity":[],"remote":null,"conflict":null,"dirty":false,"archived":false,"position":0,"createdAt":"2026-09-06T12:00:00Z","updatedAt":"2026-09-06T12:00:00Z"}"#,
    );
}

#[test]
fn the_board_automation_capability_name_is_fixed_before_any_daemon_serves_it() {
    // The string is what a peer negotiates on, so it is pinned here the moment it exists. The
    // daemon's advertised list is written out literally in its own test and does not name it:
    // this build knows the word and not the verbs, and Rule 7 says never to claim one it
    // cannot serve. Phase 3 adds it to that list in the same commit as the requests.
    assert_eq!(
        fleet_proto::response::BOARD_AUTOMATION_CAPABILITY,
        "board.automation"
    );
}
