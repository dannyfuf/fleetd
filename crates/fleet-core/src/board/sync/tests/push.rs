use super::*;

#[test]
fn unchanged_remote_dirty_card_plans_update_and_mapped_transition() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.dirty = true;
    card.title = "Local".into();
    card.status_id = "in-progress".parse().unwrap();
    board
        .sync
        .status_map
        .local_to_remote
        .insert(card.status_id.clone(), "remote-progress".into());
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), LATER);
    assert_eq!(
        result.to_push,
        [
            PushOp::Update {
                card_id: "a".parse().unwrap(),
                fields: vec!["title".into()]
            },
            PushOp::Transition {
                card_id: "a".parse().unwrap(),
                remote_status: "remote-progress".into()
            }
        ]
    );
    assert_eq!(result.summary.conflicts, 0);
    assert_eq!(result.summary.pushed, 0); // Execution belongs to the daemon.
}

#[test]
fn transition_requires_status_difference_capability_and_mapping() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.dirty = true;
    board
        .sync
        .status_map
        .local_to_remote
        .insert(card.status_id.clone(), "remote-todo".into());
    assert!(
        reconcile(
            &board,
            std::slice::from_ref(&card),
            &pull(remote("R-1")),
            caps(),
            NOW
        )
        .to_push
        .is_empty()
    );
    card.status_id = "done".parse().unwrap();
    assert!(
        reconcile(
            &board,
            std::slice::from_ref(&card),
            &pull(remote("R-1")),
            caps(),
            NOW
        )
        .to_push
        .is_empty()
    );
    board
        .sync
        .status_map
        .local_to_remote
        .insert(card.status_id.clone(), "remote-done".into());
    let mut caps = caps();
    caps.transitions = false;
    assert!(
        reconcile(&board, &[card], &pull(remote("R-1")), caps, NOW)
            .to_push
            .is_empty()
    );
}

#[test]
fn push_capabilities_gate_updates_properties_and_comments() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.dirty = true;
    card.title = "Local".into();
    card.properties
        .insert("custom".into(), PropertyValue::Text("x".into()));
    add_comment(&mut card, "c".into(), None, "Hi".into(), NOW).unwrap();
    assert!(
        reconcile(
            &board,
            std::slice::from_ref(&card),
            &pull(remote("R-1")),
            BackendCapabilities::default(),
            NOW
        )
        .to_push
        .is_empty()
    );
    let caps = BackendCapabilities {
        push_updates: true,
        ..BackendCapabilities::default()
    };
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps, NOW);
    assert_eq!(
        result.to_push,
        [PushOp::Update {
            card_id: "a".parse().unwrap(),
            fields: vec!["title".into()]
        }]
    );
}

#[test]
fn incremental_missing_dirty_link_waits_for_a_baseline() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.dirty = true;
    card.status_id = "done".parse().unwrap();
    let result = reconcile(&board, &[card], &PullResult::default(), caps(), NOW);
    assert!(result.to_push.is_empty());
    assert!(result.cards[0].dirty);
}

#[test]
fn new_local_create_requires_setting_and_capability() {
    let mut board = board();
    let card = card(&mut board, "local");
    assert!(
        reconcile(
            &board,
            std::slice::from_ref(&card),
            &PullResult::default(),
            caps(),
            NOW
        )
        .to_push
        .is_empty()
    );
    board.settings.push_new_cards = true;
    assert!(
        reconcile(
            &board,
            std::slice::from_ref(&card),
            &PullResult::default(),
            BackendCapabilities::default(),
            NOW
        )
        .to_push
        .is_empty()
    );
    assert_eq!(
        reconcile(&board, &[card], &PullResult::default(), caps(), NOW).to_push,
        [PushOp::Create {
            card_id: "local".parse().unwrap()
        }]
    );
}

