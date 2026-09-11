//! Turn lifecycle: the one authoritative settlement, the diff, the plan, and the error that is
//! **not** terminal.

use fleet_core::agents::{AbortReason, AgentEvent, ItemStatus, LifecycleKind, TurnOutcome, Usage};
use serde_json::Value;

use super::{MapOutput, decode};
use crate::agents::codex::{
    session::CodexSession,
    wire::{
        ErrorNotification, TurnCompletedNotification, TurnDiffUpdatedNotification,
        TurnPlanUpdatedNotification, TurnStartedNotification, TurnStatus,
    },
};

/// `turn/started`: authoritative for "a turn is running", together with the `turn/start` result.
pub(in crate::agents::codex) fn started(session: &mut CodexSession, params: &Value) -> MapOutput {
    let notification: TurnStartedNotification = match decode("turn/started", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    let turn = session.turn_for(&notification.turn.id);
    // A child's `turn/started` is recorded so Stop reaches it, whatever the routing says.
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        session
            .subagents
            .note_live_turn(&notification.thread_id, &notification.turn.id);
        return MapOutput::default();
    }
    if !session.may_apply(LifecycleKind::TurnStarted, Some(turn)) {
        tracing::debug!(
            target: "fleet::agents::codex",
            %turn,
            "dropping a Codex turn start that conflicts with the running turn"
        );
        return MapOutput::default();
    }
    let already_running = session.active_turn == Some(turn);
    session.adopt_turn(turn, &notification.turn.id);
    if already_running {
        return MapOutput::default();
    }
    // The user's own message is echoed back as a `userMessage` item whose `clientId` is the id
    // Fleet sent, so the optimistic row and the echoed item are the same item.
    let user_item = session
        .user_item_for(&notification.turn.id)
        .unwrap_or_default();
    MapOutput::one(AgentEvent::TurnStarted { turn, user_item })
}

/// `turn/completed`: **terminal and authoritative**.
///
/// Two traps ride on this frame. `turn.items` is a *summary* governed by `itemsView` — a capture
/// reported **one** item for a turn that produced four — so the transcript is accumulated from
/// `item/started`/`item/completed` and this frame contributes only `status`, `error` and the
/// timestamps. And an unrecognised `TurnStatus` maps to `Completed`, a deliberate
/// never-leave-the-UI-spinning default.
pub(in crate::agents::codex) fn completed(session: &mut CodexSession, params: &Value) -> MapOutput {
    let notification: TurnCompletedNotification = match decode("turn/completed", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    let turn = session.turn_for(&notification.turn.id);
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        session.subagents.clear_live_turn(&notification.thread_id);
        return MapOutput::default();
    }
    if !session.may_apply(LifecycleKind::TurnSettled, Some(turn)) {
        tracing::warn!(
            target: "fleet::agents::codex",
            %turn,
            "dropping a Codex settlement for a turn that is not the running one"
        );
        return MapOutput::default();
    }
    let mut events = Vec::new();
    let status = notification.turn.status;
    let item_status = match status {
        TurnStatus::Interrupted => ItemStatus::Stopped,
        TurnStatus::Failed => ItemStatus::Failed,
        _ => ItemStatus::Completed,
    };
    // Every open item in that turn is closed *before* the settlement, so the transcript never
    // shows a spinner on a finished turn.
    session.close_turn_items(turn, item_status, &mut events);
    let duration_ms = notification
        .turn
        .duration_ms
        .and_then(|ms| u64::try_from(ms).ok())
        .unwrap_or_default();
    match status {
        TurnStatus::Interrupted => events.push(AgentEvent::TurnAborted {
            turn,
            reason: AbortReason::User,
        }),
        TurnStatus::Failed => {
            let error = notification.turn.error.as_ref();
            events.push(AgentEvent::TurnSettled {
                turn,
                outcome: TurnOutcome::Error {
                    message: error.map(failure_sentence),
                },
                usage: Usage::default(),
                duration_ms,
                files_changed: Vec::new(),
            });
        }
        // `inProgress` and an unknown status both settle as completed rather than leaving the UI
        // spinning on a turn the harness has stopped talking about.
        TurnStatus::Completed | TurnStatus::InProgress | TurnStatus::Unknown => {
            events.push(AgentEvent::TurnSettled {
                turn,
                outcome: TurnOutcome::Completed,
                usage: Usage::default(),
                duration_ms,
                files_changed: Vec::new(),
            });
        }
    }
    session.settle_turn(turn);
    MapOutput::from(events)
}

