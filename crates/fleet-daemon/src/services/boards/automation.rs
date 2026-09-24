//! Column automation: the reservation, the run verbs, and what a finished run writes back.
//!
//! **The gate rule.** Only a request handler, [`Boards::resume_automation`],
//! [`Boards::on_slot_released`] and [`Boards::on_run_delivered`] ever take a board gate. Each of
//! them takes it, reloads the document, applies the change in memory, calls
//! [`fleet_core::board::automation::re_evaluate`], saves **once**, and drops the guard; every run
//! the plan asks for is started *after* that guard is gone, through [`Boards::start_for_card`],
//! which re-acquires the gate only to record what the delegation service answered.
//!
//! `Boards` therefore never calls a `pub` verb of its own — `move_card`, `update_card` and the
//! rest all take the gate themselves, and a second take on the same board would deadlock it for
//! the life of the daemon. Trigger sites call the `pub(crate)` helpers here instead.
//!
//! Nothing here holds a gate across a call into the delegation service either: starting,
//! cancelling and waiting all happen between two gates, because a provider that is slow to answer
//! must not stop every other card on its board from being read or moved.

#[cfg(test)]
mod card_worktree_tests;
mod resume;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use fleet_core::{
    agents::{AgentKind, Delegation, DelegationId, DelegationStatus, ModelSelection},
    board::{
        Action, ActionKind, ActivityKind, Board, BoardDocument, BoardError, Card, CardRun, Comment,
        LiveIndex, LiveRun, MAX_REPORT_COMMENTS_PER_CARD, MAX_RUNS_PER_CARD, Plan,
        REPORT_EXCERPT_CAP_BYTES, RunOutcome, Status, brief, latest_run, move_card, push_activity,
        re_evaluate, re_evaluate_settled, render_card_template, resolve_prefs, validate_env,
    },
    ids::{BoardId, CardId, StatusId, WorktreeId},
};
use fleet_proto::{error::ProtoError, event::BoardChangeReason, response::ResponseBody};
use tokio::sync::Mutex;

use super::{Boards, cards::is_working};
use crate::{
    DaemonError, DaemonResult,
    error::from_proto_error,
    services::{
        agents::delegation::{
            CardRunRequest, DelegationService, RunDeliveryHook, footer::card_child_title,
        },
        checkpoints::{ChangeKind, ChangedFile, Checkpoints},
    },
};

/// What a run verb answers on a daemon that has no native-agent database (contracts §3.5).
pub(super) const NO_DELEGATION_SERVICE: &str =
    "the native-agent database is unavailable, so board automation is refused";

/// The activity a cancel writes on a card that was only ever *owed* a run.
///
/// `Updated` rather than `RunEnded`: no run ended, because none had started — the same shape the
/// column that loses its action writes when it strands a wait (`lifecycle.rs`).
const PENDING_RUN_DROPPED: &str = "Run canceled: it was still waiting for a slot";

/// The trailing line a capped report excerpt carries, so the reader knows where the rest is.
const REPORT_ELIDED: &str = "(report elided; the full report is in the run's thread)";

/// The heading the changed-file list is printed under, in the run's own report comment.
///
/// "since this run started", not "this run changed": the diff is tree-to-tree against the run's
/// first checkpoint, so a person's own edits during the run are in it too.
const FILES_HEADING: &str = "## Files changed since this run started";

/// What the list says when the board lets more than one run share the checkout.
const FILES_SHARED_WORKTREE: &str =
    "Other runs share this worktree; some of these changes may be theirs.";

/// The provider a run uses when neither the card nor the column named one.
///
/// Claude rather than Codex because it is the provider every action kind can run: a skill action
/// is refused on Codex outright (`validate_automation`), so defaulting the other way would make a
/// column whose author expressed no preference fail on half the actions it can hold.
const DEFAULT_PROVIDER: AgentKind = AgentKind::Claude;

/// Everything column automation owns beyond the board documents themselves.
///
/// Public only because [`Boards::new`] is: the type is constructed by composition and named
/// nowhere else, and every field stays private to this module.
pub struct Automation {
    /// The one way a card run reaches a provider: a delegation whose caller is the card.
    delegations: DelegationService,
    /// Fleet-owned checkpoints, read at delivery to describe what a run left in the worktree.
    /// It is composed here because delivery has no other handle on the service.
    checkpoints: Arc<Checkpoints>,
    /// The reservation: cards a start has been decided for but whose run is not yet recorded,
    /// per board.
    ///
    /// Keyed by board because the ceiling it is counted against is: `settings.max_live_runs` is
    /// one board's, and one daemon-wide set would let a card reserved on one board park a card
    /// on another — a park nothing on that board would ever release, because the slot that frees
    /// belongs to the other board's bus event.
    ///
    /// Memory only. After a restart a reservation degrades to the card's `pending_run`, which
    /// [`Boards::resume_automation`] adopts or restarts — a durable reservation would instead
    /// have to be swept, and a swept row is indistinguishable from a live one.
    in_flight: Mutex<BTreeMap<BoardId, BTreeMap<CardId, StartReservation>>>,
    /// Boards known to hold a `pending_run`, so a freed slot costs nothing on every other board.
    pending_boards: Mutex<BTreeSet<BoardId>>,
    /// Card-worktree starts a board write decided and handed off rather than awaited
    /// ([`Boards::apply_starts_after_answer`]), so a test can wait for them to land.
    background_starts: tokio_util::task::TaskTracker,
}

/// Where a reserved start is in the only gap card mutations need to distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartPhase {
    /// The card may still be mutated while its worktree is being created; `prepare_run`
    /// revalidates every fact afterwards.
    CreatingWorktree,
    /// `prepare_run` returned the status and action the delegation is being launched for.
    Launching,
}

/// One in-memory start reservation and the cancellation requested before its row exists.
struct StartReservation {
    phase: StartPhase,
    cancel_requested: bool,
    settled: tokio_util::sync::CancellationToken,
    cancel_result: Arc<Mutex<Option<Result<(), ProtoError>>>>,
}

impl StartReservation {
    fn creating_worktree() -> Self {
        Self {
            phase: StartPhase::CreatingWorktree,
            cancel_requested: false,
            settled: tokio_util::sync::CancellationToken::new(),
            cancel_result: Arc::new(Mutex::new(None)),
        }
    }
}

impl Automation {
    /// Builds automation around the delegation service that starts runs and the checkpoints that
    /// describe what they changed.
    #[must_use]
    pub(crate) fn new(delegations: DelegationService, checkpoints: Arc<Checkpoints>) -> Self {
        Self {
            delegations,
            checkpoints,
            in_flight: Mutex::new(BTreeMap::new()),
            pending_boards: Mutex::new(BTreeSet::new()),
            background_starts: tokio_util::task::TaskTracker::new(),
        }
    }
}

/// What one decided start needs from the delegation service, and what the card must remember.
///
/// The row is separate from the request because it survives the request's *failure*: a start that
/// never happened is still a `CardRun` on the card, and it needs the same column, action and
/// provider the successful one would have carried.
struct Prepared {
    /// The identity of the run, whether or not the delegation service accepts it.
    row: RunRow,
    /// The worktree the run executes in, by [`run_worktree`]'s rule, written on the `CardRun`
    /// either way; `None` only when that rule refuses, which is then the run's refusal.
    worktree: Option<WorktreeId>,
    /// The request, or the refusal that stopped it being built.
    request: Result<CardRunRequest, DaemonError>,
}

/// What the seeds of one evaluation did, which is what decides whether rule 1 is theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seeds {
    /// The cards entered the column they are in, so the column's action is theirs to run.
    Entered,
    /// The cards are standing where they already stood — a run ended on them and moved nothing —
    /// so only the cards they block are considered.
    Settled,
}

/// How one start ended, which decides what its freed reservation owes the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartEnd {
    /// A run is recorded and holds the slot.
    Live,
    /// A failed start is recorded; no run holds the slot.
    Failed,
    /// Nothing was recorded: the card left its column, lost its action or went away first.
    Abandoned,
}

/// What `record_run` did with the provider answer after revalidating the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordEnd {
    /// The card still matched the prepared start, so the row was saved.
    Recorded,
    /// A cancel arrived while the provider was starting; a live delegation was stopped.
    Cancelled,
    /// The card no longer matched the prepared start; a live delegation was stopped.
    Abandoned,
}

/// How long `start_run` waits for a start it handed off before answering the card as it is:
/// well inside the client's request timeout, and far longer than a start that fetches nothing.
const START_ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Drops one card's reservation on its board, and the board's entry once it holds none.
async fn release(automation: &Automation, board: &BoardId, card: &CardId) {
    let mut reserved = automation.in_flight.lock().await;
    let Some(cards) = reserved.get_mut(board) else {
        return;
    };
    if let Some(reservation) = cards.remove(card) {
        reservation.settled.cancel();
    }
    if cards.is_empty() {
        reserved.remove(board);
    }
}

/// Whether the card is past revalidation and must not change until its run is recorded.
async fn launching(automation: &Automation, board: &BoardId, card: &CardId) -> bool {
    automation
        .in_flight
        .lock()
        .await
        .get(board)
        .and_then(|cards| cards.get(card))
        .is_some_and(|reservation| reservation.phase == StartPhase::Launching)
}

/// The facts about a run that come from the board rather than from the delegation.
struct RunRow {
    /// The column the run was started from.
    status_id: StatusId,
    /// What that column runs.
    action: ActionKind,
    /// The provider the card, the column and the daemon's default agreed on.
    provider: AgentKind,
    /// The model the run asked for, or `None` for the provider's own default.
    model: Option<String>,
    /// The reasoning effort the run asked for, or `None` for the provider's own default.
    effort: Option<String>,
}

