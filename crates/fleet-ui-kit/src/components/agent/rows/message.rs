//! The rows that carry prose: the user's turn, the assistant's answer and its footer, a
//! proposed plan, and the settled record of a resolved gate.

use gpui::{AnyElement, App, SharedString, div, prelude::*};

use super::render::{RowContext, body_region, chevron, header, separator, show_hint};
use super::{super::metrics, UserRowState};
use super::{
    AssistantMetaRow, AssistantRow, GateOutcome, GateRow, PlanRow, TranscriptRowId, UserRow,
};
use crate::{
    components::{KeyHint, markdown},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The user's turn: the only transcript block with a background.
///
/// No avatar, no name header, no role label — alignment and the bubble carry the role.
pub(super) fn user(row: &UserRow, id: &TranscriptRowId, ctx: &RowContext, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let toggle = ctx.toggle_for(id);
    let bubble = div()
        .flex()
        .flex_col()
        .gap(theme.space.xs)
        .max_w(metrics::AGENT_USER_MAX_W)
        .rounded(theme.radii.md)
        .bg(theme.colors.surface)
        .py(theme.space.sm)
        .px(theme.space.md)
        // A message still in flight is the cached-value opacity until the daemon's projection
        // catches up with the optimistic bubble.
        .when(row.state == UserRowState::Sending, |el| {
            el.opacity(theme.metrics.refreshing_opacity)
        })
        .when(row.state == UserRowState::Failed, |el| {
            el.border_l(theme.metrics.focus_ring_w)
                .border_color(theme.colors.danger)
        })
        .when(!row.attachments.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.space.xs)
                    .children(row.attachments.iter().map(|name| attachment_pill(name, cx))),
            )
        })
        .child(
            div()
                .flex()
                .items_start()
                .gap(theme.space.xs)
                // `↳` means "joined a turn that was already running" (§B5.4). It is a mark on
                // the message, not a second row.
                .when(row.steered, |el| {
                    el.child(Text::ui(SharedString::new_static("↳")).muted().flex_none())
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(row.collapsible && !row.expanded, |el| {
                            el.max_h(metrics::AGENT_PREVIEW_MAX_H).overflow_hidden()
                        })
                        .child(Text::ui(row.text.clone())),
                ),
        )
        .when(row.collapsible, |el| {
            el.child(
                div()
                    .id(("user-toggle", ctx.index))
                    .flex()
                    .when_some(toggle, |el, toggle| {
                        el.cursor_pointer()
                            .on_click(move |_, window, cx| toggle(window, cx))
                    })
                    .child(show_hint(row.expanded, true, "full message")),
            )
        })
        .when(row.state == UserRowState::Failed, |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(Text::hint(SharedString::new_static("not sent")).faint())
                    .child(KeyHint::labeled("r", "retry")),
            )
        });

    div()
        .w_full()
        .flex()
        .justify_end()
        .child(bubble)
        .into_any_element()
}

/// A neutral attachment pill, drawn above the user text that carries it.
pub(super) fn attachment_pill(name: &SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .h(theme.metrics.chip_h)
        .px(theme.space.sm)
        .flex()
        .items_center()
        .gap(theme.space.xxs)
        .rounded(theme.radii.sm)
        .bg(theme
            .colors
            .text
            .opacity(theme.metrics.neutral_fill_opacity))
        .child(Icon::Paperclip.el().size(IconSize::Small).tone(Tone::Muted))
        .child(Text::hint(name.clone()).tone(Tone::Secondary))
        .into_any_element()
}

/// Assistant prose: bare full-width Markdown on the app ground.
///
/// No bubble, no background, no avatar, no name header, no per-message model or token line.
pub(super) fn assistant(row: &AssistantRow, index: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    if row.empty {
        return Text::ui(SharedString::new_static("(empty response)"))
            .muted()
            .into_any_element();
    }
    div()
        .id(("assistant", index))
        .flex()
        .items_end()
        .gap(theme.space.xs)
        .px(theme.space.xs)
        .py(theme.space.xxs)
        .child(div().flex_1().min_w_0().child(markdown(&row.markdown, cx)))
        // One cell of caret trailing the last glyph. Its width and height are fixed, so the
        // paragraph does not reflow when it disappears.
        .when(row.streaming, |el| {
            el.child(
                div()
                    .flex_none()
                    .w(theme.metrics.cell_w)
                    .h(metrics::AGENT_CARET_H)
                    .bg(theme.colors.accent),
            )
        })
        .into_any_element()
}