/// `turn/diff/updated`: the turn-level aggregated diff, free. Claude has no equivalent.
pub(in crate::agents::codex) fn diff_updated(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: TurnDiffUpdatedNotification = match decode("turn/diff/updated", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return MapOutput::default();
    }
    MapOutput::one(AgentEvent::TurnDiff {
        turn: session.turn_for(&notification.turn_id),
        unified: notification.diff,
        files_changed: Vec::new(),
    })
}

/// `turn/plan/updated`: the **todo list**, not the proposed plan.
pub(in crate::agents::codex) fn plan_updated(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: TurnPlanUpdatedNotification = match decode("turn/plan/updated", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return MapOutput::default();
    }
    let steps = notification
        .plan
        .iter()
        .map(|step| step.step.clone())
        .collect::<Vec<_>>();
    MapOutput::one(AgentEvent::PlanSteps {
        turn: session.turn_for(&notification.turn_id),
        steps,
    })
}

/// `error`: **not terminal.**
///
/// The live ordering on a real failure was `thread/status/changed{idle}` → `error` →
/// `turn/completed{status:"failed"}`. `willRetry: true` means Codex owns the retry and the turn
/// stays running, so Fleet renders a retry notice and keeps the spinner. Only
/// `turn/completed.status` decides the outcome.
pub(in crate::agents::codex) fn error(session: &mut CodexSession, params: &Value) -> MapOutput {
    let notification: ErrorNotification = match decode("error", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    let turn = session.turn_for(&notification.turn_id);
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        session.subagents.clear_live_turn(&notification.thread_id);
        return MapOutput::default();
    }
    if !session.may_apply(LifecycleKind::RuntimeError, Some(turn)) {
        return MapOutput::default();
    }
    if notification.will_retry {
        return MapOutput::one(AgentEvent::Retrying {
            attempt: 0,
            retry_in_ms: 0,
            reason: notification.error.message.clone(),
        });
    }
    // The failing `turn/completed` repeats this sentence, so the usage-limit error is suppressed
    // here rather than rendered twice.
    if matches!(
        error_tag(&notification.error).as_deref(),
        Some("usageLimitExceeded")
    ) {
        return MapOutput::default();
    }
    MapOutput::one(AgentEvent::RuntimeError {
        fatal: false,
        message: failure_sentence(&notification.error),
    })
}

/// The typed tag of a `codexErrorInfo`, whether or not this build knows the variant.
fn error_tag(error: &crate::agents::codex::wire::TurnError) -> Option<String> {
    use crate::agents::codex::wire::CodexErrorInfoKnown as Known;

    let info = error.codex_error_info.as_ref()?;
    if let Some(tag) = info.unknown_tag() {
        return Some(tag.to_owned());
    }
    Some(
        match info.known()? {
            Known::ContextWindowExceeded => "contextWindowExceeded",
            Known::SessionBudgetExceeded => "sessionBudgetExceeded",
            Known::UsageLimitExceeded => "usageLimitExceeded",
            Known::ServerOverloaded => "serverOverloaded",
            Known::CyberPolicy => "cyberPolicy",
            Known::InternalServerError => "internalServerError",
            Known::Unauthorized => "unauthorized",
            Known::BadRequest => "badRequest",
            Known::ThreadRollbackFailed => "threadRollbackFailed",
            Known::SandboxError => "sandboxError",
            Known::Other => "other",
            Known::HttpConnectionFailed(_) => "httpConnectionFailed",
            Known::ResponseStreamConnectionFailed(_) => "responseStreamConnectionFailed",
            Known::ResponseStreamDisconnected(_) => "responseStreamDisconnected",
            Known::ResponseTooManyFailedAttempts(_) => "responseTooManyFailedAttempts",
            Known::ActiveTurnNotSteerable(_) => "activeTurnNotSteerable",
        }
        .to_owned(),
    )
}

