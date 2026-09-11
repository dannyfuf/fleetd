//! `AgentThreadOpen`, resolved: which shape to answer, how much transcript to put in it, and
//! where the next older page starts.
//!
//! Two answers exist and the request chooses between them, never the daemon's mood
//! ([`fleet_proto::request::RequestBody::wants_window`]): a peer that sent no window field gets
//! the version-6 [`ResponseBody::AgentThreadSnapshot`](fleet_proto::response::ResponseBody::AgentThreadSnapshot)
//! with its meaning unchanged, and a peer that sent one gets
//! [`AgentThreadWindow`]. That is what makes the window additive — a client can never be handed a
//! payload shape it has no arm for (`rust-ipc-protocol` Rule 7).
//!
//! **The admission ladder** (`docs/NATIVE-AGENTS.md` §9.3, spec-C C.8.6) is the other decision
//! here. A resume asks for `(after_seq, head]`, and on a thread that has been running for ten
//! minutes that range can be tens of thousands of events and megabytes of tool output. Past
//! [`REPLAY_MAX_EVENTS`] or [`REPLAY_MAX_BYTES`] the daemon stops replaying and sends a **windowed
//! snapshot** instead: bounded, complete for what the user is looking at, and one frame. The
//! measurement costs one `SUM(bytes)` over an indexed range, never a payload read.
//!
//! **What the window is built from.** The turn set and the page cursor come from `turns.start_seq`
//! — one index-only keyset read — and the content comes from the reducer's own projection, which
//! is already in memory for a hydrated thread. Nothing re-derives a transcript row from SQL, so
//! the window and the live stream cannot disagree about what an item says.

use fleet_core::agents::{
    Item, ItemId, Seq, SeqEvent, ThreadId, ThreadProjection, TurnId, TurnRecord,
};
use fleet_proto::{
    agents::{
        AgentSessionView, AgentThreadWindow, TranscriptPage, TranscriptWindow, clamp_turn_limit,
    },
    request::RequestBody,
};

use super::super::store::{SessionRuntime, TurnKeyset, page_token};

/// Events one resume may replay before the ladder sends a window instead.
pub(crate) const REPLAY_MAX_EVENTS: u64 = 1_000;

/// Bytes of one item body a window keeps when it has to elide the rest.
///
/// Enough to render the head of a build log or a long answer — what the reader looks at first —
/// while the remainder is read back with
/// [`RequestBody::AgentItemBody`](fleet_proto::request::RequestBody::AgentItemBody). Well under
/// [`ITEM_BODY_MAX_CHUNK_BYTES`](fleet_proto::agents::ITEM_BODY_MAX_CHUNK_BYTES), so the first
/// page a client asks for always advances past the head it already has.
pub(crate) const ELIDED_HEAD_BYTES: usize = 8 * 1024;

/// Payload bytes one resume may replay before the ladder sends a window instead.
pub(crate) const REPLAY_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// One `AgentThreadOpen`, with every wire spelling of its fields already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenRequest {
    /// Thread to open.
    pub(crate) thread: ThreadId,
    /// Resume cursor, from whichever field carried it.
    pub(crate) resume: Option<Seq>,
    /// Turns the window may carry, clamped to what the protocol allows.
    pub(crate) turn_limit: u32,
    /// Opaque page cursor from a previous window, still encoded.
    pub(crate) before_cursor: Option<String>,
    /// Whether the client asked for an explicit synchronization marker.
    pub(crate) sync_marker: bool,
    /// Whether the client asked for a window at all.
    pub(crate) windowed: bool,
}

impl OpenRequest {
    /// Resolves one open request, or `None` for any other request body.
    pub(crate) fn from_body(body: &RequestBody) -> Option<Self> {
        let RequestBody::AgentThreadOpen {
            thread,
            turn_limit,
            before_cursor,
            request_sync_marker,
            ..
        } = body
        else {
            return None;
        };
        Some(Self {
            thread: *thread,
            resume: body.resume_seq(),
            turn_limit: clamp_turn_limit(*turn_limit),
            before_cursor: before_cursor.clone(),
            sync_marker: *request_sync_marker,
            windowed: body.wants_window(),
        })
    }
}