/// `pushNewCards` is off by default, so a card made on a linked board files no issue and
/// stays dirty forever. The sync reported `0 pushed` and nothing else; the count is what
/// lets it name the setting that is holding the card.
#[test]
fn a_card_push_new_cards_holds_back_is_counted_so_the_sync_can_say_so() {
    let mut board = board();
    let card = card(&mut board, "local");
    let held = reconcile(
        &board,
        std::slice::from_ref(&card),
        &PullResult::default(),
        caps(),
        NOW,
    );
    assert!(held.to_push.is_empty());
    assert_eq!(held.summary.kept_local, 1);
    board.settings.push_new_cards = true;
    let pushed = reconcile(
        &board,
        std::slice::from_ref(&card),
        &PullResult::default(),
        caps(),
        NOW,
    );
    assert!(!pushed.to_push.is_empty());
    assert_eq!(
        pushed.summary.kept_local, 0,
        "a card the push carries is not kept local"
    );
    // A local board has no backend to withhold anything from.
    let mut local = board.clone();
    local.backend.kind = BackendRef::LOCAL.into();
    local.settings.push_new_cards = false;
    assert_eq!(
        reconcile(&local, &[card], &PullResult::default(), caps(), NOW)
            .summary
            .kept_local,
        0
    );
}

/// A create carries no status, so a card born anywhere but the backend's opening column
/// lands in the wrong one — and the create's own ack stamps a version no later pull beats.
#[test]
fn a_created_card_pushes_the_column_it_was_born_in() {
    let mut board = board();
    let schema = BackendSchema {
        statuses: vec![
            status("todo", "Todo", Some(StatusCategory::Unstarted)),
            status("done-id", "Done", Some(StatusCategory::Completed)),
        ],
        ..BackendSchema::default()
    };
    adopt_schema(&mut board, &schema, NOW);
    board.settings.push_new_cards = true;
    let done = board.sync.status_map.remote_to_local["done-id"].clone();
    let mut fresh = card(&mut board, "local");
    fresh.status_id = done.clone();
    let result = reconcile(&board, &[fresh], &PullResult::default(), caps(), NOW);
    assert_eq!(
        result.to_push,
        [
            PushOp::Create {
                card_id: "local".parse().unwrap()
            },
            PushOp::Transition {
                card_id: "local".parse().unwrap(),
                remote_status: "done-id".into()
            }
        ],
        "the column has to be pushed right behind the create that gave it something to move"
    );
}

/// A field the backend cannot write back is never a side the user can hold: offering it as a
/// conflict lets `KeepLocal` plan an `Update` the backend drops while acknowledging it.
#[test]
fn a_read_only_field_is_neither_a_conflict_nor_an_update() {
    let mut board = board();
    board.sync.readonly_fields = vec!["priority".into()];
    board.settings.conflict_policy = ConflictPolicy::Manual;
    let mut card = linked(&mut board);
    card.title = "Renamed here".into();
    card.dirty = true;
    let mut remote = remote("R-1");
    remote.version = Some("2".into());
    remote.priority = Some(Priority::Urgent);
    let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
    let conflict = result.cards[0]
        .conflict
        .as_ref()
        .expect("the title still conflicts");
    assert_eq!(
        conflict.fields,
        ["title"],
        "priority is Jira's alone; the user was never offered a side to keep"
    );
}

#[test]
fn unpushed_comments_emit_once_per_local_id_even_on_clean_cards() {
    let mut board = board();
    let mut card = linked(&mut board);
    add_comment(&mut card, "local".into(), None, "Local".into(), NOW).unwrap();
    add_comment(&mut card, "sent".into(), None, "Sent".into(), NOW).unwrap();
    card.comments[1].remote_id = Some("remote".into());
    card.dirty = false;
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), NOW);
    assert_eq!(
        result.to_push,
        [PushOp::AddComment {
            card_id: "a".parse().unwrap(),
            comment_id: "local".into()
        }]
    );
}

#[test]
fn unlinked_comments_follow_create_and_are_not_sent_without_create() {
    let mut board = board();
    let mut card = card(&mut board, "local");
    add_comment(&mut card, "c".into(), None, "Hi".into(), NOW).unwrap();
    assert!(
        reconcile(
            &board,
            std::slice::from_ref(&card),
            &PullResult::default(),
            caps(),
            NOW
        )
        .to_push
        .is_empty()
    );
    board.settings.push_new_cards = true;
    assert_eq!(
        reconcile(&board, &[card], &PullResult::default(), caps(), NOW).to_push,
        [
            PushOp::Create {
                card_id: "local".parse().unwrap()
            },
            PushOp::AddComment {
                card_id: "local".parse().unwrap(),
                comment_id: "c".into()
            }
        ]
    );
}

