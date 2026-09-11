//! The rows that show what the agent did: reasoning, one settled tool call, the live activity
//! row, a collapsed group, a subagent roster, and a diff body.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, SharedString, div, prelude::*, pulsating_between,
};

use super::super::{
    format::format_thought,
    tool_row::{ToolRow, ToolRowElement},
};
use super::render::{RowContext, body_region, chevron, header, separator, show_hint};
use super::{DiffRow, ReasoningRow, SubagentRow, TranscriptRowId, WorkGroupRow, WorkLiveRow};
use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// Reasoning: one 30 px line collapsed, the buffered summary expanded.
///
/// Before the first reasoning or text token there is no duration yet and the row is the
/// shimmer — `thinking` — which is the affordance that makes the app feel alive before
/// anything streams. It auto-collapses when the turn's terminal answer arrives, which is the
/// owner's decision, not the row's.
pub(super) fn reasoning(
    row: &ReasoningRow,
    id: &TranscriptRowId,
    ctx: &RowContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let streaming = row.duration_ms.is_none();
    let label = row
        .duration_ms
        .map_or_else(|| SharedString::new_static("thinking"), format_thought);
    let expandable = !row.text.is_empty();
    let line = header(
        ("reasoning", ctx.index),
        expandable,
        ctx.toggle_for(id),
        theme,
    )
    .min_h(theme.metrics.row_h)
    .child(Icon::Brain.el().size(IconSize::Small).tone(Tone::Muted))
    .child(div().flex().flex_1().min_w_0().child(shimmer(
        Text::ui(label).muted().into_any_element(),
        ("reasoning-shimmer", ctx.index),
        streaming && ctx.visible,
        theme,
    )))
    .child(show_hint(row.expanded, expandable, "show"));

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(line)
        .children((row.expanded && expandable).then(|| {
            body_region(
                ("reasoning-body", ctx.index),
                theme,
                Text::data_small(row.text.clone()).muted(),
            )
        }))
        .into_any_element()
}

/// One settled tool call, drawn by [`ToolRowElement`].
pub(super) fn work(row: &ToolRow, id: &TranscriptRowId, ctx: RowContext) -> AnyElement {
    let toggle = ctx.toggle_for(id);
    let mut element = ToolRowElement::new(row.clone(), ctx.index).focused(ctx.focused);
    if let Some(body) = ctx.body {
        element = element.body(body);
    }
    if let Some(toggle) = toggle {
        element = element.on_toggle(move |window, cx| toggle(window, cx));
    }
    element.into_any_element()
}

/// The single in-place live row, present-tense by rule.
///
/// Its height is pinned at `row_h` from the moment the turn starts — including the empty
/// startup window — so the handoff *working → thinking → running* keeps the same height.
pub(super) fn work_live(row: &WorkLiveRow, index: usize, visible: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .min_h(theme.metrics.row_h)
        .flex()
        .items_center()
        .gap(theme.space.md)
        .child(
            div()
                .flex_none()
                .child(row.icon.el().size(IconSize::Large).tone(Tone::Secondary)),
        )
        .child(div().flex().flex_1().min_w_0().child(shimmer(
            Text::data(row.label.clone()).ellipsize().into_any_element(),
            ("work-live", index),
            row.shimmer && visible,
            theme,
        )))
        .into_any_element()
}

/// A run of adjacent settled tool rows, collapsed to one generated summary.
///
/// **Group failure is neutral by default**: only when the *latest* entry in the group failed
/// does the header carry the mark. The failure always reaches the accessibility label.
pub(super) fn work_group(
    row: &WorkGroupRow,
    id: &TranscriptRowId,
    ctx: &RowContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    header(("work-group", ctx.index), true, ctx.toggle_for(id), theme)
        .child(
            row.icon
                .el()
                .size(IconSize::Large)
                .tone(Tone::Muted)
                .into_any_element(),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(Text::data(row.summary.clone()).muted().ellipsize()),
        )
        .children(row.latest_failed.then(|| {
            Icon::CircleX
                .el()
                .size(IconSize::Small)
                .tone(Tone::Danger)
                .opacity(theme.metrics.dimmed_opacity)
        }))
        .child(chevron(row.expanded, true))
        .into_any_element()
}

/// A subagent spawn and its roster.
///
/// Never grouped, and never folded while its work is live: workflows outlive their launching
/// turn, and folding the affordance when the turn settles makes a still-running fleet
/// invisible.
pub(super) fn subagent(
    row: &SubagentRow,
    id: &TranscriptRowId,
    ctx: RowContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let index = ctx.index;
    let line = header(("subagent", index), true, ctx.toggle_for(id), theme)
        .child(Icon::Bot.el().size(IconSize::Large).tone(Tone::Secondary))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(Text::data(row.summary.clone()).ellipsize()),
        )
        .children(
            row.status
                .clone()
                .map(|status| Text::hint(status).faint().flex_none()),
        )
        .children(row.tokens.clone().map(|_| separator()))
        .children(
            row.tokens
                .clone()
                .map(|tokens| Text::hint(tokens).faint().flex_none()),
        )
        .child(show_hint(row.expanded, true, "agents"));

    // The child region is the panel: Fleet does not build a separate agents pane, so nesting
    // in the transcript is where a running fleet is read.
    let children = (row.expanded && !row.children.is_empty()).then(|| {
        div()
            .ml(theme.space.lg)
            .pl(theme.space.md)
            .border_l(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .flex()
            .flex_col()
            .children(row.children.iter().map(|line| {
                div()
                    .min_h(theme.metrics.row_h)
                    .flex()
                    .items_center()
                    .child(Text::data_small(line.clone()).muted().ellipsize())
            }))
    });

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(line)
        .children(children)
        .children(ctx.body)
        .into_any_element()
}

/// The diff body of an expanded edit row, measured as its own row.
///
/// The owner hands in a `fleet_lazygit::diff_view::DiffView` element; the unified text is the
/// fallback for an owner that has not built one yet, so the row is never blank.
pub(super) fn diff(row: &DiffRow, ctx: RowContext, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let body = ctx.body.unwrap_or_else(|| {
        Text::data_small(row.unified.clone())
            .muted()
            .into_any_element()
    });
    body_region(("diff", ctx.index), theme, body)
        .w_full()
        .into_any_element()
}

/// Wrap an element in the traveling-highlight shimmer, or return it untouched.
///
/// **Animation is viewport-gated**: `running` is false for a row outside the list's viewport,
/// so an off-screen live row costs no frames. The duration is `motion.spinner`, the same beat
/// as every other in-flight affordance in Fleet.
fn shimmer(
    element: AnyElement,
    id: (&'static str, usize),
    running: bool,
    theme: &Theme,
) -> AnyElement {
    // Wrapped either way, so turning the animation off does not change the element tree.
    let wrapper = div().child(element);
    if !running {
        return wrapper.into_any_element();
    }
    let from = theme.metrics.dimmed_opacity;
    wrapper
        .with_animation(
            id,
            Animation::new(Duration::from_millis(theme.motion.spinner))
                .repeat()
                .with_easing(pulsating_between(from, 1.0)),
            |element, delta| element.opacity(delta),
        )
        .into_any_element()
}
