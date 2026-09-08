//! Tolerant Claude Code stream-json wire types.

use serde::Deserialize;
use serde_json::Value;

use super::ProviderError;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SystemMessage {
    pub subtype: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct AssistantMessage {
    pub message: MessageBody,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct UserMessage {
    pub message: MessageBody,
    #[serde(default)]
    pub tool_use_result: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct MessageBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub content: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct StreamMessage {
    pub event: Value,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ResultMessage {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub usage: Value,
    #[serde(default, rename = "modelUsage")]
    pub model_usage: Value,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub permission_denials: Vec<Value>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub result: Option<String>,
    /// Queued user sends the CLI still owns; a supersede leaves this above zero.
    #[serde(default)]
    pub queued_turn_count: u64,
    /// The user messages this result answers, which a steer coalesces into one turn.
    #[serde(default)]
    pub user_message_uuids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ToolProgressMessage {
    pub tool_use_id: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub elapsed_time_seconds: f64,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ControlRequestMessage {
    pub request_id: String,
    pub request: ControlRequest,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ControlRequest {
    pub subtype: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub permission_suggestions: Vec<Value>,
    #[serde(default)]
    pub decision_reason: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub suppress_always_allow_rule: bool,
    /// The CLI's own safe answer: deny is the focused option when this is set.
    #[serde(default)]
    pub default_to_no: bool,
    /// The request may not be answered automatically, whatever the permission mode says.
    #[serde(default)]
    pub requires_user_interaction: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ControlResponseMessage {
    pub response: Value,
}

/// A control request the sender stopped waiting for; it carries no response.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ControlCancelMessage {
    pub request_id: String,
}

#[derive(Debug, Clone)]
pub(super) enum ClaudeMessage {
    System(SystemMessage),
    Assistant(AssistantMessage),
    User(UserMessage),
    Stream(StreamMessage),
    Result(ResultMessage),
    ToolProgress(ToolProgressMessage),
    ControlRequest(ControlRequestMessage),
    ControlResponse(ControlResponseMessage),
    ControlCancel(ControlCancelMessage),
    KeepAlive,
    /// Documented frame Fleet has no projection for; kept as a tracing diagnostic.
    Ignored {
        kind: String,
    },
    Unknown {
        kind: String,
        value: Value,
    },
}

impl ClaudeMessage {
    /// The CLI's own name for this frame, kept on every stored event (§11).
    ///
    /// The subtype is part of the name because `system` and `control_request` are envelopes: a
    /// stored `system` tells a reader nothing, `system/compact_boundary` tells them everything.
    pub(super) fn wire_type(&self) -> String {
        match self {
            Self::System(message) => format!("system/{}", message.subtype),
            Self::Assistant(_) => "assistant".to_owned(),
            Self::User(_) => "user".to_owned(),
            Self::Stream(message) => format!(
                "stream_event/{}",
                message
                    .event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            ),
            Self::Result(message) => format!("result/{}", message.subtype),
            Self::ToolProgress(_) => "tool_progress".to_owned(),
            Self::ControlRequest(message) => {
                format!("control_request/{}", message.request.subtype)
            }
            Self::ControlResponse(_) => "control_response".to_owned(),
            Self::ControlCancel(_) => "control_cancel_request".to_owned(),
            Self::KeepAlive => "keep_alive".to_owned(),
            Self::Ignored { kind } | Self::Unknown { kind, .. } => kind.clone(),
        }
    }
}

/// Top-level frames the protocol documents but Fleet deliberately does not project.
const IGNORED: [&str; 7] = [
    // Claude 2.1.263 emits `command_lifecycle` three times per turn; Fleet projects the command
    // through its tool rows, so the envelope is a diagnostic rather than an unknown frame.
    "command_lifecycle",
    "tool_use_summary",
    "auth_status",
    "rate_limit_event",
    "prompt_suggestion",
    "conversation_reset",
    "active_goal",
];

pub(super) fn parse_line(line: &str) -> Result<ClaudeMessage, ProviderError> {
    let value: Value = serde_json::from_str(line).map_err(|error| ProviderError::Protocol {
        message: format!("invalid Claude stream-json line: {error}"),
    })?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();

    macro_rules! decode {
        ($variant:ident, $ty:ty) => {
            serde_json::from_value::<$ty>(value.clone())
                .map(ClaudeMessage::$variant)
                .map_err(|error| ProviderError::Protocol {
                    message: format!("invalid Claude {kind} frame: {error}"),
                })
        };
    }

    match kind.as_str() {
        "system" => decode!(System, SystemMessage),
        "assistant" => decode!(Assistant, AssistantMessage),
        "user" => decode!(User, UserMessage),
        "stream_event" => decode!(Stream, StreamMessage),
        "result" => decode!(Result, ResultMessage),
        "tool_progress" => decode!(ToolProgress, ToolProgressMessage),
        "control_request" => decode!(ControlRequest, ControlRequestMessage),
        "control_response" => decode!(ControlResponse, ControlResponseMessage),
        "control_cancel_request" => decode!(ControlCancel, ControlCancelMessage),
        "keep_alive" => Ok(ClaudeMessage::KeepAlive),
        _ if IGNORED.contains(&kind.as_str()) => Ok(ClaudeMessage::Ignored { kind }),
        _ => Ok(ClaudeMessage::Unknown { kind, value }),
    }
}
