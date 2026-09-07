//! The card-detail surface (BOARD §8, UX-SPEC §board).
//!
//! Two panes. The left one is the card as prose — key, title, markdown description, comments,
//! activity. The right one is the card as **facts**: one row per property, `j` / `k` selects
//! and `Enter` opens the picker that edits it. The split is the whole idea: everything on the
//! right is a value with a closed set of answers, so it never needs a text editor, and
//! everything on the left is text, so it never needs a picker.

use fleet_core::board::{Board, Card, PropertyKind, PropertySource, field_label};
use fleet_ui_kit::{
    ActiveTheme, Banner, Icon, IconSize, KeyHintRow, MarkdownText, PriorityGlyph, Row, RowColumn,
    SectionHeader, Text, Theme, Tone,
};
use gpui::{AnyElement, App, SharedString, div, prelude::*, px};

use crate::{dialogs::card_picker::PickerKind, presentation::age_label};

/// How many activity entries §8 shows.
pub const ACTIVITY_ROWS: usize = 10;
/// The width of a property row's label column.
const LABEL_WIDTH: f32 = 78.0;
/// The trailing slot the lock glyph of a backend-owned row sits in, in `ch`.
const LOCK_COLUMN_CH: f32 = 2.0;

/// What `Enter` does on a property row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyTarget {
    /// Open the picker that edits this field.
    Pick(PickerKind),
    /// Open the linked worktree's session.
    Worktree,
    /// Nothing: the row states a fact the app does not edit.
    ReadOnly,
}

/// One row of the right-hand property pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyRow {
    /// The field name.
    pub label: SharedString,
    /// The value, already rendered; an en dash when unset.
    pub value: SharedString,
    /// How the value reads: `Muted` for an unset one, `Danger` for a conflict.
    pub tone: Tone,
    /// Whether the value is an identifier and belongs in the mono face.
    pub mono: bool,
    /// Whether the board's backend owns this field and refuses local writes.
    ///
    /// The row keeps its picker target: `Enter` still has to answer, and the answer is the
    /// daemon's own sentence about the backend. A row that silently did nothing would look
    /// exactly like a broken key.
    pub locked: bool,
    /// What `Enter` does here.
    pub target: PropertyTarget,
}

impl PropertyRow {
    fn new(label: &'static str, value: Option<String>, target: PropertyTarget) -> Self {
        let unset = value.is_none();
        Self {
            label: SharedString::new_static(label),
            value: SharedString::from(value.unwrap_or_else(|| "\u{2013}".to_owned())),
            tone: if unset { Tone::Muted } else { Tone::Default },
            mono: false,
            locked: false,
            target,
        }
    }

    fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Marks the row as owned by the backend: secondary tone plus the lock glyph.
    fn locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        if locked {
            self.tone = Tone::Secondary;
        }
        self
    }
}

/// Whether the board's backend refuses local writes to a standard card field.
///
/// The same rule `fleet_core::board::ops` applies: a local board declares no backend, so
/// nothing on it is read-only, however stale a `readonly_fields` list left behind by a former
/// backend might be.
#[must_use]
pub fn is_readonly(board: &Board, field: &str) -> bool {
    !board.backend.is_local()
        && board
            .sync
            .readonly_fields
            .iter()
            .any(|readonly| readonly == field)
}

