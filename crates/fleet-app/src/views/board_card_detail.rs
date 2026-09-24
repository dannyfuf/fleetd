//! The card-detail surface (BOARD §8, UX-SPEC §board).
//!
//! Two columns inside a right-side sheet. The left one is the card as prose — title, run,
//! markdown description, comments. The right one is the card as **facts**: one row per
//! property, `j` / `k` selects, `Enter` or a click opens the picker that edits it, and the
//! activity folds under them. The split is the whole idea: everything on the right is a value
//! with a closed set of answers, so it never needs a text editor, and everything on the left is
//! text, so it never needs a picker.

use std::{collections::HashSet, rc::Rc};

use fleet_core::{
    agents::DelegationStatus,
    board::{
        ActionKind, Board, Card, Comment, PropertyKind, PropertySource, RunOutcome, StatusCategory,
        blocks, field_label, is_satisfied, latest_run, resolve_prefs,
    },
};
use fleet_ui_kit::{
    ActiveTheme, Avatar, Badge, BadgeStyle, Button, ButtonSize, ButtonStyle, Callout, ColumnAlign,
    Icon, IconSize, InfoCard, Kbd, KbdSize, MarkdownText, Row, RowColumn, RunMark, Spinner,
    StatusDot, Text, Theme, Tone, format_cost, format_duration, format_token_count,
    harness::HarnessTargetExt as _,
};
use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    actions::{board, card_detail},
    dialogs::card_picker::PickerKind,
    presentation::{age_label, parse_timestamp},
    // How many lines of a run's report the card shows before `⏎ expand`. The transcript's own
    // constant: a report comment is the delivered child result on another surface, so it folds
    // where the transcript folds it (contracts §5.3).
    screens::agent_thread::rows::item::DELEGATION_RESULT_COLLAPSE_LINES as REPORT_COLLAPSE_LINES,
};

#[cfg(test)]
mod tests;

/// How many activity entries §8 shows once the section is open.
const ACTIVITY_ROWS: usize = 10;
/// How many it shows folded, under the properties.
const ACTIVITY_FOLDED: usize = 3;
/// The width of a property row's label column, in `ch`: `Blocked by` in the UI face and a gutter.
const LABEL_COLUMN_CH: f32 = 9.0;
/// The trailing slot the key chip or the lock glyph of a row sits in, in `ch`.
const TRAILING_COLUMN_CH: f32 = 3.0;

/// What `Enter` does on a property row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PropertyTarget {
    /// Open the picker that edits this field.
    Pick(PickerKind),
    /// Open the linked worktree's session.
    Worktree,
    /// Open the remote issue in the browser, as `x` does.
    Remote,
    /// Open the card's pull request in the browser, as `B` does.
    PullRequest,
    /// Nothing: the row states a fact the app does not edit.
    ReadOnly,
}

