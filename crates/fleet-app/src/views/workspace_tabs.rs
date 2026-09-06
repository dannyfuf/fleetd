//! Turning a session's terminals into the kit's [`TerminalTab`]s (UX-SPEC §3.6).
//!
//! The tab strip is the legend for `ctrl-s 1`–`9`, so the index a tab shows **is** the argument
//! of that binding: it is the terminal's position in `Session.terminals`, never its
//! [`TerminalId`]. Everything else on a tab is derived from the daemon's terminal record, and
//! its matching per-window status. All of it is pure — the strip is rebuilt from the snapshot on
//! every frame and this module is the only place that decides what it says.
//!
//! `Terminal.foreground_command` is deliberately **not** rendered: §3.6 lists it under
//! "intentionally omitted", and the kit's `TerminalTab` has no slot for it.

use std::collections::HashSet;

use fleet_core::{
    ids::TerminalId,
    sessions::{AgentActivity, Session, Terminal, TerminalStatus, WorktreeStatus},
};
use fleet_ui_kit::{Icon, StatusKind, TerminalTab, TerminalTabKind};

/// The glyph that names a keep-alive label's kind (§3.6: `bot` / `server` / `file-pen`).
///
/// Labels come from the sleep rules in `config.json`: the process rules produce the program's
/// own name and the listening-port rule produces `:<port>`, so the mapping is on the label and
/// not on a rule id the client never sees.
#[must_use]
pub fn keep_alive_icon(label: &str) -> Icon {
    if label.starts_with(':') {
        return Icon::Server;
    }
    match label.to_ascii_lowercase().as_str() {
        "claude" | "codex" | "aider" => Icon::Bot,
        "opencode" => Icon::Sparkles,
        "server" | "serve" | "dev" => Icon::Server,
        "nvim" | "vim" | "hx" | "helix" => Icon::FilePen,
        _ => Icon::Zap,
    }
}

/// The strongest keep-alive glyph of a terminal, or `None` when nothing keeps it alive.
///
/// "Strongest" is the first label in the daemon's order: the sleep matcher lists process
/// matches before the dynamic `:<port>` ones, so an agent wins over a port it happens to open.
#[must_use]
pub fn terminal_keep_alive_icon(terminal: &Terminal) -> Option<Icon> {
    terminal
        .keep_alive
        .first()
        .map(|label| keep_alive_icon(label))
}

/// What a tab is called: the program's OSC title, unless the user named it.
///
/// §3.6: `FrameUpdate.title` names the tab "until the terminal is explicitly renamed", so a
/// `vim README.md` tab says what it is holding instead of repeating the window name from
/// `config.json`. `renamed` is the set of terminals the user renamed with `ctrl-s ,` — an
/// explicit name always wins, and so does a title the program cleared.
#[must_use]
pub fn tab_label(terminal: &Terminal, renamed: &HashSet<TerminalId>) -> String {
    if renamed.contains(&terminal.id) {
        return terminal.name.clone();
    }
    terminal
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map_or_else(|| terminal.name.clone(), str::to_owned)
}

