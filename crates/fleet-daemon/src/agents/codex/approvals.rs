//! Server→client requests: the gates, and the answers Fleet writes back.
//!
//! Ten methods exist; **Fleet implements five and answers the rest `-32601`**, logging loudly on
//! the legacy `applyPatchApproval`/`execCommandApproval` pair because seeing it means Fleet took
//! the legacy v1 path — a version-gate failure by definition.
//!
//! Three rules here are the difference between a card that works and one that lies:
//!
//! - **Gate keys are the JSON-RPC request id.** t3code keys on `approvalId ?? itemId` and that
//!   collides: `approvalId` may be null and several approvals can share one `itemId` for
//!   zsh-exec-bridge subcommands.
//! - **`availableDecisions` is authoritative and ordered.** Fleet renders exactly the options
//!   Codex offers, in Codex's order, and never invents one. Where it is absent (an older CLI) it
//!   falls back to `[accept, acceptForSession, decline, cancel]`.
//! - **The file-change approval carries no diff and no paths.** The changes live on the
//!   `fileChange` item named by `itemId`, so the card joins on the item and never renders
//!   `reason` as if it were the change.

use fleet_core::agents::{
    AgentEvent, GateAnswer, GateId, GateKind, PermissionChoice, PermissionOption, ProviderOptionId,
    Question, QuestionOption, ToolKind,
};
use serde_json::{Value, json};

use super::{
    map::decode,
    session::{ApprovalShape, CodexSession, PendingApproval, gate_id},
    wire::{
        CommandExecutionRequestApprovalParams, FileChangeRequestApprovalParams,
        McpServerElicitationRequestParams, PermissionsRequestApprovalParams,
        ToolRequestUserInputParams,
    },
};
use crate::agents::harness::{
    HarnessError, HarnessResult, ProtocolOp,
    fingerprint::{IssueKind, SchemaFingerprint},
};

/// The decisions Fleet offers when `availableDecisions` is absent.
pub const DEFAULT_DECISIONS: [&str; 4] = ["accept", "acceptForSession", "decline", "cancel"];

