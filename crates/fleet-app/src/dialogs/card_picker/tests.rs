use super::*;

#[gpui::test]
fn query_rejects_spaces_while_space_remains_the_picker_toggle(cx: &mut gpui::TestAppContext) {
    use std::{cell::Cell, rc::Rc};

    struct PickerInputHarness {
        input: Entity<TextInput>,
        toggles: Rc<Cell<usize>>,
    }

    impl gpui::Render for PickerInputHarness {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let toggles = self.toggles.clone();
            div().key_context("Dialog").child(
                div()
                    .key_context("CardPicker")
                    .on_action(move |_: &settings_actions::Toggle, _, cx| {
                        toggles.set(toggles.get() + 1);
                        cx.stop_propagation();
                    })
                    .child(self.input.clone()),
            )
        }
    }

    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_filter(Some(|character| character != ' '), cx);
        input
    });
    let toggles = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| PickerInputHarness {
        input: input.clone(),
        toggles: toggles.clone(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
    visual.simulate_input("a b");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "ab"));
    visual.simulate_keystrokes("space");
    input.read_with(&visual, |input, _| assert_eq!(input.text(), "ab"));
    assert_eq!(toggles.get(), 1);
}

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
        request_for(&draft, &card, "in-progress", None, None),
        Some(RequestBody::MoveCard { .. })
    ));
    let draft = CardPickerState {
        kind: PickerKind::Priority,
        ..CardPickerState::default()
    };
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &card, "urgent", None, None)
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
    let Some(RequestBody::UpdateCard { patch, .. }) = request_for(&draft, &card, "", None, None)
    else {
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
        request_for(&draft, &card, "ui", Some(PropertyKind::MultiSelect), None)
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
    assert!(request_for(&draft, &card, "many", None, None).is_none());
    let draft = CardPickerState {
        kind: PickerKind::DueDate,
        ..CardPickerState::default()
    };
    assert!(request_for(&draft, &card, "someday", None, None).is_none());
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
    state.board.view = Some(fleet_core::board::BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    });
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
        labels(&prepare(&state, &mut draft, "")),
        ["Unassigned", "Ana Rojas", "Bo Vang"]
    );

    // A row no derivation could produce: it survives exactly as long as a second draw returns
    // the rows the draft already holds instead of walking the cards again.
    draft.rows = std::rc::Rc::from(vec![PickerOption::new("sentinel", "sentinel")]);
    assert_eq!(
        labels(&prepare(&state, &mut draft, "")),
        ["sentinel"],
        "an unchanged draft derived its candidates again"
    );

    // Typing derives them again — an assignee takes a typed value, so the query leads.
    assert_eq!(
        labels(&prepare(&state, &mut draft, "bo")),
        ["bo", "Bo Vang"]
    );

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
        labels(&prepare(&state, &mut draft, "bo")),
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
        let mut draft = CardPickerState {
            kind: PickerKind::Assignee,
            ..Default::default()
        };
        prepare(state.read(cx), &mut draft, "");
        with_host(&state, cx, |host| host.card_picker = draft);
    });
    let rows = Rc::new(Cell::new(0));
    let draws = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| PickerFixture {
        state: state.clone(),
        rows: rows.clone(),
        draws: draws.clone(),
    });
    cx.run_until_parked();
    assert_eq!(
        rows.get(),
        3,
        "the draw did not compose the prepared values"
    );
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

/// A five-card board whose links make some candidates cycles: `1 → 2 → 3` wait on each other,
/// `4` is archived and `5` is free. (`n` is the card's key, `cards[n - 1]` its index.)
fn board_with_links() -> AppState {
    let context = fleet_core::model::Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Work".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = fleet_core::board::new_board(&context, &context.created_at);
    let mut cards: Vec<fleet_core::board::Card> = Vec::new();
    for (index, title) in ["One", "Two", "Three", "Four", "Five"]
        .into_iter()
        .enumerate()
    {
        let card = fleet_core::board::create_card(
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
        cards.push(card);
    }
    let first = cards[0].id.clone();
    let second = cards[1].id.clone();
    cards[1].blocked_by = vec![first];
    cards[2].blocked_by = vec![second];
    cards[3].archived = true;
    let mut state = AppState::new("/tmp/fleet-card-picker-links", std::time::Instant::now());
    state.board.view = Some(fleet_core::board::BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    });
    state
}

/// The cards of [`board_with_links`], in creation order.
fn linked_cards(state: &AppState) -> Vec<fleet_core::board::Card> {
    state
        .board()
        .unwrap_or_else(|| panic!("no board"))
        .cards
        .clone()
}

