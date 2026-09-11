//! Pure guards for noisy harness lifecycle streams.

use serde::{Deserialize, Serialize};

use super::{TurnId, TurnOutcome};

/// Lifecycle event class used by [`should_apply_lifecycle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    /// The harness process exited.
    SessionExited,
    /// The harness session started or was configured.
    SessionStarted,
    /// A turn began.
    TurnStarted,
    /// A turn reached its harness-authoritative terminal signal.
    TurnSettled,
    /// A turn was explicitly aborted.
    TurnAborted,
    /// A harness runtime or protocol error, which is not itself a settlement.
    RuntimeError,
    /// Any lifecycle-neutral event.
    Other,
}

/// Returns whether a lifecycle event may update the current thread state.
///
/// Harnesses can replay stale starts, omit turn ids, and deliver delayed terminal frames. The
/// pending-start argument also carries the steering exception: a conflicting start is accepted
/// only when it names the turn Fleet is already waiting for the harness to confirm.
///
/// This is the guard for the **adapter** boundary, where a harness frame may carry no turn id at
/// all: `AgentEvent` has already attributed every lifecycle event to a turn, so an adapter has to
/// decide the question before it can emit one. The reducer enforces the same four rules again on
/// the events it does receive — `ThreadProjection::validate` — because the log it writes is
/// append-only and one rejected event there must not leave a half-applied turn.
#[must_use]
pub fn should_apply_lifecycle(
    event: LifecycleKind,
    event_turn: Option<TurnId>,
    active_turn: Option<TurnId>,
    pending_start: Option<TurnId>,
) -> bool {
    let conflicts = active_turn.is_some() && event_turn.is_some() && active_turn != event_turn;
    let missing_for_active = active_turn.is_some() && event_turn.is_none();

    match event {
        LifecycleKind::SessionExited | LifecycleKind::SessionStarted | LifecycleKind::Other => true,
        // A runtime error is guarded by a weaker version of the same rule (spec A.7.1): it
        // applies unless it names a turn that is not the one running, because Codex's `error`
        // notification is not terminal and a delayed one must not fail the live turn.
        LifecycleKind::RuntimeError => !conflicts,
        LifecycleKind::TurnStarted => {
            !conflicts || (pending_start.is_some() && pending_start == event_turn)
        }
        LifecycleKind::TurnSettled | LifecycleKind::TurnAborted => {
            if conflicts || missing_for_active {
                return false;
            }
            if active_turn.is_some() {
                return active_turn == event_turn;
            }
            matches!(event, LifecycleKind::TurnSettled) && event_turn.is_some()
        }
    }
}

/// Returns the outcome a turn keeps when the harness settles it more than once.
///
/// §3.3 rule 8: settlement is **sticky**. Both harnesses can report a turn twice — Claude's
/// interrupted turn still emits its own `result`, and Codex answers `turn/interrupt` before
/// `turn/completed` lands — so the later frame routinely claims a cleaner ending than the one
/// the user caused. A failure never downgrades, and an interruption never downgrades to a
/// completion, which is what makes "the user pressed Stop and the turn completed anyway" render
/// as the stop it was.
#[must_use]
pub fn sticky_outcome(current: &TurnOutcome, incoming: &TurnOutcome) -> TurnOutcome {
    match (current, incoming) {
        (TurnOutcome::Error { .. }, _) | (TurnOutcome::Interrupted, TurnOutcome::Completed) => {
            current.clone()
        }
        _ => incoming.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> TurnId {
        TurnId::new()
    }

    #[test]
    fn stale_terminal_events_never_end_the_active_turn() {
        let active = turn();
        let stale = turn();
        assert!(!should_apply_lifecycle(
            LifecycleKind::TurnSettled,
            Some(stale),
            Some(active),
            None,
        ));
        assert!(!should_apply_lifecycle(
            LifecycleKind::TurnAborted,
            Some(stale),
            Some(active),
            None,
        ));
    }

    #[test]
    fn unattributed_terminal_events_do_not_end_an_active_turn() {
        assert!(!should_apply_lifecycle(
            LifecycleKind::TurnSettled,
            None,
            Some(turn()),
            None,
        ));
    }

    #[test]
    fn a_named_settlement_recovers_a_lost_start_but_an_abort_does_not() {
        let recovered = turn();
        assert!(should_apply_lifecycle(
            LifecycleKind::TurnSettled,
            Some(recovered),
            None,
            None,
        ));
        assert!(!should_apply_lifecycle(
            LifecycleKind::TurnAborted,
            Some(recovered),
            None,
            None,
        ));
    }

    #[test]
    fn a_runtime_error_applies_unless_it_names_another_turn() {
        let active = turn();
        assert!(should_apply_lifecycle(
            LifecycleKind::RuntimeError,
            Some(active),
            Some(active),
            None,
        ));
        assert!(should_apply_lifecycle(
            LifecycleKind::RuntimeError,
            None,
            Some(active),
            None,
        ));
        assert!(should_apply_lifecycle(
            LifecycleKind::RuntimeError,
            Some(turn()),
            None,
            None,
        ));
        assert!(!should_apply_lifecycle(
            LifecycleKind::RuntimeError,
            Some(turn()),
            Some(active),
            None,
        ));
    }

    #[test]
    fn only_the_pending_steering_start_may_replace_an_active_turn() {
        let active = turn();
        let pending = turn();
        assert!(should_apply_lifecycle(
            LifecycleKind::TurnStarted,
            Some(pending),
            Some(active),
            Some(pending),
        ));
        assert!(!should_apply_lifecycle(
            LifecycleKind::TurnStarted,
            Some(turn()),
            Some(active),
            Some(pending),
        ));
    }

    #[test]
    fn settlement_is_sticky_in_both_directions() {
        let failed = TurnOutcome::Error {
            message: Some("boom".to_owned()),
        };
        assert_eq!(
            sticky_outcome(&failed, &TurnOutcome::Completed),
            failed,
            "a failure never downgrades"
        );
        assert_eq!(
            sticky_outcome(&failed, &TurnOutcome::Interrupted),
            failed,
            "a failure never downgrades to an interruption either"
        );
        assert_eq!(
            sticky_outcome(&TurnOutcome::Interrupted, &TurnOutcome::Completed),
            TurnOutcome::Interrupted,
            "the turn the user stopped stays stopped"
        );
        assert_eq!(
            sticky_outcome(&TurnOutcome::Interrupted, &failed),
            failed,
            "an authoritative failure still lands on an interrupted turn"
        );
        assert_eq!(
            sticky_outcome(&TurnOutcome::Completed, &TurnOutcome::Interrupted),
            TurnOutcome::Interrupted,
            "a completion is not sticky: a later authoritative frame wins"
        );
    }
}
