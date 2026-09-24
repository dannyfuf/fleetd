use super::*;
use fleet_core::model::Context;
use fleet_proto::job::JobKind;

fn one_context_snapshot(active: bool) -> fleet_proto::snapshot::Snapshot {
    let id: fleet_core::ids::ContextId = "acme".parse().unwrap_or_else(|error| panic!("{error}"));
    fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-04T12:00:00Z".to_owned(),
        revision: None,
        contexts: vec![Context {
            id: id.clone(),
            name: "acme".to_owned(),
            owners: vec!["acme".to_owned()],
            created_at: "2026-09-04T09:00:00Z".to_owned(),
        }],
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: active.then_some(id),
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "0.1.0".to_owned(),
            pid: 1,
            started_at: "2026-09-04T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    }
}

#[test]
fn the_breadcrumb_names_the_context_the_switcher_shows() {
    let now = std::time::Instant::now();
    for active in [true, false] {
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_snapshot(one_context_snapshot(active), now);
        state.breadcrumb_row = Some("feature-one".to_owned());
        state.scope = RepoScope::Repo(
            "acme/widgets"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        );
        assert_eq!(
            breadcrumb_text(&state),
            "acme \u{203a} widgets \u{203a} feature-one",
            "§2.2 wants `context › repo › row`; an unset `active_context` is still the \
             first context, which is what the switcher names (active_context set: {active})"
        );
    }
}

#[test]
fn the_board_breadcrumb_names_the_card_that_is_selected_right_now() {
    let now = std::time::Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_snapshot(one_context_snapshot(true), now);
    state.screen = Screen::Hub { tab: HubTab::Board };
    let context = Context {
        id: "acme".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "acme".to_owned(),
        owners: Vec::new(),
        created_at: "2026-09-04T09:00:00Z".to_owned(),
    };
    let mut board = fleet_core::board::new_board(&context, &context.created_at);
    let mut cards = Vec::new();
    for (index, title) in ["First", "Second"].iter().enumerate() {
        let card = fleet_core::board::create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            fleet_core::board::CardDraft {
                title: (*title).to_owned(),
                ..Default::default()
            },
            &context.created_at,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    let column = board
        .statuses
        .iter()
        .position(|status| status.id == cards[0].status_id)
        .unwrap_or_else(|| panic!("no column"));
    let keys: Vec<String> = cards.iter().map(|card| card.display_key(&board)).collect();
    state.board.view = Some(fleet_core::board::BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    });
    state.board.focus = crate::state::BoardFocus { column, row: 0 };
    // Stale by construction: the status bar is prepared before the body renders, so a row the
    // board's render caches always names the previously selected card.
    state.breadcrumb_row = Some("stale".to_owned());
    assert!(breadcrumb_text(&state).ends_with(&keys[0]), "{keys:?}");
    state.board.focus.row = 1;
    assert!(breadcrumb_text(&state).ends_with(&keys[1]), "{keys:?}");

    // A dialog about the card keeps naming it; the one about the board does not, because
    // the card is the one thing board settings cannot change.
    state.open_overlay(Overlay::Dialog(Dialogs::CardDetail));
    assert!(breadcrumb_text(&state).ends_with(&keys[1]), "{keys:?}");
    state.close_overlay();
    state.open_overlay(Overlay::Dialog(Dialogs::BoardSettings));
    assert_eq!(
        breadcrumb_text(&state),
        "acme",
        "board settings edits the board, so the breadcrumb names no card"
    );
    state.close_overlay();
    assert!(breadcrumb_text(&state).ends_with(&keys[1]), "{keys:?}");
}

#[test]
fn job_kind_labels_are_single_words() {
    for kind in [
        JobKind::Clone,
        JobKind::PoolBuild,
        JobKind::CreateWorktree,
        JobKind::PrFetch,
        JobKind::Custom("thing".to_owned()),
    ] {
        let label = crate::presentation::job_kind_label(&kind);
        assert!(!label.is_empty());
        assert!(!label.contains(' '));
    }
}
