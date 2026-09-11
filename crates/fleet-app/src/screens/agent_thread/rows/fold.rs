//! Which rows of a settled turn collapse behind `worked 22s · 14 steps` (`spec-B` §B1.2).
//!
//! Every rule in the table closes a real race, so the derivation is a pure function over the
//! turn's own items and is tested one case per row. The fold is **anchored at the first hidden
//! entry**, so it appears exactly where the hidden run starts rather than under the turn's
//! closing paragraph, as if the tools had run after the answer.

use std::collections::HashSet;

use fleet_core::agents::{Item, ItemId, ItemKind, ItemStatus, TurnId, TurnRecord};

use super::{RowInputs, is_settled, is_work};

/// The fold of one turn: where its row sits and what it hides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fold {
    /// Position in the turn's own item list the fold row is emitted at.
    pub(crate) anchor: usize,
    /// Items the fold hides, in display order.
    pub(crate) hidden: Vec<ItemId>,
    /// Whether the user has opened it. The row stays visible either way, chevron rotated.
    pub(crate) expanded: bool,
}

impl Fold {
    /// Whether one item is behind this fold.
    pub(crate) fn hides(&self, item: ItemId) -> bool {
        !self.expanded && self.hidden.contains(&item)
    }
}

/// Why a turn draws no fold at all.
///
/// Returned rather than folded into a bare `bool` so the exemption table is testable by name and
/// a future rule cannot be added without saying which race it closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exempt {
    /// The turn has not settled: nothing about it is history yet.
    Unsettled,
    /// The session's running turn lags the latest recorded one — the window right after a send,
    /// before the harness mints the new turn id. Folding through it flickers.
    RunningTurnLags,
    /// The turn appears after the last user message while the thread is working: a promptless
    /// harness restart replaces the turn without adding a user message.
    AfterLastUserMessage,
    /// The turn still holds a streaming message.
    Streaming,
    /// Nothing in the turn is foldable.
    ///
    /// This also covers §B1.2's "a fold whose entire hidden set is one compaction row is not
    /// emitted": a compaction is a [`fleet_core::agents::CheckpointRecord`] in Fleet's model,
    /// never an item, so it is never in a fold's hidden set to begin with and a lone
    /// `— compacted —` separator always stands on its own.
    NothingToFold,
}

/// The fold for one settled turn, or why it has none.
///
/// `items` are the turn's **top-level** items in display order; `terminal` is the index of its
/// terminal assistant message when it has one.
pub(crate) fn derive(
    inputs: &RowInputs<'_>,
    record: &TurnRecord,
    items: &[&Item],
    terminal: Option<usize>,
    unsettled: Option<TurnId>,
) -> Result<Fold, Exempt> {
    if record.ended.is_none() {
        return Err(Exempt::Unsettled);
    }
    if unsettled == Some(record.id) {
        return Err(Exempt::RunningTurnLags);
    }
    if inputs.is_working() && is_after_last_user_message(inputs, record.id) {
        return Err(Exempt::AfterLastUserMessage);
    }
    if items.iter().any(|item| is_streaming_message(item)) {
        return Err(Exempt::Streaming);
    }

    let live_subagents: HashSet<ItemId> = items
        .iter()
        .filter(|item| is_live_subagent(inputs, item))
        .map(|item| item.id)
        .collect();
    let mut hidden = Vec::new();
    let mut anchor = None;
    for (index, item) in items.iter().enumerate() {
        if !folds(item, index, terminal, &live_subagents) {
            continue;
        }
        anchor.get_or_insert(index);
        hidden.push(item.id);
    }
    let Some(anchor) = anchor else {
        return Err(Exempt::NothingToFold);
    };
    if hidden.len() == 1
        && items
            .iter()
            .find(|item| item.id == hidden[0])
            .is_some_and(|item| matches!(item.kind, ItemKind::Plan { .. }))
    {
        // A lone plan is the turn's artifact, not its scaffolding.
        return Err(Exempt::NothingToFold);
    }
    Ok(Fold {
        anchor,
        hidden,
        expanded: inputs.unfolded.contains(&record.id),
    })
}

/// Whether one item of a settled turn goes behind the fold.
///
/// Everything except the terminal assistant message folds, with four carve-outs: a failed row
/// never folds, a subagent row never folds while its work is live, the user's own message stays,
/// and exactly one non-failing work entry trailing the terminal message stays visible.
fn folds(
    item: &Item,
    index: usize,
    terminal: Option<usize>,
    live_subagents: &HashSet<ItemId>,
) -> bool {
    if matches!(item.kind, ItemKind::UserMessage { .. }) {
        return false;
    }
    if terminal == Some(index) {
        return false;
    }
    // A user who scrolls back must see what broke without hunting for it.
    if matches!(
        item.status,
        ItemStatus::Failed | ItemStatus::Denied | ItemStatus::Stopped
    ) {
        return false;
    }
    if matches!(item.kind, ItemKind::Error { .. }) {
        return false;
    }
    // Workflows outlive their launching turn: folding the roster when the turn settles makes a
    // still-running fleet invisible.
    if live_subagents.contains(&item.id) {
        return false;
    }
    true
}

/// Whether a subagent row's fleet is still running.
fn is_live_subagent(inputs: &RowInputs<'_>, item: &Item) -> bool {
    matches!(item.kind, ItemKind::Subagent { .. })
        && (!is_settled(item.status) || inputs.projection.background_tasks.contains(&item.id))
}

/// Whether one item is a message the harness is still writing.
fn is_streaming_message(item: &Item) -> bool {
    !is_settled(item.status)
        && matches!(
            item.kind,
            ItemKind::AssistantText { .. } | ItemKind::Reasoning { .. }
        )
}

/// Whether `turn` comes after the newest turn that carries a user message.
///
/// A promptless harness restart replaces the turn without adding a user message, and folding it
/// while the thread is working hides the work that restart is doing.
fn is_after_last_user_message(inputs: &RowInputs<'_>, turn: TurnId) -> bool {
    let turns = &inputs.projection.turns;
    let Some(position) = turns.iter().position(|record| record.id == turn) else {
        return false;
    };
    let last_prompt = turns
        .iter()
        .rposition(|record| record.user_item.is_some())
        .unwrap_or(0);
    position > last_prompt
}

/// How many steps a fold states, which is what the label counts.
///
/// Work entries, not rows: an opened fold showing three tool rows and a group header still says
/// `14 steps`, so opening one never changes the number the reader just read.
pub(crate) fn steps(items: &[&Item], fold: &Fold) -> usize {
    items
        .iter()
        .filter(|item| fold.hidden.contains(&item.id) && is_work(item))
        .count()
}
