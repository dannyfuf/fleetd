//! Pure child-event rules for durable delegation state.
//!
//! Nothing here performs I/O and nothing here imports the session manager: the SQL half in
//! `store/delegations.rs` calls [`child_transition`] inside the writer's transaction, so a
//! manager call from this file would deadlock the writer thread against the operation lock.
use chrono::{DateTime, Utc};
use fleet_core::agents::{
    AbortReason, AgentEvent, Delegation, DelegationResult, DelegationStatus, ItemKind, ItemPatch,
    ItemStatus, ResultSource, StopCause, ToolCall, TurnOutcome,
};

use super::limits::MAX_NUDGES;
// `store::delegations` is private to the store, so the action enum is taken from the store's own
// re-export rather than through the module that declares it.
use crate::services::agents::store::OutboxAction;

/// Projection facts needed to transition a child without coupling to the manager or SQL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DelegationFacts {
    /// A question, plan, or permission gate was open before applying the event.
    pub gate_open: bool,
    /// The child had at least one live background task before applying the event.
    pub background_live: bool,
    /// Latest assistant message text, capped at the first 4 KiB.
    pub last_assistant_text: Option<String>,
    /// Paths touched by edit-like tools, deduplicated in first-seen order.
    pub files_changed: Vec<String>,
    /// Persisted stop cause when the child session is stopped.
    pub stop_cause: Option<StopCause>,
}

/// The new delegation row and transactional follow-up actions for one child event.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Transition {
    pub next: Delegation,
    pub actions: Vec<OutboxAction>,
    pub changed: bool,
}

