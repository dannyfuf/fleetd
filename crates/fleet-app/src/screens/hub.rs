//! The Hub: contexts, repos rail, worktrees or pull requests, detail panel (UX-SPEC §3.1–§3.5).
//!
//! **Placeholder.** The Hub agent replaces this file wholesale. Only three things are fixed,
//! and all three are documented in `docs/APP-CONTRACTS.md`: the constructor, the render
//! signature the shell calls, and the rule that the returned root element **must** call
//! `.track_focus(focus)` — that is what puts this screen's `on_action` listeners on the key
//! dispatch path, under the `Hub > <pane>` contexts the shell has already applied above it.

use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{bridge::Bridge, state::AppState};

/// The Hub screen.
pub struct HubScreen {
    _private: (),
}

impl HubScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self { _private: () }
    }

    /// Renders the Hub into the frame's body.
    ///
    /// `state` is the shared [`AppState`] entity — read it with `state.read(cx)` and mutate it
    /// with `state.update(cx, …)` from inside listeners. `bridge` is the only route to the
    /// daemon. The shell has already drawn the context bar, the status bar, the banner and the
    /// toasts.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        _bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let filtered = state.read(cx).filter.is_active();
        let theme = cx.theme();
        div()
            .track_focus(focus)
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.colors.bg)
            .child(if filtered {
                EmptyState::new("Nothing matches the filter.").action("esc  clear")
            } else {
                EmptyState::new("No worktrees yet.").action("n  create one")
            })
            .into_any_element()
    }
}
