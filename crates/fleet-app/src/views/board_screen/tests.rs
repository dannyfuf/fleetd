use super::model::{card_extras, label_chips};
use super::*;
use fleet_core::{
    board::{CardDraft, Label, create_card, new_board},
    ids::{ContextId, LabelId},
    model::Context,
};

fn context() -> Context {
    Context {
        id: ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    }
}

fn view() -> BoardView {
    let mut board = new_board(&context(), "2026-09-06T12:00:00Z");
    board.labels.push(Label {
        id: LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}")),
        name: "Bug".into(),
        color: Some("danger".into()),
    });
    let mut cards = Vec::new();
    for (index, title) in ["Fix login", "Ship the board", "Polish"].iter().enumerate() {
        let card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: (*title).to_owned(),
                assignee: (index == 1).then(|| "Danny".to_owned()),
                labels: if index == 0 {
                    vec![LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}"))]
                } else {
                    Vec::new()
                },
                ..CardDraft::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    }
}

#[test]
fn the_header_counts_only_the_cards_the_board_shows() {
    let mut view = view();
    view.cards[0].dirty = true;
    view.cards[1].dirty = true;
    view.cards[1].archived = true;
    view.cards[2].archived = true;
    view.cards[2].conflict = Some(fleet_core::board::Conflict {
        detected_at: "2026-09-06T12:00:00Z".into(),
        remote: fleet_core::board::RemoteCard::default(),
        fields: vec!["title".into()],
    });
    let facts = HeaderFacts::of(&view, None, 0, &BoardMarks::default());
    // An archived card is in no column and reachable by no key: a chip for one is a chip
    // pointing at nothing.
    assert_eq!((facts.dirty, facts.conflicts), (1, 0));
}

#[test]
fn an_emptied_property_is_no_chip_at_all() {
    let mut view = view();
    view.board
        .properties
        .push(fleet_core::board::PropertySchema {
            key: "teams".into(),
            name: "Teams".into(),
            kind: fleet_core::board::PropertyKind::MultiSelect,
            options: vec![],
            editable: true,
            source: fleet_core::board::PropertySource::Local,
            show_on_card: true,
        });
    view.cards[0].properties.insert(
        "teams".into(),
        fleet_core::board::PropertyValue::MultiSelect(vec![]),
    );
    assert!(card_extras(&view.board, &view.cards[0]).is_empty());
    view.cards[0].properties.insert(
        "teams".into(),
        fleet_core::board::PropertyValue::MultiSelect(vec!["core".into()]),
    );
    assert_eq!(
        card_extras(&view.board, &view.cards[0]),
        vec![SharedString::from("Teams: core")]
    );
}

