//! The transcript's row vocabulary: what a row *is*, before anything draws it.
//!
//! `docs/NATIVE-AGENTS.md` §5 fixes the shape: **the transcript is a flat list of rows, never a
//! tree of turn widgets.** A turn is an emergent run of rows, and a tool's children are
//! separate rows emitted after it rather than a `Vec<Row>` field, so the virtualizer measures
//! every visible thing exactly once. Nested containers make variable-height virtualization and
//! scroll anchoring unsolvable.
//!
//! Everything here is a plain value with no domain type in sight: the owner projects its model
//! into these rows once, memoised behind a revision key, and `render` composes prepared values
//! and nothing else.

use std::time::Instant;

use gpui::SharedString;

use super::tool_row::ToolRow;
use crate::{components::MarkdownDocument, icons::Icon};

mod chrome;
mod message;
mod render;
mod work;

pub(super) use render::{RowContext, row_element};

/// The identity of the thing a row draws, stable for that thing's whole life.
///
/// Two ids are shared on purpose, and both exist to stop a status change from remounting a row:
///
/// - [`TranscriptRowId::LiveActivity`] is used by the live work row, by a streaming reasoning
///   row and by the working row, so *thinking → tool A running → tool A done → tool B running*
///   is **one row changing its label**, not four mounts. Constant height, zero layout thrash.
/// - [`TranscriptRowId::Item`] is shared by a work row, the diff row under it and a subagent
///   row, so a `tool.updated` merging forward into a `tool.completed` does not remount.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TranscriptRowId {
    /// The single in-place live row.
    LiveActivity,
    /// A transcript item — a message, a tool call, a gate, a plan.
    Item(SharedString),
    /// A turn-level row: its fold, its footer.
    Turn(SharedString),
    /// A row that draws no addressable thing, keyed by its position instead.
    Ordinal(usize),
}

impl TranscriptRowId {
    /// The key the owner receives back in a [`super::TranscriptEvent`].
    ///
    /// A `TranscriptRowId` is a *model* identity, not an [`gpui::ElementId`]: element ids are
    /// keyed by list index so gpui can recycle hover and scroll state, while this is what an
    /// expand toggle, the row focus and the row diff compare.
    #[must_use]
    pub fn key(&self) -> SharedString {
        match self {
            TranscriptRowId::LiveActivity => SharedString::new_static("live-activity"),
            TranscriptRowId::Item(id) | TranscriptRowId::Turn(id) => id.clone(),
            TranscriptRowId::Ordinal(index) => SharedString::from(format!("row-{index}")),
        }
    }
}

/// How much air a row leaves under itself (`spec-B` §B1.3).
///
/// Vertical rhythm is a bottom-padding decision per row, resolved against the 4 px scale by the
/// renderer so the ladder itself stays testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRhythm {
    /// A detail row inside an expanded group.
    Detail,
    /// A header whose details sit directly below it: no gap at all.
    Attached,
    /// A fold or the working row.
    Fold,
    /// Work rows, reasoning, and commentary prose.
    Work,
    /// A block that closes a thought: a user message, a terminal answer, a plan, a gate.
    Block,
}

/// Whether a user message was sent, is still in flight, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRowState {
    /// The daemon has it.
    Sent,
    /// Dispatched, not yet reflected in the projection.
    Sending,
    /// The send failed and `[r]` retries it.
    Failed,
}

/// The user's own turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    /// What the user typed, with Fleet's own send-time additions already stripped.
    pub text: SharedString,
    /// Attachment display names, drawn as pills above the text.
    pub attachments: Vec<SharedString>,
    /// Send state.
    pub state: UserRowState,
    /// Whether the message joined a turn that was already running.
    pub steered: bool,
    /// Whether the body is long enough to collapse (> 600 chars or > 8 lines).
    pub collapsible: bool,
    /// Whether a collapsible body is currently open.
    pub expanded: bool,
}

impl UserRow {
    /// A sent message.
    #[must_use]
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
            state: UserRowState::Sent,
            steered: false,
            collapsible: false,
            expanded: false,
        }
    }
}

