//! `DelegationCancel`: stop the child and let the transition half call the ending.
//!
//! Cancelling never writes a terminal status itself: it interrupts and stops the child, and the
//! `TurnAborted` that follows reaches the transition half as `Cancelled`, which is the same path a
//! Stop from the UI takes.

use fleet_core::agents::DelegationId;
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

        // Do not write `Cancelled` here. `stop` records `TurnAborted { SessionStopped }`, and the
        // transition half changes the delegation and enqueues delivery in that same transaction.
        self.inner.manager.interrupt(current.child).await?;
        self.inner.manager.stop(current.child).await?;

        Ok(ResponseBody::Delegation(
            self.read_delegation(delegation).await?,
        ))
    }
}

fn conflict(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Conflict,
        message: message.into(),
    }
}
