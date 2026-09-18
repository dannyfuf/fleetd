//! The outbox drain: every follow-up action a committed transition queued.
//!
//! [`DelegationWorker`](super::DelegationWorker) calls this once at start, on every wake and once
//! per retry tick. Rows stay open until their side effect has either happened or has been made
//! durably unnecessary.

use std::collections::HashSet;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use fleet_core::agents::{
    Delegation, DelegationStatus, DeliveryState, ItemPatch, ItemPayloadPatch, ItemStatus,
    MessageOrigin, SessionState, StopCause, UserInput,
};
use fleet_proto::error::ErrorKind;
use rusqlite::params;

use crate::services::agents::store::{OutboxAction, OutboxRow};

use super::{
    DelegationService,
    footer::{NUDGE, delivered_message},
    limits::SETTLE_GRACE,
};

/// Performs one oldest open row per caller.
///
/// A caller can therefore receive at most one child result during this pass. If that send starts
/// a turn, the next child's row waits for the caller-item commit and the later settle wake rather
/// than being folded into the same turn.
pub(super) async fn drain(service: &DelegationService) -> anyhow::Result<()> {
    let rows = service.inner.store.delegation_outbox().await?;
    let mut callers = HashSet::new();

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
        if !callers.insert(delegation.caller) {
            continue;
        }
        let caller = delegation.caller;
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
        OutboxAction::Recover | OutboxAction::CancelChildren => {
            // Phase 6 replaces these placeholders with recovery and descendant cancellation.
            finish_row(service, row.id).await?;
            service.publish_changed(delegation);
            Ok(())
        }
    }
}

