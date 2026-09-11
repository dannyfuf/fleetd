//! Byte-shape tests for every request variant: round trips, the snake_case field contract, and
//! the legacy payloads a mixed-version fleet still has to decode.
//!
//! Split out of `request.rs` so the wire definitions stay readable on their own; the byte-exact
//! goldens live in `tests/compatibility.rs` and `tests/agent_compatibility.rs`.

use super::*;
use crate::{
    assert_round_trip,
    terminal::{Key, KeyAction, Modifiers},
};

#[test]
fn request_bodies_round_trip() {
    let repo = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
    let job = JobId::try_from("job-1").unwrap_or_else(|error| panic!("{error}"));
    let thread = ThreadId::new();
    let model = ModelSelection {
        model: "claude-sonnet-5".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    };
    let bodies = vec![
        RequestBody::Hello {
            protocol: crate::PROTOCOL_VERSION,
            client: HelloClient {
                kind: ClientKind::Proxy,
                host_id: Some(HostId::try_from("local-daemon").expect("host")),
                capabilities: vec![crate::AGENT_WINDOW_CAPABILITY.to_owned()],
            },
        },
        RequestBody::BootstrapHost {
            host: HostId::try_from("dev-box").expect("host"),
            git_ref: Some("fix/remote-agents".to_owned()),
        },
        RequestBody::DoctorHost {
            host: HostId::try_from("dev-box").expect("host"),
        },
        RequestBody::AgentThreadList,
        RequestBody::AgentThreadCreate {
            worktree: WorktreeId::try_from("acme/api#native-agents")
                .unwrap_or_else(|error| panic!("{error}")),
            provider: AgentKind::Claude,
            model: Some(model.clone()),
            mode: PermissionMode::Ask,
            resume_cursor: Some("session-1".to_owned()),
            title: Some("native agents".to_owned()),
        },
        RequestBody::AgentThreadOpen {
            thread,
            from_seq: Some(Seq(41)),
            after_seq: None,
            turn_limit: None,
            before_cursor: None,
            request_sync_marker: false,
        },
        RequestBody::AgentThreadOpen {
            thread,
            from_seq: None,
            after_seq: Some(Seq(41)),
            turn_limit: Some(10),
            before_cursor: Some("fat.1.0000".to_owned()),
            request_sync_marker: true,
        },
        RequestBody::AgentItemBody {
            thread,
            item: fleet_core::agents::ItemId::new(),
            stream: StreamKind::CommandOutput,
            offset: 262_144,
            limit: 262_144,
        },
        RequestBody::AgentThreadClose { thread },
        RequestBody::AgentSend {
            thread,
            input: UserInput {
                text: "inspect the failing test".to_owned(),
                attachments: Vec::new(),
                item: None,
            },
        },
        RequestBody::AgentInterrupt { thread },
        RequestBody::AgentRespond {
            thread,
            gate: GateId::new(),
            answer: GateAnswer::Question {
                answers: vec![vec!["SQLite".to_owned()]],
            },
        },
        RequestBody::AgentSetMode {
            thread,
            mode: PermissionMode::Plan,
        },
        RequestBody::AgentSetModel { thread, model },
        RequestBody::AgentMarkSeen {
            thread,
            seq: Seq(42),
        },
        RequestBody::AgentStop { thread },
        RequestBody::AttachTerminal {
            terminal: TerminalId(4),
            cols: 120,
            rows: 40,
        },
        RequestBody::SetConfig {
            patch: serde_json::json!({"agent":"opencode"}),
        },
        RequestBody::ScrollOrKeyTerminal {
            terminal: TerminalId(8),
            scroll: ScrollCommand::Pages(-1),
            key: KeyEvent {
                key: Key::PageUp,
                mods: Modifiers::SHIFT,
                text: None,
                action: KeyAction::Press,
            },
        },
        RequestBody::WheelTerminal {
            terminal: TerminalId(8),
            wheel: WheelEvent {
                steps: -3,
                col: 12,
                row: 8,
                mods: Modifiers::SHIFT | Modifiers::SUPER,
            },
        },
        RequestBody::ListBaseRefs {
            repo: repo.clone(),
            force: true,
        },
        RequestBody::CreateWorktreeFromPr {
            repo: repo.clone(),
            number: 42,
            host: Some(HostId::try_from("dev-box").expect("host")),
        },
        RequestBody::SetRepoHooks {
            repo: repo.clone(),
            hooks: RepoHooks::default(),
        },
        RequestBody::DismissClone { repo: repo.clone() },
        RequestBody::RestoreTrash {
            entry: "123-api".to_owned(),
        },
        RequestBody::RefreshStatuses { repo: Some(repo) },
        RequestBody::SetAgentActivity {
            session: SessionId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            terminal_id: TerminalId(8),
            activity: AgentActivity::Idle,
            attention: Some(AttentionKind::Finished),
        },
        RequestBody::RestartTerminal {
            terminal: TerminalId(8),
        },
        RequestBody::RetryJob { job },
        RequestBody::MatchKeepAliveRules,
        RequestBody::ImportFromSwarm,
    ];
    for body in bodies {
        assert_round_trip(body);
    }
}