/// One row of the right-hand property pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyRow {
    /// The field name.
    pub(crate) label: SharedString,
    /// The value, already rendered; an en dash when unset.
    pub(crate) value: SharedString,
    /// How the value reads: `Muted` for an unset one, `Danger` for a conflict.
    pub(crate) tone: Tone,
    /// Whether the value is an identifier and belongs in the mono face.
    pub(crate) mono: bool,
    /// Whether the board's backend owns this field and refuses local writes.
    ///
    /// The row keeps its picker target: `Enter` still has to answer, and the answer is the
    /// daemon's own sentence about the backend. A row that silently did nothing would look
    /// exactly like a broken key.
    pub(crate) locked: bool,
    /// What `Enter` does here.
    pub(crate) target: PropertyTarget,
    /// Where an inherited value came from, drawn muted after it (`column default`).
    ///
    /// Only the three agent rows carry one: every other row states the card's own value, and a
    /// source beside it would be noise on every board.
    pub(crate) source: Option<SharedString>,
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
            source: None,
        }
    }

    /// Names where an inherited value came from, beside it and muted.
    #[must_use]
    fn source(mut self, source: &'static str) -> Self {
        self.source = Some(SharedString::new_static(source));
        self
    }

    #[must_use]
    fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    #[must_use]
    fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Marks the row as owned by the backend: secondary tone plus the lock glyph.
    #[must_use]
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
fn is_readonly(board: &Board, field: &str) -> bool {
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
    ];
    // Directly under Status, because they answer the question Status raises on an automated
    // board — what runs this card next — and a board nobody automated has none of them.
    rows.extend(workflow_rows(board, cards, card));
    rows.extend([
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
    ]);
    // Only a review card has one, and a card's pull request never changes after creation, so
    // the row is zero-suppressed rather than a dash on every task card.
    if let Some(pull_request) = card.pull_request.as_ref() {
        rows.push(
            PropertyRow::new(
                "Pull request",
                Some(pull_request_value(&pull_request.key())),
                PropertyTarget::PullRequest,
            )
            .mono(),
        );
    }

    if let Some(remote) = card.remote.as_ref() {
        let mut value = remote.key.clone();
        if card.dirty {
            value.push_str(" \u{00b7} dirty");
        }
        rows.push(
            PropertyRow::new("Remote", Some(value), PropertyTarget::Remote)
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
            source: None,
        };
        // A backend-declared property carries its own `editable` flag, so the lock glyph says
        // the same thing here that `readonly_fields` says about the standard rows.
        rows.push(row.locked(!schema.editable));
    }
    rows
}

/// `acme/api#412 · open ↗`: the reference, and the word that says `⏎` leaves Fleet.
fn pull_request_value(key: &str) -> String {
    format!("{key} \u{00b7} open \u{2197}")
}

/// The rows a workflow board adds under Status, each one zero-suppressed (contracts §5.3).
///
/// Provider, Model and Effort exist only while the card's column runs an action, and state what
/// *this* run would use once `resolve_prefs` has read the card over the column, so a card that
/// inherits everything still reads the three values its run will get and says where they came
/// from. Blocked by and Blocks appear as soon as the board uses links at all: an empty row is
/// how the reader learns the card can have them.
fn workflow_rows(board: &Board, cards: &[Card], card: &Card) -> Vec<PropertyRow> {
    let mut rows = Vec::new();
    let action = board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .and_then(|status| status.automation.as_ref())
        .and_then(|automation| automation.on_enter.as_ref());
    if let Some(action) = action {
        let locked = is_readonly(board, "agent");
        let prefs = card.agent.as_ref();
        let resolved = resolve_prefs(card, action);
        // A skill column runs on Claude whatever the card asked for, so the provider this row
        // shows is the column's even on a card that named one — saying otherwise would be a
        // claim the run then contradicts.
        let provider_is_the_cards = prefs.is_some_and(|prefs| prefs.provider.is_some())
            && !matches!(action.kind, ActionKind::Skill { .. });
        rows.push(
            agent_row(
                "Provider",
                resolved
                    .provider
                    .map(|provider| provider.executable().to_owned()),
                provider_is_the_cards,
                PickerKind::Provider,
            )
            .locked(locked),
        );
        rows.push(
            agent_row(
                "Model",
                resolved.model,
                prefs.is_some_and(|prefs| prefs.model.is_some()),
                PickerKind::Model,
            )
            .locked(locked),
        );
        rows.push(
            agent_row(
                "Effort",
                resolved.effort,
                prefs.is_some_and(|prefs| prefs.effort.is_some()),
                PickerKind::Effort,
            )
            .locked(locked),
        );
    }
    let dependants = blocks(cards, &card.id);
    // A board nobody has linked prints exactly the property list it printed before links
    // existed; two em dashes on every card is not a feature anybody asked for.
    if card.blocked_by.is_empty()
        && dependants.is_empty()
        && !cards.iter().any(|other| !other.blocked_by.is_empty())
    {
        return rows;
    }
    let locked = is_readonly(board, "blocked_by");
    rows.extend(blocked_by_rows(board, cards, card, locked));
    rows.extend(blocks_rows(board, &dependants, locked));
    rows
}