/// What handling one server request produced.
#[derive(Debug, Default)]
pub(super) struct ApprovalOutcome {
    /// Events to publish.
    pub(super) events: Vec<AgentEvent>,
    /// An immediate answer, when Fleet answers without asking the user.
    pub(super) immediate: Option<Value>,
    /// A JSON-RPC error to answer with instead.
    pub(super) error: Option<(i64, &'static str)>,
}

/// Handles one inbound server request.
pub(super) fn handle(
    session: &mut CodexSession,
    method: &str,
    id: &Value,
    params: &Value,
) -> ApprovalOutcome {
    match method {
        "item/commandExecution/requestApproval" => command_execution(session, id, params),
        "item/fileChange/requestApproval" => file_change(session, id, params),
        "item/tool/requestUserInput" => questions(session, id, params),
        "item/permissions/requestApproval" => permissions(session, id, params),
        "mcpServer/elicitation/request" => elicitation(session, id, params),
        "applyPatchApproval" | "execCommandApproval" => {
            // A refusal, not a fallback: the v1 pair arriving means the server took the legacy
            // path, and answering it would let a legacy approval look like a v2 one.
            session.legacy_approval_seen = true;
            tracing::error!(
                target: "fleet::agents::codex",
                method,
                "Codex issued a legacy v1 approval request; Fleet speaks v2 only"
            );
            ApprovalOutcome {
                // Refused *and* surfaced: the thread cannot be trusted to gate anything once the
                // server has taken the legacy path, so the user is told rather than left with a
                // card that never appears.
                events: vec![AgentEvent::RuntimeError {
                    fatal: true,
                    message: "This Codex build asked for approval over its legacy protocol,                               which Fleet does not speak. Upgrade Codex, or open it in a                               terminal on this worktree."
                        .to_owned(),
                }],
                error: Some((
                    super::envelope::METHOD_NOT_FOUND,
                    "Fleet implements the v2 approval requests only",
                )),
                immediate: None,
            }
        }
        other => {
            tracing::debug!(
                target: "fleet::agents::codex",
                method = other,
                "answering an unimplemented Codex server request"
            );
            ApprovalOutcome {
                error: Some((
                    super::envelope::METHOD_NOT_FOUND,
                    "Fleet does not implement this request",
                )),
                ..ApprovalOutcome::default()
            }
        }
    }
}

fn command_execution(session: &mut CodexSession, id: &Value, params: &Value) -> ApprovalOutcome {
    let request: CommandExecutionRequestApprovalParams =
        match decode("item/commandExecution/requestApproval", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded_outcome(degraded),
        };
    let decisions = decision_names(request.available_decisions.as_deref());
    if request.available_decisions.is_some() {
        session.available_decisions_seen = true;
    }
    let gate = gate_id(&request.thread_id, id);
    // The payload is the command, read from `commandActions` where Codex parsed it and from the
    // raw command line only when it did not.
    let payload = request
        .command_actions
        .as_deref()
        .and_then(|actions| actions.first().map(action_command))
        .or_else(|| request.command.clone())
        .unwrap_or_default();
    let title = request.cwd.as_deref().map_or_else(
        || "Codex wants to run a command".to_owned(),
        |cwd| format!("Codex wants to run a command in {cwd}"),
    );
    let options = permission_options(&decisions, request.proposed_execpolicy_amendment.is_some());
    session.open_gate(
        gate,
        PendingApproval {
            request_id: id.clone(),
            thread: request.thread_id.clone(),
            item: Some(request.item_id.clone()),
            shape: ApprovalShape::CommandExecution,
            decisions,
        },
    );
    ApprovalOutcome {
        events: vec![AgentEvent::GateOpened {
            gate,
            turn: Some(session.turn_for(&request.turn_id)),
            kind: GateKind::Permission {
                tool: ToolKind::Bash,
                title,
                payload,
                rationale: request.reason.clone(),
                options,
            },
        }],
        ..ApprovalOutcome::default()
    }
}

fn file_change(session: &mut CodexSession, id: &Value, params: &Value) -> ApprovalOutcome {
    let request: FileChangeRequestApprovalParams =
        match decode("item/fileChange/requestApproval", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded_outcome(degraded),
        };
    let gate = gate_id(&request.thread_id, id);
    let decisions: Vec<String> = DEFAULT_DECISIONS.iter().map(ToString::to_string).collect();
    // No diff, no paths: the card joins on `itemId` and shows a loading state until the item
    // arrives. `reason` is a rationale and is never rendered as the change.
    let payload = session
        .items
        .get(&request.item_id)
        .map_or_else(String::new, |item| item.to_string());
    session.open_gate(
        gate,
        PendingApproval {
            request_id: id.clone(),
            thread: request.thread_id.clone(),
            item: Some(request.item_id.clone()),
            shape: ApprovalShape::FileChange,
            decisions: decisions.clone(),
        },
    );
    ApprovalOutcome {
        events: vec![AgentEvent::GateOpened {
            gate,
            turn: Some(session.turn_for(&request.turn_id)),
            kind: GateKind::Permission {
                tool: ToolKind::Edit,
                title: "Codex wants to apply a patch".to_owned(),
                payload,
                rationale: request.reason.clone(),
                options: permission_options(&decisions, false),
            },
        }],
        ..ApprovalOutcome::default()
    }
}

fn questions(session: &mut CodexSession, id: &Value, params: &Value) -> ApprovalOutcome {
    let request: ToolRequestUserInputParams = match decode("item/tool/requestUserInput", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded_outcome(degraded),
    };
    let gate = gate_id(&request.thread_id, id);
    let ids = request
        .questions
        .iter()
        .map(|question| question.id.clone())
        .collect::<Vec<_>>();
    let normalized = request
        .questions
        .iter()
        .map(|question| Question {
            // Answers are keyed by question **id** on Codex and by the exact question text on
            // Claude, so the id is the identity and the text rides alongside it.
            id: question.id.clone(),
            header: question.header.clone(),
            prompt: question.question.clone(),
            options: question
                .options
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|option| QuestionOption {
                    // The answer carries option **labels**, not indices.
                    id: ProviderOptionId(option.label.clone()),
                    label: option.label,
                    description: option.description,
                })
                .collect(),
            // No flag: `answers: string[]` makes multi-select implicit, so a question with
            // options is offered as single-select unless the model asked for free text too.
            multi_select: false,
            allows_other: question.is_other.unwrap_or(false) || question.options.is_none(),
            // A credential: masked input, never persisted, never logged.
            is_secret: question.is_secret.unwrap_or(false),
            blocking: request.is_blocking,
        })
        .collect::<Vec<_>>();
    session.open_gate(
        gate,
        PendingApproval {
            request_id: id.clone(),
            thread: request.thread_id.clone(),
            item: Some(request.item_id.clone()),
            shape: ApprovalShape::Questions {
                ids,
                blocking: request.is_blocking,
            },
            decisions: Vec::new(),
        },
    );
    if let Some(auto) = request.auto_resolution_ms {
        tracing::debug!(
            target: "fleet::agents::codex",
            auto_resolution_ms = auto,
            "Codex will resolve this question itself if nobody answers"
        );
    }
    ApprovalOutcome {
        events: vec![AgentEvent::GateOpened {
            gate,
            turn: Some(session.turn_for(&request.turn_id)),
            kind: GateKind::Question {
                questions: normalized,
            },
        }],
        ..ApprovalOutcome::default()
    }
}

