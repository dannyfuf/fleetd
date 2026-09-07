use super::*;

#[test]
fn patch_reports_actual_fields_and_compact_activity() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    let changed = apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            title: Some("New".into()),
            description: Some("Details".into()),
            priority: Some(Priority::High),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert_eq!(changed, ["title", "description", "priority"]);
    assert_eq!(card.updated_at, LATER);
    assert!(!card.dirty);
    assert_eq!(
        card.activity.last().unwrap().message,
        "Updated title, description, priority"
    );
}

#[test]
fn patch_dirties_only_remote_board_and_leaves_noops_alone() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    board.backend.kind = "fake".into();
    assert!(
        apply_card_patch(&board, &mut card, CardPatch::default(), LATER)
            .unwrap()
            .is_empty()
    );
    // A patch that changes nothing is not a mutation: it neither stamps nor dirties.
    assert_eq!(card.updated_at, NOW);
    assert!(!card.dirty);
    assert_eq!(card.activity.len(), 1);
    let repeat = CardPatch {
        title: Some(card.title.clone()),
        ..CardPatch::default()
    };
    assert!(
        apply_card_patch(&board, &mut card, repeat, LATER)
            .unwrap()
            .is_empty()
    );
    assert_eq!(card.updated_at, NOW);
    assert!(!card.dirty);
    apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            title: Some("Edited".into()),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert!(card.dirty);
    board.backend = BackendRef::default();
    apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            title: Some("Edited again".into()),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert!(!card.dirty);
}

#[test]
fn patch_properties_merge_remove_and_preserve_unmentioned() {
    let mut board = board();
    board
        .properties
        .extend([property("a"), property("b"), property("c")]);
    let mut card = create(&mut board, &[], "a");
    card.properties.extend([
        ("a".into(), PropertyValue::Number(1.0)),
        ("b".into(), PropertyValue::Number(2.0)),
    ]);
    let changed = apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            properties: Some(BTreeMap::from([
                ("a".into(), PropertyValue::Null),
                ("c".into(), PropertyValue::Number(3.0)),
            ])),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert_eq!(changed, ["properties"]);
    assert_eq!(
        card.properties,
        BTreeMap::from([
            ("b".into(), PropertyValue::Number(2.0)),
            ("c".into(), PropertyValue::Number(3.0))
        ])
    );
}

#[test]
fn invalid_patch_rolls_back_all_fields() {
    let mut board = board();
    board.properties.push(property("score"));
    let mut card = create(&mut board, &[], "a");
    for patch in [
        CardPatch {
            title: Some(" ".into()),
            ..CardPatch::default()
        },
        CardPatch {
            title: Some("New".into()),
            status_id: Some("missing".parse().unwrap()),
            ..CardPatch::default()
        },
        CardPatch {
            title: Some("New".into()),
            properties: Some(BTreeMap::from([(
                "score".into(),
                PropertyValue::Text("bad".into()),
            )])),
            ..CardPatch::default()
        },
        CardPatch {
            properties: Some(BTreeMap::from([(
                "unknown".into(),
                PropertyValue::Bool(true),
            )])),
            ..CardPatch::default()
        },
    ] {
        let before = card.clone();
        assert!(apply_card_patch(&board, &mut card, patch, LATER).is_err());
        assert_eq!(card, before);
    }
}

#[test]
fn patch_clears_nullable_fields_and_can_archive() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    card.assignee = Some("A".into());
    card.estimate = Some(3);
    card.due_date = Some("2026-09-07".into());
    card.parent_id = Some("parent".parse().unwrap());
    card.repo_id = Some("org/repo".parse().unwrap());
    let fields = apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            assignee: Some(None),
            estimate: Some(None),
            due_date: Some(None),
            parent_id: Some(None),
            repo_id: Some(None),
            archived: Some(true),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert_eq!(
        fields,
        [
            "assignee",
            "estimate",
            "due_date",
            "parent_id",
            "repo_id",
            "archived"
        ]
    );
    assert!(card.assignee.is_none() && card.estimate.is_none() && card.due_date.is_none());
    assert!(card.parent_id.is_none() && card.repo_id.is_none() && card.archived);
}

