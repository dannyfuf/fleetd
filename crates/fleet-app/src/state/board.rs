use super::*;
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{DelegationId, DelegationStatus},
    board::{
        BlockedTone as CoreBlockedTone, Board, BoardView, Card, CardRun, PENDING_AMBER_AFTER_SECS,
        PendingRun, RunOutcome, attention, blocked,
    },
    config::NATIVE_BOARD,
    ids::CardId,
};
use fleet_proto::response::BOARD_WORKTREE_CAPABILITY;
use fleet_ui_kit::{BlockedTone, RunMark};

/// What the app says when the connected daemon serves no worktree boards.
///
/// Word for word the CLI's refusal (`fleet-client`'s dispatch check), because the remedy is the
/// same one: a user who has read the sentence once should not have to recognise a second
/// wording for the same daemon being too old.
pub const WORKTREE_BOARDS_UNSUPPORTED: &str =
    "this daemon does not support worktree boards; run `fleet daemon restart`";

/// Which board the single mirror is pointed at (BOARD §8).
///
/// The Hub and the Workspace are never visible at the same time, so one [`BoardState`] with a
/// scope serves both surfaces. Neither half is ever turned into a board id here: a context's
/// board and a worktree's board are each asked for by their own request, and the daemon's
/// answer is what says which board that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardScope {
    /// The active context's own board — `EnsureBoard(context)`.
    Context(ContextId),
    /// One worktree's board — `EnsureWorktreeBoard(worktree)`.
    Worktree(WorktreeId),
}

/// Keyboard selection within the board's status columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoardFocus {
    /// Zero-based status column.
    pub column: usize,
    /// Zero-based card within the column.
    pub row: usize,
}

/// Optional secondary grouping beneath a status column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    /// Group by priority.
    Priority,
    /// Group by assignee.
    Assignee,
    /// Group by label.
    Labels,
}

/// What one tile says about its card's run and its blockers.
///
/// Kit vocabulary, not domain types: [`RunMark`] and [`BlockedTone`] are what `CardTile` draws,
/// so the fold from `CardRun`, `PendingRun` and the delegation mirror happens once, in
/// [`AppState::refresh_card_marks`], instead of once per tile per frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileMarks {
    /// The run mark this card's tile draws, when it has one.
    pub run: Option<RunMark>,
    /// How many cards still block it, and how loudly to say so.
    pub blocked: Option<(u32, BlockedTone)>,
}

/// Every tile mark of the shown board, derived once per change (contracts §5.2).
///
/// The two counters are the pane header's trailing slot — `{working}/{max} working` and
/// `{needs_you} needs you` — and are folded here rather than in the header, because the same
/// walk over the cards already answers both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardMarks {
    /// The marks of every card that has one; a card with neither mark is absent.
    pub by_card: HashMap<CardId, TileMarks>,
    /// Cards with a live or pending run, which is the header's numerator.
    pub working: u32,
    /// Cards waiting on a person (`ops::query::attention`).
    pub needs_you: u32,
    /// Bumped only when the map or a counter actually differs.
    ///
    /// The board projection is keyed on it, so a tick that changes no mark must not rebuild a
    /// whole board's model.
    pub revision: u64,
}

/// The shown board's data and local presentation state (BOARD §8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardState {
    /// Which board this mirror is pointed at, once a surface has claimed it.
    ///
    /// `None` is the Hub's default — the active context's own board — which is what
    /// [`AppState::board_scope`] resolves it to and what the first load records here.
    pub scope: Option<BoardScope>,
    /// Last authoritative board response.
    pub view: Option<BoardView>,
    /// Whether an EnsureBoard request is in flight.
    pub loading: bool,
    /// Most recent load failure, retained until an explicit retry.
    pub error: Option<String>,
    /// Keyboard selection.
    pub focus: BoardFocus,
    /// Card substring filter.
    pub filter: String,
    /// Whether the filter input still owns the keyboard (§3.10's two-stage `Esc`).
    ///
    /// The board is the one list whose filter is not the Hub's [`FilterState`]: its rows are
    /// cards in columns, not worktrees, so `Overlay::Filter` — which moves the worktree cursor
    /// and opens a worktree on `Enter` — cannot serve it. The screen publishes the `Filter` key
    /// context while this is set, which is what makes the bare letters type instead of fire.
    pub filter_editing: bool,
    /// Optional secondary grouping.
    pub group_secondary: Option<GroupBy>,
    /// The tile marks derived from [`Self::view`] and the delegation mirror.
    pub marks: CardMarks,
    /// A card to select as soon as a view arrives that holds it.
    ///
    /// `^s u` from a card run's thread names a card on a board this mirror has not loaded yet:
    /// the tab is asked for first and the view lands frames later, so the selection waits here
    /// rather than being dropped on the floor by a `focus_card` over an empty mirror.
    pub pending_focus: Option<CardId>,
    /// Bumped by every mutation of [`Self::view`].
    ///
    /// The board screen memoises its whole derived model behind this counter, so it has to move
    /// on every applied card and every applied view — `AppState::snapshot_revision` does not,
    /// because a card edit lands through [`AppState::apply_card`] and never through a snapshot.
    pub revision: u64,
}