/// Every property row of a card, in the order §8 fixes.
///
/// The custom properties come last and in schema order, so a backend that adds a field never
/// reshuffles the rows a user already knows the position of.
///
/// `cards` is the board's card set, which the Parent row needs: a card's `parent_id` is an
/// opaque UUID, and every name this UI gives a card is its display key.
#[must_use]
pub fn property_rows(board: &Board, cards: &[Card], card: &Card, now: i64) -> Vec<PropertyRow> {
    let status = board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .map(|status| status.name.clone());
    let labels: Vec<String> = card
        .labels
        .iter()
        .map(|id| {
            board
                .labels
                .iter()
                .find(|label| &label.id == id)
                .map_or_else(|| id.as_str().to_owned(), |label| label.name.clone())
        })
        .collect();
    let parent = card.parent_id.as_ref().map(|id| {
        cards.iter().find(|parent| parent.id == *id).map_or_else(
            || id.as_str().to_owned(),
            |parent| parent.display_key(board),
        )
    });

    let locked = |field: &str| is_readonly(board, field);
    let mut rows = vec![
        PropertyRow::new("Status", status, PropertyTarget::Pick(PickerKind::Status))
            .locked(locked("status_id")),
        PropertyRow::new(
            "Priority",
            (card.priority != fleet_core::board::Priority::None)
                .then(|| card.priority.label().to_owned()),
            PropertyTarget::Pick(PickerKind::Priority),
        )
        .locked(locked("priority")),
        PropertyRow::new(
            "Assignee",
            card.assignee.clone(),
            PropertyTarget::Pick(PickerKind::Assignee),
        )
        .locked(locked("assignee")),
        PropertyRow::new(
            "Labels",
            (!labels.is_empty()).then(|| labels.join(", ")),
            PropertyTarget::Pick(PickerKind::Labels),
        )
        .locked(locked("labels")),
        PropertyRow::new(
            "Estimate",
            card.estimate.map(|points| format!("{points} pt")),
            PropertyTarget::Pick(PickerKind::Estimate),
        )
        .locked(locked("estimate")),
        PropertyRow::new(
            "Due",
            card.due_date.clone(),
            PropertyTarget::Pick(PickerKind::DueDate),
        )
        .mono()
        .locked(locked("due_date")),
        PropertyRow::new("Parent", parent, PropertyTarget::ReadOnly)
            .mono()
            .locked(locked("parent_id")),
        PropertyRow::new(
            "Repo",
            card.repo_id.as_ref().map(|repo| repo.as_str().to_owned()),
            PropertyTarget::Pick(PickerKind::Repo),
        )
        .mono(),
        PropertyRow::new(
            "Worktree",
            card.worktree_id
                .as_ref()
                .map(|worktree| worktree.as_str().to_owned()),
            if card.worktree_id.is_some() {
                PropertyTarget::Worktree
            } else {
                PropertyTarget::ReadOnly
            },
        )
        .mono(),
    ];

    if let Some(remote) = card.remote.as_ref() {
        let mut value = remote.key.clone();
        if card.dirty {
            value.push_str(" \u{00b7} dirty");
        }
        rows.push(
            PropertyRow::new("Remote", Some(value), PropertyTarget::ReadOnly)
                .mono()
                .tone(if card.dirty {
                    Tone::Warning
                } else {
                    Tone::Default
                }),
        );
        if let Some(url) = remote.url.as_deref() {
            rows.push(
                PropertyRow::new("URL", Some(url.to_owned()), PropertyTarget::ReadOnly).mono(),
            );
        }
        rows.push(
            PropertyRow::new(
                "Synced",
                Some(age_label(&remote.synced_at, now)),
                PropertyTarget::ReadOnly,
            )
            .mono(),
        );
    }

    for schema in &board.properties {
        let value = card
            .properties
            .get(&schema.key)
            .map(fleet_core::board::PropertyValue::display)
            .filter(|value| !value.is_empty());
        let target = if schema.editable {
            PropertyTarget::Pick(PickerKind::Property(schema.key.clone()))
        } else {
            PropertyTarget::ReadOnly
        };
        let mono = matches!(
            schema.kind,
            PropertyKind::Url | PropertyKind::Date | PropertyKind::Number
        ) || schema.source == PropertySource::Backend;
        let row = PropertyRow {
            label: SharedString::from(schema.name.clone()),
            value: SharedString::from(value.clone().unwrap_or_else(|| "\u{2013}".to_owned())),
            tone: if value.is_none() {
                Tone::Muted
            } else {
                Tone::Default
            },
            mono,
            locked: false,
            target,
        };
        // A backend-declared property carries its own `editable` flag, so the lock glyph says
        // the same thing here that `readonly_fields` says about the standard rows.
        rows.push(row.locked(!schema.editable));
    }
    rows
}