fn permissions(session: &mut CodexSession, id: &Value, params: &Value) -> ApprovalOutcome {
    let request: PermissionsRequestApprovalParams =
        match decode("item/permissions/requestApproval", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded_outcome(degraded),
        };
    let gate = gate_id(&request.thread_id, id);
    let profile = serde_json::to_value(&request.permissions).unwrap_or(Value::Null);
    // A *negotiation*, not a yes/no: the client returns a possibly-narrowed profile. v1 offers
    // grant-as-asked and decline, and records the narrowing seam.
    let decisions = vec!["accept".to_owned(), "decline".to_owned()];
    session.open_gate(
        gate,
        PendingApproval {
            request_id: id.clone(),
            thread: request.thread_id.clone(),
            item: Some(request.item_id.clone()),
            shape: ApprovalShape::Permissions {
                profile: profile.clone(),
            },
            decisions: decisions.clone(),
        },
    );
    ApprovalOutcome {
        events: vec![AgentEvent::GateOpened {
            gate,
            turn: Some(session.turn_for(&request.turn_id)),
            kind: GateKind::Permission {
                tool: ToolKind::Unknown {
                    name: "permissions".to_owned(),
                },
                title: "Codex wants to widen its permissions".to_owned(),
                payload: serde_json::to_string_pretty(&profile).unwrap_or_default(),
                rationale: request.reason.clone(),
                options: permission_options(&decisions, false),
            },
        }],
        ..ApprovalOutcome::default()
    }
}

