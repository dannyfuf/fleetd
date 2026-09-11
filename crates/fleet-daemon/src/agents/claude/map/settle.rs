//! The `result` frame: the completion authority, exactly one per turn.
//!
//! All three of `subtype`, `is_error` and `terminal_reason` are read, because a
//! `subtype: "success"` can still carry `is_error: true`. `modelUsage` and `total_cost_usd` are
//! **cumulative for the process** — take the latest, never sum — and
//! `modelUsage[model].contextWindow` is the context meter's denominator, published only here.
//!
//! The flush order at settlement is the contract: close every open item in that turn, publish
//! usage, then settle the turn, then reset the session.

use std::collections::{BTreeMap, HashSet};

use fleet_core::agents::{AbortReason, AgentEvent, FileDelta, ItemStatus, TurnOutcome, Usage};
use serde_json::Value;

use super::MapOutput;
use crate::agents::claude::{frames::ResultFrame, session::ClaudeSession};

/// Maps one `result` frame.
pub(in crate::agents::claude) fn result(
    session: &mut ClaudeSession,
    frame: ResultFrame,
) -> MapOutput {
    let mut usage = usage_from_value(&frame.usage);
    if usage_is_empty(&usage) {
        if let Some(last) = &session.last_usage {
            usage = last.clone();
        }
    } else {
        session.last_usage = Some(usage.clone());
    }
    usage.tool_uses = session.tool_uses;
    // Occupancy is measured per turn; an aborted or empty frame reports nothing rather than an
    // empty window, so the last measured level stands.
    let context_tokens = context_tokens(&frame.usage).or(session.last_context_tokens);
    if context_tokens.is_some() {
        session.last_context_tokens = context_tokens;
    }
    let context_pct = context_pct(
        context_tokens,
        &frame.model_usage,
        session.session_model.as_deref(),
    );

    // A `result` with no locally-tracked turn emits usage and a tripwire, and **no settlement**:
    // an untargeted completion cannot be attributed, and flipping the session lifecycle for a
    // turn that never existed is what makes a thread claim work it never did.
    if !session.may_settle() {
        tracing::warn!(
            target: "fleet::agents::claude",
            subtype = %frame.subtype,
            "a Claude result arrived with no active turn; recording usage only"
        );
        let events = session
            .last_turn
            .map(|turn| AgentEvent::TokenUsage {
                turn,
                usage,
                context_pct,
                cost_usd: frame.total_cost_usd,
            })
            .into_iter()
            .collect::<Vec<_>>();
        return MapOutput::from(events);
    }
    let Some(turn) = session.active_turn() else {
        return MapOutput::default();
    };

    let terminal_reason = frame.terminal_reason.as_deref();
    let aborted = matches!(terminal_reason, Some("aborted_streaming" | "aborted_tools"))
        || interrupt_shaped(&frame);
    let interrupted = session.interrupted.contains(&turn);
    // A steer supersedes the in-flight stream and the CLI reports that supersede as an aborted
    // `result` while the turn keeps running: the turn settles only after the last in-flight send.
    let superseded =
        aborted && !interrupted && (frame.queued_turn_count > 0 || session.pending_steers > 0);
    tracing::debug!(
        target: "fleet::agents::claude",
        %turn,
        sends = frame.user_message_uuids.len(),
        queued_turn_count = frame.queued_turn_count,
        pending_steers = session.pending_steers,
        superseded,
        "Claude result",
    );
    if superseded {
        session.pending_steers = session.pending_steers.saturating_sub(1);
        session.stream_blocks.clear();
        session.current_message = None;
        return MapOutput::from(vec![AgentEvent::TokenUsage {
            turn,
            usage,
            context_pct,
            cost_usd: frame.total_cost_usd,
        }]);
    }

    session.active_turn = None;
    session.pending_start = None;
    let mut events = Vec::new();
    // Close every open item *in that turn* — not every open item there is. A background task is
    // the one row whose lifetime the turn does not own, so a live one survives its turn's
    // terminal, and a row left open by an earlier turn is not this turn's to settle either.
    let background = session
        .background_tasks
        .iter()
        .filter_map(|task| session.task_items.get(task).copied())
        .collect::<HashSet<_>>();
    // A tool that never returned a `tool_result` did not succeed: a failed row stays exposed when
    // the turn folds, which a silent `Done` would hide.
    let open = session
        .open_items
        .iter()
        .filter(|(item, open)| open.turn == turn && !background.contains(*item))
        .map(|(item, open)| (*item, open.tool_name.is_some()))
        .collect::<Vec<_>>();
    for (item, is_tool) in open {
        let status = if is_tool {
            ItemStatus::Failed
        } else {
            ItemStatus::Completed
        };
        session.complete_item(item, status, &mut events);
    }
    // Cost and context share the result frame but not the terminal event, so they are published
    // just before it and the terminal event stays last in the turn.
    events.push(AgentEvent::TokenUsage {
        turn,
        usage: usage.clone(),
        context_pct,
        cost_usd: frame.total_cost_usd,
    });
    if aborted {
        let reason = if interrupted {
            AbortReason::User
        } else {
            AbortReason::Other(terminal_reason.unwrap_or("aborted").to_owned())
        };
        events.push(AgentEvent::TurnAborted { turn, reason });
        session.files_changed.clear();
    } else {
        let outcome = turn_outcome(&frame, session.failure_latch.as_deref());
        let files_changed = std::mem::take(&mut session.files_changed)
            .into_iter()
            .map(|(path, (added, removed))| FileDelta {
                path,
                added,
                removed,
            })
            .collect();
        events.push(AgentEvent::TurnSettled {
            turn,
            outcome,
            usage,
            duration_ms: frame.duration_ms,
            files_changed,
        });
    }
    // A `/compact` turn that settled without a boundary frame gets a synthesised one, so the
    // thread stops claiming it is compacting.
    if session.compacting == Some(turn) {
        session.compacting = None;
        events.push(AgentEvent::Compacted(
            fleet_core::agents::CheckpointKind::CompactBoundary {
                before: session.last_context_tokens.unwrap_or_default(),
                after: None,
            },
        ));
    }
    session.stream_blocks.clear();
    session.current_message = None;
    session.interrupted.remove(&turn);
    session.tool_uses = 0;
    session.pending_steers = 0;
    session.failure_latch = None;
    session.announced_windows.clear();
    session.prune_settled_items();
    MapOutput::from(events)
}