#[test]
fn the_filter_matches_title_key_label_and_assignee() {
    let mut view = view();
    let status = view.board.statuses[0].id.clone();
    for card in &mut view.cards {
        card.status_id = status.clone();
    }
    let shown = |query: &str| {
        visible_cards(&view, &status, query)
            .iter()
            .map(|card| card.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(shown("").len(), 3, "an empty filter hides nothing");
    assert_eq!(shown("  LOGIN  "), ["Fix login"], "trimmed and case-folded");
    assert_eq!(shown("bug"), ["Fix login"], "by label name");
    assert_eq!(shown("danny"), ["Ship the board"], "by assignee");
    let key = view.cards[0].display_key(&view.board).to_lowercase();
    assert_eq!(shown(&key), ["Fix login"], "by display key");
    assert!(shown("zzz").is_empty());
}

#[test]
fn a_column_shows_only_its_unarchived_matching_cards() {
    let mut view = view();
    let status = view.board.statuses[1].id.clone();
    for card in &mut view.cards {
        card.status_id = status.clone();
    }
    assert_eq!(visible_cards(&view, &status, "").len(), 3);
    assert_eq!(visible_cards(&view, &status, "polish").len(), 1);
    view.cards[0].archived = true;
    assert_eq!(visible_cards(&view, &status, "").len(), 2);
    assert_eq!(counts(&view, ""), (2, 2));
}

#[test]
fn counts_report_the_whole_board_not_one_column() {
    let view = view();
    assert_eq!(counts(&view, ""), (3, 3));
    assert_eq!(counts(&view, "polish"), (1, 3));
}

#[test]
fn priorities_map_onto_the_kits_levels() {
    assert_eq!(priority_level(Priority::Urgent), PriorityLevel::Urgent);
    assert_eq!(priority_level(Priority::None), PriorityLevel::None);
}

#[test]
fn header_facts_count_dirty_and_conflicted_cards() {
    let mut view = view();
    view.cards[0].dirty = true;
    view.board.sync.last_synced_at = Some("2026-09-06T11:00:00Z".into());
    let facts = HeaderFacts::of(&view, Some("Local"), 1_788_523_200, &BoardMarks::default());
    assert_eq!(facts.dirty, 1);
    assert_eq!(facts.conflicts, 0);
    assert!(facts.local);
    assert!(facts.synced.is_some());
}

/// The marks the app folded reach the rows that draw them, and nothing re-derives them.
///
/// `CardRow` is what the tile is built from in `fn tile`, so a mark that stops here is a mark
/// the board never shows. A card the fold had nothing to say about carries neither.
#[test]
fn a_cards_marks_reach_its_row_by_card_id() {
    let view = view();
    let mut marks = BoardMarks::default();
    marks.by_card.insert(
        view.cards[0].id.clone(),
        TileMark {
            run: Some(RunMark::Working),
            blocked: None,
        },
    );
    marks.by_card.insert(
        view.cards[2].id.clone(),
        TileMark {
            run: None,
            blocked: Some((2, BlockedTone::Muted)),
        },
    );

    let model = build(&view, "", None, 0, &marks);
    let rows: Vec<&CardRow> = model
        .columns
        .iter()
        .flat_map(|column| column.rows.iter())
        .collect();
    let row = |title: &str| {
        *rows
            .iter()
            .find(|row| row.title == title)
            .unwrap_or_else(|| panic!("no row for {title}"))
    };
    assert_eq!(row("Fix login").run, Some(RunMark::Working));
    assert_eq!(row("Fix login").blocked, None);
    assert_eq!(row("Polish").blocked, Some((2, BlockedTone::Muted)));
    assert_eq!(
        row("Ship the board").run,
        None,
        "a card the fold said nothing about carries nothing"
    );
}

/// The header states the two counts as finished strings, and only while they say something.
#[test]
fn the_header_counts_are_composed_with_the_model_and_hidden_at_zero() {
    let mut view = view();
    view.board.settings.max_live_runs = Some(2);
    let facts = |marks: &BoardMarks| HeaderFacts::of(&view, None, 0, marks);

    let quiet = facts(&BoardMarks::default());
    assert_eq!(quiet.working_label, None);
    assert_eq!(quiet.needs_you_label, None);
    assert_eq!(
        (quiet.working, quiet.live_limit, quiet.needs_you),
        (0, 2, 0)
    );

    let busy = facts(&BoardMarks {
        working: 1,
        needs_you: 1,
        ..BoardMarks::default()
    });
    assert_eq!(busy.working_label.as_deref(), Some("1 of 2 runs working"));
    assert_eq!(busy.needs_you_label.as_deref(), Some("1 needs you"));
}

/// A column wears the `⚡` for what it starts, not for where it sends a card afterwards.
#[test]
fn only_an_on_enter_column_carries_an_action() {
    let mut view = view();
    let destination = view.board.statuses[2].id.clone();
    view.board.statuses[0].automation = Some(fleet_core::board::ColumnAutomation {
        on_enter: Some(fleet_core::board::Action {
            kind: fleet_core::board::ActionKind::Prompt,
            instructions: String::new(),
            expect: String::new(),
            agent: fleet_core::board::ColumnAgentPrefs::default(),
            env: Vec::new(),
        }),
        on_success: Some(destination.clone()),
        advance_when_unblocked: None,
    });
    view.board.statuses[1].automation = Some(fleet_core::board::ColumnAutomation {
        on_enter: None,
        on_success: None,
        advance_when_unblocked: Some(destination),
    });

    let model = build(&view, "", None, 0, &BoardMarks::default());
    assert!(model.columns[0].has_action);
    assert!(
        !model.columns[1].has_action,
        "advancing a card that is already done starts nothing"
    );
    assert!(!model.columns[2].has_action, "a plain column is unchanged");
}

#[test]
fn label_chips_carry_the_token_name_not_a_color() {
    let view = view();
    let chips = label_chips(&view.board, &view.cards[0]);
    assert_eq!(chips.len(), 1);
    assert_eq!(chips[0].0.as_ref(), "Bug");
    assert_eq!(chips[0].1.as_deref(), Some("danger"));
}

/// The filter folds case without allocating a lowercased copy of every field of every card.
///
/// The fold is [`crate::presentation::contains_folded`], not an ASCII-only scan: a board is
/// allowed to be in Spanish, and `AÑADIR` must still find `Añadir`. Case folding is also not
/// accent stripping — `sesion` is a different word from `sesión` and finds nothing.
#[test]
fn the_filter_folds_case_beyond_ascii() {
    let mut view = view();
    let status = view.board.statuses[0].id.clone();
    for card in &mut view.cards {
        card.status_id = status.clone();
    }
    view.cards[0].title = "Añadir SESIÓN".to_owned();
    let shown = |query: &str| visible_cards(&view, &status, query).len();
    assert_eq!(shown("sesión"), 1);
    assert_eq!(shown("AÑADIR"), 1);
    assert_eq!(shown("añadir SESIÓN"), 1);
    assert_eq!(shown("sesion"), 0, "folding case is not stripping accents");
}

/// A root view for the board target test: the columns use `gpui::list`, which only lays out
/// inside a rendered entity, so `VisualTestContext::draw` has to go through a view.
struct BoardHarness {
    model: BoardModel,
    lists: Vec<ListState>,
    scroll: ScrollHandle,
    filter_input: Entity<fleet_ui_kit::TextInput>,
}

impl gpui::Render for BoardHarness {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        fleet_ui_kit::AppFrame::new().body(render(
            &BoardProps {
                model: Some(&self.model),
                loading: false,
                error: None,
                filter: "",
                filter_editing: false,
                filter_input: self.filter_input.clone(),
                focus: (1, 0),
                syncing: false,
                runs: false,
                drag: &SharedDrag::default(),
            },
            &self.scroll,
            &self.lists,
            |_click, _cx| {},
            cx,
        ))
    }
}

fn draw_board(cx: &mut gpui::TestAppContext) -> Vec<String> {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let cx = cx.add_empty_window();
    let view = view();
    let model = build(&view, "", None, 1_788_523_200, &BoardMarks::default());
    let lists = model
        .columns
        .iter()
        .map(|column| {
            // One item more than the column's tiles: its `Add card` row is the list's last item.
            ListState::new(
                column.rows.len() + 1,
                gpui::ListAlignment::Top,
                gpui::px(0.0),
            )
        })
        .collect();
    let filter_input =
        cx.new(|cx| fleet_ui_kit::TextInput::new(fleet_ui_kit::InputMode::SingleLine, cx));
    let harness = cx.new(|_| BoardHarness {
        model,
        lists,
        scroll: ScrollHandle::new(),
        filter_input,
    });
    let element = harness.clone();
    cx.draw(
        gpui::Point::default(),
        gpui::size(gpui::px(1200.0), gpui::px(800.0)),
        |_, _| element.into_any_element(),
    );
    cx.update(|window, _| {
        fleet_ui_kit::harness::painted(window)
            .into_iter()
            .map(|target| target.name.to_string())
            .collect()
    })
}

/// `docs/TESTING-HARNESS.md` §3 fixes `board.column[C]` and `board.column[C].card[R]`, and a
/// Phase 5 board scenario drags a card between two of those rects. The card name is composed
/// from a column index *and* a row index, which no kit builder produces, so this pins the
/// spelling [`crate::views::harness::name`] builds.
#[gpui::test]
fn the_board_names_every_column_and_card_for_the_harness(cx: &mut gpui::TestAppContext) {
    fleet_ui_kit::harness::set_recording(true);
    let names = draw_board(cx);
    fleet_ui_kit::harness::set_recording(false);

    assert!(
        names.contains(&"board.column[0]".to_owned()),
        "every column carries its index: {names:?}"
    );
    // The fixture's three cards all land in the second status column.
    assert!(
        (0..3).all(|row| names.contains(&format!("board.column[1].card[{row}]"))),
        "a card is named by its column *and* its row: {names:?}"
    );
    assert!(
        names.iter().all(|name| !name.is_empty()),
        "an empty recorded name means a composite name was built with recording off: {names:?}"
    );
    // The pointer twins of `c`, a column's `+`, a card's ⋯ and `/` (TESTING-HARNESS §3).
    for name in [
        "board.new",
        "board.settings",
        "board.filter",
        "board.column[0].add",
        "board.column[1].add",
        "board.column[1].card[0].menu",
    ] {
        assert!(
            names.contains(&name.to_owned()),
            "{name} is painted: {names:?}"
        );
    }
    assert!(
        !names.contains(&"board.column[1].card[1].menu".to_owned()),
        "an unselected card shows its ⋯ only under the pointer: {names:?}"
    );
    assert!(
        !names.contains(&"board.sync".to_owned()),
        "a local board mirrors nothing, so it has no sync button: {names:?}"
    );
}

/// The pill names what a column's entry starts, in words built from its action.
#[test]
fn an_on_enter_column_names_its_action_in_words() {
    let action = |kind, instructions: &str, provider| fleet_core::board::Action {
        kind,
        instructions: instructions.to_owned(),
        expect: String::new(),
        agent: fleet_core::board::ColumnAgentPrefs {
            provider,
            ..Default::default()
        },
        env: Vec::new(),
    };
    let mut view = view();
    let label = |view: &BoardView, index: usize| {
        build(view, "", None, 0, &BoardMarks::default()).columns[index]
            .automation
            .clone()
            .map(|label| label.to_string())
    };
    view.board.statuses[0].automation = Some(fleet_core::board::ColumnAutomation {
        on_enter: Some(action(
            fleet_core::board::ActionKind::Prompt,
            "Implement this card in the current worktree.",
            Some(AgentKind::Codex),
        )),
        ..Default::default()
    });
    view.board.statuses[1].automation = Some(fleet_core::board::ColumnAutomation {
        on_enter: Some(action(
            fleet_core::board::ActionKind::Skill {
                name: "deep-review".to_owned(),
                args: String::new(),
            },
            "",
            None,
        )),
        ..Default::default()
    });
    assert_eq!(
        label(&view, 0).as_deref(),
        Some("On enter: codex implements")
    );
    assert_eq!(
        label(&view, 1).as_deref(),
        Some("On enter: agent reviews"),
        "a column that leaves the provider to the card does not guess one"
    );
    assert_eq!(label(&view, 2), None, "a plain column wears no pill");
}

/// A blocked card says which card it waits for when there is one, and how many otherwise.
#[test]
fn a_blocked_card_names_its_blocker() {
    let mut view = view();
    let blocker = view.cards[0].id.clone();
    view.cards[2].blocked_by = vec![blocker];
    let key = view.cards[0].display_key(&view.board);
    assert_eq!(
        model::blocked_label(&view, &view.cards[2], 1).as_ref(),
        format!("blocked by {key}")
    );
    assert_eq!(
        model::blocked_label(&view, &view.cards[2], 3).as_ref(),
        "blocked by 3 cards"
    );
}

/// The header's `1 needs you` points at the first card waiting on a person, in board order.
#[test]
fn the_needs_you_count_points_at_the_first_card_that_needs_you() {
    let view = view();
    let mut marks = BoardMarks::default();
    marks.by_card.insert(
        view.cards[1].id.clone(),
        TileMark {
            run: Some(RunMark::NeedsYou),
            blocked: None,
        },
    );
    let model = build(&view, "", None, 0, &marks);
    assert_eq!(model.needs_you_at, Some((1, 1)));
    assert_eq!(
        build(&view, "", None, 0, &BoardMarks::default()).needs_you_at,
        None
    );
}

/// The other half of the contract: naming a surface costs one flag read in production.
#[gpui::test]
fn the_board_records_no_target_with_the_harness_off(cx: &mut gpui::TestAppContext) {
    assert!(
        draw_board(cx).is_empty(),
        "the paint path must record nothing while recording is off"
    );
}
