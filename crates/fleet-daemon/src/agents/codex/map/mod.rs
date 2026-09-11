//! Codex notifications to normalized events.
//!
//! Two rules shape every function here.
//!
//! **A decode failure produces a degraded event, never silence.** Each notification is decoded
//! into its generated params type; a failure emits `AgentEvent::Unknown { method }` plus one
//! counted warning carrying a structural fingerprint. t3code's client silently swallows any
//! notification whose params fail to decode, so one new enum value can make a whole class of
//! events invisible with no error anywhere — that is the highest-value thing to do differently.
//!
//! **A hint is never authoritative.** `thread/status/changed` fires `active` *before*
//! `turn/started` and `idle` *before* `turn/completed`, so it moves the session label and never
//! the turn state. Only `turn/completed` settles a turn.

pub(in crate::agents::codex) mod items;
pub(in crate::agents::codex) mod threads;
pub(in crate::agents::codex) mod turns;

use fleet_core::agents::AgentEvent;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::session::CodexSession;
use crate::agents::harness::fingerprint::SchemaFingerprint;

/// What mapping one notification produced.
#[derive(Debug, Default)]
pub(crate) struct MapOutput {
    /// Normalized events, in order.
    pub(crate) events: Vec<AgentEvent>,
    /// A follow-up request Fleet owes the server, by method name.
    ///
    /// The transport issues it; the mapper never awaits, so a notification can ask for
    /// `skills/list` without the read loop blocking on a round trip.
    pub(crate) follow_up: Vec<&'static str>,
}

impl From<Vec<AgentEvent>> for MapOutput {
    fn from(events: Vec<AgentEvent>) -> Self {
        Self {
            events,
            follow_up: Vec::new(),
        }
    }
}

impl MapOutput {
    /// One event.
    pub(crate) fn one(event: AgentEvent) -> Self {
        Self::from(vec![event])
    }
}

/// Maps one server notification.
pub(in crate::agents::codex) fn handle(
    session: &mut CodexSession,
    method: &str,
    params: &Value,
) -> MapOutput {
    match method {
        "thread/started" => threads::started(session, params),
        "thread/status/changed" => threads::status_changed(session, params),
        "thread/settings/updated" => threads::settings_updated(session, params),
        "thread/tokenUsage/updated" => threads::token_usage(session, params),
        "thread/name/updated" => threads::name_updated(params),
        "thread/closed" => threads::closed(session, params),
        "thread/environment/connected" => threads::environment(params, true),
        "thread/environment/disconnected" => threads::environment(params, false),
        "thread/compacted" => threads::compacted(params),
        "account/updated" => threads::account_updated(),
        "account/rateLimits/updated" => threads::rate_limits(params),
        "model/rerouted" => threads::model_rerouted(params),
        "model/safetyBuffering/updated" => threads::safety_buffering(params),
        "warning" | "guardianWarning" | "deprecationNotice" | "configWarning" => {
            threads::notice(method, params)
        }
        "mcpServer/startupStatus/updated" | "mcpServer/oauthLogin/completed" => {
            threads::mcp_status(method, params)
        }
        // No payload: the composer's `/` and `$` vocabularies are re-read, and nothing is
        // appended to the transcript for it.
        "skills/changed" => MapOutput {
            events: Vec::new(),
            follow_up: vec!["skills/list"],
        },
        "turn/started" => turns::started(session, params),
        "turn/completed" => turns::completed(session, params),
        "turn/diff/updated" => turns::diff_updated(session, params),
        "turn/plan/updated" => turns::plan_updated(session, params),
        "error" => turns::error(session, params),
        "item/started" => items::started(session, params),
        "item/completed" => items::completed(session, params),
        "item/agentMessage/delta" => items::agent_message_delta(session, params),
        "item/reasoning/summaryTextDelta" => items::reasoning_summary_delta(session, params),
        "item/reasoning/summaryPartAdded" => items::reasoning_part_added(session, params),
        "item/reasoning/textDelta" => items::reasoning_text_delta(session, params),
        "item/plan/delta" => items::plan_delta(session, params),
        "item/commandExecution/outputDelta" => items::command_output_delta(session, params),
        "item/commandExecution/terminalInteraction" => items::terminal_interaction(session, params),
        "item/fileChange/patchUpdated" => items::patch_updated(session, params),
        "item/mcpToolCall/progress" => items::mcp_progress(session, params),
        "item/autoApprovalReview/started" => items::auto_review(params, true),
        "item/autoApprovalReview/completed" => items::auto_review(params, false),
        "serverRequest/resolved" => threads::server_request_resolved(session, params),
        // Hooks are a diagnostic, not a transcript row; they are the one family whose session id
        // is transient on Claude and whose payload is CLI-internal on both.
        "hook/started" | "hook/completed" => MapOutput::default(),
        // `item/fileChange/outputDelta` is dead upstream: "The server no longer emits this
        // notification." Anything else is counted, never silent.
        "item/fileChange/outputDelta" => MapOutput::default(),
        other => {
            tracing::warn!(
                target: "fleet::agents::codex",
                method = other,
                "no mapping for this Codex notification"
            );
            MapOutput::one(AgentEvent::Unknown {
                method: other.to_owned(),
            })
        }
    }
}

/// Decodes one notification's params, or reports the failure structurally.
///
/// The `Err` arm is a degraded event and a fingerprint: the payload never reaches a log line.
pub(in crate::agents::codex) fn decode<T: DeserializeOwned>(
    method: &str,
    params: &Value,
) -> Result<T, MapOutput> {
    match serde_json::from_value::<T>(params.clone()) {
        Ok(decoded) => Ok(decoded),
        Err(error) => {
            let fingerprint = SchemaFingerprint::of(&error, params);
            tracing::warn!(
                target: "fleet::agents::codex",
                method,
                fingerprint = %fingerprint.summary(),
                "could not decode a Codex notification"
            );
            Err(MapOutput::one(AgentEvent::Unknown {
                method: method.to_owned(),
            }))
        }
    }
}

/// The `type` tag of an item, read before the typed decode so an unknown item can be named.
pub(in crate::agents::codex) fn item_type(params: &Value) -> String {
    params
        .pointer("/item/type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}
