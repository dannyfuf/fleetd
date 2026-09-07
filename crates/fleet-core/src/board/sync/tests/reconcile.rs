use super::*;

#[test]
fn an_unlinked_card_keeps_the_local_parent_a_pull_knows_nothing_about() {
    let mut board = board();
    let mut parent = card(&mut board, "p");
    let mut child = card(&mut board, "c");
    child.parent_id = Some(parent.id.clone());
    parent.dirty = false;
    child.dirty = false;
    let result = reconcile(
        &board,
        &[parent.clone(), child],
        &PullResult::default(),
        caps(),
        LATER,
    );
    // The remote owns the hierarchy of the cards it linked, and only those: attaching a
    // remote backend to a local board used to flatten every hierarchy on the first pull.
    assert_eq!(result.cards[1].parent_id.as_ref(), Some(&parent.id));
}

#[test]
fn an_unsent_comment_never_becomes_a_conflict_with_nothing_to_choose() {
    let mut board = board();
    let mut card = linked(&mut board);
    add_comment(&mut card, "c-1".into(), None, "Hello".into(), NOW).unwrap();
    card.dirty = true;
    let mut moved = remote("R-1");
    moved.version = Some("2".into());
    let result = reconcile(&board, &[card], &pull(moved), caps(), LATER);
    let synced = &result.cards[0];
    // Nothing differs; the card is dirty only for a comment the backend has not taken.
    // A conflict here has no field to resolve and `plan_push` skips conflicted cards, so
    // the comment could never reach the backend at all.
    assert_eq!(synced.conflict, None);
    assert_eq!(result.summary.conflicts, 0);
    assert!(synced.dirty);
    assert_eq!(
        synced.remote.as_ref().unwrap().version.as_deref(),
        Some("2")
    );
    assert!(
        result
            .to_push
            .iter()
            .any(|op| matches!(op, PushOp::AddComment { .. }))
    );
}

#[test]
fn reconcile_creates_numbered_linked_cards_without_mutating_inputs() {
    let mut board = board();
    board.next_number = 42;
    let pull = pull(remote("R-1"));
    let before_board = board.clone();
    let before_pull = pull.clone();
    let result = reconcile(&board, &[], &pull, caps(), LATER);
    assert_eq!(board, before_board);
    assert_eq!(pull, before_pull);
    assert_eq!(result.cards.len(), 1);
    assert_eq!(result.cards[0].number, 42);
    let uuid = uuid::Uuid::parse_str(result.cards[0].id.as_str()).unwrap();
    assert_eq!(uuid.get_version_num(), 4);
    assert_eq!(result.board.next_number, 43);
    assert_eq!(result.cards[0].remote.as_ref().unwrap().key, "R-1");
    assert!(!result.cards[0].dirty);
    assert_eq!(result.cards[0].activity.len(), 1);
    assert_eq!(result.cards[0].activity[0].kind, ActivityKind::Synced);
    assert_eq!(
        (
            result.summary.pulled,
            result.summary.created,
            result.summary.updated
        ),
        (1, 1, 0)
    );
    assert_eq!(
        reconcile(&board, &[], &pull, caps(), LATER).cards,
        result.cards
    );
}

#[test]
fn reconcile_replay_does_not_duplicate_cards_or_activity() {
    let board = board();
    let pull = pull(remote("R-1"));
    let first = reconcile(&board, &[], &pull, caps(), NOW);
    let second = reconcile(&first.board, &first.cards, &pull, caps(), NOW);
    assert_eq!(first.cards, second.cards);
    assert_eq!(first.board, second.board);
    assert_eq!(
        (
            second.summary.created,
            second.summary.updated,
            second.summary.deleted
        ),
        (0, 0, 0)
    );
}

#[test]
fn version_less_replays_advance_the_clock_without_rewriting_history() {
    let mut board = board();
    let mut card = card(&mut board, "a");
    let mut remote = remote("R-1");
    remote.version = None;
    remote.updated_at = None;
    apply_remote(&mut board, &mut card, &remote, NOW);
    let pull = pull(remote);
    let mut cards = vec![card];
    let history = cards[0].activity.len();
    for now in [LATER, "2026-09-06T14:00:00Z"] {
        let result = reconcile(&board, &cards, &pull, caps(), now);
        // The backend reports neither a version nor a timestamp, so every sync re-applies
        // the same remote: that must not count as an update or grow the activity log.
        assert_eq!(result.summary.updated, 0, "{now}");
        assert_eq!(result.cards[0].activity.len(), history, "{now}");
        assert_eq!(result.cards[0].updated_at, NOW, "{now}");
        board = result.board;
        cards = result.cards;
    }
}