/// One agent row, muted-sourced when the value came from the column rather than the card.
fn agent_row(
    label: &'static str,
    value: Option<String>,
    from_the_card: bool,
    kind: PickerKind,
) -> PropertyRow {
    let inherited = value.is_some() && !from_the_card;
    let row = PropertyRow::new(label, value, PropertyTarget::Pick(kind));
    if inherited {
        row.source("column default")
    } else {
        row
    }
}

/// One row per blocker, labelled once, in the order the card lists them.
///
/// A satisfied blocker is checked rather than dropped: the link is still there, and a row that
/// vanished the moment its blocker finished would leave the reader wondering what released the
/// card. A blocker that was canceled or archived is amber, because nothing will release it now.
fn blocked_by_rows(board: &Board, cards: &[Card], card: &Card, locked: bool) -> Vec<PropertyRow> {
    if card.blocked_by.is_empty() {
        return vec![
            PropertyRow::new(
                "Blocked by",
                None,
                PropertyTarget::Pick(PickerKind::BlockedBy),
            )
            .locked(locked),
        ];
    }
    card.blocked_by
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let blocker = cards.iter().find(|other| other.id == *id);
            let satisfied = is_satisfied(board, cards, id);
            let stuck = blocker.is_none_or(|blocker| {
                blocker.archived || in_category(board, blocker, StatusCategory::Canceled)
            });
            let row = PropertyRow::new(
                if index == 0 { "Blocked by" } else { "" },
                Some(link_value(board, blocker, id.as_str(), satisfied)),
                PropertyTarget::Pick(PickerKind::BlockedBy),
            );
            if satisfied || !stuck {
                row
            } else {
                row.tone(Tone::Warning)
            }
            .locked(locked)
        })
        .collect()
}

/// One row per dependant, derived from everyone's `blocked_by` rather than stored.
fn blocks_rows(board: &Board, dependants: &[&Card], locked: bool) -> Vec<PropertyRow> {
    if dependants.is_empty() {
        return vec![
            PropertyRow::new("Blocks", None, PropertyTarget::Pick(PickerKind::Blocks))
                .locked(locked),
        ];
    }
    dependants
        .iter()
        .enumerate()
        .map(|(index, dependant)| {
            PropertyRow::new(
                if index == 0 { "Blocks" } else { "" },
                Some(link_value(
                    board,
                    Some(dependant),
                    dependant.id.as_str(),
                    false,
                )),
                PropertyTarget::Pick(PickerKind::Blocks),
            )
            .locked(locked)
        })
        .collect()
}

/// `FLT-3 · In progress`, led by `✓` when the link is already satisfied.
fn link_value(board: &Board, linked: Option<&Card>, id: &str, satisfied: bool) -> String {
    let mut value = String::new();
    if satisfied {
        value.push_str("✓ ");
    }
    match linked {
        Some(linked) => {
            value.push_str(&linked.display_key(board));
            if let Some(column) = column_name(board, linked) {
                value.push_str(" · ");
                value.push_str(column);
            }
        }
        // A link the view no longer carries still names something the reader can look up,
        // exactly as the Parent row does for a card outside the set it was handed.
        None => value.push_str(id),
    }
    value
}

/// The name of the column a card sits in.
fn column_name<'a>(board: &'a Board, card: &Card) -> Option<&'a str> {
    board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .map(|status| status.name.as_str())
}

/// Whether a card sits in a column of `category`.
fn in_category(board: &Board, card: &Card, category: StatusCategory) -> bool {
    board
        .statuses
        .iter()
        .any(|status| status.id == card.status_id && status.category == category)
}

/// What a click on property row `index` runs.
pub(crate) type OnPropertyClick = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// How one property row answers the pointer, prepared by the dialog that draws it.
pub(crate) struct PropertyRowProps {
    /// The row's position, which the click selects and the harness names.
    pub(crate) index: usize,
    /// Whether the keyboard's `j` / `k` selection is on this row.
    pub(crate) selected: bool,
    /// Whether the detail holds the keyboard (no text edit is open).
    pub(crate) focused: bool,
    /// The key that edits this row, from the live keymap, shown while the row is hovered.
    pub(crate) kbd: Option<Kbd>,
    /// Select this row and run what `⏎` runs on it.
    pub(crate) on_click: Option<OnPropertyClick>,
}