impl Boards {
    /// Starts the card's column action now, whatever its last run ended as.
    ///
    /// # Errors
    ///
    /// `Validation` when the column runs no action, `Conflict` when a run is already live, and
    /// `Unsupported` when this daemon has no automation at all.
    pub async fn start_run(&self, card: &CardId) -> DaemonResult<Card> {
        let Some(automation) = self.automation() else {
            return Err(DaemonError::Unsupported(NO_DELEGATION_SERVICE.to_owned()));
        };
        let (board, plan) = {
            let (_guard, mut doc, index) = self.locked_card_document(card).await?;
            let key = doc.cards[index].display_key(&doc.board);
            let status_id = doc.cards[index].status_id.clone();
            if on_enter(&doc.board, &status_id).is_none() {
                return Err(DaemonError::Validation(format!(
                    "{} has no action",
                    column_name(&doc.board, &status_id)
                )));
            }
            let now = self.now();
            // Live is live however it is known: the row the document remembers, and the
            // reservation an evaluation has already promised a provider.
            if is_working(&doc.cards[index])
                || automation
                    .in_flight
                    .lock()
                    .await
                    .get(&doc.board.id)
                    .is_some_and(|reserved| reserved.contains_key(card))
            {
                return Err(DaemonError::Conflict(format!(
                    "{key} is working; cancel the run first"
                )));
            }
            let plan = self
                .evaluate_with_reservation(&mut doc, std::slice::from_ref(card), &now)
                .await?;
            doc.board.updated_at = now;
            self.save(&doc, BoardChangeReason::CardChanged).await?;
            self.note_pending(&doc.board.id, holds_pending(&doc.cards))
                .await;
            (doc.board.clone(), plan)
        };
        // A card-worktree start may fetch its pull request first, which can outlast the
        // client's request timeout. It runs on its own and is awaited only for a bounded time,
        // so a quick start or refusal is in the answer and a slow one is answered as it stands.
        if let Some(started) = self.hand_off_starts(&board, plan).await?
            && tokio::time::timeout(START_ANSWER_WAIT, started)
                .await
                .is_err()
        {
            tracing::debug!(board = %board.id, %card, "a card run is still starting; answering the card as it is");
        }
        self.card_now(&board.id, card).await
    }

    /// Cancels the card's live run, or drops the slot an owed one is waiting for.
    ///
    /// A card the board has only *promised* a run to has no child to stop: what it carries is a
    /// `pending_run`, and dropping that here is the whole of the cancellation — the run the
    /// column owes it is never started, and nothing else on the board changes.
    ///
    /// # Errors
    ///
    /// `NotFound` when the card has neither a live run nor an owed one, and `Unsupported` with
    /// no automation.
    pub async fn cancel_run(&self, card: &CardId) -> DaemonResult<Card> {
        let Some(automation) = self.automation() else {
            return Err(DaemonError::Unsupported(NO_DELEGATION_SERVICE.to_owned()));
        };
        let (board, run, starting) = {
            let (_guard, mut doc, index) = self.locked_card_document(card).await?;
            match live_run(&doc.cards[index]) {
                Some(live) => (doc.board.id.clone(), Some(live), None),
                None => {
                    let mut reservations = automation.in_flight.lock().await;
                    let launching = reservations
                        .get_mut(&doc.board.id)
                        .and_then(|cards| cards.get_mut(card))
                        .filter(|reservation| reservation.phase == StartPhase::Launching);
                    if let Some(reservation) = launching {
                        reservation.cancel_requested = true;
                        (
                            doc.board.id.clone(),
                            None,
                            Some((
                                reservation.settled.clone(),
                                Arc::clone(&reservation.cancel_result),
                            )),
                        )
                    } else if doc.cards[index].pending_run.is_some() {
                        drop(reservations);
                        let now = self.now();
                        doc.cards[index].pending_run = None;
                        doc.cards[index].updated_at.clone_from(&now);
                        push_activity(
                            &mut doc.cards[index],
                            ActivityKind::Updated,
                            None,
                            PENDING_RUN_DROPPED.to_owned(),
                            &now,
                        );
                        doc.board.updated_at.clone_from(&now);
                        self.save(&doc, BoardChangeReason::CardChanged).await?;
                        self.note_pending(&doc.board.id, holds_pending(&doc.cards))
                            .await;
                        return self.card_view(&doc.board, &doc.cards[index]).await;
                    } else {
                        return Err(DaemonError::NotFound(format!(
                            "{} has no live run",
                            doc.cards[index].display_key(&doc.board)
                        )));
                    }
                }
            }
        };
        if let Some((settled, result)) = starting {
            settled.cancelled().await;
            if let Some(Err(error)) = result.lock().await.clone() {
                return Err(from_proto_error(error));
            }
            return self.card_now(&board, card).await;
        }
        // Outside the gate on purpose: cancelling stops a child process, and the board it belongs
        // to stays readable while that happens. Nothing is written here — the child's own
        // `Cancelled` transition reaches `on_run_delivered`, which is the one writer of an
        // outcome, so a cancel that raced a success cannot overwrite the success.
        let Some(run) = run else {
            return Err(DaemonError::Conflict(format!(
                "{card} lost its live run while cancellation was starting"
            )));
        };
        automation
            .delegations
            .cancel(run)
            .await
            .map_err(from_proto_error)?;
        self.card_now(&board, card).await
    }

    /// Waits for the card's newest run to end, or answers the card as it is when time runs out.
    ///
    /// # Errors
    ///
    /// The read's failure, or `Unsupported` with no automation.
    pub async fn wait_run(&self, card: &CardId, timeout_ms: u64) -> DaemonResult<Card> {
        let Some(automation) = self.automation() else {
            return Err(DaemonError::Unsupported(NO_DELEGATION_SERVICE.to_owned()));
        };
        let (board, run) = {
            let (_guard, doc, index) = self.locked_card_document(card).await?;
            (doc.board.id.clone(), live_run(&doc.cards[index]))
        };
        // A run started under a daemon that died before `record_run` landed is live with no row
        // on the card; the store is asked so a wait over that window still waits for it, instead
        // of answering "nothing is running" about a child that is.
        let run = match run {
            Some(run) => Some(run),
            None => automation
                .delegations
                .live_for_card(&board, card)
                .await?
                .map(|delegation| delegation.id),
        };
        if let Some(run) = run {
            // `None` as the caller: a card is not a thread, so this wait never consumes the
            // delivery the board hook is about to record.
            automation
                .delegations
                .wait(run, timeout_ms, None)
                .await
                .map_err(from_proto_error)?;
        }
        self.card_now(&board, card).await
    }

    /// Turns one decided start into a delegation, outside every gate.
    ///
    /// Resolves the prefs, assembles the brief and the card footer, calls the delegation
    /// service, then re-acquires the gate to record the `CardRun` — the successful one, or the
    /// failed-to-start one that carries the refusal's sentence.
    ///
    /// # Errors
    ///
    /// The failure of the board write that records the run. The run's *own* refusal is recorded
    /// on the card rather than returned: a start that never happened is a card fact.
    pub(crate) async fn start_for_card(&self, board: &BoardId, card: &CardId) -> DaemonResult<()> {
        let Some(automation) = self.automation() else {
            return Ok(());
        };
        let ended = self.try_start_for_card(automation, board, card).await;
        match &ended {
            Ok(StartEnd::Live) => {}
            // No run holds the slot this start reserved, so it is handed on at once: the card
            // waiting in line would otherwise wait for an unrelated delegation to end.
            Ok(StartEnd::Failed) => self.hand_on_slot(board, None),
            // The card left its column meanwhile, and the entry's own evaluation was refused
            // because this start still held the card: it is evaluated again now.
            Ok(StartEnd::Abandoned) => self.hand_on_slot(board, Some(card.clone())),
            Err(_) => {
                release(automation, board, card).await;
                self.hand_on_slot(board, None);
            }
        }
        ended.map(drop)
    }

    /// Hands a slot a start freed without a run to the next card, and re-evaluates a card whose
    /// start was abandoned, on a tracked task of their own.
    ///
    /// Its own task rather than an await: [`Self::release_slot`] can start the next card, whose
    /// start can end here again, and a request or event loop must not wait on that chain.
    fn hand_on_slot(&self, board: &BoardId, reentered: Option<CardId>) {
        let Some(automation) = self.automation() else {
            return;
        };
        let service = self.clone();
        let board = board.clone();
        automation.background_starts.spawn(async move {
            // The card that waited longest goes first; the one re-evaluated queues behind it.
            if let Err(error) = service.release_slot(&board).await {
                tracing::warn!(%board, %error, "a slot freed by a start that ran nothing could not be handed on");
            }
            if let Some(card) = reentered
                && let Err(error) = service.reseed(&board, &card).await
            {
                tracing::warn!(%board, %card, %error, "a card whose start was abandoned could not be evaluated again");
            }
        });
    }

    /// Evaluates one card again as having entered the column it stands in.
    ///
    /// For a card that entered a routing column while a start still held it: rule 0 refused it
    /// then, and nothing else would ever look at it again.
    async fn reseed(&self, board: &BoardId, card: &CardId) -> DaemonResult<()> {
        let (board, plan) = {
            let _guard = self.gate(board).await;
            let now = self.now();
            let mut doc = self.load(board)?;
            if !doc
                .cards
                .iter()
                .any(|other| other.id == *card && !other.archived)
            {
                return Ok(());
            }
            let plan = self
                .evaluate_with_reservation(&mut doc, std::slice::from_ref(card), &now)
                .await?;
            if resume::decided(&plan, &doc.cards, &now) {
                doc.board.updated_at = now;
                self.save(&doc, BoardChangeReason::CardChanged).await?;
            }
            self.note_pending(board, holds_pending(&doc.cards)).await;
            (doc.board, plan)
        };
        self.apply_starts_after_answer(&board, plan).await
    }

