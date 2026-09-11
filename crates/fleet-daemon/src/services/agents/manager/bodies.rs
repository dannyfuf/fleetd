//! The two halves of one contract: what a window cuts to fit its budget, and how a client reads
//! back what was cut.
//!
//! `WINDOW_MAX_WIRE_BYTES` is a ceiling the daemon must respect by **narrowing the window**, never
//! by enlarging the frame (`docs/NATIVE-AGENTS.md` §10, spec-C C.10.4). Two levers exist and they
//! are applied in this order, because they cost the reader different amounts:
//!
//! 1. **Elide the largest item bodies.** A 4 MiB build log is one row the reader has collapsed
//!    anyway. Its head is kept, its identity is listed in [`TranscriptWindow::elided`], and the
//!    client reads the rest back with `AgentItemBody` — which is what that request exists for.
//! 2. **Drop the oldest turn.** Only once no body is left worth eliding. Losing a turn is visible
//!    (the page cursor no longer reaches it from this response), so it is the second lever, and
//!    the newest turn is never dropped: a window with nothing in it is not an answer.
//!
//! Nothing here mutates the projection. The window carries clones, so the reducer's own copy of
//! an elided body is still whole — which is exactly what
//! [`AgentSessionManager::item_body`] reads it back from. The store's `items.output` column is a
//! bounded head+tail window by design (§8), so answering a paged read from SQL would hand back
//! *less* than the projection holds; the projection is the single source of truth for an item's
//! text and this read goes there.

use fleet_core::agents::{ItemId, ItemKind, StreamKind, ThreadId};
use fleet_proto::{
    agents::{AgentThreadWindow, clamp_item_body_limit},
    error::ProtoError,
    response::ResponseBody,
};

use super::{AgentSessionManager, not_found, window::ELIDED_HEAD_BYTES};

impl AgentSessionManager {
    /// Handles `AgentItemBody`: one slice of one item's append-only stream.
    ///
    /// Answered from the reducer's projection, which holds the whole body whatever the window
    /// sent — see the module docs for why not from SQL. `limit` is clamped rather than refused,
    /// and an offset past the end is an empty slice with the real `total`, so a client that
    /// raced a still-streaming item learns where the end is instead of getting an error.
    pub async fn item_body(
        &self,
        thread: ThreadId,
        item: ItemId,
        stream: StreamKind,
        offset: u64,
        limit: u32,
    ) -> Result<ResponseBody, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let body = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let item_state = state
                .projection
                .items
                .iter()
                .find(|candidate| candidate.id == item)
                .ok_or_else(|| not_found(format!("item {item} in agent thread {thread}")))?;
            stream_body(&item_state.kind, stream)
                .ok_or_else(|| {
                    not_found(format!(
                        "item {item} in agent thread {thread} has no {stream:?} stream"
                    ))
                })?
                .to_owned()
        };
        let total = body.len() as u64;
        let start = usize::try_from(offset.min(total)).unwrap_or(usize::MAX);
        let end = start.saturating_add(clamp_item_body_limit(limit) as usize);
        let slice = slice_on_boundaries(&body, start, end.min(body.len()));
        Ok(ResponseBody::AgentItemBodyChunk {
            thread,
            item,
            stream,
            offset: start as u64,
            total,
            text: slice.to_owned(),
        })
    }
}

/// One item's named append-only stream, or `None` when this kind has no such channel.
fn stream_body(kind: &ItemKind, stream: StreamKind) -> Option<&str> {
    match (kind, stream) {
        (ItemKind::Tool(call), StreamKind::CommandOutput) => Some(call.output.as_str()),
        (ItemKind::AssistantText { text }, StreamKind::AssistantText)
        | (ItemKind::Plan { text }, StreamKind::PlanText) => Some(text.as_str()),
        (ItemKind::Reasoning { summary, .. }, StreamKind::ReasoningSummary { part }) => {
            summary.get(&part).map(String::as_str)
        }
        (ItemKind::Reasoning { raw, .. }, StreamKind::ReasoningRaw { part }) => {
            raw.get(&part).map(String::as_str)
        }
        _ => None,
    }
}

/// The `[start, end)` slice of `body`, widened inward to character boundaries.
///
/// A client walks by byte offset and a body is UTF-8, so both ends are pulled back onto a
/// boundary. Pulling the *start* back rather than forward is deliberate: it can only repeat a
/// byte the client already has, where pushing it forward would drop one silently.
fn slice_on_boundaries(body: &str, start: usize, end: usize) -> &str {
    let mut from = start.min(body.len());
    while from > 0 && !body.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = end.clamp(from, body.len());
    while to > from && !body.is_char_boundary(to) {
        to -= 1;
    }
    body.get(from..to).unwrap_or("")
}