/// Whether the CLI's own error text describes an interruption rather than a failure.
fn interrupt_shaped(frame: &ResultFrame) -> bool {
    frame.errors.iter().any(|error| {
        let lower = error.to_ascii_lowercase();
        ["interrupt", "aborted", "interrupted by user"]
            .iter()
            .any(|token| lower.contains(token))
    })
}

/// Whether a usage frame reports nothing at all, which a cumulative counter never does twice.
#[must_use]
pub(in crate::agents::claude) fn usage_is_empty(usage: &Usage) -> bool {
    usage.total_tokens == 0
        && usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_write_tokens == 0
}

/// Normalizes a Claude `usage` object, keeping every additive field losslessly.
#[must_use]
pub(in crate::agents::claude) fn usage_from_value(value: &Value) -> Usage {
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let reasoning_tokens = value
        .get("output_tokens_details")
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_read_tokens = value
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_write_tokens = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let web_search_requests = value
        .get("server_tool_use")
        .and_then(|tools| tools.get("web_search_requests"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut extra = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if !matches!(
                key.as_str(),
                "input_tokens"
                    | "output_tokens"
                    | "output_tokens_details"
                    | "cache_read_input_tokens"
                    | "cache_creation_input_tokens"
                    | "server_tool_use"
            ) {
                extra.insert(key.clone(), value.clone());
            }
        }
    }
    Usage {
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens: input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_read_tokens)
            .saturating_add(cache_write_tokens),
        web_search_requests,
        tool_uses: 0,
        extra,
    }
}

