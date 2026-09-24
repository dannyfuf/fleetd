use super::*;

#[test]
fn worktree_slug_uses_crate_slugify_and_unicode_is_safe() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    card.title = "Fix_API.v2 / 中文 héllo".into();
    assert_eq!(worktree_slug(&board, &card), "fle-1-fix_api-v2-h-llo");
    board.settings.branch_template = "Feature/{slug}/{key}".into();
    assert_eq!(
        worktree_slug(&board, &card),
        "feature-fix_api-v2-h-llo-fle-1"
    );
    board.settings.branch_template = "{slug}".into();
    card.title = "中".repeat(80);
    assert_eq!(worktree_slug(&board, &card), "fle-1");
}

#[test]
fn worktree_slug_truncates_and_trims_dash_at_boundary() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    board.settings.branch_template = "{slug}".into();
    card.title = format!("{} tail", "x".repeat(47));
    assert_eq!(worktree_slug(&board, &card), "x".repeat(47));
    card.title = "x".repeat(100);
    assert_eq!(worktree_slug(&board, &card).len(), 48);
}

#[test]
fn worktree_slug_is_always_a_valid_branch_name() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    for title in [
        "Fix the login bug.",
        "release v1.lock",
        "a..b regression",
        "...",
        "____",
        ".hidden",
        "Ship it!!!",
    ] {
        card.title = title.into();
        for template in ["{key}-{slug}", "{slug}", "feature/{slug}"] {
            board.settings.branch_template = template.into();
            let slug = worktree_slug(&board, &card);
            assert!(!slug.is_empty(), "{title} / {template}");
            crate::validate::validate_branch(&slug)
                .unwrap_or_else(|error| panic!("{title} / {template}: {error}"));
            crate::validate::validate_slug(&slug)
                .unwrap_or_else(|error| panic!("{title} / {template}: {error}"));
        }
    }
}

#[test]
fn summary_excludes_archived_and_both_terminal_categories() {
    let mut board = board();
    let mut cards: Vec<_> = (0..5)
        .map(|i| create(&mut board, &[], &format!("c{i}")))
        .collect();
    cards[1].status_id = "in-progress".parse().unwrap();
    cards[2].status_id = "done".parse().unwrap();
    cards[3].status_id = "canceled".parse().unwrap();
    cards[4].archived = true;
    cards[4].dirty = true;
    cards[0].dirty = true;
    cards[0].conflict = Some(Conflict {
        detected_at: NOW.into(),
        remote: Default::default(),
        fields: vec![],
    });
    let summary = summarize(&board, &cards, &[], NOW);
    assert_eq!(
        (
            summary.card_count,
            summary.open_count,
            summary.dirty_count,
            summary.conflict_count
        ),
        (4, 2, 1, 1)
    );
}

/// `idle_started` counts the cards standing in a started column with nothing running and nothing
/// owed, and leaves out what `attention_count` already counts, so the two add up.
#[test]
fn summary_counts_idle_started_cards_apart_from_attention() {
    let mut board = board();
    let mut cards: Vec<_> = (0..4)
        .map(|i| create(&mut board, &[], &format!("c{i}")))
        .collect();
    let started: StatusId = "in-progress".parse().unwrap();
    for card in &mut cards[..3] {
        card.status_id = started.clone();
    }
    // Owed a run, and waiting long enough to need a person: working and attention, never idle.
    cards[1].pending_run = Some(crate::board::model::PendingRun {
        status_id: started.clone(),
        since: "2026-09-06T11:00:00Z".into(),
    });
    // Owed a run a moment ago: working only.
    cards[2].pending_run = Some(crate::board::model::PendingRun {
        status_id: started,
        since: NOW.into(),
    });
    // cards[3] is idle, but in an unstarted column.
    let summary = summarize(&board, &cards, &[], NOW);
    assert_eq!(
        (
            summary.working_count,
            summary.attention_count,
            summary.idle_started
        ),
        (2, 1, 1)
    );
}
