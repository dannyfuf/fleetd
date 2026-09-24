//! The outbox drain: every follow-up action a committed transition queued.
//!
//! [`DelegationWorker`](super::DelegationWorker) calls this once at start, on every wake and once
//! per retry tick. Rows stay open until their side effect has either happened or has been made
//! durably unnecessary.

use std::collections::HashSet;

use crate::services::agents::store::{OutboxAction, OutboxRow, delegations};
use anyhow::Context as _;
use chrono::Utc;
use fleet_core::{
    agents::{
        Delegation, DelegationCaller, DelegationStatus, DeliveryState, ItemPatch, ItemPayloadPatch,
        ItemStatus, MessageOrigin, ResultSource, SessionState, StopCause, ThreadId, UserInput,
    },
    ids::BoardId,
};
use fleet_proto::error::ErrorKind;

use super::{
    DelegationService,
    footer::{NUDGE, RESUME_NUDGE, delivered_message},
    limits::SETTLE_GRACE,
};
use crate::services::agents::manager::SubmissionState;

/// What the drain throttles on: at most one performed row per key per pass.
///
/// A thread caller is keyed on itself, so two child results cannot land in one transcript in a
/// single pass. A card caller is keyed on its **board**, not its card: the board document is the
/// one thing two cards' deliveries contend for, so two cards of one board record one after the
/// other while two boards record together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ThrottleKey {
    Thread(ThreadId),
    Board(BoardId),
}

impl From<&DelegationCaller> for ThrottleKey {
    fn from(caller: &DelegationCaller) -> Self {
        match caller {
            DelegationCaller::Thread(thread) => Self::Thread(*thread),
            DelegationCaller::Card { board, .. } => Self::Board(board.clone()),
        }
    }
}