#[test]
fn one_unimportable_remote_card_never_discards_the_rest_of_the_pull() {
    let board = board();
    let mut bad = remote("B-1");
    bad.title = "  ".into();
    let pull = PullResult {
        cards: vec![remote("G-1"), bad],
        ..PullResult::default()
    };
    let result = reconcile(&board, &[], &pull, caps(), NOW);
    assert_eq!(result.summary.created, 1);
    assert_eq!(result.cards.len(), 1);
    assert_eq!(
        result.cards[0]
            .remote
            .as_ref()
            .map(|link| link.key.as_str()),
        Some("G-1")
    );
    assert!(result.board.sync.last_error.is_none());
    assert_eq!(result.summary.skipped.len(), 1);
}

#[test]
fn a_local_comment_never_pins_dirty_on_a_backend_without_comments() {
    let mut board = board();
    let mut card = linked(&mut board);
    add_comment(&mut card, "local".into(), None, "Note".into(), LATER).unwrap();
    card.dirty = true;
    let caps = BackendCapabilities {
        comments: false,
        ..caps()
    };
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps, LATER);
    assert!(!result.cards[0].dirty);
    assert!(result.to_push.is_empty());
}

#[test]
fn local_wins_replay_without_remote_change_markers_is_idempotent() {
    let mut board = board();
    board.settings.conflict_policy = ConflictPolicy::LocalWins;
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    let mut remote = remote("R-1");
    remote.version = None;
    remote.updated_at = None;
    let pull = pull(remote);
    let first = reconcile(&board, &[card], &pull, caps(), LATER);
    let replay = reconcile(&first.board, &first.cards, &pull, caps(), LATER);
    assert_eq!(first.cards, replay.cards);
    assert_eq!(first.to_push, replay.to_push);
}

#[test]
fn reconcile_clean_changed_remote_applies_and_counts_update() {
    let mut board = board();
    let card = linked(&mut board);
    let before = card.clone();
    let mut remote = remote("R-1");
    remote.title = "Remote edit".into();
    remote.version = Some("2".into());
    let result = reconcile(
        &board,
        std::slice::from_ref(&card),
        &pull(remote),
        caps(),
        LATER,
    );
    assert_eq!(card, before);
    assert_eq!(result.cards[0].title, "Remote edit");
    assert_eq!(result.summary.updated, 1);
    assert!(!result.cards[0].dirty);
}

#[test]
fn manual_conflict_records_only_differing_fields_and_blocks_push() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    let mut remote = remote("R-1");
    remote.version = Some("2".into());
    let pull = pull(remote);
    let result = reconcile(&board, &[card], &pull, caps(), LATER);
    assert_eq!(result.cards[0].title, "Local");
    assert_eq!(result.cards[0].conflict.as_ref().unwrap().fields, ["title"]);
    assert!(result.to_push.is_empty());
    assert_eq!(result.summary.conflicts, 1);
    assert_eq!(
        result.cards[0].activity.last().unwrap().kind,
        ActivityKind::ConflictDetected
    );
    let replay = reconcile(&result.board, &result.cards, &pull, caps(), LATER);
    assert_eq!(replay.cards, result.cards);
}

#[test]
fn remote_wins_policy_discards_local_edits_and_conflict() {
    let mut board = board();
    board.settings.conflict_policy = ConflictPolicy::RemoteWins;
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    let mut remote = remote("R-1");
    remote.title = "Remote".into();
    remote.version = Some("2".into());
    let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
    assert_eq!(result.cards[0].title, "Remote");
    assert!(!result.cards[0].dirty);
    assert!(result.cards[0].conflict.is_none());
    assert!(result.to_push.is_empty());
    assert_eq!(result.summary.updated, 1);
}