    /// Whether an earlier evaluation has promised this card a run that is not recorded yet.
    pub(super) async fn start_in_flight(&self, board: &BoardId, card: &CardId) -> bool {
        let Some(automation) = self.automation() else {
            return false;
        };
        automation
            .in_flight
            .lock()
            .await
            .get(board)
            .is_some_and(|reserved| reserved.contains_key(card))
    }

    /// Whether the card's reserved start has passed its final revalidation and is launching.
    pub(super) async fn start_is_launching(&self, board: &BoardId, card: &CardId) -> bool {
        let Some(automation) = self.automation() else {
            return false;
        };
        launching(automation, board, card).await
    }

    /// The start itself, answering how it ended; [`Self::start_for_card`] owns what follows.
    async fn try_start_for_card(
        &self,
        automation: &Automation,
        board: &BoardId,
        card: &CardId,
    ) -> DaemonResult<StartEnd> {
        // Outside every gate, and with the reservation still held: a card-worktree run may first
        // have to fetch its pull request into a new worktree, and that clone counts against the
        // board's live-run ceiling like the run it is for.
        if let Err(error) = self.ensure_pull_request_worktree(board, card).await {
            let Some(prepared) = self.prepare_run(board, card).await? else {
                // The card left, or its column stopped running, while the worktree was made. The
                // refusal names the worktree it leaves behind, and nothing on the card can.
                tracing::warn!(%board, %card, %error, "a card run's worktree could not be linked");
                release(automation, board, card).await;
                return Ok(StartEnd::Abandoned);
            };
            let recorded = self
                .record_run(
                    board,
                    card,
                    in_worktree(
                        failed(&prepared.row, &error, &self.now()),
                        prepared.worktree,
                    ),
                )
                .await?;
            return Ok(match recorded {
                RecordEnd::Recorded | RecordEnd::Cancelled => StartEnd::Failed,
                RecordEnd::Abandoned => StartEnd::Abandoned,
            });
        }
        let prepared = match self.prepare_run(board, card).await? {
            Some(prepared) => prepared,
            // The card left the column, lost its action or was deleted between the evaluation
            // and here. Nothing was promised to a provider, so nothing is recorded — but the
            // reservation must still go, or this card is one nothing ever starts again.
            None => {
                release(automation, board, card).await;
                return Ok(StartEnd::Abandoned);
            }
        };
        let Prepared {
            row,
            worktree,
            request,
        } = prepared;
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                let recorded = self
                    .record_run(
                        board,
                        card,
                        in_worktree(failed(&row, &error, &self.now()), worktree),
                    )
                    .await?;
                return Ok(match recorded {
                    RecordEnd::Recorded | RecordEnd::Cancelled => StartEnd::Failed,
                    RecordEnd::Abandoned => StartEnd::Abandoned,
                });
            }
        };
        match automation.delegations.run_for_card(request).await {
            Ok((delegation, _warning)) => {
                let recorded = self
                    .record_run(
                        board,
                        card,
                        in_worktree(started(&row, &delegation, &self.now()), worktree),
                    )
                    .await?;
                Ok(match recorded {
                    RecordEnd::Recorded => StartEnd::Live,
                    RecordEnd::Cancelled => StartEnd::Failed,
                    RecordEnd::Abandoned => StartEnd::Abandoned,
                })
            }
            Err(error) => {
                let recorded = self
                    .record_run(
                        board,
                        card,
                        in_worktree(failed(&row, &error, &self.now()), worktree),
                    )
                    .await?;
                Ok(match recorded {
                    RecordEnd::Recorded | RecordEnd::Cancelled => StartEnd::Failed,
                    RecordEnd::Abandoned => StartEnd::Abandoned,
                })
            }
        }
    }

    /// Starts everything one evaluation decided, in the order it decided them.
    ///
    /// Called only after the gate that produced `plan` has been dropped.
    ///
    /// # Errors
    ///
    /// The first board write that failed while recording a run.
    pub(crate) async fn apply_starts(&self, board: &BoardId, plan: Plan) -> DaemonResult<()> {
        for start in plan.starts {
            self.start_for_card(board, &start.card).await?;
        }
        Ok(())
    }

    /// Starts what a card write decided without making the write wait for it, on a board that
    /// runs each card in the card's own worktree; awaits it anywhere else.
    ///
    /// A card-worktree start may first fetch its pull request and create the worktree — minutes
    /// of work — and the request that moved or created the card (`card new --pr` from a
    /// scheduled agent, a move in the app) must answer once its save has landed, not time out
    /// on a card the daemon did write. The reservation the plan holds keeps the slot counted
    /// meanwhile, and every refusal is recorded on the card's run, so the only thing the caller
    /// no longer hears is a failed board write, which is logged instead.
    ///
    /// # Errors
    ///
    /// On a board-worktree board, what [`Self::apply_starts`] returns.
    pub(crate) async fn apply_starts_after_answer(
        &self,
        board: &Board,
        plan: Plan,
    ) -> DaemonResult<()> {
        self.hand_off_starts(board, plan).await.map(drop)
    }

    /// [`Self::apply_starts_after_answer`], answering the handle of the task it handed the
    /// starts to, if it handed them off, so a caller can wait for them a bounded while.
    async fn hand_off_starts(
        &self,
        board: &Board,
        plan: Plan,
    ) -> DaemonResult<Option<tokio::task::JoinHandle<()>>> {
        if plan.starts.is_empty() || board.settings.run_location.is_board_worktree() {
            return self.apply_starts(&board.id, plan).await.map(|()| None);
        }
        let Some(automation) = self.automation() else {
            return Ok(None);
        };
        let service = self.clone();
        let board = board.id.clone();
        Ok(Some(automation.background_starts.spawn(async move {
            if let Err(error) = service.apply_starts(&board, plan).await {
                tracing::warn!(%board, %error, "a card run started after its request answered could not be recorded");
            }
        })))
    }

    /// Waits until every start [`Self::apply_starts_after_answer`] handed off has landed.
    #[cfg(test)]
    pub(crate) async fn background_starts_settled(&self) {
        let Some(automation) = self.automation() else {
            return;
        };
        let tracker = &automation.background_starts;
        tracker.close();
        tracker.wait().await;
        tracker.reopen();
    }

    /// Reads what one start needs, without holding anything.
    ///
    /// `Ok(None)` means there is nothing left to start: the card is gone, archived, or its column
    /// no longer runs an action. The inner `Err` is a refusal that must be *recorded* on the card
    /// rather than returned, which is why it travels inside [`Prepared`] rather than out of here.
    async fn prepare_run(&self, board: &BoardId, card: &CardId) -> DaemonResult<Option<Prepared>> {
        let Some(automation) = self.automation() else {
            return Ok(None);
        };
        // This gate joins the final revalidation to the phase transition. A mutation either
        // lands before this read (and is observed below), or takes the gate afterwards and sees
        // `Launching`; there is no unguarded interval between the two.
        let _guard = self.gate(board).await;
        let doc = self.load(board)?;
        let Some(index) = doc.cards.iter().position(|other| other.id == *card) else {
            return Ok(None);
        };
        let target = &doc.cards[index];
        if target.archived {
            return Ok(None);
        }
        let Some(action) = on_enter(&doc.board, &target.status_id) else {
            return Ok(None);
        };
        let prefs = resolve_prefs(target, action);
        let row = RunRow {
            status_id: target.status_id.clone(),
            action: action.kind.clone(),
            provider: prefs.provider.unwrap_or(DEFAULT_PROVIDER),
            model: prefs.model.clone(),
            effort: prefs.effort.clone(),
        };
        let key = target.display_key(&doc.board);
        let worktree = run_worktree(&doc.board, target, &key).ok();
        // Contracts §1.7: the three board rules run again here, not only when automation is
        // configured — a worktree can be adopted by another host long after its column was
        // written, and the run that would touch it is the thing that must refuse.
        let request = match self.require_automatable(&doc.board).await {
            Ok(()) => card_request(&doc.board, target, action, &key, &row, prefs.mode),
            Err(refusal) => Err(refusal),
        };
        let prepared = Prepared {
            row,
            worktree,
            request,
        };
        let mut reserved = automation.in_flight.lock().await;
        let reservation = reserved
            .get_mut(board)
            .and_then(|cards| cards.get_mut(card))
            .ok_or_else(|| {
                DaemonError::Conflict(format!(
                    "card {card} lost its start reservation before launch"
                ))
            })?;
        reservation.phase = StartPhase::Launching;
        Ok(Some(prepared))
    }

    /// Writes one run onto the card, clears its reservation, and saves once.
    ///
    /// The reservation is released *inside* the gate and after the row is written, so the next
    /// evaluation sees exactly one of the two: the promise, or the run that kept it.
    async fn record_run(
        &self,
        board: &BoardId,
        card: &CardId,
        run: CardRun,
    ) -> DaemonResult<RecordEnd> {
        let Some(automation) = self.automation() else {
            return Ok(RecordEnd::Abandoned);
        };
        let guard = self.gate(board).await;
        let now = self.now();
        let mut doc = self.load(board)?;
        let index = doc.cards.iter().position(|other| other.id == *card);
        let still_prepared = index.is_some_and(|index| {
            let target = &doc.cards[index];
            !target.archived
                && target.status_id == run.status_id
                && on_enter(&doc.board, &target.status_id)
                    .is_some_and(|action| action.kind == run.action)
        });
        let cancel_requested = automation
            .in_flight
            .lock()
            .await
            .get(board)
            .and_then(|cards| cards.get(card))
            .is_some_and(|reservation| reservation.cancel_requested);
        if !cancel_requested && let Some(index) = index.filter(|_| still_prepared) {
            let failed_to_start = run.failed_to_start();
            let ended = run.ended_at.clone();
            let outcome = run.outcome;
            push_run(&mut doc.cards[index], run, &now);
            doc.cards[index].pending_run = None;
            // A start that never happened still ends: the card carries a `RunStarted` the
            // evaluation wrote, and an entry nothing closes reads as a run still working.
            if failed_to_start && let Some(outcome) = outcome {
                let message = run_ended(outcome, ended.is_some().then_some(0), None);
                push_activity(
                    &mut doc.cards[index],
                    ActivityKind::RunEnded,
                    None,
                    message,
                    &now,
                );
            }
            doc.board.updated_at = now;
            let saved = self.save(&doc, BoardChangeReason::CardChanged).await;
            self.note_pending(board, holds_pending(&doc.cards)).await;
            drop(guard);
            release(automation, board, card).await;
            saved?;
            return Ok(RecordEnd::Recorded);
        }
        if !cancel_requested {
            tracing::warn!(
                %board,
                %card,
                status = %run.status_id,
                action = ?run.action,
                "stopping a card delegation because its prepared start is no longer current"
            );
        }
        let saved = if cancel_requested
            && let Some(index) = index
            && doc.cards[index].pending_run.take().is_some()
        {
            doc.cards[index].updated_at.clone_from(&now);
            doc.board.updated_at.clone_from(&now);
            let saved = self.save(&doc, BoardChangeReason::CardChanged).await;
            self.note_pending(board, holds_pending(&doc.cards)).await;
            saved
        } else {
            Ok(())
        };
        let live = run.is_live();
        let delegation = run.id;
        drop(guard);
        let cancelled = if live {
            automation.delegations.cancel(delegation).await.map(drop)
        } else {
            Ok(())
        };
        if cancel_requested
            && let Some(result) = automation
                .in_flight
                .lock()
                .await
                .get(board)
                .and_then(|cards| cards.get(card))
                .map(|reservation| Arc::clone(&reservation.cancel_result))
        {
            *result.lock().await = Some(cancelled.clone());
        }
        release(automation, board, card).await;
        saved?;
        cancelled.map_err(from_proto_error)?;
        Ok(if cancel_requested {
            RecordEnd::Cancelled
        } else {
            RecordEnd::Abandoned
        })
    }

    /// Walks the automation engine over a document the caller changed, holding the board's own
    /// reservation for as long as the walk takes.
    ///
    /// The reservation has to be *the* set [`Automation`] holds rather than a fresh one: a start
    /// an earlier evaluation decided still counts against the live-run ceiling until its run row
    /// exists, and two evaluations over two empty sets would both start the same card. Called by
    /// every trigger site through the tail they share, and by the two verbs here.
    ///
    /// An empty seed list, or a daemon with no automation, evaluates nothing at all — which is
    /// what every edit that cannot change a satisfaction costs.
    ///
    /// # Errors
    ///
    /// The [`fleet_core::board::BoardError`] of any in-memory move the cascade performs.
    pub(super) async fn evaluate_with_reservation(
        &self,
        doc: &mut BoardDocument,
        seeds: &[CardId],
        now: &str,
    ) -> DaemonResult<Plan> {
        self.evaluate(doc, seeds, Seeds::Entered, now).await
    }

    /// The same evaluation for a card a run has just *finished* on.
    ///
    /// `entered` is whether the outcome moved it: a card that moved is an entry like any other,
    /// and one that did not is settled, so only the cards it blocks are considered.
    async fn evaluate_after_run(
        &self,
        doc: &mut BoardDocument,
        card: &CardId,
        entered: bool,
        now: &str,
    ) -> DaemonResult<Plan> {
        let seeds = std::slice::from_ref(card);
        let kind = if entered {
            Seeds::Entered
        } else {
            Seeds::Settled
        };
        self.evaluate(doc, seeds, kind, now).await
    }

    /// The one place the board's reservation is taken, whichever rule the seeds answer.
    async fn evaluate(
        &self,
        doc: &mut BoardDocument,
        seeds: &[CardId],
        kind: Seeds,
        now: &str,
    ) -> DaemonResult<Plan> {
        let Some(automation) = self.automation() else {
            return Ok(Plan::default());
        };
        if seeds.is_empty() {
            return Ok(Plan::default());
        }
        // The live index is read from the card rows themselves: they are this daemon's own record
        // of what is running, written when a start reaches the delegation service and closed when
        // its delivery lands, so a document loaded under the gate already knows.
        let live = LiveIndex::from_runs(&doc.cards);
        let mut reserved = automation.in_flight.lock().await;
        let reservations = reserved.entry(doc.board.id.clone()).or_default();
        let mut in_flight = reservations.keys().cloned().collect::<BTreeSet<_>>();
        let plan = match kind {
            Seeds::Entered => re_evaluate(
                &doc.board,
                &mut doc.cards,
                seeds,
                &live,
                &mut in_flight,
                now,
            ),
            Seeds::Settled => re_evaluate_settled(
                &doc.board,
                &mut doc.cards,
                seeds,
                &live,
                &mut in_flight,
                now,
            ),
        };
        reservations.retain(|card, _| in_flight.contains(card));
        for card in in_flight {
            reservations
                .entry(card)
                .or_insert_with(StartReservation::creating_worktree);
        }
        // A board nobody is starting anything on keeps no entry: the map is memory the daemon
        // holds for the life of the process, and an empty set per board ever touched is a leak
        // nothing would ever collect.
        if reserved.get(&doc.board.id).is_some_and(BTreeMap::is_empty) {
            reserved.remove(&doc.board.id);
        }
        Ok(plan?)
    }

    /// The live delegations this board's cards called, for the join a read makes.
    ///
    /// Never persisted: it is asked of the delegation store on the way out, so a reader sees what
    /// is running now rather than what the document last recorded. A board on a daemon with no
    /// automation, and a store that cannot answer, both join nothing.
    pub(super) async fn live_runs(&self, board: &BoardId) -> Vec<LiveRun> {
        let Some(automation) = self.automation() else {
            return Vec::new();
        };
        match automation.delegations.live_for_board(board).await {
            Ok(live) => live.iter().filter_map(card_live_run).collect(),
            Err(error) => {
                tracing::warn!(%board, %error, "failed to join a board's live runs");
                Vec::new()
            }
        }
    }

    /// The card as a reader sees it now, re-read after a verb that wrote nothing itself.
    async fn card_now(&self, board: &BoardId, card: &CardId) -> DaemonResult<Card> {
        let doc = self.load(board)?;
        let index = doc
            .cards
            .iter()
            .position(|other| other.id == *card)
            .ok_or_else(|| {
                DaemonError::NotFound(format!("card {card} is no longer on board {board}"))
            })?;
        self.card_view(&doc.board, &doc.cards[index]).await
    }
}