/// Assistant prose, on the app ground with no bubble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantRow {
    /// The parsed document. Parsing happens in the projection, never in render.
    pub markdown: MarkdownDocument,
    /// Whether a caret trails the last glyph.
    pub streaming: bool,
    /// A finished message with empty text draws the literal `(empty response)`.
    pub empty: bool,
}

/// The hover-revealed footer of a *terminal* assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMetaRow {
    /// When the message last changed.
    pub updated_at: SharedString,
}

/// A reasoning block: one collapsed line, or the buffered summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningRow {
    /// The buffered reasoning text, shown only while expanded.
    pub text: SharedString,
    /// First delta → item completion. `None` while it is still thinking.
    pub duration_ms: Option<u64>,
    /// Whether the body is exposed.
    pub expanded: bool,
}

/// The single in-place live activity row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkLiveRow {
    /// The present-tense label: `running cargo`, `reading src/main.rs`, `editing 3 files`.
    ///
    /// Tense is a rule, not a lookup: a *completed* command inside the live row still reads
    /// `running cargo`, so the row never flickers between tenses while the turn is alive.
    pub label: SharedString,
    /// The kind glyph.
    pub icon: Icon,
    /// Whether the shimmer runs. The renderer additionally gates it on viewport visibility.
    pub shimmer: bool,
}

/// A run of adjacent settled tool rows, collapsed to one generated summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkGroupRow {
    /// The generated summary — `read 3 files and ran 2 commands`.
    pub summary: SharedString,
    /// The leading glyph.
    pub icon: Icon,
    /// How many rows the group hides.
    pub hidden: usize,
    /// Whether the group's own rows follow it.
    pub expanded: bool,
    /// Whether the **latest** entry in the group failed.
    ///
    /// Group failure is neutral by default: a group holding one failure and three successes
    /// draws no danger mark. The failure still reaches the accessibility label.
    pub latest_failed: bool,
}

/// A subagent spawn and its roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRow {
    /// `spawned 3 subagents · explore, verify, write`.
    pub summary: SharedString,
    /// The right-aligned live count, or `✓ done` / `failed` / `stopped` once settled.
    pub status: Option<SharedString>,
    /// Cumulative tokens, when the harness reports them.
    pub tokens: Option<SharedString>,
    /// One line per child, in roster order, already truncated by the projection.
    pub children: Vec<SharedString>,
    /// Whether the child region is exposed.
    pub expanded: bool,
    /// Whether the fleet is still running, which is what keeps the row unfoldable.
    pub live: bool,
}

/// The diff body of an expanded edit row, emitted as its own row so its height is measured
/// independently and an expanded diff never inflates the tool row's own measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    /// The item the diff belongs to. The owner renders it — `DiffView` lives in
    /// `fleet-lazygit`, so the kit gains no `fleet-git` dependency (ADR 0010).
    pub item: SharedString,
    /// The fallback unified-diff text, drawn when the owner supplies no element.
    pub unified: SharedString,
}

/// The collapse affordance of a settled turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFoldRow {
    /// `worked 22s · 14 steps`, or `you stopped after 12s`.
    pub label: SharedString,
    /// Whether the hidden run is exposed. The row stays visible either way, with its chevron
    /// rotated.
    pub expanded: bool,
}

/// One right-aligned line after the terminal assistant message of a settled turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFooterRow {
    /// The segments the harness actually reported, in order. Never invented.
    pub segments: Vec<SharedString>,
    /// Whether a turn diff exists to open.
    pub diff: bool,
    /// Whether a Fleet checkpoint exists for this turn. `[u]` is drawn only when it does — a
    /// drawn affordance that does nothing is worse than an absent one.
    pub revert: bool,
}

/// A proposed plan: the only rich decision artifact in the transcript, and it carries **no
/// buttons**. The verbs live on the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    /// The plan's first Markdown heading, promoted out of the body and removed from it.
    pub title: SharedString,
    /// The body, already parsed.
    pub markdown: MarkdownDocument,
    /// Whether the body is long enough to fade out (> 900 chars or > 20 lines).
    pub collapsible: bool,
    /// Whether the full body is exposed.
    pub expanded: bool,
}

