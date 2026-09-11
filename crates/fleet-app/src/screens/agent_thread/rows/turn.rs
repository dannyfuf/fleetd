//! One turn's rows, in `spec-B` §B1.4's emission order.
//!
//! A turn is **not** a container: this function appends a run of rows to the flat list and the
//! turn's identity survives only in the ids of its fold and footer. One forward pass over the
//! turn's top-level items, with the live tail, the fold and the group runs all derived before
//! the loop starts, so no row costs a scan of the transcript.

use std::collections::HashMap;

use fleet_core::agents::{
    Item, ItemId, ItemKind, ItemStatus, TurnId, TurnOutcome, TurnRecord, TurnState,
};
use fleet_ui_kit::{
    TranscriptRow, TranscriptRowId, TranscriptRowKind, TurnFoldRow, TurnFooterRow,
    format_stopped_after, format_worked, turn_footer_segments,
};
use gpui::SharedString;

use super::{BuiltRows, RowInputs, RowTarget, fold, group, is_settled, item, live, turn_failed};

/// Appends every row of one recorded turn.
pub(crate) fn emit(
    inputs: &RowInputs<'_>,
    items: &HashMap<ItemId, &Item>,
    record: &TurnRecord,
    unsettled: Option<TurnId>,
    built: &mut BuiltRows,
) {
    let own: Vec<&Item> = inputs
        .projection
        .items
        .iter()
        .filter(|item| item.turn == record.id && item.parent.is_none())
        .collect();
    let terminal = terminal_assistant(&own);
    let active = matches!(inputs.projection.turn, TurnState::Running(turn) if turn == record.id);

    // The live tail is derived before anything is emitted, because the row it stands for takes
    // the *position* of the last entry it subsumes.
    let tail = if active { live::tail(&own) } else { Vec::new() };
    let subsumed = live::subsumed(&own, &tail);
    let folded = fold::derive(inputs, record, &own, terminal, unsettled).ok();

    let mut index = 0;
    while index < own.len() {
        let entry = own[index];
        if folded.as_ref().is_some_and(|fold| fold.anchor == index) {
            emit_fold(record, &own, folded.as_ref(), built);
        }
        if folded.as_ref().is_some_and(|fold| fold.hides(entry.id)) {
            index += 1;
            continue;
        }
        if tail.first() == Some(&index)
            && let Some(row) = live::row(&own, &tail, active)
        {
            built.push(
                TranscriptRow::new(
                    TranscriptRowId::LiveActivity,
                    TranscriptRowKind::WorkLive(row),
                ),
                None,
            );
        }
        if subsumed.contains(&entry.id) {
            index += 1;
            continue;
        }
        // Greedily group the consecutive settled work entries that survive the fold and the
        // live row. A subagent and a severe error are single-entry rows and never grouped.
        let run = run_at(&own, index, folded.as_ref(), &subsumed);
        if run.len() > 1 || group::summarizes_alone(&run) {
            emit_group(inputs, items, &run, built);
            index += run.len();
            continue;
        }
        for (row, target) in item::rows_for(inputs, items, entry) {
            let streaming = matches!(
                entry.kind,
                ItemKind::AssistantText { .. } | ItemKind::Reasoning { .. }
            ) && !is_settled(entry.status);
            built.push(row, target);
            if streaming {
                built.mark_streaming(entry.id);
            }
        }
        index += 1;
    }
    // A fold anchored past the last surviving entry still has to be drawn.
    if folded.as_ref().is_some_and(|fold| fold.anchor >= own.len()) {
        emit_fold(record, &own, folded.as_ref(), built);
    }
    emit_close(inputs, record, &own, terminal, built);
}

/// Items the reducer accepted before their turn was recorded, in projection order.
///
/// They are still the user's transcript: dropping them because the turn record has not arrived
/// would make a thread look empty for exactly as long as that race lasts.
pub(crate) fn emit_orphans(
    inputs: &RowInputs<'_>,
    items: &HashMap<ItemId, &Item>,
    orphans: &[&Item],
    built: &mut BuiltRows,
) {
    for entry in orphans {
        for (row, target) in item::rows_for(inputs, items, entry) {
            built.push(row, target);
        }
        if matches!(
            entry.kind,
            ItemKind::AssistantText { .. } | ItemKind::Reasoning { .. }
        ) && !is_settled(entry.status)
        {
            built.mark_streaming(entry.id);
        }
    }
}

/// The run of consecutive groupable entries starting at `from`.
fn run_at<'a>(
    own: &[&'a Item],
    from: usize,
    folded: Option<&fold::Fold>,
    subsumed: &[ItemId],
) -> Vec<&'a Item> {
    let mut run = Vec::new();
    for entry in own.iter().skip(from) {
        let groupable = matches!(entry.kind, ItemKind::Tool(_))
            && is_settled(entry.status)
            && entry.status != ItemStatus::Failed
            && !folded.is_some_and(|fold| fold.hides(entry.id))
            && !subsumed.contains(&entry.id);
        if !groupable {
            break;
        }
        run.push(*entry);
    }
    run
}