#[async_trait::async_trait]
impl RunDeliveryHook for Boards {
    async fn on_run_delivered(
        &self,
        board: &BoardId,
        card: &CardId,
        delegation: &Delegation,
    ) -> DaemonResult<()> {
        let Some(automation) = self.automation() else {
            return Ok(());
        };
        let Some(outcome) = outcome_of(delegation) else {
            // Only a terminal delegation is delivered. Answering `Ok` closes the row rather than
            // retrying a record that would still not be terminal on the next drain.
            tracing::warn!(
                delegation = %delegation.id,
                status = delegation.status.word(),
                "a card run was delivered before it was terminal; nothing recorded"
            );
            return Ok(());
        };
        // Before the gate: the usage read is a database round trip, and no board write waits on it.
        let usage = self.run_usage(automation, delegation.id).await;
        // Also before the gate, and for the same reason twice over: the diff shells out to Git
        // once per file-listing command, and it describes the tree *now*, which is the tree the
        // run left behind. Taken under the gate it would hold every other card on the board for
        // as long as Git took to answer.
        let changed = self.changed_files(automation, board, delegation).await;
        self.record_result_files(automation, delegation.id, &changed)
            .await;
        let (next_board, plan) = {
            let _guard = self.gate(board).await;
            let now = self.now();
            let mut doc = self.load(board)?;
            let Some(index) = doc.cards.iter().position(|other| other.id == *card) else {
                return Ok(());
            };
            let Some(run) = doc.cards[index]
                .runs
                .iter()
                .position(|run| run.id == delegation.id)
            else {
                // The run is not on the card: the cap dropped it, or the document was replaced.
                // There is nothing to close, and retrying would never find it either.
                return Ok(());
            };
            // Idempotent by delegation id: a second delivery of a run this board already closed
            // writes nothing and still marks the row done.
            if doc.cards[index].runs[run].outcome.is_some() {
                return Ok(());
            }
            let run_status = doc.cards[index].runs[run].status_id.clone();
            let ended_at = delegation
                .finished
                .map_or_else(|| now.clone(), |finished| finished.to_rfc3339());
            let (cost_usd, tokens) = usage;
            {
                let row = &mut doc.cards[index].runs[run];
                row.ended_at = Some(ended_at);
                row.outcome = Some(outcome);
                row.detail = delegation.status_payload.clone();
                // The count, not the list: the card row is a summary a tile can read, and the
                // paths live in the report comment and on the delegation's own result.
                row.files_changed = u32::try_from(changed.len()).unwrap_or(u32::MAX);
                row.cost_usd = cost_usd;
                row.tokens = tokens;
            }
            let report = delegation
                .result
                .as_ref()
                .map(|result| result.text.as_str())
                .filter(|text| !text.trim().is_empty());
            if let Some(report) = report {
                let comment = uuid::Uuid::new_v4().to_string();
                // The section is appended *after* the cap, so a long report is what gets elided
                // and the file list is never the part that goes missing.
                let files = files_section(&changed, shares_worktree(&doc.board));
                doc.cards[index].comments.push(Comment {
                    id: comment.clone(),
                    author: None,
                    body: format!("{}{files}", report_excerpt(report)),
                    created_at: now.clone(),
                    remote_id: None,
                    run_id: Some(delegation.id),
                });
                doc.cards[index].runs[run].report_comment_id = Some(comment);
                rotate_reports(&mut doc.cards[index]);
            }
            let message = run_ended(
                outcome,
                Some(delegation.elapsed(self.clock.now()).num_seconds().max(0)),
                cost_usd,
            );
            push_activity(
                &mut doc.cards[index],
                ActivityKind::RunEnded,
                None,
                message,
                &now,
            );
            // Only a card the outcome *moved* has entered a column, and a column runs its action
            // on entry (`docs/BOARD.md` §11.7). A run that ended where it started leaves its card
            // standing exactly where it stood, so it is seeded as settled: seeding it as an entry
            // would start the same column again the moment this run ended, and again when that
            // one did, for as long as nobody moved the card.
            let entered = outcome == RunOutcome::Succeeded
                && self.move_on_success(&mut doc.board, &mut doc.cards, index, &run_status, &now);
            let plan = self
                .evaluate_after_run(&mut doc, card, entered, &now)
                .await?;
            doc.board.updated_at = now;
            self.save(&doc, BoardChangeReason::CardChanged).await?;
            self.note_pending(board, holds_pending(&doc.cards)).await;
            (doc.board, plan)
        };
        // Handed off on a card-worktree board: the next card's pull-request fetch must not hold
        // the delegation outbox drain, which every other delivery waits behind.
        self.apply_starts_after_answer(&next_board, plan).await
    }
}

