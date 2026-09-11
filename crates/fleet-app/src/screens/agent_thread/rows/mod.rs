//! The flat row projection of a thread (`docs/NATIVE-AGENTS.md` §5, `spec-B` §B1).
//!
//! **The transcript is a flat list of rows, never a tree of turn widgets.** A turn is an
//! emergent run of rows, and a tool's children are separate rows emitted after it rather than a
//! `Vec<Row>` field, so the virtualizer measures every visible thing exactly once. Nested
//! containers make variable-height virtualization and scroll anchoring unsolvable.
//!
//! Building rows is a pure function of the daemon projection plus the view-local expansion and
//! send state, so the whole §B1 vocabulary — folds, groups, the live row, footers, gate records
//! and the empty state — is testable without a window. `render` composes what this produced and
//! nothing else (`docs/APP-CONTRACTS.md`, "Render prepares nothing").

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use fleet_core::agents::{
    CheckpointKind, CheckpointRecord, GateId, Item, ItemId, ItemStatus, NoticeRecord, SessionState,
    ThreadProjection, TurnId, TurnRecord, TurnState,
};
use fleet_ui_kit::{
    EmptyRow, ErrorRow, NoticeRow, TranscriptRow, TranscriptRowId, TranscriptRowKind, WorkingPhase,
    WorkingRow, format_compacted, format_resumed, format_retrying,
};
use gpui::SharedString;

pub(crate) mod fold;
pub(crate) mod group;
pub(crate) mod item;
pub(crate) mod live;
pub(crate) mod turn;

pub(crate) use item::item_text;

/// What a row's `⏎` — or a click on its header line — expands or collapses.
///
/// A row key is a [`SharedString`] by the time it comes back from the kit, so the build records
/// what each one addresses instead of the view re-deriving it from the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowTarget {
    /// A tool row, a reasoning block, a user bubble, a plan, a subagent, or a group keyed by its
    /// first member.
    Item(ItemId),
    /// One turn's `worked …` fold.
    Turn(TurnId),
    /// The settled record of a resolved gate.
    Gate(GateId),
}

/// One message the user sent that the daemon has not reflected back yet (`spec-B` §B5.4).
///
/// The bubble is optimistic: it draws at `refreshing_opacity` from the frame the key was pressed
/// and is reconciled away when the projection grows the matching user item. The id is minted by
/// this client so the row keeps one identity for its whole life and never remounts when the
/// server's copy arrives — see the module note in `super` about the wire field that would let
/// the daemon adopt it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingSend {
    /// Client-minted identity, which is the row's id for its whole life.
    pub(crate) id: ItemId,
    /// Exactly what the user typed, with Fleet's own send-time additions already stripped.
    pub(crate) text: String,
    /// Whether the message joined a turn that was already running.
    pub(crate) steered: bool,
    /// Whether the dispatch failed, which draws the danger hairline and `[r] retry`.
    pub(crate) failed: bool,
    /// How many user items carrying this exact text the projection held at dispatch time.
    ///
    /// Reconciliation compares against this rather than "any item with this text", so sending
    /// the same message twice does not clear both bubbles on the first echo.
    pub(crate) echoes: usize,
}

/// One decision this window watched resolve, kept so the transcript keeps its history.
///
/// The daemon's projection holds only the **open** gates: a resolved one is gone from it, and
/// §B0.10 still wants a one-line record at the position it was asked, because that is what makes
/// docking the live drawer safe. So the view records what it saw close, and what it answered
/// with when it was this window that answered.
///
/// The record is therefore local to this window: a client that reconnects has the open gates and
/// none of the closed ones. That is a visible gap and it is named in the integration notes
/// rather than papered over with an invented outcome.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedGate {
    /// The gate as it stood while it was open.
    pub(crate) gate: fleet_core::agents::OpenGate,
    /// What it turned out to be.
    pub(crate) outcome: fleet_ui_kit::GateOutcome,
    /// The answer this window sent, when it was this window that answered.
    pub(crate) answer: Option<fleet_core::agents::GateAnswer>,
}

