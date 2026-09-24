//! Byte-exact wire goldens for the whole native-agent protocol family.
//!
//! `rust-ipc-protocol` Rule 9 asks for one golden per wire shape, and the agent family shipped
//! with **zero** — only round-trip assertions, which pass just as happily after a field is
//! renamed or a case is changed on both sides at once. These are the debt payment, and they cover
//! four things the round trips cannot:
//!
//! 1. **Every request, response, and event in the family**, so the windowed open cannot silently
//!    change the shape introduced by version 7.
//! 2. **One `SeqEvent` golden per [`AgentEvent`] variant.** That set protects the *persisted log*,
//!    not just the wire: `agent_events.payload` is this exact serialization, so a rename here is a
//!    transcript that no longer decodes on the next daemon start.
//! 3. **One fixture per variant of the delegation value types**, and one per new shape with every
//!    optional field absent — the only place a missing `skip_serializing_if` or a changed
//!    `snake_case` word is visible, since the composite goldens only ever reach one variant each.
//! 4. **Legacy-peer fixtures**, proving a payload written before the window fields, before Codex,
//!    before the sequenced log, and before delegation still decodes.

use crate::support;

#[path = "agent_compatibility/legacy.rs"]
mod legacy;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{
        AbortReason, AccountInfo, AccountKind, AccountStatus, AgentEvent, AgentKind,
        AgentThreadSummary, Attention, AttentionKind, CheckpointKind, Delegation, DelegationCaller,
        DelegationId, DelegationResult, DelegationStatus, DelegationUsage, DeliveryState,
        FileDelta, GateAnswer, GateId, GateKind, GateResolver, Item, ItemId, ItemKind, ItemPatch,
        ItemPayloadPatch, ItemStatus, MessageOrigin, ModelDescriptor, ModelSelection,
        PermissionChoice, PermissionMode, PlanAnswer, ProviderOptionId, Question, QuestionOption,
        ReasoningEffortDescriptor, ResultSource, Seq, SeqEvent, SessionState, StreamKind, ThreadId,
        ThreadProjection, ToolCall, ToolKind, TurnId, TurnOutcome, TurnState, Usage, UserInput,
    },
    ids::{BoardId, CardId, WorktreeId},
};
use fleet_proto::{
    agents::{
        AgentRevertReport, AgentSeenCursor, AgentSessionView, AgentThreadWindow, CheckpointId,
        CheckpointScope, TranscriptPage, TranscriptWindow, TurnCheckpoint,
    },
    event::Event,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use support::assert_frame;

/// Every `AgentEvent` tag this build implements, in declaration order.
///
/// The list is duplicated from `fleet_core::agents::AgentEvent` on purpose: it is the tripwire
/// that says a variant was added without a golden, and it can only do that by being written down
/// twice. Both copies move in the same commit.
const AGENT_EVENT_TAGS: &[&str] = &[
    "session_configured",
    "metadata_changed",
    "session_state_changed",
    "session_activity",
    "session_exited",
    "turn_started",
    "turn_settled",
    "turn_aborted",
    "turn_diff",
    "plan_steps",
    "item_started",
    "content_delta",
    "item_updated",
    "item_completed",
    "gate_opened",
    "gate_resolved",
    "gate_withdrawn",
    "plan_proposed",
    "token_usage",
    "rate_limits",
    "compacted",
    "retrying",
    "model_rerouted",
    "runtime_error",
    "account_changed",
    "notice",
    "unknown",
];

#[test]
fn agent_request_wire_goldens() {
    for (request, golden) in request_goldens() {
        assert_frame(request, golden);
    }
}

#[test]
fn agent_thread_create_omits_mode_for_daemon_defaults() {
    assert_frame(
        Request {
            id: 30,
            body: RequestBody::AgentThreadCreate {
                worktree: worktree(),
                provider: AgentKind::Claude,
                model: None,
                mode: None,
                resume_cursor: None,
                title: None,
            },
        },
        r#"{"id":30,"body":{"type":"agent_thread_create","worktree":"acme/api#native-agents","provider":"claude","model":null,"resume_cursor":null,"title":null}}"#,
    );
}

#[test]
fn agent_response_wire_goldens() {
    for (response, golden) in response_goldens() {
        assert_frame(response, golden);
    }
}

#[test]
fn agent_event_wire_goldens() {
    for (event, golden) in event_goldens() {
        assert_frame(event, golden);
    }
}

/// Additive `GateKind::Permission.item`: old fixtures omit it, while this golden pins the exact
/// join key a new daemon sends to a new app.
#[test]
fn permission_gate_item_wire_golden() {
    assert_frame(
        seq(
            27,
            Some("item/fileChange/requestApproval"),
            AgentEvent::GateOpened {
                gate: gate(),
                turn: Some(turn()),
                kind: GateKind::Permission {
                    item: Some(item()),
                    tool: ToolKind::Edit,
                    title: "Codex wants to apply a patch".to_owned(),
                    payload: "apply the edit to README.md".to_owned(),
                    rationale: None,
                    options: vec![fleet_core::agents::PermissionOption {
                        id: ProviderOptionId("accept".to_owned()),
                        label: PermissionChoice::AllowOnce,
                    }],
                },
            },
        ),
        r#"{"seq":27,"at":"2026-09-07T12:00:00Z","raw":"item/fileChange/requestApproval","event":{"type":"gate_opened","data":{"gate":"cccccccc-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","kind":{"type":"permission","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","tool":{"type":"edit"},"title":"Codex wants to apply a patch","payload":"apply the edit to README.md","rationale":null,"options":[{"id":"accept","label":"allow_once"}]}}}}}"#,
    );
}

#[test]
fn model_descriptors_and_skill_refresh_have_additive_wire_goldens() {
    let descriptor = ModelDescriptor {
        id: "gpt-5.6-sol".to_owned(),
        display_name: "GPT-5.6 Sol".to_owned(),
        efforts: vec![ReasoningEffortDescriptor {
            id: "high".to_owned(),
            description: "Deep reasoning".to_owned(),
        }],
        default_effort: Some("high".to_owned()),
    };
    assert_frame(
        seq(
            28,
            Some("model/list"),
            AgentEvent::SessionConfigured {
                provider: AgentKind::Codex,
                resume_cursor: Some("thread-1".to_owned()),
                model: Some(model()),
                models: vec![descriptor.clone()],
                mode: PermissionMode::Ask,
                tools: Vec::new(),
                commands: Vec::new(),
                skills: vec!["review".to_owned()],
            },
        ),
        r#"{"seq":28,"at":"2026-09-07T12:00:00Z","raw":"model/list","event":{"type":"session_configured","data":{"provider":"codex","resume_cursor":"thread-1","model":{"model":"gpt-5-codex","effort":"high"},"models":[{"id":"gpt-5.6-sol","displayName":"GPT-5.6 Sol","efforts":[{"id":"high","description":"Deep reasoning"}],"defaultEffort":"high"}],"mode":"ask","tools":[],"commands":[],"skills":["review"]}}}"#,
    );
    assert_frame(
        seq(
            29,
            Some("skills/changed"),
            AgentEvent::MetadataChanged {
                title: None,
                mode: None,
                model: None,
                skills: Some(vec!["review".to_owned(), "ship".to_owned()]),
            },
        ),
        r#"{"seq":29,"at":"2026-09-07T12:00:00Z","raw":"skills/changed","event":{"type":"metadata_changed","data":{"skills":["review","ship"]}}}"#,
    );
    assert_frame(
        AgentSessionView {
            model: Some(ModelSelection {
                model: "gpt-5.6-sol".to_owned(),
                effort: Some("high".to_owned()),
                provider: None,
            }),
            models: vec![descriptor],
            skills: vec!["review".to_owned(), "ship".to_owned()],
            ..AgentSessionView::default()
        },
        r#"{"model":{"model":"gpt-5.6-sol","effort":"high"},"models":[{"id":"gpt-5.6-sol","displayName":"GPT-5.6 Sol","efforts":[{"id":"high","description":"Deep reasoning"}],"defaultEffort":"high"}],"mode":"ask","skills":["review","ship"]}"#,
    );
    // The account rides on the same view, and a view without one keeps the bytes above: the
    // golden immediately before this proves the field is skipped when the harness reported none.
    assert_frame(
        AgentSessionView {
            account: Some(AccountStatus::SignedOut),
            ..AgentSessionView::default()
        },
        r#"{"mode":"ask","account":{"type":"signed_out"}}"#,
    );
}

#[test]
fn delegation_item_and_origin_wire_goldens() {
    assert_frame(
        seq(
            30,
            None,
            AgentEvent::ItemStarted {
                turn: turn(),
                item: item(),
                kind: ItemKind::Delegation {
                    id: delegation_id(),
                    provider: AgentKind::Codex,
                    child: child_thread(),
                    status: DelegationStatus::Running,
                },
                parent: None,
            },
        ),
        r#"{"seq":30,"at":"2026-09-07T12:00:00Z","event":{"type":"item_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","kind":{"type":"delegation","data":{"id":"dddddddd-2222-4333-8444-555555555555","provider":"codex","child":"22222222-3333-4444-8555-666666666666","status":"running"}},"parent":null}}}"#,
    );
    assert_frame(
        seq(
            31,
            None,
            AgentEvent::ItemUpdated {
                item: item(),
                patch: ItemPatch {
                    payload: Some(ItemPayloadPatch::Delegation {
                        status: DelegationStatus::Succeeded,
                    }),
                    status: None,
                },
            },
        ),
        r#"{"seq":31,"at":"2026-09-07T12:00:00Z","event":{"type":"item_updated","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","patch":{"payload":{"type":"delegation","data":{"status":"succeeded"}}}}}}"#,
    );
    assert_frame(
        seq(
            32,
            None,
            AgentEvent::ItemStarted {
                turn: turn(),
                item: item(),
                kind: ItemKind::UserMessage {
                    text: "child result".to_owned(),
                    attachments: Vec::new(),
                    steered: false,
                    origin: MessageOrigin::Delegation {
                        id: delegation_id(),
                    },
                },
                parent: None,
            },
        ),
        r#"{"seq":32,"at":"2026-09-07T12:00:00Z","event":{"type":"item_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","kind":{"type":"user_message","data":{"text":"child result","attachments":[],"origin":{"type":"delegation","data":{"id":"dddddddd-2222-4333-8444-555555555555"}}}},"parent":null}}}"#,
    );
    assert_frame(
        seq(
            33,
            None,
            AgentEvent::ItemStarted {
                turn: turn(),
                item: item(),
                kind: ItemKind::UserMessage {
                    text: "ordinary user message".to_owned(),
                    attachments: Vec::new(),
                    steered: false,
                    origin: MessageOrigin::User,
                },
                parent: None,
            },
        ),
        r#"{"seq":33,"at":"2026-09-07T12:00:00Z","event":{"type":"item_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","kind":{"type":"user_message","data":{"text":"ordinary user message","attachments":[]}},"parent":null}}}"#,
    );
}

/// One byte fixture per variant of every enum the delegation family introduced.
///
/// The composite goldens above only ever reach a handful of these — a `Succeeded` delegation that
/// was `Delivered` from a `Reported` result. The remaining variants are exactly the ones a rename
/// would slip past: they are written to `delegations.status` and `delegations.delivery` as these
/// words, so a casing change here is a row the next daemon start cannot parse.
#[test]
fn delegation_value_types_have_one_wire_golden_per_variant() {
    // `DelegationStatus::word` deliberately says "working" and "done"; the wire never does.
    for (status, golden) in [
        (DelegationStatus::Starting, r#""starting""#),
        (DelegationStatus::Running, r#""running""#),
        (DelegationStatus::Blocked, r#""blocked""#),
        (DelegationStatus::Settling, r#""settling""#),
        (DelegationStatus::Succeeded, r#""succeeded""#),
        (DelegationStatus::Incomplete, r#""incomplete""#),
        (DelegationStatus::Failed, r#""failed""#),
        (DelegationStatus::Cancelled, r#""cancelled""#),
    ] {
        assert_frame(status, golden);
    }

    assert_frame(ResultSource::Reported, r#""reported""#);
    assert_frame(ResultSource::LastAssistantText, r#""last_assistant_text""#);

    assert_frame(DeliveryState::Pending, r#"{"type":"pending"}"#);
    assert_frame(
        DeliveryState::Delivered {
            seq: Seq(42),
            turn: turn(),
        },
        r#"{"type":"delivered","data":{"seq":42,"turn":"aaaaaaaa-2222-4333-8444-555555555555"}}"#,
    );
    assert_frame(DeliveryState::Consumed, r#"{"type":"consumed"}"#);
    assert_frame(DeliveryState::Recorded, r#"{"type":"recorded"}"#);
    assert_frame(
        DeliveryState::Undeliverable {
            reason: "the caller thread was deleted".to_owned(),
        },
        r#"{"type":"undeliverable","data":{"reason":"the caller thread was deleted"}}"#,
    );

    assert_frame(MessageOrigin::User, r#"{"type":"user"}"#);
    assert_frame(
        MessageOrigin::Delegation {
            id: delegation_id(),
        },
        r#"{"type":"delegation","data":{"id":"dddddddd-2222-4333-8444-555555555555"}}"#,
    );

    // A recovered, truncated result: the half of `DelegationResult` the composite goldens skip.
    assert_frame(
        DelegationResult {
            text: "partial answer".to_owned(),
            files_changed: Vec::new(),
            source: ResultSource::LastAssistantText,
            elided: true,
        },
        r#"{"text":"partial answer","source":"last_assistant_text","elided":true}"#,
    );

    assert_frame(
        starting_delegation(),
        r#"{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","status":"starting","nudges":0,"recoveries":0,"delivery":{"type":"pending"},"created":"2026-09-07T12:00:00Z"}"#,
    );

    // The child's own spend, which only the three daemon read verbs ever attach. `Usage` is
    // reused rather than restated, so this golden also pins that the CLI and the GUI fold the
    // same eight counters.
    assert_frame(
        delegation_usage(),
        r#"{"usage":{"inputTokens":1200,"outputTokens":340,"reasoningTokens":0,"cacheReadTokens":9000,"cacheWriteTokens":0,"totalTokens":10540,"webSearchRequests":0,"toolUses":7},"costUsd":0.42,"contextPct":12.5}"#,
    );

    // And its position on `Delegation`: last, after `headline`, so a record that carries it is a
    // superset of the bytes every persisted golden above pins.
    assert_frame(
        Delegation {
            usage: Some(delegation_usage()),
            ..starting_delegation()
        },
        r#"{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","status":"starting","nudges":0,"recoveries":0,"delivery":{"type":"pending"},"created":"2026-09-07T12:00:00Z","usage":{"usage":{"inputTokens":1200,"outputTokens":340,"reasoningTokens":0,"cacheReadTokens":9000,"cacheWriteTokens":0,"totalTokens":10540,"webSearchRequests":0,"toolUses":7},"costUsd":0.42,"contextPct":12.5}}"#,
    );
}

/// The card caller is the only new `caller` shape, and the only record that omits both caller
/// keys. The thread fixture above is not edited: a thread caller is still a bare id string, so a
/// record written by a build that had no card callers decodes here unchanged, and this build
/// writes those same bytes back.
#[test]
fn a_card_called_delegation_carries_its_board_and_card_and_no_caller_turn() {
    assert_frame(
        card_called_delegation(),
        r#"{"id":"dddddddd-2222-4333-8444-555555555555","caller":{"board":"work","card":"card-12"},"child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","status":"starting","nudges":0,"recoveries":0,"delivery":{"type":"recorded"},"created":"2026-09-07T12:00:00Z"}"#,
    );
}

/// `parent` is the only field a child thread adds to two shapes a version-7 peer already knows.
/// The goldens above all carry `parent: None` and so prove it is absent; these two prove the key
/// and its position when the thread really is a delegated child.
#[test]
fn a_delegated_child_carries_its_parent_on_the_wire() {
    assert_frame(
        AgentThreadSummary {
            thread: child_thread(),
            parent: Some(thread()),
            ..summary()
        },
        r#"{"thread":"22222222-3333-4444-8555-666666666666","parent":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}"#,
    );

    let mut projection = ThreadProjection::new(child_thread(), worktree(), AgentKind::Codex);
    projection.parent = Some(thread());
    assert_frame(
        projection,
        r#"{"thread":"22222222-3333-4444-8555-666666666666","parent":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","session":{"type":"starting"},"turn":{"type":"none"},"gates":[],"items":[],"turns":[],"backgroundTasks":[],"lastSeq":0,"lastActivity":null,"cumulativeUsage":{"inputTokens":0,"outputTokens":0,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"cumulativeCostUsd":null,"contextPct":0.0,"model":null,"mode":"ask","exitCode":null,"retrying":null}"#,
    );
}

/// A capability string is wire vocabulary a peer matches literally in `HelloResponse.capabilities`,
/// so a rename here is a negotiation that fails open rather than loudly. `AGENT_CAPABILITIES` does
/// not carry this one yet — phase 3 adds it once the daemon serves the family.
#[test]
fn the_delegation_capability_string_is_pinned() {
    assert_eq!(fleet_proto::AGENT_DELEGATION_CAPABILITY, "agent.delegation");
}

/// `fleet_path` is advisory, and the only thing that makes an advisory field safe to add is that
/// its absence is indistinguishable from the payload a peer sent before it existed. The golden
/// pair pins the encode side of that; this pins the decode side, which is the half a daemon
/// facing an un-upgraded `fleet` actually runs.
#[test]
fn a_delegation_run_written_before_the_fleet_path_hint_still_decodes() {
    let request: Request = serde_json::from_str(
        r#"{"id":26,"body":{"type":"delegation_run","caller":"11111111-2222-4333-8444-555555555555","provider":"claude","brief":"summarize the diff","expectation":"one paragraph"}}"#,
    )
    .unwrap_or_else(|error| panic!("pre-fleet-path delegation run: {error}"));

    assert!(matches!(
        request.body,
        RequestBody::DelegationRun {
            fleet_path: None,
            ..
        }
    ));
}

/// `env` is the second advisory field on `delegation_run`, and it carries the same promise: a
/// caller that never sets one sends the bytes a version-7 peer sent. The golden pair above pins
/// the encode side; this pins the decode side a daemon facing an un-upgraded `fleet` runs.
#[test]
fn a_delegation_run_written_before_the_child_environment_still_decodes() {
    let request: Request = serde_json::from_str(
        r#"{"id":26,"body":{"type":"delegation_run","caller":"11111111-2222-4333-8444-555555555555","provider":"claude","brief":"summarize the diff","expectation":"one paragraph","fleet_path":"/opt/fleet/bin/fleet"}}"#,
    )
    .unwrap_or_else(|error| panic!("pre-env delegation run: {error}"));

    let RequestBody::DelegationRun { env, .. } = request.body else {
        panic!("expected a delegation run");
    };
    assert!(env.is_empty(), "{env:?}");
}

/// `caller` decides whether a terminal answer also consumes the delivery, so a daemon facing an
/// un-upgraded `fleet` must read the field as absent rather than refuse the wait. Absent means
/// "consume nothing", which is exactly what every peer did before the field existed.
#[test]
fn a_delegation_wait_written_before_the_caller_hint_still_decodes() {
    let request: Request = serde_json::from_str(
        r#"{"id":25,"body":{"type":"delegation_wait","delegation":"dddddddd-2222-4333-8444-555555555555","timeout_ms":30000}}"#,
    )
    .unwrap_or_else(|error| panic!("pre-caller delegation wait: {error}"));

    assert!(matches!(
        request.body,
        RequestBody::DelegationWait { caller: None, .. }
    ));
}

#[test]
fn the_closed_threads_capability_string_is_pinned() {
    assert_eq!(fleet_proto::AGENT_CLOSED_CAPABILITY, "agent.closed");
}

#[test]
fn legacy_permission_gate_defaults_the_additive_item() {
    let gate: GateKind = serde_json::from_str(
        r#"{"type":"permission","data":{"tool":{"type":"edit"},"title":"Apply patch","payload":"README.md","rationale":null,"options":[]}}"#,
    )
    .unwrap_or_else(|error| panic!("legacy permission gate: {error}"));

    assert!(matches!(gate, GateKind::Permission { item: None, .. }));
}

#[test]
fn every_agent_event_variant_has_a_persisted_payload_golden() {
    let goldens = seq_event_goldens();
    let tags = goldens
        .iter()
        .map(|(event, _)| {
            serde_json::to_value(&event.event)
                .unwrap_or_else(|error| panic!("{error}"))
                .get("type")
                .and_then(|tag| tag.as_str())
                .unwrap_or_else(|| panic!("an adjacently tagged payload always has a type"))
                .to_owned()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        tags, AGENT_EVENT_TAGS,
        "every AgentEvent variant needs a byte-exact SeqEvent golden, in declaration order"
    );
    for (event, golden) in goldens {
        assert_frame(event, golden);
    }
}

/// Fixed identifiers, so a golden is a byte fixture and never a snapshot of a random UUID.
fn thread() -> ThreadId {
    "11111111-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn turn() -> TurnId {
    "aaaaaaaa-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn item() -> ItemId {
    "bbbbbbbb-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn gate() -> GateId {
    "cccccccc-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn delegation_id() -> DelegationId {
    "dddddddd-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn child_thread() -> ThreadId {
    "22222222-3333-4444-8555-666666666666"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

/// The third checkpoint of [`turn`], which is what a `[u] revert turn` would address.
fn checkpoint() -> CheckpointId {
    CheckpointId::from_parts(3, CheckpointScope::Turn, turn())
}

fn worktree() -> WorktreeId {
    WorktreeId::try_from("acme/api#native-agents").unwrap_or_else(|error| panic!("{error}"))
}

fn at() -> DateTime<Utc> {
    "2026-09-07T12:00:00Z"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn model() -> ModelSelection {
    ModelSelection {
        model: "gpt-5-codex".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    }
}

fn delegation() -> Delegation {
    Delegation {
        id: delegation_id(),
        caller: DelegationCaller::Thread(thread()),
        caller_turn: Some(turn()),
        caller_item: Some(item()),
        child: child_thread(),
        provider: AgentKind::Codex,
        depth: 1,
        brief: "write protocol goldens".to_owned(),
        expectation: "all wire bytes are pinned".to_owned(),
        eager: true,
        status: DelegationStatus::Succeeded,
        status_payload: Some("reported complete".to_owned()),
        result: Some(DelegationResult {
            text: "goldens added".to_owned(),
            files_changed: vec!["crates/fleet-proto/tests/agent_compatibility.rs".to_owned()],
            source: ResultSource::Reported,
            elided: false,
        }),
        nudges: 1,
        recoveries: 0,
        delivery: DeliveryState::Delivered {
            seq: Seq(42),
            turn: turn(),
        },
        created: at(),
        finished: Some(at()),
        headline: Some("Pinned delegation wire shapes".to_owned()),
        // `usage` is computed on read and never persisted, so the record every golden below
        // pins — the one the store decodes and `publish_changed` broadcasts — carries `None`.
        usage: None,
    }
}

/// The same delegation the instant `DelegationRun` answers: nothing optional has happened yet.
fn starting_delegation() -> Delegation {
    Delegation {
        eager: false,
        status: DelegationStatus::Starting,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        finished: None,
        headline: None,
        ..delegation()
    }
}

/// A card-called delegation: a board column started it, so the caller is the board and the card,
/// there is no caller turn or transcript item, and its result is recorded on the board rather than
/// delivered into a transcript.
fn card_called_delegation() -> Delegation {
    Delegation {
        caller: DelegationCaller::Card {
            board: BoardId::try_from("work".to_owned()).unwrap_or_else(|error| panic!("{error}")),
            card: CardId::try_from("card-12".to_owned()).unwrap_or_else(|error| panic!("{error}")),
        },
        caller_turn: None,
        caller_item: None,
        delivery: DeliveryState::Recorded,
        ..starting_delegation()
    }
}

/// A child's own spend, as the three daemon read verbs attach it.
fn delegation_usage() -> DelegationUsage {
    DelegationUsage {
        usage: Usage {
            input_tokens: 1_200,
            output_tokens: 340,
            cache_read_tokens: 9_000,
            total_tokens: 10_540,
            tool_uses: 7,
            ..Usage::default()
        },
        cost_usd: Some(0.42),
        context_pct: 12.5,
    }
}

/// The reason `env` exists: two children in one worktree that must not share a build lock.
fn child_env() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "CARGO_TARGET_DIR".to_owned(),
            "/work/target-child".to_owned(),
        ),
        ("RUSTFLAGS".to_owned(), "-D warnings".to_owned()),
    ])
}

fn summary() -> AgentThreadSummary {
    AgentThreadSummary {
        thread: thread(),
        worktree: worktree(),
        host: None,
        provider: AgentKind::Codex,
        title: "Codex".to_owned(),
        attention: Attention::NeedsYou(AttentionKind::Permission),
        session: SessionState::Ready,
        turn: TurnState::Running(turn()),
        last_seq: Seq(12),
        last_activity: Some(at()),
        last_completed_seq: Some(Seq(9)),
        last_nonterminal_seq: Some(Seq(11)),
        exit_code: None,
        parent: None,
    }
}

fn window_item() -> Item {
    Item {
        id: item(),
        turn: turn(),
        parent: None,
        kind: ItemKind::AssistantText {
            text: "done".to_owned(),
        },
        status: ItemStatus::Completed,
        children: Vec::new(),
        started: at(),
        ended: Some(at()),
    }
}

fn tool_call() -> ToolCall {
    ToolCall {
        kind: ToolKind::Bash,
        name: "shell".to_owned(),
        input: serde_json::json!({"command": "cargo test"}),
        summary: Some("cargo test".to_owned()),
        result: None,
        output: "ok".to_owned(),
        diff: None,
        exit_code: Some(0),
        duration_ms: Some(1_200),
        extra: std::collections::BTreeMap::new(),
    }
}

fn seq(seq: u64, raw: Option<&str>, event: AgentEvent) -> SeqEvent {
    SeqEvent {
        seq: Seq(seq),
        at: at(),
        raw: raw.map(str::to_owned),
        event,
    }
}

fn request_goldens() -> Vec<(Request, &'static str)> {
    let thread = thread();
    vec![
        (
            Request {
                id: 1,
                body: RequestBody::AgentThreadList,
            },
            r#"{"id":1,"body":{"type":"agent_thread_list"}}"#,
        ),
        (
            Request {
                id: 2,
                body: RequestBody::AgentThreadCreate {
                    worktree: worktree(),
                    provider: AgentKind::Codex,
                    model: Some(model()),
                    mode: Some(PermissionMode::Ask),
                    resume_cursor: Some("thread-1".to_owned()),
                    title: Some("native agents".to_owned()),
                },
            },
            r#"{"id":2,"body":{"type":"agent_thread_create","worktree":"acme/api#native-agents","provider":"codex","model":{"model":"gpt-5-codex","effort":"high"},"mode":"ask","resume_cursor":"thread-1","title":"native agents"}}"#,
        ),
        (
            Request {
                id: 3,
                body: RequestBody::AgentThreadOpen {
                    thread,
                    from_seq: Some(Seq(41)),
                    after_seq: None,
                    turn_limit: None,
                    before_cursor: None,
                    request_sync_marker: false,
                },
            },
            r#"{"id":3,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":41}}"#,
        ),
        (
            Request {
                id: 4,
                body: RequestBody::AgentThreadOpen {
                    thread,
                    from_seq: None,
                    after_seq: None,
                    turn_limit: Some(10),
                    before_cursor: None,
                    request_sync_marker: true,
                },
            },
            r#"{"id":4,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":null,"turn_limit":10,"request_sync_marker":true}}"#,
        ),
        (
            Request {
                id: 5,
                body: RequestBody::AgentThreadOpen {
                    thread,
                    from_seq: None,
                    after_seq: Some(Seq(41)),
                    turn_limit: Some(25),
                    before_cursor: Some("fat.1.11111111-2222-4333-8444-555555555555.90".to_owned()),
                    request_sync_marker: true,
                },
            },
            r#"{"id":5,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":null,"after_seq":41,"turn_limit":25,"before_cursor":"fat.1.11111111-2222-4333-8444-555555555555.90","request_sync_marker":true}}"#,
        ),
        (
            Request {
                id: 6,
                body: RequestBody::AgentItemBody {
                    thread,
                    item: item(),
                    stream: StreamKind::CommandOutput,
                    offset: 262_144,
                    limit: 262_144,
                },
            },
            r#"{"id":6,"body":{"type":"agent_item_body","thread":"11111111-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","stream":{"type":"command_output"},"offset":262144,"limit":262144}}"#,
        ),
        (
            Request {
                id: 7,
                body: RequestBody::AgentThreadClose { thread },
            },
            r#"{"id":7,"body":{"type":"agent_thread_close","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 71,
                body: RequestBody::AgentThreadReopen { thread },
            },
            r#"{"id":71,"body":{"type":"agent_thread_reopen","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 8,
                body: RequestBody::AgentSend {
                    thread,
                    input: UserInput {
                        text: "inspect the failing test".to_owned(),
                        attachments: Vec::new(),
                        item: None,
                        origin: Default::default(),
                    },
                },
            },
            r#"{"id":8,"body":{"type":"agent_send","thread":"11111111-2222-4333-8444-555555555555","input":{"text":"inspect the failing test","attachments":[]}}}"#,
        ),
        (
            Request {
                id: 9,
                body: RequestBody::AgentInterrupt { thread },
            },
            r#"{"id":9,"body":{"type":"agent_interrupt","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 10,
                body: RequestBody::AgentRespond {
                    thread,
                    gate: gate(),
                    answer: GateAnswer::Permission {
                        choice: PermissionChoice::AllowSession,
                        edited_payload: None,
                    },
                },
            },
            r#"{"id":10,"body":{"type":"agent_respond","thread":"11111111-2222-4333-8444-555555555555","gate":"cccccccc-2222-4333-8444-555555555555","answer":{"type":"permission","data":{"choice":"allow_session"}}}}"#,
        ),
        (
            Request {
                id: 11,
                body: RequestBody::AgentSetMode {
                    thread,
                    mode: PermissionMode::Plan,
                },
            },
            r#"{"id":11,"body":{"type":"agent_set_mode","thread":"11111111-2222-4333-8444-555555555555","mode":"plan"}}"#,
        ),
        (
            Request {
                id: 12,
                body: RequestBody::AgentSetModel {
                    thread,
                    model: model(),
                },
            },
            r#"{"id":12,"body":{"type":"agent_set_model","thread":"11111111-2222-4333-8444-555555555555","model":{"model":"gpt-5-codex","effort":"high"}}}"#,
        ),
        (
            Request {
                id: 13,
                body: RequestBody::AgentMarkSeen {
                    thread,
                    seq: Seq(12),
                },
            },
            r#"{"id":13,"body":{"type":"agent_mark_seen","thread":"11111111-2222-4333-8444-555555555555","seq":12}}"#,
        ),
        (
            Request {
                id: 14,
                body: RequestBody::AgentStop { thread },
            },
            r#"{"id":14,"body":{"type":"agent_stop","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 15,
                body: RequestBody::AgentCheckpoints { thread },
            },
            r#"{"id":15,"body":{"type":"agent_checkpoints","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 16,
                body: RequestBody::AgentRevert {
                    thread,
                    checkpoint: checkpoint(),
                },
            },
            r#"{"id":16,"body":{"type":"agent_revert","thread":"11111111-2222-4333-8444-555555555555","checkpoint":"00003-turn-aaaaaaaa-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 17,
                body: RequestBody::AgentSeenCursors,
            },
            r#"{"id":17,"body":{"type":"agent_seen_cursors"}}"#,
        ),
        (
            Request {
                id: 171,
                body: RequestBody::AgentClosedThreads,
            },
            r#"{"id":171,"body":{"type":"agent_closed_threads"}}"#,
        ),
        (
            Request {
                id: 18,
                body: RequestBody::AgentAccountLogin { thread },
            },
            r#"{"id":18,"body":{"type":"agent_account_login","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 19,
                body: RequestBody::AgentAccountLogout { thread },
            },
            r#"{"id":19,"body":{"type":"agent_account_logout","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 20,
                body: RequestBody::DelegationRun {
                    caller: thread,
                    provider: AgentKind::Codex,
                    brief: "write protocol goldens".to_owned(),
                    expectation: "all wire bytes are pinned".to_owned(),
                    worktree: Some(worktree()),
                    mode: Some(PermissionMode::FullAccess),
                    model: Some(model()),
                    title: Some("Golden writer".to_owned()),
                    fleet_path: Some("/opt/fleet/bin/fleet".to_owned()),
                    env: child_env(),
                    eager: true,
                },
            },
            r#"{"id":20,"body":{"type":"delegation_run","caller":"11111111-2222-4333-8444-555555555555","provider":"codex","brief":"write protocol goldens","expectation":"all wire bytes are pinned","worktree":"acme/api#native-agents","mode":"full_access","model":{"model":"gpt-5-codex","effort":"high"},"title":"Golden writer","fleet_path":"/opt/fleet/bin/fleet","env":{"CARGO_TARGET_DIR":"/work/target-child","RUSTFLAGS":"-D warnings"},"eager":true}}"#,
        ),
        (
            Request {
                id: 21,
                body: RequestBody::DelegationComplete {
                    delegation: delegation_id(),
                    child: child_thread(),
                    token: "completion-token".to_owned(),
                    result: "goldens added".to_owned(),
                    blocked: true,
                },
            },
            r#"{"id":21,"body":{"type":"delegation_complete","delegation":"dddddddd-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","token":"completion-token","result":"goldens added","blocked":true}}"#,
        ),
        (
            Request {
                id: 22,
                body: RequestBody::DelegationList {
                    caller: Some(thread),
                },
            },
            r#"{"id":22,"body":{"type":"delegation_list","caller":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 23,
                body: RequestBody::DelegationGet {
                    delegation: delegation_id(),
                },
            },
            r#"{"id":23,"body":{"type":"delegation_get","delegation":"dddddddd-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 24,
                body: RequestBody::DelegationCancel {
                    delegation: delegation_id(),
                },
            },
            r#"{"id":24,"body":{"type":"delegation_cancel","delegation":"dddddddd-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 25,
                body: RequestBody::DelegationWait {
                    delegation: delegation_id(),
                    timeout_ms: 30_000,
                    caller: Some(thread),
                },
            },
            r#"{"id":25,"body":{"type":"delegation_wait","delegation":"dddddddd-2222-4333-8444-555555555555","timeout_ms":30000,"caller":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        // The four delegation requests that carry optional fields, with every option absent: a
        // missing `skip_serializing_if` would show up here as a `null` or a `false` a version-7
        // peer never sent, and nowhere else.
        (
            Request {
                id: 26,
                body: RequestBody::DelegationRun {
                    caller: thread,
                    provider: AgentKind::Claude,
                    brief: "summarize the diff".to_owned(),
                    expectation: "one paragraph".to_owned(),
                    worktree: None,
                    mode: None,
                    model: None,
                    title: None,
                    fleet_path: None,
                    env: BTreeMap::new(),
                    eager: false,
                },
            },
            // Byte-identical to what a version-7 peer sent before `fleet_path` and `env` existed:
            // the literal is deliberately unchanged, and that is the whole proof both fields are
            // optional rather than merely defaulted.
            r#"{"id":26,"body":{"type":"delegation_run","caller":"11111111-2222-4333-8444-555555555555","provider":"claude","brief":"summarize the diff","expectation":"one paragraph"}}"#,
        ),
        (
            Request {
                id: 27,
                body: RequestBody::DelegationComplete {
                    delegation: delegation_id(),
                    child: child_thread(),
                    token: "completion-token".to_owned(),
                    result: "goldens added".to_owned(),
                    blocked: false,
                },
            },
            r#"{"id":27,"body":{"type":"delegation_complete","delegation":"dddddddd-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","token":"completion-token","result":"goldens added"}}"#,
        ),
        (
            Request {
                id: 28,
                body: RequestBody::DelegationList { caller: None },
            },
            r#"{"id":28,"body":{"type":"delegation_list"}}"#,
        ),
        (
            Request {
                id: 31,
                body: RequestBody::DelegationWait {
                    delegation: delegation_id(),
                    timeout_ms: 30_000,
                    caller: None,
                },
            },
            // Byte-identical to what a version-7 peer sent before `caller` existed, which is the
            // whole proof that a waiter with no session of its own is still the old shape.
            r#"{"id":31,"body":{"type":"delegation_wait","delegation":"dddddddd-2222-4333-8444-555555555555","timeout_ms":30000}}"#,
        ),
        // Delivery of a child's result to its caller is an ordinary `AgentSend` whose input is
        // marked; request 8 above pins the same shape with the mark absent.
        (
            Request {
                id: 29,
                body: RequestBody::AgentSend {
                    thread,
                    input: UserInput {
                        text: "goldens added".to_owned(),
                        attachments: Vec::new(),
                        item: Some(item()),
                        origin: MessageOrigin::Delegation {
                            id: delegation_id(),
                        },
                    },
                },
            },
            r#"{"id":29,"body":{"type":"agent_send","thread":"11111111-2222-4333-8444-555555555555","input":{"text":"goldens added","attachments":[],"item":"bbbbbbbb-2222-4333-8444-555555555555","origin":{"type":"delegation","data":{"id":"dddddddd-2222-4333-8444-555555555555"}}}}}"#,
        ),
    ]
}

fn response_goldens() -> Vec<(Response, &'static str)> {
    let projection = ThreadProjection::new(thread(), worktree(), AgentKind::Codex);
    vec![
        (
            Response {
                id: 1,
                result: Ok(ResponseBody::AgentThreads(vec![summary()])),
            },
            r#"{"id":1,"result":{"Ok":{"type":"agent_threads","data":[{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}]}}}"#,
        ),
        (
            Response {
                id: 2,
                result: Ok(ResponseBody::AgentThreadCreated(summary())),
            },
            r#"{"id":2,"result":{"Ok":{"type":"agent_thread_created","data":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}}}}"#,
        ),
        (
            Response {
                id: 3,
                result: Ok(ResponseBody::AgentThreadSnapshot {
                    projection,
                    events_after: vec![seq(1, None, AgentEvent::Notice("ready".to_owned()))],
                }),
            },
            r#"{"id":3,"result":{"Ok":{"type":"agent_thread_snapshot","data":{"projection":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","session":{"type":"starting"},"turn":{"type":"none"},"gates":[],"items":[],"turns":[],"backgroundTasks":[],"lastSeq":0,"lastActivity":null,"cumulativeUsage":{"inputTokens":0,"outputTokens":0,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"cumulativeCostUsd":null,"contextPct":0.0,"model":null,"mode":"ask","exitCode":null,"retrying":null},"events_after":[{"seq":1,"at":"2026-09-07T12:00:00Z","event":{"type":"notice","data":"ready"}}]}}}}"#,
        ),
        (
            Response {
                id: 4,
                result: Ok(ResponseBody::AgentThreadWindow(Box::new(
                    AgentThreadWindow {
                        summary: summary(),
                        session: AgentSessionView {
                            model: Some(model()),
                            mode: PermissionMode::Ask,
                            tools: vec!["shell".to_owned()],
                            commands: vec!["/review".to_owned()],
                            ..AgentSessionView::default()
                        },
                        window: TranscriptWindow {
                            turns: Vec::new(),
                            items: vec![window_item()],
                            checkpoints: Vec::new(),
                            notices: Vec::new(),
                            gates: Vec::new(),
                            elided: vec![item()],
                        },
                        page: Some(TranscriptPage {
                            before_cursor: Some(
                                "fat.1.11111111-2222-4333-8444-555555555555.90".to_owned(),
                            ),
                            has_more: true,
                            thread_seq: Seq(12),
                        }),
                        head_seq: Seq(12),
                        projected_seq: Seq(12),
                        seen_seq: Some(Seq(9)),
                        events_after: Vec::new(),
                        synchronized: true,
                    },
                ))),
            },
            r#"{"id":4,"result":{"Ok":{"type":"agent_thread_window","data":{"summary":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null},"session":{"model":{"model":"gpt-5-codex","effort":"high"},"mode":"ask","tools":["shell"],"commands":["/review"]},"window":{"items":[{"id":"bbbbbbbb-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","kind":{"type":"assistant_text","data":{"text":"done"}},"status":"completed","children":[],"started":"2026-09-07T12:00:00Z","ended":"2026-09-07T12:00:00Z"}],"elided":["bbbbbbbb-2222-4333-8444-555555555555"]},"page":{"beforeCursor":"fat.1.11111111-2222-4333-8444-555555555555.90","hasMore":true,"threadSeq":12},"headSeq":12,"projectedSeq":12,"seenSeq":9,"synchronized":true}}}}"#,
        ),
        (
            Response {
                id: 18,
                result: Ok(ResponseBody::AgentSeenCursors(vec![AgentSeenCursor {
                    thread: thread(),
                    seq: Seq(9),
                }])),
            },
            r#"{"id":18,"result":{"Ok":{"type":"agent_seen_cursors","data":[{"thread":"11111111-2222-4333-8444-555555555555","seq":9}]}}}"#,
        ),
        (
            Response {
                id: 181,
                result: Ok(ResponseBody::AgentClosedThreads(vec![thread()])),
            },
            r#"{"id":181,"result":{"Ok":{"type":"agent_closed_threads","data":["11111111-2222-4333-8444-555555555555"]}}}"#,
        ),
        (
            Response {
                id: 5,
                result: Ok(ResponseBody::AgentItemBodyChunk {
                    thread: thread(),
                    item: item(),
                    stream: StreamKind::CommandOutput,
                    offset: 262_144,
                    total: 1_048_576,
                    text: "…tail".to_owned(),
                }),
            },
            r#"{"id":5,"result":{"Ok":{"type":"agent_item_body_chunk","data":{"thread":"11111111-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","stream":{"type":"command_output"},"offset":262144,"total":1048576,"text":"…tail"}}}}"#,
        ),
        (
            Response {
                id: 6,
                result: Ok(ResponseBody::AgentAck),
            },
            r#"{"id":6,"result":{"Ok":{"type":"agent_ack"}}}"#,
        ),
        (
            Response {
                id: 9,
                result: Ok(ResponseBody::AgentAccountLogin {
                    auth_url: "http://localhost:1455/auth/callback?state=abc".to_owned(),
                }),
            },
            r#"{"id":9,"result":{"Ok":{"type":"agent_account_login","data":{"auth_url":"http://localhost:1455/auth/callback?state=abc"}}}}"#,
        ),
        (
            Response {
                id: 7,
                result: Ok(ResponseBody::AgentCheckpoints(vec![TurnCheckpoint {
                    id: checkpoint(),
                    scope: CheckpointScope::Turn,
                    turn: turn(),
                    ordinal: 3,
                    at: at(),
                }])),
            },
            r#"{"id":7,"result":{"Ok":{"type":"agent_checkpoints","data":[{"id":"00003-turn-aaaaaaaa-2222-4333-8444-555555555555","scope":"turn","turn":"aaaaaaaa-2222-4333-8444-555555555555","ordinal":3,"at":"2026-09-07T12:00:00Z"}]}}}"#,
        ),
        (
            Response {
                id: 8,
                result: Ok(ResponseBody::AgentReverted(AgentRevertReport {
                    thread: thread(),
                    checkpoint: checkpoint(),
                    restored: 2,
                    deleted: 1,
                    paths: vec!["src/lib.rs".to_owned(), "src/new.rs".to_owned()],
                })),
            },
            r#"{"id":8,"result":{"Ok":{"type":"agent_reverted","data":{"thread":"11111111-2222-4333-8444-555555555555","checkpoint":"00003-turn-aaaaaaaa-2222-4333-8444-555555555555","restored":2,"deleted":1,"paths":["src/lib.rs","src/new.rs"]}}}}"#,
        ),
        (
            Response {
                id: 20,
                result: Ok(ResponseBody::DelegationStarted {
                    delegation: delegation(),
                    warning: Some("child started with a fallback model".to_owned()),
                }),
            },
            r#"{"id":20,"result":{"Ok":{"type":"delegation_started","data":{"delegation":{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","eager":true,"status":"succeeded","statusPayload":"reported complete","result":{"text":"goldens added","filesChanged":["crates/fleet-proto/tests/agent_compatibility.rs"],"source":"reported"},"nudges":1,"recoveries":0,"delivery":{"type":"delivered","data":{"seq":42,"turn":"aaaaaaaa-2222-4333-8444-555555555555"}},"created":"2026-09-07T12:00:00Z","finished":"2026-09-07T12:00:00Z","headline":"Pinned delegation wire shapes"},"warning":"child started with a fallback model"}}}}"#,
        ),
        (
            Response {
                id: 21,
                result: Ok(ResponseBody::Delegations(vec![delegation()])),
            },
            r#"{"id":21,"result":{"Ok":{"type":"delegations","data":[{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","eager":true,"status":"succeeded","statusPayload":"reported complete","result":{"text":"goldens added","filesChanged":["crates/fleet-proto/tests/agent_compatibility.rs"],"source":"reported"},"nudges":1,"recoveries":0,"delivery":{"type":"delivered","data":{"seq":42,"turn":"aaaaaaaa-2222-4333-8444-555555555555"}},"created":"2026-09-07T12:00:00Z","finished":"2026-09-07T12:00:00Z","headline":"Pinned delegation wire shapes"}]}}}"#,
        ),
        (
            Response {
                id: 22,
                result: Ok(ResponseBody::Delegation(delegation())),
            },
            r#"{"id":22,"result":{"Ok":{"type":"delegation","data":{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","eager":true,"status":"succeeded","statusPayload":"reported complete","result":{"text":"goldens added","filesChanged":["crates/fleet-proto/tests/agent_compatibility.rs"],"source":"reported"},"nudges":1,"recoveries":0,"delivery":{"type":"delivered","data":{"seq":42,"turn":"aaaaaaaa-2222-4333-8444-555555555555"}},"created":"2026-09-07T12:00:00Z","finished":"2026-09-07T12:00:00Z","headline":"Pinned delegation wire shapes"}}}}"#,
        ),
        // The answer to a `DelegationRun` that needed no warning, carrying the delegation as it
        // looks the instant it is created: every optional column still empty.
        (
            Response {
                id: 23,
                result: Ok(ResponseBody::DelegationStarted {
                    delegation: starting_delegation(),
                    warning: None,
                }),
            },
            r#"{"id":23,"result":{"Ok":{"type":"delegation_started","data":{"delegation":{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","status":"starting","nudges":0,"recoveries":0,"delivery":{"type":"pending"},"created":"2026-09-07T12:00:00Z"}}}}}"#,
        ),
    ]
}

fn event_goldens() -> Vec<(Event, &'static str)> {
    vec![
        (
            Event::Agent {
                thread: thread(),
                event: seq(
                    12,
                    Some("system.init"),
                    AgentEvent::Notice("ready".to_owned()),
                ),
            },
            r#"{"type":"agent","data":{"thread":"11111111-2222-4333-8444-555555555555","event":{"seq":12,"at":"2026-09-07T12:00:00Z","raw":"system.init","event":{"type":"notice","data":"ready"}}}}"#,
        ),
        (
            Event::AgentSummary(summary()),
            r#"{"type":"agent_summary","data":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}}"#,
        ),
        (
            Event::DelegationChanged(delegation()),
            r#"{"type":"delegation_changed","data":{"id":"dddddddd-2222-4333-8444-555555555555","caller":"11111111-2222-4333-8444-555555555555","callerTurn":"aaaaaaaa-2222-4333-8444-555555555555","callerItem":"bbbbbbbb-2222-4333-8444-555555555555","child":"22222222-3333-4444-8555-666666666666","provider":"codex","depth":1,"brief":"write protocol goldens","expectation":"all wire bytes are pinned","eager":true,"status":"succeeded","statusPayload":"reported complete","result":{"text":"goldens added","filesChanged":["crates/fleet-proto/tests/agent_compatibility.rs"],"source":"reported"},"nudges":1,"recoveries":0,"delivery":{"type":"delivered","data":{"seq":42,"turn":"aaaaaaaa-2222-4333-8444-555555555555"}},"created":"2026-09-07T12:00:00Z","finished":"2026-09-07T12:00:00Z","headline":"Pinned delegation wire shapes"}}"#,
        ),
        (
            Event::AgentResync {
                thread: thread(),
                from_seq: Seq(41),
            },
            r#"{"type":"agent_resync","data":{"thread":"11111111-2222-4333-8444-555555555555","from_seq":41}}"#,
        ),
        (
            Event::AgentSynchronized { thread: thread() },
            r#"{"type":"agent_synchronized","data":{"thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Event::AgentWindow { thread: thread() },
            r#"{"type":"agent_window","data":{"thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
    ]
}

fn seq_event_goldens() -> Vec<(SeqEvent, &'static str)> {
    vec![
        (
            seq(
                1,
                Some("system.init"),
                AgentEvent::SessionConfigured {
                    provider: AgentKind::Codex,
                    resume_cursor: Some("thread-1".to_owned()),
                    model: Some(model()),
                    models: Vec::new(),
                    mode: PermissionMode::Ask,
                    tools: vec!["shell".to_owned()],
                    commands: vec!["/review".to_owned()],
                    skills: Vec::new(),
                },
            ),
            r#"{"seq":1,"at":"2026-09-07T12:00:00Z","raw":"system.init","event":{"type":"session_configured","data":{"provider":"codex","resume_cursor":"thread-1","model":{"model":"gpt-5-codex","effort":"high"},"mode":"ask","tools":["shell"],"commands":["/review"],"skills":[]}}}"#,
        ),
        (
            seq(
                2,
                None,
                AgentEvent::MetadataChanged {
                    title: Some("protocol goldens".to_owned()),
                    mode: Some(PermissionMode::Plan),
                    model: None,
                    skills: None,
                },
            ),
            r#"{"seq":2,"at":"2026-09-07T12:00:00Z","event":{"type":"metadata_changed","data":{"title":"protocol goldens","mode":"plan"}}}"#,
        ),
        (
            seq(
                3,
                None,
                AgentEvent::SessionStateChanged(SessionState::Ready),
            ),
            r#"{"seq":3,"at":"2026-09-07T12:00:00Z","event":{"type":"session_state_changed","data":{"type":"ready"}}}"#,
        ),
        (
            seq(
                4,
                None,
                AgentEvent::SessionActivity {
                    phase: "compacting".to_owned(),
                },
            ),
            r#"{"seq":4,"at":"2026-09-07T12:00:00Z","event":{"type":"session_activity","data":{"phase":"compacting"}}}"#,
        ),
        (
            seq(
                5,
                None,
                AgentEvent::SessionExited {
                    code: Some(0),
                    expected: true,
                },
            ),
            r#"{"seq":5,"at":"2026-09-07T12:00:00Z","event":{"type":"session_exited","data":{"code":0,"expected":true}}}"#,
        ),
        (
            seq(
                6,
                None,
                AgentEvent::TurnStarted {
                    turn: turn(),
                    user_item: item(),
                },
            ),
            r#"{"seq":6,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","user_item":"bbbbbbbb-2222-4333-8444-555555555555"}}}"#,
        ),
        (
            seq(
                7,
                None,
                AgentEvent::TurnSettled {
                    turn: turn(),
                    outcome: TurnOutcome::Completed,
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 20,
                        ..Usage::default()
                    },
                    duration_ms: 4_200,
                    files_changed: vec![FileDelta {
                        path: std::path::PathBuf::from("crates/fleet-proto/src/agents.rs"),
                        added: 12,
                        removed: 3,
                    }],
                },
            ),
            r#"{"seq":7,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_settled","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","outcome":{"type":"completed"},"usage":{"inputTokens":100,"outputTokens":20,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"duration_ms":4200,"files_changed":[{"path":"crates/fleet-proto/src/agents.rs","added":12,"removed":3}]}}}"#,
        ),
        (
            seq(
                8,
                None,
                AgentEvent::TurnAborted {
                    turn: turn(),
                    reason: AbortReason::User,
                },
            ),
            r#"{"seq":8,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_aborted","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","reason":{"type":"user"}}}}"#,
        ),
        (
            seq(
                9,
                None,
                AgentEvent::TurnDiff {
                    turn: turn(),
                    unified: "--- a\n+++ b\n".to_owned(),
                    files_changed: Vec::new(),
                },
            ),
            r#"{"seq":9,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_diff","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","unified":"--- a\n+++ b\n","files_changed":[]}}}"#,
        ),
        (
            seq(
                10,
                None,
                AgentEvent::PlanSteps {
                    turn: turn(),
                    steps: vec!["read the spec".to_owned()],
                },
            ),
            r#"{"seq":10,"at":"2026-09-07T12:00:00Z","event":{"type":"plan_steps","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","steps":["read the spec"]}}}"#,
        ),
        (
            seq(
                11,
                None,
                AgentEvent::ItemStarted {
                    turn: turn(),
                    item: item(),
                    kind: ItemKind::Tool(Box::new(tool_call())),
                    parent: None,
                },
            ),
            r#"{"seq":11,"at":"2026-09-07T12:00:00Z","event":{"type":"item_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","kind":{"type":"tool","data":{"kind":{"type":"bash"},"name":"shell","input":{"command":"cargo test"},"summary":"cargo test","output":"ok","exit_code":0,"duration_ms":1200}},"parent":null}}}"#,
        ),
        (
            seq(
                12,
                None,
                AgentEvent::ContentDelta {
                    item: item(),
                    stream: StreamKind::ReasoningSummary { part: 0 },
                    delta: "thinking".to_owned(),
                },
            ),
            r#"{"seq":12,"at":"2026-09-07T12:00:00Z","event":{"type":"content_delta","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","stream":{"type":"reasoning_summary","data":{"part":0}},"delta":"thinking"}}}"#,
        ),
        (
            seq(
                13,
                None,
                AgentEvent::ItemUpdated {
                    item: item(),
                    patch: ItemPatch {
                        payload: Some(ItemPayloadPatch::AssistantText {
                            text: "done".to_owned(),
                        }),
                        status: Some(ItemStatus::Completed),
                    },
                },
            ),
            r#"{"seq":13,"at":"2026-09-07T12:00:00Z","event":{"type":"item_updated","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","patch":{"payload":{"type":"assistant_text","data":{"text":"done"}},"status":"completed"}}}}"#,
        ),
        (
            seq(
                14,
                None,
                AgentEvent::ItemCompleted {
                    item: item(),
                    status: ItemStatus::Completed,
                },
            ),
            r#"{"seq":14,"at":"2026-09-07T12:00:00Z","event":{"type":"item_completed","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","status":"completed"}}}"#,
        ),
        (
            seq(
                15,
                None,
                AgentEvent::GateOpened {
                    gate: gate(),
                    turn: Some(turn()),
                    kind: GateKind::Question {
                        questions: vec![Question {
                            id: "store".to_owned(),
                            header: "Storage".to_owned(),
                            prompt: "Which store?".to_owned(),
                            options: vec![QuestionOption {
                                id: ProviderOptionId("sqlite".to_owned()),
                                label: "SQLite".to_owned(),
                                description: "One file per daemon".to_owned(),
                            }],
                            multi_select: false,
                            allows_other: true,
                            is_secret: false,
                            blocking: true,
                        }],
                    },
                },
            ),
            r#"{"seq":15,"at":"2026-09-07T12:00:00Z","event":{"type":"gate_opened","data":{"gate":"cccccccc-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","kind":{"type":"question","data":{"questions":[{"id":"store","header":"Storage","prompt":"Which store?","options":[{"id":"sqlite","label":"SQLite","description":"One file per daemon"}],"multiSelect":false,"allowsOther":true,"isSecret":false,"blocking":true}]}}}}}"#,
        ),
        (
            seq(
                16,
                None,
                AgentEvent::GateResolved {
                    gate: gate(),
                    answer: GateAnswer::Plan(PlanAnswer::Approve),
                    by: GateResolver::User,
                },
            ),
            r#"{"seq":16,"at":"2026-09-07T12:00:00Z","event":{"type":"gate_resolved","data":{"gate":"cccccccc-2222-4333-8444-555555555555","answer":{"type":"plan","data":{"type":"approve"}},"by":"user"}}}"#,
        ),
        (
            seq(17, None, AgentEvent::GateWithdrawn { gate: gate() }),
            r#"{"seq":17,"at":"2026-09-07T12:00:00Z","event":{"type":"gate_withdrawn","data":{"gate":"cccccccc-2222-4333-8444-555555555555"}}}"#,
        ),
        (
            seq(
                18,
                None,
                AgentEvent::PlanProposed {
                    gate: gate(),
                    turn: turn(),
                    markdown: "Plan\n- add the goldens\n".to_owned(),
                    steps: vec!["add the goldens".to_owned()],
                },
            ),
            r#"{"seq":18,"at":"2026-09-07T12:00:00Z","event":{"type":"plan_proposed","data":{"gate":"cccccccc-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","markdown":"Plan\n- add the goldens\n","steps":["add the goldens"]}}}"#,
        ),
        (
            seq(
                19,
                None,
                AgentEvent::TokenUsage {
                    turn: turn(),
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 20,
                        ..Usage::default()
                    },
                    context_pct: 12.5,
                    cost_usd: Some(0.25),
                },
            ),
            r#"{"seq":19,"at":"2026-09-07T12:00:00Z","event":{"type":"token_usage","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","usage":{"inputTokens":100,"outputTokens":20,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"context_pct":12.5,"cost_usd":0.25}}}"#,
        ),
        (
            seq(
                20,
                None,
                AgentEvent::RateLimits {
                    limits: serde_json::json!({"resetsAt": "2026-09-07T13:00:00Z", "used": 42}),
                },
            ),
            r#"{"seq":20,"at":"2026-09-07T12:00:00Z","event":{"type":"rate_limits","data":{"limits":{"resetsAt":"2026-09-07T13:00:00Z","used":42}}}}"#,
        ),
        (
            seq(
                21,
                None,
                AgentEvent::Compacted(CheckpointKind::CompactBoundary {
                    before: 180_000,
                    after: Some(40_000),
                }),
            ),
            r#"{"seq":21,"at":"2026-09-07T12:00:00Z","event":{"type":"compacted","data":{"type":"compact_boundary","data":{"before":180000,"after":40000}}}}"#,
        ),
        (
            seq(
                22,
                None,
                AgentEvent::Retrying {
                    attempt: 2,
                    retry_in_ms: 500,
                    reason: "overloaded".to_owned(),
                },
            ),
            r#"{"seq":22,"at":"2026-09-07T12:00:00Z","event":{"type":"retrying","data":{"attempt":2,"retry_in_ms":500,"reason":"overloaded"}}}"#,
        ),
        (
            seq(
                23,
                None,
                AgentEvent::ModelRerouted {
                    from: "gpt-5-codex".to_owned(),
                    to: "gpt-5".to_owned(),
                    reason: "capacity".to_owned(),
                },
            ),
            r#"{"seq":23,"at":"2026-09-07T12:00:00Z","event":{"type":"model_rerouted","data":{"from":"gpt-5-codex","to":"gpt-5","reason":"capacity"}}}"#,
        ),
        (
            seq(
                24,
                None,
                AgentEvent::RuntimeError {
                    fatal: false,
                    message: "stream reset".to_owned(),
                },
            ),
            r#"{"seq":24,"at":"2026-09-07T12:00:00Z","event":{"type":"runtime_error","data":{"fatal":false,"message":"stream reset"}}}"#,
        ),
        (
            // The sequence numbers here are fixture ids, not an ordering: this list is ordered by
            // the enum's *declaration*, which is what `AGENT_EVENT_TAGS` checks it against.
            seq(
                27,
                Some("account/read"),
                AgentEvent::AccountChanged {
                    account: AccountStatus::SignedIn(AccountInfo {
                        kind: AccountKind::ChatGpt {
                            email: Some("dev@example.com".to_owned()),
                            plan: Some("business".to_owned()),
                        },
                    }),
                },
            ),
            r#"{"seq":27,"at":"2026-09-07T12:00:00Z","raw":"account/read","event":{"type":"account_changed","data":{"account":{"type":"signed_in","data":{"kind":{"type":"chatgpt","data":{"email":"dev@example.com","plan":"business"}}}}}}}"#,
        ),
        (
            seq(25, None, AgentEvent::Notice("deprecated flag".to_owned())),
            r#"{"seq":25,"at":"2026-09-07T12:00:00Z","event":{"type":"notice","data":"deprecated flag"}}"#,
        ),
        (
            seq(
                26,
                Some("codex/unknownMethod"),
                AgentEvent::Unknown {
                    method: "codex/unknownMethod".to_owned(),
                },
            ),
            r#"{"seq":26,"at":"2026-09-07T12:00:00Z","raw":"codex/unknownMethod","event":{"type":"unknown","data":{"method":"codex/unknownMethod"}}}"#,
        ),
    ]
}
