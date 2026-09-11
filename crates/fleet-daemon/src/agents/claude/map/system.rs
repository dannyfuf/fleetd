//! `system/*` frames, `rate_limit_event`, and the subagent surface.
//!
//! Three of these frames were not in the previous revision of the design and all three were
//! observed live: `system/status` (a truthful spinner sub-label, **never** a terminal),
//! `system/thinking_tokens` (a live reasoning-token counter), and `rate_limit_event` (two
//! utilisation windows, and the parked turn — see [`rate_limit`]).

use std::collections::HashSet;

use chrono::{TimeZone as _, Utc};
use fleet_core::agents::{
    AgentEvent, AgentKind, CheckpointKind, ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus,
    ModelSelection, SessionState, ToolPatch, WaitingReason,
};
use serde_json::{Value, json};

use super::{
    MapOutput,
    text::{notice_text, string_field, string_list, task_status, u32_field, u64_field},
    tools::tool_kind,
};
use crate::agents::claude::{
    argv::permission_mode_from_wire,
    frames::{RateLimitFrame, SILENT_SYSTEM_SUBTYPES, SystemFrame},
    session::ClaudeSession,
};

/// Maps one `system` frame.
pub(in crate::agents::claude) fn system(
    session: &mut ClaudeSession,
    frame: SystemFrame,
) -> MapOutput {
    let mut events = Vec::new();
    match frame.subtype.as_str() {
        "init" => init(session, &frame, &mut events),
        // A fine-grained activity signal ("we are waiting on the API right now"). Useful for a
        // truthful spinner sub-label; **not** a turn terminal.
        "status" => {
            if let Some(phase) = string_field(&frame.fields, "status") {
                events.push(AgentEvent::SessionActivity { phase });
            }
        }
        // The live reasoning counter that lets the collapsed row read `thinking · 132 tokens`
        // and tick with the body closed. Codex has no counterpart, and faking one there would be
        // an invented number (§4.3).
        "thinking_tokens" => {
            let tokens = u64_field(&frame.fields, "estimated_tokens");
            events.push(AgentEvent::SessionActivity {
                phase: format!("thinking · {tokens} tokens"),
            });
        }
        "session_state_changed" => {
            let state = match string_field(&frame.fields, "state").as_deref() {
                Some("idle") => SessionState::Ready,
                // `requires_action` is a wait on a gate, and attention already derives that from
                // the open gates themselves. `Waiting` is reserved for the parked turn, which is
                // the one wait with a countdown and no gate.
                Some("running" | "requires_action") => SessionState::Running,
                Some(other) => {
                    tracing::debug!(
                        target: "fleet::agents::claude",
                        state = other,
                        "no projection for this Claude session state"
                    );
                    return MapOutput::from(events);
                }
                None => return MapOutput::from(events),
            };
            events.push(AgentEvent::SessionStateChanged(state));
        }
        "compact_boundary" => {
            let metadata = frame.fields.get("compact_metadata");
            let before = metadata
                .and_then(|value| value.get("pre_tokens"))
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let after = metadata
                .and_then(|value| value.get("post_tokens"))
                .and_then(Value::as_u64);
            session.compacting = None;
            events.push(AgentEvent::Compacted(CheckpointKind::CompactBoundary {
                before,
                after,
            }));
        }
        // A heartbeat, never a warning row: a 502 storm produces ten of these and surfacing each
        // one spams the work log.
        "api_retry" => events.push(AgentEvent::Retrying {
            attempt: u32_field(&frame.fields, "attempt"),
            retry_in_ms: u64_field(&frame.fields, "retry_delay_ms"),
            reason: string_field(&frame.fields, "error")
                .unwrap_or_else(|| "Claude's API request failed".to_owned()),
        }),
        "permission_denied" => permission_denied(session, &frame, &mut events),
        "task_started" => task_started(session, &frame, &mut events),
        "task_updated" => task_updated(session, &frame, &mut events),
        "task_progress" => task_progress(session, &frame, &mut events),
        "task_notification" => task_notification(session, &frame, &mut events),
        "background_tasks_changed" => background_tasks_changed(session, &frame, &mut events),
        "files_persisted" => {
            let failed = frame
                .fields
                .get("failed")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            if failed > 0 {
                events.push(AgentEvent::Notice(format!(
                    "{failed} attachment(s) could not be saved for this turn."
                )));
            }
        }
        // Only the loud ones become a row: info, notice and suggestion are CLI chrome.
        "notification" => {
            if matches!(
                string_field(&frame.fields, "priority").as_deref(),
                Some("high" | "immediate")
            ) && let Some(text) = string_field(&frame.fields, "text")
                && let Some(notice) = notice_text(&text)
            {
                events.push(AgentEvent::Notice(notice));
            }
        }
        "informational" => {
            if string_field(&frame.fields, "level").as_deref() == Some("warning")
                && let Some(content) = string_field(&frame.fields, "content")
                && let Some(notice) = notice_text(&content)
            {
                events.push(AgentEvent::Notice(notice));
            }
        }
        "model_refusal_fallback" | "model_refusal_no_fallback" => {
            let text = string_field(&frame.fields, "api_refusal_explanation")
                .filter(|text| !text.trim().is_empty())
                .or_else(|| string_field(&frame.fields, "content"));
            if let Some(notice) = text.as_deref().and_then(notice_text) {
                events.push(AgentEvent::Notice(notice));
            }
        }
        "mirror_error" => events.push(AgentEvent::RuntimeError {
            fatal: false,
            message: string_field(&frame.fields, "message")
                .unwrap_or_else(|| "Claude reported a mirror error.".to_owned()),
        }),
        subtype if SILENT_SYSTEM_SUBTYPES.contains(&subtype) => {}
        other => {
            // Counted, never a red row: an unrecognised envelope is a diagnostic, and routing it
            // to the transcript hides the real notices behind output that says nothing.
            tracing::debug!(
                target: "fleet::agents::claude",
                subtype = other,
                "no projection for this Claude system frame"
            );
            events.push(AgentEvent::Unknown {
                method: format!("system/{other}"),
            });
        }
    }
    MapOutput::from(events)
}