/// Applies the child-side rules from `NATIVE-AGENTS.md` without performing I/O.
pub(crate) fn child_transition(
    current: &Delegation,
    event: &AgentEvent,
    facts: &DelegationFacts,
    now: DateTime<Utc>,
) -> Transition {
    if current.status.is_terminal() {
        return unchanged(current);
    }

    let mut next = current.clone();
    let mut actions = Vec::new();
    let reported_blocked = current.status_payload.as_deref() == Some("reported blocked");

    if matches!(event, AgentEvent::TurnSettled { .. }) {
        capture_last_assistant_result(&mut next, facts);
        next.headline = first_line(facts.last_assistant_text.as_deref());
    }

    match event {
        AgentEvent::SessionConfigured { .. } if current.status == DelegationStatus::Starting => {
            next.status = DelegationStatus::Running;
            next.status_payload = None;
            actions.push(OutboxAction::Mirror);
        }
        AgentEvent::GateOpened { .. } => {
            next.status = DelegationStatus::Blocked;
            if !reported_blocked {
                next.status_payload = None;
            }
            actions.push(OutboxAction::Mirror);
        }
        AgentEvent::GateResolved { .. } | AgentEvent::GateWithdrawn { .. }
            if current.status == DelegationStatus::Blocked =>
        {
            if !reported_blocked {
                next.status = DelegationStatus::Running;
                next.status_payload = None;
            }
            actions.push(OutboxAction::Mirror);
        }
        AgentEvent::TurnSettled {
            outcome: TurnOutcome::Completed,
            ..
        } if reported_blocked => {
            finish(
                &mut next,
                DelegationStatus::Failed,
                Some("reported blocked".into()),
                now,
            );
            actions.push(OutboxAction::Deliver);
        }
        AgentEvent::TurnSettled {
            outcome: TurnOutcome::Completed,
            ..
        } if facts.gate_open => {
            // A completed provider turn does not resolve a question, plan, or permission gate.
            // Keep the child blocked until the matching GateResolved/GateWithdrawn event arrives;
            // otherwise a normal blocked child can be finalized or nudged behind an open card.
            next.status = DelegationStatus::Blocked;
            next.status_payload = None;
        }
        AgentEvent::TurnSettled {
            outcome: TurnOutcome::Completed,
            ..
        } => {
            if has_reported_result(current) {
                if facts.background_live {
                    next.status = DelegationStatus::Settling;
                    next.status_payload = None;
                    actions.push(OutboxAction::Settle);
                } else {
                    finish(&mut next, DelegationStatus::Succeeded, None, now);
                    actions.push(OutboxAction::Deliver);
                }
            } else if current.nudges < MAX_NUDGES {
                next.status = DelegationStatus::Settling;
                next.status_payload = None;
                actions.push(OutboxAction::Nudge);
            } else {
                finish(&mut next, DelegationStatus::Incomplete, None, now);
                actions.push(OutboxAction::Deliver);
            }
        }
        AgentEvent::TurnSettled { outcome, .. } => match outcome {
            TurnOutcome::Error { message } => {
                fail_settled(&mut next, outcome_payload("error", message.as_deref()), now);
                actions.push(OutboxAction::Deliver);
            }
            TurnOutcome::MaxTurns => {
                fail_settled(&mut next, "max turns".into(), now);
                actions.push(OutboxAction::Deliver);
            }
            TurnOutcome::BudgetExhausted => {
                fail_settled(&mut next, "budget exhausted".into(), now);
                actions.push(OutboxAction::Deliver);
            }
            TurnOutcome::Denied => {
                fail_settled(&mut next, "denied".into(), now);
                actions.push(OutboxAction::Deliver);
            }
            TurnOutcome::Other { reason } => {
                fail_settled(&mut next, outcome_payload("other", Some(reason)), now);
                actions.push(OutboxAction::Deliver);
            }
            TurnOutcome::Interrupted => {
                finish(&mut next, DelegationStatus::Cancelled, None, now);
                actions.extend([OutboxAction::Deliver, OutboxAction::CancelChildren]);
            }
            TurnOutcome::Completed => {}
        },
        AgentEvent::TurnAborted {
            reason: AbortReason::ProviderExited,
            ..
        } => {
            if current.recoveries == 0 {
                next.recoveries = 1;
                actions.push(OutboxAction::Recover);
            } else {
                finish(
                    &mut next,
                    DelegationStatus::Failed,
                    Some("provider exited twice".into()),
                    now,
                );
                actions.push(OutboxAction::Deliver);
            }
        }
        AgentEvent::TurnAborted { .. } => {
            finish(&mut next, DelegationStatus::Cancelled, None, now);
            actions.extend([OutboxAction::Deliver, OutboxAction::CancelChildren]);
        }
        AgentEvent::RuntimeError {
            fatal: true,
            message,
        } => {
            finish(
                &mut next,
                DelegationStatus::Failed,
                Some(message.clone()),
                now,
            );
            actions.push(OutboxAction::Deliver);
        }
        AgentEvent::SessionExited {
            expected: false, ..
        } => {
            finish(
                &mut next,
                DelegationStatus::Failed,
                Some("provider exited".into()),
                now,
            );
            actions.push(OutboxAction::Deliver);
        }
        AgentEvent::SessionExited { expected: true, .. } => {
            finish(&mut next, DelegationStatus::Cancelled, None, now);
            actions.extend([OutboxAction::Deliver, OutboxAction::CancelChildren]);
        }
        AgentEvent::ItemStarted { kind, .. } => {
            next.headline = started_headline(kind);
        }
        AgentEvent::ItemUpdated { patch, .. } if patch_is_terminal(patch) => {
            next.headline = None;
        }
        AgentEvent::ItemCompleted { status, .. } if item_status_is_terminal(*status) => {
            next.headline = None;
        }
        _ => {}
    }

    let changed = next != *current;
    Transition {
        next,
        actions,
        changed,
    }
}

fn unchanged(current: &Delegation) -> Transition {
    Transition {
        next: current.clone(),
        actions: Vec::new(),
        changed: false,
    }
}

fn capture_last_assistant_result(next: &mut Delegation, facts: &DelegationFacts) {
    if has_reported_result(next) {
        return;
    }
    next.result = Some(DelegationResult {
        text: facts.last_assistant_text.clone().unwrap_or_default(),
        files_changed: facts.files_changed.clone(),
        source: ResultSource::LastAssistantText,
        elided: false,
    });
}

fn has_reported_result(delegation: &Delegation) -> bool {
    matches!(
        delegation.result.as_ref(),
        Some(DelegationResult {
            source: ResultSource::Reported,
            ..
        })
    )
}

fn finish(
    next: &mut Delegation,
    status: DelegationStatus,
    status_payload: Option<String>,
    now: DateTime<Utc>,
) {
    next.status = status;
    next.status_payload = status_payload;
    next.finished = Some(now);
}

fn fail_settled(next: &mut Delegation, status_payload: String, now: DateTime<Utc>) {
    finish(next, DelegationStatus::Failed, Some(status_payload), now);
}

fn outcome_payload(name: &str, detail: Option<&str>) -> String {
    detail.map_or_else(|| name.to_owned(), |detail| format!("{name}: {detail}"))
}