impl BoardState {
    /// Whether cards are currently being filtered.
    #[must_use]
    fn is_filtered(&self) -> bool {
        !self.filter.trim().is_empty()
    }

    /// Records that [`Self::view`] now holds something else than it did.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

impl AppState {
    /// The board the mirror currently holds, whatever scope it is pointed at.
    #[must_use]
    pub fn board(&self) -> Option<&BoardView> {
        self.board.view.as_ref()
    }

    /// Which board the mirror is pointed at: the scope a surface set, or the Hub's default.
    #[must_use]
    pub(crate) fn board_scope(&self) -> Option<BoardScope> {
        self.board
            .scope
            .clone()
            .or_else(|| self.active_context().cloned().map(BoardScope::Context))
    }

    /// Whether `view` is the board the mirror is pointed at.
    ///
    /// A worktree's board is recognised by its worktree; a context's board is the one with that
    /// context and **no** worktree, which is the daemon's own rule (BOARD §0) and the reason a
    /// worktree board can never land in the Hub's slot although both name the same context.
    #[must_use]
    fn board_scope_admits(&self, view: &BoardView) -> bool {
        match self.board_scope() {
            Some(BoardScope::Context(context)) => {
                view.board.context_id == context && view.board.worktree_id.is_none()
            }
            Some(BoardScope::Worktree(worktree)) => {
                view.board.worktree_id.as_ref() == Some(&worktree)
            }
            None => false,
        }
    }

    /// Applies an authoritative board view; responses for another scope are ignored.
    pub fn apply_board_view(&mut self, view: BoardView) {
        if !self.board_scope_admits(&view) {
            return;
        }
        // Read before the swap: a run whose start failed is only news against the board this
        // mirror already held, and after the assignment there is nothing left to compare to.
        let previous = self.newest_run_ids();
        self.board_stale = false;
        self.board.view = Some(view);
        self.board.loading = false;
        self.board.error = None;
        self.board.touch();
        self.clamp_board_focus();
        if let Some(card) = self.board.pending_focus.take() {
            self.select_card(&card);
        }
        if let Some(previous) = previous {
            self.report_failed_start(&previous);
        }
        self.refresh_card_marks(Utc::now());
    }

    /// The newest run of every card the mirror holds, or `None` when it holds no board.
    ///
    /// `None` is what makes a first load silent: with no board behind it every run id is new,
    /// and a failure the user has already been told about — or one from another session
    /// entirely — is not worth a sticky error the moment the board appears.
    fn newest_run_ids(&self) -> Option<HashMap<CardId, DelegationId>> {
        let view = self.board.view.as_ref()?;
        Some(
            view.cards
                .iter()
                .filter_map(|card| card.runs.last().map(|run| (card.id.clone(), run.id)))
                .collect(),
        )
    }

    /// Raises the sticky slot for a run that is new to this view and never reached a thread.
    ///
    /// A start that fails leaves no delegation and no child to attach to, so nothing else on
    /// the board would ever say why the card stopped: the tile's mark goes straight back to
    /// nothing (contracts §5.2). Comparing run ids is what makes it fire once — the next view
    /// carries the same newest run and says nothing again.
    fn report_failed_start(&mut self, previous: &HashMap<CardId, DelegationId>) {
        let Some(view) = self.board.view.as_ref() else {
            return;
        };
        let failure = view.cards.iter().find_map(|card| {
            let run = card.runs.last()?;
            if !run.failed_to_start() || previous.get(&card.id) == Some(&run.id) {
                return None;
            }
            Some(run.detail.clone().unwrap_or_else(|| {
                format!("{} could not start a run", card.display_key(&view.board))
            }))
        });
        if let Some(text) = failure {
            self.sticky_error = Some(StickyError {
                text,
                job: None,
                retryable: false,
            });
        }
    }

