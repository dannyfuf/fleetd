//! The Changes diff sheet: one file's diff against the worktree's base (UX-SPEC §3.6).
//!
//! A click on a file in the Workspace's Changes panel opens it. The diff itself is read by the
//! panel's git worker and lands in [`crate::state::ChangesPanel::diff`] as text; this module
//! turns that text into the embedded [`DiffView`] — the same inline diff the agent thread draws —
//! on the host's state observation, never in render.

use fleet_lazygit::diff_view::DiffView;
use fleet_ui_kit::{Sheet, SheetSide};
use gpui::{AnyElement, App, AppContext, Entity, FocusHandle, SharedString, Window};

use super::*;
use crate::{
    dialogs::{DialogHost, Dialogs, root},
    state::DiffBody,
};

/// The sheet's diff surface, keyed by the text it was built from.
#[derive(Default)]
pub(crate) struct ChangesDiffState {
    view: Option<(SharedString, Entity<DiffView>)>,
}

/// Builds or updates the diff surface once the worker's text has landed.
pub(crate) fn refresh(state: &Entity<AppState>, cx: &mut App) {
    let Some((label, text)) = state
        .read(cx)
        .changes
        .diff()
        .and_then(|diff| match &diff.body {
            DiffBody::Ready(text) => Some((diff.label.clone(), text.clone())),
            DiffBody::Loading | DiffBody::MissingBase | DiffBody::Failed(_) => None,
        })
    else {
        return;
    };
    let host = super::host::host_for(state, cx);
    let current = host
        .read(cx)
        .changes_diff
        .view
        .as_ref()
        .map(|(shown, view)| (shown == &text, view.clone()));
    match current {
        Some((true, _)) => {}
        Some((false, view)) => {
            view.update(cx, |view, cx| view.set_unified(text.clone(), cx));
            host.update(cx, |host, cx| {
                if let Some((shown, _)) = host.changes_diff.view.as_mut() {
                    *shown = text;
                }
                cx.notify();
            });
        }
        None => {
            let view = cx.new(|cx| {
                let mut view = DiffView::for_path(Some(label.clone()), text.clone(), cx);
                // A sheet is the whole diff's place: nothing folds behind "show all" here.
                view.set_expanded(true, cx);
                view
            });
            host.update(cx, |host, cx| {
                host.changes_diff.view = Some((text, view));
                cx.notify();
            });
        }
    }
}

/// The sheet, docked right over the dimmed Workspace.
pub(crate) fn render(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let diff = state.read(cx).changes.diff().cloned();
    let view = host
        .read(cx)
        .changes_diff
        .view
        .as_ref()
        .map(|(_, view)| view.clone());

    let (title, base) = diff.as_ref().map_or_else(
        || (SharedString::default(), SharedString::default()),
        |diff| (diff.label.clone(), format!("vs {}", diff.base).into()),
    );
    let header = div()
        .flex()
        .flex_col()
        .gap(theme.space.xxs)
        .px(theme.space.xl)
        .py(theme.space.lg)
        .min_w_0()
        .child(Text::section_title(title).ellipsize())
        .child(Text::hint(base));

    let message = |text: SharedString| {
        div()
            .px(theme.space.xl)
            .py(theme.space.lg)
            .child(Text::ui(text).muted())
            .into_any_element()
    };
    let body = match diff.map(|diff| diff.body) {
        Some(DiffBody::Ready(text)) if text.is_empty() => {
            message("This file no longer differs from the base.".into())
        }
        Some(DiffBody::Ready(_)) => match view {
            Some(view) => div()
                .id("changes-diff-body")
                .size_full()
                .overflow_y_scroll()
                .px(theme.space.md)
                .py(theme.space.sm)
                .child(view)
                .into_any_element(),
            None => message("Reading the diff\u{2026}".into()),
        },
        Some(DiffBody::Loading) | None => message("Reading the diff\u{2026}".into()),
        Some(DiffBody::MissingBase) => {
            message("The base is not a commit in this repository.".into())
        }
        Some(DiffBody::Failed(error)) => div()
            .px(theme.space.xl)
            .py(theme.space.lg)
            .child(Callout::new(Tone::Danger, Icon::TriangleAlert, error))
            .into_any_element(),
    };

    let sheet = Sheet::new(true)
        .side(SheetSide::Right)
        .scrim(true)
        .width(theme.metrics.sheet_w_detail)
        .dismiss_action(Dialogs::ChangesDiff.dismiss_action())
        .header(header)
        .body(body);

    root(focus).child(sheet).into_any_element()
}
