use super::*;

#[test]
fn closing_a_detail_picker_returns_to_detail_without_reseeding() {
    let draft = CardPickerState {
        then_detail: true,
        ..Default::default()
    };
    assert_eq!(draft.return_dialog(), Some(Dialogs::CardDetail));
    assert_eq!(CardPickerState::default().return_dialog(), None);
}

#[test]
fn the_repository_picker_offers_only_this_context_s_repositories() {
    let repo = |id: &str, context: &str| fleet_core::model::Repo {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        owner: "acme".into(),
        name: id.into(),
        url: "unused".into(),
        context_id: context.parse().unwrap_or_else(|error| panic!("{error}")),
        default_branch: "main".into(),
        path: "/tmp".into(),
        cloned_at: "2026-09-06T12:00:00Z".into(),
        hooks: fleet_core::model::RepoHooks::default(),
    };
    let repos = [repo("acme/api", "work"), repo("acme/site", "home")];
    let context: fleet_core::ids::ContextId =
        "work".parse().unwrap_or_else(|error| panic!("{error}"));
    let values: Vec<_> = repo_options(&repos, &context)
        .into_iter()
        .map(|option| option.value)
        .collect();
    assert_eq!(values, ["", "acme/api"]);
}

#[test]
fn only_the_open_ended_kinds_accept_a_typed_value() {
    assert!(accepts_free_text(&PickerKind::Assignee, None));
    assert!(accepts_free_text(&PickerKind::Estimate, None));
    assert!(accepts_free_text(&PickerKind::DueDate, None));
    assert!(!accepts_free_text(&PickerKind::Status, None));
    assert!(!accepts_free_text(&PickerKind::Labels, None));
    assert!(accepts_free_text(
        &PickerKind::Property("x".into()),
        Some(PropertyKind::Text)
    ));
    assert!(!accepts_free_text(
        &PickerKind::Property("x".into()),
        Some(PropertyKind::Select)
    ));
}

#[test]
fn every_picker_names_the_card_field_it_writes() {
    // These are the words `SyncState::readonly_fields` uses; a mismatch would silently
    // stop the read-only guard from ever recognising a field.
    assert_eq!(PickerKind::Status.card_field(), Some("status_id"));
    assert_eq!(PickerKind::Priority.card_field(), Some("priority"));
    assert_eq!(PickerKind::Assignee.card_field(), Some("assignee"));
    assert_eq!(PickerKind::Labels.card_field(), Some("labels"));
    assert_eq!(PickerKind::Estimate.card_field(), Some("estimate"));
    assert_eq!(PickerKind::DueDate.card_field(), Some("due_date"));
    // The repository is Fleet's own link, and a custom property carries `editable` itself.
    assert_eq!(PickerKind::Repo.card_field(), None);
    assert_eq!(PickerKind::Property("teams".into()).card_field(), None);
}

#[test]
fn dates_are_validated_as_real_calendar_days() {
    assert!(is_iso_date("2026-09-06"));
    assert!(!is_iso_date("2026-13-06"));
    assert!(!is_iso_date("2026-9-6"));
    assert!(!is_iso_date("tomorrow"));
    // Days per month and leap years, exactly as the daemon counts them: a date the picker
    // accepts and the daemon refuses is a refusal the user could have been spared.
    assert!(!is_iso_date("2026-02-31"));
    assert!(!is_iso_date("2026-04-31"));
    assert!(!is_iso_date("2025-02-29"));
    assert!(is_iso_date("2024-02-29"));
    assert_eq!(free_text_error(&PickerKind::DueDate, "", None), None);
    assert!(free_text_error(&PickerKind::DueDate, "soon", None).is_some());
}

#[test]
fn an_estimate_must_be_a_number() {
    assert_eq!(free_text_error(&PickerKind::Estimate, "8", None), None);
    assert!(free_text_error(&PickerKind::Estimate, "big", None).is_some());
}

#[test]
fn a_status_pick_moves_the_card_and_a_priority_pick_patches_it() {
    let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
    let draft = CardPickerState {
        kind: PickerKind::Status,
        ..CardPickerState::default()
    };
    assert!(matches!(
        request_for(&draft, &card, "in-progress", None),
        Some(RequestBody::MoveCard { .. })
    ));
    let draft = CardPickerState {
        kind: PickerKind::Priority,
        ..CardPickerState::default()
    };
    let Some(RequestBody::UpdateCard { patch, .. }) = request_for(&draft, &card, "urgent", None)
    else {
        panic!("priority must patch the card");
    };
    assert_eq!(patch.priority, Some(Priority::Urgent));
}