/// `system/init`: the handshake. Six fields are read and the rest is ignored.
fn init(session: &mut ClaudeSession, frame: &SystemFrame, events: &mut Vec<AgentEvent>) {
    session.declared = frame
        .fields
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect();
    if let Some(version) = string_field(&frame.fields, "claude_code_version") {
        tracing::info!(
            target: "fleet::agents::claude",
            version = %version,
            capabilities = ?session.declared,
            "Claude Code session initialised"
        );
    }
    if session.initialized {
        // A resumed session emits exactly one `SessionConfigured`.
        return;
    }
    session.initialized = true;
    // Effort is a launch flag and is **not** echoed here, so Fleet reports what it launched with.
    let model = string_field(&frame.fields, "model").map(|model| ModelSelection {
        model,
        effort: session.launched_effort.clone(),
        provider: None,
    });
    session.session_model = model.as_ref().map(|model| model.model.clone());
    // Advisory: `manual` comes back as `"default"`, so Fleet keeps its own record and this only
    // notices a mode it did not ask for.
    let mode = string_field(&frame.fields, "permissionMode")
        .as_deref()
        .map(permission_mode_from_wire)
        .unwrap_or_default();
    events.push(AgentEvent::SessionConfigured {
        provider: AgentKind::Claude,
        resume_cursor: string_field(&frame.fields, "session_id"),
        model,
        mode,
        tools: string_list(frame.fields.get("tools")),
        commands: string_list(frame.fields.get("slash_commands")),
        skills: string_list(frame.fields.get("skills")),
    });
    events.push(AgentEvent::SessionStateChanged(SessionState::Ready));
}

