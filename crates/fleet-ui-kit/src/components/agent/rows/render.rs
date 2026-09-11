//! Turning one [`TranscriptRow`] into elements.
//!
//! Every function here composes prepared values: the projection has already parsed the
//! Markdown, generated the group summary, truncated the previews and decided what is expanded.
//! Nothing in this module parses, highlights, measures or notifies — `render` prepares nothing
//! (`docs/APP-CONTRACTS.md`).

use std::rc::Rc;

use gpui::{AnyElement, App, Pixels, SharedString, Stateful, Window, div, prelude::*, px};

use super::{TranscriptRhythm, TranscriptRow, TranscriptRowId, TranscriptRowKind};
use crate::{
    components::KeyHint,
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

use super::super::metrics::{AGENT_BODY_MAX_H, AGENT_CONTENT_W};

/// What an expand toggle reports back to the transcript's owner.
pub(crate) type ToggleFn = Rc<dyn Fn(SharedString, &mut Window, &mut App) + 'static>;

/// One row's own toggle, with its key already bound in.
pub(super) type RowToggleFn = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// Everything one row needs from the list for one frame, borrowed rather than recomputed.
pub(crate) struct RowContext {
    /// The row's index, which is what every [`gpui::ElementId`] in the row is keyed by.
    pub index: usize,
    /// Whether the row carries the focus ring.
    pub focused: bool,
    /// Whether the row is inside the list's viewport. An animation runs only when it is.
    pub visible: bool,
    /// The working row's label, recomputed by the list's 1 Hz tick rather than in render.
    pub working_label: Option<SharedString>,
    /// An owner-supplied body for a work, subagent or diff row — a `DiffView`, an image, an
    /// MCP payload.
    pub body: Option<AnyElement>,
    /// What a click on an expandable row's header reports.
    pub toggle: Option<ToggleFn>,
}

impl RowContext {
    /// The click handler for a row whose header toggles, or `None` when nobody is listening.
    pub(super) fn toggle_for(&self, id: &TranscriptRowId) -> Option<RowToggleFn> {
        let toggle = self.toggle.clone()?;
        let key = id.key();
        Some(Rc::new(move |window: &mut Window, cx: &mut App| {
            toggle(key.clone(), window, cx);
        }))
    }
}

/// Draw one row, including the air it leaves under itself.
pub(crate) fn row_element(row: &TranscriptRow, ctx: RowContext, cx: &mut App) -> AnyElement {
    let rhythm = row.rhythm();
    let index = ctx.index;
    let visible = ctx.visible;
    let content = match &row.kind {
        TranscriptRowKind::User(user) => super::message::user(user, &row.id, &ctx, cx),
        TranscriptRowKind::Assistant(assistant) => super::message::assistant(assistant, index, cx),
        TranscriptRowKind::AssistantMeta(meta) => super::message::assistant_meta(meta, cx),
        TranscriptRowKind::Plan(plan) => super::message::plan(plan, &row.id, &ctx, cx),
        TranscriptRowKind::Gate(gate) => super::message::gate(gate, &row.id, &ctx, cx),
        TranscriptRowKind::Reasoning(reasoning) => {
            super::work::reasoning(reasoning, &row.id, &ctx, cx)
        }
        TranscriptRowKind::WorkLive(live) => super::work::work_live(live, index, visible, cx),
        TranscriptRowKind::WorkGroup(group) => super::work::work_group(group, &row.id, &ctx, cx),
        TranscriptRowKind::TurnFold(fold) => super::chrome::turn_fold(fold, &row.id, &ctx, cx),
        TranscriptRowKind::TurnFooter(footer) => super::chrome::turn_footer(footer, cx),
        TranscriptRowKind::Checkpoint(checkpoint) => super::chrome::checkpoint(checkpoint, cx),
        TranscriptRowKind::Notice(notice) => super::chrome::notice(notice, cx),
        TranscriptRowKind::Error(error) => super::chrome::error(error, index, cx),
        TranscriptRowKind::Empty(empty) => super::chrome::empty(empty, cx),
        TranscriptRowKind::Working(working) => {
            super::chrome::working(working, ctx.working_label.as_ref(), index, visible, cx)
        }
        // These three own the context's body slot, so they take it rather than borrowing.
        TranscriptRowKind::Work(work) => super::work::work(work, &row.id, ctx),
        TranscriptRowKind::Subagent(subagent) => super::work::subagent(subagent, &row.id, ctx, cx),
        TranscriptRowKind::Diff(diff) => super::work::diff(diff, ctx, cx),
    };

    let theme = cx.theme();
    div()
        .w_full()
        .flex()
        .pb(pad(rhythm, theme))
        .child(div().w_full().max_w(AGENT_CONTENT_W).child(content))
        .into_any_element()
}

/// The 4 px-scale value one rhythm resolves to.
fn pad(rhythm: TranscriptRhythm, theme: &Theme) -> Pixels {
    match rhythm {
        TranscriptRhythm::Detail => theme.space.xs,
        TranscriptRhythm::Attached => px(0.0),
        TranscriptRhythm::Fold => theme.space.xs + theme.space.xxs,
        TranscriptRhythm::Work => theme.space.sm,
        TranscriptRhythm::Block => theme.space.lg,
    }
}

/// The header line of a collapsible row: 30 px tall, and clickable only when it can expand.
///
/// The click belongs to this line only, so a click inside the body it opens never folds it.
pub(super) fn header(
    key: (&'static str, usize),
    expandable: bool,
    on_toggle: Option<RowToggleFn>,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    div()
        .id(key)
        .h(theme.metrics.row_h)
        .w_full()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .when_some(on_toggle.filter(|_| expandable), |el, toggle| {
            el.cursor_pointer()
                .on_click(move |_, window, cx| toggle(window, cx))
        })
}

/// The disclosure chevron. `invisible`, never absent, when a row cannot expand — alignment
/// never shifts when a row gains a body.
pub(super) fn chevron(expanded: bool, expandable: bool) -> impl IntoElement {
    div()
        .flex_none()
        .when(!expandable, |el| el.invisible())
        .child(
            if expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            }
            .el()
            .size(IconSize::Small)
            .tone(Tone::Muted),
        )
}

/// The `[⏎] <label>` hint, `invisible` when the row cannot expand.
pub(super) fn show_hint(expanded: bool, expandable: bool, label: &'static str) -> impl IntoElement {
    div()
        .flex_none()
        .when(!expandable, |el| el.invisible())
        .child(KeyHint::labeled("⏎", if expanded { "hide" } else { label }))
}

/// A body region: indented, capped, and scrolling inside its own row.
pub(super) fn body_region(
    key: (&'static str, usize),
    theme: &Theme,
    child: impl IntoElement,
) -> Stateful<gpui::Div> {
    div()
        .id(key)
        .ml(theme.space.lg)
        .max_h(AGENT_BODY_MAX_H)
        .overflow_y_scroll()
        .child(child)
}

/// A full-width hairline, for the two halves of a checkpoint separator.
pub(super) fn hairline(theme: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .h(theme.metrics.hairline)
        .bg(theme.colors.border)
}

/// The muted `·` between two metadata segments.
pub(super) fn separator() -> impl IntoElement {
    Text::hint(SharedString::new_static("·"))
        .faint()
        .flex_none()
}