async fn mirror(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    service
        .inner
        .manager
        .patch_item(
            delegation.caller,
            delegation.caller_item,
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
    mut delegation: Delegation,
) -> anyhow::Result<()> {
    // Count and close the durable request before the send. If the daemon exits after the commit,
    // the child may miss one hint, but it can never receive an unbounded series of duplicate
    // nudges after restarts.
    bump_nudge_and_finish(service, row.id, delegation.id).await?;
    delegation.nudges = delegation.nudges.saturating_add(1);
    service.publish_changed(delegation.clone());

    service
        .inner
        .manager
        .send(
            delegation.child,
            UserInput {
                text: NUDGE.to_owned(),
                ..UserInput::default()
            },
        )
        .await?;
    Ok(())
}

async fn settle(
    service: &DelegationService,
    row: &OutboxRow,
    mut delegation: Delegation,
) -> anyhow::Result<()> {
    let projection = service.inner.manager.projection(delegation.child).await?;
    let now = Utc::now();
    let grace_elapsed = now.signed_duration_since(row.created)
        >= chrono::Duration::from_std(SETTLE_GRACE).context("convert settle grace")?;
    if !projection.background_tasks.is_empty() && !grace_elapsed {
        return Ok(());
    }

    settle_and_enqueue_delivery(service, row.id, delegation.id, now).await?;
    delegation.status = DelegationStatus::Succeeded;
    delegation.finished = Some(now);
    service.publish_changed(delegation);
    Ok(())
}

async fn deliver(
    service: &DelegationService,
    row: &OutboxRow,
    delegation: Delegation,
) -> anyhow::Result<()> {
    if let Err(error) = service
        .inner
        .manager
        .patch_item(
            delegation.caller,
            delegation.caller_item,
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

    let record = match service.inner.manager.record(delegation.caller).await {
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
    let projection = service.inner.manager.projection(delegation.caller).await?;

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

    let input = UserInput {
        text: delivered_message(&delegation, Utc::now()),
        origin: MessageOrigin::Delegation { id: delegation.id },
        ..UserInput::default()
    };
    match service.inner.manager.send(delegation.caller, input).await {
        Ok(_) => Ok(()),
        Err(error) if error.kind == ErrorKind::Conflict => Ok(()),
        Err(error) => Err(anyhow::Error::new(error)),
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
    let now = timestamp(Utc::now());
    service
        .inner
        .store
        .delegation_write("finish delegation outbox row", move |tx| {
            tx.execute(
                "UPDATE delegation_outbox SET done = ?2 WHERE id = ?1 AND done IS NULL",
                params![row, now],
            )
            .with_context(|| format!("mark delegation outbox row {row} done"))?;
            Ok(((), false))
        })
        .await
}

async fn bump_nudge_and_finish(
    service: &DelegationService,
    row: i64,
    delegation: fleet_core::agents::DelegationId,
) -> anyhow::Result<()> {
    let now = timestamp(Utc::now());
    service
        .inner
        .store
        .delegation_write("count delegation nudge", move |tx| {
            let changed = tx
                .execute(
                    "UPDATE delegations SET nudges = nudges + 1 WHERE id = ?1",
                    [delegation.to_string()],
                )
                .with_context(|| format!("count nudge for delegation {delegation}"))?;
            anyhow::ensure!(changed == 1, "delegation {delegation} does not exist");
            tx.execute(
                "UPDATE delegation_outbox SET done = ?2 WHERE id = ?1 AND done IS NULL",
                params![row, now],
            )
            .with_context(|| format!("mark delegation outbox row {row} done"))?;
            Ok(((), false))
        })
        .await
}

async fn settle_and_enqueue_delivery(
    service: &DelegationService,
    row: i64,
    delegation: fleet_core::agents::DelegationId,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let stamped = timestamp(now);
    service
        .inner
        .store
        .delegation_write("settle delegation", move |tx| {
            let changed = tx
                .execute(
                    "UPDATE delegations SET status = 'succeeded', finished = ?2 WHERE id = ?1",
                    params![delegation.to_string(), stamped],
                )
                .with_context(|| format!("settle delegation {delegation}"))?;
            anyhow::ensure!(changed == 1, "delegation {delegation} does not exist");
            tx.execute(
                "INSERT INTO delegation_outbox (delegation, action, created) VALUES (?1, 'deliver', ?2)",
                params![delegation.to_string(), stamped],
            )
            .with_context(|| format!("enqueue delivery for delegation {delegation}"))?;
            tx.execute(
                "UPDATE delegation_outbox SET done = ?2 WHERE id = ?1 AND done IS NULL",
                params![row, stamped],
            )
            .with_context(|| format!("mark delegation outbox row {row} done"))?;
            Ok(((), true))
        })
        .await
}

async fn make_undeliverable(
    service: &DelegationService,
    row: i64,
    mut delegation: Delegation,
    reason: String,
) -> anyhow::Result<()> {
    let id = delegation.id;
    let stored_reason = reason.clone();
    let now = timestamp(Utc::now());
    service
        .inner
        .store
        .delegation_write("make delegation undeliverable", move |tx| {
            let changed = tx
                .execute(
                    "UPDATE delegations SET delivery = 'undeliverable', delivery_reason = ?2, \
                     delivered_seq = NULL, delivered_turn = NULL WHERE id = ?1",
                    params![id.to_string(), stored_reason],
                )
                .with_context(|| format!("make delegation {id} undeliverable"))?;
            anyhow::ensure!(changed == 1, "delegation {id} does not exist");
            tx.execute(
                "UPDATE delegation_outbox SET done = ?2 WHERE id = ?1 AND done IS NULL",
                params![row, now],
            )
            .with_context(|| format!("mark delegation outbox row {row} done"))?;
            Ok(((), false))
        })
        .await?;
    delegation.delivery = DeliveryState::Undeliverable { reason };
    service.publish_changed(delegation);
    Ok(())
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

// Keep this owned suite reachable even while the separately owned service-test stage is still
// wiring the delegation test tree into `delegation/mod.rs`.
#[cfg(test)]
#[path = "tests/worker.rs"]
mod tests;
