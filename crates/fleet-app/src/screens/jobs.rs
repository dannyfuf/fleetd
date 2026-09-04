//! The Jobs panel: the right-docked sheet of UX-SPEC §3.7.
//!
//! **Placeholder.** The Jobs agent replaces this file wholesale; the constructor, the render
//! signature and the `track_focus` rule are frozen in `docs/APP-CONTRACTS.md`.
//!
//! The panel is an **overlay**: the shell renders it into the frame's overlay layer with the
//! `Jobs` key context, so the list behind stays fully visible and readable (§3.7) while the
//! keyboard belongs to the panel. Closing it restores the exact prior focus, which the shell
//! does for free because the pane, row and mode all live in [`AppState`].

use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{bridge::Bridge, state::AppState};

/// The jobs panel.
pub struct JobsPanel {
    _private: (),
}

impl JobsPanel {
    /// Builds the panel. Called once, while the shell is being built.
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self { _private: () }
    }

    /// Renders the panel into the frame's overlay layer.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        _bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let jobs = state
            .read(cx)
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.jobs.len());
        let theme = cx.theme();
        div()
            .track_focus(focus)
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(theme.metrics.sheet_w)
            .bg(theme.colors.elevated)
            .border_l(gpui::px(1.0))
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(if jobs == 0 {
                        EmptyState::new("Nothing running.").action(
                            "Jobs and sessions live in fleetd, so they survive closing this window.",
                        )
                    } else {
                        EmptyState::new(format!("{jobs} jobs")).action("J  close")
                    }),
            )
            .into_any_element()
    }
}