    /// Upserts a card only into its currently loaded board.
    pub fn apply_card(&mut self, card: Card) {
        let Some(view) = self.board.view.as_mut() else {
            return;
        };
        if view.board.id != card.board_id {
            return;
        }
        if !view
            .board
            .statuses
            .iter()
            .any(|status| status.id == card.status_id)
        {
            self.board_stale = true;
            return;
        }
        if let Some(current) = view.cards.iter_mut().find(|current| current.id == card.id) {
            *current = card;
        } else {
            view.cards.push(card);
        }
        view.cards.sort_by(|a, b| {
            (&a.status_id, a.position, &a.created_at, a.number).cmp(&(
                &b.status_id,
                b.position,
                &b.created_at,
                b.number,
            ))
        });
        self.board.touch();
        self.clamp_board_focus();
        self.refresh_card_marks(Utc::now());
    }

    /// Applies the daemon's answer to a move that named a place in the column.
    ///
    /// The answer is the moved card alone, but `ops::move_card` renumbered every card of the
    /// destination column. Replaying that same function over the shown cards puts the
    /// neighbours where the daemon did, so the column never draws a tie between the card and
    /// the one it was dropped above while the `BoardChanged` reload is on its way.
    pub fn apply_placed_card(&mut self, card: Card, index: usize) {
        if let Some(view) = self.board.view.as_mut()
            && view.board.id == card.board_id
            && let Err(error) = fleet_core::board::move_card(
                &view.board,
                &mut view.cards,
                &card.id,
                &card.status_id,
                Some(index),
                &card.updated_at,
            )
        {
            // The shown board disagrees with the daemon's; the reload it triggers settles it.
            tracing::debug!(%error, "replaying a placed move over the shown board failed");
            self.board_stale = true;
        }
        self.apply_card(card);
    }

    /// Re-derives [`BoardState::marks`] from the shown board and the delegation mirror.
    ///
    /// Called after `apply_board_view`, after `apply_card`, after `apply_delegation` for a
    /// card-called delegation on the shown board, and from the app's existing clock tick (the
    /// one that runs `expire_toasts`), so `Stalled` appears without an event. `now` is
    /// formatted to RFC 3339 once for `ops::query::attention`, and `revision` moves only when
    /// the derived marks actually differ (contracts §5.2).
    pub fn refresh_card_marks(&mut self, now: DateTime<Utc>) {
        let next = self.derive_card_marks(now);
        if next != self.board.marks {
            let revision = self.board.marks.revision.wrapping_add(1);
            self.board.marks = next;
            self.board.marks.revision = revision;
        }
    }

    /// Folds the shown board into the marks its tiles and header draw.
    ///
    /// Carries the current revision so the caller compares the derived facts alone: the
    /// counter is the projection's rebuild key, and a tick that changes nothing must leave it
    /// where it was.
    fn derive_card_marks(&self, now: DateTime<Utc>) -> CardMarks {
        let mut marks = CardMarks {
            revision: self.board.marks.revision,
            ..CardMarks::default()
        };
        let Some(view) = self.board.view.as_ref() else {
            return marks;
        };
        // `attention` reads a stamp, not an instant, and every card compares against the same
        // one: formatting it per card would be both slower and a clock that moves mid-fold.
        let stamp = now.to_rfc3339();
        // One walk of the delegation mirror for the whole board, not one per card: the mirror
        // holds every session's delegations and the fold below already visits every card.
        let live_children = self.agents.live_card_runs(&view.board.id);
        for card in view.cards.iter().filter(|card| !card.archived) {
            // The card stays the authority on a run it has already finished: a mirror row that
            // has not caught up with its own terminal event says nothing about it.
            let live_child = live_children.get(&card.id).copied().filter(|(id, _)| {
                !card
                    .runs
                    .iter()
                    .any(|run| run.id == *id && run.outcome.is_some())
            });
            let run = self.run_mark(view, card, now, live_child.map(|(_, status)| status));
            let blocked = blocked(&view.board, &view.cards, card)
                .map(|waiting| (waiting.unsatisfied, tone_of(waiting.tone)));
            // The numerator is what occupies a run slot, which is the live and the owed runs —
            // not the marks, because a `Blocked` child is still holding its checkout. A child
            // the mirror has and the view has not is holding one too.
            if card.pending_run.is_some()
                || live_child.is_some()
                || card.runs.last().is_some_and(CardRun::is_live)
            {
                marks.working = marks.working.saturating_add(1);
            }
            if attention(card, &stamp) {
                marks.needs_you = marks.needs_you.saturating_add(1);
            }
            if run.is_some() || blocked.is_some() {
                marks
                    .by_card
                    .insert(card.id.clone(), TileMarks { run, blocked });
            }
        }
        marks
    }

