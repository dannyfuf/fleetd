//! Session terminal order is the numeric keymap order; display labels retain rename precedence.

use crate::presentation::{KeepAliveStyle, keep_alive_icon, terminal_label};
use gpui::SharedString;
use std::collections::{HashMap, HashSet};

use fleet_core::{
    ids::TerminalId,
    sessions::{AgentActivity, Session, TerminalStatus, WorktreeStatus},
};
use fleet_ui_kit::{TerminalAgentState, TerminalTab, TerminalTabKind};

#[derive(Default)]
pub(crate) struct TabLabels(HashMap<TerminalId, SharedString>);

impl TabLabels {
    /// The tabs of a session, in `Session.terminals` order.
    ///
    /// `active` is the terminal the strip underlines. A tab is marked with the amber activity dot
    /// only when it is **not** active, because output in the tab you are looking at is not news.
    /// `renamed` is [`crate::state::AppState::renamed_terminals`].
    #[must_use]
    pub(crate) fn tabs(
        &mut self,
        session: &Session,
        status: Option<&WorktreeStatus>,
        active: Option<TerminalId>,
        renamed: &HashSet<TerminalId>,
    ) -> Vec<TerminalTab> {
        self.0
            .retain(|id, _| session.terminals.iter().any(|terminal| terminal.id == *id));
        session
            .terminals
            .iter()
            .enumerate()
            .map(|(position, terminal)| {
                let label = terminal_label(terminal, renamed.contains(&terminal.id));
                let shared = self.0.entry(terminal.id).or_default();
                if shared.as_ref() != label {
                    *shared = SharedString::new(label);
                }
                let mut tab = TerminalTab::new(position + 1, shared.clone())
                    .activity(terminal.has_unseen_output && active != Some(terminal.id))
                    .starting(terminal.status == TerminalStatus::Starting)
                    .kind(if terminal.is_native() {
                        TerminalTabKind::Native
                    } else {
                        TerminalTabKind::Pty
                    });
                if let Some(label) = terminal.keep_alive.first() {
                    tab = tab.keep_alive(keep_alive_icon(label, KeepAliveStyle::Terminal));
                }
                if let Some(window) = status.and_then(|status| {
                    status
                        .windows
                        .iter()
                        .find(|window| window.index == position as u32 && window.agent.is_some())
                }) {
                    tab = match window.agent_activity {
                        AgentActivity::Working => tab.agent_status(TerminalAgentState::Working),
                        AgentActivity::Idle => tab.agent_status(TerminalAgentState::Finished),
                        AgentActivity::Unknown => tab,
                    };
                }
                if let TerminalStatus::Exited { code } = terminal.status {
                    tab = tab.exited(code);
                }
                tab
            })
            .collect()
    }
}

/// The position of a terminal in the strip, which is what `ctrl-s 1`–`9` counts.
#[must_use]
pub(crate) fn position_of(session: &Session, terminal: TerminalId) -> Option<usize> {
    session
        .terminals
        .iter()
        .position(|candidate| candidate.id == terminal)
}

/// The terminal `ctrl-s <n>` selects, or `None` when the session has fewer tabs.
#[must_use]
pub(crate) fn terminal_at(session: &Session, position: usize) -> Option<TerminalId> {
    session.terminals.get(position).map(|terminal| terminal.id)
}

/// The terminal `ctrl-s h` / `ctrl-s l` moves to, wrapping at both ends.
///
/// Wrapping is what makes `ctrl-s l` a one-key cycle through a three-tab session, which is the
/// default layout; stopping at the end would cost a second keystroke on every lap.
#[must_use]
pub(crate) fn neighbour(
    session: &Session,
    active: Option<TerminalId>,
    delta: isize,
) -> Option<TerminalId> {
    let len = session.terminals.len();
    if len == 0 {
        return None;
    }
    let current = active
        .and_then(|active| position_of(session, active))
        .unwrap_or(0);
    let len_isize = isize::try_from(len).unwrap_or(isize::MAX);
    let next = (isize::try_from(current).unwrap_or(0) + delta).rem_euclid(len_isize);
    terminal_at(session, usize::try_from(next).unwrap_or(0))
}

