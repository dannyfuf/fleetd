use std::rc::Rc;

use super::*;

#[test]
fn save_failure_retains_draft_and_old_replies_cannot_clear_new_edits() {
    let mut draft = CardDetailState::default();
    draft.begin(CardEdit::Comment, 3);
    let revision = draft.revision;
    draft.saving = Some(revision);
    draft.finish_save(revision, Some("disk full".into()));
    assert!(draft.is_editing());
    draft.saving = Some(revision);
    draft.revision = draft.revision.wrapping_add(1);
    draft.finish_save(revision, None);
    assert!(draft.is_editing());
    let current = draft.revision;
    draft.saving = Some(current);
    draft.finish_save(revision, None);
    assert!(draft.is_editing());
    draft.finish_save(current, None);
    assert!(!draft.is_editing());
}

#[test]
fn one_buffer_serves_the_three_text_surfaces() {
    let mut draft = CardDetailState::default();
    assert!(!draft.is_editing());
    draft.begin(CardEdit::Title, 3);
    assert_eq!(draft.edit, Some(CardEdit::Title));
    draft.cancel();
    assert!(!draft.is_editing());
}

/// A card carrying one long report and one ordinary comment.
fn reported() -> Card {
    let context = fleet_core::model::Context {
        id: fleet_core::ids::ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = fleet_core::board::new_board(&context, "2026-09-06T12:00:00Z");
    let mut card = fleet_core::board::create_card(
        &mut board,
        &[],
        "card-1".parse().unwrap_or_else(|error| panic!("{error}")),
        fleet_core::board::CardDraft {
            title: "Fix login".into(),
            ..fleet_core::board::CardDraft::default()
        },
        "2026-09-06T12:00:00Z",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let run = fleet_core::agents::DelegationId::new();
    card.runs.push(fleet_core::board::CardRun {
        id: run,
        thread_id: Some(fleet_core::agents::ThreadId::new()),
        status_id: card.status_id.clone(),
        action: fleet_core::board::ActionKind::Prompt,
        provider: fleet_core::agents::AgentKind::Codex,
        model: None,
        effort: None,
        started_at: "2026-09-06T12:00:00Z".to_owned(),
        ended_at: Some("2026-09-06T12:05:00Z".to_owned()),
        outcome: Some(fleet_core::board::RunOutcome::Succeeded),
        detail: None,
        report_comment_id: Some("report-1".to_owned()),
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    });
    card.comments.push(fleet_core::board::Comment {
        id: "report-1".to_owned(),
        author: None,
        body: (0..12)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        created_at: "2026-09-06T12:05:00Z".to_owned(),
        remote_id: None,
        run_id: Some(run),
    });
    card
}

/// `⏎` answers the left pane while a report is folded and the right pane once none is.
#[test]
fn enter_opens_a_folded_report_before_it_edits_a_property() {
    let card = reported();
    let mut draft = CardDetailState::default();
    assert_eq!(
        enter_target(Some(&card), &draft),
        Enter::Unfold(vec!["report-1".to_owned()])
    );
    draft.expanded_reports.insert("report-1".to_owned());
    assert_eq!(
        enter_target(Some(&card), &draft),
        Enter::Property,
        "with the report open the key means what it has always meant"
    );
}

/// The card every board has: no run, no report, and a key that goes straight to the picker.
#[test]
fn a_card_with_no_report_never_takes_the_unfold_branch() {
    let mut card = reported();
    card.comments.clear();
    card.runs.clear();
    assert_eq!(
        enter_target(Some(&card), &CardDetailState::default()),
        Enter::Property
    );
    assert_eq!(
        enter_target(None, &CardDetailState::default()),
        Enter::Property,
        "the card went away while the dialog was open"
    );
}

/// The property column is prepared outside `render`, once per board revision: a frame reads the
/// same rows and chips until the mirror moves, and each chip comes from the key table.
#[gpui::test]
fn the_property_column_is_prepared_once_per_board_revision(cx: &mut gpui::TestAppContext) {
    let context = fleet_core::model::Context {
        id: fleet_core::ids::ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = fleet_core::board::new_board(&context, "2026-09-06T12:00:00Z");
    let card = fleet_core::board::create_card(
        &mut board,
        &[],
        "card-1".parse().unwrap_or_else(|error| panic!("{error}")),
        fleet_core::board::CardDraft {
            title: "Fix login".into(),
            ..fleet_core::board::CardDraft::default()
        },
        "2026-09-06T12:00:00Z",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let card_id = card.id.clone();
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet-card-detail-model", std::time::Instant::now());
        state.board.view = Some(fleet_core::board::BoardView {
            board,
            cards: vec![card],
            live_runs: Vec::new(),
        });
        state
    });

    let model = |cx: &mut App| {
        read_host(&state, cx, |host, _| host.card_detail.properties.clone())
            .unwrap_or_else(|| panic!("the detail prepared no property column"))
    };
    let first = cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.card_detail.card_id = Some(card_id.clone());
            host.card_detail.property_row = 99;
        });
        refresh(&state, cx);
        model(cx)
    });
    assert_eq!(first.rows.len(), first.keys.len());
    let status = first
        .rows
        .iter()
        .position(|row| row.label.as_ref() == "Status")
        .unwrap_or_else(|| panic!("no Status row"));
    let chip = first.keys[status].as_ref().map(|kbd| {
        kbd.strokes()
            .iter()
            .map(|stroke| stroke.unparse())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        chip,
        Some(vec!["s".to_owned()]),
        "Status teaches the board's `s`"
    );
    cx.update(|cx| {
        assert_eq!(
            read_host(&state, cx, |host, _| host.card_detail.property_row),
            first.rows.len() - 1,
            "the row cursor is kept inside the rows where they are prepared"
        );
    });

    let again = cx.update(|cx| {
        refresh(&state, cx);
        model(cx)
    });
    assert!(
        Rc::ptr_eq(&first, &again),
        "nothing moved, nothing is rebuilt"
    );

    let moved = cx.update(|cx| {
        state.update(cx, |state, _| state.board.revision += 1);
        refresh(&state, cx);
        model(cx)
    });
    assert!(
        !Rc::ptr_eq(&first, &moved),
        "a new board revision rebuilds the rows"
    );
}

