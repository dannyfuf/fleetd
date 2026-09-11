//! The rows that frame a turn rather than belong to it: its fold, its footer, the boundaries
//! between sessions, the harness's own notices, a severe failure, the live working row, and the
//! empty thread.

use gpui::{AnyElement, App, SharedString, div, prelude::*};

use super::super::metrics::{AGENT_BODY_MAX_H, AGENT_CONTENT_W};
use super::render::{RowContext, chevron, hairline, header, separator};
use super::{
    CheckpointRow, EmptyRow, ErrorRow, NoticeRow, TranscriptRowId, TurnFoldRow, TurnFooterRow,
};
use super::{WorkingPhase, WorkingRow};
use crate::{
    components::{KeyHint, KeyHintRow, Spinner},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The collapse affordance of a settled turn.
///
/// It is anchored at the first hidden entry's id, so it appears exactly where the hidden run
/// starts, and it stays visible when expanded with the chevron turned.
pub(super) fn turn_fold(
    row: &TurnFoldRow,
    id: &TranscriptRowId,
    ctx: &RowContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    header(("turn-fold", ctx.index), true, ctx.toggle_for(id), theme)
        .child(chevron(row.expanded, true))
        .child(Text::hint(row.label.clone()).faint().ellipsize())
        .into_any_element()
}

/// One right-aligned line after the terminal assistant message of a settled turn.
///
/// Turn metadata is withheld until the turn completes, so this row never grows a segment
/// mid-stream and shoves everything above it.
pub(super) fn turn_footer(row: &TurnFooterRow, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut children: Vec<AnyElement> = Vec::new();
    for (index, segment) in row.segments.iter().enumerate() {
        if index > 0 {
            children.push(separator().into_any_element());
        }
        children.push(
            Text::hint(segment.clone())
                .tone(Tone::Secondary)
                .flex_none()
                .into_any_element(),
        );
    }
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_end()
        .gap(theme.space.sm)
        .children(children)
        // §5: `[u]` is drawn only where a checkpoint exists. A drawn affordance that does
        // nothing is worse than an absent one.
        .children(row.diff.then(|| KeyHint::labeled("⏎", "diff")))
        .children(row.revert.then(|| KeyHint::labeled("u", "revert turn")))
        .into_any_element()
}

/// A centred separator: a compaction boundary or a resume.
pub(super) fn checkpoint(row: &CheckpointRow, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(theme.space.md)
        .child(hairline(theme))
        .child(Text::hint(row.label.clone()).faint().flex_none())
        .child(hairline(theme))
        .into_any_element()
}

/// One muted line from the harness itself. Not foldable, and no `[⏎] show`.
pub(super) fn notice(row: &NoticeRow, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(theme.space.sm)
        .child(Icon::Info.el().size(IconSize::Small).tone(Tone::Warning))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Text::ui(row.text.clone()).muted()),
        )
        .into_any_element()
}

/// The severe tier of failure: a card with a 2 px danger bar and, when the daemon says the turn
/// is retryable, the key that retries it.
///
/// A nonzero command exit never reaches here — that is the routine tier, carried by the failing
/// work row as a dimmed icon and an `exit N` field.
pub(super) fn error(row: &ErrorRow, index: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .max_w(AGENT_CONTENT_W)
        .min_h(theme.metrics.banner_h)
        .flex()
        .items_stretch()
        .rounded(theme.radii.md)
        .overflow_hidden()
        .bg(theme.colors.surface)
        .child(
            div()
                .flex_none()
                .w(theme.metrics.focus_ring_w)
                .bg(theme.colors.danger),
        )
        .child(
            // A provider error message is the text that surfaces protocol drift, so the card
            // grows to the body cap and scrolls rather than ellipsizing one line.
            div()
                .id(("error-card", index))
                .flex_1()
                .min_w_0()
                .px(theme.space.md)
                .py(theme.space.xs)
                .max_h(AGENT_BODY_MAX_H)
                .overflow_y_scroll()
                .flex()
                .items_start()
                .gap(theme.space.sm)
                .child(
                    div().flex_none().pt(theme.space.xxs).child(
                        Icon::TriangleAlert
                            .el()
                            .size(IconSize::Small)
                            .tone(Tone::Danger),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(theme.space.xs)
                        .child(Text::ui(row.message.clone()))
                        .children(row.retryable.then(|| KeyHint::labeled("r", "retry"))),
                ),
        )
        .into_any_element()
}

impl WorkingPhase {
    /// The copy this phase draws, given the harness name and the clock label the list ticked.
    fn label(self, harness: &SharedString, clock: Option<&SharedString>) -> SharedString {
        match self {
            WorkingPhase::Working => clock
                .cloned()
                .unwrap_or_else(|| SharedString::new_static("working")),
            WorkingPhase::Starting => SharedString::from(format!("starting {harness}…")),
            WorkingPhase::Compacting => SharedString::new_static("compacting…"),
            WorkingPhase::Resuming => SharedString::new_static("resuming…"),
            // A rate-limit window is a parked turn, not an error: the harness may send no
            // result at all, so nothing may time out and mark the turn failed.
            WorkingPhase::Parked => SharedString::new_static("paused"),
        }
    }
}

/// The pinned row immediately before the active turn's first row.
pub(super) fn working(
    row: &WorkingRow,
    clock: Option<&SharedString>,
    index: usize,
    visible: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let parked = row.phase == WorkingPhase::Parked;
    let glyph = if parked {
        Icon::Clock
            .el()
            .size(IconSize::Small)
            .tone(Tone::Warning)
            .into_any_element()
    } else if visible {
        Spinner::new(("working", index))
            .size(IconSize::Small)
            .tone(Tone::Secondary)
            .into_any_element()
    } else {
        // Off screen: the same glyph, not turning. Animation is viewport-gated.
        Icon::LoaderCircle
            .el()
            .size(IconSize::Small)
            .tone(Tone::Secondary)
            .into_any_element()
    };
    let label = row.phase.label(&row.harness, clock);

    div()
        .w_full()
        .min_h(theme.metrics.row_h)
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .child(div().flex_none().child(glyph))
        .child(Text::hint(label).tone(Tone::Secondary).flex_none())
        .children(row.detail.clone().map(|_| separator()))
        .children(
            row.detail
                .clone()
                .map(|detail| Text::hint(detail).faint().flex_none()),
        )
        .children(parked.then(|| KeyHint::labeled("⏎", "details")))
        .into_any_element()
}

/// The empty thread: what it is, and the three triggers.
pub(super) fn empty(row: &EmptyRow, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(theme.space.sm)
        .py(theme.space.xxl)
        .child(Text::ui_strong(row.message.clone()))
        .child(
            KeyHintRow::new()
                .key("⏎", "ask anything")
                .key("@", "files")
                .key("$", "skills")
                .key("/", "commands"),
        )
        .into_any_element()
}
