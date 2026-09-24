//! Boot recovery, the freed slot, and the memo that keeps both cheap.
//!
//! Split from [`super`] because these three are the only automation paths no request drives: the
//! daemon's own maintenance task calls them, once at start and then on every terminal delegation
//! the bus reports.
//!
//! **The lock order is gate → memo.** Every other writer notes the memo while it still holds the
//! board's gate, so nothing here may hold `pending_boards` across a `gate(..).await`:
//! [`Boards::on_slot_released`] copies the memo and drops its guard before it touches a board.
//!
//! **No delegation read happens under a gate.** Boot recovery asks the delegation service what it
//! still knows *before* it takes each board's gate, applies the answers in one write, and leaves
//! the runs it decided to start to [`Boards::apply_starts`] afterwards.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{Delegation, DelegationId, DeliveryState},
    board::{
        ActivityKind, BoardDocument, Card, CardRun, Plan, RunOutcome, clear_stale_queues,
        latest_run, next_pending_except, push_activity, resolve_prefs,
    },
    ids::{BoardId, CardId},
};
use fleet_proto::{error::ErrorKind, event::BoardChangeReason, response::ResponseBody};

use super::{
    Automation, RunRow, holds_pending, in_worktree, live_run, on_enter, push_run, run_ended,
    run_worktree, started,
};
use crate::{
    DaemonResult, error::from_proto_error, services::agents::delegation::RunDeliveryHook,
    services::boards::Boards,
};

/// What a run whose delegation this daemon no longer holds is closed with.
const LOST_RECORD: &str =
    "the daemon lost this run's record while it was down, so what it did was never reported";

/// What the card says about a run the daemon adopted rather than started.
const ADOPTED: &str = "Run adopted after the daemon restarted";

/// What boot recovery must do about one open run row.
#[derive(Debug, PartialEq, Eq)]
enum Disposition {
    /// The run is still going, or its delivery is still owed: the worker closes the row.
    Adopt,
    /// The run ended and nothing will deliver it again: this sweep records its outcome.
    Deliver,
    /// The delegation is gone, so nothing can ever say how this run ended.
    Close,
}

