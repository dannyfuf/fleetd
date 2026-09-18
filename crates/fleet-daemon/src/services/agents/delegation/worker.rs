//! The outbox drain: every follow-up action a committed transition queued.
//!
//! [`DelegationWorker`](super::DelegationWorker) calls this once at start, on every wake and once
//! per retry tick. Rows stay open until their side effect has either happened or has been made
//! durably unnecessary.

use std::collections::HashSet;

use crate::services::agents::store::{OutboxAction, OutboxRow, delegations};
use anyhow::Context as _;
use chrono::Utc;
use fleet_core::agents::{
    Delegation, DelegationStatus, DeliveryState, ItemPatch, ItemPayloadPatch, ItemStatus,
    MessageOrigin, ResultSource, SessionState, StopCause, UserInput,
};
use fleet_proto::error::ErrorKind;

use super::{
    DelegationService,
    footer::{NUDGE, RESUME_NUDGE, delivered_message},
    limits::SETTLE_GRACE,
};

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
        service.inner.manager.projection(delegation.child).await?;
    }
    repair_missing_callers(service).await?;
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
    delegation: Delegation,
) -> anyhow::Result<()> {
    // Count and close the durable request before the send. If the daemon exits after the commit,
    // the child may miss one hint, but it can never receive an unbounded series of duplicate
    // nudges after restarts.
    let delegation_id = delegation.id;
    let row_id = row.id;
    let Some(delegation) = service
        .inner
        .store
        .delegation_write("count delegation nudge", move |tx| {
            let Some(mut current) = delegations::get(tx, delegation_id)? else {
                anyhow::bail!("delegation {delegation_id} does not exist");
            };
            let reported = current
                .result
                .as_ref()
                .is_some_and(|result| result.source == ResultSource::Reported);
            if current.status != DelegationStatus::Settling || reported {
                delegations::mark_done(tx, row_id, Utc::now())?;
                return Ok((None, false));
            }
            current.nudges = current.nudges.saturating_add(1);
            delegations::update(tx, &current)?;
            delegations::mark_done(tx, row_id, Utc::now())?;
            Ok((Some(current), false))
        })
        .await?
    else {
        return Ok(());
    };
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
    delegation: Delegation,
) -> anyhow::Result<()> {
    let projection = service.inner.manager.projection(delegation.child).await?;
    let now = Utc::now();
    let grace_elapsed = now.signed_duration_since(row.created)
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
    let input = UserInput {
        text: RESUME_NUDGE.to_owned(),
        ..UserInput::default()
    };
    match service.inner.manager.send(delegation.child, input).await {
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