#[test]
fn local_wins_policy_keeps_local_and_plans_update() {
    let mut board = board();
    board.settings.conflict_policy = ConflictPolicy::LocalWins;
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    let mut remote = remote("R-1");
    remote.version = Some("2".into());
    let result = reconcile(&board, &[card], &pull(remote), caps(), LATER);
    assert_eq!(result.cards[0].title, "Local");
    assert_eq!(result.cards[0].updated_at, LATER);
    assert!(result.cards[0].dirty);
    assert!(result.cards[0].conflict.is_none());
    assert_eq!(
        result.to_push,
        [PushOp::Update {
            card_id: "a".parse().unwrap(),
            fields: vec!["title".into()]
        }]
    );
    assert_eq!(
        result.cards[0].remote.as_ref().unwrap().version.as_deref(),
        Some("2")
    );
}

#[test]
fn local_wins_replayed_over_one_unchanged_pull_logs_one_resolution() {
    let mut board = board();
    board.settings.conflict_policy = ConflictPolicy::LocalWins;
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    // A backend that reports neither a version nor a timestamp is replayed on every sync:
    // `remote_changed` is always true, so every replay reaches the LocalWins arm.
    let mut incoming = remote("R-1");
    incoming.version = None;
    incoming.updated_at = None;
    // Each sync carries its own clock reading, which is what `refresh_link` writes into
    // `synced_at`: comparing the whole link would report a change every single time.
    const LATEST: &str = "2026-09-06T14:00:00Z";
    const LAST: &str = "2026-09-06T15:00:00Z";
    let first = reconcile(&board, &[card], &pull(incoming.clone()), caps(), LATER);
    let entries = first.cards[0].activity.len();
    let second = reconcile(
        &board,
        &first.cards,
        &pull(incoming.clone()),
        caps(),
        LATEST,
    );
    let third = reconcile(&board, &second.cards, &pull(incoming), caps(), LAST);
    assert_eq!(
        third.cards[0].activity.len(),
        entries,
        "one entry per sync evicts the card's real history through the 200-entry cap and \
         rewrites the document every time"
    );
}

#[test]
fn an_archived_card_takes_the_remote_instead_of_hiding_a_conflict() {
    let mut board = board();
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    card.archived = true;
    let mut incoming = remote("R-1");
    incoming.version = Some("2".into());
    incoming.title = "Remote".into();
    let result = reconcile(&board, &[card], &pull(incoming), caps(), LATER);
    assert!(
        result.cards[0].conflict.is_none(),
        "the push pass skips archived cards, so a conflict on one is invisible to \
         `summarize` and to `summary.conflicts` until an un-archive pops it out"
    );
    assert_eq!(result.cards[0].title, "Remote");
    assert!(!result.cards[0].dirty);
    assert_eq!(result.summary.conflicts, 0);
}

#[test]
fn a_remote_null_or_impossible_date_is_never_stored_as_a_property_value() {
    let mut board = board();
    board.properties.push(PropertySchema {
        kind: PropertyKind::Date,
        ..property("eta", PropertySource::Backend)
    });
    board
        .properties
        .push(property("note", PropertySource::Backend));
    let mut card = card(&mut board, "a");
    let mut incoming = remote("R-1");
    incoming
        .properties
        .insert("eta".into(), PropertyValue::Date("2026-02-30".into()));
    incoming
        .properties
        .insert("note".into(), PropertyValue::Null);
    apply_remote(&mut board, &mut card, &incoming, LATER);
    assert!(
        !card.properties.contains_key("note"),
        "`Null` is how every local path spells no value; storing it renders one no local \
         path can produce"
    );
    assert!(
        !card.properties.contains_key("eta"),
        "`validate_card` refuses the date, so storing it skips the card on every pull"
    );
    assert!(validate_card(&board, &card).is_ok());
}

#[test]
fn local_wins_still_takes_remote_comments_and_never_re_sends_them() {
    let mut board = board();
    board.settings.conflict_policy = ConflictPolicy::LocalWins;
    let mut card = linked(&mut board);
    card.title = "Local".into();
    card.dirty = true;
    let mut incoming = remote("R-1");
    incoming.version = Some("2".into());
    incoming.comments = vec![comment("rc-1")];
    let result = reconcile(&board, &[card], &pull(incoming.clone()), caps(), LATER);
    assert_eq!(result.cards[0].title, "Local");
    assert_eq!(result.cards[0].comments.len(), 1);
    assert_eq!(
        result.cards[0].comments[0].remote_id.as_deref(),
        Some("rc-1")
    );
    // An already-remote comment is never queued back to the backend.
    assert_eq!(
        result.to_push,
        [PushOp::Update {
            card_id: "a".parse().unwrap(),
            fields: vec!["title".into()]
        }]
    );
    // The baseline moved, so replaying the same pull must not duplicate the comment.
    let replay = reconcile(&board, &result.cards, &pull(incoming), caps(), LATER);
    assert_eq!(replay.cards[0].comments.len(), 1);
}

