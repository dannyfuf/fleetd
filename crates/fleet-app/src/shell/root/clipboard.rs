//! Terminal-program clipboard writes and their foreground policy.

use crate::state::AppState;
use fleet_core::ids::TerminalId;
use fleet_ui_kit::Icon;
use gpui::{App, ClipboardItem, Entity};
use std::time::Instant;

/// Applies one terminal-program clipboard write when its terminal owns the visible terminal
/// surface. Dialogs and application activation intentionally do not participate in this policy:
/// the event may arrive after an SSH round trip or after the user switched applications.
pub(super) fn handle(
    state: &Entity<AppState>,
    terminal: TerminalId,
    text: String,
    cx: &mut App,
) -> bool {
    if text.len() > fleet_proto::TERMINAL_CLIPBOARD_MAX_BYTES
        || active_terminal(state.read(cx)) != Some(terminal)
    {
        return false;
    }

    cx.write_to_clipboard(ClipboardItem::new_string(text));
    state.update(cx, |state, cx| {
        state.toast_short("copied", Icon::ClipboardCheck, Instant::now());
        cx.notify();
    });
    true
}

/// The terminal surface a programmatic clipboard write may currently speak for.
///
/// An open popup is authoritative even while its daemon session is still being ensured; in that
/// gap there is no eligible fallback. Otherwise only a process-backed Workspace tab is eligible.
fn active_terminal(state: &AppState) -> Option<TerminalId> {
    if state.agent_popup.is_some() {
        return state
            .agent_popup_session()
            .and_then(|session| session.terminals.first())
            .filter(|terminal| !terminal.is_native())
            .map(|terminal| terminal.id);
    }
    if state.active_tab_is_fleet_drawn() {
        return None;
    }
    state
        .active_terminal_record()
        .filter(|terminal| !terminal.is_native())
        .map(|terminal| terminal.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentPopupTransition, Screen};
    use fleet_core::{
        config::Agent,
        sessions::{
            Session, SessionKind, Terminal, TerminalKind, TerminalStatus, agent_session_id,
        },
    };
    use fleet_proto::snapshot::{DaemonInfo, Snapshot};
    use gpui::AppContext;

    fn terminal(id: u64) -> Terminal {
        Terminal {
            id: TerminalId(id),
            name: format!("terminal-{id}"),
            command: "sh".to_owned(),
            cwd: "/tmp".to_owned(),
            shell_pid: None,
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            agent_attention: None,
            kind: TerminalKind::Pty,
        }
    }

    fn session(id: &str, kind: SessionKind, terminals: &[u64], active: u64) -> Session {
        Session {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            host: None,
            kind,
            cwd: "/tmp".to_owned(),
            terminals: terminals.iter().copied().map(terminal).collect(),
            active_terminal: Some(TerminalId(active)),
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    fn snapshot(sessions: Vec<Session>) -> Snapshot {
        Snapshot {
            boards: Vec::new(),
            generated_at: String::new(),
            revision: None,
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions,
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: DaemonInfo {
                version: "test".to_owned(),
                pid: 1,
                started_at: String::new(),
                home: "/tmp/fleet".to_owned(),
            },
        }
    }

    fn workspace_state() -> AppState {
        let workspace = session(
            "fleet/workspace",
            SessionKind::Worktree(
                "fleet/app#clipboard"
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
            ),
            &[1, 3],
            1,
        );
        let mut state = AppState::new("/tmp/fleet-clipboard", Instant::now());
        state.screen = Screen::Workspace {
            session: workspace.id.clone(),
        };
        state.snapshot = Some(snapshot(vec![workspace]));
        state
    }

    fn clipboard_text(cx: &mut gpui::TestAppContext) -> Option<String> {
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }

    #[gpui::test]
    fn workspace_active_terminal_write_is_accepted(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| workspace_state());

        let accepted = cx.update(|cx| handle(&state, TerminalId(1), "a b".to_owned(), cx));

        assert!(accepted);
        assert_eq!(clipboard_text(cx).as_deref(), Some("a b"));
        cx.update(|cx| {
            let state = state.read(cx);
            assert_eq!(state.toasts.len(), 1);
            assert_eq!(state.toasts[0].toast.text.as_ref(), "copied");
            assert_eq!(state.toasts[0].toast.icon, Some(Icon::ClipboardCheck));
        });
    }

    #[gpui::test]
    fn open_popup_terminal_takes_precedence(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = workspace_state();
            let popup_id =
                agent_session_id(Agent::Claude).unwrap_or_else(|error| panic!("{error}"));
            let popup = session(
                popup_id.as_str(),
                SessionKind::Agent(Agent::Claude),
                &[2],
                2,
            );
            state
                .snapshot
                .as_mut()
                .unwrap_or_else(|| panic!("workspace snapshot is installed"))
                .sessions
                .push(popup);
            assert_eq!(
                state.toggle_agent_popup(Agent::Claude, None),
                AgentPopupTransition::Opened
            );
            state
        });

        assert!(cx.update(|cx| handle(&state, TerminalId(2), "popup".to_owned(), cx)));
        assert!(!cx.update(|cx| handle(&state, TerminalId(1), "workspace".to_owned(), cx)));
        assert_eq!(clipboard_text(cx).as_deref(), Some("popup"));
        cx.update(|cx| assert_eq!(state.read(cx).toasts.len(), 1));
    }

    #[gpui::test]
    fn background_terminal_write_is_rejected_without_a_toast(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| workspace_state());
        cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("original".to_owned())));

        let accepted = cx.update(|cx| handle(&state, TerminalId(3), "background".to_owned(), cx));

        assert!(!accepted);
        assert_eq!(clipboard_text(cx).as_deref(), Some("original"));
        cx.update(|cx| assert!(state.read(cx).toasts.is_empty()));
    }

    #[gpui::test]
    fn oversized_write_is_rejected_atomically_without_a_toast(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| workspace_state());
        cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("original".to_owned())));
        let oversized = "x".repeat(fleet_proto::TERMINAL_CLIPBOARD_MAX_BYTES + 1);

        let accepted = cx.update(|cx| handle(&state, TerminalId(1), oversized, cx));

        assert!(!accepted);
        assert_eq!(clipboard_text(cx).as_deref(), Some("original"));
        cx.update(|cx| assert!(state.read(cx).toasts.is_empty()));
    }
}