impl Boards {
    /// Adopts what a restart left behind, once, after the delegation worker's first drain.
    ///
    /// # Errors
    ///
    /// The failure of the board write that adopted or closed a run. One board's failure does not
    /// stop the sweep — every board is swept and the first failure is returned — because a single
    /// unreadable document must not leave every other board's runs unrecovered.
    pub(crate) async fn resume_automation(&self) -> DaemonResult<()> {
        let Some(automation) = self.automation() else {
            return Ok(());
        };
        let mut failure = None;
        for board in self.automated_boards() {
            if let Err(error) = self.resume_board(automation, &board).await {
                tracing::warn!(%board, %error, "a board's runs could not be resumed after the restart");
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Hands a slot a finished run freed to the board that has waited longest for it.
    ///
    /// # Errors
    ///
    /// The failure of the board write that started the next card.
    pub(crate) async fn on_slot_released(&self) -> DaemonResult<()> {
        let Some(automation) = self.automation() else {
            return Ok(());
        };
        // Copied, not walked under the guard: every other writer notes the memo while it holds a
        // board gate, so taking a gate with this guard held is the one lock order that deadlocks
        // the service. A board that stops waiting between here and its gate costs one reload.
        let waiting: Vec<BoardId> = automation
            .pending_boards
            .lock()
            .await
            .iter()
            .cloned()
            .collect();
        let mut failure = None;
        for board in waiting {
            if let Err(error) = self.release_slot(&board).await {
                tracing::warn!(%board, %error, "a freed run slot could not be handed to the next card");
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Records whether a board still holds a card parked behind the live-run ceiling.
    ///
    /// Written wherever a `pending_run` is set or cleared, so [`Boards::on_slot_released`] can
    /// skip every board that is not waiting instead of reloading all of them.
    pub(crate) async fn note_pending(&self, board: &BoardId, has_pending: bool) {
        let Some(automation) = self.automation() else {
            return;
        };
        let mut pending = automation.pending_boards.lock().await;
        if has_pending {
            pending.insert(board.clone());
        } else {
            pending.remove(board);
        }
    }

    /// Gives one board's freed slot to the card that has waited longest for it.
    ///
    /// The card may be parked in the action column itself, or queued in the routing column
    /// before it (`queued`): seeding it through the non-settled walk lets the engine's rule 0
    /// move a queued card into the column it waits for, and rule 1 then starts it there.
    ///
    /// A board with nothing parked leaves the memo here: the memo is a hint kept by whoever wrote
    /// a `pending_run`, and a board that never reloads would otherwise be re-read on every
    /// terminal delegation for the life of the daemon.
    pub(super) async fn release_slot(&self, board: &BoardId) -> DaemonResult<()> {
        let (next_board, plan) = {
            let _guard = self.gate(board).await;
            let now = self.now();
            let mut doc = self.load(board)?;
            // A queued card rule 0 would refuse (blocked since it queued, or its column no
            // longer routes where it waits) must not take the slot: it would start nothing, and
            // it stays first in line, so every card behind it would wait for ever.
            let cleared = clear_stale_queues(&doc.board, &mut doc.cards);
            // A card an earlier freed slot went to keeps its marker until its start is recorded.
            // Two runs ending together must fill two slots, so this one passes over it.
            let reserved = match self.automation() {
                Some(automation) => automation
                    .in_flight
                    .lock()
                    .await
                    .get(board)
                    .map(|reservations| reservations.keys().cloned().collect())
                    .unwrap_or_default(),
                None => BTreeSet::new(),
            };
            let Some(next) =
                next_pending_except(&doc.board, &doc.cards, &reserved).map(|card| card.id.clone())
            else {
                if cleared {
                    doc.board.updated_at = now;
                    self.save(&doc, BoardChangeReason::CardChanged).await?;
                }
                self.note_pending(board, false).await;
                return Ok(());
            };
            let plan = self
                .evaluate_with_reservation(&mut doc, std::slice::from_ref(&next), &now)
                .await?;
            if cleared || decided(&plan, &doc.cards, &now) {
                doc.board.updated_at = now;
                self.save(&doc, BoardChangeReason::CardChanged).await?;
            }
            self.note_pending(board, holds_pending(&doc.cards)).await;
            (doc.board, plan)
        };
        // Handed off on a card-worktree board: the maintenance loop that frees slots must not
        // wait behind the next card's pull-request fetch.
        self.apply_starts_after_answer(&next_board, plan).await
    }

    /// Every board a run could belong to: automation happens in a worktree and nowhere else —
    /// the board's own, or, on a board that runs each card in the card's worktree, the card's.
    ///
    /// An unreadable document is skipped rather than fatal, exactly as the snapshot scan skips it:
    /// boot recovery reports it once through [`Boards::scan_load`] and recovers the rest.
    fn automated_boards(&self) -> Vec<BoardId> {
        let ids = match self.store.list() {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(%error, "the board store could not be listed for boot recovery");
                return Vec::new();
            }
        };
        ids.into_iter()
            .filter(|id| {
                self.scan_load(id).is_some_and(|doc| {
                    doc.board.worktree_id.is_some()
                        || !doc.board.settings.run_location.is_board_worktree()
                })
            })
            .collect()
    }

    /// Adopts, closes and restarts what one board's restart left behind.
    async fn resume_board(&self, automation: &Automation, board: &BoardId) -> DaemonResult<()> {
        let doc = self.load(board)?;
        // Every delegation read happens here, before the gate: the board stays readable while the
        // agent database answers, and nothing in this module holds a gate across that call.
        let live = automation.delegations.live_for_board(board).await?;
        let mut adopt: Vec<(CardId, Delegation)> = Vec::new();
        let mut deliver: Vec<(CardId, Delegation)> = Vec::new();
        let mut close: Vec<(CardId, DelegationId)> = Vec::new();
        for (card, run) in open_runs(&doc.cards) {
            if live.iter().any(|delegation| delegation.id == run) {
                // Still going, and the service agrees: the row stands as it is.
                continue;
            }
            let record = match self.read_run(automation, run).await {
                Ok(record) => record,
                // A storage failure is not an answer. Leaving the row open keeps the card refusing
                // moves until the next restart, which is the safe half of the choice.
                Err(error) => {
                    tracing::warn!(%board, %run, %error, "a card run's delegation could not be read");
                    continue;
                }
            };
            match disposition(record.as_ref()) {
                Disposition::Adopt => {}
                Disposition::Deliver => {
                    if let Some(record) = record {
                        deliver.push((card, record));
                    }
                }
                Disposition::Close => close.push((card, run)),
            }
        }
        // A delegation the service is still running that no row names is the crash window between
        // `run_for_card` answering and `record_run` writing: the run exists, so the card gets it.
        for delegation in live {
            let Some(card) = delegation.caller.card().map(|(_, card)| card.clone()) else {
                continue;
            };
            if !records_run(&doc.cards, &card, delegation.id) {
                adopt.push((card, delegation));
            }
        }
        // Before the gated pass, and each through the hook's own gate: a delivery writes the
        // outcome, the report and the `on_success` move, so the evaluation below sees a closed row
        // rather than deciding around a live one.
        for (card, delegation) in deliver {
            if let Err(error) = self.on_run_delivered(board, &card, &delegation).await {
                tracing::warn!(%board, %card, %error, "a card run left terminal by the restart could not be recorded");
            }
        }
        let (next_board, plan) = {
            let _guard = self.gate(board).await;
            let now = self.now();
            let mut doc = self.load(board)?;
            let mut written = false;
            for (card, delegation) in adopt {
                written |= adopt_run(&mut doc, &card, &delegation, &now);
            }
            for (card, run) in close {
                written |= self.close_lost_run(&mut doc, &card, run, &now);
            }
            // Only the cards the board still owes a run. Seeding every card would start one for
            // every card merely sitting in an action column — the engine's rule 1 cannot tell a
            // card that entered a column from one that has stood there since before the restart.
            let seeds = owed(&doc.cards);
            let plan = self
                .evaluate_with_reservation(&mut doc, &seeds, &now)
                .await?;
            if written || decided(&plan, &doc.cards, &now) {
                doc.board.updated_at = now;
                self.save(&doc, BoardChangeReason::CardChanged).await?;
            }
            self.note_pending(board, holds_pending(&doc.cards)).await;
            (doc.board, plan)
        };
        self.apply_starts_after_answer(&next_board, plan).await
    }

    /// The delegation behind one open run row; `None` when this daemon no longer holds it.
    ///
    /// # Errors
    ///
    /// Every failure that is not "no such delegation", so the caller can leave the row alone
    /// rather than close a run that may still be going.
    async fn read_run(
        &self,
        automation: &Automation,
        run: DelegationId,
    ) -> DaemonResult<Option<Delegation>> {
        match automation.delegations.get(run).await {
            Ok(ResponseBody::Delegation(record)) => Ok(Some(record)),
            Ok(other) => Err(crate::DaemonError::Protocol(format!(
                "the delegation service answered the read of run {run} with {:?}",
                std::mem::discriminant(&other)
            ))),
            Err(error) if error.kind == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(from_proto_error(error)),
        }
    }

    /// Closes a run whose delegation this daemon can no longer find.
    ///
    /// `Incomplete` rather than `Failed`: nobody knows what the child did before the record went,
    /// and `attention` is what puts the card in front of the person who has to decide.
    fn close_lost_run(
        &self,
        doc: &mut BoardDocument,
        card: &CardId,
        run: DelegationId,
        now: &str,
    ) -> bool {
        let Some(index) = doc.cards.iter().position(|other| other.id == *card) else {
            return false;
        };
        let Some(position) = doc.cards[index]
            .runs
            .iter()
            .position(|other| other.id == run && other.is_live())
        else {
            return false;
        };
        let elapsed = elapsed_seconds(
            &doc.cards[index].runs[position].started_at,
            self.clock.now(),
        );
        {
            let row = &mut doc.cards[index].runs[position];
            row.ended_at = Some(now.to_owned());
            row.outcome = Some(RunOutcome::Incomplete);
            row.detail = Some(LOST_RECORD.to_owned());
        }
        let message = run_ended(RunOutcome::Incomplete, elapsed, None);
        push_activity(
            &mut doc.cards[index],
            ActivityKind::RunEnded,
            None,
            message,
            now,
        );
        tracing::warn!(card = %card, %run, "closed a card run whose delegation the daemon no longer holds");
        true
    }
}

/// Whether an evaluation decided anything the document must be saved for.
///
/// `Plan::queued` is not the test on its own: a card already parked for the column it stands in, or
/// already queued in a routing column for the one after it, is re-queued by every walk that
/// reaches it without anything about it changing, and saving on that
/// would rewrite — and broadcast — a board document on every terminal delegation the daemon sees
/// while one of its cards waits. A park this evaluation actually wrote carries this `now`.
pub(super) fn decided(plan: &Plan, cards: &[Card], now: &str) -> bool {
    !plan.starts.is_empty()
        || !plan.moved.is_empty()
        || cards.iter().any(|card| {
            card.pending_run
                .as_ref()
                .is_some_and(|pending| pending.since == now)
        })
}

/// What a restart must do about an open row, given what the delegation service still says.
///
/// A terminal delegation whose delivery is still owed is adopted rather than recorded here: the
/// worker's own drain performs it, and performing it twice is only safe because
/// `on_run_delivered` is idempotent — not a reason to do it twice.
fn disposition(record: Option<&Delegation>) -> Disposition {
    let Some(record) = record else {
        return Disposition::Close;
    };
    if record.status.is_live() {
        return Disposition::Adopt;
    }
    match record.delivery {
        DeliveryState::Pending | DeliveryState::Delivered { .. } => Disposition::Adopt,
        DeliveryState::Consumed | DeliveryState::Undeliverable { .. } | DeliveryState::Recorded => {
            Disposition::Deliver
        }
    }
}

/// Every card whose newest run is still open, with the delegation that row names.
fn open_runs(cards: &[Card]) -> Vec<(CardId, DelegationId)> {
    cards
        .iter()
        .filter_map(|card| live_run(card).map(|run| (card.id.clone(), run)))
        .collect()
}

/// Whether this card already carries a row for that delegation.
fn records_run(cards: &[Card], card: &CardId, run: DelegationId) -> bool {
    cards
        .iter()
        .find(|other| other.id == *card)
        .is_some_and(|card| card.runs.iter().any(|row| row.id == run))
}

/// Writes the row for a live delegation the card never recorded, and returns whether it did.
fn adopt_run(doc: &mut BoardDocument, card: &CardId, delegation: &Delegation, now: &str) -> bool {
    let Some(index) = doc.cards.iter().position(|other| other.id == *card) else {
        return false;
    };
    if doc.cards[index]
        .runs
        .iter()
        .any(|row| row.id == delegation.id)
    {
        return false;
    }
    // The column the run was owed to, when the card is still parked for one; otherwise the column
    // it stands in. A run belongs to the action that asked for it, and the park remembers which.
    let status = doc.cards[index]
        .pending_run
        .as_ref()
        .map(|pending| pending.status_id.clone())
        .filter(|status| on_enter(&doc.board, status).is_some())
        .unwrap_or_else(|| doc.cards[index].status_id.clone());
    let Some(action) = on_enter(&doc.board, &status) else {
        // Neither column runs anything any more, so there is no action to attribute the run to.
        // The delegation keeps going and its delivery still records an outcome — on a card that
        // has no row for it, which `on_run_delivered` reads as a run it cannot close.
        tracing::warn!(
            %card,
            delegation = %delegation.id,
            "a live card run has no column action left to record it against",
        );
        return false;
    };
    let prefs = resolve_prefs(&doc.cards[index], action);
    let row = RunRow {
        status_id: status,
        action: action.kind.clone(),
        provider: delegation.provider,
        model: prefs.model,
        effort: prefs.effort,
    };
    // Stamped with the delegation's own start, not with `now`: the run has been going since the
    // daemon before this one started it, and the card's duration must say so.
    // The worktree the run works in, so its delivery diffs the right tree (§11.2); a card that
    // has lost it keeps the run and records no worktree, as a run from before this field did.
    let key = doc.cards[index].display_key(&doc.board);
    let worktree = run_worktree(&doc.board, &doc.cards[index], &key).ok();
    let run = in_worktree(
        started(&row, delegation, &delegation.created.to_rfc3339()),
        worktree,
    );
    push_run(&mut doc.cards[index], run, now);
    doc.cards[index].pending_run = None;
    push_activity(
        &mut doc.cards[index],
        ActivityKind::RunStarted,
        None,
        ADOPTED,
        now,
    );
    true
}

/// The cards this board still owes a run, which are the only seeds boot recovery has.
///
/// A card queued in a routing column carries its `pending_run` too, so a restart seeds it and the
/// walk moves it on as soon as a slot is free, exactly as a freed slot would have.
fn owed(cards: &[Card]) -> Vec<CardId> {
    cards
        .iter()
        .filter(|card| !card.archived)
        .filter(|card| card.pending_run.is_some() || lost_start(card))
        .map(|card| card.id.clone())
        .collect()
}

/// Whether this card's newest start was announced but never recorded.
///
/// The evaluation writes `RunStarted` under the gate and the run row lands after the delegation
/// service has answered, so a daemon killed between the two leaves a card with an entry saying a
/// run began and nothing that could ever end it. That card is owed its run; a card with no such
/// entry is merely standing in an action column and must not be started by a restart.
fn lost_start(card: &Card) -> bool {
    if card.pending_run.is_some() || latest_run(card).is_some_and(CardRun::is_live) {
        return false;
    }
    let Some(announced) = card
        .activity
        .iter()
        .rev()
        .find(|entry| entry.kind == ActivityKind::RunStarted)
        .and_then(|entry| timestamp(&entry.at))
    else {
        return false;
    };
    !card
        .runs
        .iter()
        .any(|run| timestamp(&run.started_at).is_some_and(|started| started >= announced))
}

/// How long a run had been going, for the sentence that closes it.
fn elapsed_seconds(started_at: &str, now: DateTime<Utc>) -> Option<i64> {
    timestamp(started_at).map(|started| (now - started).num_seconds().max(0))
}

/// One board timestamp as an instant, or `None` for a document written by something else.
fn timestamp(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        agents::{AgentKind, DelegationCaller, DelegationStatus, ThreadId},
        board::{ActionKind, Activity},
    };

    use super::*;

    const NOW: &str = "2026-09-21T12:00:00Z";
    const EARLIER: &str = "2026-09-21T11:00:00Z";

    fn card() -> Card {
        serde_json::from_value(serde_json::json!({
            "id": "card-1",
            "boardId": "work",
            "number": 7,
            "title": "Fix login",
            "statusId": "in-progress",
            "createdAt": EARLIER,
            "updatedAt": EARLIER,
        }))
        .expect("a card built from its required wire fields alone")
    }

    fn run(started_at: &str, outcome: Option<RunOutcome>) -> CardRun {
        CardRun {
            id: DelegationId::new(),
            thread_id: Some(ThreadId::new()),
            status_id: "in-progress".parse().expect("a static slug is valid"),
            action: ActionKind::Prompt,
            provider: AgentKind::Claude,
            model: None,
            effort: None,
            started_at: started_at.to_owned(),
            ended_at: outcome.is_some().then(|| started_at.to_owned()),
            outcome,
            detail: None,
            report_comment_id: None,
            files_changed: 0,
            cost_usd: None,
            tokens: None,
            worktree_id: None,
        }
    }

    fn announced(at: &str) -> Activity {
        Activity {
            at: at.to_owned(),
            kind: ActivityKind::RunStarted,
            actor: None,
            message: "Run started".to_owned(),
        }
    }

    fn delegation(status: DelegationStatus, delivery: DeliveryState) -> Delegation {
        Delegation {
            id: DelegationId::new(),
            caller: DelegationCaller::Card {
                board: "work".parse().expect("a static board id is valid"),
                card: "card-1".parse().expect("a static card id is valid"),
            },
            caller_turn: None,
            caller_item: None,
            child: ThreadId::new(),
            provider: AgentKind::Claude,
            depth: 1,
            brief: "brief".to_owned(),
            expectation: "expectation".to_owned(),
            eager: false,
            status,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery,
            created: timestamp(EARLIER).expect("a static timestamp parses"),
            finished: None,
            headline: None,
            usage: None,
        }
    }

    #[test]
    fn a_run_whose_delegation_is_gone_is_closed() {
        assert_eq!(disposition(None), Disposition::Close);
    }

    #[test]
    fn a_live_delegation_is_adopted_rather_than_recorded() {
        let record = delegation(DelegationStatus::Running, DeliveryState::Pending);
        assert_eq!(disposition(Some(&record)), Disposition::Adopt);
    }

    #[test]
    fn a_terminal_delegation_the_outbox_still_owes_is_left_to_the_worker() {
        let record = delegation(DelegationStatus::Succeeded, DeliveryState::Pending);
        assert_eq!(disposition(Some(&record)), Disposition::Adopt);
    }

    #[test]
    fn a_terminal_delegation_nothing_will_deliver_again_is_recorded_by_the_sweep() {
        for delivery in [
            DeliveryState::Recorded,
            DeliveryState::Consumed,
            DeliveryState::Undeliverable {
                reason: "no board service".to_owned(),
            },
        ] {
            let record = delegation(DelegationStatus::Failed, delivery);
            assert_eq!(disposition(Some(&record)), Disposition::Deliver);
        }
    }

    #[test]
    fn a_card_merely_standing_in_a_column_is_owed_nothing() {
        assert!(!lost_start(&card()));
    }

    #[test]
    fn a_start_announced_and_never_recorded_is_owed_its_run() {
        let mut card = card();
        card.runs.push(run(EARLIER, Some(RunOutcome::Succeeded)));
        card.activity.push(announced(NOW));
        assert!(lost_start(&card));
    }

    #[test]
    fn a_start_that_reached_its_row_is_owed_nothing() {
        let mut card = card();
        card.activity.push(announced(EARLIER));
        card.runs.push(run(NOW, Some(RunOutcome::Succeeded)));
        assert!(!lost_start(&card));
    }

    #[test]
    fn a_card_that_is_working_is_owed_nothing() {
        let mut card = card();
        card.activity.push(announced(NOW));
        card.runs.push(run(EARLIER, None));
        assert!(!lost_start(&card));
    }

    #[test]
    fn a_parked_card_is_owed_its_run_by_the_park_rather_than_by_an_entry() {
        let mut card = card();
        card.pending_run = Some(fleet_core::board::PendingRun {
            status_id: "in-progress".parse().expect("a static slug is valid"),
            since: EARLIER.to_owned(),
        });
        card.activity.push(announced(NOW));
        assert!(!lost_start(&card));
        assert_eq!(owed(std::slice::from_ref(&card)).len(), 1);
    }

    /// A card queued in a routing column for the action column after it (`queued`).
    fn queued_card(since: &str) -> Card {
        let mut card = card();
        card.status_id = "ready".parse().expect("a static slug is valid");
        card.pending_run = Some(fleet_core::board::PendingRun {
            status_id: "in-progress".parse().expect("a static slug is valid"),
            since: since.to_owned(),
        });
        card
    }

    #[test]
    fn a_queued_card_is_owed_its_run_after_a_restart() {
        let card = queued_card(EARLIER);
        assert!(fleet_core::board::queued(&card));
        assert!(!lost_start(&card));
        assert_eq!(owed(std::slice::from_ref(&card)), vec![card.id.clone()]);
    }

    #[test]
    fn a_queued_card_that_keeps_its_place_is_not_a_decision() {
        let plan = Plan::default();
        assert!(!decided(&plan, &[queued_card(EARLIER)], NOW));
        assert!(decided(&plan, &[queued_card(NOW)], NOW));
    }

    #[tokio::test]
    async fn restart_recovery_includes_card_worktree_boards() {
        let (_temp, services) = crate::services::boards::lifecycle::tests::fixture().await;
        let context = "work".parse().expect("a static context id is valid");
        let tasks = services
            .boards
            .ensure(&context)
            .await
            .expect("the context board is created");
        let reviews = services
            .boards
            .ensure_reviews(&context)
            .await
            .expect("the reviews board is created");
        // A context board runs nowhere until it runs in its cards' worktrees.
        assert_eq!(
            services.boards.automated_boards(),
            vec![reviews.board.id.clone()]
        );
        crate::services::boards::lifecycle::tests::run_in_card_worktrees(
            &services,
            &tasks.board.id,
        );
        let mut recovered = services.boards.automated_boards();
        recovered.sort();
        let mut expected = vec![tasks.board.id, reviews.board.id];
        expected.sort();
        assert_eq!(recovered, expected);
    }

    #[test]
    fn an_elapsed_run_is_measured_from_its_own_start() {
        let now = timestamp(NOW).expect("a static timestamp parses");
        assert_eq!(elapsed_seconds(EARLIER, now), Some(3600));
        assert_eq!(elapsed_seconds("not a timestamp", now), None);
    }

    /// A clock that ran backwards must not print a negative duration in a card's history.
    #[test]
    fn a_run_that_started_after_now_is_zero_seconds_long() {
        let now = timestamp(EARLIER).expect("a static timestamp parses");
        assert_eq!(elapsed_seconds(NOW, now), Some(0));
    }
}