#[test]
fn full_pull_archives_missing_link_but_incremental_does_not() {
    let mut board = board();
    let linked = linked(&mut board);
    let local = card(&mut board, "local");
    let cards = [linked, local];
    let incremental = reconcile(&board, &cards, &PullResult::default(), caps(), NOW);
    assert!(!incremental.cards[0].archived);
    assert_eq!(incremental.summary.deleted, 0);
    // The pull reports a deletion of its own, so it is a pull that saw the project — not the
    // wholly empty answer a broken filter gives, which archives nothing.
    let pull = PullResult {
        full: true,
        deleted_keys: vec!["R-9".into()],
        ..PullResult::default()
    };
    let full = reconcile(&board, &cards, &pull, caps(), LATER);
    assert!(full.cards[0].archived);
    assert!(!full.cards[1].archived);
    assert_eq!(full.summary.deleted, 1);
    assert_eq!(full.cards.len(), 2);
    assert_eq!(
        full.cards[0].activity.last().unwrap().kind,
        ActivityKind::Synced
    );
    let replay = reconcile(&full.board, &full.cards, &pull, caps(), LATER);
    assert_eq!(replay.summary.deleted, 0);
    assert_eq!(replay.cards, full.cards);
}

/// A filter that matches nothing answers exactly like a project that emptied itself, and
/// only one of the two is worth archiving a whole board over.
#[test]
fn a_full_pull_that_brought_back_nothing_at_all_archives_nothing() {
    let mut board = board();
    let mut dirty = linked(&mut board);
    dirty.status_id = board.statuses[2].id.clone();
    dirty.dirty = true;
    let cards = [dirty];
    let empty = PullResult {
        full: true,
        ..PullResult::default()
    };
    let result = reconcile(&board, &cards, &empty, caps(), LATER);
    assert!(
        !result.cards[0].archived,
        "an empty full pull is a filter that matched nothing, not an emptied project"
    );
    assert_eq!(result.summary.deleted, 0);
    assert!(
        result.cards[0].dirty,
        "the queued push must survive: archiving would have cleared it for good"
    );
    // The board still has a real emptying to report the moment the backend says so.
    let deleted = PullResult {
        full: true,
        deleted_keys: vec!["R-1".into()],
        ..PullResult::default()
    };
    let gone = reconcile(&board, &cards, &deleted, caps(), LATER);
    assert!(gone.cards[0].archived);
}

/// A key can go missing for a reason that is not a deletion; the archive must be undone
/// when it comes back, or a permission blip costs the board every card it touched.
#[test]
fn a_card_the_sync_archived_comes_back_when_its_remote_does() {
    let mut board = board();
    let card = linked(&mut board);
    let vanished = PullResult {
        full: true,
        deleted_keys: vec!["R-9".into()],
        ..PullResult::default()
    };
    let gone = reconcile(&board, &[card], &vanished, caps(), LATER);
    assert!(gone.cards[0].archived);
    let back = reconcile(
        &gone.board,
        &gone.cards,
        &pull(remote("R-1")),
        caps(),
        LATER,
    );
    assert!(
        !back.cards[0].archived,
        "the remote is back; the card is not"
    );
    assert_eq!(
        back.cards[0].activity.last().unwrap().message,
        RESTORED_BY_SYNC
    );
    // Idempotent: a second pull of the same remote changes nothing about the archive.
    let again = reconcile(
        &back.board,
        &back.cards,
        &pull(remote("R-1")),
        caps(),
        LATER,
    );
    assert!(!again.cards[0].archived);
}

/// The other half of the rule: an archive the user made is theirs, and no pull undoes it.
#[test]
fn a_card_the_user_archived_stays_archived_through_a_pull() {
    let mut board = board();
    let mut card = linked(&mut board);
    apply_card_patch(
        &board,
        &mut card,
        CardPatch {
            archived: Some(true),
            ..CardPatch::default()
        },
        LATER,
    )
    .unwrap();
    card.dirty = false;
    let result = reconcile(&board, &[card], &pull(remote("R-1")), caps(), LATER);
    assert!(result.cards[0].archived);
}

