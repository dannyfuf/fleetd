use super::*;

/// Which pane of the Hub owns the cursor. The detail panel is never in the cycle (§1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubPane {
    /// The repos rail.
    Repos,
    /// The worktrees or pull-requests list.
    List,
}

/// Which list the Hub is showing (`p` toggles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubTab {
    /// The worktrees list.
    Worktrees,
    /// The pull-requests screen.
    Prs,
}

/// The two top-level screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// Contexts, repos, worktrees or PRs, detail panel.
    Hub {
        /// Which list the Hub is showing.
        tab: HubTab,
    },
    /// One session's terminals.
    Workspace {
        /// The session being displayed.
        session: SessionId,
    },
}

impl Screen {
    /// The Hub, on the worktrees list.
    #[must_use]
    pub const fn hub() -> Self {
        Self::Hub {
            tab: HubTab::Worktrees,
        }
    }
}

/// The Workspace's sub-mode. Meaningless on the Hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    /// Every key goes to the PTY except `ctrl-s`.
    Terminal,
    /// The native pane handles keys under the same app reservations as a PTY.
    Native,
    /// One-shot, entered by `ctrl-s`, left by the very next key.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
}

/// The floating agent popup's terminal sub-mode.
///
/// This is deliberately separate from [`TerminalMode`]: a Workspace keeps its own mode while
/// the popup is above it, so hiding the popup restores the exact surface the user left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPopupMode {
    /// Every unreserved key goes to the agent PTY.
    Terminal,
    /// One-shot, entered by `ctrl-s`, left by the very next key.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
}

/// The persistent, screen-independent floating-agent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPopupState {
    /// Which fixed daemon-owned agent session is visible.
    pub agent: Agent,
    /// Which terminal input mode owns the popup keyboard.
    pub mode: AgentPopupMode,
    /// Returning from a prefix must preserve Scroll's selection and viewport ownership.
    pub prefix_return: AgentPopupMode,
}

/// The result of pressing an agent toggle key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPopupTransition {
    /// A closed popup opened.
    Opened,
    /// The other agent replaced the visible one.
    Switched,
    /// Pressing the already-visible agent's key hid the popup.
    Hidden,
}

/// The overlay that currently owns the keyboard. Overlays shadow the Hub and the Workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// The filter input over the focused list.
    Filter,
    /// The command palette.
    Palette,
    /// The right-docked jobs panel.
    Jobs,
    /// A modal dialog.
    Dialog(Dialogs),
}

impl Overlay {
    /// The key contexts this overlay pushes, outermost first.
    #[must_use]
    pub fn context_chain(&self) -> Vec<&'static str> {
        match self {
            Self::Filter => vec!["Filter"],
            Self::Palette => vec!["Palette"],
            Self::Jobs => vec!["Jobs"],
            Self::Dialog(dialog) => vec!["Dialog", dialog.context_name()],
        }
    }
}

/// Input mode derived from the screen and overlay stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Hub lists.
    Normal,
    /// Keys go to the PTY.
    Terminal,
    /// Keys go to the Fleet-drawn pane in the active tab.
    Native,
    /// One key after `ctrl-s`.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
    /// Filter input.
    Filter,
    /// Command palette.
    Palette,
    /// A dialog is open.
    Dialog,
    /// The jobs panel is open.
    Jobs,
}

impl Mode {
    /// The kit's mode word for this mode.
    #[must_use]
    pub const fn word(self) -> ModeWord {
        match self {
            Self::Normal => ModeWord::Normal,
            // Native tabs retain the TERMINAL mode word; their glyph identifies the kind.
            Self::Terminal | Self::Native => ModeWord::Terminal,
            Self::Prefix => ModeWord::Prefix,
            Self::Scroll => ModeWord::Scroll,
            Self::Filter => ModeWord::Filter,
            Self::Palette => ModeWord::Palette,
            Self::Dialog => ModeWord::Dialog,
            Self::Jobs => ModeWord::Jobs,
        }
    }
}

/// A most-recently-used list. The front is the most recent entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mru<T> {
    entries: Vec<T>,
}

impl<T> Default for Mru<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<T: PartialEq> Mru<T> {
    /// Moves `entry` to the front, capping the list.
    pub fn touch(&mut self, entry: T) {
        self.entries.retain(|existing| existing != &entry);
        self.entries.insert(0, entry);
        self.entries.truncate(MRU_CAPACITY);
    }

