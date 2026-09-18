//! `DelegationCancel`: stop the child and let the transition half call the ending.
//!
//! Cancelling never writes a terminal status itself: it interrupts and stops the child, and the
//! `TurnAborted` that follows reaches the transition half as `Cancelled`, which is the same path a
//! Stop from the UI takes.

use std::collections::HashSet;

use fleet_core::agents::{Delegation, DelegationId, ThreadId};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::ResponseBody,
};

use super::DelegationService;

impl DelegationService {
    /// Cancels one live delegation and answers the record as it stands afterwards.
    pub(crate) async fn cancel(
        &self,
        delegation: DelegationId,
    ) -> Result<ResponseBody, ProtoError> {
        let current = self.read_delegation(delegation).await?;
        if current.status.is_terminal() {
            return Err(conflict(format!(
                "delegation {delegation} is already terminal ({})",
                current.status.word()
            )));
        }

        self.cancel_descendants(current.child).await?;
        self.cancel_one(&current).await?;

        Ok(ResponseBody::Delegation(
            self.read_delegation(delegation).await?,
        ))
    }

    /// Cancels every live delegation below `caller`, deepest children first.
    pub(super) async fn cancel_descendants(&self, caller: ThreadId) -> Result<(), ProtoError> {
        let roots = self
            .inner
            .store
            .live_delegations(Some(caller))
            .await
            .map_err(storage_error)?;
        let mut stack = roots
            .into_iter()
            .rev()
            .map(|delegation| (delegation, false))
            .collect::<Vec<_>>();
        let mut expanded = HashSet::new();

        while let Some((delegation, children_seen)) = stack.pop() {
            let current = self.read_delegation(delegation.id).await?;
            if current.status.is_terminal() {
                continue;
            }
            if children_seen {
                self.cancel_one(&current).await?;
                continue;
            }
            if !expanded.insert(current.id) {
                continue;
            }

            stack.push((current.clone(), true));
            let children = self
                .inner
                .store
                .live_delegations(Some(current.child))
                .await
                .map_err(storage_error)?;
            stack.extend(children.into_iter().rev().map(|child| (child, false)));
        }
        Ok(())
    }

    async fn cancel_one(&self, delegation: &Delegation) -> Result<(), ProtoError> {
        // Do not write `Cancelled` here. `stop` records `TurnAborted { SessionStopped }`, and the
        // transition half changes the delegation and enqueues delivery in that same transaction.
        self.inner.manager.interrupt(delegation.child).await?;
        self.inner.manager.stop(delegation.child).await?;
        Ok(())
    }
}

fn conflict(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Conflict,
        message: message.into(),
    }
}

fn storage_error(error: anyhow::Error) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Fs,
        message: format!("native-agent storage failed: {error:#}"),
    }
}