#[test]
fn every_board_request_uses_contracted_snake_case_names() {
    let requests = [
        serde_json::json!({"type":"list_boards","context_id":null}),
        serde_json::json!({"type":"get_board","board_id":"work"}),
        serde_json::json!({"type":"ensure_board","context_id":"work"}),
        serde_json::json!({"type":"create_board","context_id":"work","name":null,"prefix":null,"backend":null}),
        serde_json::json!({"type":"update_board","board_id":"work","patch":{}}),
        serde_json::json!({"type":"delete_board","board_id":"work"}),
        serde_json::json!({"type":"create_card","board_id":"work","draft":{"title":"Task"}}),
        serde_json::json!({"type":"update_card","card_id":"card-1","patch":{"assignee":null}}),
        serde_json::json!({"type":"move_card","card_id":"card-1","status_id":"todo","index":1}),
        serde_json::json!({"type":"delete_card","card_id":"card-1"}),
        serde_json::json!({"type":"add_card_comment","card_id":"card-1","body":"Hello"}),
        serde_json::json!({"type":"create_worktree_from_card","card_id":"card-1","repo_id":null,"base":null,"host":null}),
        serde_json::json!({"type":"sync_board","board_id":"work"}),
        serde_json::json!({"type":"resolve_card_conflict","card_id":"card-1","resolution":"take_remote"}),
        serde_json::json!({"type":"describe_board_backend","board_id":"work"}),
    ];
    for json in requests {
        let request: RequestBody =
            serde_json::from_value(json.clone()).unwrap_or_else(|error| panic!("{error}"));
        let encoded = serde_json::to_value(&request).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(encoded["type"], json["type"]);
        let fields = encoded.as_object().expect("tagged request object");
        for (key, value) in json.as_object().expect("fixture object") {
            // A patch and a draft carry their own camelCase contract; only the variant's
            // own fields are snake_case.
            if key != "patch" && key != "draft" {
                assert_eq!(&encoded[key], value, "wire field {key}");
            }
        }
        assert!(!fields.keys().any(|key| key.contains(char::is_uppercase)));
        assert_round_trip(request);
    }
}

#[test]
fn a_version_seven_open_with_no_window_fields_still_decodes() {
    // The whole additive contract in one assertion: a payload written by a peer that has
    // never heard of windowing decodes, asks for no window, and resumes from `from_seq`.
    let request: Request = serde_json::from_str(
        r#"{"id":7,"body":{"type":"agent_thread_open","thread":"0f1b1f2a-0000-4000-8000-000000000001","from_seq":41}}"#,
    )
    .expect("legacy agent open");
    assert!(!request.body.wants_window());
    assert_eq!(request.body.resume_seq(), Some(Seq(41)));
}