/// The tokens the conversation itself occupies at the end of a turn, if the frame measured any.
///
/// Occupancy is a level, not a total: `usage.iterations` is the per-request breakdown, and its
/// *last* entry is what the model was holding when the turn ended. Summing the iterations counts
/// the same resident context once per request and walks a long thread to 100% on an empty window.
#[must_use]
pub(in crate::agents::claude) fn context_tokens(usage: &Value) -> Option<u64> {
    let resident = |value: &Value| {
        [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_u64))
        .fold(0_u64, u64::saturating_add)
    };
    let tokens = usage
        .get("iterations")
        .and_then(Value::as_array)
        .and_then(|iterations| iterations.last())
        .map_or_else(|| resident(usage), resident);
    (tokens > 0).then_some(tokens)
}

/// How much of the session model's context window the turn is holding.
///
/// `modelUsage` is keyed by model id, is cumulative across the query process and includes
/// pipeline subcalls: a summarisation entry sits in there beside the model actually answering.
/// `serde_json::Map` is a `BTreeMap`, so taking the first entry takes the alphabetically-first
/// model id and with it that model's window. The session's own model is looked up by name, and
/// the widest window is the fallback, since a subcall's window is never the larger of the two.
#[must_use]
pub(in crate::agents::claude) fn context_pct(
    tokens: Option<u64>,
    model_usage: &Value,
    session_model: Option<&str>,
) -> f32 {
    let Some(used) = tokens else {
        return 0.0;
    };
    let Some(models) = model_usage.as_object() else {
        return 0.0;
    };
    let model = session_model
        .and_then(|model| models.get(model))
        .or_else(|| {
            models.values().max_by_key(|model| {
                model
                    .get("contextWindow")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            })
        });
    let Some(model) = model else {
        return 0.0;
    };
    let window = model
        .get("contextWindow")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if window == 0 {
        0.0
    } else {
        (used as f64 * 100.0 / window as f64).min(100.0) as f32
    }
}

/// The terminal classification of one `result`.
///
/// The mapped sentence for a named `terminal_reason` is the harness's own vocabulary turned into
/// something a person can read; an unknown reason falls through to the subtype rather than
/// becoming an error, and errors prefixed `[ede_diagnostic]` are **never shown to the user**.
#[must_use]
pub(in crate::agents::claude) fn turn_outcome(
    frame: &ResultFrame,
    latch: Option<&str>,
) -> TurnOutcome {
    let reason = frame.terminal_reason.as_deref();
    if matches!(frame.subtype.as_str(), "error_max_turns") || reason == Some("max_turns") {
        return TurnOutcome::MaxTurns;
    }
    if matches!(frame.subtype.as_str(), "error_max_budget_usd")
        || reason == Some("budget_exhausted")
    {
        return TurnOutcome::BudgetExhausted;
    }
    if !frame.permission_denials.is_empty() && frame.is_error {
        return TurnOutcome::Denied;
    }
    // 529 is the only structured signal for an overloaded API, and it can ride a successful
    // subtype.
    if frame.api_error_status == Some(529) {
        return TurnOutcome::Error {
            message: Some("Claude's API is overloaded (529). Try again shortly.".to_owned()),
        };
    }
    if frame.is_error || frame.subtype != "success" {
        return TurnOutcome::Error {
            message: reason
                .and_then(mapped_reason)
                .map(ToOwned::to_owned)
                .or_else(|| frame.errors.iter().find_map(|error| user_facing(error)))
                .or_else(|| frame.result.clone())
                .or_else(|| latch.and_then(latched_reason).map(ToOwned::to_owned))
                .or_else(|| reason.map(ToOwned::to_owned)),
        };
    }
    match reason {
        None | Some("completed") => TurnOutcome::Completed,
        Some(other) => match mapped_reason(other) {
            // A named cause on an otherwise successful frame is still a failure the user must see.
            Some(sentence) => TurnOutcome::Error {
                message: Some(sentence.to_owned()),
            },
            None => TurnOutcome::Other {
                reason: other.to_owned(),
            },
        },
    }
}

/// The sentence a named `terminal_reason` becomes. `None` means "not a cause Fleet names".
fn mapped_reason(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "api_error" => "Claude's API returned an error.",
        "malformed_tool_use_exhausted" => "Claude could not produce a usable tool call.",
        "structured_output_retry_exhausted" => "Claude could not produce the requested output.",
        "tool_deferred_unavailable" => "A tool Claude needed was unavailable.",
        "turn_setup_failed" => "Claude could not start the turn.",
        "blocking_limit" => "Claude hit a blocking limit.",
        "rapid_refill_breaker" => "Claude's usage breaker tripped.",
        "prompt_too_long" => "The prompt is too long for this model's context window.",
        "image_error" => "An attached image could not be read.",
        "model_error" => "The model returned an error.",
        _ => return None,
    })
}

