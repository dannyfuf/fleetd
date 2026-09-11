//! Thread lifecycle, settings, usage, account and advisory notifications.

use fleet_core::agents::{
    AgentEvent, AgentKind, CheckpointKind, PermissionMode, SessionState, Usage,
};
use serde_json::Value;

use super::{MapOutput, decode};
use crate::agents::codex::{
    session::{CodexSession, gate_id},
    wire::{
        AccountRateLimitsUpdatedNotification, ContextCompactedNotification,
        ServerRequestResolvedNotification, ThreadClosedNotification, ThreadNameUpdatedNotification,
        ThreadSettingsUpdatedNotification, ThreadStartedNotification, ThreadStatus,
        ThreadStatusChangedNotification, ThreadTokenUsageUpdatedNotification,
    },
};

/// `thread/started`. Also fires for subagent threads on the same connection.
pub(in crate::agents::codex) fn started(session: &mut CodexSession, params: &Value) -> MapOutput {
    let notification: ThreadStartedNotification = match decode("thread/started", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    let thread = notification.thread;
    // A child's `thread/started` is a parent-state mutator and is dropped by the routing table;
    // reaching here for a foreign thread means the root has not been adopted yet.
    match session.root.as_deref() {
        Some(root) if root != thread.id => MapOutput::default(),
        Some(_) => MapOutput::default(),
        None => {
            session.root = Some(thread.id.clone());
            MapOutput::one(AgentEvent::SessionConfigured {
                provider: AgentKind::Codex,
                resume_cursor: Some(thread.id),
                model: session.model_selection(),
                mode: PermissionMode::default(),
                tools: Vec::new(),
                commands: Vec::new(),
                skills: Vec::new(),
            })
        }
    }
}

/// `thread/status/changed`: a **hint**.
///
/// It fires `active` before `turn/started` and `idle` before `turn/completed`, so it moves the
/// session label and never the turn. `activeFlags` is where Fleet's attention state comes free —
/// cross-checked against the open gates, never used as their source.
pub(in crate::agents::codex) fn status_changed(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ThreadStatusChangedNotification =
        match decode("thread/status/changed", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return MapOutput::default();
    }
    let state = match notification.status {
        ThreadStatus::Active { .. } => SessionState::Running,
        ThreadStatus::Idle => SessionState::Ready,
        ThreadStatus::SystemError => SessionState::Error,
        // `notLoaded` and a status this build does not know say nothing about the session.
        ThreadStatus::NotLoaded | ThreadStatus::Unknown => return MapOutput::default(),
    };
    // An `idle` status with an open turn for more than a moment is a logged inconsistency: the
    // turn stays open, because `turn/completed` is still the authority.
    if matches!(state, SessionState::Ready) && session.active_turn.is_some() {
        tracing::debug!(
            target: "fleet::agents::codex",
            "Codex reported idle with a turn still open; waiting for turn/completed"
        );
    }
    MapOutput::one(AgentEvent::SessionStateChanged(state))
}

/// `thread/settings/updated`: what is actually in force after a change.
pub(in crate::agents::codex) fn settings_updated(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ThreadSettingsUpdatedNotification =
        match decode("thread/settings/updated", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return MapOutput::default();
    }
    let settings = notification.thread_settings;
    session.controls.model = Some(settings.model.clone());
    session.controls.effort = effort_label(settings.effort.as_ref());
    MapOutput::one(AgentEvent::MetadataChanged {
        title: None,
        mode: None,
        model: session.model_selection(),
    })
}

/// The wire spelling of a reasoning effort, which is a **free string**, not an enum.
fn effort_label(effort: Option<&crate::agents::codex::wire::ReasoningEffort>) -> Option<String> {
    let effort = effort?;
    serde_json::to_value(effort)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
}

/// `thread/tokenUsage/updated`: pushed **per model step**, not once per turn.
///
/// `total` is cumulative for the thread, `last` is the most recent step, and
/// `modelContextWindow` is the denominator — which is why the Codex context meter can move
/// during a turn and the Claude one cannot.
pub(in crate::agents::codex) fn token_usage(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ThreadTokenUsageUpdatedNotification =
        match decode("thread/tokenUsage/updated", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return MapOutput::default();
    }
    let usage = notification.token_usage;
    if let Some(window) = usage
        .model_context_window
        .and_then(|w| u64::try_from(w).ok())
    {
        session.context_window = Some(window);
    }
    let total = &usage.total;
    let used = u64::try_from(total.total_tokens).unwrap_or_default();
    let context_pct = session.context_window.map_or(0.0, |window| {
        if window == 0 {
            0.0
        } else {
            (used as f64 * 100.0 / window as f64).min(100.0) as f32
        }
    });
    let normalized = Usage {
        input_tokens: u64::try_from(total.input_tokens).unwrap_or_default(),
        output_tokens: u64::try_from(total.output_tokens).unwrap_or_default(),
        reasoning_tokens: u64::try_from(total.reasoning_output_tokens).unwrap_or_default(),
        cache_read_tokens: u64::try_from(total.cached_input_tokens).unwrap_or_default(),
        cache_write_tokens: total
            .cache_write_input_tokens
            .and_then(|tokens| u64::try_from(tokens).ok())
            .unwrap_or_default(),
        total_tokens: used,
        web_search_requests: 0,
        tool_uses: 0,
        extra: std::collections::BTreeMap::new(),
    };
    MapOutput::one(AgentEvent::TokenUsage {
        turn: session.turn_for(&notification.turn_id),
        usage: normalized,
        context_pct,
        cost_usd: None,
    })
}

/// `thread/name/updated`: Codex publishes a title; Claude does not.
pub(in crate::agents::codex) fn name_updated(params: &Value) -> MapOutput {
    let notification: ThreadNameUpdatedNotification = match decode("thread/name/updated", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    let Some(title) = notification
        .thread_name
        .filter(|name| !name.trim().is_empty())
    else {
        return MapOutput::default();
    };
    MapOutput::one(AgentEvent::MetadataChanged {
        title: Some(title),
        mode: None,
        model: None,
    })
}

/// `thread/closed`.
pub(in crate::agents::codex) fn closed(session: &mut CodexSession, params: &Value) -> MapOutput {
    let notification: ThreadClosedNotification = match decode("thread/closed", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        // A child closing deletes its live-turn entry and nothing else.
        session.subagents.clear_live_turn(&notification.thread_id);
        return MapOutput::default();
    }
    MapOutput::one(AgentEvent::SessionExited {
        code: None,
        expected: true,
    })
}

/// `thread/environment/{connected,disconnected}`: two methods, one params type.
pub(in crate::agents::codex) fn environment(params: &Value, connected: bool) -> MapOutput {
    let name = params
        .get("environmentId")
        .and_then(Value::as_str)
        .unwrap_or("an environment");
    let notice = if connected {
        format!("Connected to {name}.")
    } else {
        format!("Disconnected from {name}.")
    };
    MapOutput::one(AgentEvent::Notice(notice))
}

/// `thread/compacted`, deprecated upstream in favour of the `contextCompaction` item.
///
/// Both are accepted and deduplicated on the item, so a build that emits both does not append
/// two boundaries.
pub(in crate::agents::codex) fn compacted(params: &Value) -> MapOutput {
    let notification: ContextCompactedNotification = match decode("thread/compacted", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    tracing::debug!(
        target: "fleet::agents::codex",
        turn = %notification.turn_id,
        "Codex compacted its own context"
    );
    MapOutput::one(AgentEvent::Compacted(CheckpointKind::CompactBoundary {
        before: 0,
        after: None,
    }))
}

/// `account/updated`.
///
/// Account metadata is a chip, not a transcript row, and the plan label it carries belongs to the
/// account surface rather than to one thread's log. Nothing is appended for it.
pub(in crate::agents::codex) fn account_updated() -> MapOutput {
    MapOutput::default()
}

/// `account/rateLimits/updated`: a **sparse merge**.
///
/// A field the update omits keeps its earlier value and a null does **not** clear one, so the
/// event carries the snapshot as it arrived and the projection merges. A snapshot whose `limitId`
/// is set and is not `"codex"` is **discarded, not merged**: a model-specific allowance must not
/// clobber the main rows.
pub(in crate::agents::codex) fn rate_limits(params: &Value) -> MapOutput {
    let notification: AccountRateLimitsUpdatedNotification =
        match decode("account/rateLimits/updated", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    if let Some(limit) = notification.rate_limits.limit_id.as_deref()
        && limit != "codex"
    {
        tracing::debug!(
            target: "fleet::agents::codex",
            "discarding a model-specific Codex rate-limit snapshot"
        );
        return MapOutput::default();
    }
    let limits = serde_json::to_value(&notification.rate_limits).unwrap_or(Value::Null);
    MapOutput::one(AgentEvent::RateLimits { limits })
}

/// `model/rerouted`: Codex can silently move the user to another model for safety.
pub(in crate::agents::codex) fn model_rerouted(params: &Value) -> MapOutput {
    let from = params
        .get("fromModel")
        .or_else(|| params.get("from"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let to = params
        .get("toModel")
        .or_else(|| params.get("to"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("safety")
        .to_owned();
    MapOutput::one(AgentEvent::ModelRerouted { from, to, reason })
}

/// `model/safetyBuffering/updated`: keep a spinner up while a response is withheld.
pub(in crate::agents::codex) fn safety_buffering(params: &Value) -> MapOutput {
    let buffering = params
        .get("showBufferingUi")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !buffering {
        return MapOutput::default();
    }
    MapOutput::one(AgentEvent::SessionActivity {
        phase: "checking the response".to_owned(),
    })
}

/// `warning`, `guardianWarning`, `deprecationNotice`, `configWarning`.
pub(in crate::agents::codex) fn notice(method: &str, params: &Value) -> MapOutput {
    let message = ["message", "warning", "notice", "detail"]
        .into_iter()
        .find_map(|key| params.get(key).and_then(Value::as_str))
        .unwrap_or(method)
        .to_owned();
    MapOutput::one(AgentEvent::Notice(message))
}

/// `mcpServer/*`: server startup and OAuth status.
pub(in crate::agents::codex) fn mcp_status(method: &str, params: &Value) -> MapOutput {
    // Only a failure is worth a row: a server that started is the expected case.
    let failed = params
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| params.get("message").and_then(Value::as_str));
    match failed {
        Some(detail) => MapOutput::one(AgentEvent::Notice(format!("{method}: {detail}"))),
        None => MapOutput::default(),
    }
}

/// `serverRequest/resolved`: a pending server request no longer needs an answer.
///
/// The user interrupted, or Codex's own auto-reviewer answered it. Fleet closes the gate and
/// **sends nothing**. This is the Codex twin of Claude's `control_cancel_request`, and dropping
/// it is what makes approval cards stick forever — so it **always** reaches the parent, whatever
/// the routing table says.
pub(in crate::agents::codex) fn server_request_resolved(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ServerRequestResolvedNotification =
        match decode("serverRequest/resolved", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    let thread = notification.thread_id.clone();
    let gate = gate_id(&thread, &notification.request_id);
    if session.gates.remove(&gate).is_none() {
        // A gate Fleet never opened, or one it already answered: nothing to withdraw.
        return MapOutput::default();
    }
    MapOutput::one(AgentEvent::GateWithdrawn { gate })
}