/// The group header and, while it is open, its own rows.
fn emit_group(
    inputs: &RowInputs<'_>,
    items: &HashMap<ItemId, &Item>,
    run: &[&Item],
    built: &mut BuiltRows,
) {
    // The group is keyed by its first member, which is stable for the group's whole life: a
    // later entry joining the run must not remount the header the reader is looking at.
    let Some(first) = run.first() else {
        return;
    };
    let expanded = inputs.is_expanded(first.id);
    let Some(row) = group::row(run, expanded) else {
        return;
    };
    built.push(
        TranscriptRow::new(
            TranscriptRowId::Item(SharedString::from(format!("group-{}", first.id))),
            TranscriptRowKind::WorkGroup(row),
        )
        .attached(expanded),
        Some(RowTarget::Item(first.id)),
    );
    if !expanded {
        return;
    }
    for entry in run {
        for (row, target) in item::rows_for(inputs, items, entry) {
            built.push(row, target);
        }
    }
}

/// The `worked 22s · 14 steps` row, stated in the turn's **own** time.
///
/// Every interval a gate stood open is the user's time and the reducer has already subtracted
/// it, which is why a turn that was on screen for four minutes folds as `worked 22s`.
fn emit_fold(
    record: &TurnRecord,
    own: &[&Item],
    folded: Option<&fold::Fold>,
    built: &mut BuiltRows,
) {
    let Some(folded) = folded else {
        return;
    };
    let key = SharedString::from(format!("fold-{}", record.id));
    if built.targets.contains_key(&key) {
        return;
    }
    let duration = record.footer().map(|footer| footer.duration_ms);
    let interrupted = matches!(
        record.ended.as_ref().map(|ended| &ended.outcome),
        Some(TurnOutcome::Interrupted)
    );
    let label = match (interrupted, duration) {
        (true, Some(duration)) => format_stopped_after(duration),
        (true, None) => SharedString::new_static("you stopped"),
        (false, duration) => format_worked(duration, fold::steps(own, folded)),
    };
    built.push(
        TranscriptRow::new(
            TranscriptRowId::Turn(key),
            TranscriptRowKind::TurnFold(TurnFoldRow {
                label,
                expanded: folded.expanded,
            }),
        ),
        Some(RowTarget::Turn(record.id)),
    );
}

/// The rows that close a settled turn: the detached assistant meta, then one footer.
///
/// The meta is re-emitted **after** the turn's last trailing work row rather than beside the
/// prose it belongs to, so prose + tools + footer read as one block.
fn emit_close(
    inputs: &RowInputs<'_>,
    record: &TurnRecord,
    own: &[&Item],
    terminal: Option<usize>,
    built: &mut BuiltRows,
) {
    let Some(ended) = record.ended.as_ref() else {
        return;
    };
    if let Some(item) = terminal.and_then(|index| own.get(index)) {
        let (row, target) = item::assistant_meta(item);
        built.push(row, target);
    }
    let footer = record.footer();
    let mut segments = turn_footer_segments(
        footer
            .map(|footer| footer.tokens)
            .filter(|tokens| *tokens > 0),
        inputs.projection.cumulative_cost_usd,
        footer
            .filter(|footer| footer.files_changed > 0)
            .map(|footer| (footer.files_changed, footer.added, footer.removed)),
    );
    // The outcome leads the footer when it is not a plain completion, because "what happened"
    // outranks "what it cost".
    if let Some(word) = outcome_word(&ended.outcome) {
        segments.insert(0, SharedString::new_static(word));
    }
    built.push(
        TranscriptRow::new(
            TranscriptRowId::Turn(SharedString::from(format!("footer-{}", record.id))),
            TranscriptRowKind::TurnFooter(TurnFooterRow {
                segments,
                diff: footer.is_some_and(|footer| footer.files_changed > 0),
                revert: inputs.checkpoints.contains_key(&record.id),
            }),
        ),
        Some(RowTarget::Turn(record.id)),
    );
    // §B1.2's severe tier: a turn the harness reported failed is a card, and it is the last
    // thing in the turn so the reader meets it after the work that led to it.
    if turn_failed(record)
        && let TurnOutcome::Error { message } = &ended.outcome
    {
        built.push(
            TranscriptRow::new(
                TranscriptRowId::Turn(SharedString::from(format!("error-{}", record.id))),
                TranscriptRowKind::Error(fleet_ui_kit::ErrorRow {
                    message: SharedString::from(
                        message
                            .clone()
                            .unwrap_or_else(|| "the turn failed".to_owned()),
                    ),
                    retryable: false,
                }),
            ),
            None,
        );
    }
}

/// The word a non-plain outcome leads its footer with.
const fn outcome_word(outcome: &TurnOutcome) -> Option<&'static str> {
    match outcome {
        TurnOutcome::Completed => None,
        TurnOutcome::Interrupted => Some("stopped"),
        TurnOutcome::Error { .. } => Some("failed"),
        TurnOutcome::Denied => Some("denied"),
        TurnOutcome::MaxTurns => Some("turn limit"),
        TurnOutcome::BudgetExhausted => Some("budget exhausted"),
        // A harness-specific reason is shown verbatim by the error card below rather than
        // squeezed into a footer segment, because it is a sentence and not a word.
        TurnOutcome::Other { .. } => Some("stopped"),
    }
}

/// The index of the turn's **terminal** assistant message.
///
/// Only the last assistant message of a turn is terminal; every earlier one is commentary. Only
/// a terminal message gets a meta footer and a turn footer.
pub(crate) fn terminal_assistant(own: &[&Item]) -> Option<usize> {
    own.iter()
        .enumerate()
        .rev()
        .find(|(_, item)| {
            matches!(item.kind, ItemKind::AssistantText { .. })
                && !item::item_text(item).trim().is_empty()
        })
        .map(|(index, _)| index)
}