/// Whether a resume of `events`/`bytes` may be replayed rather than windowed.
///
/// Both halves are needed: a thousand one-line deltas is a cheap replay and three tool outputs
/// can be eight megabytes, so neither a count nor a size alone describes the frame that would be
/// produced.
#[must_use]
pub(crate) const fn admits_replay(events: u64, bytes: u64) -> bool {
    events <= REPLAY_MAX_EVENTS && bytes <= REPLAY_MAX_BYTES
}

/// Builds the windowed answer for one open.
///
/// `keyset` decides the turns; `events_after` is the replay tail the ladder admitted, empty when
/// it did not. `synchronized` says whether the owner has confirmed this content — always `false`
/// on a mirrored thread answered locally, because the mirror never fabricates a synchronization.
pub(crate) fn window_response(
    projection: &ThreadProjection,
    session: Option<&SessionRuntime>,
    request: &OpenRequest,
    keyset: &TurnKeyset,
    events_after: Vec<SeqEvent>,
    synchronized: bool,
) -> AgentThreadWindow {
    let kept = keyset
        .turns
        .iter()
        .map(|(turn, _)| *turn)
        .collect::<Vec<_>>();
    let window = transcript(projection, &kept, keyset.oldest_seq);
    let paged = keyset.has_more || request.before_cursor.is_some();
    AgentThreadWindow {
        summary: projection.summary(Seq::default()),
        session: session_view(projection, session),
        window,
        page: paged.then(|| TranscriptPage {
            before_cursor: keyset
                .has_more
                .then(|| {
                    keyset
                        .oldest_seq
                        .map(|seq| page_token(projection.thread, seq))
                })
                .flatten(),
            has_more: keyset.has_more,
            thread_seq: projection.last_seq,
        }),
        head_seq: projection.last_seq,
        projected_seq: projection.last_seq,
        events_after,
        synchronized,
    }
}

/// The transcript slice for one turn set.
///
/// Every open gate is carried whatever its turn, because a decision the user has to answer is
/// never paged out: a gate four turns behind the window would otherwise be unanswerable until
/// the user scrolled back to it.
fn transcript(
    projection: &ThreadProjection,
    kept: &[TurnId],
    oldest_seq: Option<Seq>,
) -> TranscriptWindow {
    let turns = projection
        .turns
        .iter()
        .filter(|turn| kept.contains(&turn.id))
        .cloned()
        .collect::<Vec<TurnRecord>>();
    let items = projection
        .items
        .iter()
        .filter(|item| kept.contains(&item.turn))
        .cloned()
        .collect::<Vec<Item>>();
    let floor = oldest_seq.unwrap_or_default();
    TranscriptWindow {
        turns,
        items,
        checkpoints: projection
            .checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.seq >= floor)
            .cloned()
            .collect(),
        notices: projection
            .notices
            .iter()
            .filter(|notice| notice.seq >= floor)
            .cloned()
            .collect(),
        gates: projection.gates.clone(),
        elided: Vec::<ItemId>::new(),
    }
}