/// `⏎` on the Pull request row opens the pull request in the browser, as `B` does.
#[gpui::test]
fn enter_on_the_pull_request_row_opens_it_in_the_browser(cx: &mut gpui::TestAppContext) {
    use crate::state::{BoardFocus, Overlay};

    let url = "https://github.com/acme/api/pull/412";
    let mut card = reported();
    card.pull_request = Some(fleet_core::board::PullRequestRef {
        repo: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
        number: 412,
        url: url.to_owned(),
    });
    let context = fleet_core::model::Context {
        id: fleet_core::ids::ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let board = fleet_core::board::new_board(&context, "2026-09-06T12:00:00Z");
    let rows = detail::property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let index = rows
        .iter()
        .position(|row| row.target == PropertyTarget::PullRequest)
        .unwrap_or_else(|| panic!("no Pull request row"));
    let column = board
        .statuses
        .iter()
        .position(|status| status.id == card.status_id)
        .unwrap_or_else(|| panic!("the card landed in no column"));
    let id = card.id.clone();
    let mut app = AppState::new("/tmp/fleet-card-detail-pr", std::time::Instant::now());
    app.board.view = Some(fleet_core::board::BoardView {
        board,
        cards: vec![card],
        live_runs: Vec::new(),
    });
    app.board.focus = BoardFocus { column, row: 0 };
    app.open_overlay(Overlay::Dialog(Dialogs::CardDetail));
    let state = cx.new(|_| app);
    let bridge = Bridge::closed();
    cx.update(|cx| {
        with_host(&state, cx, |host| host.card_detail.card_id = Some(id));
        open_row(&state, &bridge, &rows, index, cx);
    });
    assert_eq!(cx.opened_url().as_deref(), Some(url));
    let selected = cx.update(|cx| with_host(&state, cx, |host| host.card_detail.property_row));
    assert_eq!(selected, index);
}