#[test]
fn a_board_patch_that_changes_nothing_stamps_nothing() {
    let mut board = board();
    assert!(
        !apply_board_patch(&mut board, BoardPatch::default(), LATER).unwrap(),
        "an empty patch changes nothing"
    );
    assert_eq!(board.updated_at, NOW);
    let repeat = BoardPatch {
        name: Some(board.name.clone()),
        prefix: Some(board.prefix.clone()),
        settings: Some(board.settings.clone()),
        ..BoardPatch::default()
    };
    assert!(
        !apply_board_patch(&mut board, repeat, LATER).unwrap(),
        "a patch that restates the board changes nothing"
    );
    assert_eq!(
        board.updated_at, NOW,
        "stamping a no-op rewrites the document and announces a change nobody made"
    );
    assert!(
        apply_board_patch(
            &mut board,
            BoardPatch {
                name: Some("Renamed".into()),
                ..BoardPatch::default()
            },
            LATER
        )
        .unwrap()
    );
    assert_eq!(board.updated_at, LATER);
}

#[test]
fn board_patch_validates_prefix_atomically() {
    for prefix in ["", "lower", "ABCDEFGHI", "AB-1", "É"] {
        let mut board = board();
        let before = board.clone();
        assert!(
            apply_board_patch(
                &mut board,
                BoardPatch {
                    name: Some("New".into()),
                    prefix: Some(prefix.into()),
                    ..BoardPatch::default()
                },
                LATER
            )
            .is_err()
        );
        assert_eq!(board, before);
    }
    let mut board = board();
    apply_board_patch(
        &mut board,
        BoardPatch {
            prefix: Some("ABC12345".into()),
            ..BoardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert_eq!(board.prefix, "ABC12345");
    assert_eq!(board.updated_at, LATER);
}

#[test]
fn board_patch_rejects_empty_and_duplicate_schema_ids() {
    let mut board = board();
    let label = Label {
        id: "bug".parse().unwrap(),
        name: "Bug".into(),
        color: None,
    };
    for patch in [
        BoardPatch {
            statuses: Some(vec![]),
            ..BoardPatch::default()
        },
        BoardPatch {
            statuses: Some(vec![board.statuses[0].clone(); 2]),
            ..BoardPatch::default()
        },
        BoardPatch {
            labels: Some(vec![label; 2]),
            ..BoardPatch::default()
        },
        BoardPatch {
            properties: Some(vec![property("a"); 2]),
            ..BoardPatch::default()
        },
    ] {
        let before = board.clone();
        assert!(apply_board_patch(&mut board, patch, LATER).is_err());
        assert_eq!(board, before);
    }
}

#[test]
fn board_patch_allows_status_removal_for_service_to_check() {
    let mut board = board();
    let statuses = vec![board.statuses[0].clone()];
    apply_board_patch(
        &mut board,
        BoardPatch {
            statuses: Some(statuses.clone()),
            ..BoardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert_eq!(board.statuses, statuses);
}

#[test]
fn settings_pairs_merge_as_json_when_they_parse_and_as_text_otherwise() {
    let base = serde_json::json!({"project": "OLD", "jql": "sprint in openSprints()"});
    let merged = merge_settings(
        &base,
        &[
            ("project".into(), "SP".into()),
            ("maxConcurrency".into(), "8".into()),
            ("statuses".into(), r#"["To Do","Done"]"#.into()),
            ("fullSync".into(), "true".into()),
            ("jql".into(), "null".into()),
        ],
    )
    .unwrap();
    assert_eq!(
        merged,
        serde_json::json!({
            "project": "SP",
            "maxConcurrency": 8,
            "statuses": ["To Do", "Done"],
            "fullSync": true
        })
    );
    // `base` is untouched, and a null base starts from an empty object.
    assert_eq!(base["project"], serde_json::json!("OLD"));
    assert_eq!(
        merge_settings(&serde_json::Value::Null, &[("project".into(), "SP".into())]).unwrap(),
        serde_json::json!({"project": "SP"})
    );
    // A value that is not JSON is the string the user typed, quoting and all.
    assert_eq!(
        merge_settings(
            &serde_json::Value::Null,
            &[("jql".into(), "assignee = currentUser()".into())]
        )
        .unwrap(),
        serde_json::json!({"jql": "assignee = currentUser()"})
    );
    assert!(matches!(
        merge_settings(&serde_json::Value::Null, &[(" ".into(), "x".into())]),
        Err(BoardError::Invalid { .. })
    ));
    assert!(matches!(
        merge_settings(&serde_json::json!([1]), &[]),
        Err(BoardError::Invalid { .. })
    ));
}
