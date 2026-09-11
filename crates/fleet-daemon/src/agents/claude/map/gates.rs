//! Permission, question and plan gates.
//!
//! All three arrive as `control_request { subtype: "can_use_tool" }`; the **tool name is the
//! discriminator** and there is no separate frame kind. `AskUserQuestion` and `ExitPlanMode` are
//! intercepted **before** any permission-mode short circuit, because plan mode depends on
//! questions working even in a fully permissive mode.
//!
//! Two rules here are safety requirements rather than niceties:
//!
//! - **Scope rewriting.** The live suggestion came back `destination: "localSettings"`. Echoing
//!   it verbatim for a *session* choice writes a **permanent** rule into the user's
//!   `.claude/settings.local.json`. The session decision rewrites `destination` to `"session"`,
//!   and the persistent decision is offered separately, with copy that says so.
//! - **`ExitPlanMode` is always denied**, in every permission mode, with an instruction to stop
//!   and wait. The plan becomes a decision surface Fleet owns; leaving the model parked on a card
//!   is what makes a plan review time out.

use fleet_core::agents::{
    AgentEvent, GateId, GateKind, GateResolver, PermissionChoice, PermissionOption, PlanAnswer,
    ProviderOptionId, Question, QuestionOption,
};
use serde_json::{Value, json};

use super::{
    MapOutput,
    text::{strip_ansi, top_level_steps},
    tools::{permission_payload, tool_kind},
};
use crate::agents::claude::{
    frames::ControlRequestFrame,
    session::{ClaudeSession, GateShape, PendingGate, WireQuestion, closed_answer},
};
use crate::agents::harness::{
    HarnessError, HarnessResult, ProtocolOp,
    fingerprint::{IssueKind, SchemaFingerprint},
};

/// The instruction that rides on the `ExitPlanMode` denial.
const PLAN_STOP_INSTRUCTION: &str = "The client captured your proposed plan. Stop here and wait for the user's feedback or implementation request in a later turn.";

/// Maps one inbound control request.
pub(in crate::agents::claude) fn control_request(
    session: &mut ClaudeSession,
    frame: ControlRequestFrame,
) -> MapOutput {
    if frame.request.subtype == "can_use_tool" {
        return can_use_tool(session, frame);
    }
    let response = match frame.request.subtype.as_str() {
        // Fleet does not open URLs or forms on the user's behalf from a Claude elicitation.
        "elicitation" => control_success(&frame.request_id, json!({"action": "decline"})),
        other => control_error(
            &frame.request_id,
            &format!("Fleet does not implement the Claude `{other}` control request."),
        ),
    };
    MapOutput {
        events: Vec::new(),
        writes: vec![response],
        raw: None,
    }
}

/// Maps a `can_use_tool` request into a gate, and answers it immediately when it is a plan.
fn can_use_tool(session: &mut ClaudeSession, frame: ControlRequestFrame) -> MapOutput {
    let tool_name = frame
        .request
        .tool_name
        .clone()
        .unwrap_or_else(|| "Unknown".to_owned());
    let turn = session.active_turn();
    let gate = GateId::new();
    let mut writes = Vec::new();
    let (kind, shape, event) = match tool_name.as_str() {
        "AskUserQuestion" => {
            let (questions, wire_questions) = questions_from_input(&frame.request.input);
            (
                GateKind::Question { questions },
                GateShape::Question(wire_questions),
                None,
            )
        }
        "ExitPlanMode" => {
            let markdown = frame
                .request
                .input
                .get("plan")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| session.recorded_plan(frame.request.tool_use_id.as_deref()))
                .or_else(|| session.last_assistant_text.clone())
                .unwrap_or_default();
            let steps = top_level_steps(&markdown);
            // Always denied, whatever the mode, so the model stops and waits rather than
            // blocking on a card that a plan review will outlive.
            writes.push(control_success(
                &frame.request_id,
                json!({
                    "behavior": "deny",
                    "message": PLAN_STOP_INSTRUCTION,
                    "interrupt": false,
                    "toolUseID": frame.request.tool_use_id,
                }),
            ));
            let proposed = turn.map(|turn| AgentEvent::PlanProposed {
                gate,
                turn,
                markdown: markdown.clone(),
                steps: steps.clone(),
            });
            (
                GateKind::Plan { markdown, steps },
                GateShape::Plan,
                proposed,
            )
        }
        _ => (
            permission_gate(&tool_name, &frame),
            GateShape::Permission,
            None,
        ),
    };
    let answered = !writes.is_empty();
    session.gates.insert(
        gate,
        PendingGate {
            // An answered request must never be answered twice: an empty id is the record that
            // the wire is already settled and only Fleet's own decision is outstanding.
            request_id: if answered {
                String::new()
            } else {
                frame.request_id.clone()
            },
            tool_name: tool_name.clone(),
            tool_use_id: frame.request.tool_use_id.clone(),
            input: frame.request.input.clone(),
            suggestions: frame.request.permission_suggestions.clone(),
            shape,
        },
    );
    // A plan that arrived twice (once on the snapshot, once here) must not open two cards.
    let mut events = Vec::new();
    match event {
        Some(proposed) => events.push(proposed),
        None => events.push(AgentEvent::GateOpened { gate, turn, kind }),
    }
    MapOutput {
        events,
        writes,
        raw: None,
    }
}

