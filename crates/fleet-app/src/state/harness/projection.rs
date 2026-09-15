//! Builds the harness projection and memoises it against everything it reads.
//!
//! The builder lives beside the types it produces rather than inside them because it is the
//! half that has to stay honest about *inputs*: [`ProjectionKey`] is the list of every piece of
//! [`AppState`] the projection touches, and the memo is only as fresh as that list is complete.
//! The Jobs panel remains authoritative for its cursor and filter; this projection reads their
//! explicitly synchronized [`crate::state::JobsPanelMirror`] rather than owning another copy.
//! Nothing here is reachable from a `render` body (`docs/APP-CONTRACTS.md`).

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use fleet_core::{
    agents::{AgentKind, Attention, AttentionKind},
    board::Card,
    ids::TerminalId,
    sessions::Terminal as SessionTerminal,
};

use super::{
    AgentThreadSnapshot, AgentsSnapshot, CursorSnapshot, DaemonSnapshot, DialogSnapshot,
    HarnessProjection, IdleSnapshot, JobSnapshot, ListSnapshot, RowSnapshot, SNAPSHOT_VERSION,
    TerminalSnapshot, ToastSnapshot, UiSnapshot, ViewportSnapshot, WindowSnapshot,
};
use crate::presentation::DisplayedHub;
use crate::state::{
    AgentPopupMode, AppState, BoardFocus, Cursors, DaemonLink, FilterState, HubPane, HubTab,
    JobsPanelMirror, LiveToast, Mode, Overlay, RepoScope, Screen, StickyError, running_jobs,
};

/// Every input the builder reads, as cheap comparable values.
///
/// Adding a field to the builder means adding its input here: a key that misses an input is a
/// stale snapshot, which makes `await` hang until its timeout. The derived scalars are stored
/// as their projected form because deriving them is cheaper than keying on everything they
/// read (`mode` alone reads the overlay, the popup, the board filter and the agent mirror).
#[derive(Debug, Clone, PartialEq)]
struct ProjectionKey {
    derived: Derived,
    snapshot_revision: u64,
    board_revision: u64,
    link_generation: u64,
    scope: RepoScope,
    pr_tab: fleet_core::github::PrTab,
    filter: FilterState,
    cursors: Cursors,
    jobs_panel: JobsPanelMirror,
    board_focus: BoardFocus,
    board_filter: String,
    board_filter_editing: bool,
    displayed_hub: DisplayedHub,
    toasts: Vec<LiveToast>,
    sticky_error: Option<StickyError>,
    grid: Option<GridKey>,
    targets_revision: u64,
    window: WindowSnapshot,
    in_flight_requests: u32,
    pending_frame: bool,
    armed_debounces: u32,
    settling_mutations: u32,
    link_opening: bool,
    renamed_terminals: HashSet<TerminalId>,
}

/// The identity and generation of the mirror grid the snapshot reports.
///
/// A mirror only ever changes through [`crate::state::MirrorGrid::apply`], which advances `seq`, and
/// through the two out-of-band edges that do not: a desync mark and a PTY exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridKey {
    terminal: TerminalId,
    seq: u64,
    desynced: bool,
    exited: bool,
}

/// The cheap scalars the key holds and the builder reuses.
#[derive(Debug, Clone, PartialEq)]
struct Derived {
    screen: String,
    mode: String,
    hub_pane: Option<String>,
    hub_tab: Option<String>,
    overlay: Option<String>,
    key_contexts: Vec<String>,
    focused: Option<String>,
    dialog: Option<DialogSnapshot>,
    agents: AgentsSnapshot,
    daemon: DaemonSnapshot,
}

/// The memo: one key, one snapshot, one revision.
#[derive(Debug)]
pub(in crate::state) struct HarnessCache {
    revision: u64,
    key: ProjectionKey,
    snapshot: Rc<UiSnapshot>,
    /// How many times the builder actually ran, which the memo test asserts on.
    builds: u64,
}