#[test]
fn the_five_workflow_kinds_name_themselves_and_their_fields() {
    assert_eq!(PickerKind::BlockedBy.label(), "Blocked by");
    assert_eq!(PickerKind::Blocks.label(), "Blocks");
    assert_eq!(PickerKind::Provider.label(), "Provider");
    assert_eq!(PickerKind::Model.label(), "Model");
    assert_eq!(PickerKind::Effort.label(), "Effort");
    // Both ends of a link write the same field, and the three agent rows write one block.
    assert_eq!(PickerKind::BlockedBy.card_field(), Some("blocked_by"));
    assert_eq!(PickerKind::Blocks.card_field(), Some("blocked_by"));
    assert_eq!(PickerKind::Provider.card_field(), Some("agent"));
    assert_eq!(PickerKind::Model.card_field(), Some("agent"));
    assert_eq!(PickerKind::Effort.card_field(), Some("agent"));
    // Only the two link kinds toggle; the agent rows set one value and close.
    assert!(PickerKind::BlockedBy.is_multi_select(None));
    assert!(PickerKind::Blocks.is_multi_select(None));
    assert!(!PickerKind::Provider.is_multi_select(None));
    assert!(!PickerKind::Model.is_multi_select(None));
    assert!(!PickerKind::Effort.is_multi_select(None));
    // A model or an effort the app has never seen declared still has to be settable; the
    // provider is a closed set of two.
    assert!(accepts_free_text(&PickerKind::Model, None));
    assert!(accepts_free_text(&PickerKind::Effort, None));
    assert!(!accepts_free_text(&PickerKind::Provider, None));
    assert!(!accepts_free_text(&PickerKind::BlockedBy, None));
    assert_eq!(free_text_error(&PickerKind::Effort, "ultra", None), None);
}

#[test]
fn the_blocked_by_picker_lists_the_live_cards_and_disables_a_cycle() {
    let state = board_with_links();
    let cards = linked_cards(&state);
    let rows = options(&state, &PickerKind::BlockedBy, Some(&cards[0].id));
    let row = |card: &fleet_core::board::Card| {
        rows.iter()
            .find(|row| row.value == card.id.as_str())
            .unwrap_or_else(|| panic!("{} is not offered", card.title))
            .clone()
    };
    assert_eq!(
        rows[0].label, "No blockers",
        "the set needs a row that empties it"
    );
    assert!(
        !rows.iter().any(|row| row.value == cards[0].id.as_str()),
        "a card cannot block itself"
    );
    assert!(
        !rows.iter().any(|row| row.value == cards[3].id.as_str()),
        "an archived card is not a blocker anyone can pick"
    );
    // `Two` already waits for `One`, and `Three` waits for it through `Two`: either would
    // close the loop, so both are listed with the reason and neither can be taken.
    for card in [&cards[1], &cards[2]] {
        let row = row(card);
        assert!(row.disabled, "{} would close a cycle", card.title);
        assert_eq!(row.detail.as_deref(), Some("would cycle"));
    }
    let free = row(&cards[4]);
    assert!(!free.disabled);
    assert!(
        free.label.contains("Five"),
        "the row reads its key and title"
    );
    assert_ne!(free.detail.as_deref(), Some("would cycle"));
}

#[test]
fn the_blocks_picker_disables_the_same_cycle_from_the_other_end() {
    let state = board_with_links();
    let cards = linked_cards(&state);
    let rows = options(&state, &PickerKind::Blocks, Some(&cards[2].id));
    let disabled: Vec<&str> = rows
        .iter()
        .filter(|row| row.disabled)
        .map(|row| row.value.as_str())
        .collect();
    // `Three` already waits for `One` through `Two`, so making it block either of them is the
    // same loop seen from the dependant's end.
    assert_eq!(disabled, [cards[0].id.as_str(), cards[1].id.as_str()]);
    assert_eq!(rows[0].label, "Blocks nothing");
}

#[test]
fn the_provider_picker_offers_the_column_default_and_the_two_harnesses() {
    let state = board_with_links();
    let cards = linked_cards(&state);
    let rows = options(&state, &PickerKind::Provider, Some(&cards[0].id));
    let values: Vec<&str> = rows.iter().map(|row| row.value.as_str()).collect();
    assert_eq!(values, ["", "claude", "codex"]);
    assert_eq!(rows[0].label, "column default");
}

#[test]
fn a_model_or_effort_nobody_declared_is_still_the_typed_row() {
    let state = board_with_links();
    let cards = linked_cards(&state);
    // No thread is open, so no harness has declared a vocabulary: the column default is the
    // only offered row and the typed value leads, exactly as an assignee does.
    let mut draft = CardPickerState {
        kind: PickerKind::Effort,
        card_id: Some(cards[0].id.clone()),
        ..CardPickerState::default()
    };
    assert_eq!(labels(&prepare(&state, &mut draft, "")), ["column default"]);
    assert_eq!(labels(&prepare(&state, &mut draft, "ultra")), ["ultra"]);
    let mut draft = CardPickerState {
        kind: PickerKind::Model,
        card_id: Some(cards[0].id.clone()),
        ..CardPickerState::default()
    };
    assert_eq!(labels(&prepare(&state, &mut draft, "")), ["column default"]);
}

