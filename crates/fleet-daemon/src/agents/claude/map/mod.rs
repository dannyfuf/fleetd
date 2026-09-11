//! Claude frames to normalized events.
//!
//! Split by concern rather than by frame: [`stream`] owns streaming and item identity,
//! [`settle`] owns the one authoritative `result`, [`gates`] owns the three decision surfaces,
//! [`system`] owns the `system/*` envelope and the parked turn, and [`tools`]/[`text`] are the
//! pure helpers both sides share.

pub(crate) mod gates;
pub(crate) mod settle;
pub(crate) mod stream;
pub(crate) mod system;
pub(crate) mod text;
pub(crate) mod tools;

use fleet_core::agents::AgentEvent;
use serde_json::Value;

use super::{frames::Frame, session::ClaudeSession};

/// What mapping one frame produced.
#[derive(Debug, Default)]
pub(crate) struct MapOutput {
    /// Normalized events, in order.
    pub(crate) events: Vec<AgentEvent>,
    /// Frames Fleet owes the CLI in reply, in order.
    pub(crate) writes: Vec<Value>,
    /// The CLI's own name for the frame these events were mapped from.
    pub(crate) raw: Option<String>,
}

impl From<Vec<AgentEvent>> for MapOutput {
    fn from(events: Vec<AgentEvent>) -> Self {
        Self {
            events,
            writes: Vec::new(),
            raw: None,
        }
    }
}

/// Maps one frame, stamping it with the CLI's own name for it.
pub(in crate::agents::claude) fn handle(session: &mut ClaudeSession, frame: Frame) -> MapOutput {
    let raw = frame.wire_type();
    // Every **durable** frame's session id updates the resume cursor. A hook frame's id is
    // transient and adopting it corrupts the cursor, so `durable_session_id` filters those out.
    if let Some(session_id) = frame.durable_session_id() {
        let session_id = session_id.to_owned();
        session.adopt_cursor(&session_id);
    }
    let mut output = dispatch(session, frame);
    output.raw = Some(raw);
    output
}

fn dispatch(session: &mut ClaudeSession, frame: Frame) -> MapOutput {
    match frame {
        Frame::System(frame) => system::system(session, frame),
        Frame::Assistant(frame) => stream::assistant(session, frame),
        Frame::User(frame) => stream::user(session, frame),
        Frame::Stream(frame) => stream::stream(session, frame),
        Frame::Result(frame) => settle::result(session, frame),
        Frame::RateLimit(frame) => system::rate_limit(session, frame),
        Frame::ToolProgress(frame) => tool_progress(session, &frame),
        Frame::ToolSummary(frame) => tool_summary(session, &frame),
        Frame::ControlRequest(frame) => gates::control_request(session, frame),
        // The transport owns the outbound pending map and resolves the response there; reaching
        // here means the response matched nothing, which is worth one notice and no more.
        Frame::ControlResponse(frame) => {
            if frame.response.subtype == "error" {
                let detail = frame
                    .response
                    .error
                    .unwrap_or_else(|| "no reason was given".to_owned());
                return MapOutput::from(vec![AgentEvent::Notice(format!(
                    "Claude rejected a control request: {detail}"
                ))]);
            }
            MapOutput::default()
        }
        Frame::ControlCancel(frame) => gates::control_cancel(session, &frame.request_id),
        Frame::KeepAlive | Frame::Silent { .. } => MapOutput::default(),
        // Counted, never silent: the UI can say "1 event Fleet doesn't understand" instead of
        // losing a tool call.
        Frame::Unknown { kind } => {
            tracing::warn!(
                target: "fleet::agents::claude",
                frame = %kind,
                "no mapping for this Claude frame kind"
            );
            MapOutput::from(vec![AgentEvent::Unknown { method: kind }])
        }
    }
}

/// `tool_progress`: a hint that keeps the row's elapsed time honest.
fn tool_progress(
    session: &mut ClaudeSession,
    frame: &super::frames::ToolProgressFrame,
) -> MapOutput {
    use fleet_core::agents::{ItemPatch, ItemPayloadPatch, ItemStatus, ToolPatch};

    let Some(item) = session.tool_items.get(&frame.tool_use_id).copied() else {
        tracing::debug!(
            target: "fleet::agents::claude",
            tool = %frame.tool_name,
            "progress for a Claude tool this thread never opened"
        );
        return MapOutput::default();
    };
    let suffix = frame
        .task_id
        .as_deref()
        .map(|task| format!(" · task {task}"))
        .unwrap_or_default();
    MapOutput::from(vec![AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                result: Some(serde_json::json!(format!(
                    "{:.1}s{suffix}",
                    frame.elapsed_time_seconds.max(0.0)
                ))),
                ..ToolPatch::default()
            }))),
            status: Some(ItemStatus::InProgress),
        },
    }])
}

/// `tool_use_summary`: the CLI's own one-line summary of a tool call.
fn tool_summary(session: &mut ClaudeSession, frame: &super::frames::ToolSummaryFrame) -> MapOutput {
    use fleet_core::agents::{ItemPatch, ItemPayloadPatch, ToolPatch};

    let Some(item) = session.tool_items.get(&frame.tool_use_id).copied() else {
        return MapOutput::default();
    };
    let Some(summary) = frame.summary.clone().filter(|text| !text.trim().is_empty()) else {
        return MapOutput::default();
    };
    MapOutput::from(vec![AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                summary: Some(summary),
                ..ToolPatch::default()
            }))),
            status: None,
        },
    }])
}
