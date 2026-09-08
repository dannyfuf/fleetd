use super::*;

#[test]
fn a_label_listed_twice_is_stored_once() {
    let mut board = board();
    board.labels = vec![Label {
        id: "bug".parse().unwrap(),
        name: "Bug".into(),
        color: None,
    }];
    let bug: LabelId = "bug".parse().unwrap();
    // The CLI dedupes what it resolves; the protocol takes whatever a client sends, and a
    // label repeated in a draft would render twice on the card forever.
    let mut card = create_card(
        &mut board,
        &[],
        "a".parse().unwrap(),
        CardDraft {
            title: "Fix login".into(),
            labels: vec![bug.clone(), bug.clone()],
            ..CardDraft::default()
        },
        NOW,
    )
    .unwrap();
    assert_eq!(card.labels, std::slice::from_ref(&bug));
    card.labels.clear();
    apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            labels: Some(vec![bug.clone(), bug.clone()]),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    assert_eq!(card.labels, [bug]);
}

#[test]
fn create_numbers_from_counter_and_records_creation() {
    let mut board = board();
    board.next_number = 42;
    let card = create(&mut board, &[], "a");
    assert_eq!((card.number, board.next_number, card.position), (42, 43, 0));
    assert_eq!(card.status_id.as_str(), "todo");
    assert_eq!(card.activity.len(), 1);
    assert_eq!(card.activity[0].kind, ActivityKind::Created);
    assert_eq!(card.created_at, NOW);
    assert!(!card.dirty);
}

#[test]
fn create_uses_first_status_without_unstarted() {
    let mut board = board();
    board
        .statuses
        .retain(|status| status.category != StatusCategory::Unstarted);
    assert_eq!(create(&mut board, &[], "a").status_id.as_str(), "backlog");
}

#[test]
fn create_honors_explicit_status_and_all_draft_fields() {
    let mut board = board();
    board.backend.kind = "fake".into();
    board.properties.push(property("score"));
    board.labels.push(Label {
        id: "bug".parse().unwrap(),
        name: "Bug".into(),
        color: None,
    });
    let draft = CardDraft {
        title: "Title".into(),
        description: "Details".into(),
        status_id: Some("done".parse().unwrap()),
        priority: Priority::High,
        labels: vec!["bug".parse().unwrap()],
        assignee: Some("A".into()),
        estimate: Some(3),
        due_date: Some("2028-02-29".into()),
        parent_id: Some("parent".parse().unwrap()),
        repo_id: Some("org/repo".parse().unwrap()),
        properties: BTreeMap::from([("score".into(), PropertyValue::Number(2.0))]),
    };
    let card = create_card(&mut board, &[], "a".parse().unwrap(), draft.clone(), NOW).unwrap();
    assert_eq!(card.title, draft.title);
    assert_eq!(card.description, draft.description);
    assert_eq!(Some(card.status_id), draft.status_id);
    assert_eq!(card.priority, draft.priority);
    assert_eq!(card.labels, draft.labels);
    assert_eq!(card.assignee, draft.assignee);
    assert_eq!(card.estimate, draft.estimate);
    assert_eq!(card.due_date, draft.due_date);
    assert_eq!(card.parent_id, draft.parent_id);
    assert_eq!(card.repo_id, draft.repo_id);
    assert_eq!(card.properties, draft.properties);
    assert!(card.dirty);
}

#[test]
fn create_appends_after_maximum_live_position() {
    let mut board = board();
    let mut first = create(&mut board, &[], "a");
    first.position = 77;
    let mut archived = first.clone();
    archived.id = "archived".parse().unwrap();
    archived.position = 900;
    archived.archived = true;
    let mut other = first.clone();
    other.id = "other".parse().unwrap();
    other.status_id = "done".parse().unwrap();
    other.position = 1000;
    assert_eq!(
        create(&mut board, &[other, first, archived], "b").position,
        87
    );
}

#[test]
fn invalid_create_is_atomic() {
    for draft in [
        CardDraft::default(),
        CardDraft {
            title: "  \n".into(),
            ..CardDraft::default()
        },
        CardDraft {
            title: "x".into(),
            status_id: Some("missing".parse().unwrap()),
            ..CardDraft::default()
        },
        CardDraft {
            title: "x".into(),
            labels: vec!["missing".parse().unwrap()],
            ..CardDraft::default()
        },
        CardDraft {
            title: "x".into(),
            due_date: Some("2025-02-29".into()),
            ..CardDraft::default()
        },
    ] {
        let mut board = board();
        let before = board.clone();
        assert!(create_card(&mut board, &[], "a".parse().unwrap(), draft, LATER).is_err());
        assert_eq!(board, before);
    }
}

#[test]
fn duplicate_card_ids_and_exhausted_counters_fail() {
    let mut board = board();
    let card = create(&mut board, &[], "a");
    let draft = CardDraft {
        title: "x".into(),
        ..CardDraft::default()
    };
    assert!(
        create_card(
            &mut board,
            &[card],
            "a".parse().unwrap(),
            draft.clone(),
            NOW
        )
        .is_err()
    );
    board.next_number = u64::MAX;
    assert!(create_card(&mut board, &[], "b".parse().unwrap(), draft, NOW).is_err());
    assert_eq!(board.next_number, u64::MAX);
}