impl AppState {
    /// The memoised harness projection, built outside rendering.
    ///
    /// No render path calls this: the only callers are the harness socket's `dump`, `await` and
    /// `assert` commands, which run in update paths (`docs/APP-CONTRACTS.md`). The cache is
    /// allocated lazily, so an app running without the harness socket never pays for it.
    #[must_use]
    pub fn harness_projection(&self) -> HarnessProjection {
        let key = self.projection_key();
        let mut slot = self.harness_cache.borrow_mut();
        if let Some(cache) = slot.as_mut() {
            if cache.key == key {
                return HarnessProjection {
                    revision: cache.revision,
                    snapshot: Rc::clone(&cache.snapshot),
                };
            }
            let snapshot = Rc::new(self.build_snapshot(&key));
            cache.builds = cache.builds.wrapping_add(1);
            cache.key = key;
            // An input moved without changing anything the harness can see — a repainted frame,
            // a re-applied identical snapshot. Holding the revision keeps `await` from waking
            // for a predicate that cannot possibly have changed its answer.
            if cache.snapshot != snapshot {
                cache.revision = cache.revision.wrapping_add(1);
                cache.snapshot = snapshot;
            }
            return HarnessProjection {
                revision: cache.revision,
                snapshot: Rc::clone(&cache.snapshot),
            };
        }
        let snapshot = Rc::new(self.build_snapshot(&key));
        *slot = Some(Box::new(HarnessCache {
            revision: 1,
            key,
            snapshot: Rc::clone(&snapshot),
            builds: 1,
        }));
        HarnessProjection {
            revision: 1,
            snapshot,
        }
    }

    /// The harness projection alone, for callers that do not track revisions.
    #[must_use]
    pub fn harness_snapshot(&self) -> Rc<UiSnapshot> {
        self.harness_projection().snapshot
    }

    /// How many times the builder has run, so the memo can be asserted on.
    #[cfg(test)]
    pub(super) fn harness_builds(&self) -> u64 {
        self.harness_cache
            .borrow()
            .as_ref()
            .map_or(0, |cache| cache.builds)
    }

    /// Whether the projection cache has ever been allocated.
    #[cfg(test)]
    pub(super) fn harness_cache_allocated(&self) -> bool {
        self.harness_cache.borrow().is_some()
    }

    fn projection_key(&self) -> ProjectionKey {
        ProjectionKey {
            derived: self.derived(),
            snapshot_revision: self.snapshot_revision,
            board_revision: self.board.revision,
            link_generation: self.link_generation,
            scope: self.scope.clone(),
            pr_tab: self.pr_tab,
            filter: self.filter.clone(),
            cursors: self.cursors.clone(),
            jobs_panel: self.jobs_panel,
            board_focus: self.board.focus,
            board_filter: self.board.filter.clone(),
            board_filter_editing: self.board.filter_editing,
            displayed_hub: self.displayed_hub.clone(),
            toasts: self.toasts.clone(),
            sticky_error: self.sticky_error.clone(),
            grid: self.harness_grid().map(|(terminal, grid)| GridKey {
                terminal,
                seq: grid.seq,
                desynced: grid.desynced,
                exited: grid.exit_code.is_some(),
            }),
            targets_revision: self.harness.targets_revision,
            window: self.harness.window.clone(),
            in_flight_requests: self.harness.in_flight_requests(),
            pending_frame: self.harness.pending_frame,
            armed_debounces: self.harness.armed_debounces(),
            settling_mutations: self.harness.settling_mutations(),
            link_opening: matches!(self.daemon, DaemonLink::Starting),
            renamed_terminals: self.renamed_terminals.clone(),
        }
    }

    fn derived(&self) -> Derived {
        Derived {
            screen: screen_name(&self.screen).to_owned(),
            mode: mode_name(self.mode()).to_owned(),
            hub_pane: matches!(self.screen, Screen::Hub { .. })
                .then(|| hub_pane_name(self.hub_pane).to_owned()),
            hub_tab: match &self.screen {
                Screen::Hub { tab } => Some(hub_tab_name(*tab).to_owned()),
                Screen::Workspace { .. } => None,
            },
            overlay: self.overlay.as_ref().map(overlay_name).map(str::to_owned),
            key_contexts: self
                .context_chain()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            focused: self.focus_target(),
            dialog: self.dialog_snapshot(),
            agents: self.agents_snapshot(),
            daemon: self.daemon_snapshot(),
        }
    }