#[test]
fn explicit_deletions_archive_even_dirty_conflicts_and_win_over_pull() {
    let mut board = board();
    let mut card = linked(&mut board);
    conflict(&mut card, remote("R-1"));
    let pull = PullResult {
        cards: vec![remote("R-1")],
        deleted_keys: vec!["R-1".into()],
        ..PullResult::default()
    };
    let result = reconcile(&board, &[card], &pull, caps(), LATER);
    assert!(result.cards[0].archived);
    assert!(!result.cards[0].dirty);
    assert!(result.cards[0].conflict.is_none());
    assert!(result.to_push.is_empty());
    assert_eq!((result.summary.deleted, result.summary.conflicts), (1, 0));
}

/// "Absent from a full pull" means deleted — but a key the backend listed and then could
/// not read is not absent, it is unread. Archiving it takes a live issue off the board and
/// clears the queued push of every field on it.
#[test]
fn a_key_the_pull_could_not_read_is_not_archived_by_a_full_pull() {
    let mut first = board();
    let card = linked(&mut first);
    let pull = PullResult {
        cards: vec![remote("R-2")],
        full: true,
        failed_keys: vec!["R-1: 429 Too Many Requests".into()],
        ..PullResult::default()
    };
    let result = reconcile(&first, &[card], &pull, caps(), LATER);
    let kept = result
        .cards
        .iter()
        .find(|card| card.remote.as_ref().is_some_and(|link| link.key == "R-1"))
        .expect("the unread card is still on the board");
    assert!(!kept.archived);
    assert_eq!(result.summary.deleted, 0);
    // A key that really is gone from the same pull still archives.
    let gone = PullResult {
        cards: vec![remote("R-2")],
        full: true,
        ..PullResult::default()
    };
    let mut second = board();
    let card = linked(&mut second);
    let result = reconcile(&second, &[card], &gone, caps(), LATER);
    assert_eq!(result.summary.deleted, 1);
}

#[test]
fn reconcile_resolves_parent_keys_in_both_orders_and_existing_ids() {
    let mut board = board();
    let parent = linked(&mut board);
    let mut child = remote("A-child");
    child.parent_key = Some("R-1".into());
    let pull = PullResult {
        cards: vec![child, remote("R-1")],
        ..PullResult::default()
    };
    let result = reconcile(&board, &[parent], &pull, caps(), NOW);
    let child = result
        .cards
        .iter()
        .find(|card| card.remote.as_ref().unwrap().key == "A-child")
        .unwrap();
    assert_eq!(child.parent_id.as_ref().unwrap().as_str(), "a");
    let result = reconcile(&board, &[], &pull, caps(), NOW);
    let parent = result
        .cards
        .iter()
        .find(|card| card.remote.as_ref().unwrap().key == "R-1")
        .unwrap();
    let child = result
        .cards
        .iter()
        .find(|card| card.remote.as_ref().unwrap().key == "A-child")
        .unwrap();
    assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
}

#[test]
fn a_parent_arriving_later_links_even_when_the_child_did_not_change() {
    let board = board();
    let mut child = remote("A-child");
    child.parent_key = Some("R-1".into());
    let orphaned = reconcile(&board, &[], &pull(child.clone()), caps(), NOW);
    assert_eq!(orphaned.cards.len(), 1);
    assert_eq!(orphaned.cards[0].parent_id, None);
    // The same unchanged child, now pulled alongside its parent: the relationship
    // must resolve without any remote version bump to trigger an update.
    let both = PullResult {
        cards: vec![child, remote("R-1")],
        ..PullResult::default()
    };
    let result = reconcile(&board, &orphaned.cards, &both, caps(), LATER);
    let parent = result
        .cards
        .iter()
        .find(|card| card.remote.as_ref().unwrap().key == "R-1")
        .unwrap();
    let child = result
        .cards
        .iter()
        .find(|card| card.remote.as_ref().unwrap().key == "A-child")
        .unwrap();
    assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
}