/// Narrows `response` until it fits the window budget.
///
/// Returns the number of narrowing steps taken, so the caller can log a window that had to be
/// cut without measuring it twice.
///
/// # Errors
///
/// Returns the serializer's own error when a payload cannot be encoded at all.
pub(super) fn narrow_to_budget(
    response: &mut AgentThreadWindow,
) -> Result<usize, serde_json::Error> {
    let mut steps = 0;
    while !response.fits_wire_budget()? {
        if elide_largest_body(response) || drop_oldest_turn(response) {
            steps += 1;
            continue;
        }
        // Neither lever applies: one turn, no body left to cut. The response goes out over
        // budget rather than empty, and the caller logs it — a frame the peer may refuse is
        // still a better answer than a window with no content and no explanation.
        break;
    }
    Ok(steps)
}

/// Elides the single largest elidable body, or answers `false` when none is left.
fn elide_largest_body(response: &mut AgentThreadWindow) -> bool {
    let Some((index, _)) = response
        .window
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| elidable_len(&item.kind).map(|len| (index, len)))
        .max_by_key(|(_, len)| *len)
    else {
        return false;
    };
    let Some(item) = response.window.items.get_mut(index) else {
        return false;
    };
    let id = item.id;
    if !elide(&mut item.kind) {
        return false;
    }
    record_elided(&mut response.window.elided, id);
    true
}

/// Bytes an item's largest elidable body still holds past the head that would be kept.
fn elidable_len(kind: &ItemKind) -> Option<usize> {
    let len = match kind {
        ItemKind::Tool(call) => call.output.len(),
        ItemKind::AssistantText { text } | ItemKind::Plan { text } => text.len(),
        ItemKind::Reasoning { raw, .. } => raw.values().map(String::len).max().unwrap_or(0),
        _ => 0,
    };
    (len > ELIDED_HEAD_BYTES).then_some(len)
}

/// Replaces an item's largest body with its head, answering whether anything was cut.
fn elide(kind: &mut ItemKind) -> bool {
    match kind {
        ItemKind::Tool(call) => truncate_head(&mut call.output),
        ItemKind::AssistantText { text } | ItemKind::Plan { text } => truncate_head(text),
        ItemKind::Reasoning { raw, .. } => raw
            .values_mut()
            .max_by_key(|part| part.len())
            .is_some_and(truncate_head),
        _ => false,
    }
}

/// Cuts `text` to [`ELIDED_HEAD_BYTES`] on a character boundary, answering whether it changed.
fn truncate_head(text: &mut String) -> bool {
    if text.len() <= ELIDED_HEAD_BYTES {
        return false;
    }
    let mut cut = ELIDED_HEAD_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    true
}

/// Notes one item as elided, without letting a second pass list it twice.
fn record_elided(elided: &mut Vec<ItemId>, id: ItemId) {
    if !elided.contains(&id) {
        elided.push(id);
    }
}

