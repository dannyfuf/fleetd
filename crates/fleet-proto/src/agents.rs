//! Wire payloads shared by the native-agent request, response, and event families.
//!
//! Everything in this module is additive on protocol **7** and gated on one of the `agent.*`
//! capability strings in [`crate`]. A daemon that does not advertise
//! [`AGENT_WINDOW_CAPABILITY`](crate::AGENT_WINDOW_CAPABILITY) is only ever sent the version-6
//! [`RequestBody::AgentThreadOpen`](crate::request::RequestBody::AgentThreadOpen) shape and only
//! ever answers with the unbounded
//! [`ResponseBody::AgentThreadSnapshot`](crate::response::ResponseBody::AgentThreadSnapshot),
//! whose meaning never changes (`rust-ipc-protocol` Rule 7).
//!
//! The reason a bounded window exists at all is a decode failure, not a performance target:
//! `AgentThreadSnapshot` serializes a whole transcript into one frame against
//! [`MAX_FRAME_SIZE`](crate::codec::MAX_FRAME_SIZE), and past that ceiling the frame is
//! permanently *undecodable* — a codec error rather than a truncation, on a thread that is
//! otherwise perfectly readable. So the window carries a hard byte budget of its own
//! ([`WINDOW_MAX_WIRE_BYTES`]), an eighth of the frame limit, and the legacy snapshot gets a
//! refusal ([`SNAPSHOT_MAX_WIRE_BYTES`]) instead of a frame no peer can read.

mod checkpoints;

use std::io;

pub use checkpoints::{
    AgentRevertReport, CheckpointId, CheckpointScope, InvalidCheckpointId, ParsedCheckpointId,
    REVERT_PATH_SAMPLE, TurnCheckpoint, checkpoint_missing_error, checkpoints_capability_error,
};

use fleet_core::agents::{
    AgentThreadSummary, CheckpointRecord, GateId, HarnessCapabilities, Item, ModelSelection,
    NoticeRecord, OpenGate, PermissionMode, RetryState, Seq, ThreadId, TurnRecord, Usage,
};
use serde::{Deserialize, Serialize};

use crate::{
    codec::MAX_FRAME_SIZE,
    error::{ErrorKind, ProtoError},
};

/// Hard ceiling on one serialized [`TranscriptWindow`], asserted on every window response.
///
/// An eighth of [`MAX_FRAME_SIZE`], so a window plus its summary, session view, and live tail
/// still leaves the frame room to spare. A daemon that would exceed it must narrow the window —
/// fewer turns, elided item bodies the client re-reads with
/// [`RequestBody::AgentItemBody`](crate::request::RequestBody::AgentItemBody) — never enlarge the
/// frame.
pub const WINDOW_MAX_WIRE_BYTES: usize = 2 * 1024 * 1024;

/// Ceiling above which the legacy unbounded snapshot is refused rather than framed.
///
/// Half the frame limit, which leaves the envelope and the event tail their own room. Past this,
/// answering at all produces a frame the peer cannot decode, so the daemon returns
/// [`snapshot_ceiling_error`] naming the capability that fixes it.
pub const SNAPSHOT_MAX_WIRE_BYTES: usize = MAX_FRAME_SIZE / 2;

/// Largest slice one [`AgentItemBodyChunk`](crate::response::ResponseBody::AgentItemBodyChunk)
/// answers with; a larger `limit` is clamped, never refused.
pub const ITEM_BODY_MAX_CHUNK_BYTES: u32 = 256 * 1024;

/// Turns in a window when the client asks for no explicit `turn_limit`.
pub const WINDOW_DEFAULT_TURNS: u32 = 10;

/// Most turns one window response may carry, whatever the client asked for.
pub const WINDOW_MAX_TURNS: u32 = 200;

/// A bounded slice of one thread's transcript.
///
/// Projection *pieces* rather than a whole [`ThreadProjection`](fleet_core::agents::ThreadProjection):
/// an older page has to merge into what the client already holds, and a projection is not
/// mergeable while these vectors are. The client materializes the newest window into a
/// projection once and prepends older pages to it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptWindow {
    /// Turns in this window, chronological.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<TurnRecord>,
    /// Items belonging to those turns, in display order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    /// Compaction and resume boundaries inside the window, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CheckpointRecord>,
    /// Provider notices inside the window, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<NoticeRecord>,
    /// **Every** open gate, in opening order, whether or not its turn falls inside the window.
    ///
    /// A decision the user has to answer is never paged out: a gate that opened four turns
    /// before the window would otherwise be unanswerable until the user scrolled back to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<OpenGate>,
    /// Item identities whose body was elided from this window because it was too large.
    ///
    /// The client renders the head it has and asks for the rest with
    /// [`RequestBody::AgentItemBody`](crate::request::RequestBody::AgentItemBody).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elided: Vec<fleet_core::agents::ItemId>,
}

