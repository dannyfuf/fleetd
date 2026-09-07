use super::*;
use fleet_core::{
    board::{CardDraft, Label, RemoteLink, create_card, new_board},
    ids::{ContextId, LabelId},
    model::Context,
};

fn fixture() -> (Board, Card) {
    let context = Context {
        id: ContextId::try_from("work").unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: vec![],
        created_at: "2026-09-06T12:00:00Z".into(),
    };
    let mut board = new_board(&context, "2026-09-06T12:00:00Z");
    board.labels.push(Label {
        id: LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}")),
        name: "Bug".into(),
        color: None,
    });
    let card = create_card(
        &mut board,
        &[],
        "card-1".parse().unwrap_or_else(|error| panic!("{error}")),
        CardDraft {
            title: "Fix login".into(),
            labels: vec![LabelId::try_from("bug").unwrap_or_else(|error| panic!("{error}"))],
            ..CardDraft::default()
        },
        "2026-09-06T12:00:00Z",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    (board, card)
}

#[test]
fn the_rows_are_the_contract_order_and_unset_values_read_as_a_dash() {
    let (board, card) = fixture();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_ref()).collect();
    assert_eq!(
        labels,
        [
            "Status", "Priority", "Assignee", "Labels", "Estimate", "Due", "Parent", "Repo",
            "Worktree"
        ]
    );
    let assignee = &rows[2];
    assert_eq!(assignee.value.as_ref(), "\u{2013}");
    assert_eq!(assignee.tone, Tone::Muted);
    assert_eq!(rows[3].value.as_ref(), "Bug");
}

#[test]
fn every_editable_row_names_the_picker_that_edits_it() {
    let (board, card) = fixture();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert_eq!(rows[0].target, PropertyTarget::Pick(PickerKind::Status));
    assert_eq!(rows[5].target, PropertyTarget::Pick(PickerKind::DueDate));
    assert_eq!(
        rows[6].target,
        PropertyTarget::ReadOnly,
        "parent is v1 read-only"
    );
    assert_eq!(rows[8].target, PropertyTarget::ReadOnly, "no worktree yet");
}

#[test]
fn a_linked_card_gains_the_remote_rows_and_a_worktree_target() {
    let (board, mut card) = fixture();
    card.worktree_id = Some(
        "buk/payroll#fix"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
    );
    card.dirty = true;
    card.remote = Some(RemoteLink {
        parent_key: None,
        backend: "jira".into(),
        key: "PROJ-12".into(),
        url: Some("https://example.test/PROJ-12".into()),
        version: None,
        synced_at: "2026-09-06T12:00:00Z".into(),
        remote_updated_at: None,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert_eq!(rows[8].target, PropertyTarget::Worktree);
    let remote = rows
        .iter()
        .find(|row| row.label.as_ref() == "Remote")
        .unwrap_or_else(|| panic!("no remote row"));
    assert!(remote.value.contains("PROJ-12"));
    assert!(remote.value.contains("dirty"));
    assert_eq!(remote.tone, Tone::Warning);
    assert!(rows.iter().any(|row| row.label.as_ref() == "URL"));
    assert!(rows.iter().any(|row| row.label.as_ref() == "Synced"));
}

#[test]
fn a_backend_owned_field_reads_as_locked_but_keeps_its_picker() {
    let (mut board, card) = fixture();
    board.sync.readonly_fields = vec!["priority".into(), "parent_id".into()];
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    assert!(
        rows.iter().all(|row| !row.locked),
        "a local board declares no backend, so nothing on it is read-only"
    );

    board.backend.kind = "jira".into();
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let locked: Vec<&str> = rows
        .iter()
        .filter(|row| row.locked)
        .map(|row| row.label.as_ref())
        .collect();
    assert_eq!(locked, ["Priority", "Parent"]);
    assert_eq!(rows[1].tone, Tone::Secondary);
    assert_eq!(
        rows[1].target,
        PropertyTarget::Pick(PickerKind::Priority),
        "the row still has to answer Enter \u{2014} with the backend's own sentence"
    );
    assert!(!rows[0].locked, "status is writable on this board");
}

#[test]
fn a_property_the_schema_calls_uneditable_wears_the_same_lock() {
    let (mut board, card) = fixture();
    board.properties.push(fleet_core::board::PropertySchema {
        key: "created".into(),
        name: "Created".into(),
        kind: PropertyKind::Date,
        options: Vec::new(),
        editable: false,
        source: PropertySource::Backend,
        show_on_card: false,
    });
    let rows = property_rows(&board, std::slice::from_ref(&card), &card, 0);
    let row = rows.last().unwrap_or_else(|| panic!("no property row"));
    assert!(row.locked);
    assert_eq!(row.tone, Tone::Secondary);
    assert_eq!(row.target, PropertyTarget::ReadOnly);
}

#[test]
fn a_conflict_banner_names_the_fields_that_differ() {
    let (_, mut card) = fixture();
    assert!(conflict_banner(&card).is_none());
    card.conflict = Some(fleet_core::board::Conflict {
        detected_at: "2026-09-06T12:00:00Z".into(),
        remote: fleet_core::board::RemoteCard {
            key: "PROJ-12".into(),
            ..fleet_core::board::RemoteCard::default()
        },
        fields: vec!["title".into(), "status".into()],
    });
    assert!(conflict_banner(&card).is_some());
}

/// `differing_fields` answers in wire names; the banner has to say what the rows say.
#[test]
fn the_conflict_banner_names_fields_the_way_the_property_rows_do() {
    assert_eq!(field_label("status_id"), "Status");
    assert_eq!(field_label("due_date"), "Due");
    assert_eq!(field_label("parent_id"), "Parent");
    assert_eq!(field_label("title"), "Title");
    // A name nobody mapped is still printed rather than dropped.
    assert_eq!(field_label("something_new"), "something_new");
}
