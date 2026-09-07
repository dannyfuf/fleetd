use super::actions::{NO_REMOTE, NO_REMOTE_URL, adjacent_status, picker_target};
use super::*;
use crate::state::BoardFocus;
use fleet_core::board::{BoardView, CardDraft, create_card, new_board};

fn view() -> BoardView {
    let context = fleet_core::model::Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = new_board(&context, "2026-09-06T12:00:00Z");
    let mut cards = Vec::new();
    for (index, title) in ["Fix login", "Ship the board"].iter().enumerate() {
        let card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: (*title).to_owned(),
                ..CardDraft::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    BoardView { board, cards }
}

fn state() -> AppState {
    let mut state = AppState::new("/tmp/fleet-board-screen", Instant::now());
    let view = view();
    let column = view
        .board
        .statuses
        .iter()
        .position(|status| status.id == view.cards[0].status_id)
        .unwrap_or_else(|| panic!("no column"));
    state.board.view = Some(view);
    state.board.focus = BoardFocus { column, row: 0 };
    state
}

#[test]
fn detail_picker_keeps_its_card_when_refresh_moves_it_away_from_selection() {
    let mut state = state();
    let detail_id = selected_card(&state).unwrap().id.clone();
    state.board.view.as_mut().unwrap().cards[0].status_id = "done".parse().unwrap();
    state.clamp_board_focus();
    let selected = selected_card(&state).map(|card| card.id.clone());
    assert_ne!(selected, Some(detail_id.clone()));
    assert_eq!(
        picker_target(selected, true, Some(&detail_id)),
        Some(detail_id)
    );
}

#[test]
fn the_selection_follows_the_filter_not_the_raw_column() {
    let mut state = state();
    assert_eq!(
        selected_card(&state).map(|card| card.title.clone()),
        Some("Fix login".to_owned())
    );
    state.board.filter = "board".to_owned();
    state.clamp_board_focus();
    assert_eq!(
        selected_card(&state).map(|card| card.title.clone()),
        Some("Ship the board".to_owned()),
        "the first visible card is the selected one"
    );
    state.board.filter = "zzz".to_owned();
    state.clamp_board_focus();
    assert!(selected_card(&state).is_none());
}

#[test]
fn adjacent_columns_stop_at_both_ends() {
    let mut state = state();
    state.board.focus.column = 0;
    assert!(adjacent_status(&state, -1).is_none());
    assert!(adjacent_status(&state, 1).is_some());
    let last = state
        .board()
        .unwrap_or_else(|| panic!("no board"))
        .board
        .statuses
        .len()
        - 1;
    state.board.focus.column = last;
    assert!(adjacent_status(&state, 1).is_none());
}

#[test]
fn a_board_without_a_sync_job_is_not_syncing() {
    let state = state();
    assert!(!syncing(&state));
}

#[test]
fn a_read_only_field_is_named_with_the_backends_own_label() {
    let mut state = state();
    let board = &mut state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .board;
    board.backend.kind = "jira".into();
    board.sync.readonly_fields = vec!["priority".into(), "due_date".into()];
    state.apply_backends(vec![fleet_core::board::BackendDescriptor {
        kind: "jira".into(),
        label: "Jira (acli)".into(),
        capabilities: fleet_core::board::BackendCapabilities::default(),
        settings_schema: Vec::new(),
    }]);
    assert_eq!(
        readonly_message(&state, &PickerKind::Priority).as_deref(),
        Some("Priority is read-only on Jira (acli) boards"),
        "the sentence names the field the user pressed a key for, not the wire key"
    );
    assert_eq!(
        readonly_message(&state, &PickerKind::DueDate).as_deref(),
        Some("Due date is read-only on Jira (acli) boards")
    );
    assert_eq!(readonly_message(&state, &PickerKind::Status), None);
    // The repository is Fleet's own link; no backend has an opinion about it.
    assert_eq!(readonly_message(&state, &PickerKind::Repo), None);
}

#[test]
fn a_local_board_refuses_nothing_however_stale_its_list_is() {
    let mut state = state();
    state
        .board
        .view
        .as_mut()
        .unwrap_or_else(|| panic!("no board"))
        .board
        .sync
        .readonly_fields = vec!["priority".into()];
    assert_eq!(readonly_message(&state, &PickerKind::Priority), None);
}

/// Both `x` surfaces answer with the same sentence, and it is not the same sentence for the
/// two cases: the card detail's used to say "no remote issue" about a linked card whose
/// board simply has no address for its backend, naming the wrong fact and hiding the fix.
#[test]
fn the_refusal_x_answers_with_names_which_of_the_two_facts_it_is() {
    let mut card = state()
        .board
        .view
        .as_ref()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0]
        .clone();
    assert_eq!(no_remote_reason(&card), NO_REMOTE);
    card.remote = Some(fleet_core::board::RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "SP-1".into(),
        url: None,
        version: None,
        remote_updated_at: None,
        synced_at: "now".into(),
    });
    assert_eq!(no_remote_reason(&card), NO_REMOTE_URL);
}

#[test]
fn only_a_link_with_an_address_is_openable() {
    let mut card = state()
        .board
        .view
        .as_ref()
        .unwrap_or_else(|| panic!("no board"))
        .cards[0]
        .clone();
    assert_eq!(remote_url(&card), None);
    let mut link = fleet_core::board::RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "SP-1".into(),
        url: None,
        version: None,
        synced_at: "2026-09-06T12:00:00Z".into(),
        remote_updated_at: None,
    };
    card.remote = Some(link.clone());
    assert_eq!(remote_url(&card), None, "a key is not an address");
    link.url = Some("   ".into());
    card.remote = Some(link.clone());
    assert_eq!(remote_url(&card), None, "and neither is blank text");
    link.url = Some(" https://example.test/browse/SP-1 ".into());
    card.remote = Some(link);
    assert_eq!(
        remote_url(&card).as_deref(),
        Some("https://example.test/browse/SP-1")
    );
}