impl TranscriptWindow {
    /// Whether this window carries nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
            && self.items.is_empty()
            && self.checkpoints.is_empty()
            && self.notices.is_empty()
            && self.gates.is_empty()
    }

    /// Serialized size of this window, for the [`WINDOW_MAX_WIRE_BYTES`] assertion.
    ///
    /// # Errors
    ///
    /// Returns the serializer's own error when a payload cannot be encoded at all.
    pub fn wire_bytes(&self) -> Result<usize, serde_json::Error> {
        wire_bytes(self)
    }

    /// Whether this window fits [`WINDOW_MAX_WIRE_BYTES`].
    ///
    /// # Errors
    ///
    /// Returns the serializer's own error when a payload cannot be encoded at all.
    pub fn fits_wire_budget(&self) -> Result<bool, serde_json::Error> {
        self.wire_bytes().map(window_fits)
    }
}

/// Where the next older slice of a transcript starts, and whether one exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptPage {
    /// Opaque exclusive keyset cursor for the next older slice; `None` when fully loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_cursor: Option<String>,
    /// Whether older history exists behind this window.
    pub has_more: bool,
    /// Thread head at page read time.
    ///
    /// A client MUST have applied live events through this sequence before merging the page.
    /// Otherwise a streaming turn outside the window can have its deltas replayed on top of page
    /// content that already includes them, and the transcript grows a duplicate nobody can
    /// explain.
    pub thread_seq: Seq,
}

/// Harness session runtime as the daemon last projected it.
///
/// The controls in the composer are gated on this: an interaction the harness cannot express is
/// disabled rather than sent and silently ignored (`docs/NATIVE-AGENTS.md` §4.5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentSessionView {
    /// Active model, when the harness has reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// Active permission policy.
    pub mode: PermissionMode,
    /// Harness-native tool names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Harness-native slash command names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Harness-native skill names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Negotiated harness capabilities, absent until the harness has been probed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Box<HarnessCapabilities>>,
    /// Latest cumulative usage.
    ///
    /// The four fields below are the thread-header facts the unbounded projection used to carry
    /// for free. A window that dropped them would silently blank the context meter and the cost
    /// readout on every cold open, which reads as a bug in the meter rather than in the protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Latest cumulative cost, when the harness reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Latest context-window utilization percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_pct: Option<f32>,
    /// Retry the harness is currently backing off through, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrying: Option<RetryState>,
}

/// Thread header, session runtime, and one bounded transcript window read together.
///
/// The three travel as one payload because they are read in one database transaction: a window
/// whose head was read separately can be ahead of the window itself, the client resumes from too
/// far, and the events in between are never sent and never replayed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadWindow {
    /// Thread header and denormalized counters. Never the transcript.
    pub summary: AgentThreadSummary,
    /// Session runtime: model, mode, tools, commands, skills, capabilities.
    #[serde(default)]
    pub session: AgentSessionView,
    /// The bounded transcript slice.
    #[serde(default)]
    pub window: TranscriptWindow,
    /// Absent when the whole thread was returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<TranscriptPage>,
    /// Highest sequence applied to this thread at read time; the client resumes from here.
    pub head_seq: Seq,
    /// Highest sequence the projection has applied.
    ///
    /// `projected_seq < head_seq` means the daemon is mid-rebuild for this thread and the window
    /// is a prefix, not the whole truth — which the surface says rather than presents as
    /// complete.
    #[serde(default)]
    pub projected_seq: Seq,
    /// Ordered events after `head_seq`, when the daemon chose the replay path over a window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events_after: Vec<fleet_core::agents::SeqEvent>,
    /// Whether the owner has confirmed this content.
    ///
    /// `false` on a mirrored thread the local daemon answered from its own cache before the
    /// owner replied: the surface paints it as cached, and
    /// [`Event::AgentSynchronized`](crate::event::Event::AgentSynchronized) is the only
    /// transition into live.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synchronized: bool,
}

