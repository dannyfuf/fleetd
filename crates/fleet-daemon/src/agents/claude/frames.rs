//! Tolerant serde types for the Claude Code stream-json frames, with an `Unknown` arm on every
//! closed set.
//!
//! Two rules from §4.5 are implemented here rather than described:
//!
//! 1. **A decode failure produces a degraded frame, never silence.** [`parse_frame`] cannot fail:
//!    a line that is not JSON, or whose shape does not fit, becomes
//!    [`ParsedFrame::Undecodable`] carrying a structural fingerprint, and the caller turns that
//!    into `AgentEvent::Unknown` plus one rate-limited warning. t3code's client silently swallows
//!    these, which is how one new enum value makes a whole class of events invisible.
//! 2. **The known-noise list is data, not match arms**, so it extends without a protocol change.

use serde::Deserialize;
use serde_json::Value;

use crate::agents::harness::fingerprint::{IssueKind, SchemaFingerprint};

/// A `system` frame; the subtype is the discriminator and the rest stays as fields.
///
/// Flattened rather than enumerated because `system` carries thirty-odd subtypes with disjoint
/// payloads and new ones arrive without a version bump.
#[derive(Debug, Clone, Deserialize)]
pub struct SystemFrame {
    /// The subtype, which is the real frame kind.
    pub subtype: String,
    /// Everything else the frame carried.
    #[serde(flatten)]
    pub fields: serde_json::Map<String, Value>,
}

/// An `assistant` snapshot frame. One frame **per content block**; several share one message id.
#[derive(Debug, Clone, Deserialize)]
pub struct AssistantFrame {
    /// The message envelope.
    pub message: MessageBody,
    /// Set when this output belongs to a subagent.
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    /// A failure latch (`authentication_failed`, `rate_limit`).
    #[serde(default)]
    pub error: Option<String>,
    /// The durable session id, which is the resume cursor.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// A `user` frame, which is how tool results arrive.
#[derive(Debug, Clone, Deserialize)]
pub struct UserFrame {
    /// The message envelope, carrying `tool_result` content blocks.
    pub message: MessageBody,
    /// The **structured** result, which is what a `Read` row's line count is computed from.
    #[serde(default)]
    pub tool_use_result: Option<Value>,
    /// The durable session id.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// The `message` envelope shared by assistant and user frames.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageBody {
    /// The message id. Not the transcript atom: the content block is.
    #[serde(default)]
    pub id: Option<String>,
    /// The content blocks.
    #[serde(default)]
    pub content: Value,
}

/// A `stream_event` frame, the source of token-level streaming.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamFrame {
    /// The Anthropic streaming event.
    pub event: Value,
    /// Set when the block belongs to a subagent: narration is dropped, tools are not.
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

/// The `result` frame: the completion authority, exactly one per turn.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResultFrame {
    /// `success`, `error_during_execution`, …
    #[serde(default)]
    pub subtype: String,
    /// May be `true` even when `subtype == "success"`.
    #[serde(default)]
    pub is_error: bool,
    /// The terminal reason, separate from both `subtype` and `stop_reason`.
    #[serde(default)]
    pub terminal_reason: Option<String>,
    /// Harness-reported wall duration.
    #[serde(default)]
    pub duration_ms: u64,
    /// Per-turn usage.
    #[serde(default)]
    pub usage: Value,
    /// Cumulative per-model usage, including each model's `contextWindow`.
    #[serde(default, rename = "modelUsage")]
    pub model_usage: Value,
    /// Cumulative process cost. Take the latest, never sum.
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    /// Denials the CLI applied on its own.
    #[serde(default)]
    pub permission_denials: Vec<Value>,
    /// Harness error strings.
    #[serde(default)]
    pub errors: Vec<String>,
    /// The assistant's final text, when the CLI reported one.
    #[serde(default)]
    pub result: Option<String>,
    /// The only structured signal for an overloaded API (529).
    #[serde(default)]
    pub api_error_status: Option<i64>,
    /// How many steered sends the CLI folded into this turn.
    #[serde(default)]
    pub queued_turn_count: u64,
    /// The user messages this result answers.
    #[serde(default)]
    pub user_message_uuids: Vec<String>,
    /// Subagent accounting, including *why* one was refused.
    #[serde(default)]
    pub subagent_stats: Value,
    /// The durable session id.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// A `rate_limit_event` frame, which arrives unsolicited mid-stream.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitFrame {
    /// The whole `rate_limit_info` object.
    #[serde(default)]
    pub rate_limit_info: Value,
}