/// The session runtime a windowed open reports.
///
/// The model and mode come from the reducer, which owns them; the harness-native tool, command
/// and skill names come from the `sessions` row, which is the only place they are kept. A window
/// that dropped the second half would blank every gated control on a cold open.
fn session_view(
    projection: &ThreadProjection,
    session: Option<&SessionRuntime>,
) -> AgentSessionView {
    AgentSessionView {
        model: projection.model.clone(),
        mode: projection.mode,
        tools: session
            .map(|session| session.tools.clone())
            .unwrap_or_default(),
        commands: session
            .map(|session| session.commands.clone())
            .unwrap_or_default(),
        skills: session
            .map(|session| session.skills.clone())
            .unwrap_or_default(),
        // Negotiated harness capabilities are owned by the harness layer and are not projected
        // yet; the surface gates on `None` by disabling, which is the safe direction.
        capabilities: None,
        usage: Some(projection.cumulative_usage.clone()),
        cost_usd: projection.cumulative_cost_usd,
        context_pct: Some(projection.context_pct),
        retrying: projection.retrying.clone(),
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{AgentKind, ThreadId};
    use fleet_core::ids::WorktreeId;

    use super::*;

    fn open(body: RequestBody) -> OpenRequest {
        OpenRequest::from_body(&body).unwrap_or_else(|| panic!("an open request"))
    }

    #[test]
    fn a_version_six_open_never_asks_for_a_window() {
        let request = open(RequestBody::AgentThreadOpen {
            thread: ThreadId::new(),
            from_seq: Some(Seq(7)),
            after_seq: None,
            turn_limit: None,
            before_cursor: None,
            request_sync_marker: false,
        });

        assert!(!request.windowed);
        assert_eq!(request.resume, Some(Seq(7)));
        // Absent means the protocol's default, not "everything".
        assert_eq!(
            request.turn_limit,
            fleet_proto::agents::WINDOW_DEFAULT_TURNS
        );
    }

    #[test]
    fn any_window_field_asks_for_a_window_and_the_newer_cursor_wins() {
        let request = open(RequestBody::AgentThreadOpen {
            thread: ThreadId::new(),
            from_seq: Some(Seq(7)),
            after_seq: Some(Seq(12)),
            turn_limit: Some(0),
            before_cursor: None,
            request_sync_marker: true,
        });

        assert!(request.windowed);
        assert!(request.sync_marker);
        assert_eq!(request.resume, Some(Seq(12)));
        assert_eq!(
            request.turn_limit,
            fleet_proto::agents::WINDOW_DEFAULT_TURNS
        );
    }

    #[test]
    fn the_ladder_trips_on_either_half() {
        assert!(admits_replay(REPLAY_MAX_EVENTS, REPLAY_MAX_BYTES));
        assert!(!admits_replay(REPLAY_MAX_EVENTS + 1, 0));
        assert!(!admits_replay(0, REPLAY_MAX_BYTES + 1));
        // Three tool outputs can be eight megabytes, so the count alone would admit a frame no
        // peer should be sent.
        assert!(!admits_replay(3, REPLAY_MAX_BYTES + 1));
    }

    #[test]
    fn a_window_carries_every_open_gate_and_only_its_own_turns() {
        let thread = ThreadId::new();
        let worktree =
            WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}"));
        let mut projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
        let kept = TurnId::new();
        let dropped = TurnId::new();
        projection.turns = vec![turn(dropped), turn(kept)];
        projection.last_seq = Seq(40);
        let keyset = TurnKeyset {
            turns: vec![(kept, Seq(30))],
            oldest_seq: Some(Seq(30)),
            has_more: true,
        };

        let response = window_response(
            &projection,
            None,
            &open(RequestBody::AgentThreadOpen {
                thread,
                from_seq: None,
                after_seq: None,
                turn_limit: Some(1),
                before_cursor: None,
                request_sync_marker: false,
            }),
            &keyset,
            Vec::new(),
            false,
        );

        assert_eq!(
            response
                .window
                .turns
                .iter()
                .map(|turn| turn.id)
                .collect::<Vec<_>>(),
            vec![kept]
        );
        let page = response.page.unwrap_or_else(|| panic!("a page"));
        assert!(page.has_more);
        assert_eq!(page.thread_seq, Seq(40));
        assert_eq!(page.before_cursor, Some(page_token(thread, Seq(30))));
        // A mirrored answer is never presented as live.
        assert!(!response.synchronized);
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
}