/// The sentence a failure latch becomes, when the `result` named no cause of its own.
fn latched_reason(latch: &str) -> Option<&'static str> {
    Some(match latch {
        "authentication_failed" => {
            "Claude Code is not signed in. Run `claude` in a terminal on this worktree to sign in."
        }
        "rate_limit" => "Claude's usage limit was reached.",
        _ => return None,
    })
}

/// Whether a harness error string may be shown at all.
fn user_facing(error: &str) -> Option<String> {
    // `[ede_diagnostic]` errors are internal diagnostics and leak implementation detail into a
    // user-facing red row.
    if error.starts_with("[ede_diagnostic]") {
        return None;
    }
    Some(error.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(subtype: &str, is_error: bool, reason: Option<&str>) -> ResultFrame {
        ResultFrame {
            subtype: subtype.to_owned(),
            is_error,
            terminal_reason: reason.map(ToOwned::to_owned),
            ..ResultFrame::default()
        }
    }

    #[test]
    fn all_three_terminal_fields_are_read() {
        assert_eq!(
            turn_outcome(&frame("success", false, Some("completed")), None),
            TurnOutcome::Completed
        );
        // A successful subtype carrying is_error is still a failure.
        assert!(matches!(
            turn_outcome(&frame("success", true, None), None),
            TurnOutcome::Error { .. }
        ));
        // An unknown terminal reason falls through to the subtype, never to an error.
        assert_eq!(
            turn_outcome(&frame("success", false, Some("brand_new_reason")), None),
            TurnOutcome::Other {
                reason: "brand_new_reason".to_owned()
            }
        );
    }

    #[test]
    fn an_overloaded_api_is_named_from_its_status_code() {
        let mut overloaded = frame("success", false, None);
        overloaded.api_error_status = Some(529);
        let TurnOutcome::Error { message } = turn_outcome(&overloaded, None) else {
            panic!("529 must fail the turn");
        };
        assert_eq!(
            message.as_deref(),
            Some("Claude's API is overloaded (529). Try again shortly.")
        );
    }

    #[test]
    fn an_internal_diagnostic_is_never_shown_and_the_latch_speaks_instead() {
        let mut failed = frame("error_during_execution", true, None);
        failed.errors = vec!["[ede_diagnostic] internal shape mismatch".to_owned()];
        let TurnOutcome::Error { message } = turn_outcome(&failed, Some("authentication_failed"))
        else {
            panic!("the turn must fail");
        };
        let message = message.unwrap_or_default();
        assert!(!message.contains("ede_diagnostic"), "{message}");
        assert!(message.contains("not signed in"), "{message}");
    }

    #[test]
    fn the_context_meter_measures_the_session_model_and_not_the_first_key() {
        let usage = serde_json::json!({
            "input_tokens": 10,
            "cache_read_input_tokens": 90,
        });
        let models = serde_json::json!({
            "aaa-summariser": {"contextWindow": 1_000},
            "claude-opus-5": {"contextWindow": 200_000},
        });
        let tokens = context_tokens(&usage);
        assert_eq!(tokens, Some(100));
        let pct = context_pct(tokens, &models, Some("claude-opus-5"));
        assert!((pct - 0.05).abs() < 0.001, "{pct}");
        // With no session model the widest window wins, never the alphabetically first.
        let fallback = context_pct(tokens, &models, None);
        assert!((fallback - 0.05).abs() < 0.001, "{fallback}");
    }

    #[test]
    fn context_occupancy_reads_the_last_iteration_rather_than_the_sum() {
        let usage = serde_json::json!({
            "input_tokens": 1,
            "iterations": [
                {"input_tokens": 10, "cache_read_input_tokens": 0},
                {"input_tokens": 5, "cache_read_input_tokens": 20},
            ],
        });
        assert_eq!(context_tokens(&usage), Some(25));
    }
}