impl Boards {
    /// What the run left in the worktree it ran in, as its delivery can describe it.
    ///
    /// Every way of not knowing answers an empty list and logs: a board that is not a worktree
    /// board, a worktree this daemon no longer holds, a checkout that is not a Git working tree,
    /// a thread whose provider never took a checkpoint, and a Git command that failed. A run's
    /// outcome is the fact this delivery came to record, and it is not worth losing to a diff.
    async fn changed_files(
        &self,
        automation: &Automation,
        board: &BoardId,
        delegation: &Delegation,
    ) -> Vec<ChangedFile> {
        let worktree = match self.load(board) {
            Ok(doc) => diff_worktree(&doc, delegation.id),
            Err(error) => {
                tracing::warn!(%board, %error, "a card run's board could not be read for its diff");
                return Vec::new();
            }
        };
        // Automation refuses a run with no worktree at `card_request`, so this is a board that
        // lost its worktree while a run was working rather than one that never had one.
        let Some(worktree) = worktree else {
            return Vec::new();
        };
        let state = match self.state_store.load().await {
            Ok(state) => state,
            Err(error) => {
                tracing::warn!(%board, %error, "daemon state could not be read for a card run's diff");
                return Vec::new();
            }
        };
        let Some(record) = self.known_worktree(&state, &worktree) else {
            tracing::warn!(%board, %worktree, "a card run's worktree is gone; nothing to diff");
            return Vec::new();
        };
        match automation
            .checkpoints
            .changed_since(Path::new(&record.path), &delegation.child)
            .await
        {
            Ok(changed) => changed,
            Err(error) => {
                tracing::warn!(
                    delegation = %delegation.id,
                    %error,
                    "a card run's changed files could not be described"
                );
                Vec::new()
            }
        }
    }

    /// Hands the paths to the delegation record, so a reader of the run itself sees them too.
    ///
    /// An empty list is not written: the diff answers empty both when the run changed nothing and
    /// when it could not be taken at all, and a blind write would erase the list a recovered
    /// completion had already filled in.
    async fn record_result_files(
        &self,
        automation: &Automation,
        delegation: DelegationId,
        changed: &[ChangedFile],
    ) {
        if changed.is_empty() {
            return;
        }
        let paths = changed.iter().map(|file| file.path.clone()).collect();
        if let Err(error) = automation
            .delegations
            .set_result_files(&delegation, paths)
            .await
        {
            // Not a delivery failure: the card keeps the count and the report keeps the list, so
            // retrying the whole hook would rewrite both to recover one column.
            tracing::warn!(%delegation, %error, "a card run's changed files could not be recorded");
        }
    }

    /// What the child spent, read from the delegation service's own usage path.
    ///
    /// A failure here is not a delivery failure: the outcome is the fact worth keeping, and a
    /// retry would re-run the whole hook to recover two numbers nothing depends on.
    async fn run_usage(
        &self,
        automation: &Automation,
        delegation: DelegationId,
    ) -> (Option<f64>, Option<u64>) {
        match automation.delegations.get(delegation).await {
            Ok(ResponseBody::Delegation(record)) => record.usage.map_or((None, None), |usage| {
                (usage.cost_usd, Some(usage.usage.total_tokens))
            }),
            Ok(other) => {
                tracing::warn!(
                    %delegation,
                    response = ?std::mem::discriminant(&other),
                    "the delegation service answered a usage read with an unexpected response"
                );
                (None, None)
            }
            Err(error) => {
                tracing::warn!(%delegation, %error, "a card run's usage could not be read");
                (None, None)
            }
        }
    }

    /// Moves a card whose run succeeded to the column its run column routes to, and reports
    /// whether it moved.
    ///
    /// The answer is what tells the delivery's evaluation which card *entered* a column: only a
    /// card that moved did, and only an entry runs an action.
    ///
    /// A move that fails is logged rather than propagated: the outcome this delivery came to
    /// record is already in memory, and returning would leave the `Deliver` row open to retry a
    /// write that would fail the same way every time.
    fn move_on_success(
        &self,
        board: &mut Board,
        cards: &mut [Card],
        index: usize,
        run_status: &StatusId,
        now: &str,
    ) -> bool {
        let Some(target) = column(board, run_status)
            .and_then(|status| status.automation.as_ref())
            .and_then(|automation| automation.on_success.clone())
        else {
            return false;
        };
        let card = cards[index].id.clone();
        let message = format!("Moved to {}: run succeeded", column_name(board, &target));
        match move_card(board, cards, &card, &target, None, now) {
            Ok(true) => {
                record_auto_move(&mut cards[index], message, now);
                true
            }
            Ok(false) => false,
            Err(error) => {
                tracing::warn!(
                    %card,
                    %error,
                    "a successful run could not be moved to its column's on_success target"
                );
                false
            }
        }
    }
}

/// The worktree one card's run executes in, by the board's `run_location`.
///
/// A board-worktree board runs every card in its own worktree; a card-worktree board runs each
/// card in the card's, which [`Boards::ensure_pull_request_worktree`] linked just before. Both
/// refusals are unreachable behind that step and `require_automatable`, which every start passes
/// first; they are kept because the worktree is what the request is built from, and they are
/// spelt in the contracts' own words.
fn run_worktree(board: &Board, card: &Card, key: &str) -> Result<WorktreeId, DaemonError> {
    let (worktree, reason) = if board.settings.run_location.is_board_worktree() {
        (
            board.worktree_id.clone(),
            "automation is available on worktree boards only".to_owned(),
        )
    } else {
        (
            card.worktree_id.clone(),
            format!(
                "{key} has no worktree to run in; link a pull request or create its worktree first"
            ),
        )
    };
    worktree.ok_or_else(|| {
        BoardError::Invalid {
            field: "automation".into(),
            reason,
        }
        .into()
    })
}

/// The worktree a delivered run's changed files are read from: the one it ran in, or the
/// board's for a run recorded before runs carried their worktree.
fn diff_worktree(doc: &BoardDocument, delegation: DelegationId) -> Option<WorktreeId> {
    doc.cards
        .iter()
        .flat_map(|card| &card.runs)
        .find(|run| run.id == delegation)
        .and_then(|run| run.worktree_id.clone())
        .or_else(|| doc.board.worktree_id.clone())
}

/// Whether another run may be working in the same checkout as this one.
///
/// Only a board that runs every card in its own worktree shares one; each card of a
/// card-worktree board has a checkout of its own.
fn shares_worktree(board: &Board) -> bool {
    board.settings.run_location.is_board_worktree() && board.settings.max_live_runs() > 1
}

/// Assembles what the delegation service needs to start one card's run.
///
/// Every refusal it can raise is a *card* fact: the board is not a worktree board, or a column
/// carries an environment entry no run may set. They are returned rather than raised so the
/// caller records them on the card, where the person who wrote the column can read them.
fn card_request(
    board: &Board,
    card: &Card,
    action: &Action,
    key: &str,
    row: &RunRow,
    mode: fleet_core::agents::PermissionMode,
) -> Result<CardRunRequest, DaemonError> {
    let worktree = run_worktree(board, card, key)?;
    validate_env(&action.env)?;
    // `validate_env` has already refused every entry without an `=`, so the filter drops nothing
    // a board can actually hold.
    let env = action
        .env
        .iter()
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, value)| (name.to_owned(), render_card_template(value, key, card)))
        .collect();
    let reports: Vec<&Comment> = card
        .comments
        .iter()
        .filter(|comment| comment.run_id.is_some())
        .collect();
    Ok(CardRunRequest {
        board: board.id.clone(),
        card: card.id.clone(),
        key: key.to_owned(),
        worktree,
        provider: row.provider,
        brief: brief(action, key, card, &reports),
        expectation: action.expect.clone(),
        mode,
        model: model_selection(row.model.clone(), row.effort.clone()),
        title: card_child_title(key, &card.title),
        env,
    })
}