/// The permission card for a tool that is neither a question nor a plan.
fn permission_gate(tool_name: &str, frame: &ControlRequestFrame) -> GateKind {
    let mut options = vec![permission_option("allow_once", PermissionChoice::AllowOnce)];
    // A request the CLI says needs a human cannot be answered by a stored rule either.
    let rules_allowed =
        !frame.request.suppress_always_allow_rule && !frame.request.requires_user_interaction;
    if rules_allowed {
        options.push(permission_option(
            "allow_session",
            PermissionChoice::AllowSession,
        ));
        // The persistent scope, which t3code's `acceptAlways` silently turned into a *deny*.
        // It is offered separately and its copy has to say that it writes a stored rule.
        options.push(permission_option(
            "allow_persistent",
            PermissionChoice::AllowDirectory,
        ));
    }
    options.push(permission_option("deny", PermissionChoice::Deny));
    options.push(permission_option(
        "deny_and_stop",
        PermissionChoice::DenyAndStop,
    ));
    if tool_name == "Bash" {
        options.push(permission_option("edit", PermissionChoice::Edit));
    }
    // Honour `default_to_no` by leading with the CLI's own safe answer, which is the option the
    // card focuses.
    if frame.request.default_to_no
        && let Some(deny) = options
            .iter()
            .position(|option| option.label == PermissionChoice::Deny)
    {
        let deny = options.remove(deny);
        options.insert(0, deny);
    }
    // `display_name` is the label to show, not `tool_name`.
    let label = frame
        .request
        .display_name
        .clone()
        .unwrap_or_else(|| tool_name.to_owned());
    let title = frame
        .request
        .title
        .clone()
        .unwrap_or_else(|| format!("Claude wants to use {label}"));
    // The payload is the protected invocation, read literally. The model's prose `description`
    // is a rationale, never the command the user is approving.
    let payload = permission_payload(tool_name, &frame.request.input);
    let rationale = frame
        .request
        .decision_reason
        .as_deref()
        .map(strip_ansi)
        .or_else(|| frame.request.description.clone());
    GateKind::Permission {
        tool: tool_kind(tool_name),
        title,
        payload,
        rationale,
        options,
    }
}

/// Closes the gate a cancelled control request opened.
///
/// `control_cancel_request` has **no response**: the CLI has stopped waiting, so the card must go
/// with it rather than sit at the top of `NeedsYou` for the life of the thread.
pub(in crate::agents::claude) fn control_cancel(
    session: &mut ClaudeSession,
    request_id: &str,
) -> MapOutput {
    let Some((gate, pending)) = session
        .gates
        .iter()
        .find(|(_, pending)| pending.request_id == request_id)
        .map(|(gate, pending)| (*gate, pending.clone()))
    else {
        return MapOutput::default();
    };
    session.gates.remove(&gate);
    MapOutput::from(vec![AgentEvent::GateResolved {
        gate,
        answer: closed_answer(&pending.shape),
        by: GateResolver::ProviderClosed,
    }])
}