#[test]
fn a_windowed_open_is_recognised_by_any_one_of_its_four_fields() {
    let thread = ThreadId::new();
    let open =
        |after_seq, turn_limit, before_cursor: Option<&str>, marker| RequestBody::AgentThreadOpen {
            thread,
            from_seq: None,
            after_seq,
            turn_limit,
            before_cursor: before_cursor.map(str::to_owned),
            request_sync_marker: marker,
        };
    assert!(!open(None, None, None, false).wants_window());
    assert!(open(Some(Seq(1)), None, None, false).wants_window());
    assert!(open(None, Some(10), None, false).wants_window());
    assert!(open(None, None, Some("fat.1.0.1"), false).wants_window());
    assert!(open(None, None, None, true).wants_window());
    // `after_seq` is the newer name and wins, so the daemon reads one cursor and not two.
    assert_eq!(
        RequestBody::AgentThreadOpen {
            thread,
            from_seq: Some(Seq(2)),
            after_seq: Some(Seq(9)),
            turn_limit: None,
            before_cursor: None,
            request_sync_marker: false,
        }
        .resume_seq(),
        Some(Seq(9))
    );
    assert_eq!(RequestBody::AgentThreadList.resume_seq(), None);
    assert!(!RequestBody::AgentThreadList.wants_window());
}

#[test]
fn the_seven_agent_mutations_serialize_per_thread_and_the_reads_do_not() {
    let thread = ThreadId::new();
    let serialized = [
        RequestBody::AgentSend {
            thread,
            input: UserInput {
                text: "go".to_owned(),
                attachments: Vec::new(),
                item: None,
            },
        },
        RequestBody::AgentRespond {
            thread,
            gate: GateId::new(),
            answer: GateAnswer::Plan(fleet_core::agents::PlanAnswer::Approve),
        },
        RequestBody::AgentInterrupt { thread },
        RequestBody::AgentSetMode {
            thread,
            mode: PermissionMode::Plan,
        },
        RequestBody::AgentSetModel {
            thread,
            model: ModelSelection {
                model: "claude-sonnet-5".to_owned(),
                effort: None,
                provider: None,
            },
        },
        RequestBody::AgentStop { thread },
        RequestBody::AgentRevert {
            thread,
            checkpoint: crate::agents::CheckpointId::from_parts(
                1,
                crate::agents::CheckpointScope::Turn,
                fleet_core::agents::TurnId::new(),
            ),
        },
    ];
    for body in serialized {
        assert_eq!(
            agent_request_is_serialized(&body),
            Some(thread),
            "{body:?} must not interleave with another mutation on its thread"
        );
    }

    let concurrent = [
        RequestBody::AgentThreadList,
        RequestBody::AgentThreadOpen {
            thread,
            from_seq: None,
            after_seq: None,
            turn_limit: Some(10),
            before_cursor: None,
            request_sync_marker: false,
        },
        RequestBody::AgentItemBody {
            thread,
            item: fleet_core::agents::ItemId::new(),
            stream: StreamKind::CommandOutput,
            offset: 0,
            limit: 1,
        },
        RequestBody::AgentMarkSeen {
            thread,
            seq: Seq(1),
        },
        RequestBody::AgentCheckpoints { thread },
        RequestBody::DaemonPing,
    ];
    for body in concurrent {
        assert_eq!(
            agent_request_is_serialized(&body),
            None,
            "{body:?} is a read and must not queue behind a slow mutation"
        );
    }
}

#[test]
fn hello_accepts_the_protocol_v6_string_client_wire_shape() {
    let request: Request =
        serde_json::from_str(r#"{"id":1,"body":{"type":"hello","protocol":6,"client":"fleet"}}"#)
            .expect("legacy Hello request");
    let RequestBody::Hello { protocol, client } = request.body else {
        panic!("a hello request");
    };
    assert_eq!(protocol, 6);
    assert_eq!(client.kind, ClientKind::App);
    assert_eq!(client.host_id, None);
    // A peer that named nothing supports nothing optional, which is what withholds the agent
    // stream-control events from it rather than killing its frames.
    assert!(client.capabilities.is_empty());
    assert!(!client.supports(crate::AGENT_RESYNC_CAPABILITY));
}

/// A peer that names capabilities round-trips them, and the metadata shape stays camelCase.
#[test]
fn hello_carries_the_capabilities_a_client_can_decode() {
    let request: Request = serde_json::from_str(
        r#"{"id":1,"body":{"type":"hello","protocol":7,"client":{"kind":"app","capabilities":["agent.resync"]}}}"#,
    )
    .expect("a capability-carrying Hello");
    let RequestBody::Hello { client, .. } = request.body else {
        panic!("a hello request");
    };
    assert!(client.supports(crate::AGENT_RESYNC_CAPABILITY));
    assert!(!client.supports(crate::AGENT_WINDOW_CAPABILITY));
}
