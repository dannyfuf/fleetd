use super::*;

#[test]
fn blank_display_names_are_refused_everywhere_a_name_is_shown() {
    let base = board();
    let mut blank_status = base.clone();
    blank_status.statuses[0].name = "  ".into();
    assert!(validate_board(&blank_status).is_err());

    let mut blank_label = base.clone();
    blank_label.labels.push(Label {
        id: "bug".parse().unwrap(),
        name: String::new(),
        color: None,
    });
    assert!(validate_board(&blank_label).is_err());

    let mut blank_property = base;
    let mut schema = property("eta");
    schema.name = String::new();
    blank_property.properties.push(schema);
    assert!(validate_board(&blank_property).is_err());
}

#[test]
fn a_date_property_takes_only_a_real_calendar_day() {
    let mut board = board();
    let mut schema = property("eta");
    schema.kind = PropertyKind::Date;
    board.properties.push(schema);
    let mut card = create(&mut board, &[], "a");
    for value in ["not-a-date", "2026-02-30", "2026-13-01"] {
        let patch = CardPatch {
            properties: Some(
                [("eta".to_owned(), PropertyValue::Date(value.into()))]
                    .into_iter()
                    .collect(),
            ),
            ..CardPatch::default()
        };
        assert!(
            apply_card_patch(&board, &mut card.clone(), patch, LATER).is_err(),
            "`{value}` is not a day any picker could offer back"
        );
    }
    let patch = CardPatch {
        properties: Some(
            [("eta".to_owned(), PropertyValue::Date("2028-02-29".into()))]
                .into_iter()
                .collect(),
        ),
        ..CardPatch::default()
    };
    assert!(apply_card_patch(&board, &mut card, patch, LATER).is_ok());
}

#[test]
fn a_backend_owned_property_is_not_writable_by_a_patch_or_a_draft() {
    let mut board = board();
    let mut schema = property("points");
    schema.editable = false;
    schema.source = PropertySource::Backend;
    board.properties.push(schema);
    let properties: BTreeMap<_, _> = [("points".to_owned(), PropertyValue::Number(3.0))]
        .into_iter()
        .collect();
    let mut card = create(&mut board, &[], "a");
    let patch = CardPatch {
        properties: Some(properties.clone()),
        ..CardPatch::default()
    };
    assert!(
        apply_card_patch(&board, &mut card, patch, LATER).is_err(),
        "the next `apply_remote` discards it, and until then it raises a conflict on a \
         field no surface offers"
    );
    assert!(
        create_card(
            &mut board,
            &[card],
            "b".parse().unwrap(),
            CardDraft {
                title: "Fix login".into(),
                properties,
                ..CardDraft::default()
            },
            LATER
        )
        .is_err()
    );
}

/// A board whose backend refuses a field refuses it here, and nowhere else.
#[test]
fn a_read_only_field_is_refused_on_a_linked_board_only() {
    let mut local = board();
    let card = create(&mut local, &[], "a");
    // The list only means anything next to a backend: a local board declares none, and one
    // left over from a board that was unlinked must not freeze its own cards.
    local.sync.readonly_fields = vec!["priority".into(), "status_id".into()];
    assert_eq!(
        apply_card_patch(
            &local,
            &mut card.clone(),
            CardPatch {
                priority: Some(Priority::Urgent),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap(),
        vec!["priority".to_owned()]
    );

    let mut board = local.clone();
    board.backend = BackendRef {
        kind: "jira".into(),
        settings: serde_json::json!({"project": "SP"}),
    };
    let mut linked = card.clone();
    let error = apply_card_patch(
        &board,
        &mut linked,
        CardPatch {
            priority: Some(Priority::Urgent),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap_err();
    assert!(matches!(&error, BoardError::ReadOnlyField(field) if field == "priority"));
    assert_eq!(
        error.to_string(),
        "priority is read-only on this board's backend"
    );
    // The refusal is total: the card the caller handed in is untouched.
    assert_eq!(linked, card);

    // A field the backend does write goes through, and dirties the card for the push.
    assert_eq!(
        apply_card_patch(
            &board,
            &mut linked,
            CardPatch {
                title: Some("Fix logout".into()),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap(),
        vec!["title".to_owned()]
    );
    assert!(linked.dirty);

    // A patch that names the field but changes nothing is not an edit, and is not refused.
    let same = linked.priority;
    assert!(
        apply_card_patch(
            &board,
            &mut linked,
            CardPatch {
                priority: Some(same),
                ..CardPatch::default()
            },
            LATER,
        )
        .unwrap()
        .is_empty()
    );
}

/// The parent of a card the backend has never seen is the create's to carry.
///
/// `check_draft_writable` leaves `parent_id` out for exactly that reason, and the patch had
/// to make the same exemption or the field could only ever be set in the one draft that
/// created the card — which no surface offers — leaving a Jira board unable to express a
/// subtask at all.
#[test]
fn a_read_only_parent_is_still_settable_until_the_card_is_linked() {
    let mut board = board();
    board.backend = BackendRef {
        kind: "jira".into(),
        settings: serde_json::json!({"project": "SP"}),
    };
    board.sync.readonly_fields = vec!["parent_id".into()];
    let parent = create(&mut board, &[], "parent");
    let mut card = create(&mut board, std::slice::from_ref(&parent), "a");
    let patch = || CardPatch {
        parent_id: Some(Some(parent.id.clone())),
        ..CardPatch::default()
    };
    assert_eq!(
        apply_card_patch(&board, &mut card, patch(), LATER).unwrap(),
        vec!["parent_id".to_owned()]
    );
    assert_eq!(card.parent_id, Some(parent.id.clone()));

    // Once the card is linked the backend owns it, and the same edit is refused.
    let mut linked = create(&mut board, std::slice::from_ref(&parent), "b");
    linked.remote = Some(RemoteLink {
        backend: "jira".into(),
        key: "SP-9".into(),
        url: None,
        version: None,
        remote_updated_at: None,
        parent_key: None,
        synced_at: NOW.into(),
    });
    let before = linked.clone();
    assert!(matches!(
        apply_card_patch(&board, &mut linked, patch(), LATER),
        Err(BoardError::ReadOnlyField(field)) if field == "parent_id"
    ));
    assert_eq!(linked, before);
}

/// `status_id` is read-only through both doors: the patch and the move.
#[test]
fn a_read_only_status_refuses_a_move_but_still_reorders() {
    let mut board = board();
    let mut cards = vec![create(&mut board, &[], "a")];
    cards.push(create(&mut board, &cards, "b"));
    board.backend = BackendRef {
        kind: "jira".into(),
        settings: serde_json::Value::Null,
    };
    board.sync.readonly_fields = vec!["status_id".into()];
    let done: StatusId = "done".parse().unwrap();
    let id: CardId = "a".parse().unwrap();
    assert!(matches!(
        move_card(&board, &mut cards, &id, &done, None, LATER),
        Err(BoardError::ReadOnlyField(field)) if field == "status_id"
    ));
    assert_eq!(cards[0].status_id.as_str(), "todo");
    // Position is local ordering no backend is told about, so reordering stays allowed.
    let todo: StatusId = "todo".parse().unwrap();
    assert!(move_card(&board, &mut cards, &id, &todo, Some(1), LATER).unwrap());
    assert_eq!(cards[0].position, 10);
    assert!(!cards[0].dirty);
}
