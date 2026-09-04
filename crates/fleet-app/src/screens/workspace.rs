//! The Workspace: one session's terminals, its header, tab strip and grid (UX-SPEC §3.6).
//!
//! **Placeholder.** The Workspace agent replaces this file wholesale; the constructor, the
//! render signature and the `track_focus` rule are frozen in `docs/APP-CONTRACTS.md`.
//!
//! Two behaviours this screen owns and the shell cannot do for it:
//!
//! * **Keys to the PTY.** In `Workspace > Terminal` only `ctrl-s` is bound, so every other key
//!   reaches this element's `on_key_down`. Forward it to the daemon **only** while
//!   [`AppState::terminal_mode`](crate::state::AppState::terminal_mode) is
//!   [`TerminalMode::Terminal`](crate::state::TerminalMode::Terminal) — a key typed in `Prefix`
//!   or `Scroll` must never reach the PTY.
//! * **Keys are dropped, never buffered, while the daemon is gone** (§3.12 C):
//!   [`AppState::drops_terminal_keys`](crate::state::AppState::drops_terminal_keys) says when.
//!   The shell already paints the 55 % veil over whatever this returns.

use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{bridge::Bridge, state::AppState};

/// The Workspace screen.
pub struct WorkspaceScreen {
    _private: (),
}

impl WorkspaceScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self { _private: () }
    }

    /// Renders the Workspace into the frame's body.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        _bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let attached = state.read(cx).active_grid().is_some();
        let theme = cx.theme();
        div()
            .track_focus(focus)
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.colors.bg)
            .child(if attached {
                EmptyState::new("Terminal rendering is not wired yet.").action("^s s  hub")
            } else {
                EmptyState::new("attaching…").action("^s s  hub")
            })
            .into_any_element()
    }
}
