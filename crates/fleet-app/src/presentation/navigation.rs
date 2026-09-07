use fleet_core::ids::SessionId;

use crate::state::{AppState, Screen, TerminalMode};

/// Applies every client-side invariant of entering an existing session.
pub fn enter_session(state: &mut AppState, session: SessionId) {
    state.leave_prefix();
    state.touch_session(session.clone());
    state.screen = Screen::Workspace { session };
    state.terminal_mode = TerminalMode::Terminal;
    state.sync_terminal_mode();
}