/// Performs one oldest open row per caller.
///
/// A caller can therefore receive at most one child result during this pass. If that send starts
/// a turn, the next child's row waits for the caller-item commit and the later settle wake rather
/// than being folded into the same turn.
pub(super) async fn drain(service: &DelegationService) -> anyhow::Result<()> {
    // The manager repairs restart orphans in the background. Hydrating live delegation children
    // here makes the worker's startup order deterministic: every ProviderExited transition and
    // its Recover row exist before this pass reads the outbox.
    for delegation in service.inner.store.live_delegations(None).await? {
        // A start between reserving this row and creating its child: the child is missing
        // because it is being made, not because a restart lost it.
        if service.is_creating(delegation.id) {
            continue;
        }
        match service.inner.manager.projection(delegation.child).await {
            Ok(_) => {}
            Err(error) if error.kind == ErrorKind::NotFound => {
                let id = delegation.id;
                service
                    .inner
                    .store
                    .delegation_write("release missing-child delegation reservation", move |tx| {
                        delegations::delete(tx, id)?;
                        Ok(((), false))
                    })
                    .await?;
                tracing::warn!(
                    target: "fleet::agents",
                    delegation = %delegation.id,
                    child = %delegation.child,
                    "released a delegation reservation whose child was never created",
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
    repair_missing_callers(service).await?;
    let mut rows = service.inner.store.delegation_outbox().await?;
    let open = rows.iter().map(|row| row.id).collect::<HashSet<_>>();
    service
        .inner
        .in_flight_rows
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|row| open.contains(row));
    rows.sort_by_key(|row| (row.action != OutboxAction::CancelChildren, row.id));
    let mut callers: HashSet<ThrottleKey> = HashSet::new();

    for row in rows {
        let Some(delegation) = service.inner.store.delegation(row.delegation).await? else {
            // A foreign-key violation should make this impossible. Closing the poison row is
            // still safer than letting it prevent useful rows from being retried forever.
            finish_row(service, row.id).await?;
            tracing::warn!(
                target: "fleet::agents",
                delegation = %row.delegation,
                action = row.action.as_str(),
                "discarded an outbox row whose delegation no longer exists",
            );
            continue;
        };
        // Cancellation is not a delivery: it writes to the child, never to the caller, so it is
        // never held back by another row of the same caller.
        if row.action != OutboxAction::CancelChildren
            && !callers.insert(ThrottleKey::from(&delegation.caller))
        {
            continue;
        }
        let caller = delegation.caller.clone();
        let child = delegation.child;

        tracing::info!(
            target: "fleet::agents",
            delegation = %delegation.id,
            action = row.action.as_str(),
            caller = %delegation.caller,
            child = %delegation.child,
            "performing delegation outbox action",
        );
        if let Err(error) = perform(service, &row, delegation).await {
            tracing::warn!(
                target: "fleet::agents",
                delegation = %row.delegation,
                action = row.action.as_str(),
                caller = %caller,
                child = %child,
                error = %format!("{error:#}"),
                "delegation outbox action remains open",
            );
        }
    }
    Ok(())
}

async fn perform(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    match row.action {
        OutboxAction::Mirror => mirror(service, row, delegation).await,
        OutboxAction::Nudge => nudge(service, row, delegation).await,
        OutboxAction::Settle => settle(service, row, delegation).await,
        OutboxAction::Deliver => deliver(service, row, delegation).await,
        OutboxAction::Recover => recover(service, row, delegation).await,
        OutboxAction::CancelChildren => cancel_children(service, row, delegation).await,
    }
}

async fn repair_missing_callers(service: &DelegationService) -> anyhow::Result<()> {
    let now = Utc::now();
    let repaired = service
        .inner
        .store
        .delegation_write("repair missing delegation callers", move |tx| {
            let changed = delegations::mark_missing_callers_undeliverable(tx, now)?;
            Ok((changed, false))
        })
        .await?;
    for delegation in repaired {
        tracing::warn!(
            target: "fleet::agents",
            delegation = %delegation.id,
            caller = %delegation.caller,
            "marked a terminal delegation with a deleted caller undeliverable",
        );
        service.publish_changed(delegation);
    }
    Ok(())
}

async fn mirror(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    // Mirroring writes the child's status onto the caller's transcript item. A card caller has
    // neither a thread nor an item, and the board learns the same fact from `DelegationChanged`,
    // so the row is closed and nothing is written.
    let (Some(caller), Some(caller_item)) =
        (delegation.caller.thread().copied(), delegation.caller_item)
    else {
        finish_row(service, row.id).await?;
        service.publish_changed(delegation);
        return Ok(());
    };
    service
        .inner
        .manager
        .patch_item(
            caller,
            caller_item,
            status_patch(delegation.status),
            terminal_item_status(delegation.status),
        )
        .await?;
    finish_row(service, row.id).await?;
    service.publish_changed(delegation);
    Ok(())
}

async fn nudge(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    let delegation_id = delegation.id;
    let row_id = row.id;
    let current = service
        .inner
        .store
        .delegation(delegation_id)
        .await?
        .context("nudge delegation disappeared")?;
    let reported = current
        .result
        .as_ref()
        .is_some_and(|result| result.source == ResultSource::Reported);
    if current.status != DelegationStatus::Settling || reported {
        finish_row(service, row_id).await?;
        return Ok(());
    }
    let item = delegations::outbox_item(delegation.id, OutboxAction::Nudge, row.id);
    match durable_submission_state(service, row, delegation.child, item).await? {
        SubmissionState::Committed => {
            return acknowledge_nudge(service, delegation_id, row_id).await;
        }
        SubmissionState::Pending => return Ok(()),
        SubmissionState::Unknown => {}
    }
    let Some(_claim) = InFlightClaim::acquire(service, row_id) else {
        return Ok(());
    };
    mark_submission(service, row_id).await?;

    service
        .inner
        .manager
        .send_durable(
            delegation.child,
            UserInput {
                text: NUDGE.to_owned(),
                item: Some(item),
                ..UserInput::default()
            },
        )
        .await?;
    acknowledge_nudge(service, delegation_id, row_id).await
}

async fn acknowledge_nudge(
    service: &DelegationService,
    delegation_id: fleet_core::agents::DelegationId,
    row_id: i64,
) -> anyhow::Result<()> {
    let changed = service
        .inner
        .store
        .delegation_write("acknowledge delegation nudge", move |tx| {
            let Some(mut current) = delegations::get(tx, delegation_id)? else {
                anyhow::bail!("delegation {delegation_id} does not exist");
            };
            current.nudges = current.nudges.saturating_add(1);
            delegations::update(tx, &current)?;
            delegations::mark_done(tx, row_id, Utc::now())?;
            Ok((current, false))
        })
        .await?;
    service.publish_changed(changed);
    Ok(())
}

async fn settle(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    let projection = service.inner.manager.projection(delegation.child).await?;
    let now = Utc::now();
    let grace_elapsed = now.signed_duration_since(delegation.created)
        >= chrono::Duration::from_std(SETTLE_GRACE).context("convert settle grace")?;
    if !projection.background_tasks.is_empty() && !grace_elapsed {
        return Ok(());
    }

    let delegation_id = delegation.id;
    let row_id = row.id;
    let Some(delegation) = service
        .inner
        .store
        .delegation_write("settle delegation", move |tx| {
            let Some(mut current) = delegations::get(tx, delegation_id)? else {
                anyhow::bail!("delegation {delegation_id} does not exist");
            };
            if current.status != DelegationStatus::Settling {
                delegations::mark_done(tx, row_id, now)?;
                return Ok((None, false));
            }
            current.status = DelegationStatus::Succeeded;
            current.finished = Some(now);
            delegations::update(tx, &current)?;
            delegations::enqueue(tx, current.id, OutboxAction::Deliver, now)?;
            delegations::mark_done(tx, row_id, now)?;
            Ok((Some(current), true))
        })
        .await?
    else {
        return Ok(());
    };
    service.publish_changed(delegation);
    Ok(())
}

async fn recover(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    let item = delegations::outbox_item(delegation.id, OutboxAction::Recover, row.id);
    match durable_submission_state(service, row, delegation.child, item).await? {
        SubmissionState::Committed => {
            finish_row(service, row.id).await?;
            let current = service
                .inner
                .store
                .delegation(delegation.id)
                .await?
                .unwrap_or(delegation);
            service.publish_changed(current);
            return Ok(());
        }
        SubmissionState::Pending => return Ok(()),
        SubmissionState::Unknown => {}
    }
    let Some(_claim) = InFlightClaim::acquire(service, row.id) else {
        return Ok(());
    };
    mark_submission(service, row.id).await?;
    let input = UserInput {
        text: RESUME_NUDGE.to_owned(),
        item: Some(item),
        ..UserInput::default()
    };
    match service
        .inner
        .manager
        .send_durable(delegation.child, input)
        .await
    {
        Ok(_) => {
            finish_row(service, row.id).await?;
            let current = service
                .inner
                .store
                .delegation(delegation.id)
                .await?
                .unwrap_or(delegation);
            service.publish_changed(current);
            Ok(())
        }
        Err(error) if error.kind == ErrorKind::Conflict => {
            let record = service.inner.manager.record(delegation.child).await?;
            if record.resume_cursor.is_some() {
                return Err(anyhow::Error::new(error));
            }
            fail_recovery(
                service,
                row.id,
                delegation.id,
                "agent thread has no resume cursor".to_owned(),
            )
            .await
        }
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

async fn fail_recovery(
    service: &DelegationService,
    row: i64,
    delegation: fleet_core::agents::DelegationId,
    reason: String,
) -> anyhow::Result<()> {
    let now = Utc::now();
    let changed = service
        .inner
        .store
        .delegation_write("fail delegation recovery", move |tx| {
            let Some(mut current) = delegations::get(tx, delegation)? else {
                anyhow::bail!("delegation {delegation} does not exist");
            };
            if !current.status.is_terminal() {
                current.status = DelegationStatus::Failed;
                current.status_payload = Some(reason);
                current.finished = Some(now);
                delegations::update(tx, &current)?;
                delegations::enqueue(tx, current.id, OutboxAction::Deliver, now)?;
            }
            delegations::mark_done(tx, row, now)?;
            Ok((current, true))
        })
        .await?;
    service.publish_changed(changed);
    Ok(())
}

async fn cancel_children(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    service.cancel_descendants(delegation.child).await?;
    finish_row(service, row.id).await?;
    service.publish_changed(delegation);
    Ok(())
}

async fn deliver(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    if row_in_flight(service, row.id) {
        return Ok(());
    }
    if delegation.caller.is_card() {
        return deliver_to_card(service, row, delegation).await;
    }
    // A thread caller stores its transcript item when the delegation is created. A row without
    // one has nothing to patch and nothing to inject into, so closing it is all that is left.
    let (Some(caller), Some(caller_item)) =
        (delegation.caller.thread().copied(), delegation.caller_item)
    else {
        return finish_row(service, row.id).await;
    };
    if let Err(error) = service
        .inner
        .manager
        .patch_item(
            caller,
            caller_item,
            status_patch(delegation.status),
            terminal_item_status(delegation.status),
        )
        .await
    {
        if error.kind == ErrorKind::NotFound {
            return make_undeliverable(
                service,
                row.id,
                delegation,
                "caller thread no longer exists".to_owned(),
            )
            .await;
        }
        return Err(error.into());
    }

    // The caller already read this result through its own `wait`, so injecting it now is exactly
    // the duplicate user message that state exists to stop. The item patch above still had to run
    // — it is what stops the caller's transcript row saying "working" — but nothing is sent.
    // `consume` closes this row in its own transaction, so seeing it open here means this pass
    // read the outbox before that commit landed; close it and move on.
    if matches!(delegation.delivery, DeliveryState::Consumed) {
        tracing::info!(
            target: "fleet::agents",
            delegation = %delegation.id,
            caller = %caller,
            "skipped delivery injection for a result the caller already read",
        );
        return finish_row(service, row.id).await;
    }

    let record = match service.inner.manager.record(caller).await {
        Ok(record) => record,
        Err(error) if error.kind == ErrorKind::NotFound => {
            return make_undeliverable(
                service,
                row.id,
                delegation,
                "caller thread no longer exists".to_owned(),
            )
            .await;
        }
        Err(error) => return Err(error.into()),
    };
    let projection = service.inner.manager.projection(caller).await?;

    // An open caller gate wins over every session state. Sending while it is open could answer a
    // question or permission prompt instead of starting the result turn.
    if !projection.gates.is_empty() {
        return Ok(());
    }

    let should_send = if matches!(projection.turn, fleet_core::agents::TurnState::Running(_)) {
        delegation.eager
    } else {
        match projection.session {
            SessionState::Ready => true,
            SessionState::Running => false,
            SessionState::Stopped | SessionState::Error
                if record.stop_cause == Some(StopCause::User) =>
            {
                false
            }
            SessionState::Stopped | SessionState::Error if record.resume_cursor.is_none() => {
                return make_undeliverable(
                    service,
                    row.id,
                    delegation,
                    "caller thread has no resume cursor".to_owned(),
                )
                .await;
            }
            SessionState::Stopped | SessionState::Error
                if record.stop_cause == Some(StopCause::ProviderExit) =>
            {
                true
            }
            SessionState::Starting
            | SessionState::Waiting(_)
            | SessionState::Stopped
            | SessionState::Error => false,
        }
    };
    if !should_send {
        return Ok(());
    }

    let item = delegations::outbox_item(delegation.id, OutboxAction::Deliver, row.id);
    let input = UserInput {
        text: delivered_message(&delegation, Utc::now()),
        origin: MessageOrigin::Delegation { id: delegation.id },
        item: Some(item),
        ..UserInput::default()
    };
    match durable_submission_state(service, row, caller, item).await? {
        SubmissionState::Committed => {
            finish_row(service, row.id).await?;
            return Ok(());
        }
        SubmissionState::Pending => return Ok(()),
        SubmissionState::Unknown => {}
    }
    let Some(_claim) = InFlightClaim::acquire(service, row.id) else {
        return Ok(());
    };
    mark_submission(service, row.id).await?;
    match service.inner.manager.send_durable(caller, input).await {
        Ok(_) => Ok(()),
        Err(error) if error.kind == ErrorKind::Conflict => Ok(()),
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

/// Records a terminal card run on its board, and closes the row only once that write committed.
///
/// None of the thread path applies here: a card has no transcript item to patch, no gate that
/// could answer the wrong prompt, no session to resume and no user message to inject. The board
/// write *is* the delivery, so the hook's `Ok` is the only thing that closes the row — an `Err`
/// leaves it open and the next drain asks again, which is why the hook must be idempotent by
/// delegation id.
async fn deliver_to_card(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    let Some((board, card)) = delegation
        .caller
        .card()
        .map(|(board, card)| (board.clone(), card.clone()))
    else {
        // Unreachable: `deliver` routes only a card caller here. Closing the row is still safer
        // than leaving one open that no later pass could ever perform.
        return finish_row(service, row.id).await;
    };
    let Some(hook) = service.run_delivery_hook() else {
        // Either composition installed no boards service, or it has been dropped. Neither can
        // change while this daemon runs, so the delivery is durably failed rather than retried
        // for the life of the process.
        return make_undeliverable(service, row.id, delegation, "no board service".to_owned())
            .await;
    };
    hook.on_run_delivered(&board, &card, &delegation)
        .await
        .with_context(|| format!("record card run {} on card {board}/{card}", delegation.id))?;

    let delegation_id = delegation.id;
    let recorded = service
        .inner
        .store
        .delegation_write("record a card run's delivery", move |tx| {
            let Some(mut current) = delegations::get(tx, delegation_id)? else {
                anyhow::bail!("delegation {delegation_id} does not exist");
            };
            let now = Utc::now();
            current.delivery = DeliveryState::Recorded;
            delegations::update(tx, &current)?;
            // No wake: this closes the delivery work rather than opening new work.
            delegations::mark_done_for(tx, delegation_id, OutboxAction::Deliver, now)?;
            Ok((current, false))
        })
        .await?;
    tracing::info!(
        target: "fleet::agents",
        delegation = %recorded.id,
        caller = %recorded.caller,
        child = %recorded.child,
        "recorded a card run's outcome on its board",
    );
    service.publish_changed(recorded);
    Ok(())
}

fn row_in_flight(service: &DelegationService, row: i64) -> bool {
    service
        .inner
        .in_flight_rows
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&row)
}

fn clear_in_flight(service: &DelegationService, row: i64) {
    service
        .inner
        .in_flight_rows
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&row);
}

async fn mark_submission(service: &DelegationService, row: i64) -> anyhow::Result<()> {
    service
        .inner
        .store
        .delegation_write("mark delegation submission durable", move |tx| {
            delegations::mark_submitted(tx, row, Utc::now())?;
            Ok(((), false))
        })
        .await
}

/// Resolves the stable transcript identity before an outbox row is allowed to submit again.
///
/// The in-memory queue covers an ordinary failed commit. After a daemon restart that queue is
/// gone, so a durable pre-send marker first resumes the provider and drains the history returned
/// by that open. Only a history miss may retry, under the same stable provider item identity.
async fn durable_submission_state(
    service: &DelegationService,
    row: &OutboxRow,
    thread: fleet_core::agents::ThreadId,
    item: fleet_core::agents::ItemId,
) -> anyhow::Result<SubmissionState> {
    let mut state = service.inner.manager.submission_state(thread, item).await?;
    if state == SubmissionState::Pending {
        state = service
            .inner
            .manager
            .reconcile_submission(thread, item)
            .await?;
    }
    if state == SubmissionState::Unknown && row.submitted.is_some() {
        state = service
            .inner
            .manager
            .reconcile_provider_history(thread, item)
            .await?;
        if state == SubmissionState::Pending {
            state = service
                .inner
                .manager
                .reconcile_submission(thread, item)
                .await?;
        }
    }
    Ok(state)
}

/// Process-local overlap suppression that can never outlive the future which acquired it.
struct InFlightClaim<'a> {
    service: &'a DelegationService,
    row: i64,
}

impl<'a> InFlightClaim<'a> {
    fn acquire(service: &'a DelegationService, row: i64) -> Option<Self> {
        service
            .inner
            .in_flight_rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(row)
            .then_some(Self { service, row })
    }
}

impl Drop for InFlightClaim<'_> {
    fn drop(&mut self) {
        clear_in_flight(self.service, self.row);
    }
}

fn status_patch(status: DelegationStatus) -> ItemPatch {
    ItemPatch {
        payload: Some(ItemPayloadPatch::Delegation { status }),
        ..ItemPatch::default()
    }
}

const fn terminal_item_status(status: DelegationStatus) -> Option<ItemStatus> {
    if !status.is_terminal() {
        None
    } else if matches!(status, DelegationStatus::Succeeded) {
        Some(ItemStatus::Completed)
    } else {
        Some(ItemStatus::Failed)
    }
}

async fn finish_row(service: &DelegationService, row: i64) -> anyhow::Result<()> {
    let now = Utc::now();
    service
        .inner
        .store
        .delegation_write("finish delegation outbox row", move |tx| {
            delegations::mark_done(tx, row, now)?;
            Ok(((), false))
        })
        .await
}

async fn make_undeliverable(
    service: &DelegationService,
    row: i64,
    mut delegation: Delegation,
    reason: String,
) -> anyhow::Result<()> {
    let delegation_id = delegation.id;
    delegation = service
        .inner
        .store
        .delegation_write("make delegation undeliverable", move |tx| {
            let Some(mut current) = delegations::get(tx, delegation_id)? else {
                anyhow::bail!("delegation {delegation_id} does not exist");
            };
            current.delivery = DeliveryState::Undeliverable { reason };
            delegations::update(tx, &current)?;
            delegations::mark_done(tx, row, Utc::now())?;
            Ok((current, false))
        })
        .await?;
    service.publish_changed(delegation);
    Ok(())
}