#[test]
fn an_agent_picker_opens_on_the_card_s_own_choice_not_its_column_s() {
    let mut card = bare_card();
    // Nothing set opens on `column default`, which is the row that clears the field.
    assert_eq!(
        current_value(&card, &PickerKind::Provider).as_deref(),
        Some("")
    );
    assert_eq!(
        current_value(&card, &PickerKind::Model).as_deref(),
        Some("")
    );
    assert_eq!(
        current_value(&card, &PickerKind::Effort).as_deref(),
        Some("")
    );
    card.agent = Some(fleet_core::board::CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: Some("gpt-5.6-sol".into()),
        effort: Some("high".into()),
    });
    assert_eq!(
        current_value(&card, &PickerKind::Provider).as_deref(),
        Some("codex"),
        "the provider picker spells its rows the way the harness is named"
    );
    assert_eq!(
        current_value(&card, &PickerKind::Model).as_deref(),
        Some("gpt-5.6-sol")
    );
    assert_eq!(
        current_value(&card, &PickerKind::Effort).as_deref(),
        Some("high")
    );
    // The link kinds are multi-valued and open on `selected`, as labels do.
    assert_eq!(current_value(&card, &PickerKind::BlockedBy), None);
    assert_eq!(current_value(&card, &PickerKind::Blocks), None);
}

#[test]
fn the_blocked_by_picker_sends_the_whole_set_for_this_card() {
    let state = board_with_links();
    let cards = linked_cards(&state);
    let mut draft = CardPickerState {
        kind: PickerKind::BlockedBy,
        card_id: Some(cards[2].id.clone()),
        ..CardPickerState::default()
    };
    draft.toggle_value(cards[0].id.as_str());
    draft.toggle_value(cards[4].id.as_str());
    let Some(RequestBody::UpdateCard { card_id, patch }) =
        request_for(&draft, &cards[2].id, "", None, None)
    else {
        panic!("a blocker set must patch the card it was opened on");
    };
    assert_eq!(card_id, cards[2].id);
    assert_eq!(
        patch.blocked_by,
        Some(vec![cards[0].id.clone(), cards[4].id.clone()])
    );
    // Emptying the set clears the links rather than sending nothing at all.
    draft.toggle_value("");
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &cards[2].id, "", None, None)
    else {
        panic!("an emptied set must still patch the card");
    };
    assert_eq!(patch.blocked_by, Some(Vec::new()));
}

#[test]
fn the_blocks_picker_patches_only_the_dependants_whose_membership_changed() {
    let state = board_with_links();
    let cards = linked_cards(&state);
    // `One` blocks `Two` today; the picker is applied with `Five` in its place.
    let updates = blocks_updates(&state, &cards[0], &[cards[4].id.as_str().to_owned()]);
    let changed: Vec<&CardId> = updates.iter().map(|(card, _)| card).collect();
    assert_eq!(
        changed,
        [&cards[1].id, &cards[4].id],
        "one request per changed dependant, in key order"
    );
    assert!(
        updates[0].1.is_empty(),
        "the dependant that was dropped loses this card"
    );
    assert_eq!(updates[1].1, vec![cards[0].id.clone()]);
    // `Three` waits for `Two`, not for `One`: a card nobody touched is not patched.
    assert!(
        !updates.iter().any(|(card, _)| card == &cards[2].id),
        "a card whose membership did not change is left alone"
    );
    // And an unchanged set sends nothing at all.
    assert!(blocks_updates(&state, &cards[0], &[cards[1].id.as_str().to_owned()]).is_empty());
    // `Blocks` never patches the card it was opened on.
    let draft = CardPickerState {
        kind: PickerKind::Blocks,
        card_id: Some(cards[0].id.clone()),
        ..CardPickerState::default()
    };
    assert!(request_for(&draft, &cards[0].id, "", None, None).is_none());
}