/// Renders one property row with its selection and cursor state.
#[must_use]
pub fn property_row(row: &PropertyRow, selected: bool, focused: bool, theme: &Theme) -> AnyElement {
    let value = if row.mono {
        Text::data_small(row.value.clone())
            .tone(row.tone)
            .ellipsize()
    } else {
        Text::ui(row.value.clone()).tone(row.tone).ellipsize()
    };
    Row::new()
        .selected(selected)
        .cursor(selected && focused)
        .column(RowColumn::fixed(
            px(LABEL_WIDTH),
            Text::label(row.label.clone()),
        ))
        .column(RowColumn::flex(value))
        // Trailing, not leading: a glyph in front of the label would push the whole value
        // gutter one column right on exactly the boards that have locked fields.
        .columns(row.locked.then(|| {
            RowColumn::fixed_ch(
                LOCK_COLUMN_CH,
                Icon::Lock
                    .el()
                    .size(IconSize::Small)
                    .color(Tone::Secondary.color(theme)),
            )
        }))
        .into_any_element()
}

/// The conflict banner of §8, with the two keys that resolve it.
///
/// `field_label` lives in the core: the CLI's card report prints the same conflict list, and
/// "Conflict — status_id, due_date" is a sentence the user has to translate on either surface.
#[must_use]
pub fn conflict_banner(card: &Card) -> Option<Banner> {
    let conflict = card.conflict.as_ref()?;
    let fields = if conflict.fields.is_empty() {
        "the remote changed".to_owned()
    } else {
        conflict
            .fields
            .iter()
            .map(|field| field_label(field))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Some(
        Banner::danger(format!(
            "Conflict \u{2014} {fields} differ from {}",
            conflict.remote.key
        ))
        .icon(Icon::TriangleAlert)
        .hints(
            KeyHintRow::new()
                .key("K", "keep local")
                .key("R", "take remote"),
        ),
    )
}

/// The comments list, newest last, each with its author and age.
#[must_use]
pub fn comments(card: &Card, now: i64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.sm)
        .child(SectionHeader::new(format!(
            "Comments ({})",
            card.comments.len()
        )))
        .children((card.comments.is_empty()).then(|| Text::ui("No comments yet.").faint()))
        .children(card.comments.iter().map(|comment| {
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(theme.space.xxs)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.xs)
                        .child(
                            Text::label(comment.author.clone().unwrap_or_else(|| "you".to_owned()))
                                .tone(Tone::Secondary),
                        )
                        .child(Text::hint(age_label(&comment.created_at, now))),
                )
                .child(MarkdownText::new(comment.body.clone()))
        }))
        .into_any_element()
}

/// The last [`ACTIVITY_ROWS`] activity entries, newest first.
#[must_use]
pub fn activity(card: &Card, now: i64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let entries: Vec<_> = card.activity.iter().rev().take(ACTIVITY_ROWS).collect();
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.xxs)
        .child(SectionHeader::new("Activity"))
        .children(entries.is_empty().then(|| Text::ui("Nothing yet.").faint()))
        .children(entries.into_iter().map(|entry| {
            div()
                .flex()
                .items_baseline()
                .gap(theme.space.xs)
                .w_full()
                .child(Text::hint(age_label(&entry.at, now)).w(px(44.0)))
                .child(Text::ui(entry.message.clone()).muted().ellipsize())
        }))
        .into_any_element()
}

/// The card's key, priority glyph and title, as the left pane's first line.
#[must_use]
pub fn title_line(board: &Board, card: &Card, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.xxs)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(Text::data_small(card.display_key(board)).faint())
                .children(
                    (card.priority != fleet_core::board::Priority::None).then(|| {
                        PriorityGlyph::new(super::board_screen::priority_level(card.priority))
                            .with_label(true)
                    }),
                )
                .children(card.worktree_id.is_some().then(|| {
                    Icon::GitBranch
                        .el()
                        .size(IconSize::Small)
                        .color(Tone::Secondary.color(theme))
                })),
        )
        .child(Text::title(card.title.clone()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
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
            "the row still has to answer Enter — with the backend's own sentence"
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
}
