use super::*;

#[test]
fn remote_updates_preserve_local_property_ownership() {
    let mut board = board();
    board.properties.push(PropertySchema {
        key: "note".into(),
        name: "Note".into(),
        kind: PropertyKind::Text,
        options: vec![],
        editable: true,
        source: PropertySource::Local,
        show_on_card: false,
    });
    board
        .properties
        .push(property("backend", PropertySource::Backend));
    let mut card = linked(&mut board);
    card.properties
        .insert("note".into(), PropertyValue::Text("private".into()));
    card.properties
        .insert("backend".into(), PropertyValue::Text("old".into()));
    let mut remote = remote("R-1");
    remote
        .properties
        .insert("backend".into(), PropertyValue::Text("new".into()));
    // Neither schema declares this key, so it must not reach the card: a property the
    // board cannot validate would make every later `validate_card` reject the document.
    remote
        .properties
        .insert("undeclared".into(), PropertyValue::Text("x".into()));
    apply_remote(&mut board, &mut card, &remote, LATER);
    assert!(!card.properties.contains_key("undeclared"));
    assert!(validate_card(&board, &card).is_ok());
    assert_eq!(
        card.properties["note"],
        PropertyValue::Text("private".into())
    );
    assert_eq!(
        card.properties["backend"],
        PropertyValue::Text("new".into())
    );
    remote
        .properties
        .insert("note".into(), PropertyValue::Text("collision".into()));
    apply_remote(&mut board, &mut card, &remote, LATER);
    assert_eq!(
        card.properties["note"],
        PropertyValue::Text("private".into())
    );
}

#[test]
fn a_reverted_edit_or_reorder_clears_dirty_only_with_no_pending_comments() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.dirty = true;
    card.position = 20;
    let result = reconcile(&board, &[card.clone()], &pull(remote("R-1")), caps(), NOW);
    assert!(!result.cards[0].dirty);
    assert!(result.to_push.is_empty());
    add_comment(&mut card, "pending".into(), None, "Hello".into(), NOW).unwrap();
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), NOW);
    assert!(result.cards[0].dirty);
    assert!(matches!(
        result.to_push.as_slice(),
        [PushOp::AddComment { .. }]
    ));
}

#[test]
fn remote_versions_take_precedence_over_timestamps() {
    let mut board = board();
    let card = linked(&mut board);
    let link = card.remote.unwrap();
    let mut remote = remote("R-1");
    remote.updated_at = Some(LATER.into());
    assert!(!remote_changed(&link, &remote));
    remote.version = Some("2".into());
    remote.updated_at = Some(NOW.into());
    assert!(remote_changed(&link, &remote));
}

#[test]
fn remote_change_falls_back_to_time_then_always_changed() {
    let mut board = board();
    let card = linked(&mut board);
    let mut link = card.remote.unwrap();
    let mut remote = remote("R-1");
    remote.version = None;
    assert!(!remote_changed(&link, &remote));
    remote.updated_at = Some(LATER.into());
    assert!(remote_changed(&link, &remote));
    remote.updated_at = None;
    assert!(remote_changed(&link, &remote));
    link.version = None;
    link.remote_updated_at = None;
    assert!(remote_changed(&link, &remote));
}

#[test]
fn apply_remote_maps_status_by_explicit_map_before_name_and_category() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    board
        .sync
        .status_map
        .remote_to_local
        .insert("remote-todo".into(), "done".parse().unwrap());
    apply_remote(&mut board, &mut card, &remote("R-1"), NOW);
    assert_eq!(card.status_id.as_str(), "done");
    board.sync.status_map = StatusMap::default();
    let mut remote = remote("R-1");
    remote.status.category = Some(StatusCategory::Started);
    apply_remote(&mut board, &mut card, &remote, NOW);
    assert_eq!(card.status_id.as_str(), "todo");
    remote.status.name = "Something".into();
    apply_remote(&mut board, &mut card, &remote, NOW);
    assert_eq!(card.status_id.as_str(), "in-progress");
}