/// `base`, or `base2`, `base3`, … — the first name the session does not already use.
///
/// fleetd refuses a duplicate terminal name, so every caller that invents one goes through
/// here. A numbered suffix keeps the tab under the strip's 84 px minimum width.
#[must_use]
pub(crate) fn unique_terminal_name(session: &Session, base: &str) -> String {
    let taken = |candidate: &str| {
        session
            .terminals
            .iter()
            .any(|terminal| terminal.name == candidate)
    };
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|index| format!("{base}{index}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        config::Agent,
        ids::SessionId,
        sessions::{SessionKind, TerminalStatus, WorktreeWindowStatus},
    };

    use super::*;
    use fleet_core::sessions::Terminal;
    use fleet_ui_kit::Icon;

    fn terminal(id: u64, name: &str) -> Terminal {
        Terminal {
            id: TerminalId(id),
            name: name.to_owned(),
            command: String::new(),
            cwd: "/tmp".to_owned(),
            shell_pid: None,
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            kind: fleet_core::sessions::TerminalKind::Pty,
        }
    }

    fn session(names: &[&str]) -> Session {
        Session {
            id: SessionId::try_from("payroll/feat-x").unwrap_or_else(|error| panic!("{error}")),
            kind: SessionKind::Agent(Agent::Claude),
            cwd: "/tmp".to_owned(),
            terminals: names
                .iter()
                .enumerate()
                .map(|(index, name)| terminal(index as u64 + 1, name))
                .collect(),
            active_terminal: None,
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    #[test]
    fn an_osc_title_names_the_tab_until_the_user_renames_it() {
        let mut terminal = terminal(1, "nvim");
        assert_eq!(terminal_label(&terminal, false), "nvim");

        // §3.6: `FrameUpdate.title` names the tab.
        terminal.title = Some("README.md".to_owned());
        assert_eq!(terminal_label(&terminal, false), "README.md");

        // A cleared or blank title falls back to the configured window name.
        terminal.title = Some("   ".to_owned());
        assert_eq!(terminal_label(&terminal, false), "nvim");
        terminal.title = Some("README.md".to_owned());

        // After `ctrl-s ,` the user's name wins, whatever the program sets.
        let renamed: HashSet<TerminalId> = [TerminalId(1)].into_iter().collect();
        terminal.name = "editor".to_owned();
        assert_eq!(
            terminal_label(&terminal, renamed.contains(&terminal.id)),
            "editor"
        );
    }

    #[test]
    fn a_native_tab_is_marked_but_keeps_its_number() {
        let mut session = session(&["nvim", "cc", "lg"]);
        session.terminals[2].kind = fleet_core::sessions::TerminalKind::Native;
        let tabs = TabLabels::default().tabs(&session, None, Some(TerminalId(3)), &HashSet::new());
        assert_eq!(
            tabs.iter().map(|tab| tab.kind).collect::<Vec<_>>(),
            vec![
                TerminalTabKind::Pty,
                TerminalTabKind::Pty,
                TerminalTabKind::Native
            ]
        );
        // The whole point of keeping a native tab in the session: `ctrl-s 3` still reaches it.
        assert_eq!(tabs[2].index, 3);
        assert_eq!(terminal_at(&session, 2), Some(TerminalId(3)));
    }

    #[test]
    fn tab_indexes_are_positions_not_identifiers() {
        let mut session = session(&["nvim", "cc", "lg"]);
        session.terminals[0].id = TerminalId(41);
        let tabs = TabLabels::default().tabs(&session, None, Some(TerminalId(41)), &HashSet::new());
        assert_eq!(
            tabs.iter().map(|tab| tab.index).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn the_active_tab_never_shows_the_activity_dot() {
        let mut session = session(&["nvim", "cc"]);
        session.terminals[0].has_unseen_output = true;
        session.terminals[1].has_unseen_output = true;
        let tabs = TabLabels::default().tabs(&session, None, Some(TerminalId(1)), &HashSet::new());
        assert!(!tabs[0].activity);
        assert!(tabs[1].activity);
    }

    #[test]
    fn an_exited_terminal_carries_its_code() {
        let mut session = session(&["test"]);
        session.terminals[0].status = TerminalStatus::Exited { code: Some(1) };
        assert_eq!(
            TabLabels::default().tabs(&session, None, None, &HashSet::new())[0].exited,
            Some(Some(1))
        );
    }

    #[test]
    fn keep_alive_labels_pick_their_glyph() {
        assert_eq!(
            keep_alive_icon("claude", KeepAliveStyle::Terminal),
            Icon::Bot
        );
        assert_eq!(
            keep_alive_icon("OpenCode", KeepAliveStyle::Terminal),
            Icon::Sparkles
        );
        assert_eq!(
            keep_alive_icon(":3000", KeepAliveStyle::Terminal),
            Icon::Server
        );
        assert_eq!(
            keep_alive_icon("nvim", KeepAliveStyle::Terminal),
            Icon::FilePen
        );
        assert_eq!(
            keep_alive_icon("something-else", KeepAliveStyle::Terminal),
            Icon::Zap
        );
    }

    #[test]
    fn the_first_keep_alive_label_wins() {
        let mut session = session(&["cc"]);
        session.terminals[0].keep_alive = vec!["claude".to_owned(), ":3000".to_owned()];
        assert_eq!(
            TabLabels::default().tabs(&session, None, None, &HashSet::new())[0].keep_alive,
            Some(Icon::Bot)
        );
    }

    #[test]
    fn recognized_agents_carry_their_per_terminal_activity_glyph() {
        let session = session(&["cc", "shell"]);
        let status = WorktreeStatus {
            worktree_id: "buk/payroll#feat"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            session: fleet_core::sessions::SessionState::Detached,
            windows: vec![WorktreeWindowStatus {
                index: 0,
                name: "cc".to_owned(),
                command: "claude".to_owned(),
                keep_alive: vec!["claude".to_owned()],
                agent: Some("claude".to_owned()),
                agent_activity: AgentActivity::Working,
                agent_activity_changed_at: Some("2026-09-05T12:00:00Z".to_owned()),
            }],
            running: vec!["claude".to_owned()],
            agent_activity: AgentActivity::Working,
            agent_activity_changed_at: Some("2026-09-05T12:00:00Z".to_owned()),
        };

        let tabs = TabLabels::default().tabs(&session, Some(&status), None, &HashSet::new());
        assert_eq!(tabs[0].agent_status, Some(TerminalAgentState::Working));
        assert_eq!(tabs[1].agent_status, None);
    }

    #[test]
    fn tab_movement_wraps_at_both_ends() {
        let session = session(&["nvim", "cc", "lg"]);
        assert_eq!(
            neighbour(&session, Some(TerminalId(3)), 1),
            Some(TerminalId(1))
        );
        assert_eq!(
            neighbour(&session, Some(TerminalId(1)), -1),
            Some(TerminalId(3))
        );
        assert_eq!(neighbour(&session, None, 1), Some(TerminalId(2)));
    }

    #[test]
    fn tab_movement_on_an_empty_session_is_a_no_op() {
        assert_eq!(neighbour(&session(&[]), None, 1), None);
    }

    #[test]
    fn numeric_selection_is_bounded_by_the_strip() {
        let session = session(&["nvim", "cc"]);
        assert_eq!(terminal_at(&session, 0), Some(TerminalId(1)));
        assert_eq!(terminal_at(&session, 5), None);
    }

    #[test]
    fn new_tabs_avoid_the_names_already_on_the_strip() {
        assert_eq!(unique_terminal_name(&session(&["nvim"]), "sh"), "sh");
        assert_eq!(unique_terminal_name(&session(&["sh"]), "sh"), "sh2");
        assert_eq!(unique_terminal_name(&session(&["sh", "sh2"]), "sh"), "sh3");
    }
}