/// A `tool_progress` frame.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolProgressFrame {
    /// The tool this progress belongs to.
    pub tool_use_id: String,
    /// The tool's name.
    #[serde(default)]
    pub tool_name: String,
    /// Seconds elapsed.
    #[serde(default)]
    pub elapsed_time_seconds: f64,
    /// The subagent task, when the tool is running inside one.
    #[serde(default)]
    pub task_id: Option<String>,
}

/// A `tool_use_summary` frame.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolSummaryFrame {
    /// The tool being summarised.
    pub tool_use_id: String,
    /// The one-line summary.
    #[serde(default)]
    pub summary: Option<String>,
}

/// An inbound `control_request`, which is how every gate arrives.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlRequestFrame {
    /// The CLI-minted correlation id. Also the gate key.
    pub request_id: String,
    /// The request body.
    pub request: ControlRequest,
}

/// The body of an inbound control request.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlRequest {
    /// `can_use_tool`, `elicitation`, `hook_callback`, …
    pub subtype: String,
    /// The tool being requested; the discriminator between permission, question and plan.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// The label to show. Not `tool_name`.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The tool input. The card's payload comes from here, never from `description`.
    #[serde(default)]
    pub input: Value,
    /// The CLI's own suggested permission updates, with its own `destination`.
    #[serde(default)]
    pub permission_suggestions: Vec<Value>,
    /// Why the CLI is asking.
    #[serde(default)]
    pub decision_reason: Option<String>,
    /// A coarse classification of that reason.
    #[serde(default)]
    pub decision_reason_type: Option<String>,
    /// A title, when the CLI supplies one.
    #[serde(default)]
    pub title: Option<String>,
    /// The model's prose about what it is doing. A rationale, never the payload.
    #[serde(default)]
    pub description: Option<String>,
    /// The tool call this request gates.
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// Set when no stored rule may answer this request.
    #[serde(default)]
    pub suppress_always_allow_rule: bool,
    /// The CLI's own safe answer: deny is the focused option when this is set.
    #[serde(default)]
    pub default_to_no: bool,
    /// The request may not be answered automatically, whatever the mode says.
    #[serde(default)]
    pub requires_user_interaction: bool,
}

/// A `control_response` answering one of Fleet's own outbound control requests.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlResponseFrame {
    /// The response envelope.
    pub response: ControlResponseBody,
}

/// The body of a `control_response`.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlResponseBody {
    /// `success` or `error`.
    #[serde(default)]
    pub subtype: String,
    /// The request being answered.
    #[serde(default)]
    pub request_id: Option<String>,
    /// The payload, on success.
    #[serde(default)]
    pub response: Value,
    /// The CLI's own error text, on failure.
    #[serde(default)]
    pub error: Option<String>,
}

/// A `control_cancel_request`: the CLI stopped waiting. **Answer nothing.**
#[derive(Debug, Clone, Deserialize)]
pub struct ControlCancelFrame {
    /// The request being withdrawn.
    pub request_id: String,
}

/// One inbound Claude frame.
#[derive(Debug, Clone)]
pub enum Frame {
    /// A `system` frame.
    System(SystemFrame),
    /// An `assistant` snapshot.
    Assistant(AssistantFrame),
    /// A `user` frame, carrying tool results.
    User(UserFrame),
    /// A `stream_event`.
    Stream(StreamFrame),
    /// The turn's `result`.
    Result(ResultFrame),
    /// A `rate_limit_event`.
    RateLimit(RateLimitFrame),
    /// A `tool_progress` frame.
    ToolProgress(ToolProgressFrame),
    /// A `tool_use_summary` frame.
    ToolSummary(ToolSummaryFrame),
    /// An inbound control request: a gate.
    ControlRequest(ControlRequestFrame),
    /// A response to one of Fleet's outbound control requests.
    ControlResponse(ControlResponseFrame),
    /// A withdrawn control request.
    ControlCancel(ControlCancelFrame),
    /// A keep-alive.
    KeepAlive,
    /// A frame the protocol documents and Fleet deliberately does not project.
    Silent {
        /// The frame's own name.
        kind: String,
    },
    /// A frame this build has no mapping for. Counted, never silent.
    Unknown {
        /// The frame's own name.
        kind: String,
    },
}