/// A move into a column no remote status is mapped to is dropped by `plan_push`; without
/// this the card is dirty forever and the sync reports nothing at all.
#[test]
fn a_move_into_an_unmapped_column_is_named_by_the_summary() {
    let mut board = board();
    let mut card = linked(&mut board);
    // The remote sits in the first column, the card was moved to the second, and no remote
    // status is mapped to that one — exactly what a sampled `describe` leaves behind when
    // no issue happens to be in that status.
    board
        .sync
        .status_map
        .remote_to_local
        .insert("remote-todo".into(), board.statuses[0].id.clone());
    board.sync.status_map.local_to_remote.clear();
    card.dirty = true;
    card.status_id = board.statuses[1].id.clone();
    let column = board.statuses[1].name.clone();
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), LATER);
    assert!(
        result
            .to_push
            .iter()
            .all(|op| !matches!(op, PushOp::Transition { .. }))
    );
    assert!(
        result.summary.unmapped_statuses.contains(&column),
        "{:?}",
        result.summary.unmapped_statuses
    );
}

#[test]
fn archived_cards_are_not_pushed() {
    let mut board = board();
    board.settings.push_new_cards = true;
    let mut card = card(&mut board, "a");
    card.archived = true;
    assert!(
        reconcile(&board, &[card], &PullResult::default(), caps(), NOW)
            .to_push
            .is_empty()
    );
}

/// A create files the parent alongside the child, but the ack that links the child only
/// carries its own key. Rebuilding the link from the previous one leaves `parent_key` at
/// `None`, and the next pull — which reports the very `updated` the ack stored, so nothing
/// looks changed — then derives `parent_id` from that `None` and unparents a card Jira
/// holds, in a read-only field the user cannot put back.
#[test]
fn a_created_child_keeps_the_parent_the_push_filed_for_it() {
    let mut board = board();
    let mut parent = card(&mut board, "a");
    parent.dirty = true;
    let mut child = card(&mut board, "b");
    child.parent_id = Some("a".parse().unwrap());
    child.dirty = true;
    let mut cards = vec![parent, child];
    let acks = PushResult {
        acks: vec![
            PushAck {
                card_id: "a".parse().unwrap(),
                key: "R-1".into(),
                url: None,
                version: Some("1".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            },
            PushAck {
                card_id: "b".parse().unwrap(),
                key: "R-2".into(),
                url: None,
                version: Some("1".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            },
        ],
        failures: vec![],
    };
    apply_push_result(&mut cards, &acks, "fake", &[], NOW);
    assert_eq!(
        cards[1].remote.as_ref().unwrap().parent_key.as_deref(),
        Some("R-1")
    );
    // The pull that follows repeats the version the ack stored, so nothing about the child
    // reads as changed; the hierarchy still has to survive it.
    let mut remote_child = remote("R-2");
    remote_child.parent_key = Some("R-1".into());
    let pull = PullResult {
        cards: vec![remote("R-1"), remote_child],
        ..PullResult::default()
    };
    let result = reconcile(&board, &cards, &pull, caps(), LATER);
    let child = result
        .cards
        .iter()
        .find(|card| card.id.as_str() == "b")
        .unwrap();
    assert_eq!(child.parent_id.as_ref().map(CardId::as_str), Some("a"));
    assert_eq!(
        child.remote.as_ref().unwrap().parent_key.as_deref(),
        Some("R-1")
    );
}

#[test]
fn local_backend_never_plans_remote_writes() {
    let mut board = board();
    board.backend = BackendRef::default();
    board.settings.push_new_cards = true;
    let card = card(&mut board, "a");
    assert!(
        reconcile(&board, &[card], &PullResult::default(), caps(), NOW)
            .to_push
            .is_empty()
    );
}
