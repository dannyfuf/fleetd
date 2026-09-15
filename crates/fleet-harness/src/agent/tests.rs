//! Colocated tests for the scripted agent.
//!
//! Every test drives a whole conversation in memory: the client's half is a pre-baked script of
//! frames, the player's half is asserted frame by frame. Pacing is [`Pace::Instant`], so a
//! transcript that paces at 10 ms costs no wall time and no test can be flaky for it.
//!
//! What these prove is what the contract claims and no more: that the player emits the frames the
//! two research documents describe, in the order they describe. They do not prove a vendor emits
//! them — that is what the opt-in `real-agents` tests in `fleet-daemon` are for.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    Provider, Transcript, TranscriptStep,
    peer::{Pace, Peer},
    transcript::{diff_sides, text_chunks, validate},
};

/// Runs one conversation and returns the frames the player wrote plus its exit status.
pub(super) fn drive(
    provider: Provider,
    transcript: &Transcript,
    client: &[Value],
) -> (Vec<Value>, i32) {
    let mut input = String::new();
    for frame in client {
        input.push_str(&frame.to_string());
        input.push('\n');
    }
    let mut output: Vec<u8> = Vec::new();
    let code = {
        let mut peer = Peer::new(
            std::io::Cursor::new(input.into_bytes()),
            &mut output,
            Pace::Instant,
        );
        let code = super::play(provider, transcript, &mut peer)
            .unwrap_or_else(|error| panic!("play the transcript: {error}"));
        assert!(peer.written() > 0, "the player answered nothing at all");
        code
    };
    let rendered =
        String::from_utf8(output).unwrap_or_else(|error| panic!("the player wrote UTF-8: {error}"));
    let frames = rendered
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("the player wrote JSON: {error}: {line}"))
        })
        .collect();
    (frames, code)
}

/// The directory holding the starter transcripts.
pub(super) fn transcripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("transcripts")
}

/// Loads a starter transcript synchronously, so a test needs no runtime to read a fixture.
pub(super) fn starter(name: &str) -> Transcript {
    let path = transcripts().join(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let transcript: Transcript = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    validate(&transcript).unwrap_or_else(|error| panic!("{} is invalid: {error}", path.display()));
    transcript
}

/// The client frame Fleet writes to start a Claude turn.
pub(super) fn claude_prompt(text: &str) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": text}})
}

/// Every frame of `kind`, by its `type` tag.
pub(super) fn of_type<'a>(frames: &'a [Value], kind: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| frame.get("type").and_then(Value::as_str) == Some(kind))
        .collect()
}

/// Every notification of `method`.
pub(super) fn of_method<'a>(frames: &'a [Value], method: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| frame.get("method").and_then(Value::as_str) == Some(method))
        .collect()
}

/// The concatenation of every Claude `text_delta`.
pub(super) fn claude_streamed_text(frames: &[Value]) -> String {
    frames
        .iter()
        .filter_map(|frame| frame.pointer("/event/delta"))
        .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("text_delta"))
        .filter_map(|delta| delta.get("text").and_then(Value::as_str))
        .collect()
}

// ---------------------------------------------------------------- the document

#[test]
fn a_transcript_from_a_future_version_is_refused_by_naming_both_versions() {
    let transcript = Transcript {
        version: 7,
        steps: Vec::new(),
        session_id: "s".to_owned(),
        thread_id: "t".to_owned(),
        model: "m".to_owned(),
        context_window: 1,
    };
    let error = validate(&transcript)
        .err()
        .unwrap_or_else(|| panic!("version 7 must be refused"))
        .to_string();
    assert!(error.contains('1') && error.contains('7'), "{error}");
}