/// Whether a row answers a click: it edits something, opens something, or links somewhere.
///
/// A backend-owned row is locked instead: it wears the lock glyph and no hover, and `⏎` on it
/// still answers with the backend's sentence.
#[must_use]
pub(crate) fn is_clickable(row: &PropertyRow) -> bool {
    !row.locked && row.target != PropertyTarget::ReadOnly
}

/// Renders one property row: label, value, and the key chip the pointer sees on hover.
#[must_use]
pub(crate) fn property_row(
    row: &PropertyRow,
    props: PropertyRowProps,
    theme: &Theme,
) -> AnyElement {
    let link = matches!(
        row.target,
        PropertyTarget::Worktree | PropertyTarget::PullRequest
    );
    let tone = if link && !row.locked {
        // The worktree and the pull request are links: a click opens the session, as `o` does,
        // or the pull request in the browser, as `B` does.
        Tone::Accent
    } else {
        row.tone
    };
    let text = if row.mono {
        Text::data_small(row.value.clone()).tone(tone).ellipsize()
    } else {
        Text::ui(row.value.clone()).tone(tone).ellipsize()
    };
    // The source rides in the value column rather than a column of its own: it exists on three
    // rows out of a dozen, and a fourth column would move the lock glyph on every board.
    let value = div()
        .flex()
        .items_baseline()
        .min_w_0()
        .gap(theme.space.xs)
        .child(text)
        .children(
            row.source
                .clone()
                .map(|source| Text::caption(source).tone(Tone::Muted).flex_none()),
        );
    let clickable = is_clickable(row);
    let index = props.index;
    let trailing = if row.locked {
        Some(RowColumn::fixed_ch(
            TRAILING_COLUMN_CH,
            Icon::Lock
                .el()
                .size(IconSize::Small)
                .color(Tone::Secondary.color(theme)),
        ))
    } else {
        props.kbd.filter(|_| clickable).map(|kbd| {
            RowColumn::fixed_ch(TRAILING_COLUMN_CH, kbd.size(KbdSize::Small))
                .align(ColumnAlign::Right)
                .hover_only()
        })
    };
    Row::with_id(("card-detail-property", index))
        .selected(props.selected)
        .cursor(props.selected && props.focused)
        .hoverable(clickable)
        .column(RowColumn::fixed_ch(
            LABEL_COLUMN_CH,
            Text::ui(row.label.clone()).tone(Tone::Secondary),
        ))
        .column(RowColumn::flex(value))
        .columns(trailing)
        .when_some(props.on_click.filter(|_| clickable), |el, on_click| {
            el.on_click(move |_, window, cx| on_click(index, window, cx))
        })
        .harness_target_indexed("card_detail.property", index)
        .into_any_element()
}

/// The conflict callout of §8: the fields that differ, and the two buttons that settle it.
///
/// `field_label` lives in the core: the CLI's card report prints the same conflict list, and
/// "Conflict — status_id, due_date" is a sentence the user has to translate on either surface.
/// Each button dispatches the action its key runs, and shows that key.
#[must_use]
pub(crate) fn conflict_banner(card: &Card, theme: &Theme) -> Option<Callout> {
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
        Callout::new(
            Tone::Warning,
            Icon::TriangleAlert,
            format!(
                "Conflict \u{2014} {fields} differ from {}",
                conflict.remote.key
            ),
        )
        .actions(
            div()
                .flex()
                .gap(theme.space.xs)
                .child(
                    Button::new("card-detail-keep-local", "Keep local")
                        .size(ButtonSize::Compact)
                        .action(Box::new(card_detail::KeepLocal)),
                )
                .child(
                    Button::new("card-detail-take-remote", "Take remote")
                        .size(ButtonSize::Compact)
                        .action(Box::new(card_detail::TakeRemote)),
                ),
        ),
    )
}