/// Everything outside the daemon projection that one row build depends on.
pub(crate) struct RowInputs<'a> {
    /// The projection the rows describe.
    pub(crate) projection: &'a ThreadProjection,
    /// Items, groups (keyed by their first member) and plans the user expanded.
    pub(crate) expanded: &'a HashSet<ItemId>,
    /// Turns whose `worked …` fold the user opened.
    pub(crate) unfolded: &'a HashSet<TurnId>,
    /// Settled gate records the user expanded.
    pub(crate) expanded_gates: &'a HashSet<GateId>,
    /// The decisions this window watched resolve, oldest first.
    pub(crate) resolved: &'a [ResolvedGate],
    /// Optimistic user bubbles, oldest first.
    pub(crate) pending: &'a [PendingSend],
    /// The checkpoint each turn can be reverted to. `[u] revert turn` is drawn only where one
    /// exists, because a drawn affordance that does nothing is worse than an absent one.
    pub(crate) checkpoints: &'a HashMap<TurnId, fleet_proto::agents::CheckpointId>,
    /// When the live row's clock started. The row carries the instant and ticks itself; no
    /// elapsed figure ever enters the model.
    pub(crate) started_at: Option<Instant>,
    /// The parked detail line — `weekly limit resets in ~3h` — when a usage window parked the
    /// turn. A parked turn stays `working` with an explicit flag; no timeout marks it failed.
    pub(crate) parked: Option<SharedString>,
    /// The empty thread's invitation, which names the harness and the worktree.
    pub(crate) empty: SharedString,
}

impl RowInputs<'_> {
    /// §3.3's `Working`: a running turn, a live session, a background task, or a backoff.
    ///
    /// This is the one predicate the live row, the fold exemptions and the `AgentWorking` key
    /// context are all derived from, so none of the three can disagree with the others.
    pub(crate) fn is_working(&self) -> bool {
        let projection = self.projection;
        matches!(projection.turn, TurnState::Running(_))
            || projection.session == SessionState::Running
            || projection.session == SessionState::Starting
            || !projection.background_tasks.is_empty()
            || projection.retrying.is_some()
            || !self.pending.is_empty()
    }

    /// The turn the harness is actually working on, which is **not** always the latest one.
    ///
    /// Right after a send the previous turn is still the newest recorded one until the harness
    /// mints the new id; folding on "latest" flickers through that window, so the session's
    /// running turn wins whenever it lags (`spec-B` §B1.2, `deriveUnsettledTurnId`).
    pub(crate) fn unsettled_turn(&self) -> Option<TurnId> {
        if let TurnState::Running(turn) = self.projection.turn {
            return Some(turn);
        }
        self.projection
            .turns
            .iter()
            .rev()
            .find(|turn| turn.ended.is_none())
            .map(|turn| turn.id)
    }

    /// What the live row is waiting on, which decides its copy alone.
    pub(crate) fn phase(&self) -> WorkingPhase {
        if self.parked.is_some() {
            return WorkingPhase::Parked;
        }
        match self.projection.session {
            SessionState::Starting => WorkingPhase::Starting,
            SessionState::Waiting(_) => WorkingPhase::Parked,
            _ => WorkingPhase::Working,
        }
    }

    /// Whether an item's body is exposed.
    pub(crate) fn is_expanded(&self, item: ItemId) -> bool {
        self.expanded.contains(&item)
    }
}

/// The row model, plus the two indexes the view needs back from the build.
#[derive(Debug, Default)]
pub(crate) struct BuiltRows {
    /// Rows in display order.
    pub(crate) rows: Vec<TranscriptRow>,
    /// What each row key toggles.
    pub(crate) targets: HashMap<SharedString, RowTarget>,
    /// Where each streaming item's row sits, so a `ContentDelta` rewrites exactly one row.
    pub(crate) streaming: HashMap<ItemId, usize>,
}

impl BuiltRows {
    /// Appends one row and records what it toggles.
    pub(crate) fn push(&mut self, row: TranscriptRow, target: Option<RowTarget>) {
        if let Some(target) = target {
            self.targets.insert(row.id.key(), target);
        }
        self.rows.push(row);
    }

    /// Records that `item`'s text streams into the row just pushed.
    pub(crate) fn mark_streaming(&mut self, item: ItemId) {
        let index = self.rows.len().saturating_sub(1);
        self.streaming.insert(item, index);
    }
}