    /// The daemon link, flattened to the four scalars a predicate can read.
    fn daemon_snapshot(&self) -> DaemonSnapshot {
        match &self.daemon {
            DaemonLink::Starting => DaemonSnapshot {
                link: "starting",
                attempt: 0,
                dismissed: false,
                restarted: false,
            },
            DaemonLink::Failed { .. } => DaemonSnapshot {
                link: "failed",
                attempt: 0,
                dismissed: false,
                restarted: false,
            },
            DaemonLink::Connected => DaemonSnapshot {
                link: "connected",
                attempt: 0,
                dismissed: false,
                restarted: false,
            },
            DaemonLink::Lost {
                attempt, dismissed, ..
            } => DaemonSnapshot {
                link: "lost",
                attempt: *attempt,
                dismissed: *dismissed,
                restarted: false,
            },
            DaemonLink::Reconnected { restarted, .. } => DaemonSnapshot {
                link: "reconnected",
                attempt: 0,
                dismissed: false,
                restarted: *restarted,
            },
        }
    }

    fn build_snapshot(&self, key: &ProjectionKey) -> UiSnapshot {
        let derived = &key.derived;
        let jobs = self.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
            snapshot
                .jobs
                .iter()
                .map(|job| JobSnapshot {
                    id: job.id.as_str().to_owned(),
                    status: job_status_name(&job.status).to_owned(),
                })
                .collect()
        });
        let running = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| running_jobs(&snapshot.jobs).len());
        UiSnapshot {
            version: SNAPSHOT_VERSION,
            screen: derived.screen.clone(),
            mode: derived.mode.clone(),
            hub_pane: derived.hub_pane.clone(),
            hub_tab: derived.hub_tab.clone(),
            overlay: derived.overlay.clone(),
            key_contexts: derived.key_contexts.clone(),
            focused: derived.focused.clone(),
            lists: self.lists_snapshot(),
            dialog: derived.dialog.clone(),
            toasts: self
                .toasts
                .iter()
                .map(|live| ToastSnapshot {
                    level: toast_level_name(live.toast.tone).to_owned(),
                    text: live.toast.text.to_string(),
                    count: saturating_u32(live.toast.count),
                })
                .collect(),
            sticky_error: self.sticky_error.as_ref().map(|error| error.text.clone()),
            jobs,
            agents: derived.agents.clone(),
            terminal: self.terminal_snapshot(),
            targets: self.harness.targets.clone(),
            daemon: derived.daemon,
            idle: IdleSnapshot::new(
                self.harness.in_flight_requests(),
                saturating_u32(running),
                self.harness.pending_frame,
                saturating_u32(self.toasts.len()),
                self.harness.armed_debounces(),
                key.settling_mutations,
                key.link_opening,
            ),
            window: self.harness.window.clone(),
        }
    }
}

impl AppState {
    /// The stable target name of whatever owns the keyboard.
    ///
    /// The names are the ones `docs/TESTING-HARNESS.md` §3 freezes for mouse targets, so a
    /// scenario that asserts on `focused` and one that clicks use the same vocabulary. The
    /// index is the cursor's own position, which an empty list still has: focus is a fact
    /// about the pane, not about whether it currently holds a row.
    fn focus_target(&self) -> Option<String> {
        if let Some(overlay) = &self.overlay {
            return Some(match overlay {
                Overlay::Filter => "filter.input".to_owned(),
                Overlay::Palette => "palette.input".to_owned(),
                Overlay::Jobs => format!("jobs.row[{}]", self.jobs_panel.cursor),
                // Which field or button holds focus lives in the dialog host entity, not in
                // `AppState`; the dialog itself is as precise as this projection can be.
                Overlay::Dialog(_) => "dialog".to_owned(),
            });
        }
        if self.agent_popup.is_some() {
            return Some("agents.popup".to_owned());
        }
        if self.board_filter_owns_keys() {
            return Some("board.filter".to_owned());
        }
        match &self.screen {
            Screen::Hub { tab } => Some(match (self.hub_pane, tab) {
                (_, HubTab::Board) => format!(
                    "board.column[{}].card[{}]",
                    self.board.focus.column, self.board.focus.row
                ),
                (HubPane::Repos, _) => format!("repos.row[{}]", self.cursors.repos),
                (HubPane::List, HubTab::Worktrees) => {
                    format!("worktrees.row[{}]", self.cursors.worktrees)
                }
                (HubPane::List, HubTab::Prs) => format!("prs.row[{}]", self.pr_cursor()),
            }),
            Screen::Workspace { .. } => {
                let tabs = self.tab_rows();
                let index = tabs.selected?;
                Some(if tabs.rows.get(index).is_some_and(|row| row.agent) {
                    format!("agents.tabs.tab[{index}]")
                } else {
                    format!("tabs.tab[{index}]")
                })
            }
        }
    }