#[test]
fn apply_remote_unknown_status_keeps_local_and_records_reason() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    let mut remote = remote("R-1");
    remote.status = status("weird", "Weird", None);
    apply_remote(&mut board, &mut card, &remote, LATER);
    assert_eq!(card.status_id.as_str(), "todo");
    assert!(
        card.activity
            .last()
            .unwrap()
            .message
            .contains("unmapped status weird")
    );
}

#[test]
fn apply_remote_creates_missing_labels_with_unique_slug_ids() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    let mut remote = remote("R-1");
    remote.labels = vec!["Bug!".into(), "Bug?".into(), "中文".into(), "Bug!".into()];
    apply_remote(&mut board, &mut card, &remote, NOW);
    assert_eq!(
        board
            .labels
            .iter()
            .map(|label| label.id.as_str())
            .collect::<Vec<_>>(),
        ["bug", "bug-2", "label"]
    );
    assert_eq!(card.labels.len(), 3);
    assert!(validate_card(&board, &card).is_ok());
    let before = board.clone();
    apply_remote(&mut board, &mut card, &remote, NOW);
    assert_eq!(board, before);
}

#[test]
fn apply_remote_copies_fields_and_keeps_local_worktree_metadata() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    card.repo_id = Some("org/repo".parse().unwrap());
    card.parent_id = Some("parent".parse().unwrap());
    card.position = 70;
    card.archived = true;
    let mut remote = remote("R-1");
    remote.description = "Details".into();
    remote.priority = Some(Priority::Urgent);
    remote.assignee = Some("A".into());
    remote.estimate = Some(8);
    remote.due_date = Some("2026-09-07".into());
    board
        .properties
        .push(property("a", PropertySource::Backend));
    remote
        .properties
        .insert("a".into(), PropertyValue::Text("value".into()));
    remote.url = Some("https://example.test/R-1".into());
    apply_remote(&mut board, &mut card, &remote, LATER);
    assert_eq!(card.description, "Details");
    assert_eq!(card.priority, Priority::Urgent);
    assert_eq!(card.assignee, remote.assignee);
    assert_eq!(card.estimate, remote.estimate);
    assert_eq!(card.due_date, remote.due_date);
    assert_eq!(card.properties, remote.properties);
    assert!(card.parent_id.is_none());
    assert_eq!(card.position, 70);
    assert!(card.archived);
    assert_eq!(card.repo_id.unwrap().as_str(), "org/repo");
    let link = card.remote.unwrap();
    assert_eq!(link.version, remote.version);
    assert_eq!(link.remote_updated_at, remote.updated_at);
    assert_eq!(link.url, remote.url);
    assert_eq!(link.synced_at, LATER);
    assert!(!card.dirty);
}

#[test]
fn apply_remote_merges_comments_by_remote_id_and_preserves_unsent() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    add_comment(&mut card, "remote:1".into(), None, "Unsent".into(), NOW).unwrap();
    let mut remote = remote("R-1");
    remote.comments = vec![comment("1")];
    apply_remote(&mut board, &mut card, &remote, NOW);
    assert_eq!(card.comments.len(), 2);
    assert_ne!(card.comments[0].id, card.comments[1].id);
    let local_id = card.comments[1].id.clone();
    remote.comments[0].body = "Edited".into();
    apply_remote(&mut board, &mut card, &remote, LATER);
    assert_eq!(card.comments.len(), 2);
    assert_eq!(card.comments[0].body, "Unsent");
    assert_eq!(card.comments[1].body, "Edited");
    assert_eq!(card.comments[1].id, local_id);
    let before = card.clone();
    apply_remote(&mut board, &mut card, &remote, LATER);
    assert_eq!(card, before);
}

