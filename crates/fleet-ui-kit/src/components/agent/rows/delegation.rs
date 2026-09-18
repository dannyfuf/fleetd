//! Native delegation rows: one durable child link and the result it reports back.

use gpui::{AnyElement, App, SharedString, div, prelude::*};

use super::render::{RowContext, chevron, header, separator};
use super::{DelegationResultCard, DelegationRow, DelegationRowStatus, TranscriptRowId};
use crate::{
    components::{Spinner, StatusDot, markdown},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

use super::super::metrics::AGENT_PREVIEW_MAX_H;

/// One native child delegation.
pub(super) fn delegation(row: &DelegationRow, ctx: &RowContext, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let line = header(("delegation", ctx.index), false, None, theme)
        .child(Text::ui(SharedString::new_static("↳")).muted().flex_none())
        .child(Icon::Bot.el().size(IconSize::Large).tone(Tone::Secondary))
        .child(Text::data_small(row.provider.clone()).muted().flex_none())
        .child(separator())
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .child(Text::data(row.title.clone()).ellipsize()),
        )
        .child(status_mark(row.status, ctx.index))
        .child(
            Text::hint(row.status.word())
                .tone(status_tone(row.status))
                .flex_none(),
        )
        .child(Text::hint(row.elapsed.clone()).faint().flex_none())
        .child(
            Text::hint(row.hint.clone())
                .tone(Tone::Secondary)
                .flex_none(),
        );

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(line)
        .children(row.headline.clone().map(|headline| {
            div()
                .ml(theme.space.xl)
                .min_w_0()
                .overflow_hidden()
                .child(Text::data_small(headline).muted().ellipsize())
        }))
        .into_any_element()
}

/// A native child's result delivered into its caller's transcript.
pub(super) fn delegation_result(
    row: &DelegationResultCard,
    id: &TranscriptRowId,
    ctx: &RowContext,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let line = header(
        ("delegation-result", ctx.index),
        row.collapsible,
        ctx.toggle_for(id),
        theme,
    )
    .child(Text::ui(SharedString::new_static("↳")).muted().flex_none())
    .child(Icon::Bot.el().size(IconSize::Large).tone(Tone::Secondary))
    .child(
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .child(Text::data(row.header.clone()).ellipsize()),
    )
    .child(chevron(row.expanded, row.collapsible))
    .child(
        Text::hint(row.hint.clone())
            .tone(Tone::Secondary)
            .flex_none(),
    );

    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(theme.radii.md)
        .bg(theme.colors.surface)
        .px(theme.space.md)
        .pb(theme.space.sm)
        .child(line)
        .child(
            div()
                .w_full()
                .when(row.collapsible && !row.expanded, |el| {
                    el.max_h(AGENT_PREVIEW_MAX_H).overflow_hidden()
                })
                .child(markdown(&row.body, cx)),
        )
        .into_any_element()
}

fn status_mark(status: DelegationRowStatus, index: usize) -> AnyElement {
    match status {
        DelegationRowStatus::Starting | DelegationRowStatus::Working => {
            Spinner::new(("delegation-status", index))
                .size(IconSize::Small)
                .tone(Tone::Secondary)
                .into_any_element()
        }
        DelegationRowStatus::Blocked => StatusDot::small(Tone::Warning).into_any_element(),
        DelegationRowStatus::Done => Icon::CircleCheck
            .el()
            .size(IconSize::Small)
            .tone(Tone::Success)
            .into_any_element(),
        DelegationRowStatus::Incomplete
        | DelegationRowStatus::Failed
        | DelegationRowStatus::Cancelled => Icon::CircleX
            .el()
            .size(IconSize::Small)
            .tone(Tone::Danger)
            .into_any_element(),
    }
}

const fn status_tone(status: DelegationRowStatus) -> Tone {
    match status {
        DelegationRowStatus::Starting | DelegationRowStatus::Working => Tone::Secondary,
        DelegationRowStatus::Blocked => Tone::Warning,
        DelegationRowStatus::Done => Tone::Success,
        DelegationRowStatus::Incomplete
        | DelegationRowStatus::Failed
        | DelegationRowStatus::Cancelled => Tone::Danger,
    }
}