/// The comments, newest last: avatar, author, age, then the body.
///
/// A run's report carries a `run {n}` badge beside its provider and folds at
/// [`REPORT_COLLAPSE_LINES`], the way the transcript folds a delivered child result: a report is
/// the whole of what a run said, and three of them would otherwise bury the card's own
/// conversation. `expanded` holds the comment ids the reader has already opened; a folded report
/// ends in a `Show more` button that runs `⏎`, which opens every folded report.
#[must_use]
pub(crate) fn comments(card: &Card, now: i64, expanded: &HashSet<String>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.md)
        .child(Text::sentence_label(format!(
            "Comments \u{00b7} {}",
            card.comments.len()
        )))
        .children(card.comments.iter().map(|comment| {
            let run = report_run(card, comment);
            let folded = run.is_some()
                && comment.body.lines().count() > REPORT_COLLAPSE_LINES
                && !expanded.contains(&comment.id);
            let author = match run {
                // A report is written by the run: its provider is the author, and its number is
                // what the rest of the card refers to it by.
                Some(index) => card.runs.get(index - 1).map_or_else(
                    || "agent".to_owned(),
                    |run| run.provider.executable().to_owned(),
                ),
                None => comment.author.clone().unwrap_or_else(|| "You".to_owned()),
            };
            let avatar = if run.is_some() {
                Avatar::new(&author).tone(Tone::Secondary)
            } else {
                Avatar::new(&author)
            };
            div()
                .flex()
                .w_full()
                .gap(theme.space.sm)
                .child(avatar)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(theme.space.xxs)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.space.xs)
                                .child(Text::ui_strong(author.clone()))
                                .children(run.map(|index| {
                                    Badge::new(format!("run {index}")).style(BadgeStyle::Outlined)
                                }))
                                .child(
                                    Text::caption(format!(
                                        "\u{00b7} {}",
                                        age_label(&comment.created_at, now)
                                    ))
                                    .tone(Tone::Muted),
                                ),
                        )
                        .child(MarkdownText::new(if folded {
                            head_lines(&comment.body)
                        } else {
                            comment.body.clone()
                        }))
                        .children(folded.then(|| {
                            div().flex().child(
                                Button::new(
                                    ("card-detail-show-more", run.unwrap_or(0)),
                                    "Show more",
                                )
                                .style(ButtonStyle::Ghost)
                                .size(ButtonSize::Compact)
                                .action(Box::new(card_detail::EditProperty)),
                            )
                        })),
                )
        }))
        .into_any_element()
}

/// The 1-based number of the run a comment reports on, or `None` for an ordinary comment.
///
/// A report whose run has already aged out of `MAX_RUNS_PER_CARD` has no number to carry, so it
/// reads as the comment it also is rather than as `run ?`.
fn report_run(card: &Card, comment: &Comment) -> Option<usize> {
    let run = comment.run_id?;
    card.runs
        .iter()
        .position(|candidate| candidate.id == run)
        .map(|index| index + 1)
}

