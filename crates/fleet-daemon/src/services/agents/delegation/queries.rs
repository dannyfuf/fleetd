//! `DelegationList`, `DelegationGet` and `DelegationWait`: the three read verbs.
//!
//! Reads come from SQLite, which is authoritative; bus events only wake `wait` for another read.
//!
//! These three, and only these three, fill [`Delegation::usage`]. It is computed, never stored, so
//! it is attached here rather than by the store's row decoder — and deliberately *not* on the
//! change path: `publish_changed` carries a repaint hint the GUI already has its own numbers for,
//! and a SQL read on every `DelegationChanged` would put one on the event path.

use std::time::Duration;

use chrono::Utc;
use fleet_core::agents::{Delegation, DelegationId, ThreadId};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    response::ResponseBody,
};
use tokio::sync::broadcast;

use super::DelegationService;
use crate::services::agents::store::delegations;

impl DelegationService {
    /// Every delegation, newest first, optionally narrowed to one caller.
    pub(crate) async fn list(&self, caller: Option<ThreadId>) -> Result<ResponseBody, ProtoError> {
        let mut delegations = self
            .inner
            .store
            .delegations(caller)
            .await
            .map_err(storage_error)?;
        self.fill_usage(&mut delegations).await?;
        Ok(ResponseBody::Delegations(delegations))
    }

    /// One delegation by id.
    pub(crate) async fn get(&self, delegation: DelegationId) -> Result<ResponseBody, ProtoError> {
        let record = self.read_delegation(delegation).await?;
        Ok(ResponseBody::Delegation(self.with_usage(record).await?))
    }

    /// Blocks until one delegation is terminal, or until `timeout_ms` passes.
    ///
    /// `caller` is the thread the waiter is acting for, when it knows its own. Answering a
    /// terminal record to that thread is the caller reading its own child's result, so the
    /// delivery is consumed here and never injected again; see [`Self::consume_for`]. Every other
    /// waiter — a third party, or one that named nobody — is answered exactly as before.
    pub(crate) async fn wait(
        &self,
        delegation: DelegationId,
        timeout_ms: u64,
        caller: Option<ThreadId>,
    ) -> Result<ResponseBody, ProtoError> {
        // Subscribe before the first read. A transition committed between those two operations is
        // then either visible in the row or waiting in this receiver, so it cannot be missed.
        let mut events = self.inner.events.subscribe();
        let current = self.read_delegation(delegation).await?;
        if current.status.is_terminal() {
            let consumed = self.consume_for(current, caller).await?;
            return Ok(ResponseBody::Delegation(self.with_usage(consumed).await?));
        }

        let terminal = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            loop {
                match events.recv().await {
                    Ok(Event::DelegationChanged(changed)) if changed.id == delegation => {
                        let current = self.read_delegation(delegation).await?;
                        if current.status.is_terminal() {
                            return Ok::<Delegation, ProtoError>(current);
                        }
                    }
                    Ok(_) => {}
                    // The event is only a repaint hint. Re-read after lag because the committed
                    // SQLite row is authoritative and may already be terminal.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let current = self.read_delegation(delegation).await?;
                        if current.status.is_terminal() {
                            return Ok(current);
                        }
                    }
                    // The service owns a sender, so this is unreachable while `self` is alive.
                    // If that invariant changes, remain under the caller's deadline and answer
                    // the final authoritative read instead of spinning on a closed receiver.
                    Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
                }
            }
        })
        .await;

        let current = match terminal {
            Ok(result) => result?,
            Err(_) => self.read_delegation(delegation).await?,
        };
        let consumed = self.consume_for(current, caller).await?;
        Ok(ResponseBody::Delegation(self.with_usage(consumed).await?))
    }

    /// Attaches what one child has spent to the record about to be answered.
    ///
    /// Called *after* [`Self::consume_for`], never before: consuming answers a record re-read from
    /// the store, which would drop a `usage` attached to the record that went in.
    async fn with_usage(&self, mut record: Delegation) -> Result<Delegation, ProtoError> {
        record.usage = self
            .inner
            .store
            .delegation_usage(record.child)
            .await
            .map_err(storage_error)?;
        Ok(record)
    }

    /// The same attachment for a whole page, in one trip to the reader pool.
    async fn fill_usage(&self, records: &mut [Delegation]) -> Result<(), ProtoError> {
        if records.is_empty() {
            return Ok(());
        }
        let children = records.iter().map(|record| record.child).collect();
        let usage = self
            .inner
            .store
            .delegation_usages(children)
            .await
            .map_err(storage_error)?;
        for record in records {
            record.usage = usage.get(&record.child).cloned();
        }
        Ok(())
    }

    /// Marks a terminal result read when the waiter is the delegation's own caller.
    ///
    /// Identity, not authorisation: a mismatch changes nothing and still answers the record. The
    /// point is the duplicate — a caller handed its child's result here would otherwise be sent
    /// the same text again as a user message once its turn settles, once per child.
    ///
    /// Best effort and idempotent, because the delivery worker is racing this. `consume` answers
    /// `None` for anything but a terminal `Pending` delivery, so a `wait` that loses that race
    /// simply reports `delivered`, which is the truth.
    async fn consume_for(
        &self,
        current: Delegation,
        caller: Option<ThreadId>,
    ) -> Result<Delegation, ProtoError> {
        if !current.status.is_terminal() || caller != Some(current.caller) {
            return Ok(current);
        }
        let id = current.id;
        let consumed = self
            .inner
            .store
            .delegation_write(
                "consume a delegation result read by its caller",
                move |tx| {
                    // No wake: this closes the delivery row rather than opening new work.
                    Ok((delegations::consume(tx, id, Utc::now())?, false))
                },
            )
            .await
            .map_err(storage_error)?;
        let Some(consumed) = consumed else {
            return Ok(current);
        };
        tracing::info!(
            target: "fleet::agents",
            delegation = %consumed.id,
            caller = %consumed.caller,
            "consumed a delegation result its caller read through wait",
        );
        self.publish_changed(consumed.clone());
        Ok(consumed)
    }

    pub(super) async fn read_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Delegation, ProtoError> {
        self.inner
            .store
            .delegation(delegation)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| not_found(format!("delegation {delegation} does not exist")))
    }
}

fn storage_error(error: anyhow::Error) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Fs,
        message: one_line(&format!("native-agent storage failed: {error:#}")),
    }
}

fn not_found(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::NotFound,
        message: one_line(&message.into()),
    }
}

fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}
