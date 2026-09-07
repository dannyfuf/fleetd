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
    BoardView { board, cards }
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
    let facts = HeaderFacts::of(&view, None, 0);
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
    let facts = HeaderFacts::of(&view, Some("Local"), 1_788_523_200);
    assert_eq!(facts.dirty, 1);
    assert_eq!(facts.conflicts, 0);
    assert!(facts.local);
    assert!(facts.synced.is_some());
}

#[test]
fn label_chips_carry_the_token_name_not_a_color() {
    let view = view();
    let chips = label_chips(&view.board, &view.cards[0]);
    assert_eq!(chips.len(), 1);
    assert_eq!(chips[0].0.as_ref(), "Bug");
    assert_eq!(chips[0].1.as_deref(), Some("danger"));
}