/// Drops the oldest turn and its items, answering whether one was dropped.
///
/// The newest turn is never dropped, and the page metadata is left alone: `before_cursor` already
/// points at the oldest turn this *keyset* read, and re-pointing it at the turn the budget cut
/// would hand the client a cursor that skips history it never received.
fn drop_oldest_turn(response: &mut AgentThreadWindow) -> bool {
    if response.window.turns.len() < 2 {
        return false;
    }
    let dropped = response.window.turns.remove(0).id;
    response.window.items.retain(|item| item.turn != dropped);
    response
        .window
        .elided
        .retain(|id| response.window.items.iter().any(|item| item.id == *id));
    true
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{
        AgentKind, Item, ItemStatus, Seq, ThreadId, ThreadProjection, ToolCall, ToolKind, TurnId,
        TurnRecord,
    };
    use fleet_core::ids::WorktreeId;
    use fleet_proto::agents::{TranscriptWindow, WINDOW_MAX_WIRE_BYTES};

    use super::*;

    fn tool_call(output_bytes: usize) -> ToolCall {
        ToolCall {
            kind: ToolKind::Bash,
            name: "Bash".to_owned(),
            input: serde_json::json!({"command": "cargo build"}),
            summary: None,
            result: None,
            output: "x".repeat(output_bytes),
            diff: None,
            exit_code: Some(0),
            duration_ms: None,
            extra: std::collections::BTreeMap::new(),
        }
    }

    fn tool_item(turn: TurnId, output_bytes: usize) -> Item {
        Item {
            id: ItemId::new(),
            turn,
            parent: None,
            kind: ItemKind::Tool(Box::new(tool_call(output_bytes))),
            status: ItemStatus::Completed,
            children: Vec::new(),
            started: chrono::Utc::now(),
            ended: None,
        }
    }

    fn turn(id: TurnId) -> TurnRecord {
        TurnRecord {
            id,
            user_item: None,
            started_at: chrono::Utc::now(),
            ended: None,
            blocked_ms: 0,
        }
    }

    fn window(items: Vec<Item>, turns: Vec<TurnRecord>) -> AgentThreadWindow {
        let worktree =
            WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}"));
        let projection = ThreadProjection::new(ThreadId::new(), worktree, AgentKind::Claude);
        AgentThreadWindow {
            summary: projection.summary(Seq::default()),
            session: Default::default(),
            window: TranscriptWindow {
                turns,
                items,
                ..TranscriptWindow::default()
            },
            page: None,
            head_seq: Seq::default(),
            projected_seq: Seq::default(),
            events_after: Vec::new(),
            synchronized: false,
        }
    }

    /// A window inside the budget is handed back byte for byte: narrowing is never speculative.
    #[test]
    fn a_window_inside_the_budget_is_untouched() -> Result<(), serde_json::Error> {
        let turn_id = TurnId::new();
        let mut response = window(vec![tool_item(turn_id, 64)], vec![turn(turn_id)]);
        let before = response.clone();

        assert_eq!(narrow_to_budget(&mut response)?, 0);

        assert_eq!(response, before);
        assert!(response.window.elided.is_empty());
        Ok(())
    }

    /// The oversized body is cut to its head and *named*, so the client knows to read the rest
    /// rather than rendering a silently truncated log as complete.
    #[test]
    fn an_oversized_body_is_elided_and_named() -> Result<(), serde_json::Error> {
        let turn_id = TurnId::new();
        let mut response = window(
            vec![tool_item(turn_id, WINDOW_MAX_WIRE_BYTES + 4_096)],
            vec![turn(turn_id)],
        );
        let item = response.window.items[0].id;

        assert!(narrow_to_budget(&mut response)? >= 1);

        assert!(response.fits_wire_budget()?);
        assert_eq!(response.window.elided, vec![item]);
        let ItemKind::Tool(call) = &response.window.items[0].kind else {
            panic!("the item is a tool call");
        };
        assert_eq!(call.output.len(), ELIDED_HEAD_BYTES);
        // The turn survives: eliding a body is the cheap lever and it was enough.
        assert_eq!(response.window.turns.len(), 1);
        Ok(())
    }

    /// Many bodies, all already at the head: the second lever drops whole turns rather than
    /// sending a frame the peer cannot decode, and it stops at the newest one.
    #[test]
    fn turns_are_dropped_only_once_no_body_is_left_to_elide() -> Result<(), serde_json::Error> {
        let turns = (0..8).map(|_| TurnId::new()).collect::<Vec<_>>();
        let items = turns
            .iter()
            .flat_map(|turn| (0..48).map(move |_| tool_item(*turn, ELIDED_HEAD_BYTES)))
            .collect::<Vec<_>>();
        let mut response = window(items, turns.iter().copied().map(turn).collect());

        assert!(narrow_to_budget(&mut response)? >= 1);

        assert!(response.fits_wire_budget()?);
        assert!(!response.window.turns.is_empty(), "the newest turn is kept");
        assert!(response.window.turns.len() < 8, "older turns were dropped");
        // The newest turn is the one kept, never an arbitrary survivor.
        assert_eq!(
            response.window.turns.last().map(|turn| turn.id),
            turns.last().copied()
        );
        Ok(())
    }

    /// A multi-byte character is never split: the head is cut on a boundary, so the client
    /// receives valid UTF-8 and the paged remainder starts where the head ended.
    #[test]
    fn the_head_is_cut_on_a_character_boundary() {
        let mut text = "é".repeat(ELIDED_HEAD_BYTES);
        assert!(truncate_head(&mut text));
        assert!(text.len() <= ELIDED_HEAD_BYTES);
        assert_eq!(text.len() % 2, 0, "'é' is two bytes; neither half is kept");
    }

    /// The stream the client is told to continue is the stream the window cut. The client asks
    /// through `fleet_proto::agents::elided_stream`, so this asserts the two agree on every kind
    /// this pass can elide rather than restating one of them.
    #[test]
    fn the_named_stream_is_the_one_that_was_elided() -> Result<(), serde_json::Error> {
        let mut raw = std::collections::BTreeMap::new();
        raw.insert(0_u32, "short".to_owned());
        raw.insert(1_u32, "y".repeat(ELIDED_HEAD_BYTES * 2));
        let kinds = [
            ItemKind::Tool(Box::new(tool_call(ELIDED_HEAD_BYTES * 2))),
            ItemKind::AssistantText {
                text: "z".repeat(ELIDED_HEAD_BYTES * 2),
            },
            ItemKind::Plan {
                text: "p".repeat(ELIDED_HEAD_BYTES * 2),
            },
            ItemKind::Reasoning {
                summary: std::collections::BTreeMap::new(),
                raw,
            },
        ];
        for kind in kinds {
            let named = fleet_proto::agents::elided_stream(&kind)
                .unwrap_or_else(|| panic!("an elidable kind names its stream: {kind:?}"));
            let mut cut = kind.clone();
            assert!(elide(&mut cut), "{kind:?} is elidable");
            // The named stream is the one whose bytes changed.
            assert_ne!(
                body_of(&kind, named),
                body_of(&cut, named),
                "{named:?} is the stream that was cut"
            );
        }
        // A user message carries no body worth paging, so nothing names a stream for it.
        assert_eq!(
            fleet_proto::agents::elided_stream(&ItemKind::UserMessage {
                text: String::new(),
                attachments: Vec::new(),
                steered: false,
            }),
            None
        );
        Ok(())
    }

    fn body_of(kind: &ItemKind, stream: StreamKind) -> String {
        stream_body(kind, stream).unwrap_or_default().to_owned()
    }
}