/// The first [`REPORT_COLLAPSE_LINES`] lines of a report.
fn head_lines(body: &str) -> String {
    body.lines()
        .take(REPORT_COLLAPSE_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The report comments still folded, oldest first.
///
/// `⏎` opens these before it goes back to meaning "edit the selected property": a folded
/// report hides the very thing the reader opened the card for. The fold's `Show more` is drawn
/// only while one is closed, so it never names a key that would do nothing.
#[must_use]
pub(crate) fn folded_reports(card: &Card, expanded: &HashSet<String>) -> Vec<String> {
    card.comments
        .iter()
        .filter(|comment| report_run(card, comment).is_some())
        .filter(|comment| comment.body.lines().count() > REPORT_COLLAPSE_LINES)
        .filter(|comment| !expanded.contains(&comment.id))
        .map(|comment| comment.id.clone())
        .collect()
}

/// The activity section under the properties: the last [`ACTIVITY_FOLDED`] entries, newest
/// first, and the last [`ACTIVITY_ROWS`] once `open`.
///
/// `on_toggle` is the `Show all` / `Show less` button's click; it is drawn only when there is
/// more to show.
#[must_use]
pub(crate) fn activity(
    card: &Card,
    now: i64,
    open: bool,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let shown = if open { ACTIVITY_ROWS } else { ACTIVITY_FOLDED };
    let total = card.activity.len().min(ACTIVITY_ROWS);
    let entries: Vec<_> = card.activity.iter().rev().take(shown).collect();
    let toggle = (total > ACTIVITY_FOLDED).then(|| {
        Button::new(
            "card-detail-activity-toggle",
            if open {
                "Show less".to_owned()
            } else {
                format!("Show all {total}")
            },
        )
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .on_click(move |_, window, cx| on_toggle(window, cx))
    });
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.xs)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(Text::sentence_label("Activity"))
                .children(toggle),
        )
        .children(
            entries
                .is_empty()
                .then(|| Text::caption("Nothing yet.").tone(Tone::Muted)),
        )
        .children(entries.into_iter().map(|entry| {
            Text::caption(format!(
                "{} \u{00b7} {}",
                entry.message,
                age_label(&entry.at, now)
            ))
            .tone(Tone::Muted)
            .ellipsize()
        }))
        .into_any_element()
}

/// The run card between the title and the description (contracts §5.3).
///
/// Every string it draws is built in [`run_line`]; the renderer lays out a glyph, three texts and
/// the buttons, and computes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLine {
    /// The mark the board's tile draws for the same card, when it has one.
    ///
    /// `None` where the board deliberately suppresses one — a canceled run, or a success whose
    /// column already moved the card on — which still has a row here, because the detail is
    /// where a run is read rather than counted.
    pub(crate) mark: Option<RunMark>,
    /// The whole line, `working 4m · codex · gpt-5 · high`, or `pending 2m · waiting for a slot`.
    pub(crate) text: SharedString,
    /// The state and its age, sentence-cased for the card's first line: `Working 4m`.
    pub(crate) head: SharedString,
    /// What follows the head: `codex · gpt-5 · high`, or `waiting for a slot`.
    pub(crate) facts: SharedString,
    /// A finished run's usage, `12.4k tok · $0.31`, drawn at the line's right end.
    pub(crate) usage: Option<SharedString>,
}

/// The card's run line, or `None` when it has neither a run nor one owed to it.
///
/// `mark` comes from the board's own fold (`state::CardMarks`), so the tile and this row can
/// never disagree. `live` is the delegation status of a run the card still believes is live,
/// which the caller asks of the delegation mirror first and the view's join second; a live run
/// neither of them knows is still working, because the card is the durable record of whether a
/// run has ended. An owed run wins over a finished one: it is the one still moving.
#[must_use]
pub fn run_line(
    card: &Card,
    mark: Option<RunMark>,
    live: Option<DelegationStatus>,
    now: i64,
) -> Option<RunLine> {
    let (head, facts, usage) = if let Some(pending) = card.pending_run.as_ref() {
        // Which is a different fact from a slow run, and the only one the reader can act on.
        (
            head("pending", elapsed(&pending.since, None, now)),
            vec!["waiting for a slot".to_owned()],
            Vec::new(),
        )
    } else {
        let run = latest_run(card)?;
        let word = run.outcome.map_or_else(
            || live.map_or(DelegationStatus::Running.word(), DelegationStatus::word),
            RunOutcome::word,
        );
        let mut facts = vec![run.provider.executable().to_owned()];
        // A missing model or effort drops its separator with it rather than printing a dash:
        // the harness picked one, and this row states only what is known.
        facts.extend(run.model.clone());
        facts.extend(run.effort.clone());
        let mut usage = Vec::new();
        if !run.is_live() {
            usage.extend(
                run.tokens
                    .map(|tokens| format!("{} tok", format_token_count(tokens))),
            );
            usage.extend(run.cost_usd.map(|cost| format_cost(cost).to_string()));
        }
        (
            head(word, elapsed(&run.started_at, run.ended_at.as_deref(), now)),
            facts,
            usage,
        )
    };
    let text = std::iter::once(head.clone())
        .chain(facts.iter().cloned())
        .chain(usage.iter().cloned())
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    Some(RunLine {
        mark,
        text: SharedString::from(text),
        head: SharedString::from(sentence_case(&head)),
        facts: SharedString::from(facts.join(" \u{b7} ")),
        usage: (!usage.is_empty()).then(|| SharedString::from(usage.join(" \u{b7} "))),
    })
}

