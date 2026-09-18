//! Peer tests for the Claude adapter.

use super::*;

/// The mock peer: a real child that greets with `system/init` and answers one turn.
#[tokio::test]
async fn a_scripted_peer_drives_init_a_turn_and_its_result() {
    let init = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e",
        "capabilities": ["interrupt_receipt_v1", "interrupt_cancel_queued_v1", "msg_lifecycle_v1"],
        "model": "claude-haiku-4-5-20251001",
        "permissionMode": "default",
        "tools": ["Read", "Bash"],
        "slash_commands": ["compact"],
        "skills": [],
        "claude_code_version": "2.1.266",
    })
    .to_string();
    let result = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "terminal_reason": "completed",
        "duration_ms": 12,
        "session_id": "6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e",
        "usage": {"input_tokens": 2, "output_tokens": 4},
        "modelUsage": {"claude-haiku-4-5-20251001": {"contextWindow": 200000}},
    })
    .to_string();
    // A frame with neither `method` nor `subtype` is the `user` line Fleet writes.
    let peer = MockPeer::new()
        .greeting(&[&init])
        .on(
            "initialize",
            Some(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"__ID__","response":{"models":[{"value":"haiku","displayName":"Haiku","resolvedModel":"claude-haiku-4-5-20251001","supportedEffortLevels":["low","high"]}]}}}"#,
            ),
            &[],
        )
        .on("", Some(&result), &[])
        .lingering()
        .build();

    let mut harness = harness(peer.command());
    let mut events = harness.events();
    let mut request = start_request();
    request.model = None;
    let thread = request.thread;
    let opened = harness
        .open(OpenSession { start: request })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    assert_eq!(
        opened.resume_cursor.as_deref(),
        Some("6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e"),
        "the cursor is durable before any turn runs"
    );
    // The capability gate is read from `system/init`, never from a version compare.
    assert!(matches!(
        harness.capabilities().interrupt,
        fleet_core::agents::InterruptSupport::Receipted {
            cancel_queued: true
        }
    ));

    let turn = TurnId::new();
    let submitted = harness
        .submit(Submit {
            turn,
            input: UserInput {
                text: "hello".to_owned(),
                attachments: Vec::new(),
                item: None,
                origin: Default::default(),
            },
            intent: SubmitIntent::Fresh,
        })
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    assert_eq!(submitted.turn(), turn);
    assert!(!submitted.joined_active());

    let mut seen = Vec::new();
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            let terminal = matches!(event.event, AgentEvent::TurnSettled { .. });
            seen.push(event.event);
            if terminal {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or_else(|_| panic!("the result never settled the turn: {:?}", names(&seen)));
    assert!(settled);
    let mapped = names(&seen);
    assert!(mapped.contains(&"turn_started"), "{mapped:?}");
    assert_eq!(mapped.last().copied(), Some("turn_settled"));

    let mut projection = fleet_core::agents::ThreadProjection::new(
        thread,
        fleet_core::ids::WorktreeId::try_from("acme/api#claude-default")
            .unwrap_or_else(|error| panic!("worktree id: {error}")),
        AgentKind::Claude,
    );
    for (index, event) in seen.iter().cloned().enumerate() {
        projection
            .apply(&fleet_core::agents::SeqEvent {
                seq: fleet_core::agents::Seq(index as u64 + 1),
                at: chrono::Utc::now(),
                raw: None,
                event,
            })
            .unwrap_or_else(|error| panic!("project Claude event: {error}"));
    }
    let selected = projection
        .model
        .as_ref()
        .unwrap_or_else(|| panic!("system/init should select a catalogue model"));
    assert_eq!(selected.model, "haiku");
    assert!(
        projection
            .models
            .iter()
            .any(|model| model.id == selected.model),
        "the active selector must exact-match a discovered descriptor: {projection:?}"
    );

    let written = peer.wait_for_frames(2, Duration::from_secs(5)).await;
    let user = written
        .get(1)
        .unwrap_or_else(|| panic!("Fleet wrote a frame"));
    assert!(user.contains(r#""type":"user""#), "{user}");
    harness
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// Stop against a child that never answers still finishes, and the child is killed.
#[tokio::test]
async fn stop_finishes_against_a_child_that_ignores_sigterm() {
    let peer = MockPeer::new().ignoring_sigterm().build();
    let mut harness = harness(peer.command());
    let events = harness.events();
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("a silent child is still a live session: {error}"));
    let stopped = tokio::time::timeout(
        Duration::from_secs(20),
        harness.shutdown(ShutdownReason::User),
    )
    .await
    .unwrap_or_else(|_| panic!("shutdown must not hang on a wedged child"));
    stopped.unwrap_or_else(|error| panic!("shutdown: {error}"));
    drop(events);
}

/// A configured binary that is not Claude at all.
///
/// `agentBinaries.claude = "cc"` resolves to the C compiler, which rejects Fleet's launch line
/// and dies inside the spawn window. The handshake error has to carry the configured command,
/// the exit code and the child's own complaint — without them the user is told only "the process
/// exited during startup" and is pointed at a binary Fleet never ran.
#[tokio::test]
async fn a_child_that_dies_at_startup_reports_its_command_code_and_stderr() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory =
        tempfile::tempdir().unwrap_or_else(|error| panic!("startup fixture dir: {error}"));
    let script = directory.path().join("cc");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         printf '%s\\n' \"cc: error: unrecognized command-line option '--output-format'\" >&2\n\
         exit 1\n",
    )
    .unwrap_or_else(|error| panic!("write the fake binary: {error}"));
    let mut permissions = std::fs::metadata(&script)
        .unwrap_or_else(|error| panic!("stat the fake binary: {error}"))
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions)
        .unwrap_or_else(|error| panic!("chmod the fake binary: {error}"));

    let command = script.to_string_lossy().into_owned();
    let mut harness = harness(command.clone());
    let events = harness.events();
    let message = harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .err()
        .unwrap_or_else(|| panic!("a child that exits at once cannot open a session"))
        .to_string();
    assert!(message.contains(&command), "{message}");
    assert!(message.contains("exited with code 1"), "{message}");
    assert!(
        message.contains("unrecognized command-line option"),
        "{message}"
    );
    drop(events);
}