/// Builds the complete ordered row model for one thread.
///
/// One forward pass with every map precomputed: the checkpoint and notice records are bucketed
/// by the turn they follow, the turn's own items are collected once, and the per-turn emission
/// in [`turn`] does the fold, group and live derivation. Nothing here scans the transcript per
/// row.
pub(crate) fn build_rows(inputs: &RowInputs<'_>) -> BuiltRows {
    let projection = inputs.projection;
    let mut built = BuiltRows::default();
    let items: HashMap<ItemId, &Item> = projection
        .items
        .iter()
        .map(|item| (item.id, item))
        .collect();
    let unsettled = inputs.unsettled_turn();

    // §5 gives a compaction, a resume and a user-facing notice rows of their own, at the
    // boundary they happened on rather than a line only the log has.
    boundary_rows(inputs, &mut built, None);
    gate_rows(inputs, &mut built, None);
    for record in &projection.turns {
        turn::emit(inputs, &items, record, unsettled, &mut built);
        boundary_rows(inputs, &mut built, Some(record.id));
        gate_rows(inputs, &mut built, Some(record.id));
    }
    // Items the reducer accepted before their turn was recorded still have to be visible; they
    // are appended in projection order rather than silently dropped.
    let recorded: HashSet<TurnId> = projection.turns.iter().map(|turn| turn.id).collect();
    let orphans: Vec<&Item> = projection
        .items
        .iter()
        .filter(|item| item.parent.is_none() && !recorded.contains(&item.turn))
        .collect();
    if !orphans.is_empty() {
        turn::emit_orphans(inputs, &items, &orphans, &mut built);
    }

    // The rows that close the transcript come before the emptiness test, because a dead session
    // and a backoff are exactly the two cases where a thread with no items is still not empty.
    trailing_rows(inputs, &mut built);
    if built.rows.is_empty() {
        built.push(
            TranscriptRow::new(
                TranscriptRowId::Ordinal(ordinal(Ordinal::Empty, 0)),
                TranscriptRowKind::Empty(EmptyRow {
                    message: inputs.empty.clone(),
                }),
            ),
            None,
        );
    }
    built
}

/// The settled gate records of one turn, at the position the decision was asked.
fn gate_rows(inputs: &RowInputs<'_>, built: &mut BuiltRows, after: Option<TurnId>) {
    for resolved in inputs
        .resolved
        .iter()
        .filter(|resolved| resolved.gate.turn == after)
    {
        let (row, target) = item::gate_row(
            &resolved.gate,
            resolved.outcome,
            resolved.answer.as_ref(),
            inputs.expanded_gates.contains(&resolved.gate.id),
        );
        built.push(row, target);
    }
}

/// The checkpoint and notice rows recorded after one turn, in observation order.
fn boundary_rows(inputs: &RowInputs<'_>, built: &mut BuiltRows, after: Option<TurnId>) {
    for (index, checkpoint) in checkpoints_after(&inputs.projection.checkpoints, after) {
        built.push(
            TranscriptRow::new(
                TranscriptRowId::Ordinal(ordinal(Ordinal::Checkpoint, index)),
                TranscriptRowKind::Checkpoint(fleet_ui_kit::CheckpointRow {
                    label: match &checkpoint.kind {
                        CheckpointKind::CompactBoundary { before, after } => {
                            format_compacted(*before, *after)
                        }
                        CheckpointKind::Resumed { age_ms } => format_resumed(*age_ms),
                    },
                }),
            ),
            None,
        );
    }
    for (index, notice) in notices_after(&inputs.projection.notices, after) {
        built.push(
            TranscriptRow::new(
                TranscriptRowId::Ordinal(ordinal(Ordinal::Notice, index)),
                TranscriptRowKind::Notice(NoticeRow {
                    text: SharedString::new(notice.text.as_str()),
                }),
            ),
            None,
        );
    }
}

/// The row kinds that draw no addressable thing and are therefore keyed by position.
///
/// All four share one ordinal space, so the discriminant is folded into the number: without it
/// the third notice and the third checkpoint would be one row key and the list would recycle one
/// row's measured height onto the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ordinal {
    /// A compaction or resume separator.
    Checkpoint,
    /// A harness notice.
    Notice,
    /// A row that closes the transcript: a backoff line, a dead session.
    Trailing,
    /// The empty state.
    Empty,
}