/// The tabs of a session, in `Session.terminals` order.
///
/// `active` is the terminal the strip underlines. A tab is marked with the amber activity dot
/// only when it is **not** active, because output in the tab you are looking at is not news.
/// `renamed` is [`crate::state::AppState::renamed_terminals`]; see [`tab_label`].
#[must_use]
pub fn tabs(
    session: &Session,
    status: Option<&WorktreeStatus>,
    active: Option<TerminalId>,
    renamed: &HashSet<TerminalId>,
) -> Vec<TerminalTab> {
    session
        .terminals
        .iter()
        .enumerate()
        .map(|(position, terminal)| {
            let mut tab = TerminalTab::new(position + 1, tab_label(terminal, renamed))
                .activity(terminal.has_unseen_output && active != Some(terminal.id))
                .starting(terminal.status == TerminalStatus::Starting)
                .kind(if terminal.is_native() {
                    TerminalTabKind::Native
                } else {
                    TerminalTabKind::Pty
                });
            if let Some(icon) = terminal_keep_alive_icon(terminal) {
                tab = tab.keep_alive(icon);
            }
            if let Some(window) = status.and_then(|status| {
                status
                    .windows
                    .iter()
                    .find(|window| window.index == position as u32 && window.agent.is_some())
            }) {
                tab = match window.agent_activity {
                    AgentActivity::Working => tab.agent_status(StatusKind::AgentWorking),
                    AgentActivity::Idle => tab.agent_status(StatusKind::AgentFinished),
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

/// The position of a terminal in the strip, which is what `ctrl-s 1`–`9` counts.
#[must_use]
pub fn position_of(session: &Session, terminal: TerminalId) -> Option<usize> {
    session
        .terminals
        .iter()
        .position(|candidate| candidate.id == terminal)
}

/// The terminal `ctrl-s <n>` selects, or `None` when the session has fewer tabs.
#[must_use]
pub fn terminal_at(session: &Session, position: usize) -> Option<TerminalId> {
    session.terminals.get(position).map(|terminal| terminal.id)
}

/// The terminal `ctrl-s h` / `ctrl-s l` moves to, wrapping at both ends.
///
/// Wrapping is what makes `ctrl-s l` a one-key cycle through a three-tab session, which is the
/// default layout; stopping at the end would cost a second keystroke on every lap.
#[must_use]
pub fn neighbour(
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

/// A name for a new tab that no existing tab already uses.
///
/// `ctrl-s c` opens a plain shell, so the first one is `sh` and the next ones are `sh2`, `sh3`;
/// a numbered suffix keeps the tab under the strip's 84 px minimum width.
#[must_use]
pub fn new_terminal_name(session: &Session) -> String {
    unique_terminal_name(session, "sh")
}

/// `base`, or `base2`, `base3`, … — the first name the session does not already use.
///
/// fleetd refuses a duplicate terminal name, so every caller that invents one goes through
/// here.
#[must_use]
pub fn unique_terminal_name(session: &Session, base: &str) -> String {
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
        assert_eq!(tab_label(&terminal, &HashSet::new()), "nvim");

        // §3.6: `FrameUpdate.title` names the tab.
        terminal.title = Some("README.md".to_owned());
        assert_eq!(tab_label(&terminal, &HashSet::new()), "README.md");

        // A cleared or blank title falls back to the configured window name.
        terminal.title = Some("   ".to_owned());
        assert_eq!(tab_label(&terminal, &HashSet::new()), "nvim");
        terminal.title = Some("README.md".to_owned());

        // After `ctrl-s ,` the user's name wins, whatever the program sets.
        let renamed: HashSet<TerminalId> = [TerminalId(1)].into_iter().collect();
        terminal.name = "editor".to_owned();
        assert_eq!(tab_label(&terminal, &renamed), "editor");
    }

    #[test]
    fn a_native_tab_is_marked_but_keeps_its_number() {
        let mut session = session(&["nvim", "cc", "lg"]);
        session.terminals[2].kind = fleet_core::sessions::TerminalKind::Native;
        let tabs = tabs(&session, None, Some(TerminalId(3)), &HashSet::new());
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
        let tabs = tabs(&session, None, Some(TerminalId(41)), &HashSet::new());
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
        let tabs = tabs(&session, None, Some(TerminalId(1)), &HashSet::new());
        assert!(!tabs[0].activity);
        assert!(tabs[1].activity);
    }

    #[test]
    fn an_exited_terminal_carries_its_code() {
        let mut session = session(&["test"]);
        session.terminals[0].status = TerminalStatus::Exited { code: Some(1) };
        assert_eq!(
            tabs(&session, None, None, &HashSet::new())[0].exited,
            Some(Some(1))
        );
    }

    #[test]
    fn keep_alive_labels_pick_their_glyph() {
        assert_eq!(keep_alive_icon("claude"), Icon::Bot);
        assert_eq!(keep_alive_icon("OpenCode"), Icon::Sparkles);
        assert_eq!(keep_alive_icon(":3000"), Icon::Server);
        assert_eq!(keep_alive_icon("nvim"), Icon::FilePen);
        assert_eq!(keep_alive_icon("something-else"), Icon::Zap);
    }

    #[test]
    fn the_first_keep_alive_label_wins() {
        let mut session = session(&["cc"]);
        session.terminals[0].keep_alive = vec!["claude".to_owned(), ":3000".to_owned()];
        assert_eq!(
            terminal_keep_alive_icon(&session.terminals[0]),
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

        let tabs = tabs(&session, Some(&status), None, &HashSet::new());
        assert_eq!(tabs[0].agent_status, Some(StatusKind::AgentWorking));
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
        assert_eq!(new_terminal_name(&session(&["nvim"])), "sh");
        assert_eq!(new_terminal_name(&session(&["sh"])), "sh2");
        assert_eq!(new_terminal_name(&session(&["sh", "sh2"])), "sh3");
    }
}