/// What a resolved gate turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// Allowed, in any scope.
    Allowed,
    /// Declined, with or without a stop.
    Declined,
    /// Answered — a question.
    Answered,
    /// The agent stopped waiting, or a restart made the request stale.
    Withdrawn,
}

/// The settled record a resolved gate leaves at the position it was asked.
///
/// This is the row that makes docking the live decision safe: the drawer is always on screen,
/// and the history of what was asked stays in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRow {
    /// What happened.
    pub outcome: GateOutcome,
    /// The scope clause: `allowed once`, `allowed for this session`, `answered`.
    pub label: SharedString,
    /// The one-line subject: `Bash: git push --force`, `which package manager? → pnpm`.
    pub detail: SharedString,
    /// The full request payload and answer, shown when the row is expanded.
    pub payload: Option<SharedString>,
    /// Whether the payload is exposed.
    pub expanded: bool,
}

/// A centred separator: a compaction boundary or a resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRow {
    /// `compacted 120k → 30k`, `resumed · 2h ago`.
    pub label: SharedString,
}

/// One muted line from the harness itself: a config warning, a deprecation, a retry.
///
/// Not foldable and no `[⏎] show`: neither wire gives a notice a body, and the design system
/// does not list a command that cannot fire. An unrecognised harness frame is a `tracing`
/// diagnostic, never a notice — the transcript keeps the signals and the log keeps the noise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeRow {
    /// What the harness said.
    pub text: SharedString,
}

/// The severe tier of failure: a card, not a line.
///
/// The routine tier is not a row at all — the failing work row carries it. `Retrying` is not an
/// error either: a backoff renders as a [`NoticeRow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRow {
    /// What broke.
    pub message: SharedString,
    /// Whether the daemon says the turn can be retried, which is what draws `[r] retry`.
    pub retryable: bool,
}

/// What the working row is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingPhase {
    /// A turn is running.
    Working,
    /// The session process is starting.
    Starting,
    /// The harness is compacting its context.
    Compacting,
    /// The session is being resumed.
    Resuming,
    /// A rate-limit window parked the turn. No timeout may mark it failed.
    Parked,
}

/// The pinned row immediately before the active turn's first row.
///
/// **The clock is not in the model.** The row carries `started_at`, never an elapsed figure,
/// and the list is the only thing that ticks — one element repaints per second instead of the
/// whole tree.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingRow {
    /// Which phase's copy to draw.
    pub phase: WorkingPhase,
    /// When the turn started, for the `working 1m 12s` clock.
    pub started_at: Option<Instant>,
    /// The harness's name, for `starting claude…`.
    pub harness: SharedString,
    /// The parked detail line, e.g. `weekly limit resets in ~3h`.
    pub detail: Option<SharedString>,
}

/// The empty thread: the harness, the model, the worktree and the three triggers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyRow {
    /// `new claude thread · feat/payroll-fix`.
    pub message: SharedString,
}

/// One line of the transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptRowKind {
    /// The user's own turn.
    User(UserRow),
    /// Assistant prose.
    Assistant(AssistantRow),
    /// The footer of a terminal assistant message.
    AssistantMeta(AssistantMetaRow),
    /// A reasoning block.
    Reasoning(ReasoningRow),
    /// One settled tool row.
    Work(ToolRow),
    /// The single in-place live row.
    WorkLive(WorkLiveRow),
    /// A collapsed run of settled tool rows.
    WorkGroup(WorkGroupRow),
    /// A subagent spawn and its roster.
    Subagent(SubagentRow),
    /// The diff body of an expanded edit row.
    Diff(DiffRow),
    /// A settled turn's collapse affordance.
    TurnFold(TurnFoldRow),
    /// A settled turn's metadata.
    TurnFooter(TurnFooterRow),
    /// A proposed plan.
    Plan(PlanRow),
    /// The settled record of a resolved gate.
    Gate(GateRow),
    /// A compaction or resume boundary.
    Checkpoint(CheckpointRow),
    /// A harness notice.
    Notice(NoticeRow),
    /// A severe error.
    Error(ErrorRow),
    /// The live working row.
    Working(WorkingRow),
    /// The empty thread.
    Empty(EmptyRow),
}