impl AgentThreadWindow {
    /// Serialized size of this whole response payload.
    ///
    /// # Errors
    ///
    /// Returns the serializer's own error when a payload cannot be encoded at all.
    pub fn wire_bytes(&self) -> Result<usize, serde_json::Error> {
        wire_bytes(self)
    }

    /// Whether this response fits [`WINDOW_MAX_WIRE_BYTES`].
    ///
    /// # Errors
    ///
    /// Returns the serializer's own error when a payload cannot be encoded at all.
    pub fn fits_wire_budget(&self) -> Result<bool, serde_json::Error> {
        self.wire_bytes().map(window_fits)
    }
}

/// Serialized byte length of `value` without materializing its bytes.
///
/// A window is measured before it is framed, and a 2 MiB scratch allocation per open is exactly
/// the cost the windowed read exists to avoid — so this counts through a sink instead.
///
/// # Errors
///
/// Returns the serializer's own error when a payload cannot be encoded at all.
pub fn wire_bytes<T>(value: &T) -> Result<usize, serde_json::Error>
where
    T: Serialize + ?Sized,
{
    let mut counter = ByteCounter::default();
    let mut serializer = serde_json::Serializer::new(&mut counter);
    value.serialize(&mut serializer)?;
    Ok(counter.bytes)
}

/// Whether a measured window fits [`WINDOW_MAX_WIRE_BYTES`].
#[must_use]
pub const fn window_fits(bytes: usize) -> bool {
    bytes <= WINDOW_MAX_WIRE_BYTES
}

/// Whether a measured legacy snapshot fits [`SNAPSHOT_MAX_WIRE_BYTES`].
#[must_use]
pub const fn snapshot_fits(bytes: usize) -> bool {
    bytes <= SNAPSHOT_MAX_WIRE_BYTES
}

/// Clamps a client-supplied item-body `limit` to [`ITEM_BODY_MAX_CHUNK_BYTES`].
///
/// A zero limit means "the default chunk": a client that asks for nothing is asking for the next
/// page, not for an empty answer.
#[must_use]
pub const fn clamp_item_body_limit(limit: u32) -> u32 {
    if limit == 0 || limit > ITEM_BODY_MAX_CHUNK_BYTES {
        ITEM_BODY_MAX_CHUNK_BYTES
    } else {
        limit
    }
}

/// Which of an item's append-only streams a window elides, and a client therefore pages back.
///
/// One function rather than two implementations: the daemon cuts this stream when it narrows a
/// window, and the client asks for this stream when it sees the item's id in
/// [`TranscriptWindow::elided`]. If they disagreed, a paged read would continue a body the head
/// on screen did not come from. `None` for an item kind that has no body large enough to be
/// worth paging — a user message, a subagent card, an error line.
#[must_use]
pub fn elided_stream(
    kind: &fleet_core::agents::ItemKind,
) -> Option<fleet_core::agents::StreamKind> {
    use fleet_core::agents::{ItemKind, StreamKind};
    match kind {
        ItemKind::Tool(_) => Some(StreamKind::CommandOutput),
        ItemKind::AssistantText { .. } => Some(StreamKind::AssistantText),
        ItemKind::Plan { .. } => Some(StreamKind::PlanText),
        // The longest raw part, which is the one the narrowing pass cuts.
        ItemKind::Reasoning { raw, .. } => raw
            .iter()
            .max_by_key(|(_, part)| part.len())
            .map(|(part, _)| StreamKind::ReasoningRaw { part: *part }),
        _ => None,
    }
}

/// Clamps a client-supplied `turn_limit` to [`WINDOW_MAX_TURNS`], defaulting an absent one.
#[must_use]
pub const fn clamp_turn_limit(turn_limit: Option<u32>) -> u32 {
    match turn_limit {
        None | Some(0) => WINDOW_DEFAULT_TURNS,
        Some(turns) if turns > WINDOW_MAX_TURNS => WINDOW_MAX_TURNS,
        Some(turns) => turns,
    }
}