    /// Keeps only the entries a predicate accepts — used to reconcile with a fresh snapshot.
    pub fn retain(&mut self, keep: impl Fn(&T) -> bool) {
        self.entries.retain(|entry| keep(entry));
    }

    /// The alternate entry: what `ctrl-s w` and `ctrl-s Tab` jump to.
    #[must_use]
    pub fn alternate(&self) -> Option<&T> {
        self.entries.get(1)
    }

    /// The entries, most recent first.
    #[must_use]
    pub fn entries(&self) -> &[T] {
        &self.entries
    }
}

/// Which repository the lists are scoped to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RepoScope {
    /// The `All` pseudo-repo.
    #[default]
    All,
    /// One repository.
    Repo(RepoId),
}

/// One cursor per list, so switching panes and screens never loses a position (§1.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursors {
    /// The repos rail.
    pub repos: usize,
    /// The worktrees list.
    pub worktrees: usize,
    /// The `Mine` PR tab.
    pub prs_mine: usize,
    /// The `Review` PR tab.
    pub prs_review: usize,
    /// The jobs panel.
    pub jobs: usize,
}

/// Moves a cursor by `delta` rows, clamped to the list. Background events never move it (§5.11).
#[must_use]
pub fn move_cursor(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len - 1;
    let next = current as isize + delta;
    next.clamp(0, last as isize) as usize
}

/// Clamps a cursor after the list behind it changed length.
#[must_use]
pub fn clamp_cursor(current: usize, len: usize) -> usize {
    if len == 0 { 0 } else { current.min(len - 1) }
}

/// How many rows `ctrl-d` and `ctrl-u` move, given the rows a pane can show.
#[must_use]
pub fn half_page(visible_rows: usize) -> isize {
    (visible_rows.max(2) / 2) as isize
}

/// The filter of the focused list (§3.10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterState {
    /// The live query. An empty query means no filter.
    pub query: String,
    /// Whether the input still owns the keyboard. `false` means the filter is retained.
    pub editing: bool,
}

impl FilterState {
    /// Whether rows are currently being filtered.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }
}

/// What `Esc` does in filter mode. The two stages are the reason `Esc` never quits (§A13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterEscape {
    /// Leave the input, keep the filter, and show the retained `⌕query` chip.
    LeaveInput,
    /// Clear the filter and restore the pane header.
    ClearFilter,
}

/// The two-stage `Esc` of §3.10.
#[must_use]
pub const fn filter_escape(editing: bool) -> FilterEscape {
    if editing {
        FilterEscape::LeaveInput
    } else {
        FilterEscape::ClearFilter
    }
}