#[test]
fn clearing_a_value_sends_some_none_not_nothing() {
    let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
    let draft = CardPickerState {
        kind: PickerKind::Assignee,
        ..CardPickerState::default()
    };
    let Some(RequestBody::UpdateCard { patch, .. }) = request_for(&draft, &card, "", None) else {
        panic!("assignee must patch the card");
    };
    assert_eq!(patch.assignee, Some(None), "`Some(None)` clears the field");
}

#[test]
fn a_multi_select_property_writes_the_whole_toggled_set() {
    let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
    let mut draft = CardPickerState {
        kind: PickerKind::Property("teams".into()),
        ..CardPickerState::default()
    };
    assert!(draft.kind.is_multi_select(Some(PropertyKind::MultiSelect)));
    draft.toggle_value("core");
    draft.toggle_value("ui");
    draft.toggle_value("core");
    draft.toggle_value("");
    assert!(draft.selected.is_empty());
    draft.toggle_value("core");
    draft.toggle_value("ui");
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &card, "ui", Some(PropertyKind::MultiSelect))
    else {
        panic!("a property must patch the card");
    };
    let properties = patch.properties.unwrap_or_else(|| panic!("no properties"));
    assert_eq!(
        properties.get("teams"),
        Some(&PropertyValue::MultiSelect(vec![
            "core".to_owned(),
            "ui".to_owned()
        ]))
    );
}

#[test]
fn an_unparsable_value_produces_no_request_at_all() {
    let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
    let draft = CardPickerState {
        kind: PickerKind::Estimate,
        ..CardPickerState::default()
    };
    assert!(request_for(&draft, &card, "many", None).is_none());
    let draft = CardPickerState {
        kind: PickerKind::DueDate,
        ..CardPickerState::default()
    };
    assert!(request_for(&draft, &card, "someday", None).is_none());
}

