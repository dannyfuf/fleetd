//! The windowed-open request and how a bounded window becomes a client projection.
//!
//! A window is projection *pieces* — turns, items, checkpoints, notices, every open gate — and
//! the mirror holds a [`ThreadProjection`], so the two are joined here rather than in the mirror
//! or in a view. One place also means one answer to the question a window makes possible and a
//! whole-transcript snapshot never did: **which sequence has this content actually applied?**

use fleet_core::agents::{Seq, ThreadProjection};
use fleet_proto::{agents::AgentThreadWindow, request::RequestBody};

/// What a client asks for when it opens a thread on a windowing daemon.
///
/// Every field is optional and defaults to the daemon's own choice, so
/// `AgentWindowRequest::default()` is the plain "open this thread" that a first paint wants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentWindowRequest {
    /// Resume from this sequence instead of reading a window.
    pub after_seq: Option<Seq>,
    /// Turns to include; `None` takes the daemon's default.
    pub turn_limit: Option<u32>,
    /// Opaque cursor from a previous window's `page.before_cursor`, to read older history.
    pub before_cursor: Option<String>,
    /// Ask for an explicit synchronization marker between catch-up and live.
    pub request_sync_marker: bool,
}

impl AgentWindowRequest {
    /// A resume-only open: no window, just everything after `after_seq`.
    ///
    /// This is what a reconnect sends. It carries zero transcript bytes when nothing happened
    /// while the client was away, which is the whole reason a reconnect is cheap.
    #[must_use]
    pub fn resume(after_seq: Seq) -> Self {
        Self {
            after_seq: Some(after_seq),
            request_sync_marker: true,
            ..Self::default()
        }
    }

    /// A first-paint open of the newest `turns` turns.
    #[must_use]
    pub fn newest(turns: u32) -> Self {
        Self {
            turn_limit: Some(turns),
            request_sync_marker: true,
            ..Self::default()
        }
    }

    /// The older page that follows a window, from that window's cursor.
    #[must_use]
    pub fn older_than(cursor: impl Into<String>, turns: u32) -> Self {
        Self {
            turn_limit: Some(turns),
            before_cursor: Some(cursor.into()),
            ..Self::default()
        }
    }

    /// Builds the wire request for `thread`.
    #[must_use]
    pub fn into_body(self, thread: fleet_core::agents::ThreadId) -> RequestBody {
        RequestBody::AgentThreadOpen {
            thread,
            // Never both: `from_seq` is the version-6 field, and a daemon that understands the
            // window reads `after_seq`. Sending the pair would make the precedence a wire
            // question instead of a local one.
            from_seq: None,
            after_seq: self.after_seq,
            turn_limit: self.turn_limit,
            before_cursor: self.before_cursor,
            request_sync_marker: self.request_sync_marker,
        }
    }
}

/// The sequence a window's content has actually applied through.
///
/// `head_seq` is the log head and `projected_seq` is how far the projection got. They are equal
/// on every healthy thread; when the daemon is mid-rebuild the window is a *prefix*, and taking
/// the head as the resume point would skip every event in between — never sent, never replayed,
/// invisible until a user says "it skipped a message". Taking the smaller one costs a replay and
/// loses nothing, which is the right way round for that trade.
///
/// A peer that omits `projected_seq` therefore reads as "projected nothing" and gets a full
/// replay. That is the safe degradation, and it is why the field is not silently treated as
/// equal to the head.
#[must_use]
pub fn applied_seq(window: &AgentThreadWindow) -> Seq {
    window.projected_seq.min(window.head_seq)
}

/// Materializes a bounded window into a client-side projection.
///
/// The projection's `last_seq` is [`applied_seq`], not the summary's own cursor: the mirror
/// decides continuity from it, and it must describe the content in hand rather than the newest
/// row the daemon has.
///
/// The result is only ever fed to [`ThreadProjection::apply`], which rebuilds the item and turn
/// indexes it owns before it reads them. `ThreadProjection::accepts` on its own would consult
/// indexes this function cannot fill, so a caller that wants to pre-validate must apply.
#[must_use]
pub fn projection_from_window(window: &AgentThreadWindow) -> ThreadProjection {
    let summary = &window.summary;
    let mut projection =
        ThreadProjection::new(summary.thread, summary.worktree.clone(), summary.provider);
    projection.title = summary.title.clone();
    projection.session = summary.session.clone();
    projection.turn = summary.turn.clone();
    projection.last_activity = summary.last_activity;
    projection.last_completed_seq = summary.last_completed_seq;
    projection.last_nonterminal_seq = summary.last_nonterminal_seq;
    projection.exit_code = summary.exit_code;

    projection.model = window.session.model.clone();
    projection.mode = window.session.mode;
    projection.cumulative_usage = window.session.usage.clone().unwrap_or_default();
    projection.cumulative_cost_usd = window.session.cost_usd;
    projection.context_pct = window.session.context_pct.unwrap_or_default();
    projection.retrying = window.session.retrying.clone();

    projection.turns = window.window.turns.clone();
    projection.items = window.window.items.clone();
    projection.gates = window.window.gates.clone();
    projection.checkpoints = window.window.checkpoints.clone();
    projection.notices = window.window.notices.clone();
    projection.last_seq = applied_seq(window);
    projection
}
