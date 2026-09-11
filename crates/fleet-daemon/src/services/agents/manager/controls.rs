//! Changing a runtime control: what the adapter can take now, and what needs a new process.
//!
//! Two things live here because they are two halves of one verb. [`AgentSessionManager::apply_control`]
//! makes the change true — which on Claude means replacing the process, since its model, effort
//! and mode are launch flags — and [`AgentSessionManager::update_settings`] records it as a
//! durable event, so a client mirror learns the new model from the same stream as everything else
//! rather than keeping the old one until its tab is re-opened.
//!
//! The restart decision is **here and not in the adapter**: `apply_runtime` answers what a change
//! would cost, and only the manager knows whether a turn is running (§3.1). §7 is explicit that
//! nothing applies mid-turn.

use fleet_core::agents::{AgentEvent, ModelSelection, PermissionMode, SessionState};
use fleet_proto::error::ProtoError;

use crate::agents::harness::RuntimeChange;

use super::{
    AgentSessionManager, Serialized, ThreadRuntime, conflict, provider_error, runtime_inflight,
};

impl AgentSessionManager {
    /// Records a mode or model change as a durable event rather than a silent projection edit.
    ///
    /// §3 makes the reducer the only writer of projected state and §6 makes the log the
    /// transcript, so a client mirror learns the new model from the same stream as everything
    /// else instead of keeping the old one until its tab is re-opened.
    pub(super) async fn update_settings(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
        mode: Option<PermissionMode>,
        model: Option<ModelSelection>,
    ) -> Result<(), ProtoError> {
        self.apply_one(
            runtime,
            operation,
            AgentEvent::MetadataChanged {
                title: None,
                mode,
                model,
            },
            None,
        )
        .await
    }

    /// Applies one runtime control, restarting the harness only at a turn boundary.
    ///
    /// The adapter answers **what the change costs** and this is the only thing that knows
    /// whether a turn is running, which is why [`AgentProvider::apply_runtime`] reports a
    /// [`RestartPlan`](crate::agents::harness::RestartPlan) instead of performing one (§3.1). §7 is explicit that nothing applies
    /// mid-turn, so a change whose cost is a restart is refused while a turn runs rather than
    /// killing it: the app holds the draft and re-sends it at the next boundary.
    ///
    /// `session = Starting` is published before the restart begins, so the tab reads `starting…`
    /// in the same frame rather than after the round trip (§7.1).
    pub(super) async fn apply_control(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
        change: RuntimeChange,
    ) -> Result<(), ProtoError> {
        let thread = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record
            .thread;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        let applied = provider
            .apply_runtime(change.clone())
            .await
            .map_err(provider_error)?;
        let Some(plan) = applied.restart.clone() else {
            return Ok(());
        };
        if let Some(turn) = runtime_inflight(runtime) {
            let fields = plan
                .fields
                .iter()
                .map(|field| format!("{field:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(conflict(format!(
                "{} applies {fields} by restarting, which only happens at a turn boundary;                  turn {turn} is still running",
                provider.kind().display_name()
            )));
        }
        drop(provider_slot);
        self.apply_one(
            runtime,
            operation,
            AgentEvent::SessionStateChanged(SessionState::Starting),
            Some("control_restart".to_owned()),
        )
        .await?;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        let restarted = provider.restart(&plan, &change).await;
        drop(provider_slot);
        match restarted {
            Ok(()) => {
                self.apply_one(
                    runtime,
                    operation,
                    AgentEvent::SessionStateChanged(SessionState::Ready),
                    Some("control_restart".to_owned()),
                )
                .await
            }
            Err(error) => {
                // The optimistic `Starting` above is now a lie, and the projection is the only
                // thing the tab reads: the failure is recorded before it is returned.
                self.apply_one(
                    runtime,
                    operation,
                    AgentEvent::SessionStateChanged(SessionState::Error),
                    Some("control_restart".to_owned()),
                )
                .await?;
                Err(provider_error(error))
            }
        }
    }
}