#[test]
fn an_approval_with_no_file_change_before_it_is_refused_at_load() {
    let transcript = Transcript {
        version: 1,
        steps: vec![
            TranscriptStep::Text {
                text: "hello".to_owned(),
                pace_ms: 0,
            },
            TranscriptStep::Approval {
                id: "gate-1".to_owned(),
                summary: "apply it".to_owned(),
            },
        ],
        ..starter("two-turns.json")
    };
    let error = validate(&transcript)
        .err()
        .unwrap_or_else(|| panic!("an approval with nothing to approve must be refused"))
        .to_string();
    assert!(error.contains("file_change"), "{error}");
}

#[test]
fn two_gates_may_not_share_an_id_and_a_step_may_not_follow_exit() {
    let duplicated = Transcript {
        version: 1,
        steps: vec![
            TranscriptStep::Permission {
                id: "gate-1".to_owned(),
                command: "cargo test".to_owned(),
            },
            TranscriptStep::Permission {
                id: "gate-1".to_owned(),
                command: "cargo build".to_owned(),
            },
        ],
        ..starter("two-turns.json")
    };
    assert!(
        validate(&duplicated).is_err(),
        "a duplicate gate id is ambiguous"
    );

    let unreachable = Transcript {
        version: 1,
        steps: vec![
            TranscriptStep::Exit { code: 3 },
            TranscriptStep::Text {
                text: "never".to_owned(),
                pace_ms: 0,
            },
        ],
        ..starter("two-turns.json")
    };
    assert!(
        validate(&unreachable).is_err(),
        "a step after `exit` can never be played"
    );
}

#[test]
fn all_three_starter_transcripts_load_and_validate() {
    for name in [
        "two-turns.json",
        "edit-approval.json",
        "error-mid-stream.json",
    ] {
        let transcript = starter(name);
        assert_eq!(transcript.version, 1, "{name}");
        assert!(!transcript.steps.is_empty(), "{name}");
    }
}

#[test]
fn streamed_chunks_reproduce_the_message_and_a_diff_splits_into_its_two_sides() {
    let text = "It is a plain binary crate, with no dependencies at all.";
    assert_eq!(text_chunks(text).concat(), text);

    let (before, after) = diff_sides(
        "--- a/README.md\n+++ b/README.md\n@@ -1,2 +1,2 @@\n # demo\n-old line\n+new line\n",
    );
    assert_eq!(before, "# demo\nold line\n");
    assert_eq!(after, "# demo\nnew line\n");
}

// ---------------------------------------------------------------------- Claude