/// The run row a successful start writes: identity, thread, and what it was started with.
fn started(row: &RunRow, delegation: &Delegation, now: &str) -> CardRun {
    CardRun {
        id: delegation.id,
        thread_id: Some(delegation.child),
        status_id: row.status_id.clone(),
        action: row.action.clone(),
        provider: row.provider,
        model: row.model.clone(),
        effort: row.effort.clone(),
        started_at: now.to_owned(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    }
}

/// The run row a refused start writes: a run that began and ended in the same breath.
///
/// It carries a freshly minted id because no delegation exists to lend it one, and no thread,
/// which is what `CardRun::failed_to_start` reads to tell this row from every other failure.
fn failed(row: &RunRow, error: &DaemonError, now: &str) -> CardRun {
    CardRun {
        id: DelegationId::new(),
        thread_id: None,
        status_id: row.status_id.clone(),
        action: row.action.clone(),
        provider: row.provider,
        model: row.model.clone(),
        effort: row.effort.clone(),
        started_at: now.to_owned(),
        ended_at: Some(now.to_owned()),
        outcome: Some(RunOutcome::Failed),
        detail: Some(error.to_string()),
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    }
}

/// A run row, stamped with the worktree it ran in — or would have, for a refused start.
///
/// Kept apart from [`started`] and [`failed`] because boot recovery builds the row of a run it
/// adopts from [`started`] too, with no prepared start to read the worktree from.
fn in_worktree(run: CardRun, worktree: Option<WorktreeId>) -> CardRun {
    CardRun {
        worktree_id: worktree,
        ..run
    }
}

/// Appends a run to the card, dropping the oldest once the cap is reached.
fn push_run(card: &mut Card, run: CardRun, now: &str) {
    card.runs.push(run);
    if card.runs.len() > MAX_RUNS_PER_CARD {
        card.runs.drain(..card.runs.len() - MAX_RUNS_PER_CARD);
    }
    card.updated_at = now.into();
}

/// Keeps at most [`MAX_REPORT_COMMENTS_PER_CARD`] report excerpts, oldest dropped first.
///
/// The run that owned a dropped comment loses its `report_comment_id` in the same pass: a card
/// detail that offers to open a comment the document no longer holds is worse than one that
/// offers nothing.
fn rotate_reports(card: &mut Card) {
    let reports = card
        .comments
        .iter()
        .filter(|comment| comment.run_id.is_some())
        .count();
    let mut excess = reports.saturating_sub(MAX_REPORT_COMMENTS_PER_CARD);
    if excess == 0 {
        return;
    }
    let mut dropped: Vec<String> = Vec::with_capacity(excess);
    card.comments.retain(|comment| {
        if excess == 0 || comment.run_id.is_none() {
            return true;
        }
        excess -= 1;
        dropped.push(comment.id.clone());
        false
    });
    for run in &mut card.runs {
        if run
            .report_comment_id
            .as_ref()
            .is_some_and(|id| dropped.contains(id))
        {
            run.report_comment_id = None;
        }
    }
}

/// Caps a report at [`REPORT_EXCERPT_CAP_BYTES`], on a character boundary, saying that it did.
fn report_excerpt(report: &str) -> String {
    if report.len() <= REPORT_EXCERPT_CAP_BYTES {
        return report.to_owned();
    }
    let mut cut = REPORT_EXCERPT_CAP_BYTES;
    while cut > 0 && !report.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…\n\n{REPORT_ELIDED}", &report[..cut])
}

/// The changed-file section appended to a run's report comment, or nothing at all.
///
/// A run that changed nothing gets no heading: an empty list under a heading reads as a claim,
/// and the heading is also written when the diff could not be taken at all. The list keeps the
/// order [`Checkpoints::changed_since`] returned it in, which is already sorted by path.
///
/// It reaches the next run of the card through `brief`'s previous-reports part, which is why it
/// lives in the comment body rather than anywhere a later brief would have to look for it.
fn files_section(changed: &[ChangedFile], shared_worktree: bool) -> String {
    if changed.is_empty() {
        return String::new();
    }
    let mut section = format!("\n\n{FILES_HEADING}\n");
    for file in changed {
        let mark = match file.kind {
            ChangeKind::Modified => 'M',
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
        };
        section.push('\n');
        section.push(mark);
        section.push(' ');
        section.push_str(&file.path);
    }
    if shared_worktree {
        section.push_str("\n\n");
        section.push_str(FILES_SHARED_WORKTREE);
    }
    section
}

/// `Run ended · {outcome word} · {Nm SSs}`, plus the cost when the provider reported one.
fn run_ended(outcome: RunOutcome, elapsed_seconds: Option<i64>, cost_usd: Option<f64>) -> String {
    let seconds = elapsed_seconds.unwrap_or_default().max(0);
    let mut message = format!(
        "Run ended · {} · {}m {:02}s",
        outcome.word(),
        seconds / 60,
        seconds % 60
    );
    if let Some(cost) = cost_usd {
        message.push_str(&format!(" · ${cost:.2}"));
    }
    message
}

/// The card outcome one terminal delegation status means (contracts §3.5).
///
/// `None` for a delegation that is not terminal, which the hook is never called with.
fn outcome_of(delegation: &Delegation) -> Option<RunOutcome> {
    match delegation.status {
        DelegationStatus::Succeeded => Some(RunOutcome::Succeeded),
        // A child that reported itself blocked is not a failure to fix; it is a question for the
        // person who owns the card, and `attention` is what puts it in front of them.
        DelegationStatus::Failed
            if delegation.status_payload.as_deref() == Some("reported blocked") =>
        {
            Some(RunOutcome::NeedsYou)
        }
        DelegationStatus::Failed => Some(RunOutcome::Failed),
        DelegationStatus::Incomplete => Some(RunOutcome::Incomplete),
        DelegationStatus::Cancelled => Some(RunOutcome::Cancelled),
        DelegationStatus::Starting
        | DelegationStatus::Running
        | DelegationStatus::Blocked
        | DelegationStatus::Settling => None,
    }
}

/// Rewrites the entry [`move_card`] just appended as automation's own sentence.
///
/// `Moved` is reserved for a human's or the CLI's move: `ops::attention` reads a `Moved` newer
/// than a run's end as "a person has seen this", so an automatic move that left one behind would
/// clear the very attention it should be raising.
fn record_auto_move(card: &mut Card, message: String, now: &str) {
    if card
        .activity
        .last()
        .is_some_and(|entry| entry.kind == ActivityKind::Moved)
    {
        card.activity.pop();
    }
    push_activity(card, ActivityKind::AutoMoved, None, message, now);
}

/// One live delegation as the read join states it, or `None` when no card called it.
fn card_live_run(delegation: &Delegation) -> Option<LiveRun> {
    let (_, card_id) = delegation.caller.card()?;
    Some(LiveRun {
        card_id: card_id.clone(),
        run: delegation.id,
        status: delegation.status,
        headline: delegation.headline.clone(),
        started: delegation.created.to_rfc3339(),
    })
}

/// The live delegation this card's newest run names, if that run has not ended.
pub(super) fn live_run(card: &Card) -> Option<DelegationId> {
    latest_run(card)
        .filter(|run| run.is_live() && !run.failed_to_start())
        .map(|run| run.id)
}

/// Whether any card on the board is still parked behind the live-run ceiling.
fn holds_pending(cards: &[Card]) -> bool {
    cards.iter().any(|card| card.pending_run.is_some())
}

/// Builds the model selection one run asks for, or `None` to leave both to the provider.
///
/// An effort with no model travels as the empty-string sentinel `ModelSelection.model`
/// documents: the harness keeps its configured model and still spends the effort.
fn model_selection(model: Option<String>, effort: Option<String>) -> Option<ModelSelection> {
    match (model, effort) {
        (None, None) => None,
        (model, effort) => Some(ModelSelection {
            model: model.unwrap_or_default(),
            effort,
            provider: None,
        }),
    }
}

/// The column with this id.
fn column<'a>(board: &'a Board, status_id: &StatusId) -> Option<&'a Status> {
    board.statuses.iter().find(|status| status.id == *status_id)
}

/// What a column runs on a card that enters it, if anything.
fn on_enter<'a>(board: &'a Board, status_id: &StatusId) -> Option<&'a Action> {
    column(board, status_id)
        .and_then(|status| status.automation.as_ref())
        .and_then(|automation| automation.on_enter.as_ref())
}

