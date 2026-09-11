//! Claude adapter tests: the captured fixtures, the byte-exact outbound goldens, the three
//! privacy tests, and a mock peer for the process-level behaviour.
//!
//! Split by concern: [`mapping`] replays real captures, [`turns`] owns the turn state machine,
//! [`goldens`] pins every outbound frame, [`privacy`] proves no payload escapes, and [`peer`]
//! drives a spawned child.

mod goldens;
mod mapping;
mod peer;
mod privacy;
mod turns;

use std::time::Duration;

use fleet_core::agents::{
    AbortReason, AgentEvent, AgentKind, Attachment, AttachmentSource, GateAnswer, GateKind,
    GateResolver, ItemId, ItemKind, ItemStatus, ModelSelection, PermissionChoice, PermissionMode,
    PlanAnswer, SessionState, StartRequest, ThreadId, TurnId, TurnOutcome, UserInput,
};
use semver::Version;
use serde_json::{Value, json};

use super::{
    ClaudeHarness,
    frames::{Frame, ParsedFrame, parse_frame},
    map,
    session::ClaudeSession,
    user_frame,
};
use crate::agents::harness::{
    Harness, HarnessConfig, OpenSession, ShutdownReason, Submit, SubmitIntent,
    capture::{SECRETS, assert_no_secret, logged},
    mockpeer::MockPeer,
    probe::Probed,
};

const BASIC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-basic.ndjson"
));
const PERMISSION: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-permission-stdout.ndjson"
));
/// The live capture of a real `can_use_tool`, `localSettings` suggestion included.
const CAN_USE_TOOL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-can-use-tool.ndjson"
));
/// The live capture of a real `AskUserQuestion`.
const QUESTION: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-question.ndjson"
));
/// The whole 2.1.266 ground-truth capture: one turn, thinking, a tool call and a `result`.
const LIVE_TURN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-live-turn.ndjson"
));
/// The whole 2.1.266 capture of a turn whose Bash command the CLI asked about.
const LIVE_PERMISSION: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-permission-live.ndjson"
));

fn start_request() -> StartRequest {
    StartRequest {
        thread: ThreadId::new(),
        worktree_path: std::env::temp_dir(),
        provider: AgentKind::Claude,
        model: Some(ModelSelection {
            model: "claude-haiku-4-5-20251001".to_owned(),
            effort: Some("low".to_owned()),
            provider: None,
        }),
        mode: PermissionMode::Ask,
        resume_cursor: None,
        fork: false,
        env: std::collections::BTreeMap::new(),
        sandbox: fleet_core::agents::SandboxPolicy::default(),
        approval_policy: fleet_core::agents::ApprovalPolicy::default(),
        permission_profile: None,
        title: None,
    }
}

fn harness(command: String) -> ClaudeHarness {
    ClaudeHarness::new(
        HarnessConfig {
            command,
            client_version: "0.1.0".to_owned(),
            ..HarnessConfig::default()
        },
        Probed {
            kind: AgentKind::Claude,
            version: Version::new(2, 1, 266),
            reported: "2.1.266 (Claude Code)".to_owned(),
        },
    )
}

/// Replays a captured stream through the mapper, opening a turn once init lands.
fn replay(fixture: &str) -> (ClaudeSession, Vec<AgentEvent>) {
    let mut session = ClaudeSession::default();
    let mut events = Vec::new();
    let mut started = false;
    for line in fixture.lines() {
        let ParsedFrame::Frame(frame) = parse_frame(line) else {
            panic!("every captured line must decode");
        };
        let frame = *frame;
        let is_init = matches!(&frame, Frame::System(system) if system.subtype == "init");
        events.extend(map::handle(&mut session, frame).events);
        if is_init && !started {
            started = true;
            if let Some(event) = session
                .begin_turn(TurnId::new(), ItemId::new())
                .unwrap_or_else(|error| panic!("{error}"))
            {
                events.push(event);
            }
        }
    }
    (session, events)
}

fn names(events: &[AgentEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AgentEvent::SessionConfigured { .. } => "session_configured",
            AgentEvent::MetadataChanged { .. } => "metadata_changed",
            AgentEvent::SessionStateChanged(_) => "session_state_changed",
            AgentEvent::SessionActivity { .. } => "session_activity",
            AgentEvent::SessionExited { .. } => "session_exited",
            AgentEvent::TurnStarted { .. } => "turn_started",
            AgentEvent::TurnSettled { .. } => "turn_settled",
            AgentEvent::TurnAborted { .. } => "turn_aborted",
            AgentEvent::TurnDiff { .. } => "turn_diff",
            AgentEvent::PlanSteps { .. } => "plan_steps",
            AgentEvent::ItemStarted { .. } => "item_started",
            AgentEvent::ContentDelta { .. } => "content_delta",
            AgentEvent::ItemUpdated { .. } => "item_updated",
            AgentEvent::ItemCompleted { .. } => "item_completed",
            AgentEvent::GateOpened { .. } => "gate_opened",
            AgentEvent::GateResolved { .. } => "gate_resolved",
            AgentEvent::GateWithdrawn { .. } => "gate_withdrawn",
            AgentEvent::PlanProposed { .. } => "plan_proposed",
            AgentEvent::TokenUsage { .. } => "token_usage",
            AgentEvent::RateLimits { .. } => "rate_limits",
            AgentEvent::Compacted(_) => "compacted",
            AgentEvent::Retrying { .. } => "retrying",
            AgentEvent::ModelRerouted { .. } => "model_rerouted",
            AgentEvent::RuntimeError { .. } => "runtime_error",
            AgentEvent::Notice(_) => "notice",
            AgentEvent::Unknown { .. } => "unknown",
        })
        .collect()
}

fn frame(line: &str) -> Frame {
    match parse_frame(line) {
        ParsedFrame::Frame(frame) => *frame,
        ParsedFrame::Undecodable { kind, fingerprint } => {
            panic!("{kind} did not decode: {}", fingerprint.summary())
        }
    }
}