#[test]
fn a_claude_session_greets_with_the_capabilities_the_agent_tab_is_gated_on() {
    let (frames, code) = drive(Provider::Claude, &starter("two-turns.json"), &[]);
    assert_eq!(code, 0);
    let init = frames
        .first()
        .unwrap_or_else(|| panic!("the player greets before it reads anything"));
    assert_eq!(init.get("subtype").and_then(Value::as_str), Some("init"));
    let capabilities = init
        .get("capabilities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("system/init declares capabilities"));
    assert!(
        capabilities.contains(&json!("msg_lifecycle_v1")),
        "Fleet refuses a Claude that does not declare msg_lifecycle_v1: {capabilities:?}"
    );
    assert_eq!(
        init.get("session_id").and_then(Value::as_str),
        Some("6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e"),
        "the durable session id is the resume cursor, and it is the transcript's"
    );
}

#[test]
fn a_clean_claude_conversation_streams_two_turns_and_settles_each_one_once() {
    let transcript = starter("two-turns.json");
    let (frames, _) = drive(
        Provider::Claude,
        &transcript,
        &[
            claude_prompt("what is this crate?"),
            claude_prompt("and its binary?"),
        ],
    );

    let results = of_type(&frames, "result");
    assert_eq!(results.len(), 2, "exactly one result per turn");
    for result in &results {
        assert_eq!(
            result.get("subtype").and_then(Value::as_str),
            Some("success")
        );
        assert_eq!(result.get("is_error").and_then(Value::as_bool), Some(false));
    }

    let streamed = claude_streamed_text(&frames);
    assert!(
        streamed.contains("Let me read the manifest first.")
            && streamed.contains("The binary name is demo"),
        "both turns streamed their prose: {streamed:?}"
    );

    // Snapshots backfill a streamed block; the tool call is a snapshot of its own.
    let tools: Vec<_> = of_type(&frames, "assistant")
        .into_iter()
        .filter_map(|frame| frame.pointer("/message/content/0"))
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .collect();
    assert_eq!(tools.len(), 1, "the transcript called one tool");
    assert_eq!(tools[0].get("name").and_then(Value::as_str), Some("Read"));
}

#[test]
fn a_claude_edit_approval_shows_the_change_and_completes_with_the_transcript_diff() {
    let transcript = starter("edit-approval.json");
    let allow = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "gate-edit-1",
            "response": {"behavior": "allow", "toolUseID": "x"},
        },
    });
    let (frames, _) = drive(
        Provider::Claude,
        &transcript,
        &[claude_prompt("fix the readme"), allow],
    );

    let gate = of_type(&frames, "control_request")
        .into_iter()
        .find(|frame| {
            frame.pointer("/request/subtype").and_then(Value::as_str) == Some("can_use_tool")
        })
        .unwrap_or_else(|| panic!("the edit was gated"));
    assert_eq!(
        gate.get("request_id").and_then(Value::as_str),
        Some("gate-edit-1"),
        "the gate key is the transcript's own id, so the answer correlates"
    );
    assert_eq!(
        gate.pointer("/request/tool_name").and_then(Value::as_str),
        Some("Edit"),
        "the tool name is the discriminator between a permission, a question and a plan"
    );
    let new_string = gate
        .pointer("/request/input/new_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        new_string.contains("A crate with a trailing newline."),
        "the card shows the change, not a prose description of it: {new_string:?}"
    );

    let result = of_type(&frames, "user")
        .into_iter()
        .find_map(|frame| frame.get("tool_use_result").cloned())
        .unwrap_or_else(|| panic!("the allowed edit completed with a structured result"));
    assert_eq!(
        result.get("unified_diff").and_then(Value::as_str),
        Some(
            "--- a/README.md\n+++ b/README.md\n@@ -1,2 +1,3 @@\n # demo\n-A crate with no trailing newline.\n+A crate with a trailing newline.\n+\n"
        ),
        "the completed row carries the transcript's own diff, byte for byte"
    );
    assert_eq!(of_type(&frames, "result").len(), 1);
}

#[test]
fn a_denied_claude_approval_becomes_a_denied_row_rather_than_a_hang() {
    let deny = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "gate-edit-1",
            "response": {"behavior": "deny", "interrupt": false},
        },
    });
    let (frames, _) = drive(
        Provider::Claude,
        &starter("edit-approval.json"),
        &[claude_prompt("fix the readme"), deny],
    );
    let denied = of_type(&frames, "system")
        .into_iter()
        .find(|frame| frame.get("subtype").and_then(Value::as_str) == Some("permission_denied"))
        .unwrap_or_else(|| panic!("a refusal renders as a denied tool row, never as silence"));
    assert_eq!(
        denied.get("tool_name").and_then(Value::as_str),
        Some("Edit")
    );
    // The turn still settles: a declined edit is not a failed turn.
    let results = of_type(&frames, "result");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].get("subtype").and_then(Value::as_str),
        Some("success")
    );
}

#[test]
fn a_claude_command_permission_carries_the_command_and_allows_it() {
    let transcript = Transcript {
        version: 1,
        steps: vec![
            TranscriptStep::Permission {
                id: "gate-run-1".to_owned(),
                command: "cargo test --workspace".to_owned(),
            },
            TranscriptStep::EndTurn { status: None },
        ],
        ..starter("two-turns.json")
    };
    let allow = json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": "gate-run-1", "response": {"behavior": "allow"}},
    });
    let (frames, _) = drive(
        Provider::Claude,
        &transcript,
        &[claude_prompt("run the tests"), allow],
    );
    let gate = of_type(&frames, "control_request")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the command was gated"));
    assert_eq!(
        gate.pointer("/request/input/command")
            .and_then(Value::as_str),
        Some("cargo test --workspace"),
        "the payload is the invocation itself, which is what the card shows literally"
    );
    assert_eq!(
        gate.pointer("/request/tool_name").and_then(Value::as_str),
        Some("Bash")
    );
}

