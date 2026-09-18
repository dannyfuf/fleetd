//! `DelegationList`, `DelegationGet` and `DelegationWait`: the three read verbs.
//!
//! Reads come from SQLite, which is authoritative; bus events only wake `wait` for another read.

use std::time::Duration;

use fleet_core::agents::{Delegation, DelegationId, ThreadId};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    response::ResponseBody,
};
use tokio::sync::broadcast;

use super::DelegationService;

impl DelegationService {
    /// Every delegation, newest first, optionally narrowed to one caller.
    pub(crate) async fn list(&self, caller: Option<ThreadId>) -> Result<ResponseBody, ProtoError> {
        let delegations = self
            .inner
            .store
            .delegations(caller)
            .await
            .map_err(storage_error)?;
        Ok(ResponseBody::Delegations(delegations))
    }

    /// One delegation by id.
    pub(crate) async fn get(&self, delegation: DelegationId) -> Result<ResponseBody, ProtoError> {
        Ok(ResponseBody::Delegation(
            self.read_delegation(delegation).await?,
        ))
    }

    /// Blocks until one delegation is terminal, or until `timeout_ms` passes.
    pub(crate) async fn wait(
        &self,
        delegation: DelegationId,
        timeout_ms: u64,
    ) -> Result<ResponseBody, ProtoError> {
        // Subscribe before the first read. A transition committed between those two operations is
        // then either visible in the row or waiting in this receiver, so it cannot be missed.
        let mut events = self.inner.events.subscribe();
        let current = self.read_delegation(delegation).await?;
        if current.status.is_terminal() {
            return Ok(ResponseBody::Delegation(current));
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
        Ok(ResponseBody::Delegation(current))
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