/// The `control_response` Fleet writes for one gate answer, or `None` when the wire is settled.
pub(in crate::agents::claude) fn response_for(
    session: &ClaudeSession,
    gate: GateId,
    answer: &fleet_core::agents::GateAnswer,
) -> HarnessResult<Option<Value>> {
    use fleet_core::agents::GateAnswer;

    let pending = session
        .gates
        .get(&gate)
        .ok_or(HarnessError::GateGone { gate })?;
    // A plan request was denied the moment it arrived, so its decision writes nothing: the
    // approval travels as the next turn's message instead.
    if pending.request_id.is_empty() {
        return Ok(None);
    }
    let response = match (&pending.shape, answer) {
        (
            GateShape::Permission,
            GateAnswer::Permission {
                choice,
                edited_payload,
            },
        ) => permission_response(pending, *choice, edited_payload.as_deref())?,
        (GateShape::Question(questions), GateAnswer::Question { answers }) => {
            question_response(pending, questions, answers)?
        }
        (GateShape::Plan, GateAnswer::Plan(answer)) => plan_response(pending, answer),
        _ => {
            return Err(HarnessError::Protocol {
                op: ProtocolOp::Respond,
                method: Some("control_response".to_owned()),
                fingerprint: SchemaFingerprint::of_value(IssueKind::Shape, &Value::Null),
            });
        }
    };
    Ok(Some(control_success(&pending.request_id, response)))
}

fn permission_response(
    pending: &PendingGate,
    choice: PermissionChoice,
    edited_payload: Option<&str>,
) -> HarnessResult<Value> {
    let mut input = pending.input.clone();
    // An edited payload is applied whether the answer arrives as `Edit` or as the `AllowOnce` the
    // card sends after editing.
    if let Some(edited) = edited_payload
        && matches!(choice, PermissionChoice::AllowOnce | PermissionChoice::Edit)
    {
        let Some(object) = input.as_object_mut() else {
            return Err(respond_error());
        };
        object.insert("command".to_owned(), Value::String(edited.to_owned()));
    }
    let response = match choice {
        PermissionChoice::AllowOnce => json!({
            "behavior": "allow",
            "updatedInput": input,
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_allow",
        }),
        PermissionChoice::AllowSession => json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": session_scoped(pending),
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_allow",
        }),
        // The persistent scope keeps the CLI's own offered destination, which is the project's
        // settings file. The copy on this option is what tells the user it is stored.
        PermissionChoice::AllowDirectory => json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": persistent_scoped(pending),
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_permanent",
        }),
        PermissionChoice::Edit => {
            if edited_payload.is_none() {
                return Err(respond_error());
            }
            json!({
                "behavior": "allow",
                "updatedInput": input,
                "toolUseID": pending.tool_use_id,
                "decisionClassification": "user_allow",
            })
        }
        PermissionChoice::Deny | PermissionChoice::DenyAndStop => json!({
            "behavior": "deny",
            "message": if choice == PermissionChoice::DenyAndStop {
                "User cancelled tool execution."
            } else {
                "User declined tool execution."
            },
            "interrupt": choice == PermissionChoice::DenyAndStop,
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_reject",
        }),
    };
    Ok(response)
}

/// The suggested permission updates, rewritten to the session scope the card promises.
///
/// A suggestion arrives with whatever destination the CLI would have used — `localSettings`
/// writes a permanent rule into the user's settings file. Where the CLI offers no suggestion at
/// all (common for MCP tools) the session decision falls back to a whole-tool session allow rule:
/// degrading it into a one-shot accept means the card comes back on the next call and the user
/// believes the toggle is broken.
fn session_scoped(pending: &PendingGate) -> Vec<Value> {
    let rescoped = rescope(&pending.suggestions, "session");
    if !rescoped.is_empty() {
        return rescoped;
    }
    if pending.tool_name.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "type": "addRules",
        "rules": [{"toolName": pending.tool_name}],
        "behavior": "allow",
        "destination": "session",
    })]
}