#[test]
fn reconcile_reports_unknown_status_and_preserves_cursor_on_empty_incremental() {
    let mut board = board();
    board.sync.cursor = Some("old".into());
    board.sync.last_error = Some("old error".into());
    let mut remote = remote("R-1");
    remote.status = status("mystery", "Mystery", None);
    let result = reconcile(&board, &[], &pull(remote), caps(), LATER);
    assert_eq!(result.summary.unmapped_statuses, ["mystery"]);
    assert_eq!(result.board.sync.cursor.as_deref(), Some("old"));
    assert_eq!(result.board.sync.last_synced_at.as_deref(), Some(LATER));
    assert!(result.board.sync.last_error.is_none());
    let full = reconcile(
        &board,
        &[],
        &PullResult {
            full: true,
            ..PullResult::default()
        },
        caps(),
        LATER,
    );
    assert!(full.board.sync.cursor.is_none());
}

#[test]
fn reconcile_bad_new_remote_does_not_panic_or_consume_number() {
    let board = board();
    let mut remote = remote("bad");
    remote.title = " ".into();
    let result = reconcile(&board, &[], &pull(remote), caps(), NOW);
    assert!(result.cards.is_empty());
    assert_eq!(result.board.next_number, board.next_number);
    // The pull is reported as partially skipped, never turned into a board-wide failure
    // that would throw away everything else the same pull imported.
    assert!(result.board.sync.last_error.is_none());
    assert_eq!(result.summary.skipped.len(), 1);
    assert!(result.summary.skipped[0].starts_with("bad: "));
}

#[test]
fn an_unimportable_remote_update_is_skipped_and_keeps_every_card_that_reconciled() {
    let mut board = board();
    let mut first = linked(&mut board);
    first.id = "a".parse().unwrap();
    let mut second = card(&mut board, "b");
    apply_remote(&mut board, &mut second, &remote("R-2"), NOW);
    let mut broken = remote("R-1");
    broken.title = "   ".into();
    broken.version = Some("2".into());
    let mut good = remote("R-2");
    good.title = "Renamed".into();
    good.version = Some("2".into());
    let result = reconcile(
        &board,
        &[first.clone(), second],
        &PullResult {
            cards: vec![broken, good],
            ..PullResult::default()
        },
        caps(),
        LATER,
    );
    assert_eq!(result.summary.skipped.len(), 1);
    assert!(result.summary.skipped[0].starts_with("R-1: "));
    // The refused remote leaves its card exactly as it was, and the other one still lands.
    assert_eq!(result.cards[0].title, first.title);
    assert_eq!(result.cards[1].title, "Renamed");
    assert_eq!(result.summary.updated, 1);
    for card in &result.cards {
        validate_card(&result.board, card).unwrap();
    }
}

#[test]
fn an_impossible_remote_due_date_is_skipped_instead_of_wedging_the_pull() {
    let board = board();
    let mut broken = remote("R-9");
    broken.due_date = Some("2026-02-31".into());
    let result = reconcile(&board, &[], &pull(broken), caps(), NOW);
    assert!(result.cards.is_empty());
    assert_eq!(result.summary.created, 0);
    // The number the refused draft took must go back: the next pull creates it for real.
    assert_eq!(result.board.next_number, board.next_number);
    assert_eq!(result.summary.skipped.len(), 1);
    assert!(result.summary.skipped[0].starts_with("R-9: "));
}

#[test]
fn a_remote_parent_key_that_closes_a_cycle_is_dropped() {
    let mut board = board();
    let first = linked(&mut board);
    let mut second = card(&mut board, "b");
    apply_remote(&mut board, &mut second, &remote("R-2"), NOW);
    let mut own = remote("R-1");
    own.parent_key = Some("R-1".into());
    own.version = Some("2".into());
    let result = reconcile(
        &board,
        std::slice::from_ref(&first),
        &pull(own),
        caps(),
        LATER,
    );
    assert_eq!(result.cards[0].parent_id, None);

    let mut one = remote("R-1");
    one.parent_key = Some("R-2".into());
    one.version = Some("2".into());
    let mut two = remote("R-2");
    two.parent_key = Some("R-1".into());
    two.version = Some("2".into());
    let result = reconcile(
        &board,
        &[first, second],
        &PullResult {
            cards: vec![one, two],
            ..PullResult::default()
        },
        caps(),
        LATER,
    );
    assert!(result.cards.iter().all(|card| card.parent_id.is_none()));
}