/// `rate_limit_event`, and the parked turn it announces.
///
/// A window whose `status == "rejected"` with no allowed overage **parks the turn inside the CLI:
/// no further frames arrive and no `result` ever lands.** Without a row the thread spins forever.
/// So Fleet publishes a first-class `Waiting { UsageLimit }` state — not a warning row — and the
/// app ticks a live countdown locally instead of rendering a string that goes stale.
///
/// `allowed_warning` stays quiet: it still has headroom, and an account spending provisioned
/// overage keeps running despite a reject.
pub(in crate::agents::claude) fn rate_limit(
    session: &mut ClaudeSession,
    frame: RateLimitFrame,
) -> MapOutput {
    let info = frame.rate_limit_info;
    let mut events = vec![AgentEvent::RateLimits {
        limits: info.clone(),
    }];
    let status = info.get("status").and_then(Value::as_str).unwrap_or("");
    let overage = info
        .get("overageStatus")
        .and_then(Value::as_str)
        .unwrap_or("");
    let using_overage = ["isUsingOverage", "overageInUse"]
        .into_iter()
        .any(|key| info.get(key).and_then(Value::as_bool).unwrap_or(false));
    let blocked = status == "rejected"
        && !(matches!(overage, "allowed" | "allowed_warning") || using_overage);
    if !blocked {
        return MapOutput::from(events);
    }
    let window = info
        .get("rateLimitType")
        .and_then(Value::as_str)
        .unwrap_or("usage")
        .to_owned();
    let resets_at = info
        .get("resetsAt")
        .and_then(Value::as_i64)
        .or_else(|| {
            info.get("unifiedWindows")
                .and_then(Value::as_object)
                .and_then(|windows| {
                    windows
                        .get(window.as_str())
                        .and_then(|value| value.get("resetsAt"))
                        .and_then(Value::as_i64)
                })
        })
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
        .unwrap_or_else(Utc::now);
    // Deduplicated per turn per `<window>:<resets_at>`: a parked window re-fires as the wait
    // shrinks, and a turn can park on more than one window.
    let key = format!("{window}:{}", resets_at.timestamp());
    if session.announced_windows.insert(key) {
        events.push(AgentEvent::SessionStateChanged(SessionState::Waiting(
            WaitingReason::UsageLimit { window, resets_at },
        )));
    }
    MapOutput::from(events)
}

/// `system/permission_denied`: the frame that says "this was refused without asking you".
///
/// **Must render.** Dropping it makes a refusal look like a hang. When the tool it names was
/// never opened as an item, one is opened here and closed denied, rather than the denial being
/// lost with it.
fn permission_denied(
    session: &mut ClaudeSession,
    frame: &SystemFrame,
    events: &mut Vec<AgentEvent>,
) {
    let tool_name = string_field(&frame.fields, "tool_name").unwrap_or_else(|| "a tool".to_owned());
    let reason = string_field(&frame.fields, "message")
        .or_else(|| string_field(&frame.fields, "decision_reason"));
    let existing = string_field(&frame.fields, "tool_use_id")
        .and_then(|id| session.tool_items.get(&id).copied());
    let item = match (existing, session.active_turn()) {
        (Some(item), _) => item,
        (None, Some(turn)) => {
            let input = frame.fields.get("input").cloned().unwrap_or(Value::Null);
            session.start_item(
                turn,
                ItemKind::Tool(Box::new(fleet_core::agents::ToolCall {
                    kind: tool_kind(&tool_name),
                    name: tool_name.clone(),
                    input: input.clone(),
                    summary: reason.clone(),
                    result: None,
                    output: String::new(),
                    diff: None,
                    exit_code: None,
                    duration_ms: None,
                    extra: std::collections::BTreeMap::new(),
                })),
                None,
                Some(tool_name.clone()),
                input,
                events,
            )
        }
        (None, None) => {
            events.push(AgentEvent::Notice(format!(
                "{tool_name} was refused without asking: {}",
                reason.unwrap_or_else(|| "no reason was given".to_owned())
            )));
            return;
        }
    };
    events.push(AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                result: reason.map(Value::String),
                ..ToolPatch::default()
            }))),
            status: Some(ItemStatus::Denied),
        },
    });
    session.complete_item(item, ItemStatus::Denied, events);
}

fn task_started(session: &mut ClaudeSession, frame: &SystemFrame, events: &mut Vec<AgentEvent>) {
    let (Some(turn), Some(task_id)) = (
        session.active_turn(),
        string_field(&frame.fields, "task_id"),
    ) else {
        return;
    };
    let description = string_field(&frame.fields, "description").unwrap_or_default();
    let name = string_field(&frame.fields, "subagent_type")
        .or_else(|| string_field(&frame.fields, "task_type"))
        .unwrap_or_else(|| "subagent".to_owned());
    let parent = string_field(&frame.fields, "tool_use_id")
        .and_then(|id| session.tool_items.get(&id).copied());
    let item = session.start_item(
        turn,
        ItemKind::Subagent {
            name,
            description,
            result: None,
        },
        parent,
        None,
        Value::Null,
        events,
    );
    session.task_items.insert(task_id, item);
}