impl Frame {
    /// The CLI's own name for this frame, stored on every event.
    ///
    /// The subtype is part of the name because `system` and `control_request` are envelopes: a
    /// stored `system` tells a reader nothing, `system/compact_boundary` tells them everything.
    #[must_use]
    pub fn wire_type(&self) -> String {
        match self {
            Self::System(frame) => format!("system/{}", frame.subtype),
            Self::Assistant(_) => "assistant".to_owned(),
            Self::User(_) => "user".to_owned(),
            Self::Stream(frame) => format!(
                "stream_event/{}",
                frame
                    .event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            Self::Result(frame) => format!("result/{}", frame.subtype),
            Self::RateLimit(_) => "rate_limit_event".to_owned(),
            Self::ToolProgress(_) => "tool_progress".to_owned(),
            Self::ToolSummary(_) => "tool_use_summary".to_owned(),
            Self::ControlRequest(frame) => format!("control_request/{}", frame.request.subtype),
            Self::ControlResponse(_) => "control_response".to_owned(),
            Self::ControlCancel(_) => "control_cancel_request".to_owned(),
            Self::KeepAlive => "keep_alive".to_owned(),
            Self::Silent { kind } | Self::Unknown { kind } => kind.clone(),
        }
    }

    /// The durable session id this frame carries, if it is one Fleet may adopt as the cursor.
    ///
    /// spec A.2.4: `hook_started`/`hook_progress`/`hook_response` carry a **transient** session
    /// id, and adopting one corrupts the cursor so the next resume opens the wrong conversation.
    /// Only the frames below are durable.
    #[must_use]
    pub fn durable_session_id(&self) -> Option<&str> {
        match self {
            Self::Assistant(frame) => frame.session_id.as_deref(),
            Self::User(frame) => frame.session_id.as_deref(),
            Self::Result(frame) => frame.session_id.as_deref(),
            Self::System(frame)
                if !TRANSIENT_SESSION_SUBTYPES.contains(&frame.subtype.as_str()) =>
            {
                frame.fields.get("session_id").and_then(Value::as_str)
            }
            _ => None,
        }
    }
}

/// `system` subtypes whose `session_id` is transient and must never become the resume cursor.
pub const TRANSIENT_SESSION_SUBTYPES: [&str; 3] =
    ["hook_started", "hook_progress", "hook_response"];

/// Top-level frames the protocol documents but Fleet deliberately does not project.
///
/// Data, not match arms. `conversation_reset` announces a CLI-side conversation id swap (`/clear`)
/// and Fleet keeps its own thread identity and cursor, so it is noise here.
pub const SILENT_FRAMES: [&str; 5] = [
    "command_lifecycle",
    "prompt_suggestion",
    "conversation_reset",
    "active_goal",
    "auth_status",
];

/// `system` subtypes Fleet reads for their side effects but never turns into a transcript row.
///
/// The regression test fires all of them plus four loud ones and asserts exactly four notices.
pub const SILENT_SYSTEM_SUBTYPES: [&str; 12] = [
    "local_command_output",
    "plugin_install",
    "commands_changed",
    "memory_recall",
    "elicitation_complete",
    "control_request_progress",
    "worker_shutting_down",
    "vcs_state_changed",
    "code_change_published",
    "hook_started",
    "hook_progress",
    "hook_response",
];

/// A parsed line: a frame, or a structural description of why it is not one.
///
/// The frame is boxed because it is by far the wider arm — an `assistant` snapshot carries whole
/// content blocks — and every line the reader parses moves one of these.
#[derive(Debug, Clone)]
pub enum ParsedFrame {
    /// A frame Fleet can dispatch.
    Frame(Box<Frame>),
    /// A line that did not decode. Never silence: the caller emits `AgentEvent::Unknown`.
    Undecodable {
        /// The frame's own `type`, when the line was at least JSON with one.
        kind: String,
        /// The structural description. Carries no payload.
        fingerprint: SchemaFingerprint,
    },
}

/// Parses one NDJSON line. Infallible by construction.
#[must_use]
pub fn parse_frame(line: &str) -> ParsedFrame {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return ParsedFrame::Undecodable {
                kind: "invalid_json".to_owned(),
                fingerprint: SchemaFingerprint::of_unparsable(&error),
            };
        }
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("untyped")
        .to_owned();