#[test]
fn a_claude_error_mid_stream_settles_the_turn_as_a_failure_that_names_its_cause() {
    let (frames, _) = drive(
        Provider::Claude,
        &starter("error-mid-stream.json"),
        &[claude_prompt("read the manifest")],
    );
    let results = of_type(&frames, "result");
    assert_eq!(
        results.len(),
        1,
        "a failed turn still produces exactly one result"
    );
    let result = results[0];
    assert_eq!(
        result.get("subtype").and_then(Value::as_str),
        Some("error_during_execution")
    );
    assert_eq!(result.get("is_error").and_then(Value::as_bool), Some(true));
    let errors = result
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the result names the cause"));
    assert_eq!(
        errors,
        &vec![json!(
            "Selected model is at capacity. Please try a different model."
        )]
    );
}

#[test]
fn a_claude_interrupt_is_receipted_and_aborts_the_turn_it_arrives_in() {
    let transcript = starter("edit-approval.json");
    let interrupt = json!({
        "type": "control_request",
        "request_id": "int-1",
        "request": {"subtype": "interrupt", "cancel_queued": true},
    });
    let allow = json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": "gate-edit-1", "response": {"behavior": "allow"}},
    });
    // The interrupt arrives while the edit gate is open, which is the only point the player reads.
    let (frames, _) = drive(
        Provider::Claude,
        &transcript,
        &[claude_prompt("fix the readme"), interrupt, allow],
    );
    let receipt = of_type(&frames, "control_response")
        .into_iter()
        .find(|frame| {
            frame
                .pointer("/response/request_id")
                .and_then(Value::as_str)
                == Some("int-1")
        })
        .unwrap_or_else(|| panic!("an interrupt is receipted before the interrupted result"));
    assert_eq!(
        receipt.pointer("/response/subtype").and_then(Value::as_str),
        Some("success")
    );
    let results = of_type(&frames, "result");
    assert_eq!(
        results[0].get("terminal_reason").and_then(Value::as_str),
        Some("aborted_streaming"),
        "the interrupted turn still emits a result, named as an abort"
    );
}

// ----------------------------------------------------------------------- Codex

/// The client frames that open a Codex thread.
fn codex_handshake() -> Vec<Value> {
    vec![
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "fleet"}}}),
        json!({"method": "initialized"}),
        json!({"id": 2, "method": "thread/start", "params": {"cwd": "/tmp"}}),
    ]
}

/// A `turn/start` request carrying one prompt.
fn codex_turn(id: i64, text: &str, client_item: &str) -> Value {
    json!({
        "id": id,
        "method": "turn/start",
        "params": {
            "threadId": "01999c4a-7f00-7000-8000-0000000000a1",
            "input": [{"type": "text", "text": text}],
            "clientUserMessageId": client_item,
        },
    })
}

#[test]
fn a_codex_handshake_resolves_the_home_and_names_the_thread_twice() {
    let (frames, code) = drive(
        Provider::Codex,
        &starter("two-turns.json"),
        &codex_handshake(),
    );
    assert_eq!(code, 0);
    let initialize = frames
        .first()
        .unwrap_or_else(|| panic!("initialize is answered first"));
    assert_eq!(initialize.get("id").and_then(Value::as_i64), Some(1));
    assert!(
        initialize.pointer("/result/codexHome").is_some(),
        "codexHome comes back resolved, so Fleet never guesses the config root"
    );
    assert!(
        initialize.get("jsonrpc").is_none(),
        "the envelope is JSON-RPC-shaped and carries no `jsonrpc` field"
    );
    let started = of_method(&frames, "thread/started");
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0]
            .pointer("/params/thread/id")
            .and_then(Value::as_str),
        Some("01999c4a-7f00-7000-8000-0000000000a1"),
    );
}