    /// What one card's newest run says about it, or `None` when it says nothing.
    ///
    /// A live child the delegation mirror holds is read first, because it is the only fact that
    /// does not wait for a board round trip: the daemon records the run on the card and emits a
    /// `BoardChanged`, and the app answers that with a whole `EnsureWorktreeBoard`, so a run
    /// that lives five seconds can otherwise be over before the tile ever says `working`. The
    /// mirror's `DelegationChanged` lands in milliseconds and names the card that called it, so
    /// the mark covers the whole live window. An owed run wins over a finished one: the card is
    /// about to start again, and the mark that matters is the one that is still moving.
    fn run_mark(
        &self,
        view: &BoardView,
        card: &Card,
        now: DateTime<Utc>,
        live_child: Option<DelegationStatus>,
    ) -> Option<RunMark> {
        if let Some(status) = live_child {
            return Some(live_status_mark(status));
        }
        if let Some(pending) = card.pending_run.as_ref() {
            return Some(pending_mark(pending, now));
        }
        let run = card.runs.last()?;
        let Some(outcome) = run.outcome else {
            return Some(self.live_mark(view, run));
        };
        match outcome {
            // Someone stopped this deliberately; a tile that kept saying so would be reporting
            // a decision as an event.
            RunOutcome::Cancelled => None,
            // A column that advances on success says it by moving the card, so a check beside
            // the key would mark every card the workflow already carried on from.
            RunOutcome::Succeeded => {
                (!auto_advances(&view.board, run)).then_some(RunMark::Succeeded)
            }
            RunOutcome::NeedsYou | RunOutcome::Failed | RunOutcome::Incomplete => {
                Some(RunMark::NeedsYou)
            }
        }
    }

    /// The mark of a run the card still believes is live.
    ///
    /// The delegation mirror is asked first and the view's join second: `live_runs` is as old
    /// as the last board response, while `DelegationChanged` keeps arriving between them. A run
    /// neither of them knows is drawn as working — the card is the authority on whether it has
    /// ended, and until it says otherwise a child is out there.
    fn live_mark(&self, view: &BoardView, run: &CardRun) -> RunMark {
        let status = self
            .agents
            .delegation(run.id)
            .map(|delegation| delegation.status)
            .or_else(|| {
                view.live_runs
                    .iter()
                    .find(|live| live.run == run.id)
                    .map(|live| live.status)
            });
        status.map_or(RunMark::Working, live_status_mark)
    }

    /// Clears board data and invalidates requests from the previous context or connection.
    pub(super) fn clear_board(&mut self) {
        self.invalidate_board();
        // A reconnect can land on a different fleetd with a different registry, and the
        // descriptors are what the header and the settings dialog are drawn from. The list
        // itself is kept until a newer one arrives so the header's label does not flicker.
        self.board_backends_asked = false;
    }

    /// Drops the mirror and strands every response the slot it held still owed.
    ///
    /// The generation counter is the one invalidation mechanism the board has: bumping it is
    /// what makes a reply already in flight — from the previous context, the previous
    /// connection or the previous scope — land on a slot that is no longer its own.
    fn invalidate_board(&mut self) {
        if matches!(
            self.overlay,
            Some(Overlay::Dialog(
                Dialogs::CardDetail
                    | Dialogs::CardCreate
                    | Dialogs::CardPicker
                    | Dialogs::BoardSettings
            ))
        ) {
            self.close_overlay();
        }
        let revision = self.board.revision.wrapping_add(1);
        self.board = BoardState::default();
        self.board.revision = revision;
        self.board_stale = true;
        self.board_generation = self.board_generation.wrapping_add(1);
    }