/// The row key of one positional row.
pub(crate) const fn ordinal(kind: Ordinal, index: usize) -> usize {
    index * 4 + kind as usize
}

fn checkpoints_after(
    checkpoints: &[CheckpointRecord],
    after: Option<TurnId>,
) -> impl Iterator<Item = (usize, &CheckpointRecord)> {
    checkpoints
        .iter()
        .enumerate()
        .filter(move |(_, checkpoint)| checkpoint.after_turn == after)
}

fn notices_after(
    notices: &[NoticeRecord],
    after: Option<TurnId>,
) -> impl Iterator<Item = (usize, &NoticeRecord)> {
    notices
        .iter()
        .enumerate()
        .filter(move |(_, notice)| notice.after_turn == after)
}

/// The rows that close the transcript: a backoff line, a dead session, and the live row.
fn trailing_rows(inputs: &RowInputs<'_>, built: &mut BuiltRows) {
    let projection = inputs.projection;
    // §B1.2: `Retrying` is not an error. A backoff still counting down is progress, so it reads
    // as a notice line that replaces itself in place rather than as a danger card.
    if let Some(retry) = &projection.retrying {
        built.push(
            TranscriptRow::new(
                TranscriptRowId::Ordinal(ordinal(Ordinal::Trailing, 0)),
                TranscriptRowKind::Notice(NoticeRow {
                    text: SharedString::from(format!(
                        "{} \u{b7} attempt {}",
                        format_retrying(&retry.reason, retry.retry_in_ms),
                        retry.attempt
                    )),
                }),
            ),
            None,
        );
    }
    if let Some(code) = projection.exit_code.filter(|code| *code != 0) {
        built.push(
            TranscriptRow::new(
                TranscriptRowId::Ordinal(ordinal(Ordinal::Trailing, 1)),
                TranscriptRowKind::Error(ErrorRow {
                    message: SharedString::from(format!(
                        "{} exited {code}",
                        projection.provider.executable()
                    )),
                    retryable: false,
                }),
            ),
            None,
        );
    }
    // The optimistic bubbles sit after everything the daemon has confirmed: they are the newest
    // thing in the thread by construction.
    for pending in inputs.pending {
        built.push(item::pending_row(pending), None);
    }
    if let Some(row) = live_row(inputs, built) {
        built.push(row, None);
    }
}

/// The single live row, when one is called for and no work row already is it.
///
/// A `WorkLive` row emitted by the turn pass already owns `RowId::LiveActivity`; the working row
/// is the fallback for the window in which nothing has started yet, which is exactly why the two
/// share the id and therefore the measured height.
fn live_row(inputs: &RowInputs<'_>, built: &BuiltRows) -> Option<TranscriptRow> {
    if !inputs.is_working() {
        return None;
    }
    let claimed = built
        .rows
        .iter()
        .any(|row| row.id == TranscriptRowId::LiveActivity);
    if claimed {
        return None;
    }
    Some(TranscriptRow::new(
        TranscriptRowId::LiveActivity,
        TranscriptRowKind::Working(WorkingRow {
            phase: inputs.phase(),
            started_at: inputs.started_at,
            harness: SharedString::new_static(inputs.projection.provider.executable()),
            detail: inputs.parked.clone(),
        }),
    ))
}

/// Whether an item occupies a 30 px work row rather than a prose block.
pub(crate) fn is_work(item: &Item) -> bool {
    matches!(
        item.kind,
        fleet_core::agents::ItemKind::Tool(_) | fleet_core::agents::ItemKind::Subagent { .. }
    )
}

/// Whether an item has settled, whichever way it settled.
pub(crate) const fn is_settled(status: ItemStatus) -> bool {
    !matches!(status, ItemStatus::InProgress)
}

/// The turn a `TurnRecord` reports as failed, so a fold and a footer can say so.
pub(crate) fn turn_failed(turn: &TurnRecord) -> bool {
    turn.ended
        .as_ref()
        .is_some_and(|ended| matches!(ended.outcome, fleet_core::agents::TurnOutcome::Error { .. }))
}