#[test]
fn apply_push_result_links_card_and_acknowledges_comment_ids() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    add_comment(&mut card, "c".into(), None, "Hi".into(), NOW).unwrap();
    let mut cards = vec![card];
    apply_push_result(
        &mut cards,
        &PushResult {
            acks: vec![PushAck {
                card_id: "a".parse().unwrap(),
                key: "R-9".into(),
                url: Some("url".into()),
                version: Some("2".into()),
                remote_updated_at: None,
                comment_ids: vec![("c".into(), "remote-c".into())],
            }],
            failures: vec![],
        },
        "fake",
        &[],
        LATER,
    );
    assert!(!cards[0].dirty);
    assert_eq!(cards[0].remote.as_ref().unwrap().key, "R-9");
    assert_eq!(cards[0].remote.as_ref().unwrap().synced_at, LATER);
    assert_eq!(cards[0].comments[0].remote_id.as_deref(), Some("remote-c"));
    assert_eq!(cards[0].activity.last().unwrap().kind, ActivityKind::Synced);
}

#[test]
fn a_transition_only_ack_keeps_fields_the_backend_never_accepted_dirty() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.status_id = "in-progress".parse().unwrap();
    card.dirty = true;
    board
        .sync
        .status_map
        .local_to_remote
        .insert(card.status_id.clone(), "remote-progress".into());
    let caps = BackendCapabilities {
        push_updates: false,
        ..caps()
    };
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps, LATER);
    // Only the transition can be sent; the retitle has nowhere to go.
    assert_eq!(
        result.to_push,
        [PushOp::Transition {
            card_id: "a".parse().unwrap(),
            remote_status: "remote-progress".into()
        }]
    );
    assert_eq!(result.unpushed, ["a".parse::<CardId>().unwrap()]);
    let mut cards = result.cards;
    apply_push_result(
        &mut cards,
        &PushResult {
            acks: vec![PushAck {
                card_id: "a".parse().unwrap(),
                key: "R-1".into(),
                url: None,
                version: Some("2".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        },
        "fake",
        &result.unpushed,
        LATER,
    );
    // Clearing dirty here would let the next pull overwrite the unsent title.
    assert!(cards[0].dirty);
    assert_eq!(cards[0].title, "Local");
}

#[test]
fn push_partial_failure_keeps_dirty_and_records_error_after_ack() {
    let mut board = board();
    let card = linked(&mut board);
    let mut cards = vec![card];
    let result = PushResult {
        acks: vec![PushAck {
            card_id: "a".parse().unwrap(),
            key: "R-1".into(),
            url: None,
            version: Some("2".into()),
            remote_updated_at: None,
            comment_ids: vec![],
        }],
        failures: vec![PushFailure {
            card_id: "a".parse().unwrap(),
            error: "Transition rejected".into(),
        }],
    };
    apply_push_result(&mut cards, &result, "fake", &[], LATER);
    assert!(cards[0].dirty);
    assert!(
        cards[0]
            .activity
            .last()
            .unwrap()
            .message
            .contains("Transition rejected")
    );
    // The ack carries a version, not the remote's own stamp: the last one a pull saw
    // stands, because dropping it leaves the card with no `remoteUpdatedAt` at all until
    // some later pull happens to change `version`.
    assert_eq!(
        cards[0]
            .remote
            .as_ref()
            .unwrap()
            .remote_updated_at
            .as_deref(),
        Some("2026-09-06T12:00:00Z")
    );
}

#[test]
fn push_results_ignore_missing_card_and_comment_ids() {
    let mut board = board();
    let card = linked(&mut board);
    let mut cards = vec![card.clone()];
    apply_push_result(
        &mut cards,
        &PushResult {
            acks: vec![PushAck {
                card_id: "missing".parse().unwrap(),
                key: "R-1".into(),
                url: None,
                version: None,
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![PushFailure {
                card_id: "missing".parse().unwrap(),
                error: "Oops".into(),
            }],
        },
        "fake",
        &[],
        NOW,
    );
    assert_eq!(cards, [card]);
}

#[test]
fn resolve_keep_local_updates_baseline_and_avoids_repeat_conflict() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.title = "Local".into();
    let mut remote = remote("R-1");
    remote.version = Some("2".into());
    conflict(&mut card, remote.clone());
    resolve_conflict(&board, &mut card, ConflictResolution::KeepLocal, LATER).unwrap();
    assert_eq!(card.title, "Local");
    assert!(card.dirty);
    assert!(card.conflict.is_none());
    assert_eq!(
        card.activity.last().unwrap().kind,
        ActivityKind::ConflictResolved
    );
    let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
    assert_eq!(result.summary.conflicts, 0);
    assert_eq!(result.to_push.len(), 1);
}

/// The other half of "keep local": the fields the *remote* owns are not a side the user
/// can hold. `differing_fields` already drops them from the conflict for that reason, and
/// leaving them behind froze the local copy at a value no push could carry — the link this
/// resolution stamps carries the remote's own version, so no later pull ever revisited it.
#[test]
fn resolve_keep_local_still_adopts_the_fields_the_remote_owns() {
    let mut board = board();
    board.sync.readonly_fields = vec!["priority".into(), "due_date".into()];
    board.properties.push(PropertySchema {
        key: "jira.issue_type".into(),
        name: "Issue type".into(),
        kind: PropertyKind::Select,
        options: vec![],
        editable: false,
        source: PropertySource::Backend,
        show_on_card: false,
    });
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.priority = Priority::Medium;
    card.properties.insert(
        "jira.issue_type".into(),
        PropertyValue::Select("Task".into()),
    );
    let mut remote = remote("R-1");
    remote.version = Some("2".into());
    remote.title = "Remote".into();
    remote.priority = Some(Priority::Urgent);
    remote.due_date = Some("2026-12-31".into());
    remote.properties.insert(
        "jira.issue_type".into(),
        PropertyValue::Select("Bug".into()),
    );
    conflict(&mut card, remote.clone());
    resolve_conflict(&board, &mut card, ConflictResolution::KeepLocal, LATER).unwrap();
    // The side the user chose to keep.
    assert_eq!(card.title, "Local");
    // The sides only the remote can write.
    assert_eq!(card.priority, Priority::Urgent);
    assert_eq!(card.due_date.as_deref(), Some("2026-12-31"));
    assert_eq!(
        card.properties.get("jira.issue_type"),
        Some(&PropertyValue::Select("Bug".into()))
    );
    // And no second conflict over any of them.
    let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
    assert_eq!(result.summary.conflicts, 0);
}

#[test]
fn resolve_take_remote_applies_fields_and_clears_dirty() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.title = "Local".into();
    let mut remote = remote("R-1");
    remote.title = "Remote".into();
    conflict(&mut card, remote);
    resolve_conflict(&board, &mut card, ConflictResolution::TakeRemote, LATER).unwrap();
    assert_eq!(card.title, "Remote");
    assert!(!card.dirty);
    assert!(card.conflict.is_none());
    assert_eq!(
        card.activity.last().unwrap().kind,
        ActivityKind::ConflictResolved
    );
}

#[test]
fn resolve_missing_conflict_is_an_atomic_error() {
    let mut board = board();
    let mut card = linked(&mut board);
    let before = card.clone();
    assert!(resolve_conflict(&board, &mut card, ConflictResolution::KeepLocal, LATER).is_err());
    assert_eq!(card, before);
}

#[test]
fn resolve_take_remote_requires_service_to_materialize_new_labels() {
    let mut board = board();
    let mut card = linked(&mut board);
    let mut remote = remote("R-1");
    remote.labels = vec!["New label".into()];
    conflict(&mut card, remote.clone());
    let before = card.clone();
    assert!(matches!(
        resolve_conflict(&board, &mut card, ConflictResolution::TakeRemote, LATER),
        Err(BoardError::UnknownLabel(_))
    ));
    assert_eq!(card, before);
    apply_remote(&mut board, &mut card.clone(), &remote, LATER);
    resolve_conflict(&board, &mut card, ConflictResolution::TakeRemote, LATER).unwrap();
    assert_eq!(card.labels[0].as_str(), "new-label");
}