/// A card with nothing set, so a test can say what it is about.
fn bare_card() -> fleet_core::board::Card {
    let mut board = fleet_core::board::new_board(
        &fleet_core::model::Context {
            id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "Work".into(),
            owners: vec![],
            created_at: "2026-09-06T12:00:00Z".into(),
        },
        "2026-09-06T12:00:00Z",
    );
    fleet_core::board::create_card(
        &mut board,
        &[],
        "11111111-1111-4111-8111-111111111111"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        fleet_core::board::CardDraft {
            title: "Task".into(),
            ..fleet_core::board::CardDraft::default()
        },
        "2026-09-06T12:00:00Z",
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

/// The cursor opens on the value the card holds. At `0` a picker opened only to look at is
/// a write: `Enter` moves the card to the first column, or sets `Urgent`.
#[test]
fn a_picker_opens_on_the_value_its_card_already_holds() {
    let mut card = bare_card();
    assert_eq!(
        current_value(&card, &PickerKind::Status).as_deref(),
        Some(card.status_id.as_str())
    );
    card.priority = fleet_core::board::Priority::Low;
    assert_eq!(
        current_value(&card, &PickerKind::Priority).as_deref(),
        Some("low"),
        "the priority picker spells its values the way `Priority::ALL` does"
    );
    // An unset optional value is the picker's own clear row, which is always first.
    assert_eq!(
        current_value(&card, &PickerKind::Assignee).as_deref(),
        Some("")
    );
    card.assignee = Some("Ana Rojas".into());
    assert_eq!(
        current_value(&card, &PickerKind::Assignee).as_deref(),
        Some("Ana Rojas")
    );
    card.estimate = Some(3);
    assert_eq!(
        current_value(&card, &PickerKind::Estimate).as_deref(),
        Some("3")
    );
    // Labels are multi-valued and open on `selected` instead.
    assert_eq!(current_value(&card, &PickerKind::Labels), None);
}

/// The priority values the picker offers must be the ones `current_value` answers with, or
/// the cursor lands on row zero and `Enter` writes `Urgent`.
#[test]
fn every_priority_the_picker_offers_is_one_a_card_can_report() {
    let spelled: Vec<String> = fleet_core::board::Priority::ALL
        .iter()
        .map(|priority| format!("{priority:?}").to_lowercase())
        .collect();
    for priority in fleet_core::board::Priority::ALL {
        let mut card = bare_card();
        card.priority = priority;
        let value = current_value(&card, &PickerKind::Priority).unwrap_or_default();
        assert!(spelled.contains(&value), "{value} is not an offered row");
    }
}

/// A two-card board whose cards carry assignees, so the assignee picker has values to offer.
fn board_with_assignees() -> AppState {
    let context = fleet_core::model::Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Work".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = fleet_core::board::new_board(&context, &context.created_at);
    let mut cards: Vec<fleet_core::board::Card> = Vec::new();
    for (index, (title, assignee)) in [("Fix login", "Ana Rojas"), ("Ship the board", "Bo Vang")]
        .into_iter()
        .enumerate()
    {
        let mut card = fleet_core::board::create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            fleet_core::board::CardDraft {
                title: title.to_owned(),
                ..fleet_core::board::CardDraft::default()
            },
            &context.created_at,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        card.assignee = Some(assignee.to_owned());
        cards.push(card);
    }
    let mut state = AppState::new("/tmp/fleet-card-picker", std::time::Instant::now());
    state.board.view = Some(fleet_core::board::BoardView { board, cards });
    state
}

fn labels(rows: &[PickerOption]) -> Vec<&str> {
    rows.iter().map(|option| option.label.as_str()).collect()
}

/// The picker's rows are derived from the board, not from the frame: an assignee list walks
/// every card on the board, so `render` must read rows that a change already prepared
/// (`docs/APP-CONTRACTS.md`, "render prepares nothing").
#[test]
fn candidates_are_prepared_once_per_query() {
    let mut state = board_with_assignees();
    let mut draft = CardPickerState {
        kind: PickerKind::Assignee,
        ..CardPickerState::default()
    };
    assert_eq!(
        labels(&prepare(&state, &mut draft)),
        ["Unassigned", "Ana Rojas", "Bo Vang"]
    );

    // A row no derivation could produce: it survives exactly as long as a second draw returns
    // the rows the draft already holds instead of walking the cards again.
    draft.rows = std::rc::Rc::from(vec![PickerOption::new("sentinel", "sentinel")]);
    assert_eq!(
        labels(&prepare(&state, &mut draft)),
        ["sentinel"],
        "an unchanged draft derived its candidates again"
    );

    // Typing derives them again — an assignee takes a typed value, so the query leads.
    draft.query = "bo".into();
    assert_eq!(labels(&prepare(&state, &mut draft)), ["bo", "Bo Vang"]);

    // and so does a card that changed under the open picker.
    let mut card = state
        .board()
        .unwrap_or_else(|| panic!("no board"))
        .cards
        .first()
        .unwrap_or_else(|| panic!("no card"))
        .clone();
    card.assignee = Some("Bobby Tables".into());
    state.apply_card(card);
    assert_eq!(
        labels(&prepare(&state, &mut draft)),
        ["bo", "Bo Vang", "Bobby Tables"],
        "a card edit under the open picker left the rows it derived before"
    );
}

/// The render path itself: a frame that changed nothing must compose the rows the draft
/// already holds, not walk the board's cards again (`docs/APP-CONTRACTS.md`).
#[gpui::test]
fn a_redraw_composes_the_rows_it_already_prepared(cx: &mut gpui::TestAppContext) {
    use std::{cell::Cell, rc::Rc};

    struct PickerFixture {
        state: Entity<AppState>,
        rows: Rc<Cell<usize>>,
        draws: Rc<Cell<usize>>,
    }

    impl gpui::Render for PickerFixture {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            self.draws.set(self.draws.get() + 1);
            // Exactly what `view::render` reads its list from.
            self.rows.set(prepared(&self.state, cx).len());
            div()
        }
    }

    let state = cx.new(|_| board_with_assignees());
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.card_picker.kind = PickerKind::Assignee;
        });
    });
    let rows = Rc::new(Cell::new(0));
    let draws = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| PickerFixture {
        state: state.clone(),
        rows: rows.clone(),
        draws: draws.clone(),
    });
    cx.run_until_parked();
    assert_eq!(rows.get(), 3, "the first draw prepares the offered values");
    let drawn = draws.get();
    assert!(drawn > 0, "the fixture never drew");

    // A row no derivation could produce, so a redraw that rebuilt the list would drop it.
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.card_picker.rows = std::rc::Rc::from(vec![PickerOption::new("x", "sentinel")]);
        });
    });
    window
        .update(cx, |_, _, cx| cx.notify())
        .unwrap_or_else(|error| panic!("{error}"));
    cx.run_until_parked();
    assert!(draws.get() > drawn, "the window did not draw again");
    assert_eq!(rows.get(), 1, "a redraw derived the candidates again");
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert_eq!(labels(&host.card_picker.rows), ["sentinel"]);
        });
    });
}