    /// Points the mirror at another board; returns whether that changed which board is shown.
    ///
    /// Comparing against the *resolved* scope is what keeps the Hub still: pointing a mirror
    /// that has never been pointed anywhere at the active context is where it already is, so
    /// it keeps the view it loaded instead of paying for a round trip to redraw the same board.
    fn point_board_at(&mut self, scope: Option<BoardScope>) -> bool {
        let moved = self.board_scope() != scope;
        if moved {
            self.invalidate_board();
        }
        self.board.scope = scope;
        moved
    }

    /// Points the board at the active context's board, which is the one the Hub tab shows.
    ///
    /// Returns whether the mirror moved, so an observation that runs on every notify only
    /// notifies when something actually changed.
    pub(crate) fn enter_context_board_scope(&mut self) -> bool {
        let scope = self.active_context().cloned().map(BoardScope::Context);
        self.point_board_at(scope)
    }

    /// Points the board at one worktree's board, or refuses when this daemon has none.
    ///
    /// The refusal is a toast and not the sticky slot (§2.7): it is a fact about the daemon on
    /// the other end, not a failure of the keystroke, and the sentence carries its own remedy.
    /// `fleet-client` rechecks the capability at dispatch, so a connection that changes under
    /// this answer fails the request with the same sentence rather than hanging.
    ///
    /// A refusal reached with the board pane already on screen — a `fleet://board` tab the user
    /// put in `windows[]`, a daemon downgraded under a live one — also drops the mirror and
    /// writes the same sentence to [`BoardState::error`]. The pane draws whatever the mirror
    /// holds, and what the mirror holds is never this tab's board: without that it would draw
    /// skeleton columns for a load that can never go out, or the Hub's context board under a
    /// worktree's heading. `ctrl-s b` from a tab that is not the board's only toasts.
    pub(crate) fn enter_worktree_board_scope(
        &mut self,
        worktree: WorktreeId,
        now: Instant,
    ) -> bool {
        if !self.daemon_capabilities.contains(BOARD_WORKTREE_CAPABILITY) {
            self.toast(
                Toast::new(WORKTREE_BOARDS_UNSUPPORTED).icon(Icon::Info),
                now,
                dwell_for(ToastDuration::Normal),
            );
            if self.board_pane_is_active() {
                self.invalidate_board();
                self.board.error = Some(WORKTREE_BOARDS_UNSUPPORTED.to_owned());
            }
            return false;
        }
        self.point_board_at(Some(BoardScope::Worktree(worktree)));
        true
    }

    /// Whether a `ListBoardBackends` request should go out now, marking it as issued.
    pub(crate) fn begin_backends_load(&mut self) -> bool {
        if self.board_backends_asked || self.refuses_mutations() {
            return false;
        }
        self.board_backends_asked = true;
        true
    }

    /// Adopts the daemon's backend registry.
    pub(crate) fn apply_backends(&mut self, backends: Vec<BackendDescriptor>) {
        self.board_backends = backends;
    }

    /// Releases the once-per-connection guard so a failed ask can be made again.
    ///
    /// The flag is set before the request, so without this one refused `ListBoardBackends`
    /// leaves the header showing the raw kind and the settings dialog with no backend rows for
    /// the rest of the connection — a permanent consequence of a momentary failure.
    pub(crate) fn backends_load_failed(&mut self) {
        self.board_backends_asked = false;
    }

    /// The descriptor of one backend kind, when the daemon registers it.
    #[must_use]
    pub(crate) fn backend_descriptor(&self, kind: &str) -> Option<&BackendDescriptor> {
        self.board_backends
            .iter()
            .find(|descriptor| descriptor.kind == kind)
    }

    /// The human name of a backend kind, falling back to the raw key.
    ///
    /// The fallback matters on the first frames of a connection and against an older daemon:
    /// a header that draws nothing at all where the backend goes reads as a broken board.
    #[must_use]
    pub(crate) fn backend_label(&self, kind: &str) -> String {
        self.backend_descriptor(kind)
            .map_or_else(|| kind.to_owned(), |descriptor| descriptor.label.clone())
    }

    /// The standard card fields this board's backend cannot write back.
    ///
    /// Empty on a local board, exactly as `fleet_core::board::ops` reads it: a local board
    /// declares no backend, so nothing it holds is read-only.
    #[must_use]
    fn readonly_fields(&self) -> &[String] {
        self.board()
            .filter(|view| !view.board.backend.is_local())
            .map_or(&[][..], |view| &view.board.sync.readonly_fields)
    }