/// A column's name, falling back to its id on a board that no longer carries it.
fn column_name(board: &Board, status_id: &StatusId) -> String {
    column(board, status_id).map_or_else(|| status_id.to_string(), |status| status.name.clone())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use fleet_core::{
        agents::{
            Delegation, DelegationCaller, DelegationResult, DeliveryState, ResultSource, ThreadId,
        },
        board::{ColumnAgentPrefs, ops::push_activity},
    };

    use super::*;

    const NOW: &str = "2026-09-21T12:00:00Z";

    /// A card carrying only its required wire fields, which is what a defaulted card is.
    fn card() -> Card {
        serde_json::from_value(serde_json::json!({
            "id": "card-1",
            "boardId": "work",
            "number": 7,
            "title": "Fix login",
            "statusId": "in-progress",
            "createdAt": NOW,
            "updatedAt": NOW,
        }))
        .expect("a card built from its required wire fields alone")
    }

    fn row() -> RunRow {
        RunRow {
            status_id: "in-progress"
                .parse()
                .expect("a static status slug is valid"),
            action: ActionKind::Prompt,
            provider: AgentKind::Codex,
            model: Some("gpt-5".into()),
            effort: Some("high".into()),
        }
    }

    fn delegation(status: DelegationStatus, payload: Option<&str>) -> Delegation {
        Delegation {
            id: DelegationId::new(),
            caller: DelegationCaller::Card {
                board: "work".parse().expect("a static board id is valid"),
                card: "card-1".parse().expect("a static card id is valid"),
            },
            caller_turn: None,
            caller_item: None,
            child: ThreadId::new(),
            provider: AgentKind::Codex,
            depth: 1,
            brief: "Fix login".into(),
            expectation: "tests pass".into(),
            eager: false,
            status,
            status_payload: payload.map(str::to_owned),
            result: Some(DelegationResult {
                text: "Done.".into(),
                files_changed: vec![],
                source: ResultSource::Reported,
                elided: false,
            }),
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: DateTime::<Utc>::from_timestamp(1_700_000_000, 0)
                .expect("a static timestamp is valid"),
            finished: None,
            headline: None,
            usage: None,
        }
    }

    fn report(id: &str, run: DelegationId, created_at: &str) -> Comment {
        Comment {
            id: id.into(),
            author: None,
            body: "report".into(),
            created_at: created_at.into(),
            remote_id: None,
            run_id: Some(run),
        }
    }

    fn run_with_report(id: DelegationId, comment: &str) -> CardRun {
        let mut run = started(&row(), &delegation(DelegationStatus::Succeeded, None), NOW);
        run.id = id;
        run.report_comment_id = Some(comment.into());
        run
    }

    /// Every terminal status the delegation service can hand the board, including the one the
    /// child reports for itself: blocked is a question for a person, not a failure to fix.
    #[test]
    fn every_terminal_delegation_status_maps_to_one_card_outcome() {
        let cases = [
            (DelegationStatus::Succeeded, None, RunOutcome::Succeeded),
            (
                DelegationStatus::Failed,
                Some("reported blocked"),
                RunOutcome::NeedsYou,
            ),
            (
                DelegationStatus::Failed,
                Some("the provider exited"),
                RunOutcome::Failed,
            ),
            (DelegationStatus::Failed, None, RunOutcome::Failed),
            (DelegationStatus::Incomplete, None, RunOutcome::Incomplete),
            (DelegationStatus::Cancelled, None, RunOutcome::Cancelled),
        ];
        for (status, payload, expected) in cases {
            assert_eq!(
                outcome_of(&delegation(status, payload)),
                Some(expected),
                "{status:?} with {payload:?}"
            );
        }
    }

    #[test]
    fn a_delegation_that_is_still_working_records_no_outcome() {
        for status in [
            DelegationStatus::Starting,
            DelegationStatus::Running,
            DelegationStatus::Blocked,
            DelegationStatus::Settling,
        ] {
            assert_eq!(outcome_of(&delegation(status, None)), None, "{status:?}");
        }
    }

    /// The sentence `docs/BOARD.md` §11.3 fixes, including the two-digit seconds.
    #[test]
    fn the_run_ended_sentence_has_the_contract_format() {
        assert_eq!(
            run_ended(RunOutcome::Succeeded, Some(842), None),
            "Run ended · succeeded · 14m 02s"
        );
        assert_eq!(
            run_ended(RunOutcome::NeedsYou, Some(59), Some(1.5)),
            "Run ended · needs you · 0m 59s · $1.50"
        );
        assert_eq!(
            run_ended(RunOutcome::Failed, None, None),
            "Run ended · failed · 0m 00s"
        );
    }

    #[test]
    fn a_report_under_the_cap_is_kept_whole() {
        assert_eq!(report_excerpt("all green"), "all green");
    }

    /// The cut lands on a character boundary and says where the rest of the report is: a card
    /// comment is an excerpt, and a reader who cannot tell that would read a truncated report as
    /// the whole one.
    #[test]
    fn a_report_over_the_cap_is_cut_on_a_character_boundary_and_says_so() {
        let report = "é".repeat(REPORT_EXCERPT_CAP_BYTES);
        let excerpt = report_excerpt(&report);
        assert!(
            excerpt.ends_with(&format!("…\n\n{REPORT_ELIDED}")),
            "{excerpt}"
        );
        let kept = excerpt
            .strip_suffix(&format!("…\n\n{REPORT_ELIDED}"))
            .expect("the excerpt ends with the elision");
        assert!(kept.len() <= REPORT_EXCERPT_CAP_BYTES);
        assert!(report.starts_with(kept));
        assert_eq!(kept.chars().count(), REPORT_EXCERPT_CAP_BYTES / 2);
    }

    fn changed(path: &str, kind: ChangeKind) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            kind,
        }
    }

    /// A run that changed nothing must not claim a heading: an empty list under one reads as
    /// "these are the files", and the same empty list is what a diff that could not be taken at
    /// all answers.
    #[test]
    fn a_run_that_changed_nothing_appends_no_section() {
        assert_eq!(files_section(&[], false), "");
        assert_eq!(files_section(&[], true), "");
    }

    #[test]
    fn the_files_section_marks_each_path_and_keeps_the_order_it_was_given() {
        let section = files_section(
            &[
                changed("crates/fleet-core/src/board.rs", ChangeKind::Modified),
                changed("crates/fleet-core/src/board/new.rs", ChangeKind::Added),
                changed("crates/fleet-core/src/board/old.rs", ChangeKind::Deleted),
            ],
            false,
        );
        assert_eq!(
            section,
            format!(
                "\n\n{FILES_HEADING}\n\nM crates/fleet-core/src/board.rs\n\
                 A crates/fleet-core/src/board/new.rs\nD crates/fleet-core/src/board/old.rs"
            )
        );
    }

    /// The sentence is the whole mitigation for a shared checkout: the diff is tree-to-tree, so a
    /// concurrent run's edits are in the list and nothing can tell them apart.
    #[test]
    fn a_board_that_runs_more_than_one_card_at_once_says_the_worktree_is_shared() {
        let section = files_section(&[changed("a.rs", ChangeKind::Modified)], true);
        assert!(
            section.ends_with(&format!("M a.rs\n\n{FILES_SHARED_WORKTREE}")),
            "{section}"
        );
        assert!(
            !files_section(&[changed("a.rs", ChangeKind::Modified)], false)
                .contains(FILES_SHARED_WORKTREE)
        );
    }

    /// The cap is applied to the report and the section is appended after it, so what goes
    /// missing from an over-long report is the agent's prose and never the file list.
    #[test]
    fn the_files_section_survives_a_report_that_is_over_the_cap() {
        let report = "x".repeat(REPORT_EXCERPT_CAP_BYTES * 2);
        let body = format!(
            "{}{}",
            report_excerpt(&report),
            files_section(&[changed("a.rs", ChangeKind::Added)], false)
        );
        assert!(body.contains(REPORT_ELIDED), "the report was capped");
        assert!(body.ends_with("A a.rs"), "{}", &body[body.len() - 80..]);
    }

    /// Past three, the oldest excerpt goes and the run that owned it stops offering to open it.
    #[test]
    fn report_comments_rotate_at_three_and_clear_the_run_that_owned_them() {
        let mut card = card();
        let ids: Vec<DelegationId> = (0..4).map(|_| DelegationId::new()).collect();
        card.comments.push(Comment {
            id: "human".into(),
            author: Some("danny".into()),
            body: "have a look".into(),
            created_at: NOW.into(),
            remote_id: None,
            run_id: None,
        });
        for (index, id) in ids.iter().enumerate() {
            card.comments
                .push(report(&format!("report-{index}"), *id, NOW));
            card.runs
                .push(run_with_report(*id, &format!("report-{index}")));
        }
        rotate_reports(&mut card);
        let kept: Vec<&str> = card
            .comments
            .iter()
            .map(|comment| comment.id.as_str())
            .collect();
        assert_eq!(kept, vec!["human", "report-1", "report-2", "report-3"]);
        assert_eq!(card.runs[0].report_comment_id, None);
        assert_eq!(
            card.runs[1].report_comment_id.as_deref(),
            Some("report-1"),
            "a run whose excerpt survived keeps it"
        );
    }

    #[test]
    fn runs_are_capped_at_twenty_with_the_oldest_dropped() {
        let mut card = card();
        let first = DelegationId::new();
        for index in 0..=MAX_RUNS_PER_CARD {
            let mut run = started(&row(), &delegation(DelegationStatus::Succeeded, None), NOW);
            if index == 0 {
                run.id = first;
            }
            push_run(&mut card, run, NOW);
        }
        assert_eq!(card.runs.len(), MAX_RUNS_PER_CARD);
        assert!(card.runs.iter().all(|run| run.id != first));
        assert_eq!(card.updated_at, NOW);
    }

    /// `Moved` is a person's word. An automatic move that left one behind would tell `attention`
    /// a human had already looked at the run it is trying to raise.
    #[test]
    fn an_automatic_move_replaces_the_move_entry_it_produced() {
        let mut card = card();
        push_activity(
            &mut card,
            ActivityKind::Moved,
            None,
            "Moved from a to b",
            NOW,
        );
        record_auto_move(&mut card, "Moved to In review: run succeeded".into(), NOW);
        assert_eq!(card.activity.len(), 1);
        assert_eq!(card.activity[0].kind, ActivityKind::AutoMoved);
        assert_eq!(
            card.activity[0].message,
            "Moved to In review: run succeeded"
        );
    }

    #[test]
    fn an_automatic_move_keeps_an_entry_it_did_not_write() {
        let mut card = card();
        push_activity(
            &mut card,
            ActivityKind::Commented,
            None,
            "Added a comment",
            NOW,
        );
        record_auto_move(&mut card, "Moved to Done: run succeeded".into(), NOW);
        assert_eq!(card.activity.len(), 2);
        assert_eq!(card.activity[0].kind, ActivityKind::Commented);
    }

    /// The read join states a card's live run and drops everything that is not one.
    #[test]
    fn the_live_run_join_maps_a_card_caller_and_drops_a_thread_caller() {
        let mut card_called = delegation(DelegationStatus::Running, None);
        card_called.headline = Some("reading the tests".to_owned());
        let live = card_live_run(&card_called).expect("a card caller joins");
        assert_eq!(live.card_id.as_str(), "card-1");
        assert_eq!(live.run, card_called.id);
        assert_eq!(live.status, DelegationStatus::Running);
        assert_eq!(live.headline.as_deref(), Some("reading the tests"));
        assert_eq!(live.started, card_called.created.to_rfc3339());

        let mut thread_called = delegation(DelegationStatus::Running, None);
        thread_called.caller = fleet_core::agents::DelegationCaller::Thread(ThreadId::new());
        assert!(
            card_live_run(&thread_called).is_none(),
            "a thread's child belongs to no card and joins onto no board"
        );
    }

    /// A refused start has no thread, so nothing can attach to it and nothing waits on it.
    #[test]
    fn a_run_that_never_started_is_terminal_and_has_no_thread() {
        let mut card = card();
        let run = failed(
            &row(),
            &DaemonError::from(BoardError::Invalid {
                field: "automation".into(),
                reason: "automation is available on worktree boards only".into(),
            }),
            NOW,
        );
        assert!(run.failed_to_start());
        assert!(!run.is_live());
        assert_eq!(run.outcome, Some(RunOutcome::Failed));
        assert_eq!(run.ended_at.as_deref(), Some(NOW));
        assert_eq!(
            run.detail.as_deref(),
            Some(
                "validation failed: invalid automation: automation is available on worktree boards only"
            )
        );
        push_run(&mut card, run, NOW);
        assert_eq!(live_run(&card), None);
    }

    #[test]
    fn a_started_run_names_the_child_thread_and_is_live_until_it_ends() {
        let mut card = card();
        let delegation = delegation(DelegationStatus::Running, None);
        let run = started(&row(), &delegation, NOW);
        assert_eq!(run.thread_id, Some(delegation.child));
        assert!(run.is_live());
        assert_eq!(run.files_changed, 0, "phase 3 records no diff");
        push_run(&mut card, run, NOW);
        assert_eq!(live_run(&card), Some(delegation.id));
    }

    /// An effort with no model is the documented empty-string sentinel: the harness keeps its
    /// configured model and still spends the effort.
    #[test]
    fn an_effort_without_a_model_travels_as_the_empty_model_sentinel() {
        assert_eq!(model_selection(None, None), None);
        assert_eq!(
            model_selection(None, Some("high".into())),
            Some(ModelSelection {
                model: String::new(),
                effort: Some("high".into()),
                provider: None,
            })
        );
        assert_eq!(
            model_selection(Some("opus".into()), None),
            Some(ModelSelection {
                model: "opus".into(),
                effort: None,
                provider: None,
            })
        );
    }

    /// The column's environment is rendered per card, and a `FLEET_` key is refused before a run
    /// is minted rather than being quietly overwritten by the daemon's own identity.
    #[test]
    fn the_column_environment_is_templated_per_card_and_refuses_the_fleet_namespace() {
        let board = board();
        let card = card();
        let mut action = action();
        action.env = vec!["BRANCH=work/{key}".into()];
        let request = card_request(
            &board,
            &card,
            &action,
            "FLT-7",
            &row(),
            fleet_core::agents::PermissionMode::FullAccess,
        )
        .expect("a worktree board with a legal column environment");
        assert_eq!(
            request.env,
            vec![("BRANCH".to_owned(), "work/FLT-7".to_owned())]
        );
        assert_eq!(request.title, "↳ FLT-7 — Fix login");
        assert_eq!(request.key, "FLT-7");

        action.env = vec!["FLEET_CARD=other".into()];
        // `let Err(..) else` rather than `expect_err`: a `CardRunRequest` carries a whole brief
        // and is deliberately not `Debug`.
        let Err(refused) = card_request(
            &board,
            &card,
            &action,
            "FLT-7",
            &row(),
            fleet_core::agents::PermissionMode::FullAccess,
        ) else {
            panic!("a column may not set a FLEET_ key");
        };
        assert!(refused.to_string().contains("FLEET_CARD"), "{refused}");
    }

    /// A board with no worktree has nowhere to run: the refusal is recorded on the card, in the
    /// words §1.7 fixes for every surface.
    #[test]
    fn a_board_with_no_worktree_refuses_the_run_in_the_contract_sentence() {
        let mut board = board();
        board.worktree_id = None;
        let Err(refused) = card_request(
            &board,
            &card(),
            &action(),
            "FLT-7",
            &row(),
            fleet_core::agents::PermissionMode::FullAccess,
        ) else {
            panic!("automation needs a worktree board");
        };
        assert_eq!(
            refused.to_string(),
            "validation failed: invalid automation: automation is available on worktree boards only"
        );
    }

    #[test]
    fn a_card_parked_behind_the_ceiling_marks_its_board_as_pending() {
        let mut card = card();
        assert!(!holds_pending(std::slice::from_ref(&card)));
        card.pending_run = Some(fleet_core::board::PendingRun {
            status_id: "in-progress"
                .parse()
                .expect("a static status slug is valid"),
            since: NOW.into(),
        });
        assert!(holds_pending(std::slice::from_ref(&card)));
    }

    #[test]
    fn a_column_the_board_no_longer_carries_is_named_by_its_id() {
        let board = board();
        assert_eq!(
            column_name(
                &board,
                &"gone".parse().expect("a static status slug is valid")
            ),
            "gone"
        );
        assert!(
            on_enter(
                &board,
                &"gone".parse().expect("a static status slug is valid")
            )
            .is_none()
        );
    }

    fn action() -> Action {
        Action {
            kind: ActionKind::Prompt,
            instructions: String::new(),
            expect: String::new(),
            agent: ColumnAgentPrefs::default(),
            env: Vec::new(),
        }
    }

    fn board() -> Board {
        let mut board = fleet_core::board::new_board(
            &fleet_core::model::Context {
                id: "work".parse().expect("a static context slug is valid"),
                name: "Work".into(),
                owners: vec![],
                created_at: NOW.into(),
            },
            NOW,
        );
        board.worktree_id = Some(
            "acme/api#feature"
                .parse()
                .expect("a static worktree id is valid"),
        );
        board
    }

    /// A run with this delegation id, recorded in `worktree`.
    fn run_in(id: DelegationId, worktree: Option<&str>) -> CardRun {
        CardRun {
            worktree_id: worktree.map(|id| id.parse().expect("a static worktree id is valid")),
            ..started(&row(), &delegation_with(id), NOW)
        }
    }

    fn delegation_with(id: DelegationId) -> Delegation {
        Delegation {
            id,
            ..delegation(DelegationStatus::Succeeded, None)
        }
    }

    fn document(board: Board, runs: Vec<CardRun>) -> BoardDocument {
        let mut card = card();
        card.runs = runs;
        BoardDocument {
            version: fleet_core::board::document_version(&board, std::slice::from_ref(&card)),
            board,
            cards: vec![card],
        }
    }

    #[test]
    fn a_card_worktree_run_diffs_its_own_worktree() {
        let delivered = DelegationId::new();
        let mut board = board();
        board.settings.run_location = fleet_core::board::RunLocation::CardWorktree;
        let doc = document(
            board,
            vec![
                run_in(DelegationId::new(), Some("acme/api#review-8")),
                run_in(delivered, Some("acme/api#review-7")),
            ],
        );
        assert_eq!(
            diff_worktree(&doc, delivered).map(|id| id.to_string()),
            Some("acme/api#review-7".to_owned()),
            "the delivered run's own worktree, not the board's or another run's"
        );
    }

    #[test]
    fn a_run_recorded_before_worktree_ids_diffs_the_board_worktree() {
        let delivered = DelegationId::new();
        let doc = document(board(), vec![run_in(delivered, None)]);
        assert_eq!(
            diff_worktree(&doc, delivered).map(|id| id.to_string()),
            Some("acme/api#feature".to_owned())
        );
    }

    #[test]
    fn card_worktree_runs_never_print_the_shared_sentence() {
        let changed = [changed("src/lib.rs", ChangeKind::Modified)];
        let mut board = board();
        board.settings.max_live_runs = Some(3);
        assert!(
            shares_worktree(&board),
            "a board worktree run by three is shared"
        );
        assert!(files_section(&changed, shares_worktree(&board)).contains(FILES_SHARED_WORKTREE));

        board.settings.run_location = fleet_core::board::RunLocation::CardWorktree;
        assert!(!shares_worktree(&board));
        let section = files_section(&changed, shares_worktree(&board));
        assert!(section.contains("M src/lib.rs"));
        assert!(!section.contains(FILES_SHARED_WORKTREE), "{section}");
    }

    #[test]
    fn a_card_worktree_board_runs_each_card_in_the_card_worktree() {
        let mut board = board();
        board.settings.run_location = fleet_core::board::RunLocation::CardWorktree;
        let mut card = card();
        let key = card.display_key(&board);
        let refused = card_request(
            &board,
            &card,
            &action(),
            &key,
            &row(),
            fleet_core::agents::PermissionMode::default(),
        )
        .map(|request| request.worktree);
        match refused {
            Err(error) => assert_eq!(
                error.to_string(),
                format!(
                    "validation failed: invalid automation: {key} has no worktree to run in; link a pull request or create its worktree first"
                )
            ),
            Ok(worktree) => panic!("a card with no worktree ran in {worktree}"),
        }

        card.worktree_id = Some(
            "acme/api#review-7"
                .parse()
                .expect("a static worktree id is valid"),
        );
        let request = card_request(
            &board,
            &card,
            &action(),
            &key,
            &row(),
            fleet_core::agents::PermissionMode::default(),
        )
        .expect("a linked card builds its request");
        assert_eq!(request.worktree.to_string(), "acme/api#review-7");
    }
}
