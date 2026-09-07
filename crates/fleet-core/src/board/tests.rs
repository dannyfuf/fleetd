use super::defaults::default_prefix;
use super::*;
use crate::{
    ids::{CardId, ContextId},
    model::Context,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, fmt::Debug};

fn board() -> Board {
    new_board(
        &Context {
            id: ContextId::try_from("work").unwrap(),
            name: "Fleet".into(),
            owners: vec![],
            created_at: "2026-09-06T12:00:00Z".into(),
        },
        "2026-09-06T12:00:00Z",
    )
}
fn card(board: &Board) -> Card {
    // Required wire fields alone must produce a complete defaulted card.
    serde_json::from_value(serde_json::json!({
        "id": "card-1", "boardId": board.id, "number": 12, "title": "Fix login",
        "statusId": "todo", "createdAt": board.created_at, "updatedAt": board.updated_at
    }))
    .unwrap()
}
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    assert_eq!(
        &serde_json::from_str::<T>(&serde_json::to_string(value).unwrap()).unwrap(),
        value
    );
}
#[test]
fn board_card_and_document_round_trip() {
    let board = board();
    let mut card = card(&board);
    card.properties
        .insert("score".into(), PropertyValue::Number(2.5));
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "fake".into(),
        key: "REMOTE-4".into(),
        url: None,
        version: Some("v1".into()),
        synced_at: board.created_at.clone(),
        remote_updated_at: None,
    });
    card.conflict = Some(Conflict {
        detected_at: board.updated_at.clone(),
        remote: RemoteCard::default(),
        fields: vec!["title".into()],
    });
    round_trip(&board);
    round_trip(&card);
    round_trip(&BoardDocument {
        version: BOARD_DOCUMENT_VERSION,
        board,
        cards: vec![card],
    });
}
#[test]
fn property_value_wire_tags_and_display() {
    let cases = [
        (
            PropertyValue::Text("text".into()),
            serde_json::json!({"kind":"text","value":"text"}),
            Some(PropertyKind::Text),
        ),
        (
            PropertyValue::Number(2.5),
            serde_json::json!({"kind":"number","value":2.5}),
            Some(PropertyKind::Number),
        ),
        (
            PropertyValue::Bool(true),
            serde_json::json!({"kind":"bool","value":true}),
            Some(PropertyKind::Bool),
        ),
        (
            PropertyValue::Date("2026-09-06".into()),
            serde_json::json!({"kind":"date","value":"2026-09-06"}),
            Some(PropertyKind::Date),
        ),
        (
            PropertyValue::Select("a".into()),
            serde_json::json!({"kind":"select","value":"a"}),
            Some(PropertyKind::Select),
        ),
        (
            PropertyValue::MultiSelect(vec!["a".into(), "b".into()]),
            serde_json::json!({"kind":"multi_select","value":["a","b"]}),
            Some(PropertyKind::MultiSelect),
        ),
        (
            PropertyValue::User("Danny".into()),
            serde_json::json!({"kind":"user","value":"Danny"}),
            Some(PropertyKind::User),
        ),
        (
            PropertyValue::Url("https://example.com".into()),
            serde_json::json!({"kind":"url","value":"https://example.com"}),
            Some(PropertyKind::Url),
        ),
        (
            PropertyValue::Null,
            serde_json::json!({"kind":"null"}),
            None,
        ),
    ];
    for (value, json, kind) in cases {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        round_trip(&value);
        if let Some(kind) = kind {
            assert!(value.matches_kind(kind));
        }
    }
    assert_eq!(
        PropertyValue::MultiSelect(vec!["a".into(), "b".into()]).display(),
        "a, b"
    );
    assert!(PropertyValue::Null.matches_kind(PropertyKind::Text));
    assert!(!PropertyValue::Text("1".into()).matches_kind(PropertyKind::Number));
}
#[test]
fn defaults_agree_with_serde_and_prefix_is_ascii() {
    let settings: BoardSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings, BoardSettings::default());
    assert!(settings.start_on_worktree);
    assert_eq!(settings.branch_template, "{key}-{slug}");
    assert_eq!(board().prefix, "FLE");
    let context = Context {
        id: "work".parse().unwrap(),
        name: "中文!?".into(),
        owners: vec![],
        created_at: String::new(),
    };
    assert_eq!(default_prefix(&context), "FLT");
    assert_eq!(default_statuses().len(), 5);
}
#[test]
fn patch_null_clears_while_absence_leaves_unchanged() {
    let patch: CardPatch =
        serde_json::from_str(r#"{"assignee":null,"estimate":null,"repoId":null}"#).unwrap();
    assert_eq!(patch.assignee, Some(None));
    assert_eq!(patch.estimate, Some(None));
    assert_eq!(patch.repo_id, Some(None));
    assert_eq!(patch.parent_id, None);
    round_trip(&patch);
    round_trip(&CardPatch::default());
    assert!(CardPatch::default().is_empty());
    assert!(!patch.is_empty());
    let board_patch: BoardPatch = serde_json::from_str(r#"{"defaultRepoId":null}"#).unwrap();
    assert_eq!(board_patch.default_repo_id, Some(None));
    round_trip(&board_patch);
    round_trip(&BoardPatch::default());
}
#[test]
fn validates_references_prefix_and_calendar_date() {
    let mut board = board();
    let mut card = card(&board);
    assert!(validate_board(&board).is_ok());
    assert!(validate_card(&board, &card).is_ok());
    card.due_date = Some("2024-02-29".into());
    assert!(validate_card(&board, &card).is_ok());
    for bad in [
        "2025-02-29",
        "2026-2-03",
        "2026-13-03",
        "0000-01-01",
        "ééééé",
    ] {
        card.due_date = Some(bad.into());
        assert!(validate_card(&board, &card).is_err());
    }
    card.due_date = None;
    card.status_id = "missing".parse().unwrap();
    assert!(matches!(
        validate_card(&board, &card),
        Err(BoardError::UnknownStatus(_))
    ));
    card.status_id = "todo".parse().unwrap();
    card.labels.push("bug".parse().unwrap());
    assert!(matches!(
        validate_card(&board, &card),
        Err(BoardError::UnknownLabel(_))
    ));
    board.statuses.push(board.statuses[0].clone());
    assert!(validate_board(&board).is_err());
    board.statuses.pop();
    board.prefix = "bad".into();
    assert!(validate_board(&board).is_err());
}
#[test]
fn checks_custom_property_schema() {
    let mut board = board();
    let mut card = card(&board);
    board.properties.push(PropertySchema {
        key: "score".into(),
        name: "Score".into(),
        kind: PropertyKind::Number,
        options: vec![],
        editable: true,
        source: PropertySource::Local,
        show_on_card: true,
    });
    card.properties = BTreeMap::from([("score".into(), PropertyValue::Number(1.5))]);
    assert!(validate_card(&board, &card).is_ok());
    card.properties
        .insert("score".into(), PropertyValue::Text("1".into()));
    assert!(validate_card(&board, &card).is_err());
}
#[test]
fn orders_columns_and_summarizes_live_cards() {
    let board = board();
    let mut first = card(&board);
    first.position = 10;
    let mut second = first.clone();
    second.id = CardId::try_from("card-2").unwrap();
    second.number = 13;
    second.position = 0;
    second.dirty = true;
    let mut archived = second.clone();
    archived.archived = true;
    let mut done = first.clone();
    done.status_id = "done".parse().unwrap();
    let cards = vec![first, second, archived, done];
    let column = column_cards(&cards, &"todo".parse().unwrap());
    assert_eq!(
        column.iter().map(|c| c.number).collect::<Vec<_>>(),
        vec![13, 12]
    );
    let summary = summarize(&board, &cards);
    assert_eq!(
        (summary.card_count, summary.open_count, summary.dirty_count),
        (3, 2, 1)
    );
    assert_eq!(
        first_status_in(&board, StatusCategory::Started)
            .unwrap()
            .id
            .as_str(),
        "in-progress"
    );
}
#[test]
fn comments_stamp_activity_and_cap_history() {
    let board = board();
    let mut card = card(&board);
    assert!(add_comment(&mut card, "empty".into(), None, "  ".into(), "now").is_err());
    add_comment(&mut card, "comment".into(), None, "hello".into(), "now").unwrap();
    assert_eq!(card.updated_at, "now");
    assert!(!card.dirty);
    assert_eq!(card.activity[0].kind, ActivityKind::Commented);
    for i in 0..205 {
        push_activity(
            &mut card,
            ActivityKind::Updated,
            None,
            i.to_string(),
            "later",
        );
    }
    assert_eq!(card.activity.len(), 200);
    assert_eq!(card.activity[0].message, "5");
}
#[test]
fn worktree_slug_uses_remote_keys_template_and_limits() {
    let mut board = board();
    let mut card = card(&board);
    assert_eq!(card.local_key(&board), "FLE-12");
    assert_eq!(worktree_slug(&board, &card), "fle-12-fix-login");
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "fake".into(),
        key: "EXT-9".into(),
        url: None,
        version: None,
        synced_at: String::new(),
        remote_updated_at: None,
    });
    assert_eq!(card.display_key(&board), "EXT-9");
    assert_eq!(worktree_slug(&board, &card), "ext-9-fix-login");
    card.title = "x".repeat(100);
    assert_eq!(worktree_slug(&board, &card).len(), 48);
    board.settings.branch_template = "!?".into();
    assert_eq!(worktree_slug(&board, &card), "fle-12");
}