/// The refusal for a window field sent to a daemon that never advertised the capability.
///
/// Typed rather than prose: a client that sees this disables paging for the connection instead
/// of substring-matching a sentence that may be translated or reworded.
#[must_use]
pub fn window_capability_error() -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unsupported,
        message: format!(
            "this daemon does not implement windowed agent transcripts (capability `{}`)",
            crate::AGENT_WINDOW_CAPABILITY
        ),
    }
}

/// The refusal for a legacy snapshot that would exceed [`SNAPSHOT_MAX_WIRE_BYTES`].
#[must_use]
pub fn snapshot_ceiling_error(thread: ThreadId, bytes: usize) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unsupported,
        message: format!(
            "agent thread {thread} needs {bytes} bytes as an unbounded snapshot, over the \
             {SNAPSHOT_MAX_WIRE_BYTES}-byte ceiling; upgrade this peer for capability `{}`",
            crate::AGENT_WINDOW_CAPABILITY
        ),
    }
}

/// The refusal for a gate answer that arrived after the gate closed.
///
/// [`ErrorKind::Conflict`] is load-bearing: the surface clears the card on this and restores it
/// only on a transient failure. A card restored because a "gate already resolved" error was read
/// as transient is a card the user can never dismiss.
#[must_use]
pub fn gate_closed_error(gate: GateId) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Conflict,
        message: format!("agent gate {gate} was already resolved or withdrawn"),
    }
}

/// Counts serialized bytes and keeps none of them.
#[derive(Debug, Default)]
struct ByteCounter {
    bytes: usize,
}

impl io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_budget_is_an_eighth_of_the_frame_ceiling() {
        const { assert!(WINDOW_MAX_WIRE_BYTES * 8 == MAX_FRAME_SIZE) };
        const { assert!(WINDOW_MAX_WIRE_BYTES < SNAPSHOT_MAX_WIRE_BYTES) };
        assert!(window_fits(WINDOW_MAX_WIRE_BYTES));
        assert!(!window_fits(WINDOW_MAX_WIRE_BYTES + 1));
        assert!(snapshot_fits(SNAPSHOT_MAX_WIRE_BYTES));
        assert!(!snapshot_fits(SNAPSHOT_MAX_WIRE_BYTES + 1));
    }

    #[test]
    fn wire_bytes_counts_exactly_what_serialization_would_write() {
        let page = TranscriptPage {
            before_cursor: Some("fat.1.0000".to_owned()),
            has_more: true,
            thread_seq: Seq(12),
        };
        let encoded = serde_json::to_vec(&page).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            wire_bytes(&page).unwrap_or_else(|error| panic!("{error}")),
            encoded.len()
        );
    }

    #[test]
    fn an_absent_or_absurd_limit_lands_on_a_servable_one() {
        assert_eq!(clamp_item_body_limit(0), ITEM_BODY_MAX_CHUNK_BYTES);
        assert_eq!(clamp_item_body_limit(u32::MAX), ITEM_BODY_MAX_CHUNK_BYTES);
        assert_eq!(clamp_item_body_limit(4_096), 4_096);
        assert_eq!(clamp_turn_limit(None), WINDOW_DEFAULT_TURNS);
        assert_eq!(clamp_turn_limit(Some(0)), WINDOW_DEFAULT_TURNS);
        assert_eq!(clamp_turn_limit(Some(u32::MAX)), WINDOW_MAX_TURNS);
        assert_eq!(clamp_turn_limit(Some(3)), 3);
    }

    #[test]
    fn every_refusal_carries_a_kind_the_surface_can_branch_on() {
        assert_eq!(window_capability_error().kind, ErrorKind::Unsupported);
        assert!(
            window_capability_error()
                .message
                .contains(crate::AGENT_WINDOW_CAPABILITY)
        );
        let ceiling = snapshot_ceiling_error(ThreadId::new(), SNAPSHOT_MAX_WIRE_BYTES + 1);
        assert_eq!(ceiling.kind, ErrorKind::Unsupported);
        assert!(ceiling.message.contains(crate::AGENT_WINDOW_CAPABILITY));
        assert_eq!(gate_closed_error(GateId::new()).kind, ErrorKind::Conflict);
    }

    #[test]
    fn an_empty_window_is_recognised_as_empty() {
        assert!(TranscriptWindow::default().is_empty());
        assert!(
            TranscriptWindow::default()
                .fits_wire_budget()
                .unwrap_or_else(|error| panic!("{error}"))
        );
    }
}