/// `working 4m` → `Working 4m`: the run card leads with its state as a sentence.
fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `working 4m`, or the bare word when neither end of the run can be dated.
fn head(word: &str, elapsed: Option<String>) -> String {
    elapsed.map_or_else(|| word.to_owned(), |elapsed| format!("{word} {elapsed}"))
}

/// How long a run has been going, or took.
fn elapsed(started: &str, ended: Option<&str>, now: i64) -> Option<String> {
    let started = parse_timestamp(started)?;
    let ended = match ended {
        Some(ended) => parse_timestamp(ended)?,
        None => now,
    };
    let seconds = u64::try_from(ended.saturating_sub(started)).ok()?;
    Some(format_duration(seconds.saturating_mul(1_000)).to_string())
}

/// Which of the run card's three buttons can act on this card (`CardMenu`'s answer).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RunButtons {
    /// `A`: the run has a thread to attach.
    pub(crate) attach: bool,
    /// `>`: the card's column runs an action.
    pub(crate) rerun: bool,
    /// `X`: the run is live or owed.
    pub(crate) cancel: bool,
}

/// Draws the run card: the board's own mark and the prepared line, then the buttons that act
/// on the run, each dispatching the key it shows (contracts §5.3, ADR 0023).
#[must_use]
pub(crate) fn run_row(line: &RunLine, buttons: RunButtons, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let first = div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(theme.space.xs)
        .children(line.mark.map(run_glyph))
        .child(Text::ui_strong(line.head.clone()).flex_none())
        .child(
            Text::ui(line.facts.clone())
                .tone(Tone::Secondary)
                .ellipsize(),
        )
        .child(div().flex_1())
        .children(
            line.usage
                .clone()
                .map(|usage| Text::data_small(usage).faint()),
        );
    let actions = div()
        .flex()
        .items_center()
        .gap(theme.space.xs)
        .children(buttons.attach.then(|| {
            Button::new("card-detail-run-attach", "Attach")
                .style(ButtonStyle::Primary)
                .size(ButtonSize::Compact)
                .action(Box::new(board::AttachRun))
                .harness_target("card_detail.run.attach")
        }))
        .children(buttons.rerun.then(|| {
            Button::new("card-detail-run-rerun", "Re-run")
                .size(ButtonSize::Compact)
                .action(Box::new(board::RunNow))
                .harness_target("card_detail.run.rerun")
        }))
        .children(buttons.cancel.then(|| {
            Button::new("card-detail-run-cancel", "Cancel run")
                .style(ButtonStyle::GhostDanger)
                .size(ButtonSize::Compact)
                .action(Box::new(board::CancelRun))
                .harness_target("card_detail.run.cancel")
        }));
    let has_actions = buttons.attach || buttons.rerun || buttons.cancel;
    let mut card = InfoCard::new().line(first);
    if has_actions {
        card = card.line(actions);
    }
    card.into_any_element()
}

/// The glyph a mark draws, which is the one the card tile draws for it.
///
/// `CardTile` keeps its own table private to the kit, so the two live apart; they are the same
/// three glyphs and must stay so (`docs/DESIGN-SYSTEM.md` §6).
fn run_glyph(mark: RunMark) -> AnyElement {
    match mark {
        RunMark::Pending | RunMark::Working => Spinner::new("card-detail-run")
            .size(IconSize::Small)
            .tone(Tone::Secondary)
            .into_any_element(),
        RunMark::Stalled | RunMark::NeedsYou => StatusDot::small(Tone::Warning).into_any_element(),
        RunMark::Succeeded => Icon::Check
            .el()
            .size(IconSize::Small)
            .tone(Tone::Muted)
            .into_any_element(),
    }
}
