use super::*;

fn apply_structural(
    projection: &mut ThreadProjection,
    events: &mut EventBuilder,
    event: AgentEvent,
) {
    let event = events.next(event);
    assert_eq!(
        projection
            .apply_described(&event)
            .unwrap_or_else(|error| panic!("{error}")),
        Applied::Structural,
        "{:?} must invalidate structure",
        event.event
    );
}

#[test]
fn apply_described_classifies_every_normalized_event_variant() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(900);
    let user = item_id(901);
    let assistant = item_id(902);
    let first_gate = gate_id(903);
    let second_gate = gate_id(904);
    let plan_gate = gate_id(905);

    apply_structural(&mut projection, &mut events, session_started());
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::MetadataChanged {
            title: Some("described".to_owned()),
            mode: Some(PermissionMode::Plan),
            model: None,
            skills: None,
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::SessionStateChanged(SessionState::Running),
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::SessionActivity {
            phase: "thinking".to_owned(),
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::TurnStarted {
            turn,
            user_item: user,
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::ItemStarted {
            turn,
            item: assistant,
            kind: ItemKind::AssistantText {
                text: "hello".to_owned(),
            },
            parent: None,
        },
    );
    let delta = events.next(AgentEvent::ContentDelta {
        item: assistant,
        stream: StreamKind::AssistantText,
        delta: " world".to_owned(),
    });
    assert_eq!(
        projection
            .apply_described(&delta)
            .unwrap_or_else(|error| panic!("{error}")),
        Applied::Text {
            item: assistant,
            stream: StreamKind::AssistantText,
            appended: 5..11,
        }
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::ItemUpdated {
            item: assistant,
            patch: ItemPatch::default(),
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::ItemCompleted {
            item: assistant,
            status: ItemStatus::Completed,
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate: first_gate,
            turn: Some(turn),
            kind: GateKind::Question {
                questions: Vec::new(),
            },
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::GateResolved {
            gate: first_gate,
            answer: GateAnswer::Question {
                answers: Vec::new(),
            },
            by: GateResolver::User,
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate: second_gate,
            turn: Some(turn),
            kind: GateKind::Question {
                questions: Vec::new(),
            },
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::GateWithdrawn { gate: second_gate },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::PlanProposed {
            gate: plan_gate,
            turn,
            markdown: "# Plan".to_owned(),
            steps: vec!["ship".to_owned()],
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::GateWithdrawn { gate: plan_gate },
    );
    for event in [
        AgentEvent::TurnDiff {
            turn,
            unified: String::new(),
            files_changed: Vec::new(),
        },
        AgentEvent::PlanSteps {
            turn,
            steps: vec!["test".to_owned()],
        },
        AgentEvent::TokenUsage {
            turn,
            usage: usage(8),
            context_pct: 1.0,
            cost_usd: Some(0.01),
        },
        AgentEvent::RateLimits { limits: json!({}) },
        AgentEvent::Compacted(crate::agents::CheckpointKind::CompactBoundary {
            before: 10,
            after: Some(2),
        }),
        AgentEvent::Retrying {
            attempt: 1,
            retry_in_ms: 50,
            reason: "busy".to_owned(),
        },
        AgentEvent::ModelRerouted {
            from: "a".to_owned(),
            to: "b".to_owned(),
            reason: "capacity".to_owned(),
        },
        AgentEvent::RuntimeError {
            fatal: false,
            message: "recoverable".to_owned(),
        },
        AgentEvent::Notice("notice".to_owned()),
        AgentEvent::Unknown {
            method: "future/event".to_owned(),
        },
    ] {
        apply_structural(&mut projection, &mut events, event);
    }
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Completed,
            usage: usage(8),
            duration_ms: 10,
            files_changed: Vec::new(),
        },
    );
    let aborted = turn_id(906);
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::TurnStarted {
            turn: aborted,
            user_item: item_id(907),
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::TurnAborted {
            turn: aborted,
            reason: AbortReason::User,
        },
    );
    apply_structural(
        &mut projection,
        &mut events,
        AgentEvent::SessionExited {
            code: Some(0),
            expected: true,
        },
    );
}