/// The footer of a *terminal* assistant message: the copy key and when it last changed.
///
/// Only the last assistant message of a turn is terminal, and only a terminal message gets a
/// footer — a commentary message has nothing to copy yet.
pub(super) fn assistant_meta(row: &AssistantMetaRow, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .px(theme.space.xs)
        .child(KeyHint::labeled("y", "copy"))
        .child(Text::hint(row.updated_at.clone()).faint())
        .into_any_element()
}

/// The proposed plan: a durable artifact the reader will scroll back to, quote and copy.
///
/// It carries **no buttons**. `[y] implement` and `[n] refine` live on the composer, because
/// whether you implement or refine is decided by whether you typed anything.
pub(super) fn plan(row: &PlanRow, id: &TranscriptRowId, ctx: &RowContext, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let toggle = ctx.toggle_for(id);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(theme.space.sm)
        .rounded(theme.radii.md)
        .bg(theme.colors.surface)
        .p(theme.space.md)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(
                    div()
                        .flex_none()
                        .h(theme.metrics.chip_h)
                        .px(theme.space.sm)
                        .flex()
                        .items_center()
                        .rounded(theme.radii.sm)
                        .bg(Tone::Info.fill(theme))
                        .child(Text::label(SharedString::new_static("plan")).tone(Tone::Info)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::ui_strong(row.title.clone()).ellipsize()),
                ),
        )
        .child(
            div()
                .w_full()
                .when(row.collapsible && !row.expanded, |el| {
                    el.max_h(metrics::AGENT_PLAN_PREVIEW_H).overflow_hidden()
                })
                .child(markdown(&row.markdown, cx)),
        )
        .when(row.collapsible, |el| {
            el.child(
                div()
                    .id(("plan-toggle", ctx.index))
                    .flex()
                    .when_some(toggle, |el, toggle| {
                        el.cursor_pointer()
                            .on_click(move |_, window, cx| toggle(window, cx))
                    })
                    .child(show_hint(row.expanded, true, "full plan")),
            )
        })
        .into_any_element()
}

impl GateOutcome {
    /// The glyph and tone a settled gate record draws.
    fn glyph(self) -> (Icon, Tone) {
        match self {
            GateOutcome::Allowed => (Icon::Lock, Tone::Muted),
            GateOutcome::Declined => (Icon::CircleSlash, Tone::Muted),
            GateOutcome::Answered => (Icon::MessageSquareWarning, Tone::Muted),
            // A withdrawn request is not a failure: the agent stopped waiting.
            GateOutcome::Withdrawn => (Icon::Lock, Tone::Muted),
        }
    }
}

/// The one-line record a resolved gate leaves at the position it was asked.
pub(super) fn gate(row: &GateRow, id: &TranscriptRowId, ctx: &RowContext, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (icon, tone) = row.outcome.glyph();
    let expandable = row.payload.is_some();
    let line = header(("gate", ctx.index), expandable, ctx.toggle_for(id), theme)
        .child(icon.el().size(IconSize::Small).tone(tone))
        .child(Text::ui(row.label.clone()).muted().flex_none())
        .child(separator())
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(Text::data(row.detail.clone()).muted().ellipsize()),
        )
        .child(chevron(row.expanded, expandable));

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(line)
        .children(
            (row.expanded)
                .then(|| {
                    row.payload.clone().map(|payload| {
                        body_region(
                            ("gate-body", ctx.index),
                            theme,
                            Text::data_small(payload).muted(),
                        )
                    })
                })
                .flatten(),
        )
        .into_any_element()
}
