use super::*;

/// A file approval carries the exact normalized item id and a human path sentence, never a UUID.
#[test]
fn a_file_approval_joins_the_file_change_item_by_id() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let provider_turn = "01a089f2-579b-75c2-8e51-3492ec617046";
    let turn = session.turn_for(provider_turn);
    session.begin_turn(turn);
    session.adopt_turn(turn, provider_turn);
    let started = map::handle(
        &mut session,
        "item/started",
        &json!({
            "threadId": "t",
            "turnId": provider_turn,
            "startedAtMs": 1,
            "item": {
                "type": "fileChange",
                "id": "patch-1",
                "changes": [{
                    "path": "README.md",
                    "diff": "--- a/README.md\n+++ b/README.md\n+fixed\n",
                    "kind": {"type": "update"},
                }],
                "status": "inProgress",
            },
        }),
    );
    let started_item = started.events.iter().find_map(|event| match event {
        AgentEvent::ItemStarted { item, .. } => Some(*item),
        _ => None,
    });
    let outcome = approvals::handle(
        &mut session,
        "item/fileChange/requestApproval",
        &json!("approval-1"),
        &json!({
            "threadId": "t",
            "turnId": provider_turn,
            "itemId": "patch-1",
            "startedAtMs": 2,
        }),
    );
    let Some(AgentEvent::GateOpened {
        kind: fleet_core::agents::GateKind::Permission { item, payload, .. },
        ..
    }) = outcome.events.first()
    else {
        panic!("expected a file approval gate");
    };
    assert_eq!(*item, started_item);
    assert_eq!(payload, "apply the edit to README.md");
}

/// A gate may beat its item, but its card is explicit about waiting rather than empty.
#[test]
fn a_file_approval_before_its_item_has_a_loading_payload() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let outcome = approvals::handle(
        &mut session,
        "item/fileChange/requestApproval",
        &json!("approval-1"),
        &json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "itemId": "patch-1",
            "startedAtMs": 1,
        }),
    );
    let Some(AgentEvent::GateOpened {
        kind: fleet_core::agents::GateKind::Permission { item, payload, .. },
        ..
    }) = outcome.events.first()
    else {
        panic!("expected a file approval gate");
    };
    assert!(
        item.is_some(),
        "the future item already has a deterministic id"
    );
    assert_eq!(payload, "loading edit details\u{2026}");
}

/// Codex builds that omit `clientId` still reconcile both item lifecycle frames by text.
#[test]
fn a_user_echo_without_client_id_is_reconciled_once_by_text() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let provider_turn = "01a089f2-579b-75c2-8e51-3492ec617046";
    let turn = TurnId::new();
    let item = ItemId::new();
    session.alias_turn(provider_turn, turn);
    session.remember_user_item(turn, item, "same text");
    session.begin_turn(turn);
    session.adopt_turn(turn, provider_turn);
    let mut frame = |method: &str| {
        let timestamp = if method == "item/started" {
            ("startedAtMs", 1)
        } else {
            ("completedAtMs", 2)
        };
        let mut value = json!({
            "threadId": "t",
            "turnId": provider_turn,
            "item": {
                "type": "userMessage",
                "id": "user-1",
                "content": [{"type": "text", "text": "same text", "text_elements": []}],
            },
        });
        value
            .as_object_mut()
            .unwrap_or_else(|| panic!("fixed object"))
            .insert(timestamp.0.to_owned(), json!(timestamp.1));
        map::handle(&mut session, method, &value)
    };
    assert!(frame("item/started").events.is_empty());
    assert!(frame("item/completed").events.is_empty());
}

/// Resumed history restores Fleet's durable item identity from Codex's client id.
#[test]
fn resumed_user_history_adopts_the_stable_client_item_id() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let provider_turn = "01a089f2-579b-75c2-8e51-3492ec617046";
    let turn = TurnId::new();
    let item = ItemId::new();
    session.alias_turn(provider_turn, turn);
    session.adopt_turn(turn, provider_turn);

    let output = map::handle(
        &mut session,
        "item/started",
        &json!({
            "threadId": "t",
            "turnId": provider_turn,
            "startedAtMs": 1,
            "item": {
                "type": "userMessage",
                "id": "provider-user-1",
                "clientId": item.to_string(),
                "content": [{"type": "text", "text": "durable delivery", "text_elements": []}],
            },
        }),
    );

    assert!(matches!(
        output.events.as_slice(),
        [AgentEvent::ItemStarted { item: restored, .. }] if *restored == item
    ));
    assert_eq!(session.item_for("t", "provider-user-1"), item);
}

/// A failed `turn/start` leaves no pending correlation for a later unrelated notification.
#[test]
fn a_failed_turn_start_rolls_back_its_pending_alias_and_echo() {
    let mut session = CodexSession::default();
    let turn = TurnId::new();
    let item = ItemId::new();
    session.remember_user_item(turn, item, "will fail");
    session.begin_turn(turn);

    session.rollback_turn_start(turn);

    assert_eq!(session.pending_start, None);
    assert!(!session.user_items.contains_key(&turn));
    assert!(!session.client_items.contains_key(&item.to_string()));
    assert!(session.pending_user_echoes.is_empty());
}

#[test]
fn model_discovery_adopts_the_selected_models_declared_default_effort() {
    let mut session = CodexSession::default();
    session.controls.model = Some("gpt-5.1-codex".to_owned());

    session.install_models(vec![json!({
        "id": "gpt-5.1-codex",
        "supportedReasoningEfforts": [
            {"reasoningEffort": "low", "description": "fast"},
            {"reasoningEffort": "high", "description": "deep"},
        ],
        "defaultReasoningEffort": "high",
    })]);

    assert_eq!(
        session.model_selection().and_then(|model| model.effort),
        Some("high".to_owned())
    );
    assert_eq!(
        session.models[0]
            .pointer("/supportedReasoningEfforts/1/description")
            .and_then(Value::as_str),
        Some("deep")
    );
}

#[test]
fn model_discovery_normalizes_effort_descriptions_and_the_declared_default() {
    let descriptors = catalogue::model_descriptors(&[json!({
        "id": "gpt-5.6-sol",
        "displayName": "GPT-5.6 Sol",
        "supportedReasoningEfforts": [
            {"reasoningEffort": "low", "description": "Fast answers"},
            {"reasoningEffort": "xhigh", "description": "Deep reasoning"},
        ],
        "defaultReasoningEffort": "low",
    })]);

    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].id, "gpt-5.6-sol");
    assert_eq!(descriptors[0].display_name, "GPT-5.6 Sol");
    assert_eq!(descriptors[0].default_effort.as_deref(), Some("low"));
    assert_eq!(descriptors[0].efforts[1].id, "xhigh");
    assert_eq!(descriptors[0].efforts[1].description, "Deep reasoning");
}