    /// Whether the daemon would refuse a local edit to `field` on the shown board.
    #[must_use]
    pub(crate) fn is_readonly_field(&self, field: &str) -> bool {
        self.readonly_fields()
            .iter()
            .any(|readonly| readonly == field)
    }

    /// Whether a surface that draws the board is showing.
    ///
    /// The Hub's tab is one. A worktree scope is the other: only the Workspace's board pane
    /// sets one, and it points the mirror back at the context when it goes away, so the scope
    /// itself says whether that pane is there to draw the answer.
    #[must_use]
    pub(super) fn board_is_shown(&self) -> bool {
        matches!(self.screen, Screen::Hub { tab: HubTab::Board })
            || matches!(self.board.scope, Some(BoardScope::Worktree(_)))
    }

    /// Whether the Workspace's `fleet://board` tab is the surface drawing the board.
    ///
    /// The reserved command decides it, not the tab's name, exactly as the Workspace's own
    /// model does: the daemon owns the tab list, and an agent tab shadows the terminal strip
    /// entirely, so neither of them may answer for a board pane that is not on screen.
    #[must_use]
    pub(crate) fn board_pane_is_active(&self) -> bool {
        matches!(self.screen, Screen::Workspace { .. })
            && self.active_agent_thread().is_none()
            && self
                .active_terminal_record()
                .is_some_and(|terminal| terminal.is_native() && terminal.command == NATIVE_BOARD)
    }

    /// The invalidation counter every board request in flight is stamped with.
    ///
    /// The Workspace's board pane records it beside the worktree it claimed the scope for, so
    /// a [`AppState::clear_board`] — a reconnect, a context switch — makes the claim stale and
    /// the pane points the mirror again, while a refusal the daemon has already explained is
    /// asked for exactly once.
    #[must_use]
    pub(crate) const fn board_generation(&self) -> u64 {
        self.board_generation
    }

    /// Claims one load; an error waits for reload instead of retrying every render.
    pub(crate) fn begin_board_load(&mut self) -> Option<(BoardScope, u64)> {
        if !self.board_is_shown() {
            return None;
        }
        if self.refuses_mutations() {
            // "Cold" means a load is in flight. With the daemon gone none ever will be, so the
            // board says why instead of drawing skeleton columns forever.
            if self.board.view.is_none() {
                self.board.error = Some("fleetd is not reachable".to_owned());
            }
            return None;
        }
        let Some(scope) = self.board_scope() else {
            // With no scope set the board is the active context's, so with no active context
            // there is nothing to ask for and skeleton columns would promise a load that never
            // goes out. `apply_snapshot` clears the board the moment one is activated, which
            // drops this message and makes the load stale again.
            if self.board.view.is_none() {
                self.board.error = Some(NO_ACTIVE_CONTEXT.to_owned());
            }
            return None;
        };
        if self.board.loading || !self.board_stale {
            return None;
        }
        // The Hub's default becomes explicit here: from the first load on, the mirror says
        // which board it holds rather than leaving it to be re-derived on every read.
        self.board.scope = Some(scope.clone());
        self.board.loading = true;
        self.board.error = None;
        self.board_stale = false;
        Some((scope, self.board_generation))
    }

    /// Completes a load only if its scope and generation still own the board slot.
    pub(crate) fn finish_board_load(
        &mut self,
        scope: &BoardScope,
        generation: u64,
        result: Result<BoardView, String>,
    ) {
        if generation != self.board_generation || self.board_scope().as_ref() != Some(scope) {
            return;
        }
        self.board.loading = false;
        match result {
            Ok(view) if self.board_scope_admits(&view) => {
                let stale = self.board_stale;
                self.apply_board_view(view);
                self.board_stale = stale;
            }
            Ok(_) => self.board.error = Some("the daemon answered with another board".into()),
            Err(error) => self.board.error = Some(error),
        }
    }

    /// Puts the board focus on `card`, wherever the current view puts it.
    ///
    /// The one implementation: `screens::board::focus_card` is this, and
    /// [`AppState::apply_board_view`] applies a staged selection through it too. A card the view
    /// does not hold leaves the focus where it was, clamped.
    pub(crate) fn select_card(&mut self, card: &CardId) {
        let found = self.board.view.as_ref().and_then(|view| {
            view.board
                .statuses
                .iter()
                .enumerate()
                .find_map(|(column, status)| {
                    crate::views::board_screen::visible_cards(view, &status.id, &self.board.filter)
                        .iter()
                        .position(|candidate| &candidate.id == card)
                        .map(|row| (column, row))
                })
        });
        if let Some((column, row)) = found {
            self.board.focus.column = column;
            self.board.focus.row = row;
        }
        self.clamp_board_focus();
    }

