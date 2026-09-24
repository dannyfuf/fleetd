//! The Workspace's Changes panel (UX-SPEC §3.6): what this worktree changed against its base.
//!
//! A 300 px column at the right of the Workspace body. From the top: the title and the base,
//! the changed files — a status letter, the path, the lines added and removed — then the commits
//! ahead of the base, and at the foot *Open in Lazygit*. A click on a file opens its diff.
//!
//! Everything drawn here was prepared when the reading landed ([`crate::state::ChangesModel`]);
//! this module lays it out and nothing else.

use std::rc::Rc;

use fleet_ui_kit::{
    ActiveTheme, Button, ButtonStyle, Callout, HarnessTargetExt as _, Icon, ListView, NavItem,
    Text, Tone,
};
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _,
};

use crate::state::{ChangesModel, ChangesReading, ReadingBody};

/// The panel's title.
const TITLE: &str = "Changes";
/// The commits section's label.
const COMMITS_AHEAD: &str = "Commits ahead";
/// The footer button.
const OPEN_IN_LAZYGIT: &str = "Open in Lazygit";

/// A pointer handler over a prepared row index.
pub(crate) type OnFile = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// A pointer handler for a button.
pub(crate) type OnPress = Rc<dyn Fn(&mut Window, &mut App)>;

/// Everything one frame of the panel needs.
pub(crate) struct ChangesProps<'a> {
    /// The reading the panel draws.
    pub reading: &'a ChangesReading,
    /// The file list's scroll position, owned by the Workspace.
    pub scroll: &'a UniformListScrollHandle,
    /// A click on file `ix`.
    pub on_file: OnFile,
    /// *Open in Lazygit*.
    pub on_lazygit: OnPress,
}

/// The panel.
pub(crate) fn render(props: ChangesProps<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let base: SharedString = format!("vs {}", props.reading.base).into();
    let header = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(theme.space.sm)
        .px(theme.space.lg)
        .pt(theme.space.lg)
        .pb(theme.space.sm)
        .child(Text::section_title(TITLE).flex_none())
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .child(Text::hint(base).ellipsize()),
        );

    let on_lazygit = Rc::clone(&props.on_lazygit);
    let footer = div()
        .flex()
        .flex_none()
        .px(theme.space.lg)
        .py(theme.space.md)
        .border_t(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(
            Button::new("changes-lazygit", OPEN_IN_LAZYGIT)
                .style(ButtonStyle::Secondary)
                .icon(Icon::GitBranch)
                .full_width()
                .on_click(move |_, window, cx| on_lazygit(window, cx))
                .harness_target("changes.lazygit"),
        );

    let body = match &props.reading.body {
        ReadingBody::Ready(model) => ready(model, &props, cx),
        ReadingBody::Loading => sentence("Reading changes\u{2026}", cx),
        ReadingBody::Remote => sentence(
            "This worktree is on another machine, and changes are read on this one. \
             Lazygit shows them there.",
            cx,
        ),
        ReadingBody::MissingBase => sentence(
            format!(
                "{} is not a commit in this repository, so there is nothing to compare.",
                props.reading.base
            ),
            cx,
        ),
        ReadingBody::Failed(error) => div()
            .px(theme.space.lg)
            .child(Callout::new(
                Tone::Danger,
                Icon::TriangleAlert,
                error.clone(),
            ))
            .into_any_element(),
    };

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(theme.metrics.changes_w)
        .h_full()
        .min_h_0()
        .bg(theme.colors.chrome)
        .border_l(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(header)
        .child(div().flex().flex_col().flex_1().min_h_0().child(body))
        .child(footer)
        .harness_target("changes.panel")
        .into_any_element()
}

/// The files, then the commits ahead.
fn ready(model: &Rc<ChangesModel>, props: &ChangesProps<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row_h = theme.metrics.row_h;
    let files = if model.files.is_empty() {
        sentence(format!("No changes against {}.", props.reading.base), cx)
    } else {
        let rows = Rc::clone(model);
        let on_file = Rc::clone(&props.on_file);
        let list = ListView::new("changes-files", model.files.len(), move |ix, _, _, _| {
            let Some(file) = rows.files.get(ix) else {
                return div().into_any_element();
            };
            let on_file = Rc::clone(&on_file);
            let mut item = NavItem::new(("changes-file", ix), file.label.clone())
                .leading(Text::data_small(file.letter.clone()).tone(file.tone));
            if let Some(added) = file.added.clone() {
                item = item.trailing(Text::data_small(added).tone(Tone::Success));
            }
            if let Some(removed) = file.removed.clone() {
                item = item.trailing(Text::data_small(removed).tone(Tone::Danger));
            }
            div()
                .id(("changes-file-press", ix))
                .cursor_pointer()
                .on_click(move |_, window, cx| on_file(ix, window, cx))
                .child(item)
                .harness_target_indexed("changes.file", ix)
                .into_any_element()
        })
        .row_height(row_h)
        .track_scroll(props.scroll);
        // As tall as its rows, and no taller than the panel leaves it: the commits sit right
        // under the last file, and a long list scrolls instead of pushing them away.
        let height = row_h * model.files.len() as f32;
        div()
            .flex_shrink(1.0)
            .min_h_0()
            .h(height)
            .px(theme.space.sm)
            .child(list)
            .into_any_element()
    };

    let count: SharedString = model.ahead.to_string().into();
    let more = u64::try_from(model.commits.len())
        .ok()
        .and_then(|shown| model.ahead.checked_sub(shown))
        .filter(|more| *more > 0);
    let commits = div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(theme.space.sm)
        .mx(theme.space.lg)
        .mt(theme.space.md)
        .pt(theme.space.md)
        .border_t(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(Text::sentence_label(COMMITS_AHEAD))
                .child(Text::data_small(count).muted()),
        )
        .when(model.commits.is_empty(), |el| {
            el.child(Text::hint(format!(
                "Nothing committed beyond {} yet.",
                props.reading.base
            )))
        })
        .children(model.commits.iter().map(|commit| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .min_w_0()
                .child(Text::data_small(commit.short.clone()).muted().flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(Text::ui(commit.subject.clone()).ellipsize()),
                )
        }))
        .children(more.map(|more| Text::hint(format!("and {more} more"))));

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(files)
        .child(commits)
        .into_any_element()
}

/// A one-sentence state in place of the rows.
fn sentence(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .px(theme.space.lg)
        .py(theme.space.sm)
        .child(Text::ui(text).muted())
        .into_any_element()
}