/// MCP elicitation, with the two rules that keep an unanswerable prompt off the screen.
///
/// **URL-mode elicitations are auto-declined and never shown** — Fleet does not open URLs on the
/// user's behalf — and an elicitation whose accept response cannot be constructed is auto-declined
/// with a warning rather than shown, because presenting a prompt that cannot be answered is worse
/// than declining it.
fn elicitation(session: &mut CodexSession, id: &Value, params: &Value) -> ApprovalOutcome {
    // The mode is read **before** the typed decode: a URL-mode elicitation is declined without
    // ever being shown, and that must hold even for a payload this build cannot decode.
    let mode = params
        .pointer("/elicitation/mode")
        .or_else(|| params.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("form");
    if mode == "url" {
        tracing::debug!(
            target: "fleet::agents::codex",
            "auto-declining a URL-mode MCP elicitation"
        );
        return ApprovalOutcome {
            immediate: Some(json!({"action": "decline"})),
            ..ApprovalOutcome::default()
        };
    }
    let request: McpServerElicitationRequestParams =
        match decode("mcpServer/elicitation/request", params) {
            Ok(decoded) => decoded,
            // An elicitation whose accept response cannot be constructed is auto-declined with a
            // warning rather than shown: never present a prompt that cannot be answered.
            Err(degraded) => return degraded_outcome(degraded),
        };
    let app = app_name(params);
    let gate = gate_id(&request.thread_id, id);
    let decisions = vec![
        "accept".to_owned(),
        "decline".to_owned(),
        "cancel".to_owned(),
    ];
    session.open_gate(
        gate,
        PendingApproval {
            request_id: id.clone(),
            thread: request.thread_id.clone(),
            item: None,
            shape: ApprovalShape::Elicitation,
            decisions: decisions.clone(),
        },
    );
    ApprovalOutcome {
        events: vec![AgentEvent::GateOpened {
            gate,
            turn: None,
            kind: GateKind::Permission {
                tool: ToolKind::Mcp {
                    server: app.clone(),
                },
                title: format!("Allow Codex to use {app}?"),
                payload: params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                rationale: None,
                options: permission_options(&decisions, false),
            },
        }],
        ..ApprovalOutcome::default()
    }
}

/// The app-name resolution ladder, copied exactly.
///
/// It is the difference between "Allow ChatGPT to use Safari?" and "Allow computer-use?".
fn app_name(params: &Value) -> String {
    let direct = [
        "/_meta/app_name",
        "/appName",
        "/app",
        "/target/app",
        "/target/name",
        "/tool_params/app_name",
        "/tool_params/app",
    ]
    .into_iter()
    .find_map(|pointer| params.pointer(pointer).and_then(Value::as_str));
    if let Some(name) = direct {
        return name.to_owned();
    }
    if let Some(message) = params.get("message").and_then(Value::as_str)
        && let Some(rest) = message.strip_prefix("Allow ChatGPT to use ")
        && let Some(name) = rest.strip_suffix('?')
    {
        return name.to_owned();
    }
    ["/connector_name", "/connectorName", "/serverName"]
        .into_iter()
        .find_map(|pointer| params.pointer(pointer).and_then(Value::as_str))
        .unwrap_or("this app")
        .to_owned()
}

/// The wire answer for one gate decision, and the events that go with it.
pub(super) fn answer(
    pending: &PendingApproval,
    gate: GateId,
    answer: &GateAnswer,
) -> HarnessResult<Value> {
    match (&pending.shape, answer) {
        (
            ApprovalShape::CommandExecution | ApprovalShape::FileChange,
            GateAnswer::Permission { choice, .. },
        ) => {
            let decision = decision_for(*choice, &pending.decisions).ok_or_else(|| {
                // A decision Codex did not offer is never invented: the card is re-rendered from
                // `availableDecisions` and the answer refused rather than downgraded silently.
                HarnessError::Protocol {
                    op: ProtocolOp::Respond,
                    method: Some("approval".to_owned()),
                    fingerprint: SchemaFingerprint::of_value(IssueKind::Shape, &Value::Null),
                }
            })?;
            Ok(json!({"decision": decision}))
        }
        (ApprovalShape::Questions { ids, .. }, GateAnswer::Question { answers }) => {
            let mut map = serde_json::Map::new();
            for (id, values) in ids.iter().zip(answers) {
                map.insert(id.clone(), json!({"answers": values}));
            }
            Ok(json!({"answers": map}))
        }
        (ApprovalShape::Permissions { profile }, GateAnswer::Permission { choice, .. }) => {
            if matches!(
                choice,
                PermissionChoice::Deny | PermissionChoice::DenyAndStop
            ) {
                // Declining a widening request is an empty grant, which is what "no" means here.
                return Ok(json!({"permissions": {}, "scope": "turn"}));
            }
            Ok(json!({
                "permissions": profile,
                "scope": if matches!(choice, PermissionChoice::AllowSession) { "session" } else { "turn" },
            }))
        }
        (ApprovalShape::Elicitation, GateAnswer::Permission { choice, .. }) => {
            let action = match choice {
                PermissionChoice::AllowOnce
                | PermissionChoice::AllowSession
                | PermissionChoice::AllowDirectory
                | PermissionChoice::Edit => "accept",
                PermissionChoice::Deny => "decline",
                PermissionChoice::DenyAndStop => "cancel",
            };
            Ok(json!({"action": action}))
        }
        // An async question has no request behind it: the answer travels as a new message.
        (ApprovalShape::AsyncQuestions { .. }, _) => Err(HarnessError::GateGone { gate }),
        _ => Err(HarnessError::Protocol {
            op: ProtocolOp::Respond,
            method: Some("approval".to_owned()),
            fingerprint: SchemaFingerprint::of_value(IssueKind::Shape, &Value::Null),
        }),
    }
}

/// The Codex decision one Fleet choice maps to, **only if Codex offered it**.
fn decision_for(choice: PermissionChoice, offered: &[String]) -> Option<Value> {
    let name = match choice {
        PermissionChoice::AllowOnce | PermissionChoice::Edit => "accept",
        PermissionChoice::AllowSession | PermissionChoice::AllowDirectory => "acceptForSession",
        PermissionChoice::Deny => "decline",
        PermissionChoice::DenyAndStop => "cancel",
    };
    offered
        .iter()
        .any(|offered| offered == name)
        .then(|| Value::String(name.to_owned()))
}

/// The card's options, in **Codex's** order.
fn permission_options(decisions: &[String], amendment: bool) -> Vec<PermissionOption> {
    let mut options = decisions
        .iter()
        .filter_map(|decision| {
            let choice = match decision.as_str() {
                "accept" => PermissionChoice::AllowOnce,
                "acceptForSession" => PermissionChoice::AllowSession,
                "decline" => PermissionChoice::Deny,
                "cancel" => PermissionChoice::DenyAndStop,
                // An amendment variant is offered separately, below, because it needs the
                // server's own proposal to echo.
                _ => return None,
            };
            Some(PermissionOption {
                id: ProviderOptionId(decision.clone()),
                label: choice,
            })
        })
        .collect::<Vec<_>>();
    if amendment
        && decisions
            .iter()
            .any(|decision| decision == "acceptWithExecpolicyAmendment")
    {
        // Fleet never *invents* an amendment; it echoes the server's own proposal. Offering it is
        // a real affordance the user is otherwise denied.
        options.push(PermissionOption {
            id: ProviderOptionId("acceptWithExecpolicyAmendment".to_owned()),
            label: PermissionChoice::AllowDirectory,
        });
    }
    options
}

/// The names of the decisions Codex offered, in order, or the four-decision default.
fn decision_names(
    available: Option<&[crate::agents::codex::wire::CommandExecutionApprovalDecision]>,
) -> Vec<String> {
    let Some(available) = available else {
        return DEFAULT_DECISIONS.iter().map(ToString::to_string).collect();
    };
    if available.is_empty() {
        return DEFAULT_DECISIONS.iter().map(ToString::to_string).collect();
    }
    available.iter().map(decision_name).collect()
}

fn decision_name(
    decision: &crate::agents::codex::wire::CommandExecutionApprovalDecision,
) -> String {
    use crate::agents::codex::wire::CommandExecutionApprovalDecisionKnown as Known;

    if let Some(tag) = decision.unknown_tag() {
        return tag.to_owned();
    }
    match decision.known() {
        Some(Known::Accept) => "accept",
        Some(Known::AcceptForSession) => "acceptForSession",
        Some(Known::AcceptWithExecpolicyAmendment(_)) => "acceptWithExecpolicyAmendment",
        Some(Known::ApplyNetworkPolicyAmendment(_)) => "applyNetworkPolicyAmendment",
        Some(Known::Decline) => "decline",
        Some(Known::Cancel) => "cancel",
        None => "unknown",
    }
    .to_owned()
}

/// The renderable command of one action.
fn action_command(action: &crate::agents::codex::wire::CommandAction) -> String {
    use crate::agents::codex::wire::CommandAction as Action;

    match action {
        Action::Read { command, .. }
        | Action::ListFiles { command, .. }
        | Action::Search { command, .. }
        | Action::Unknown { command } => command.clone(),
        Action::Unrecognized => String::new(),
    }
}

fn degraded_outcome(degraded: super::map::MapOutput) -> ApprovalOutcome {
    ApprovalOutcome {
        events: degraded.events,
        // A request Fleet cannot decode is declined rather than left hanging: the agent gets an
        // answer, the turn continues, and the transcript carries the degraded event.
        immediate: Some(json!({"decision": "decline"})),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::codex::session::CodexSession;

    fn session() -> CodexSession {
        CodexSession {
            root: Some("t".to_owned()),
            ..CodexSession::default()
        }
    }

    /// `availableDecisions` is authoritative and ordered, and Fleet never invents one.
    #[test]
    fn only_the_offered_decisions_are_rendered_and_in_codexs_order() {
        let mut session = session();
        let params = serde_json::json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "itemId": "exec-1",
            "startedAtMs": 1,
            "command": "/usr/bin/bash -lc \"rm -rf build\"",
            "commandActions": [{"type": "unknown", "command": "rm -rf build"}],
            "availableDecisions": ["decline", "accept"],
        });
        let outcome = handle(
            &mut session,
            "item/commandExecution/requestApproval",
            &serde_json::json!(4),
            &params,
        );
        let Some(AgentEvent::GateOpened { gate, kind, .. }) = outcome.events.first() else {
            panic!("expected a gate");
        };
        let GateKind::Permission {
            options, payload, ..
        } = kind
        else {
            panic!("expected a permission gate");
        };
        assert_eq!(
            options
                .iter()
                .map(|option| option.id.0.as_str())
                .collect::<Vec<_>>(),
            ["decline", "accept"],
            "Codex's order is the card's order"
        );
        // The payload is the parsed command, never the `bash -lc` wrapper.
        assert_eq!(payload, "rm -rf build");

        // An answer Codex did not offer is refused rather than downgraded.
        let pending = session
            .gates
            .get(gate)
            .unwrap_or_else(|| panic!("the gate is pending"))
            .clone();
        assert!(
            answer(
                &pending,
                *gate,
                &GateAnswer::Permission {
                    choice: PermissionChoice::AllowSession,
                    edited_payload: None,
                },
            )
            .is_err(),
            "acceptForSession was not offered"
        );
        assert_eq!(
            answer(
                &pending,
                *gate,
                &GateAnswer::Permission {
                    choice: PermissionChoice::Deny,
                    edited_payload: None,
                },
            )
            .unwrap_or_else(|error| panic!("{error}")),
            serde_json::json!({"decision": "decline"})
        );
    }

    #[test]
    fn an_absent_decision_list_degrades_to_the_four_decision_default() {
        assert_eq!(
            decision_names(None),
            ["accept", "acceptForSession", "decline", "cancel"]
        );
        assert_eq!(decision_names(Some(&[])).len(), 4);
    }

    /// Two approvals sharing one `itemId` are two gates, because the key is the request id.
    #[test]
    fn two_approvals_on_one_item_open_two_gates() {
        let mut session = session();
        let params = serde_json::json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "itemId": "exec-shared",
            "startedAtMs": 1,
            "approvalId": null,
            "command": "sh -lc 'a && b'",
        });
        handle(
            &mut session,
            "item/commandExecution/requestApproval",
            &serde_json::json!(1),
            &params,
        );
        handle(
            &mut session,
            "item/commandExecution/requestApproval",
            &serde_json::json!(2),
            &params,
        );
        assert_eq!(session.gates.len(), 2);
    }

    #[test]
    fn a_secret_question_is_marked_and_a_non_blocking_one_does_not_block() {
        let mut session = session();
        let params = serde_json::json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "itemId": "tool-1",
            "isBlocking": false,
            "questions": [{
                "id": "q1",
                "header": "Token",
                "question": "Paste the deploy token",
                "isSecret": true,
                "isOther": true,
                "options": null,
            }],
        });
        let outcome = handle(
            &mut session,
            "item/tool/requestUserInput",
            &serde_json::json!("r1"),
            &params,
        );
        let Some(AgentEvent::GateOpened {
            kind: GateKind::Question { questions },
            ..
        }) = outcome.events.first()
        else {
            panic!("expected a question gate");
        };
        let question = questions.first().unwrap_or_else(|| panic!("one question"));
        assert!(question.is_secret, "a credential must be masked");
        assert!(
            !question.blocking,
            "a non-blocking question never needs you"
        );
        assert!(question.allows_other);
        assert_eq!(question.id, "q1", "answers are keyed by question id");
    }

    #[test]
    fn the_legacy_pair_is_refused_and_latched() {
        let mut session = session();
        let outcome = handle(
            &mut session,
            "applyPatchApproval",
            &serde_json::json!(9),
            &serde_json::json!({}),
        );
        assert_eq!(
            outcome.error,
            Some((-32601, "Fleet implements the v2 approval requests only"))
        );
        assert!(session.legacy_approval_seen, "the version gate must latch");
        assert!(
            matches!(
                outcome.events.first(),
                Some(AgentEvent::RuntimeError { fatal: true, .. })
            ),
            "the refusal is surfaced, not only logged"
        );
    }

    #[test]
    fn a_url_mode_elicitation_is_declined_without_ever_being_shown() {
        let mut session = session();
        let outcome = handle(
            &mut session,
            "mcpServer/elicitation/request",
            &serde_json::json!(3),
            &serde_json::json!({
                "threadId": "t",
                "mode": "url",
                "message": "Allow ChatGPT to use Safari?",
            }),
        );
        assert!(outcome.events.is_empty(), "it must never be shown");
        assert_eq!(
            outcome.immediate,
            Some(serde_json::json!({"action": "decline"}))
        );
    }

    #[test]
    fn the_app_name_ladder_prefers_the_declared_name_then_the_message() {
        assert_eq!(
            app_name(&serde_json::json!({"_meta": {"app_name": "Safari"}})),
            "Safari"
        );
        assert_eq!(
            app_name(&serde_json::json!({"message": "Allow ChatGPT to use Safari?"})),
            "Safari"
        );
        assert_eq!(
            app_name(&serde_json::json!({"serverName": "computer-use"})),
            "computer-use"
        );
        assert_eq!(app_name(&serde_json::json!({})), "this app");
    }
}