#[test]
fn a_clean_codex_conversation_settles_each_turn_with_turn_completed() {
    let transcript = starter("two-turns.json");
    let mut client = codex_handshake();
    client.push(codex_turn(3, "what is this crate?", "client-1"));
    client.push(codex_turn(4, "and its binary?", "client-2"));
    let (frames, _) = drive(Provider::Codex, &transcript, &client);

    let completed = of_method(&frames, "turn/completed");
    assert_eq!(completed.len(), 2, "one settlement per turn");
    for frame in &completed {
        assert_eq!(
            frame.pointer("/params/turn/status").and_then(Value::as_str),
            Some("completed")
        );
        assert_eq!(
            frame
                .pointer("/params/turn/itemsView")
                .and_then(Value::as_str),
            Some("summary"),
            "the settlement's items are a summary; the transcript comes from item/* instead"
        );
    }

    // The user's own message is echoed back, keyed by the id the client sent.
    let echo = of_method(&frames, "item/started")
        .into_iter()
        .find(|frame| {
            frame.pointer("/params/item/type").and_then(Value::as_str) == Some("userMessage")
        })
        .unwrap_or_else(|| panic!("the user message is echoed as an item"));
    assert_eq!(
        echo.pointer("/params/item/clientId")
            .and_then(Value::as_str),
        Some("client-1"),
        "clientId is the reconciliation key for the optimistic bubble"
    );

    let deltas: String = of_method(&frames, "item/agentMessage/delta")
        .into_iter()
        .filter_map(|frame| frame.pointer("/params/delta").and_then(Value::as_str))
        .collect();
    assert!(
        deltas.contains("Let me read the manifest first."),
        "{deltas:?}"
    );

    // A tool call renders from Codex's own parse of the command, never from `command`.
    let command = of_method(&frames, "item/completed")
        .into_iter()
        .find(|frame| {
            frame.pointer("/params/item/type").and_then(Value::as_str) == Some("commandExecution")
        })
        .unwrap_or_else(|| panic!("the tool call became a commandExecution item"));
    assert_eq!(
        command
            .pointer("/params/item/commandActions/0/type")
            .and_then(Value::as_str),
        Some("read"),
        "a Read becomes a `read` action, which is the kind column"
    );
    assert_eq!(
        command
            .pointer("/params/item/status")
            .and_then(Value::as_str),
        Some("completed")
    );
}