/// A child that survives its startup is accepting input, so the session reads `Ready` at once:
/// `system/init` only arrives with the first prompt, and a thread that showed `starting…` until
/// then could not be told apart from a hang.
#[tokio::test]
async fn a_child_that_survives_its_startup_is_ready_before_the_first_prompt() {
    let peer = MockPeer::new().lingering().build();
    let mut harness = harness(peer.command());
    let mut events = harness.events();
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        seen.push(event.event);
    }
    assert!(
        seen.iter().any(|event| matches!(
            event,
            AgentEvent::SessionStateChanged(fleet_core::agents::SessionState::Ready)
        )),
        "{:?}",
        names(&seen)
    );
    harness
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// `--resume` of a session the CLI never wrote a conversation for — a thread created and never
/// prompted before the daemon went away — falls back to a fresh launch under the same id, so the
/// thread keeps its cursor instead of dying with "No conversation found".
#[tokio::test]
async fn a_resume_of_a_conversation_claude_never_wrote_starts_fresh_under_the_same_id() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory =
        tempfile::tempdir().unwrap_or_else(|error| panic!("resume fixture dir: {error}"));
    let script = directory.path().join("claude");
    let argv_log = directory.path().join("argv.log");
    // The first launch (`--resume`) is refused the way 2.1.266 refuses it; the second one, which
    // must carry `--session-id`, stays up like a healthy child.
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {log}\n\
             case \" $* \" in\n\
             \x20 *' --resume '*)\n\
             \x20   printf '%s\\n' 'No conversation found with session ID: x' >&2\n\
             \x20   exit 0 ;;\n\
             esac\n\
             sleep 120\n",
            log = argv_log.display()
        ),
    )
    .unwrap_or_else(|error| panic!("write the fake binary: {error}"));
    let mut permissions = std::fs::metadata(&script)
        .unwrap_or_else(|error| panic!("stat the fake binary: {error}"))
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions)
        .unwrap_or_else(|error| panic!("chmod the fake binary: {error}"));

    let cursor = "6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e";
    let mut harness = harness(script.to_string_lossy().into_owned());
    let events = harness.events();
    let opened = harness
        .open(OpenSession {
            start: StartRequest {
                resume_cursor: Some(cursor.to_owned()),
                ..start_request()
            },
        })
        .await
        .unwrap_or_else(|error| panic!("the fallback launch must open: {error}"));
    assert_eq!(opened.resume_cursor.as_deref(), Some(cursor));

    let launches = std::fs::read_to_string(&argv_log)
        .unwrap_or_else(|error| panic!("read the argv log: {error}"));
    let launches: Vec<&str> = launches.lines().collect();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert!(
        launches[0].contains(&format!("--resume {cursor}")),
        "{launches:?}"
    );
    assert!(
        launches[1].contains(&format!("--session-id {cursor}")),
        "{launches:?}"
    );
    assert!(!launches[1].contains("--resume"), "{launches:?}");
    harness
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
    drop(events);
}