fn first_line(text: Option<&str>) -> Option<String> {
    text.and_then(|text| text.lines().next())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn started_headline(kind: &ItemKind) -> Option<String> {
    match kind {
        ItemKind::Tool(call) => tool_headline(call),
        _ => None,
    }
}

fn tool_headline(call: &ToolCall) -> Option<String> {
    call.summary
        .as_deref()
        .filter(|summary| !summary.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            call.input
                .get("command")
                .and_then(serde_json::Value::as_str)
                .filter(|command| !command.is_empty())
                .map(str::to_owned)
        })
}

fn patch_is_terminal(patch: &ItemPatch) -> bool {
    patch.status.is_some_and(item_status_is_terminal)
}

const fn item_status_is_terminal(status: ItemStatus) -> bool {
    matches!(
        status,
        ItemStatus::Completed | ItemStatus::Failed | ItemStatus::Denied | ItemStatus::Stopped
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use fleet_core::agents::{
        AgentKind, DelegationCaller, DeliveryState, GateAnswer, GateId, GateKind, GateResolver,
        ItemId, PermissionMode, ThreadId, ToolKind, TurnId, Usage,
    };
    use serde_json::json;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_842, 0).expect("valid test timestamp")
    }

    fn delegation(status: DelegationStatus) -> Delegation {
        Delegation {
            id: fleet_core::agents::DelegationId::new(),
            caller: DelegationCaller::Thread(ThreadId::new()),
            caller_turn: Some(TurnId::new()),
            caller_item: Some(ItemId::new()),
            child: ThreadId::new(),
            provider: AgentKind::Codex,
            depth: 1,
            brief: "Implement transitions".into(),
            expectation: "Focused tests pass".into(),
            eager: false,
            status,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: now() - chrono::Duration::seconds(842),
            finished: None,
            headline: None,
            usage: None,
        }
    }

    fn facts() -> DelegationFacts {
        DelegationFacts {
            gate_open: false,
            background_live: false,
            last_assistant_text: Some("Finished the work.\nMore detail.".into()),
            files_changed: vec!["src/lib.rs".into()],
            stop_cause: None,
        }
    }

    fn configured() -> AgentEvent {
        AgentEvent::SessionConfigured {
            provider: AgentKind::Codex,
            resume_cursor: None,
            model: None,
            models: Vec::new(),
            mode: PermissionMode::FullAccess,
            tools: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
        }
    }

    fn settled(outcome: TurnOutcome) -> AgentEvent {
        AgentEvent::TurnSettled {
            turn: TurnId::new(),
            outcome,
            usage: Usage::default(),
            duration_ms: 1,
            files_changed: Vec::new(),
        }
    }

    fn reported_result() -> DelegationResult {
        DelegationResult {
            text: "Explicit report".into(),
            files_changed: vec!["src/lib.rs".into()],
            source: ResultSource::Reported,
            elided: false,
        }
    }

    fn assert_terminal(
        transition: &Transition,
        status: DelegationStatus,
        actions: &[OutboxAction],
    ) {
        assert_eq!(transition.next.status, status);
        assert_eq!(transition.next.finished, Some(now()));
        assert_eq!(transition.actions, actions);
        assert!(transition.changed);
    }

    #[test]
    fn configured_moves_starting_to_running_and_mirrors() {
        let transition = child_transition(
            &delegation(DelegationStatus::Starting),
            &configured(),
            &facts(),
            now(),
        );
        assert_eq!(transition.next.status, DelegationStatus::Running);
        assert_eq!(transition.actions, [OutboxAction::Mirror]);
        assert!(transition.changed);
    }

    #[test]
    fn every_gate_kind_blocks_and_mirrors() {
        let gates = [
            GateKind::Question {
                questions: Vec::new(),
            },
            GateKind::Plan {
                markdown: "Plan".into(),
                steps: Vec::new(),
            },
            GateKind::Permission {
                item: None,
                tool: ToolKind::Bash,
                title: "Run".into(),
                payload: "cargo test".into(),
                rationale: None,
                options: Vec::new(),
            },
        ];
        for kind in gates {
            let event = AgentEvent::GateOpened {
                gate: GateId::new(),
                turn: None,
                kind,
            };
            let transition = child_transition(
                &delegation(DelegationStatus::Running),
                &event,
                &facts(),
                now(),
            );
            assert_eq!(transition.next.status, DelegationStatus::Blocked);
            assert_eq!(transition.actions, [OutboxAction::Mirror]);
        }
    }

    #[test]
    fn resolving_or_withdrawing_a_gate_resumes_and_mirrors() {
        let gate = GateId::new();
        let events = [
            AgentEvent::GateResolved {
                gate,
                answer: GateAnswer::Question {
                    answers: Vec::new(),
                },
                by: GateResolver::User,
            },
            AgentEvent::GateWithdrawn { gate },
        ];
        for event in events {
            let transition = child_transition(
                &delegation(DelegationStatus::Blocked),
                &event,
                &facts(),
                now(),
            );
            assert_eq!(transition.next.status, DelegationStatus::Running);
            assert_eq!(transition.actions, [OutboxAction::Mirror]);
        }
    }

    #[test]
    fn completed_report_succeeds_and_delivers_without_background_work() {
        let mut current = delegation(DelegationStatus::Running);
        current.result = Some(reported_result());
        let transition =
            child_transition(&current, &settled(TurnOutcome::Completed), &facts(), now());
        assert_terminal(
            &transition,
            DelegationStatus::Succeeded,
            &[OutboxAction::Deliver],
        );
        assert_eq!(transition.next.result, current.result);
    }

    #[test]
    fn completed_turn_stays_blocked_while_a_gate_is_open() {
        let mut current = delegation(DelegationStatus::Blocked);
        current.result = Some(reported_result());
        let mut facts = facts();
        facts.gate_open = true;

        let transition =
            child_transition(&current, &settled(TurnOutcome::Completed), &facts, now());

        assert_eq!(transition.next.status, DelegationStatus::Blocked);
        assert_eq!(transition.next.finished, None);
        assert!(transition.actions.is_empty());
    }

    #[test]
    fn completed_report_settles_while_background_work_is_live() {
        let mut current = delegation(DelegationStatus::Running);
        current.result = Some(reported_result());
        let mut facts = facts();
        facts.background_live = true;
        let transition =
            child_transition(&current, &settled(TurnOutcome::Completed), &facts, now());
        assert_eq!(transition.next.status, DelegationStatus::Settling);
        assert_eq!(transition.next.finished, None);
        assert_eq!(transition.actions, [OutboxAction::Settle]);
    }

    #[test]
    fn completed_without_report_settles_and_nudges_below_the_limit() {
        let transition = child_transition(
            &delegation(DelegationStatus::Running),
            &settled(TurnOutcome::Completed),
            &facts(),
            now(),
        );
        assert_eq!(transition.next.status, DelegationStatus::Settling);
        assert_eq!(transition.actions, [OutboxAction::Nudge]);
        assert_eq!(
            transition.next.result,
            Some(DelegationResult {
                text: "Finished the work.\nMore detail.".into(),
                files_changed: vec!["src/lib.rs".into()],
                source: ResultSource::LastAssistantText,
                elided: false,
            })
        );
    }

    #[test]
    fn completed_without_report_becomes_incomplete_when_nudges_are_exhausted() {
        let mut current = delegation(DelegationStatus::Settling);
        current.nudges = MAX_NUDGES;
        let transition =
            child_transition(&current, &settled(TurnOutcome::Completed), &facts(), now());
        assert_terminal(
            &transition,
            DelegationStatus::Incomplete,
            &[OutboxAction::Deliver],
        );
        assert!(matches!(
            transition.next.result,
            Some(DelegationResult {
                source: ResultSource::LastAssistantText,
                ..
            })
        ));
    }

    #[test]
    fn reported_blocked_settle_fails_and_delivers() {
        let mut current = delegation(DelegationStatus::Blocked);
        current.status_payload = Some("reported blocked".into());
        current.result = Some(reported_result());
        let transition =
            child_transition(&current, &settled(TurnOutcome::Completed), &facts(), now());
        assert_terminal(
            &transition,
            DelegationStatus::Failed,
            &[OutboxAction::Deliver],
        );
        assert_eq!(
            transition.next.status_payload.as_deref(),
            Some("reported blocked")
        );
    }

    #[test]
    fn gate_events_cannot_erase_a_reported_blocked_cause() {
        let mut current = delegation(DelegationStatus::Blocked);
        current.status_payload = Some("reported blocked".into());
        current.result = Some(reported_result());
        let gate = GateId::new();
        for event in [
            AgentEvent::GateOpened {
                gate,
                turn: None,
                kind: GateKind::Plan {
                    markdown: "Plan".into(),
                    steps: Vec::new(),
                },
            },
            AgentEvent::GateResolved {
                gate,
                answer: GateAnswer::Plan(fleet_core::agents::PlanAnswer::Approve),
                by: GateResolver::User,
            },
            AgentEvent::GateWithdrawn { gate },
        ] {
            let transition = child_transition(&current, &event, &facts(), now());
            assert_eq!(transition.next.status, DelegationStatus::Blocked);
            assert_eq!(
                transition.next.status_payload.as_deref(),
                Some("reported blocked")
            );
        }

        let mut open_gate = facts();
        open_gate.gate_open = true;
        let transition = child_transition(
            &current,
            &settled(TurnOutcome::Completed),
            &open_gate,
            now(),
        );
        assert_terminal(
            &transition,
            DelegationStatus::Failed,
            &[OutboxAction::Deliver],
        );
        assert_eq!(
            transition.next.status_payload.as_deref(),
            Some("reported blocked")
        );
    }

    #[test]
    fn failed_settle_outcomes_name_the_reason_capture_text_and_deliver() {
        let cases = [
            (
                TurnOutcome::Error {
                    message: Some("boom".into()),
                },
                "error: boom",
            ),
            (TurnOutcome::Error { message: None }, "error"),
            (TurnOutcome::MaxTurns, "max turns"),
            (TurnOutcome::BudgetExhausted, "budget exhausted"),
            (TurnOutcome::Denied, "denied"),
            (
                TurnOutcome::Other {
                    reason: "lost".into(),
                },
                "other: lost",
            ),
        ];
        for (outcome, payload) in cases {
            let transition = child_transition(
                &delegation(DelegationStatus::Running),
                &settled(outcome),
                &facts(),
                now(),
            );
            assert_terminal(
                &transition,
                DelegationStatus::Failed,
                &[OutboxAction::Deliver],
            );
            assert_eq!(transition.next.status_payload.as_deref(), Some(payload));
            assert!(matches!(
                transition.next.result,
                Some(DelegationResult {
                    source: ResultSource::LastAssistantText,
                    ..
                })
            ));
        }
    }

    #[test]
    fn interrupted_settle_cancels_delivers_and_cancels_children() {
        let transition = child_transition(
            &delegation(DelegationStatus::Running),
            &settled(TurnOutcome::Interrupted),
            &facts(),
            now(),
        );
        assert_terminal(
            &transition,
            DelegationStatus::Cancelled,
            &[OutboxAction::Deliver, OutboxAction::CancelChildren],
        );
    }

    #[test]
    fn explicit_abort_reasons_cancel_deliver_and_cancel_children() {
        let reasons = [
            AbortReason::User,
            AbortReason::SessionStopped,
            AbortReason::Timeout,
            AbortReason::Superseded,
            AbortReason::Other("native reason".into()),
        ];
        for reason in reasons {
            let event = AgentEvent::TurnAborted {
                turn: TurnId::new(),
                reason,
            };
            let transition = child_transition(
                &delegation(DelegationStatus::Running),
                &event,
                &facts(),
                now(),
            );
            assert_terminal(
                &transition,
                DelegationStatus::Cancelled,
                &[OutboxAction::Deliver, OutboxAction::CancelChildren],
            );
        }
    }

    #[test]
    fn provider_exit_abort_recovers_once_then_fails_and_delivers() {
        let event = AgentEvent::TurnAborted {
            turn: TurnId::new(),
            reason: AbortReason::ProviderExited,
        };
        let first = child_transition(
            &delegation(DelegationStatus::Running),
            &event,
            &facts(),
            now(),
        );
        assert_eq!(first.next.status, DelegationStatus::Running);
        assert_eq!(first.next.recoveries, 1);
        assert_eq!(first.next.finished, None);
        assert_eq!(first.actions, [OutboxAction::Recover]);
        assert!(first.changed);

        let transition = child_transition(&first.next, &event, &facts(), now());
        assert_terminal(
            &transition,
            DelegationStatus::Failed,
            &[OutboxAction::Deliver],
        );
        assert_eq!(
            transition.next.status_payload.as_deref(),
            Some("provider exited twice")
        );
    }

    #[test]
    fn provider_exit_preserves_a_blocked_status_during_its_one_recovery() {
        let current = delegation(DelegationStatus::Blocked);
        let transition = child_transition(
            &current,
            &AgentEvent::TurnAborted {
                turn: TurnId::new(),
                reason: AbortReason::ProviderExited,
            },
            &facts(),
            now(),
        );
        assert_eq!(transition.next.status, DelegationStatus::Blocked);
        assert_eq!(transition.next.recoveries, 1);
        assert_eq!(transition.actions, [OutboxAction::Recover]);
    }

    #[test]
    fn fatal_runtime_error_fails_and_delivers() {
        let event = AgentEvent::RuntimeError {
            fatal: true,
            message: "transport failed".into(),
        };
        let transition = child_transition(
            &delegation(DelegationStatus::Running),
            &event,
            &facts(),
            now(),
        );
        assert_terminal(
            &transition,
            DelegationStatus::Failed,
            &[OutboxAction::Deliver],
        );
        assert_eq!(
            transition.next.status_payload.as_deref(),
            Some("transport failed")
        );
    }

    #[test]
    fn unexpected_session_exit_fails_and_delivers() {
        let event = AgentEvent::SessionExited {
            code: Some(1),
            expected: false,
        };
        let transition = child_transition(
            &delegation(DelegationStatus::Running),
            &event,
            &facts(),
            now(),
        );
        assert_terminal(
            &transition,
            DelegationStatus::Failed,
            &[OutboxAction::Deliver],
        );
    }

    #[test]
    fn expected_session_exit_cancels_delivers_and_cancels_children() {
        let event = AgentEvent::SessionExited {
            code: Some(0),
            expected: true,
        };
        let transition = child_transition(
            &delegation(DelegationStatus::Running),
            &event,
            &facts(),
            now(),
        );
        assert_terminal(
            &transition,
            DelegationStatus::Cancelled,
            &[OutboxAction::Deliver, OutboxAction::CancelChildren],
        );
    }

    #[test]
    fn tool_start_uses_summary_then_command_for_the_headline() {
        for (summary, input, expected) in [
            (
                Some("Run focused tests".into()),
                json!({}),
                "Run focused tests",
            ),
            (None, json!({ "command": "cargo test" }), "cargo test"),
        ] {
            let event = AgentEvent::ItemStarted {
                turn: TurnId::new(),
                item: ItemId::new(),
                kind: ItemKind::Tool(Box::new(ToolCall {
                    kind: ToolKind::Bash,
                    name: "exec".into(),
                    input,
                    summary,
                    result: None,
                    output: String::new(),
                    diff: None,
                    exit_code: None,
                    duration_ms: None,
                    extra: BTreeMap::new(),
                })),
                parent: None,
            };
            let transition = child_transition(
                &delegation(DelegationStatus::Running),
                &event,
                &facts(),
                now(),
            );
            assert_eq!(transition.next.headline.as_deref(), Some(expected));
            assert!(transition.changed);
        }
    }

    #[test]
    fn terminal_item_updates_and_completions_clear_the_headline() {
        let events = [
            AgentEvent::ItemUpdated {
                item: ItemId::new(),
                patch: ItemPatch {
                    payload: None,
                    status: Some(ItemStatus::Completed),
                },
            },
            AgentEvent::ItemCompleted {
                item: ItemId::new(),
                status: ItemStatus::Failed,
            },
        ];
        for event in events {
            let mut current = delegation(DelegationStatus::Running);
            current.headline = Some("cargo test".into());
            let transition = child_transition(&current, &event, &facts(), now());
            assert_eq!(transition.next.headline, None);
            assert!(transition.changed);
        }
    }

    #[test]
    fn settle_headline_is_the_first_line_of_the_last_assistant_text() {
        let transition = child_transition(
            &delegation(DelegationStatus::Running),
            &settled(TurnOutcome::Completed),
            &facts(),
            now(),
        );
        assert_eq!(
            transition.next.headline.as_deref(),
            Some("Finished the work.")
        );
    }

    #[test]
    fn content_delta_never_changes_the_headline() {
        let mut current = delegation(DelegationStatus::Running);
        current.headline = Some("cargo test".into());
        let event = AgentEvent::ContentDelta {
            item: ItemId::new(),
            stream: fleet_core::agents::StreamKind::AssistantText,
            delta: "partial".into(),
        };
        let transition = child_transition(&current, &event, &facts(), now());
        assert_eq!(transition.next, current);
        assert!(!transition.changed);
        assert!(transition.actions.is_empty());
    }

    #[test]
    fn terminal_delegations_ignore_later_terminal_events() {
        let mut current = delegation(DelegationStatus::Succeeded);
        current.finished = Some(now());
        let transition = child_transition(
            &current,
            &settled(TurnOutcome::Error {
                message: Some("late".into()),
            }),
            &facts(),
            now() + chrono::Duration::seconds(1),
        );
        assert_eq!(transition.next, current);
        assert!(!transition.changed);
        assert!(transition.actions.is_empty());
    }
}