    macro_rules! decode {
        ($variant:ident, $ty:ty) => {
            match serde_json::from_value::<$ty>(value.clone()) {
                Ok(frame) => ParsedFrame::Frame(Box::new(Frame::$variant(frame))),
                Err(error) => ParsedFrame::Undecodable {
                    kind,
                    fingerprint: SchemaFingerprint::of(&error, &value),
                },
            }
        };
    }

    match kind.as_str() {
        "system" => decode!(System, SystemFrame),
        "assistant" => decode!(Assistant, AssistantFrame),
        "user" => decode!(User, UserFrame),
        "stream_event" => decode!(Stream, StreamFrame),
        "result" => decode!(Result, ResultFrame),
        "rate_limit_event" => decode!(RateLimit, RateLimitFrame),
        "tool_progress" => decode!(ToolProgress, ToolProgressFrame),
        "tool_use_summary" => decode!(ToolSummary, ToolSummaryFrame),
        "control_request" => decode!(ControlRequest, ControlRequestFrame),
        "control_response" => decode!(ControlResponse, ControlResponseFrame),
        "control_cancel_request" => decode!(ControlCancel, ControlCancelFrame),
        "keep_alive" => ParsedFrame::Frame(Box::new(Frame::KeepAlive)),
        _ if SILENT_FRAMES.contains(&kind.as_str()) => {
            ParsedFrame::Frame(Box::new(Frame::Silent { kind }))
        }
        "untyped" => ParsedFrame::Undecodable {
            fingerprint: SchemaFingerprint::of_value(IssueKind::MissingField, &value),
            kind,
        },
        _ => ParsedFrame::Frame(Box::new(Frame::Unknown { kind })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::harness::capture::{SECRETS, assert_no_secret};

    #[test]
    fn a_line_that_is_not_json_is_degraded_rather_than_dropped() {
        match parse_frame("{\"type\":\"system\"") {
            ParsedFrame::Undecodable { kind, fingerprint } => {
                assert_eq!(kind, "invalid_json");
                assert_eq!(fingerprint.issue_count, 1);
            }
            other => panic!("expected a degraded frame, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_with_no_type_is_degraded_and_named() {
        match parse_frame("{\"session_id\":\"x\"}") {
            ParsedFrame::Undecodable { kind, .. } => assert_eq!(kind, "untyped"),
            other => panic!("expected a degraded frame, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_frame_kind_is_named_and_not_an_error() {
        match parse_frame("{\"type\":\"holograph\",\"payload\":1}") {
            ParsedFrame::Frame(frame) => match *frame {
                Frame::Unknown { kind } => assert_eq!(kind, "holograph"),
                other => panic!("expected an unknown frame, got {other:?}"),
            },
            other => panic!("expected an unknown frame, got {other:?}"),
        }
    }

    /// Privacy test 1 of 3 (spec A.7.6): a decode failure describes the frame structurally and
    /// carries none of it.
    #[test]
    fn a_decode_failure_never_carries_the_payload_it_failed_on() {
        let line = format!(
            "{{\"type\":\"control_request\",\"request\":{{\"input\":{{\"token\":\"{}\",\"body\":\"{}\"}}}}}}",
            SECRETS[0], SECRETS[1]
        );
        // `request_id` is required, so this cannot decode.
        let parsed = parse_frame(&line);
        let ParsedFrame::Undecodable { kind, fingerprint } = parsed else {
            panic!("expected a degraded frame");
        };
        assert_eq!(kind, "control_request");
        let rendered = format!("{fingerprint:?} {}", fingerprint.summary());
        assert_no_secret(&rendered, "the Claude decode fingerprint");
        assert_eq!(
            fingerprint
                .present_fields
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["request", "type"]
        );
    }

    #[test]
    fn only_durable_frames_offer_a_resume_cursor() {
        let hook = parse_frame(
            "{\"type\":\"system\",\"subtype\":\"hook_started\",\"session_id\":\"transient\"}",
        );
        let ParsedFrame::Frame(hook) = hook else {
            panic!("hook frame must decode");
        };
        assert_eq!(hook.durable_session_id(), None);

        let init =
            parse_frame("{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"durable\"}");
        let ParsedFrame::Frame(init) = init else {
            panic!("init frame must decode");
        };
        assert_eq!(init.durable_session_id(), Some("durable"));
    }
}