    /// Which PR cursor the active tab uses.
    fn pr_cursor(&self) -> usize {
        match self.pr_tab {
            fleet_core::github::PrTab::Mine => self.cursors.prs_mine,
            fleet_core::github::PrTab::Review => self.cursors.prs_review,
        }
    }

    /// The open dialog, by the name `docs/KEYMAP.md` gives its key context.
    ///
    /// Fields, buttons and the message body belong to the dialog host entity and are not
    /// mirrored into `AppState`, so they are reported empty rather than guessed.
    fn dialog_snapshot(&self) -> Option<DialogSnapshot> {
        let Some(Overlay::Dialog(dialog)) = self.overlay.as_ref() else {
            return None;
        };
        Some(DialogSnapshot {
            name: dialog.context_name().to_owned(),
            fields: Vec::new(),
            buttons: Vec::new(),
            message: None,
        })
    }

    /// The floating popup's sub-mode and every native agent thread the daemon lists.
    fn agents_snapshot(&self) -> AgentsSnapshot {
        AgentsSnapshot {
            popup: self
                .agent_popup
                .as_ref()
                .map(|popup| agent_popup_mode_name(popup.mode).to_owned()),
            threads: self
                .agents
                .summaries()
                .iter()
                .map(|summary| {
                    let attention = self.agents.attention(summary.thread);
                    AgentThreadSnapshot {
                        id: summary.thread.to_string(),
                        provider: provider_name(summary.provider).to_owned(),
                        state: attention_name(attention).to_owned(),
                        unread: self.agents.seen(summary.thread) < summary.last_seq,
                        pending_gate: pending_gate_name(attention).map(str::to_owned),
                    }
                })
                .collect(),
        }
    }