/// One projected transcript row: an identity and what it draws.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptRow {
    /// Stable identity, which is what the row diff and the row focus compare.
    pub id: TranscriptRowId,
    /// What the row draws.
    pub kind: TranscriptRowKind,
    /// Whether the row's details sit directly below it, which closes the gap under it.
    pub attached: bool,
}

impl TranscriptRow {
    /// A row with an explicit identity.
    #[must_use]
    pub const fn new(id: TranscriptRowId, kind: TranscriptRowKind) -> Self {
        Self {
            id,
            kind,
            attached: false,
        }
    }

    /// A row identified by its position, for the rows that draw no addressable thing.
    #[must_use]
    pub const fn ordinal(index: usize, kind: TranscriptRowKind) -> Self {
        Self::new(TranscriptRowId::Ordinal(index), kind)
    }

    /// Close the gap under this row because its own details follow it.
    #[must_use]
    pub const fn attached(mut self, attached: bool) -> Self {
        self.attached = attached;
        self
    }

    /// Whether `⏎` on this row expands or collapses something.
    #[must_use]
    pub fn is_expandable(&self) -> bool {
        match &self.kind {
            TranscriptRowKind::Work(row) => row.is_expandable(),
            TranscriptRowKind::User(row) => row.collapsible,
            TranscriptRowKind::Plan(row) => row.collapsible,
            TranscriptRowKind::Gate(row) => row.payload.is_some(),
            TranscriptRowKind::Reasoning(_)
            | TranscriptRowKind::WorkGroup(_)
            | TranscriptRowKind::Subagent(_)
            | TranscriptRowKind::TurnFold(_) => true,
            TranscriptRowKind::Assistant(_)
            | TranscriptRowKind::AssistantMeta(_)
            | TranscriptRowKind::WorkLive(_)
            | TranscriptRowKind::Diff(_)
            | TranscriptRowKind::TurnFooter(_)
            | TranscriptRowKind::Checkpoint(_)
            | TranscriptRowKind::Notice(_)
            | TranscriptRowKind::Error(_)
            | TranscriptRowKind::Working(_)
            | TranscriptRowKind::Empty(_) => false,
        }
    }

    /// How much air the row leaves under itself (`spec-B` §B1.3), checked in that order.
    #[must_use]
    pub fn rhythm(&self) -> TranscriptRhythm {
        if self.attached {
            return TranscriptRhythm::Attached;
        }
        match &self.kind {
            TranscriptRowKind::Diff(_) => TranscriptRhythm::Detail,
            TranscriptRowKind::TurnFold(_) | TranscriptRowKind::Working(_) => {
                TranscriptRhythm::Fold
            }
            TranscriptRowKind::Work(_)
            | TranscriptRowKind::WorkLive(_)
            | TranscriptRowKind::WorkGroup(_)
            | TranscriptRowKind::Subagent(_)
            | TranscriptRowKind::Reasoning(_) => TranscriptRhythm::Work,
            _ => TranscriptRhythm::Block,
        }
    }
}

/// The single [`gpui::ListState::splice`] that turns one row projection into the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSplice {
    /// The replaced range of old rows.
    pub old_range: std::ops::Range<usize>,
    /// How many new rows take its place.
    pub count: usize,
}

/// The rows that really changed between two projections, or `None` when nothing did.
///
/// The common head and tail are matched by content, so a row that did not change keeps the
/// height the list measured for it. Appending one row splices `len..len`; a streaming row
/// splices only itself. This is the load-bearing property of §B7.6: **a stream chunk never
/// re-runs grouping, folding or summarization**, and the splice it produces touches one row.
#[must_use]
pub fn diff_rows(old: &[TranscriptRow], new: &[TranscriptRow]) -> Option<RowSplice> {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(before, after)| before == after)
        .count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let bounded = old.len().min(new.len()) - prefix;
    let suffix = (0..bounded)
        .take_while(|back| old[old.len() - 1 - back] == new[new.len() - 1 - back])
        .count();
    Some(RowSplice {
        old_range: prefix..old.len() - suffix,
        count: new.len() - suffix - prefix,
    })
}

#[cfg(test)]
mod tests;