impl AppState {
    /// The nested key contexts of the focused element, outermost first.
    ///
    /// [`crate::keymap`] predicates are written against exactly this chain, which is why the
    /// Hub's panes are `Hub > Repos` and a dialog is `Dialog > <name>`. The daemon banner is
    /// appended **innermost** so its `r` / `l` / `Esc` outrank the Hub's while it is showing,
    /// and dismissing it (`Esc`) gives them straight back.
    #[must_use]
    pub fn context_chain(&self) -> Vec<&'static str> {
        // Overlays also own keys above first-run and daemon-failure surfaces.
        if let Some(overlay) = &self.overlay {
            return overlay.context_chain();
        }
        // §3.12 B replaces the whole window, so its keys outrank every base surface's.
        if matches!(self.daemon, DaemonLink::Failed { .. }) {
            return vec!["Daemon", "Down"];
        }
        if self.is_first_run() {
            return vec!["FirstRun"];
        }
        let mut chain = match (self.agent_popup, &self.screen) {
            (Some(popup), _) => vec![
                "Agent",
                match popup.mode {
                    AgentPopupMode::Terminal => "Terminal",
                    AgentPopupMode::Prefix => "Prefix",
                    AgentPopupMode::Scroll => "Scroll",
                },
            ],
            (None, Screen::Hub { tab }) => vec![
                "Hub",
                match (self.hub_pane, tab) {
                    (HubPane::Repos, _) => "Repos",
                    (HubPane::List, HubTab::Worktrees) => "Worktrees",
                    (HubPane::List, HubTab::Prs) => "Prs",
                },
            ],
            (None, Screen::Workspace { .. }) => vec![
                "Workspace",
                match self.terminal_mode {
                    TerminalMode::Terminal => "Terminal",
                    TerminalMode::Native => "Native",
                    TerminalMode::Prefix => "Prefix",
                    TerminalMode::Scroll => "Scroll",
                },
            ],
        };
        if let DaemonLink::Lost {
            dismissed: false, ..
        } = self.daemon
        {
            chain.extend_from_slice(&["Daemon", "Banner"]);
        }
        chain
    }

    /// The mode word for the status bar (§2.8).
    #[must_use]
    pub fn mode(&self) -> Mode {
        if let Some(overlay) = &self.overlay {
            return match overlay {
                Overlay::Filter => Mode::Filter,
                Overlay::Palette => Mode::Palette,
                Overlay::Jobs => Mode::Jobs,
                Overlay::Dialog(_) => Mode::Dialog,
            };
        }
        if let Some(popup) = self.agent_popup {
            return match popup.mode {
                AgentPopupMode::Terminal => Mode::Terminal,
                AgentPopupMode::Prefix => Mode::Prefix,
                AgentPopupMode::Scroll => Mode::Scroll,
            };
        }
        match self.screen {
            Screen::Hub { .. } => Mode::Normal,
            Screen::Workspace { .. } => match self.terminal_mode {
                TerminalMode::Terminal => Mode::Terminal,
                TerminalMode::Native => Mode::Native,
                TerminalMode::Prefix => Mode::Prefix,
                TerminalMode::Scroll => Mode::Scroll,
            },
        }
    }

    /// Enters the one-shot prefix mode.
    pub fn enter_prefix(&mut self) {
        if matches!(self.screen, Screen::Workspace { .. }) {
            self.terminal_mode = TerminalMode::Prefix;
        }
    }

    /// Opens, switches, or hides the floating agent popup without changing the base screen.
    pub fn toggle_agent_popup(&mut self, agent: Agent) -> AgentPopupTransition {
        let transition = match self.agent_popup {
            Some(current) if current.agent == agent => {
                self.agent_popup = None;
                return AgentPopupTransition::Hidden;
            }
            Some(_) => AgentPopupTransition::Switched,
            None => AgentPopupTransition::Opened,
        };
        self.agent_popup = Some(AgentPopupState {
            agent,
            mode: AgentPopupMode::Terminal,
            prefix_return: AgentPopupMode::Terminal,
        });
        transition
    }

    /// Hides the floating agent surface. The daemon session is intentionally untouched.
    pub fn hide_agent_popup(&mut self) -> bool {
        self.agent_popup.take().is_some()
    }

    /// Enters the popup's one-shot prefix mode.
    pub fn enter_agent_prefix(&mut self) {
        if let Some(popup) = &mut self.agent_popup {
            popup.prefix_return = popup.mode;
            popup.mode = AgentPopupMode::Prefix;
        }
    }

    /// Leaves the popup prefix after its one following key.
    pub fn leave_agent_prefix(&mut self) -> bool {
        let Some(popup) = &mut self.agent_popup else {
            return false;
        };
        if popup.mode == AgentPopupMode::Prefix {
            popup.mode = popup.prefix_return;
            true
        } else {
            false
        }
    }

    /// The daemon session currently selected by the floating agent popup.
    #[must_use]
    pub fn agent_popup_session(&self) -> Option<&Session> {
        let agent = self.agent_popup?.agent;
        let id = fleet_core::sessions::agent_session_id(agent).ok()?;
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .find(|session| session.id == id)
    }

    /// The selected popup agent whose fixed runtime session is absent from a reachable daemon.
    ///
    /// The shell uses this as the pure half of its single-flight recovery gate after reconnects
    /// and authoritative snapshots that remove an agent session.
    #[must_use]
    pub fn missing_agent_popup_session(&self) -> Option<Agent> {
        let popup = self.agent_popup?;
        if !self.daemon.is_connected() {
            return None;
        }
        let session = fleet_core::sessions::agent_session_id(popup.agent).ok()?;
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .all(|candidate| candidate.id != session)
            .then_some(popup.agent)
    }

    /// Leaves the prefix, whatever the key was. Called for **every** key seen in `Prefix`,
    /// which is what makes the mode one-shot without a timeout.
    ///
    /// Returns whether the mode actually changed.
    pub fn leave_prefix(&mut self) -> bool {
        if self.terminal_mode == TerminalMode::Prefix {
            self.terminal_mode = self.resting_terminal_mode();
            true
        } else {
            false
        }
    }

    /// The mode a Workspace tab rests in: what owns the keyboard when no Fleet mode is active.
    #[must_use]
    pub fn resting_terminal_mode(&self) -> TerminalMode {
        if self.active_terminal_is_native() {
            TerminalMode::Native
        } else {
            TerminalMode::Terminal
        }
    }

    /// Whether the active session's active tab is drawn by Fleet rather than by a PTY.
    #[must_use]
    pub fn active_terminal_is_native(&self) -> bool {
        self.active_terminal_record()
            .is_some_and(fleet_core::sessions::Terminal::is_native)
    }

    /// The active session's active terminal record, straight from the snapshot.
    #[must_use]
    pub fn active_terminal_record(&self) -> Option<&fleet_core::sessions::Terminal> {
        let session = self.active_session()?;
        let active = session.active_terminal?;
        session
            .terminals
            .iter()
            .find(|terminal| terminal.id == active)
    }

    /// Re-derives the resting mode after a snapshot moved, created or replaced the active tab.
    ///
    /// `Prefix` and `Scroll` are transient Fleet modes the user is standing in; a snapshot must
    /// not yank them away underneath. Everything else follows the tab.
    pub fn sync_terminal_mode(&mut self) {
        if matches!(
            self.terminal_mode,
            TerminalMode::Terminal | TerminalMode::Native
        ) {
            self.terminal_mode = self.resting_terminal_mode();
        }
    }

    /// `^s v` toggles only local watch visibility and always returns to terminal mode.
    pub fn toggle_watch_pane(&mut self, now: Instant) {
        self.leave_prefix();
        if let Some(session) = self.active_session().map(|s| s.id.clone())
            && !self.watches.toggle(&session)
        {
            self.toast_short("no subagent watches", Icon::Info, now);
        }
    }

    /// `^s N/P` shows and cycles watches without changing the active terminal.
    pub fn cycle_watch(&mut self, forward: bool, now: Instant) {
        self.leave_prefix();
        if let Some(session) = self.active_session().map(|s| s.id.clone())
            && !self.watches.cycle(&session, forward)
        {
            self.toast_short("no subagent watches", Icon::Info, now);
        }
    }

    /// `^s V` / ×: returns an exited watch to dismiss; a running watch is only hidden.
    pub fn close_selected_watch(&mut self, now: Instant) -> Option<fleet_core::watches::WatchId> {
        self.leave_prefix();
        let session = self.active_session()?.id.clone();
        let id = self.watches.panes.get(&session)?.selected?;
        if self.watches.entries.get(&id)?.watch.status == fleet_core::watches::WatchStatus::Running
        {
            self.watches.hide(&session);
            self.toast_short("watch still running; pane hidden", Icon::Info, now);
            return None;
        }
        Some(id)
    }

    /// Opens an overlay, replacing whatever was open.
    pub fn open_overlay(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::Jobs) {
            self.jobs_focus = self
                .sticky_error
                .as_ref()
                .and_then(|error| error.job.clone());
            if let Some(job) = self.jobs_focus.clone() {
                self.seen_failed.insert(job);
            }
            if let Some(snapshot) = self.snapshot.as_ref() {
                notifications::refresh_job_sticky_error(
                    &mut self.sticky_error,
                    &snapshot.jobs,
                    &self.seen_failed,
                );
            }
        }
        self.overlay = Some(overlay);
    }

    /// Closes the topmost overlay, returning whether one was open.
    pub fn close_overlay(&mut self) -> bool {
        self.overlay.take().is_some()
    }

    /// `Esc` outside a dialog: clear the filter if any, else close the topmost overlay, else
    /// nothing. It never quits (§A13).
    pub fn cancel(&mut self) -> bool {
        if self.filter.is_active() || self.filter.editing {
            match filter_escape(self.filter.editing) {
                FilterEscape::LeaveInput => {
                    self.filter.editing = false;
                    if matches!(self.overlay, Some(Overlay::Filter)) {
                        self.overlay = None;
                    }
                }
                FilterEscape::ClearFilter => self.filter.query.clear(),
            }
            return true;
        }
        self.close_overlay()
    }

    /// Moves the session MRU as a session is opened.
    pub fn touch_session(&mut self, session: SessionId) {
        self.session_mru.touch(session);
    }

    /// Moves a session's terminal MRU as a tab is selected.
    pub fn touch_terminal(&mut self, session: &SessionId, terminal: TerminalId) {
        self.terminal_mru
            .entry(session.clone())
            .or_default()
            .touch(terminal);
    }

    /// The session shown by the Workspace, when that is the current screen.
    #[must_use]
    pub fn active_session(&self) -> Option<&Session> {
        let Screen::Workspace { session } = &self.screen else {
            return None;
        };
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .find(|candidate| &candidate.id == session)
    }
}

#[cfg(test)]
mod tests;
