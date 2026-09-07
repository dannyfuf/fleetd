use super::*;

pub(super) struct Model {
    pub(super) agent: Agent,
    pub(super) session: SessionId,
    pub(super) mode: AgentPopupMode,
    pub(super) terminal: Option<TerminalId>,
    pub(super) base_terminal: Option<TerminalId>,
    pub(super) generation: u64,
    pub(super) primed: bool,
    pub(super) reachable: bool,
    pub(super) cols: Option<u16>,
    pub(super) history_epoch: Option<u64>,
    pub(super) alt_screen: bool,
    pub(super) scroll_offset: usize,
    pub(super) scrollback_len: usize,
    pub(super) terminal_state: AgentTerminalState,
    pub(super) exit_code: Option<Option<i32>>,
    pub(super) activity: AgentActivity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentTerminalState {
    Unknown,
    Starting,
    Running,
    Exited,
}

impl Model {
    pub(super) fn build(app: &AppState) -> Option<Self> {
        let popup = app.agent_popup?;
        let session = agent_session_id(popup.agent).ok()?;
        let record = app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.sessions.iter().find(|entry| entry.id == session));
        let terminal_record = record.and_then(|session| session.terminals.first());
        let terminal = terminal_record.map(|terminal| terminal.id);
        let grid = terminal.and_then(|terminal| app.grids.get(&terminal));
        let base_terminal = match &app.screen {
            Screen::Workspace { session } => app.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .sessions
                    .iter()
                    .find(|entry| &entry.id == session)
                    .and_then(|entry| entry.active_terminal)
            }),
            Screen::Hub { .. } => None,
        };
        let terminal_state = terminal_record.map_or(AgentTerminalState::Unknown, |terminal| {
            match terminal.status {
                TerminalStatus::Starting => AgentTerminalState::Starting,
                TerminalStatus::Running => AgentTerminalState::Running,
                TerminalStatus::Exited { .. } => AgentTerminalState::Exited,
            }
        });
        let exit_code = terminal_record.and_then(|terminal| match terminal.status {
            TerminalStatus::Exited { code } => Some(code),
            TerminalStatus::Starting | TerminalStatus::Running => None,
        });
        let activity = app.session_agent_activity(&session);
        Some(Self {
            agent: popup.agent,
            session,
            mode: popup.mode,
            terminal,
            base_terminal,
            generation: app.link_generation,
            primed: grid.is_some_and(|grid| grid.primed),
            reachable: app.daemon.is_connected(),
            cols: grid.map(|grid| grid.cols),
            history_epoch: grid.map(|grid| grid.viewport.history_epoch),
            alt_screen: grid.is_some_and(|grid| grid.modes.alt_screen),
            scroll_offset: grid.map_or(0, |grid| grid.viewport.offset),
            scrollback_len: grid.map_or(0, |grid| grid.viewport.scrollback_len),
            terminal_state,
            exit_code,
            activity,
        })
    }
}