    /// Keeps the board selection inside the columns and rows that are actually drawn.
    ///
    /// The filter is part of that: a selection that indexes a hidden card is a selection the
    /// user cannot see, and every key that acts on "the focused card" would act on the wrong
    /// one.
    pub(crate) fn clamp_board_focus(&mut self) {
        let Some(view) = self.board.view.as_ref() else {
            return;
        };
        self.board.focus.column = clamp_cursor(self.board.focus.column, view.board.statuses.len());
        let len = view
            .board
            .statuses
            .get(self.board.focus.column)
            .map_or(0, |status| {
                crate::views::board_screen::visible_cards(view, &status.id, &self.board.filter)
                    .len()
            });
        self.board.focus.row = clamp_cursor(self.board.focus.row, len);
    }

    /// Whether the board's filter input, rather than the board itself, owns the keyboard.
    ///
    /// Both surfaces that draw the board answer here: the Hub's tab and the Workspace's board
    /// pane share one editor, one `BoardState.filter` and one two-stage `Esc`, so the pane
    /// publishes `Filter > BoardFilter` over `Workspace > Native > Board` for the same reason
    /// the Hub publishes it over `Hub > Board` — the bare letters have to type.
    #[must_use]
    pub(crate) fn board_filter_owns_keys(&self) -> bool {
        self.overlay.is_none()
            && self.agent_popup.is_none()
            && (matches!(self.screen, Screen::Hub { tab: HubTab::Board })
                || self.board_pane_is_active())
            && self.board.filter_editing
    }

    /// `Esc` on the board: leave the filter input, then clear the filter (§3.10, [D-15]).
    ///
    /// Returns whether it consumed the key; when it did not, `Esc` belongs to whoever owns the
    /// surface behind the board.
    pub(crate) fn board_filter_escape(&mut self) -> bool {
        match filter_escape(self.board.filter_editing) {
            FilterEscape::LeaveInput => {
                self.board.filter_editing = false;
                true
            }
            FilterEscape::ClearFilter if self.board.is_filtered() => {
                self.board.filter.clear();
                self.clamp_board_focus();
                true
            }
            FilterEscape::ClearFilter => false,
        }
    }
}

/// How an owed run reads: amber once the wait itself is the story (contracts §5.1).
///
/// A `since` this build cannot parse is drawn as an ordinary wait rather than as trouble: the
/// stamp comes from the daemon's clock, and a mark is not the place to report a malformed one.
fn pending_mark(pending: &PendingRun, now: DateTime<Utc>) -> RunMark {
    let stalled = DateTime::parse_from_rfc3339(&pending.since).is_ok_and(|since| {
        u64::try_from((now - since.with_timezone(&Utc)).num_seconds())
            .is_ok_and(|secs| secs >= PENDING_AMBER_AFTER_SECS)
    });
    if stalled {
        RunMark::Stalled
    } else {
        RunMark::Pending
    }
}

/// What a child that is still out says on its card's tile.
///
/// Only a child parked for a person changes the word. Every other status is drawn as work in
/// progress, including one the mirror has not caught up on: the card is the authority on whether
/// its run ended, and until it says otherwise a child is out there (BOARD.md §11.8).
const fn live_status_mark(status: DelegationStatus) -> RunMark {
    match status {
        DelegationStatus::Blocked => RunMark::NeedsYou,
        _ => RunMark::Working,
    }
}

/// Whether the column that started this run carries the card on by itself when it succeeds.
fn auto_advances(board: &Board, run: &CardRun) -> bool {
    board
        .statuses
        .iter()
        .find(|status| status.id == run.status_id)
        .and_then(|status| status.automation.as_ref())
        .is_some_and(|automation| automation.on_success.is_some())
}

/// The kit's tone for the domain's. Two enums on purpose: `fleet-ui-kit` takes no domain type.
const fn tone_of(tone: CoreBlockedTone) -> BlockedTone {
    match tone {
        CoreBlockedTone::Muted => BlockedTone::Muted,
        CoreBlockedTone::Warning => BlockedTone::Warning,
    }
}

#[cfg(test)]
mod tests;
