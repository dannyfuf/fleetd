//! Legacy-peer fixtures for the additive agent protocol.

use super::*;

#[test]
fn a_version_seven_agent_open_still_decodes_without_the_window_fields() {
    let legacy = r#"{"id":9,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":41}}"#;
    let request: Request = serde_json::from_str(legacy).expect("legacy agent open");

    assert!(!request.body.wants_window());
    assert_eq!(request.body.resume_seq(), Some(Seq(41)));
    assert!(matches!(
        request.body,
        RequestBody::AgentThreadOpen {
            after_seq: None,
            turn_limit: None,
            before_cursor: None,
            request_sync_marker: false,
            ..
        }
    ));
}

#[test]
fn an_agent_create_written_before_codex_still_decodes() {
    let legacy = r#"{"id":3,"body":{"type":"agent_thread_create","worktree":"acme/api#native-agents","provider":"claude","model":null,"mode":"ask","resume_cursor":null,"title":null}}"#;
    let request: Request = serde_json::from_str(legacy).expect("pre-Codex agent create");

    assert!(matches!(
        request.body,
        RequestBody::AgentThreadCreate {
            provider: AgentKind::Claude,
            ..
        }
    ));

    // And the value the capability gate exists for decodes on a build that has it.
    let codex = legacy.replace("\"claude\"", "\"codex\"");
    let request: Request = serde_json::from_str(&codex).expect("Codex agent create");
    assert!(matches!(
        request.body,
        RequestBody::AgentThreadCreate {
            provider: AgentKind::Codex,
            ..
        }
    ));
}

#[test]
fn a_seq_event_written_by_the_ndjson_era_build_still_decodes() {
    // The C.5.5 import replays these rows verbatim into `agent_events.payload`, so this fixture
    // is what makes that import provably lossless: no `raw`, defaulted usage, defaulted
    // `files_changed`, and a payload written before those fields had `#[serde(default)]`.
    let legacy = r#"{"seq":7,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_settled","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","outcome":{"type":"completed"}}}}"#;
    let event: SeqEvent = serde_json::from_str(legacy).expect("NDJSON-era event");

    assert_eq!(event.seq, Seq(7));
    assert!(event.raw.is_none());
    assert!(matches!(
        event.event,
        AgentEvent::TurnSettled {
            outcome: TurnOutcome::Completed,
            duration_ms: 0,
            ..
        }
    ));
}

#[test]
fn a_window_response_decodes_without_its_optional_halves() {
    // A daemon mid-rebuild, a fully loaded thread, and a mirror that has not heard from its owner
    // all omit different fields; none of them may need a newer client to read.
    let minimal = r#"{"id":4,"result":{"Ok":{"type":"agent_thread_window","data":{"summary":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"idle"},"session":{"type":"ready"},"turn":{"type":"none"},"lastSeq":0,"lastActivity":null,"exitCode":null},"headSeq":12}}}}"#;
    let response: Response = serde_json::from_str(minimal).expect("minimal window response");

    let Ok(ResponseBody::AgentThreadWindow(window)) = response.result else {
        panic!("expected a window response");
    };
    assert!(window.window.is_empty());
    assert!(window.page.is_none());
    assert!(!window.synchronized);
    assert_eq!(window.projected_seq, Seq(0));
    assert!(window.events_after.is_empty());
    assert!(window.session.capabilities.is_none());
}