#[test]
fn exhausted_column_position_does_not_consume_number() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    card.position = u64::MAX;
    let before = board.clone();
    assert!(
        create_card(
            &mut board,
            &[card],
            "b".parse().unwrap(),
            CardDraft {
                title: "x".into(),
                ..CardDraft::default()
            },
            LATER
        )
        .is_err()
    );
    assert_eq!(board, before);
}

#[test]
fn move_same_column_uses_index_after_removal() {
    let mut board = board();
    let a = create(&mut board, &[], "a");
    let b = create(&mut board, std::slice::from_ref(&a), "b");
    let c = create(&mut board, &[a.clone(), b.clone()], "c");
    let mut cards = vec![a, b, c];
    move_card(
        &board,
        &mut cards,
        &"a".parse().unwrap(),
        &"todo".parse().unwrap(),
        Some(1),
        LATER,
    )
    .unwrap();
    let column = column_cards(&cards, &"todo".parse().unwrap());
    assert_eq!(
        column
            .iter()
            .map(|card| (card.id.as_str(), card.position))
            .collect::<Vec<_>>(),
        [("b", 0), ("a", 10), ("c", 20)]
    );
    assert_eq!(cards[0].activity.last().unwrap().kind, ActivityKind::Moved);
    assert!(!cards[0].dirty);
}

#[test]
fn a_move_that_changes_nothing_reports_it_and_writes_no_history() {
    let mut board = board();
    let a = create(&mut board, &[], "a");
    let b = create(&mut board, std::slice::from_ref(&a), "b");
    let mut cards = vec![a, b];
    let before = cards.clone();
    for _ in 0..3 {
        assert!(
            !move_card(
                &board,
                &mut cards,
                &"a".parse().unwrap(),
                &"todo".parse().unwrap(),
                Some(0),
                LATER,
            )
            .unwrap()
        );
    }
    // Six identical moves used to append six activity entries and restamp the card.
    assert_eq!(cards, before);
    // A move that does reorder still reports and records one.
    assert!(
        move_card(
            &board,
            &mut cards,
            &"a".parse().unwrap(),
            &"todo".parse().unwrap(),
            Some(1),
            LATER,
        )
        .unwrap()
    );
    assert_eq!(cards[0].activity.len(), before[0].activity.len() + 1);
}

#[test]
fn move_cross_column_renumbers_target_and_marks_changes() {
    let mut board = board();
    board.backend.kind = "fake".into();
    let mut a = create(&mut board, &[], "a");
    let mut b = create(&mut board, &[], "b");
    a.dirty = false;
    b.dirty = false;
    b.status_id = "done".parse().unwrap();
    b.position = 45;
    let mut cards = vec![a, b];
    move_card(
        &board,
        &mut cards,
        &"a".parse().unwrap(),
        &"done".parse().unwrap(),
        Some(0),
        LATER,
    )
    .unwrap();
    assert_eq!((cards[0].position, cards[1].position), (0, 10));
    assert!(cards.iter().all(|card| card.updated_at == LATER));
    // Only the card that changed status owes the backend a push; `position` is local.
    assert!(cards[0].dirty);
    assert!(!cards[1].dirty);
}

#[test]
fn same_column_reorder_never_dirties_a_card_and_reads_as_a_reorder() {
    let mut board = board();
    board.backend.kind = "fake".into();
    let mut a = create(&mut board, &[], "a");
    let mut b = create(&mut board, &[], "b");
    a.dirty = false;
    b.dirty = false;
    b.position = 10;
    let mut cards = vec![a, b];
    move_card(
        &board,
        &mut cards,
        &"b".parse().unwrap(),
        &"todo".parse().unwrap(),
        Some(0),
        LATER,
    )
    .unwrap();
    assert!(cards.iter().all(|card| !card.dirty));
    assert_eq!(
        cards[1].activity.last().unwrap().message,
        "Reordered in todo"
    );
}

#[test]
fn move_none_and_oversized_indices_append_excluding_archived() {
    for index in [None, Some(999)] {
        let mut board = board();
        let a = create(&mut board, &[], "a");
        let b = create(&mut board, &[], "b");
        let mut archived = create(&mut board, &[], "archived");
        archived.archived = true;
        archived.position = 42;
        let mut cards = vec![a, b, archived.clone()];
        move_card(
            &board,
            &mut cards,
            &"a".parse().unwrap(),
            &"todo".parse().unwrap(),
            index,
            LATER,
        )
        .unwrap();
        assert_eq!((cards[0].position, cards[1].position), (10, 0));
        assert_eq!(cards[2], archived);
    }
}

#[test]
fn invalid_move_preserves_cards() {
    let mut board = board();
    let mut cards = vec![create(&mut board, &[], "a")];
    let before = cards.clone();
    assert!(matches!(
        move_card(
            &board,
            &mut cards,
            &"a".parse().unwrap(),
            &"bad".parse().unwrap(),
            None,
            LATER
        ),
        Err(BoardError::UnknownStatus(_))
    ));
    assert!(matches!(
        move_card(
            &board,
            &mut cards,
            &"bad".parse().unwrap(),
            &"todo".parse().unwrap(),
            None,
            LATER
        ),
        Err(BoardError::CardNotFound(_))
    ));
    assert_eq!(cards, before);
}
