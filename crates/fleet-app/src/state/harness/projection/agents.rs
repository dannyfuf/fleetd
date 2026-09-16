//! Native-agent fields in the frozen harness snapshot.

use fleet_core::agents::{GateKind, ItemKind};

use super::AppState;
use crate::state::harness::AgentDecisionSnapshot;

/// The decision currently docked above the active agent composer.
pub(super) fn decision_snapshot(state: &AppState) -> Option<AgentDecisionSnapshot> {
    let thread = state.active_agent_thread()?;
    let projection = state.agents.projection(thread)?;
    let gate = projection.gates.last()?;
    let (kind, item) = match &gate.kind {
        GateKind::Permission { item, .. } => ("approval", *item),
        GateKind::Question { .. } => ("question", None),
        GateKind::Plan { .. } => ("plan", None),
    };
    let diff = item.is_some_and(|item| {
        projection.items.iter().any(|candidate| {
            candidate.id == item
                && matches!(&candidate.kind, ItemKind::Tool(call) if call.diff.is_some())
        })
    });
    Some(AgentDecisionSnapshot {
        kind,
        item: item.map(|item| item.to_string()),
        diff,
    })
}
