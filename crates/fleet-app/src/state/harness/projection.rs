//! Builds the harness projection and memoises it against everything it reads.
//!
//! The builder lives beside the types it produces rather than inside them because it is the
//! half that has to stay honest about *inputs*: [`ProjectionKey`] is the list of every piece of
//! [`AppState`] the projection touches, and the memo is only as fresh as that list is complete.
//! The Jobs panel remains authoritative for its cursor and filter; this projection reads their
//! explicitly synchronized [`crate::state::JobsPanelMirror`] rather than owning another copy.
//! Nothing here is reachable from a `render` body (`docs/APP-CONTRACTS.md`).

mod agents;

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use fleet_core::{
    agents::{AgentKind, Attention, AttentionKind, DelegationStatus, GateKind},
    board::{BoardView, Card, CardRun, RunOutcome},
    ids::TerminalId,
    sessions::Terminal as SessionTerminal,
};
use fleet_ui_kit::RunMark;

use super::{
    AgentThreadDecisionSnapshot, AgentThreadSnapshot, AgentsSnapshot, CursorSnapshot,
    DaemonSnapshot, DelegationSnapshot, DialogSnapshot, HarnessProjection, IdleSnapshot,
    JobSnapshot, ListSnapshot, RowSnapshot, SNAPSHOT_VERSION, TerminalSnapshot, ToastSnapshot,
    UiSnapshot, ViewportSnapshot, WindowSnapshot,
};
use crate::dialogs::Dialogs;
use crate::presentation::{DisplayedHub, selected_worktree_id};
use crate::state::{
    AgentPopupMode, AppState, BoardFocus, Cursors, DaemonLink, FilterState, HubPane, HubTab,
    JobsPanelMirror, LiveToast, Mode, Overlay, RepoScope, Screen, StickyError, board::TileMarks,
    running_jobs,
};
use crate::views::board_card_detail as detail;

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
    delegations_revision: u64,
    attached_revision: u64,
    board_revision: u64,
    /// The tile-mark fold's revision (contracts §5.2).
    ///
    /// The marks are derived from the board *and* from the clock: a pending run that has waited
    /// long enough turns `stalled` on the tick that re-derives them, with no board revision and
    /// no delegation behind it. Without this input that mark would never reach a waiting
    /// `await`.
    marks: u64,
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
            delegations_revision: self.agents.delegations_revision(),
            attached_revision: self.agents.attached_revision(),
            board_revision: self.board.revision,
            marks: self.board.marks.revision,
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
            Screen::Workspace { .. } if self.board_pane_is_active() => Some(format!(
                // The board pane's rows are the Hub board's rows, drawn by the same view, so a
                // scenario that drives one drives the other with the same vocabulary
                // (`docs/TESTING-HARNESS.md` §3).
                "board.column[{}].card[{}]",
                self.board.focus.column, self.board.focus.row
            )),
            Screen::Workspace { .. } => {
                let tabs = self.tab_rows();
                let index = tabs.selected?;
                Some(if tabs.rows.get(index).is_some_and(|row| row.agent) {
                    self.active_agent_thread().map_or_else(
                        || format!("agents.tabs.tab[{index}]"),
                        |thread| {
                            if self.agents.composer_is_focused(thread) {
                                "agents.composer".to_owned()
                            } else {
                                format!("agents.tabs.tab[{index}]")
                            }
                        },
                    )
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

    /// The pane header's two composed counts, as one row, or `None` while it says neither.
    ///
    /// Word for word `views::board_screen::model::HeaderFacts`'s `working_label` and
    /// `needs_you_label` — the harness reads the sentence the user reads — joined with the
    /// separator the board's own chrome uses, because a snapshot row is one string where the
    /// header is two labels in a gap. A board with nothing running and nobody waiting carries
    /// no counts at all, so the list is absent rather than a row saying `0/1 working`.
    fn board_summary_row(&self, view: &BoardView) -> Option<RowSnapshot> {
        let marks = &self.board.marks;
        let limit = view.board.settings.max_live_runs();
        let mut parts = Vec::new();
        if marks.working > 0 {
            parts.push(format!("{}/{limit} working", marks.working));
        }
        if marks.needs_you > 0 {
            parts.push(format!("{} needs you", marks.needs_you));
        }
        (!parts.is_empty()).then(|| RowSnapshot {
            id: "summary".to_owned(),
            label: parts.join(" \u{b7} "),
            badges: Vec::new(),
            marks: Vec::new(),
        })
    }

    /// One row per run of the open card, oldest first, as the detail draws them.
    ///
    /// Every label is [`detail::run_line`]'s own text rather than a second formatting of the
    /// same run: the dump and the screen would otherwise drift apart a word at a time. That
    /// function states the card's *latest* run, so the card is cloned once and truncated back
    /// one run at a time to ask it about each — a clone the harness pays for only while the
    /// detail is open and only when something else already rebuilt the snapshot.
    ///
    /// The elapsed time inside a label is as of that rebuild: nothing keys on the wall clock,
    /// so a scenario awaits a row or its mark, never a duration.
    fn card_run_rows(&self, view: &BoardView, card: &Card) -> Vec<RowSnapshot> {
        if card.runs.is_empty() {
            return Vec::new();
        }
        let now = chrono::Utc::now().timestamp();
        // The tile's own mark belongs to the run the tile is about: the newest one, unless a
        // run is owed, in which case the mark is the *pending* run's and every row here reads
        // its own run instead.
        let tile_speaks_for_newest = card.pending_run.is_none();
        let newest = card.runs.len() - 1;
        let mut trimmed = card.clone();
        trimmed.pending_run = None;
        let mut rows = Vec::with_capacity(card.runs.len());
        for (index, run) in card.runs.iter().enumerate().rev() {
            trimmed.runs.truncate(index + 1);
            let live = self.live_run_status(view, run);
            let mark = if index == newest && tile_speaks_for_newest {
                self.board
                    .marks
                    .by_card
                    .get(&card.id)
                    .and_then(|marks| marks.run)
            } else {
                finished_run_mark(run)
            };
            let label = detail::run_line(&trimmed, mark, live, now)
                .map(|line| line.text.to_string())
                .unwrap_or_default();
            rows.push(RowSnapshot {
                id: run.id.to_string(),
                label,
                badges: vec![provider_name(run.provider).to_owned()],
                marks: mark
                    .map(run_mark_word)
                    .map(str::to_owned)
                    .into_iter()
                    .collect(),
            });
        }
        rows.reverse();
        rows
    }

    /// The status of a run the card still believes is live, or `None` once it has ended.
    ///
    /// The delegation mirror is asked first and the view's join second, which is the order
    /// `state::board`'s own fold uses: `live_runs` is as old as the last board response, while
    /// `DelegationChanged` keeps arriving between them.
    fn live_run_status(&self, view: &BoardView, run: &CardRun) -> Option<DelegationStatus> {
        if run.outcome.is_some() {
            return None;
        }
        self.agents
            .delegation(run.id)
            .map(|delegation| delegation.status)
            .or_else(|| {
                view.live_runs
                    .iter()
                    .find(|live| live.run == run.id)
                    .map(|live| live.status)
            })
    }

    /// The open dialog, by the name `docs/KEYMAP.md` gives its key context.
    ///
    /// Fields, buttons and the message body belong to the dialog host entity and are not
    /// mirrored into `AppState`; the fields and the message are recorded by the harness command
    /// that is about to project, and buttons stay empty because Fleet's dialogs paint none
    /// (`docs/TESTING-HARNESS.md` §3).
    fn dialog_snapshot(&self) -> Option<DialogSnapshot> {
        let Some(Overlay::Dialog(dialog)) = self.overlay.as_ref() else {
            return None;
        };
        Some(DialogSnapshot {
            name: dialog.context_name().to_owned(),
            // Recorded by the harness command that is about to project, because the live
            // editors belong to the dialog host entity rather than to `AppState`.
            fields: self.harness.dialog_fields().to_vec(),
            buttons: Vec::new(),
            // The Confirm dialog's consequence sentence; every other dialog answers `null`.
            message: self.harness.dialog_message().map(str::to_owned),
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
                    let pending_gate = self
                        .agents
                        .projection(summary.thread)
                        .and_then(|projection| projection.gates.last())
                        .map(|gate| pending_gate_name(&gate.kind))
                        .or_else(|| {
                            (self.active_agent_thread() != Some(summary.thread))
                                .then(|| match attention {
                                    Attention::NeedsYou(kind) => pending_attention_name(kind),
                                    Attention::Working
                                    | Attention::Waiting
                                    | Attention::Failed
                                    | Attention::Unread
                                    | Attention::Idle => None,
                                })
                                .flatten()
                        });
                    AgentThreadSnapshot {
                        id: summary.thread.to_string(),
                        provider: provider_name(summary.provider).to_owned(),
                        state: attention_name(attention).to_owned(),
                        unread: self.agents.seen(summary.thread) < summary.last_seq,
                        pending_gate: pending_gate.map(str::to_owned),
                        decision: self.agents.decision(summary.thread).map(|decision| {
                            AgentThreadDecisionSnapshot {
                                kind: decision.kind,
                                title: decision.title.clone(),
                                paths: decision.paths.clone(),
                                has_diff: decision.has_diff,
                            }
                        }),
                        parent: summary.parent.map(|parent| parent.to_string()),
                        attached: self.agents.is_attached(summary.thread),
                        delegation_rows: self
                            .agents
                            .projection(summary.thread)
                            .map_or(0, |projection| {
                                saturating_u32(
                                    projection
                                        .items
                                        .iter()
                                        .filter(|item| {
                                            matches!(item.kind, fleet_core::agents::ItemKind::Delegation { .. })
                                        })
                                        .count(),
                                )
                            }),
                        result_cards: self
                            .agents
                            .projection(summary.thread)
                            .map_or(0, |projection| {
                                saturating_u32(
                                    projection
                                        .items
                                        .iter()
                                        .filter(|item| {
                                            matches!(
                                                &item.kind,
                                                fleet_core::agents::ItemKind::UserMessage {
                                                    origin: fleet_core::agents::MessageOrigin::Delegation { .. },
                                                    ..
                                                }
                                            )
                                        })
                                        .count(),
                                )
                            }),
                        focused_row: self.agents.focused_row(summary.thread),
                        expanded_result_cards: self
                            .agents
                            .expanded_result_cards(summary.thread),
                    }
                })
                .collect(),
            delegations: self
                .agents
                .delegations()
                .into_iter()
                .map(|delegation| DelegationSnapshot {
                    id: delegation.id.to_string(),
                    status: delegation.status.word(),
                    caller: delegation.caller.to_string(),
                    child: delegation.child.to_string(),
                    delivery: delegation.delivery.word(),
                    headline: delegation.headline.clone(),
                })
                .collect(),
            decision: agents::decision_snapshot(self),
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
                    // The badge is the count the column header draws, so it counts the same
                    // population the pane shows: neither an archived card, which is in no
                    // column, nor one the filter is hiding.
                    let cards = crate::views::board_screen::visible_cards(
                        view,
                        &status.id,
                        &self.board.filter,
                    )
                    .len();
                    RowSnapshot {
                        id: status.id.as_str().to_owned(),
                        label: status.name.clone(),
                        badges: vec![cards.to_string()],
                        marks: column_marks(status),
                    }
                })
                .collect();
            // The column's cards exactly as the pane draws them: `visible_cards` is the same
            // call `AppState::select_card` counts positions with, so `rows[R]` and
            // `focused == board.column[C].card[R]` can never name two different cards, and a
            // filtered board reports the rows it is actually showing rather than all of them.
            let cards = view
                .board
                .statuses
                .get(self.board.focus.column)
                .map(|status| {
                    crate::views::board_screen::visible_cards(view, &status.id, &self.board.filter)
                        .into_iter()
                        .map(|card| {
                            card_row(
                                &view.board.prefix,
                                card,
                                self.board.marks.by_card.get(&card.id),
                            )
                        })
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
            if let Some(summary) = self.board_summary_row(view) {
                lists.insert(
                    "board.summary".to_owned(),
                    list(vec![summary], 0, String::new()),
                );
            }
            // Two dialogs project a list of their own, and both are drawn from the same loaded
            // board: the detail opens on the board's selected card (`card_detail::seed`) and
            // the settings dialog is seeded from the board's own columns
            // (`board_settings::persistence::seed`), so the builder reads what the dialog is
            // showing without reaching into the `DialogHost` entity it cannot see.
            match self.overlay.as_ref() {
                Some(Overlay::Dialog(Dialogs::CardDetail)) => {
                    if let Some(card) = crate::screens::board::selected_card(self) {
                        let runs = self.card_run_rows(view, card);
                        if !runs.is_empty() {
                            lists.insert("card.runs".to_owned(), unselected(runs));
                        }
                    }
                }
                Some(Overlay::Dialog(Dialogs::BoardSettings)) => {
                    lists.insert(
                        "settings.columns".to_owned(),
                        unselected(settings_column_rows(view)),
                    );
                }
                _ => {}
            }
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
        if matches!(self.overlay, Some(Overlay::Palette)) {
            let rows = self.palette_rows();
            lists.insert(
                "palette".to_owned(),
                ListSnapshot {
                    selected: None,
                    rows,
                    filter: "agents".to_owned(),
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

    /// Every job the panel shows, in the order it draws them (`JobFilter::visible`).
    fn job_rows(&self, snapshot: &fleet_proto::snapshot::Snapshot) -> Vec<RowSnapshot> {
        self.jobs_panel
            .filter
            .visible(&snapshot.jobs)
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
                        badges: if summary.parent.is_some() {
                            vec![
                                provider_name(summary.provider).to_owned(),
                                "child".to_owned(),
                            ]
                        } else {
                            vec![provider_name(summary.provider).to_owned()]
                        },
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

    /// Native-thread rows exposed while the agent picker owns the palette.
    fn palette_rows(&self) -> Vec<RowSnapshot> {
        let current = selected_worktree_id(self);
        let summaries = self.agents.summaries();
        let callers: Vec<_> = summaries
            .iter()
            .filter(|summary| {
                summary.parent.is_none()
                    && current
                        .as_ref()
                        .is_none_or(|worktree| &summary.worktree == worktree)
            })
            .collect();
        let caller_ids: HashSet<_> = callers.iter().map(|summary| summary.thread).collect();
        let children: Vec<_> = summaries
            .iter()
            .filter(|summary| {
                summary
                    .parent
                    .is_some_and(|parent| caller_ids.contains(&parent))
            })
            .collect();

        callers
            .into_iter()
            .chain(children.iter().copied().filter(|summary| {
                current
                    .as_ref()
                    .is_none_or(|worktree| &summary.worktree == worktree)
            }))
            .chain(children.iter().copied().filter(|summary| {
                current
                    .as_ref()
                    .is_some_and(|worktree| &summary.worktree != worktree)
            }))
            .map(|summary| {
                let child = summary.parent.is_some();
                let attached = self.agents.is_attached(summary.thread);
                let other_worktree =
                    child && current.as_ref().is_some_and(|id| id != &summary.worktree);
                let mut label = crate::screens::agent_thread::presentation::tab_title(summary);
                if other_worktree {
                    label.push_str(" · ");
                    label.push_str(summary.worktree.as_str());
                }
                RowSnapshot {
                    id: summary.thread.to_string(),
                    label,
                    badges: vec![
                        provider_name(summary.provider).to_owned(),
                        if child { "child" } else { "caller" }.to_owned(),
                        summary.worktree.as_str().to_owned(),
                    ],
                    marks: vec![
                        attention_name(self.agents.attention(summary.thread)).to_owned(),
                        if attached || !child { "go" } else { "attach" }.to_owned(),
                        if attached { "attached" } else { "hidden" }.to_owned(),
                    ],
                }
            })
            .collect()
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

/// A list nothing is selected in, for the two dialog surfaces that have no cursor of their own.
fn unselected(rows: Vec<RowSnapshot>) -> ListSnapshot {
    ListSnapshot {
        selected: None,
        rows,
        filter: String::new(),
    }
}

/// One card row, with the marks its tile draws in the order the tile draws them.
///
/// The run mark leads, the blocked count follows and the assignee — which says who owns the
/// card rather than what is happening to it — comes last, so `marks[0]` is the state a
/// workflow scenario is waiting for whether or not anybody is assigned. `CardTile` shows only
/// one of the first two on the key line, the run mark winning; the snapshot states both,
/// because a dump is read for facts rather than for space.
fn card_row(prefix: &str, card: &Card, marks: Option<&TileMarks>) -> RowSnapshot {
    let mut row_marks = Vec::new();
    if let Some(marks) = marks {
        row_marks.extend(marks.run.map(run_mark_word).map(str::to_owned));
        row_marks.extend(marks.blocked.map(|(count, _)| format!("blocked:{count}")));
    }
    if let Some(assignee) = card.assignee.as_ref() {
        row_marks.push(assignee.clone());
    }
    RowSnapshot {
        id: card.id.as_str().to_owned(),
        label: card.title.clone(),
        badges: vec![format!("{prefix}-{}", card.number)],
        marks: row_marks,
    }
}

/// `action` for a column that starts a run on arrival, and nothing otherwise.
///
/// Only `on_enter` earns the mark, exactly as `KanbanColumn`'s `\u{26a1}` does: `on_success` and
/// `advance_when_unblocked` move a card the column has already finished with, and a mark
/// promising a run for either would lie.
fn column_marks(status: &fleet_core::board::Status) -> Vec<String> {
    if status
        .automation
        .as_ref()
        .is_some_and(|automation| automation.on_enter.is_some())
    {
        vec!["action".to_owned()]
    } else {
        Vec::new()
    }
}

/// The Board settings Columns pane, as the board it is seeded from states it.
///
/// `disabled` is the draft's `automation_locked` (`board_settings::persistence::seed`): a
/// context board has no checkout to run in and a linked board's columns answer to its backend,
/// so every column of such a board draws its automation rows disabled. It rides on the column
/// rather than on the drilled-in row because the drill-in belongs to the `DialogHost` entity,
/// which the builder cannot read.
fn settings_column_rows(view: &BoardView) -> Vec<RowSnapshot> {
    let locked = view.board.worktree_id.is_none() || !view.board.backend.is_local();
    view.board
        .statuses
        .iter()
        .map(|status| {
            let mut marks = column_marks(status);
            if locked {
                marks.push("disabled".to_owned());
            }
            RowSnapshot {
                id: status.id.as_str().to_owned(),
                label: status.name.clone(),
                badges: Vec::new(),
                marks,
            }
        })
        .collect()
}

/// The mark a finished run of an older generation carries.
///
/// The card's own fold states the newest run alone, because that is the one the tile is about.
/// An older row is read from its outcome by the same table (`state::board`'s fold): a run that
/// was cancelled leaves no mark, since somebody stopped it deliberately and a mark would
/// report a decision as an event.
fn finished_run_mark(run: &CardRun) -> Option<RunMark> {
    match run.outcome? {
        RunOutcome::Cancelled => None,
        RunOutcome::Succeeded => Some(RunMark::Succeeded),
        RunOutcome::NeedsYou | RunOutcome::Failed | RunOutcome::Incomplete => {
            Some(RunMark::NeedsYou)
        }
    }
}

/// The mark vocabulary `docs/TESTING-HARNESS.md` §3 freezes, one word per mark.
fn run_mark_word(mark: RunMark) -> &'static str {
    match mark {
        RunMark::Pending => "pending",
        RunMark::Stalled => "stalled",
        RunMark::Working => "working",
        RunMark::NeedsYou => "needs you",
        RunMark::Succeeded => "done",
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
/// The gate a thread's installed projection can act on.
fn pending_gate_name(gate: &GateKind) -> &'static str {
    match gate {
        GateKind::Permission { .. } => "permission",
        GateKind::Question { .. } => "question",
        GateKind::Plan { .. } => "plan",
    }
}

fn pending_attention_name(attention: AttentionKind) -> Option<&'static str> {
    match attention {
        AttentionKind::Permission => Some("permission"),
        AttentionKind::Question => Some("question"),
        AttentionKind::Plan => Some("plan"),
        AttentionKind::Finished => None,
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
mod tests;