#[test]
fn an_agent_pick_replaces_one_field_of_the_whole_block() {
    let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
    let prefs = fleet_core::board::CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: Some("gpt-5.6-sol".into()),
        effort: None,
    };
    let draft = CardPickerState {
        kind: PickerKind::Effort,
        ..CardPickerState::default()
    };
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &card, "ultra", None, Some(&prefs))
    else {
        panic!("an effort must patch the card");
    };
    assert_eq!(
        patch.agent,
        Some(Some(fleet_core::board::CardAgentPrefs {
            provider: Some(AgentKind::Codex),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("ultra".into()),
        })),
        "a typed effort is applied verbatim and the other two fields ride along"
    );
    // A provider the picker never offered is not a value: no request goes out at all.
    let draft = CardPickerState {
        kind: PickerKind::Provider,
        ..CardPickerState::default()
    };
    assert!(request_for(&draft, &card, "gemini", None, Some(&prefs)).is_none());
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &card, "claude", None, Some(&prefs))
    else {
        panic!("a provider must patch the card");
    };
    assert_eq!(
        patch.agent.flatten().and_then(|prefs| prefs.provider),
        Some(AgentKind::Claude)
    );
}

#[test]
fn column_default_clears_one_field_and_the_last_one_clears_the_block() {
    let card: CardId = "card-1".parse().unwrap_or_else(|error| panic!("{error}"));
    let prefs = fleet_core::board::CardAgentPrefs {
        provider: Some(AgentKind::Codex),
        model: Some("gpt-5.6-sol".into()),
        effort: None,
    };
    let draft = CardPickerState {
        kind: PickerKind::Model,
        ..CardPickerState::default()
    };
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &card, "", None, Some(&prefs))
    else {
        panic!("clearing a model must patch the card");
    };
    assert_eq!(
        patch.agent,
        Some(Some(fleet_core::board::CardAgentPrefs {
            provider: Some(AgentKind::Codex),
            model: None,
            effort: None,
        })),
        "`column default` clears its own field and leaves the rest of the block"
    );
    // The last field back on the column's default takes the override off the card entirely.
    let last = fleet_core::board::CardAgentPrefs {
        model: Some("gpt-5.6-sol".into()),
        ..fleet_core::board::CardAgentPrefs::default()
    };
    let Some(RequestBody::UpdateCard { patch, .. }) =
        request_for(&draft, &card, "", None, Some(&last))
    else {
        panic!("clearing the last field must patch the card");
    };
    assert_eq!(patch.agent, Some(None), "`Some(None)` clears the block");
}

/// A listed row that cannot be taken owns its keys: `space` consumes the toggle without
/// changing the set, so the dialog stays open with the reason on the row (contracts §5.3).
#[gpui::test]
fn a_cycle_row_is_listed_but_refuses_the_toggle(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| board_with_links());
    cx.update(|cx| {
        let cards = linked_cards(state.read(cx));
        let mut draft = CardPickerState {
            kind: PickerKind::BlockedBy,
            card_id: Some(cards[0].id.clone()),
            ..CardPickerState::default()
        };
        let rows = prepare(state.read(cx), &mut draft, "");
        let at = |value: &str| {
            rows.iter()
                .position(|row| row.value == value)
                .unwrap_or_else(|| panic!("{value} is not offered"))
        };
        draft.cursor = at(cards[1].id.as_str());
        assert!(rows[draft.cursor].disabled);
        with_host(&state, cx, |host| host.card_picker = draft);
        toggle(&state, cx);
        with_host(&state, cx, |host| {
            assert!(
                host.card_picker.selected.is_empty(),
                "a cycle row must not join the set"
            );
            host.card_picker.cursor = at(cards[4].id.as_str());
        });
        toggle(&state, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.card_picker.selected, [cards[4].id.as_str().to_owned()]);
        });
    });
}

/// `docs/TESTING-HARNESS.md` §3: the picker reports its query as `dialog.field[0]`.
///
/// The query is the one place a typed value lives — an effort or a model no catalogue offered
/// is in the field and in no list — and `seed` puts the card's own value there when no row
/// carries it, so this field is how a scenario reads back what a card now holds. Nothing else
/// about this dialog is projected: the rows belong to the host entity, not to `AppState`.
#[gpui::test]
fn the_harness_reads_the_picker_query_as_its_only_dialog_field(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/card-picker-query", std::time::Instant::now()));
    cx.update(|cx| {
        let input = cx.new(|cx| TextInput::new(InputMode::SingleLine, cx));
        input.update(cx, |input, cx| input.set_text("blistering", cx));
        with_host(&state, cx, |host| {
            host.card_picker_input = Some(input);
        });
        state.update(cx, |app, _| {
            app.overlay = Some(crate::state::Overlay::Dialog(Dialogs::CardPicker));
        });
        let fields = crate::dialogs::dialog_fields(&state, cx);
        assert_eq!(
            fields.len(),
            1,
            "the query is this dialog's whole tab cycle, and `dialog.field[0]` is painted on it"
        );
        assert_eq!(fields[0].name, "query");
        assert_eq!(fields[0].value, "blistering");
    });
}