#[test]
fn a_codex_edit_approval_is_a_server_request_joined_to_its_item_by_id() {
    let transcript = starter("edit-approval.json");
    let mut client = codex_handshake();
    client.push(codex_turn(3, "fix the readme", "client-1"));
    client.push(json!({"id": "gate-edit-1", "result": {"decision": "accept"}}));
    let (frames, _) = drive(Provider::Codex, &transcript, &client);

    let gate = frames
        .iter()
        .find(|frame| {
            frame.get("method").and_then(Value::as_str) == Some("item/fileChange/requestApproval")
        })
        .unwrap_or_else(|| panic!("the edit was gated by a server request"));
    assert_eq!(gate.get("id").and_then(Value::as_str), Some("gate-edit-1"));
    assert!(
        gate.pointer("/params/diff").is_none() && gate.pointer("/params/changes").is_none(),
        "the approval carries no patch: the card joins to the fileChange item by itemId"
    );
    let item = gate
        .pointer("/params/itemId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let applied = of_method(&frames, "item/completed")
        .into_iter()
        .find(|frame| frame.pointer("/params/item/id").and_then(Value::as_str) == Some(item))
        .unwrap_or_else(|| panic!("the accepted change completed"));
    assert_eq!(
        applied
            .pointer("/params/item/status")
            .and_then(Value::as_str),
        Some("completed")
    );
    assert_eq!(
        applied
            .pointer("/params/item/changes/0/path")
            .and_then(Value::as_str),
        Some("README.md")
    );
    // The turn-level diff arrives for free; Claude has no equivalent.
    assert_eq!(of_method(&frames, "turn/diff/updated").len(), 1);
}

#[test]
fn a_declined_codex_command_completes_as_declined_and_the_turn_still_completes() {
    let transcript = Transcript {
        version: 1,
        steps: vec![
            TranscriptStep::Permission {
                id: "gate-run-1".to_owned(),
                command: "rm -rf target".to_owned(),
            },
            TranscriptStep::EndTurn { status: None },
        ],
        ..starter("two-turns.json")
    };
    let mut client = codex_handshake();
    client.push(codex_turn(3, "clean the build", "client-1"));
    client.push(json!({"id": "gate-run-1", "result": {"decision": "decline"}}));
    let (frames, _) = drive(Provider::Codex, &transcript, &client);

    let gate = frames
        .iter()
        .find(|frame| {
            frame.get("method").and_then(Value::as_str)
                == Some("item/commandExecution/requestApproval")
        })
        .unwrap_or_else(|| panic!("the command was gated"));
    let decisions = gate
        .pointer("/params/availableDecisions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("availableDecisions is authoritative and ordered"));
    assert_eq!(
        decisions,
        &vec![
            json!("accept"),
            json!("acceptForSession"),
            json!("decline"),
            json!("cancel")
        ]
    );

    let completed = of_method(&frames, "item/completed")
        .into_iter()
        .find(|frame| {
            frame.pointer("/params/item/type").and_then(Value::as_str) == Some("commandExecution")
        })
        .unwrap_or_else(|| panic!("the declined command still completed as an item"));
    assert_eq!(
        completed
            .pointer("/params/item/status")
            .and_then(Value::as_str),
        Some("declined"),
        "`declined` is a first-class terminal status; a denied tool is not an error"
    );
    let turn = of_method(&frames, "turn/completed");
    assert_eq!(
        turn[0]
            .pointer("/params/turn/status")
            .and_then(Value::as_str),
        Some("completed"),
        "decline continues the turn; only cancel interrupts it"
    );
}

#[test]
fn a_cancelled_codex_approval_interrupts_the_turn() {
    let transcript = starter("edit-approval.json");
    let mut client = codex_handshake();
    client.push(codex_turn(3, "fix the readme", "client-1"));
    client.push(json!({"id": "gate-edit-1", "result": {"decision": "cancel"}}));
    let (frames, _) = drive(Provider::Codex, &transcript, &client);
    let turn = of_method(&frames, "turn/completed");
    assert_eq!(
        turn[0]
            .pointer("/params/turn/status")
            .and_then(Value::as_str),
        Some("interrupted")
    );
}

#[test]
fn a_codex_error_notification_is_not_terminal_and_turn_completed_still_decides() {
    let transcript = starter("error-mid-stream.json");
    let mut client = codex_handshake();
    client.push(codex_turn(3, "read the manifest", "client-1"));
    let (frames, _) = drive(Provider::Codex, &transcript, &client);

    let error = of_method(&frames, "error")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the failure arrived as an error notification"));
    assert_eq!(
        error.pointer("/params/willRetry").and_then(Value::as_bool),
        Some(false),
        "willRetry is what tells Fleet an error is informational"
    );
    assert_eq!(
        error
            .pointer("/params/error/codexErrorInfo")
            .and_then(Value::as_str),
        Some("serverOverloaded"),
        "the taxonomy is typed, so Fleet can offer a model switch instead of printing a string"
    );
    let turn = of_method(&frames, "turn/completed");
    assert_eq!(turn.len(), 1, "the error did not replace the settlement");
    assert_eq!(
        turn[0]
            .pointer("/params/turn/status")
            .and_then(Value::as_str),
        Some("failed")
    );
}

#[test]
fn a_codex_method_this_build_does_not_implement_is_refused_rather_than_guessed() {
    let mut client = codex_handshake();
    client.push(json!({"id": 9, "method": "account/login/start", "params": {}}));
    let (frames, _) = drive(Provider::Codex, &starter("two-turns.json"), &client);
    let refusal = frames
        .iter()
        .find(|frame| frame.get("id").and_then(Value::as_i64) == Some(9))
        .unwrap_or_else(|| panic!("every request gets an answer"));
    assert_eq!(
        refusal.pointer("/error/code").and_then(Value::as_i64),
        Some(-32601)
    );
}

#[test]
fn the_model_catalogue_a_models_step_declares_is_what_model_list_answers() {
    let mut client = codex_handshake();
    client.push(json!({"id": 5, "method": "model/list", "params": {}}));
    let (frames, _) = drive(Provider::Codex, &starter("two-turns.json"), &client);
    let listed = frames
        .iter()
        .find(|frame| frame.get("id").and_then(Value::as_i64) == Some(5))
        .unwrap_or_else(|| panic!("model/list is answered"));
    assert_eq!(
        listed
            .pointer("/result/models/0/id")
            .and_then(Value::as_str),
        Some("scripted-sonnet")
    );
    assert_eq!(
        listed
            .pointer("/result/models/0/supportedReasoningEfforts/2/reasoningEffort")
            .and_then(Value::as_str),
        Some("high"),
        "the legal effort set is per model and comes from the catalogue, never a hardcoded ladder"
    );
}

// -------------------------------------------------------------------- lifecycle

#[test]
fn an_exit_step_ends_the_process_with_the_status_the_transcript_named() {
    let transcript = Transcript {
        version: 1,
        steps: vec![
            TranscriptStep::Text {
                text: "about to die".to_owned(),
                pace_ms: 0,
            },
            TranscriptStep::Exit { code: 9 },
        ],
        ..starter("two-turns.json")
    };
    let (_, code) = drive(Provider::Claude, &transcript, &[claude_prompt("go")]);
    assert_eq!(code, 9, "the harness-death surface needs the real status");
}

#[test]
fn a_prompt_past_the_end_of_the_transcript_settles_instead_of_hanging() {
    let transcript = starter("error-mid-stream.json");
    let (frames, _) = drive(
        Provider::Claude,
        &transcript,
        &[
            claude_prompt("one"),
            claude_prompt("two"),
            claude_prompt("three"),
        ],
    );
    assert_eq!(
        of_type(&frames, "result").len(),
        3,
        "an exhausted transcript answers every further prompt rather than leaving a turn open"
    );
}

#[test]
fn the_launcher_answers_the_version_probe_and_drops_the_vendor_launch_flags() {
    let directory =
        tempfile::tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
    let transcript = transcripts().join("two-turns.json");
    let script = super::write_launcher(directory.path(), Provider::Codex, &transcript)
        .unwrap_or_else(|error| panic!("write the launcher: {error}"));

    // The probe: `<command> --version`, with the vendor's own argv appended after it.
    let probe = std::process::Command::new(&script)
        .args(["app-server", "--version"])
        .output()
        .unwrap_or_else(|error| panic!("run the launcher: {error}"));
    assert!(probe.status.success(), "the probe exits cleanly");
    assert_eq!(
        String::from_utf8_lossy(&probe.stdout).trim(),
        Provider::Codex.version_line(),
        "the probe parses the first semver-shaped token of this line"
    );

    let body = std::fs::read_to_string(&script)
        .unwrap_or_else(|error| panic!("read the launcher: {error}"));
    assert!(
        body.contains("--provider codex") && body.contains(&transcript.display().to_string()),
        "the launcher execs the player with the transcript it was written for: {body}"
    );
    assert!(
        !body.contains("\"$@\"\nexec") && body.contains("exec '"),
        "the vendor flags are dropped rather than forwarded: {body}"
    );
}