/// The sentence a turn error becomes.
///
/// `codexErrorInfo` is a **typed** taxonomy and four members earn distinct copy. The usage-limit
/// message in particular is **rewritten**: OpenAI's own sentence blames credits for a window that
/// simply ran out, which is actively misleading on a Business workspace.
pub(in crate::agents::codex) fn failure_sentence(
    error: &crate::agents::codex::wire::TurnError,
) -> String {
    match error_tag(error).as_deref() {
        Some("contextWindowExceeded") => {
            "This thread filled the model's context window. Compact it and try again.".to_owned()
        }
        Some("usageLimitExceeded") => {
            "Codex's usage limit was reached. Send the message again once the limit resets."
                .to_owned()
        }
        Some("serverOverloaded") => {
            "The selected model is at capacity — try another model.".to_owned()
        }
        Some("unauthorized") => {
            "Codex is not signed in. Run `codex` in a terminal on this worktree to sign in."
                .to_owned()
        }
        // Every other member keeps the harness's own message, which is harness-authored copy and
        // not a Fleet payload echo.
        _ => error.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::agents::codex::wire::{CodexErrorInfoKnown, TurnError};

    use super::*;

    fn error_with(info: Option<crate::agents::codex::wire::CodexErrorInfo>) -> TurnError {
        TurnError {
            additional_details: None,
            codex_error_info: info,
            message: "Selected model is at capacity. Please try a different model.".to_owned(),
        }
    }

    #[test]
    fn the_four_typed_errors_earn_their_own_sentence() {
        let overloaded = error_with(Some(crate::agents::codex::tolerant::Tolerant::Known(
            CodexErrorInfoKnown::ServerOverloaded,
        )));
        assert_eq!(
            failure_sentence(&overloaded),
            "The selected model is at capacity — try another model."
        );
        let context = error_with(Some(crate::agents::codex::tolerant::Tolerant::Known(
            CodexErrorInfoKnown::ContextWindowExceeded,
        )));
        assert!(failure_sentence(&context).contains("Compact"));
    }

    /// A value added upstream without a version bump keeps the harness's own message rather than
    /// failing the decode or losing the error.
    #[test]
    fn an_unknown_error_info_keeps_the_harnesss_own_message() {
        let added = error_with(Some(crate::agents::codex::tolerant::Tolerant::Unknown(
            serde_json::json!("rateLimitExceeded"),
        )));
        assert_eq!(error_tag(&added).as_deref(), Some("rateLimitExceeded"));
        assert!(failure_sentence(&added).contains("capacity"));
    }

    #[test]
    fn a_retrying_error_never_becomes_a_runtime_error() {
        let mut session = CodexSession {
            root: Some("t".to_owned()),
            ..CodexSession::default()
        };
        let turn = "01a089f2-579b-75c2-8e51-3492ec617046";
        session.begin_turn(session.turn_for(turn));
        let mapped = session.turn_for(turn);
        session.adopt_turn(mapped, turn);
        let params = serde_json::json!({
            "threadId": "t",
            "turnId": turn,
            "willRetry": true,
            "error": {"message": "stream disconnected"},
        });
        let output = error(&mut session, &params);
        assert!(matches!(
            output.events.first(),
            Some(AgentEvent::Retrying { .. })
        ));
        assert_eq!(session.active_turn, Some(mapped), "the turn keeps running");
    }
}