    /// Every list the current surface shows, keyed by the name a predicate addresses it with.
    fn lists_snapshot(&self) -> BTreeMap<String, ListSnapshot> {
        let mut lists = BTreeMap::new();
        let Some(snapshot) = self.snapshot.as_ref() else {
            return lists;
        };
        let (rail_filter, list_filter) = match self.hub_pane {
            HubPane::Repos => (self.filter.query.clone(), String::new()),
            HubPane::List => (String::new(), self.filter.query.clone()),
        };

        lists.insert(
            "repos".to_owned(),
            list(self.repo_rows(), self.cursors.repos, rail_filter),
        );
        lists.insert(
            "worktrees".to_owned(),
            list(
                self.worktree_rows(snapshot),
                self.cursors.worktrees,
                list_filter.clone(),
            ),
        );
        lists.insert(
            "prs".to_owned(),
            list(self.pr_rows(), self.pr_cursor(), list_filter),
        );
        lists.insert(
            "jobs".to_owned(),
            list(
                self.job_rows(snapshot),
                self.jobs_panel.cursor,
                self.jobs_panel
                    .filter
                    .label()
                    .unwrap_or_default()
                    .to_owned(),
            ),
        );
        if let Some(view) = self.board.view.as_ref() {
            let columns: Vec<RowSnapshot> = view
                .board
                .statuses
                .iter()
                .map(|status| {
                    let cards = view
                        .cards
                        .iter()
                        .filter(|card| card.status_id == status.id)
                        .count();
                    RowSnapshot {
                        id: status.id.as_str().to_owned(),
                        label: status.name.clone(),
                        badges: vec![cards.to_string()],
                        marks: Vec::new(),
                    }
                })
                .collect();
            let cards = view
                .board
                .statuses
                .get(self.board.focus.column)
                .map(|status| {
                    view.cards
                        .iter()
                        .filter(|card| card.status_id == status.id)
                        .map(|card| card_row(&view.board.prefix, card))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            lists.insert(
                "board".to_owned(),
                list(columns, self.board.focus.column, self.board.filter.clone()),
            );
            lists.insert(
                "board.cards".to_owned(),
                list(cards, self.board.focus.row, self.board.filter.clone()),
            );
        }
        let tabs = self.tab_rows();
        if !tabs.rows.is_empty() {
            lists.insert(
                "tabs".to_owned(),
                ListSnapshot {
                    selected: tabs
                        .selected
                        .and_then(|index| tabs.rows.get(index))
                        .map(|row| row.row.clone()),
                    rows: tabs.rows.iter().map(|row| row.row.clone()).collect(),
                    filter: String::new(),
                },
            );
        }
        lists
    }

    /// The repos rail, exactly as the Hub last prepared it.
    fn repo_rows(&self) -> Vec<RowSnapshot> {
        self.displayed_hub
            .repos
            .iter()
            .map(|row| {
                use crate::presentation::DisplayedRepoKind as Kind;
                let id = row
                    .repo
                    .as_ref()
                    .map_or_else(|| "all".to_owned(), |repo| repo.as_str().to_owned());
                let scoped = match (&self.scope, row.repo.as_ref()) {
                    (RepoScope::All, None) => true,
                    (RepoScope::Repo(scope), Some(repo)) => scope == repo,
                    _ => false,
                };
                RowSnapshot {
                    label: if row.kind == Kind::All {
                        "All".to_owned()
                    } else {
                        id.clone()
                    },
                    id,
                    badges: match row.kind {
                        Kind::All | Kind::Repo => Vec::new(),
                        Kind::Cloning => vec!["cloning".to_owned()],
                        Kind::CloneFailed => vec!["clone-failed".to_owned()],
                    },
                    marks: if scoped {
                        vec!["scope".to_owned()]
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect()
    }

    /// The worktrees list, as the Hub last prepared it, labelled from the daemon mirror.
    fn worktree_rows(&self, snapshot: &fleet_proto::snapshot::Snapshot) -> Vec<RowSnapshot> {
        self.displayed_hub
            .worktrees
            .iter()
            .map(|row| {
                let record = snapshot
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.id == row.id);
                let mut marks = Vec::new();
                let mut badges = Vec::new();
                if let Some(record) = record {
                    badges.push(record.branch.clone());
                    if record.degraded.is_some() {
                        marks.push("degraded".to_owned());
                    }
                    if record.host.is_some() {
                        marks.push("remote".to_owned());
                    }
                }
                RowSnapshot {
                    id: row.id.as_str().to_owned(),
                    label: record
                        .map_or_else(|| row.id.as_str().to_owned(), |record| record.slug.clone()),
                    badges,
                    marks,
                }
            })
            .collect()
    }

    /// The pull-request list of the active tab, as the Hub last prepared it.
    fn pr_rows(&self) -> Vec<RowSnapshot> {
        self.displayed_hub
            .prs
            .iter()
            .map(|row| RowSnapshot {
                id: format!("{}#{}", row.repo.as_str(), row.number),
                label: format!("#{}", row.number),
                badges: vec![row.repo.as_str().to_owned()],
                marks: if row.local.is_some() {
                    vec!["local".to_owned()]
                } else {
                    Vec::new()
                },
            })
            .collect()
    }

    /// Every job the panel shows, in its filtered snapshot order.
    fn job_rows(&self, snapshot: &fleet_proto::snapshot::Snapshot) -> Vec<RowSnapshot> {
        snapshot
            .jobs
            .iter()
            .filter(|job| self.jobs_panel.filter.matches(job))
            .map(|job| {
                let mut marks = Vec::new();
                if job.cancellable {
                    marks.push("cancellable".to_owned());
                }
                if job.retryable {
                    marks.push("retryable".to_owned());
                }
                RowSnapshot {
                    id: job.id.as_str().to_owned(),
                    label: job.title.clone(),
                    badges: vec![job_status_name(&job.status).to_owned()],
                    marks,
                }
            })
            .collect()
    }

    /// The Workspace tab strip: the session's terminals, then its open agent threads.
    fn tab_rows(&self) -> TabRows {
        let mut rows = Vec::new();
        let Some(session) = self.active_session() else {
            return TabRows {
                rows,
                selected: None,
            };
        };
        let mut selected = None;
        for terminal in &session.terminals {
            if Some(terminal.id) == session.active_terminal {
                selected = Some(rows.len());
            }
            rows.push(TabRow {
                row: self.terminal_row(terminal),
                agent: false,
            });
        }
        if let fleet_core::sessions::SessionKind::Worktree(worktree) = &session.kind {
            let active = self.active_agent_thread();
            for summary in self.agents.of_worktree(worktree) {
                if Some(summary.thread) == active {
                    selected = Some(rows.len());
                }
                rows.push(TabRow {
                    row: RowSnapshot {
                        id: summary.thread.to_string(),
                        label: summary.title.clone(),
                        badges: vec![provider_name(summary.provider).to_owned()],
                        marks: vec![
                            attention_name(self.agents.attention(summary.thread)).to_owned(),
                        ],
                    },
                    agent: true,
                });
            }
        }
        TabRows { rows, selected }
    }

    fn terminal_row(&self, terminal: &SessionTerminal) -> RowSnapshot {
        let label = if self.renamed_terminals.contains(&terminal.id) {
            terminal.name.clone()
        } else {
            terminal
                .title
                .clone()
                .unwrap_or_else(|| terminal.name.clone())
        };
        let mut marks = Vec::new();
        if terminal.has_unseen_output {
            marks.push("unseen".to_owned());
        }
        if terminal.agent_attention.is_some() {
            marks.push("needs_you".to_owned());
        }
        RowSnapshot {
            id: terminal.id.to_string(),
            label,
            badges: vec![
                if terminal.is_native() {
                    "native"
                } else {
                    "pty"
                }
                .to_owned(),
            ],
            marks,
        }
    }

    /// The visible terminal as text. Absent when no mirror is on screen — it is the largest
    /// field in the snapshot and there is nothing to say about a surface that is not showing.
    fn terminal_snapshot(&self) -> Option<TerminalSnapshot> {
        let (_, grid) = self.harness_grid()?;
        let rows = grid.harness_rows();
        Some(TerminalSnapshot {
            text: rows.join("\n"),
            rows,
            cursor: CursorSnapshot {
                row: grid.cursor.row.into(),
                col: grid.cursor.col.into(),
                shape: cursor_shape_name(grid.cursor).to_owned(),
            },
            viewport: ViewportSnapshot {
                top: saturating_u32(grid.viewport.offset),
                rows: grid.rows.into(),
                history: saturating_u32(grid.viewport.scrollback_len),
            },
        })
    }
}

/// One tab-strip row plus whether it is an agent thread rather than a terminal.
struct TabRow {
    row: RowSnapshot,
    agent: bool,
}

/// The tab strip and which row is selected.
struct TabRows {
    rows: Vec<TabRow>,
    selected: Option<usize>,
}

fn list(rows: Vec<RowSnapshot>, cursor: usize, filter: String) -> ListSnapshot {
    ListSnapshot {
        selected: rows.get(cursor).cloned(),
        rows,
        filter,
    }
}

fn card_row(prefix: &str, card: &Card) -> RowSnapshot {
    let mut marks = Vec::new();
    if let Some(assignee) = card.assignee.as_ref() {
        marks.push(assignee.clone());
    }
    RowSnapshot {
        id: card.id.as_str().to_owned(),
        label: card.title.clone(),
        badges: vec![format!("{prefix}-{}", card.number)],
        marks,
    }
}

/// Narrows a count for the wire without ever wrapping it into a smaller, wrong number.
fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn screen_name(screen: &Screen) -> &'static str {
    match screen {
        Screen::Hub { .. } => "Hub",
        Screen::Workspace { .. } => "Workspace",
    }
}
fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "Normal",
        Mode::Terminal => "Terminal",
        Mode::Native => "Native",
        Mode::Agent => "Agent",
        Mode::Prefix => "Prefix",
        Mode::Scroll => "Scroll",
        Mode::Filter => "Filter",
        Mode::Palette => "Palette",
        Mode::Dialog => "Dialog",
        Mode::Jobs => "Jobs",
    }
}
fn hub_pane_name(pane: HubPane) -> &'static str {
    match pane {
        HubPane::Repos => "Repos",
        HubPane::List => "List",
    }
}
fn hub_tab_name(tab: HubTab) -> &'static str {
    match tab {
        HubTab::Worktrees => "Worktrees",
        HubTab::Prs => "Prs",
        HubTab::Board => "Board",
    }
}
fn agent_popup_mode_name(mode: AgentPopupMode) -> &'static str {
    match mode {
        AgentPopupMode::Terminal => "Terminal",
        AgentPopupMode::Prefix => "Prefix",
        AgentPopupMode::Scroll => "Scroll",
    }
}
fn overlay_name(overlay: &Overlay) -> &'static str {
    match overlay {
        Overlay::Filter => "Filter",
        Overlay::Palette => "Palette",
        Overlay::Jobs => "Jobs",
        Overlay::Dialog(_) => "Dialog",
    }
}
/// The §2.7 toast tones, in the UX vocabulary `docs/TESTING-HARNESS.md` §3 freezes.
///
/// `Danger` cannot reach a live toast — `push_toast` downgrades it, because errors are
/// sticky and never transient (§1.8) — but the mapping is total so a future tone cannot silently
/// become `info`.
fn toast_level_name(tone: fleet_ui_kit::Tone) -> &'static str {
    use fleet_ui_kit::Tone;
    match tone {
        Tone::Success => "success",
        Tone::Warning => "warning",
        Tone::Danger => "error",
        Tone::Default
        | Tone::Secondary
        | Tone::Muted
        | Tone::Accent
        | Tone::Info
        | Tone::Inverse => "info",
    }
}
fn job_status_name(status: &fleet_proto::job::JobStatus) -> &'static str {
    use fleet_proto::job::JobStatus;
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Cancelling => "cancelling",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed { .. } => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}
fn provider_name(provider: AgentKind) -> &'static str {
    match provider {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
}
/// The attention table of `docs/NATIVE-AGENTS.md` §3.3, as one word per row.
fn attention_name(attention: Attention) -> &'static str {
    match attention {
        Attention::NeedsYou(_) => "needs_you",
        Attention::Working => "working",
        Attention::Waiting => "waiting",
        Attention::Failed => "failed",
        Attention::Unread => "unread",
        Attention::Idle => "idle",
    }
}
/// The gate a thread is blocked on. A finished turn needs the user but is not a gate.
fn pending_gate_name(attention: Attention) -> Option<&'static str> {
    match attention {
        Attention::NeedsYou(AttentionKind::Permission) => Some("permission"),
        Attention::NeedsYou(AttentionKind::Question) => Some("question"),
        Attention::NeedsYou(AttentionKind::Plan) => Some("plan"),
        Attention::NeedsYou(AttentionKind::Finished)
        | Attention::Working
        | Attention::Waiting
        | Attention::Failed
        | Attention::Unread
        | Attention::Idle => None,
    }
}
fn cursor_shape_name(cursor: fleet_proto::terminal::CursorState) -> &'static str {
    use fleet_proto::terminal::CursorShape;
    if !cursor.visible {
        return "hidden";
    }
    match cursor.shape {
        CursorShape::Block => "block",
        CursorShape::Bar => "bar",
        CursorShape::Underline => "underline",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use fleet_core::ids::TerminalId;
    use fleet_proto::job::JobStatus;

    use super::super::tests::{job, state};
    use crate::dialogs::Dialogs;
    use crate::state::{JobFilter, JobsPanelMirror, Overlay, Screen, test_support};

    #[test]
    fn jobs_panel_cursor_changes_focus_and_selected_row() {
        let now = Instant::now();
        let mut state = state();
        let mut snapshot = test_support::snapshot();
        snapshot.jobs.push(job("job-1", JobStatus::Running));
        snapshot.jobs.push(job("job-2", JobStatus::Succeeded));
        state.apply_snapshot(snapshot, now);
        state.open_overlay(Overlay::Jobs);

        let before = state.harness_projection();
        assert_eq!(before.snapshot.focused.as_deref(), Some("jobs.row[0]"));
        assert_eq!(
            before
                .snapshot
                .lists
                .get("jobs")
                .and_then(|jobs| jobs.selected.as_ref())
                .map(|row| row.id.as_str()),
            Some("job-1")
        );

        assert!(state.set_jobs_panel(JobsPanelMirror {
            cursor: 1,
            filter: JobFilter::All,
        }));
        let after = state.harness_projection();

        assert_eq!(after.snapshot.focused.as_deref(), Some("jobs.row[1]"));
        assert_eq!(
            after
                .snapshot
                .lists
                .get("jobs")
                .and_then(|jobs| jobs.selected.as_ref())
                .map(|row| row.id.as_str()),
            Some("job-2")
        );
        assert!(after.revision > before.revision);
    }

    #[test]
    fn jobs_panel_filter_rebases_the_projected_rows() {
        let now = Instant::now();
        let mut state = state();
        let mut snapshot = test_support::snapshot();
        snapshot.jobs.push(job("job-1", JobStatus::Running));
        snapshot.jobs.push(job(
            "job-2",
            JobStatus::Failed {
                error: "boom".to_owned(),
            },
        ));
        snapshot.jobs.push(job("job-3", JobStatus::Succeeded));
        state.apply_snapshot(snapshot, now);
        state.open_overlay(Overlay::Jobs);
        let all = state.harness_projection().snapshot;
        let all_jobs = all.lists.get("jobs").expect("jobs list");
        assert_eq!(all_jobs.rows.len(), 3);
        assert_eq!(all_jobs.filter, "");

        assert!(state.set_jobs_panel(JobsPanelMirror {
            cursor: 0,
            filter: JobFilter::Failed,
        }));
        let dump = state.harness_projection().snapshot;
        let jobs = dump.lists.get("jobs").expect("jobs list");

        assert_eq!(dump.focused.as_deref(), Some("jobs.row[0]"));
        assert_eq!(jobs.filter, "failed");
        assert_eq!(
            jobs.rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["job-2"]
        );
        assert_eq!(
            jobs.selected.as_ref().map(|row| row.id.as_str()),
            jobs.rows.first().map(|row| row.id.as_str()),
            "jobs.row[0] and lists.jobs.rows[0] address the same filtered job"
        );
    }

    #[test]
    fn rename_only_change_rebuilds_the_projection() {
        let now = Instant::now();
        let mut state = state();
        let session_id = "buk/payroll#feat";
        let mut snapshot = test_support::snapshot();
        let mut session = test_support::session_with(session_id, &[1]);
        session.terminals[0].title = Some("shell title".to_owned());
        snapshot.sessions.push(session);
        state.apply_snapshot(snapshot, now);
        state.screen = Screen::Workspace {
            session: session_id.parse().unwrap_or_else(|error| panic!("{error}")),
        };

        let before = state.harness_projection();
        assert_eq!(
            before
                .snapshot
                .lists
                .get("tabs")
                .and_then(|tabs| tabs.rows.first())
                .map(|row| row.label.as_str()),
            Some("shell title")
        );
        state.renamed_terminals.insert(TerminalId(1));
        let after = state.harness_projection();

        assert_eq!(state.harness_builds(), 2);
        assert_eq!(
            after
                .snapshot
                .lists
                .get("tabs")
                .and_then(|tabs| tabs.rows.first())
                .map(|row| row.label.as_str()),
            Some("t1")
        );
        assert!(after.revision > before.revision);
    }

    #[test]
    fn settle_expiry_rebuilds_a_cached_busy_projection() {
        let state = state();
        let generation = state.harness.settle().begin(None);
        let busy = state.harness_projection();

        assert_eq!(busy.snapshot.idle.settling_mutations, 1);
        assert!(!busy.snapshot.idle.idle);
        assert!(state.harness.settle().expire(generation));

        let idle = state.harness_projection();
        assert_eq!(state.harness_builds(), 2);
        assert_eq!(idle.snapshot.idle.settling_mutations, 0);
        assert!(idle.snapshot.idle.link_opening);
        assert!(idle.revision > busy.revision);
    }

    #[test]
    fn open_dialog_keeps_unmirrored_fields_and_message_empty() {
        let mut state = state();
        state.open_overlay(Overlay::Dialog(Dialogs::CreateWorktree));

        let dump = state.harness_projection().snapshot;
        let dialog = dump.dialog.as_ref().expect("open dialog snapshot");

        assert!(dialog.fields.is_empty());
        assert!(dialog.message.is_none());
    }
}