fn task_updated(session: &mut ClaudeSession, frame: &SystemFrame, events: &mut Vec<AgentEvent>) {
    let Some(task_id) = string_field(&frame.fields, "task_id") else {
        return;
    };
    let Some(item) = session.task_items.get(&task_id).copied() else {
        return;
    };
    let patch = frame.fields.get("patch").unwrap_or(&Value::Null);
    let status = patch.get("status").and_then(Value::as_str);
    if let Some(description) = patch.get("description").and_then(Value::as_str) {
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                payload: Some(ItemPayloadPatch::Subagent {
                    name: None,
                    description: Some(description.to_owned()),
                    result: None,
                }),
                status: None,
            },
        });
    }
    if let Some(status) = task_status(status) {
        session.complete_item(item, status, events);
    }
}

fn task_progress(session: &mut ClaudeSession, frame: &SystemFrame, events: &mut Vec<AgentEvent>) {
    let Some(task_id) = string_field(&frame.fields, "task_id") else {
        return;
    };
    let Some(item) = session.task_items.get(&task_id).copied() else {
        return;
    };
    let usage = frame.fields.get("usage").unwrap_or(&Value::Null);
    let tool_uses = usage
        .get("tool_uses")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let duration_ms = usage
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    events.push(AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Subagent {
                name: None,
                description: string_field(&frame.fields, "summary")
                    .or_else(|| string_field(&frame.fields, "description")),
                result: Some(json!({
                    "toolUses": tool_uses,
                    "durationMs": duration_ms,
                })),
            }),
            status: Some(ItemStatus::InProgress),
        },
    });
}

fn task_notification(
    session: &mut ClaudeSession,
    frame: &SystemFrame,
    events: &mut Vec<AgentEvent>,
) {
    let Some(task_id) = string_field(&frame.fields, "task_id") else {
        return;
    };
    let Some(item) = session.task_items.get(&task_id).copied() else {
        return;
    };
    let status = match string_field(&frame.fields, "status").as_deref() {
        Some("completed") => ItemStatus::Completed,
        Some("failed") => ItemStatus::Failed,
        Some("stopped") => ItemStatus::Stopped,
        _ => return,
    };
    events.push(AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Subagent {
                name: None,
                description: None,
                result: string_field(&frame.fields, "summary").map(Value::String),
            }),
            status: Some(status),
        },
    });
    session.complete_item(item, status, events);
}

/// `background_tasks_changed` has **replace** semantics, and only for background tasks.
///
/// A foreground subagent row is closed by its own `task_updated`/`task_notification`, never by
/// dropping out of this list.
fn background_tasks_changed(
    session: &mut ClaudeSession,
    frame: &SystemFrame,
    events: &mut Vec<AgentEvent>,
) {
    let tasks = frame
        .fields
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let live = tasks
        .iter()
        .filter_map(|task| {
            task.get("task_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect::<HashSet<_>>();
    if let Some(turn) = session.active_turn() {
        for task in &tasks {
            let Some(task_id) = task.get("task_id").and_then(Value::as_str) else {
                continue;
            };
            if session.task_items.contains_key(task_id) {
                continue;
            }
            let name = task
                .get("task_type")
                .and_then(Value::as_str)
                .unwrap_or("subagent")
                .to_owned();
            let description = task
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let item = session.start_item(
                turn,
                ItemKind::Subagent {
                    name,
                    description,
                    result: None,
                },
                None,
                None,
                Value::Null,
                events,
            );
            session.task_items.insert(task_id.to_owned(), item);
        }
    }
    for task in &live {
        if session.task_items.contains_key(task) {
            session.background_tasks.insert(task.clone());
        }
    }
    let ended = session
        .background_tasks
        .iter()
        .filter(|task| !live.contains(*task))
        .cloned()
        .collect::<Vec<_>>();
    for task in ended {
        session.background_tasks.remove(&task);
        if let Some(item) = session.task_items.get(&task).copied() {
            session.complete_item(item, ItemStatus::Completed, events);
        }
    }
}