/// The suggested permission updates for the persistent decision, with the offered destination.
fn persistent_scoped(pending: &PendingGate) -> Vec<Value> {
    pending
        .suggestions
        .iter()
        .filter(|suggestion| {
            matches!(
                suggestion.get("type").and_then(Value::as_str),
                Some("addRules" | "setMode")
            )
        })
        .cloned()
        .collect()
}

fn rescope(suggestions: &[Value], destination: &str) -> Vec<Value> {
    suggestions
        .iter()
        .filter(|suggestion| {
            matches!(
                suggestion.get("type").and_then(Value::as_str),
                Some("addRules" | "setMode")
            )
        })
        .cloned()
        .map(|mut suggestion| {
            if let Some(object) = suggestion.as_object_mut() {
                object.insert(
                    "destination".to_owned(),
                    Value::String(destination.to_owned()),
                );
            }
            suggestion
        })
        .collect()
}

/// The answer map is keyed by the **exact question text**: the CLI looks answers up by it.
fn question_response(
    pending: &PendingGate,
    questions: &[WireQuestion],
    answers: &[Vec<String>],
) -> HarnessResult<Value> {
    if answers.len() != questions.len() {
        return Err(respond_error());
    }
    let mut answer_map = serde_json::Map::new();
    for (question, values) in questions.iter().zip(answers) {
        let value = if question.multi_select {
            Value::Array(values.iter().cloned().map(Value::String).collect())
        } else {
            Value::String(values.first().cloned().unwrap_or_default())
        };
        answer_map.insert(question.text.clone(), value);
    }
    let mut input = pending.input.clone();
    let Some(object) = input.as_object_mut() else {
        return Err(respond_error());
    };
    object.insert("answers".to_owned(), Value::Object(answer_map));
    Ok(json!({
        "behavior": "allow",
        "updatedInput": input,
        "toolUseID": pending.tool_use_id,
    }))
}

fn plan_response(pending: &PendingGate, answer: &PlanAnswer) -> Value {
    match answer {
        PlanAnswer::Approve => json!({
            "behavior": "allow",
            "updatedInput": pending.input,
            "toolUseID": pending.tool_use_id,
        }),
        PlanAnswer::AskForChanges { note } => json!({
            "behavior": "deny",
            "message": note,
            "interrupt": false,
            "toolUseID": pending.tool_use_id,
        }),
    }
}

fn respond_error() -> HarnessError {
    HarnessError::Protocol {
        op: ProtocolOp::Respond,
        method: Some("control_response".to_owned()),
        fingerprint: SchemaFingerprint::of_value(IssueKind::Shape, &Value::Null),
    }
}

/// A successful `control_response` envelope.
#[must_use]
pub(in crate::agents::claude) fn control_success(request_id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        }
    })
}

/// A failing `control_response` envelope.
#[must_use]
pub(in crate::agents::claude) fn control_error(request_id: &str, error: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": error,
        }
    })
}

fn permission_option(id: &str, label: PermissionChoice) -> PermissionOption {
    PermissionOption {
        id: ProviderOptionId(id.to_owned()),
        label,
    }
}

/// The questions an `AskUserQuestion` input carries, in Fleet's shape and in the CLI's.
fn questions_from_input(input: &Value) -> (Vec<Question>, Vec<WireQuestion>) {
    let mut questions = Vec::new();
    let mut wire = Vec::new();
    for value in input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = value
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let multi_select = value
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let options = value
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|option| {
                let label = option
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                QuestionOption {
                    // Claude's option identity *is* the label: it is what the answer carries.
                    id: ProviderOptionId(label.clone()),
                    label,
                    description: option
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }
            })
            .collect();
        questions.push(Question {
            // Claude keys answers by the exact question text, so that text is the identity.
            id: text.clone(),
            header: value
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            prompt: text.clone(),
            options,
            multi_select,
            // A conventional "something else" is always allowed on Claude; Codex says so
            // explicitly with `isOther`.
            allows_other: true,
            // Claude has no `isSecret` and no `isBlocking: false`.
            is_secret: false,
            blocking: true,
        });
        wire.push(WireQuestion { text, multi_select });
    }
    (questions, wire)
}
