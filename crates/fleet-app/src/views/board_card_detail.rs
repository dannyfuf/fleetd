//! The card-detail surface (BOARD §8, UX-SPEC §board).
//!
//! Two panes. The left one is the card as prose — key, title, markdown description, comments,
//! activity. The right one is the card as **facts**: one row per property, `j` / `k` selects
//! and `Enter` opens the picker that edits it. The split is the whole idea: everything on the
//! right is a value with a closed set of answers, so it never needs a text editor, and
//! everything on the left is text, so it never needs a picker.

use std::collections::HashSet;

use fleet_core::{
    agents::DelegationStatus,
    board::{
        ActionKind, Board, Card, Comment, PropertyKind, PropertySource, RunOutcome, StatusCategory,
        blocks, field_label, is_satisfied, latest_run, resolve_prefs,
    },
};
use fleet_ui_kit::{
    ActiveTheme, Badge, BadgeStyle, Banner, Icon, IconSize, KeyHintRow, MarkdownText,
    PriorityGlyph, Row, RowColumn, RunMark, SectionHeader, Spinner, StatusDot, Text, Theme, Tone,
    format_cost, format_duration, format_token_count,
};
use gpui::{AnyElement, App, SharedString, div, prelude::*, px};

use crate::{
    dialogs::card_picker::PickerKind,
    presentation::{age_label, parse_timestamp},
    // How many lines of a run's report the card shows before `⏎ expand`. The transcript's own
    // constant: a report comment is the delivered child result on another surface, so it folds
    // where the transcript folds it (contracts §5.3).
    screens::agent_thread::rows::item::DELEGATION_RESULT_COLLAPSE_LINES as REPORT_COLLAPSE_LINES,
};

#[cfg(test)]
mod tests;

/// How many activity entries §8 shows.
const ACTIVITY_ROWS: usize = 10;
/// The width of a property row's label column.
const LABEL_WIDTH: f32 = 78.0;
/// The width of an activity entry's age column, which the messages line up against.
const ACTIVITY_AGE_WIDTH: f32 = 44.0;
/// The trailing slot the lock glyph of a backend-owned row sits in, in `ch`.
const LOCK_COLUMN_CH: f32 = 2.0;

/// What `Enter` does on a property row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PropertyTarget {
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
            source: None,
        };
        // A backend-declared property carries its own `editable` flag, so the lock glyph says
        // the same thing here that `readonly_fields` says about the standard rows.
        rows.push(row.locked(!schema.editable));
    }
    rows
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

/// Renders one property row with its selection and cursor state.
#[must_use]
pub(crate) fn property_row(
    row: &PropertyRow,
    selected: bool,
    focused: bool,
    theme: &Theme,
) -> AnyElement {
    let text = if row.mono {
        Text::data_small(row.value.clone())
            .tone(row.tone)
            .ellipsize()
    } else {
        Text::ui(row.value.clone()).tone(row.tone).ellipsize()
    };
    // The source rides in the value column rather than a column of its own: it exists on three
    // rows out of a dozen, and a fourth column would move the lock glyph on every board.
    let value = div()
        .flex()
        .items_baseline()
        .min_w_0()
        .gap(theme.space.xs)
        .child(text)
        .children(row.source.clone().map(Text::hint));
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
pub(crate) fn conflict_banner(card: &Card) -> Option<Banner> {
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
///
/// A run's report carries a `run {n}` badge instead of an author and folds at
/// [`REPORT_COLLAPSE_LINES`], the way the transcript folds a delivered child result: a report is
/// the whole of what a run said, and three of them would otherwise bury the card's own
/// conversation. `expanded` holds the comment ids the reader has already opened.
#[must_use]
pub(crate) fn comments(card: &Card, now: i64, expanded: &HashSet<String>, cx: &App) -> AnyElement {
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
            let run = report_run(card, comment);
            let folded = run.is_some()
                && comment.body.lines().count() > REPORT_COLLAPSE_LINES
                && !expanded.contains(&comment.id);
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
                        .child(match run {
                            // A report has no author worth printing: the run is who wrote it,
                            // and its number is what the rest of the card refers to it by.
                            Some(index) => Badge::new(format!("run {index}"))
                                .style(BadgeStyle::Outlined)
                                .into_any_element(),
                            None => Text::label(
                                comment.author.clone().unwrap_or_else(|| "you".to_owned()),
                            )
                            .tone(Tone::Secondary)
                            .into_any_element(),
                        })
                        .child(Text::hint(age_label(&comment.created_at, now))),
                )
                .child(MarkdownText::new(if folded {
                    head_lines(&comment.body)
                } else {
                    comment.body.clone()
                }))
                .children(folded.then(|| Text::hint("⏎ expand")))
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
/// report hides the very thing the reader opened the card for. The fold is drawn only while one
/// is closed, so the hint never names a key that would do nothing.
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

/// The last [`ACTIVITY_ROWS`] activity entries, newest first.
#[must_use]
pub(crate) fn activity(card: &Card, now: i64, cx: &App) -> AnyElement {
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
                .child(Text::hint(age_label(&entry.at, now)).w(px(ACTIVITY_AGE_WIDTH)))
                .child(Text::ui(entry.message.clone()).muted().ellipsize())
        }))
        .into_any_element()
}

/// The card's key, priority glyph and title, as the left pane's first line.
#[must_use]
pub(crate) fn title_line(board: &Board, card: &Card, cx: &App) -> AnyElement {
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

/// The run row between the title and the description (contracts §5.3).
///
/// Every string it draws is built in [`run_line`]; the renderer lays out a glyph and one line of
/// text and computes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLine {
    /// The mark the board's tile draws for the same card, when it has one.
    ///
    /// `None` where the board deliberately suppresses one — a canceled run, or a success whose
    /// column already moved the card on — which still has a row here, because the detail is
    /// where a run is read rather than counted.
    pub(crate) mark: Option<RunMark>,
    /// `working 4m · codex · gpt-5 · high`, or `pending 2m · waiting for a slot`.
    pub(crate) text: SharedString,
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
    let mut parts = Vec::new();
    if let Some(pending) = card.pending_run.as_ref() {
        parts.push(head("pending", elapsed(&pending.since, None, now)));
        // Which is a different fact from a slow run, and the only one the reader can act on.
        parts.push("waiting for a slot".to_owned());
    } else {
        let run = latest_run(card)?;
        let word = run.outcome.map_or_else(
            || live.map_or(DelegationStatus::Running.word(), DelegationStatus::word),
            RunOutcome::word,
        );
        parts.push(head(
            word,
            elapsed(&run.started_at, run.ended_at.as_deref(), now),
        ));
        parts.push(run.provider.executable().to_owned());
        // A missing model or effort drops its separator with it rather than printing a dash:
        // the harness picked one, and this row states only what is known.
        parts.extend(run.model.clone());
        parts.extend(run.effort.clone());
        if !run.is_live() {
            parts.extend(
                run.tokens
                    .map(|tokens| format!("{} tok", format_token_count(tokens))),
            );
            parts.extend(run.cost_usd.map(|cost| format_cost(cost).to_string()));
        }
    }
    Some(RunLine {
        mark,
        text: SharedString::from(parts.join(" · ")),
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

/// Draws the prepared run line: the board's own mark, then the line.
#[must_use]
pub(crate) fn run_row(line: &RunLine, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(theme.space.xs)
        .children(line.mark.map(run_glyph))
        .child(
            Text::ui(line.text.clone())
                .tone(Tone::Secondary)
                .ellipsize(),
        )
        .into_any_element()
}

/// What the run row's keys do, drawn under it (contracts §5.3, P9-T01).
///
/// Drawn only beside a run, and only now that `A`, `X` and `>` are bound on this surface: a
/// hint that names a key nothing has implemented is worse than no hint at all. `>` says
/// `re-run` here, where a run already exists; the dialog's own footer says `run`, because it is
/// drawn on cards that have never run.
#[must_use]
pub(crate) fn run_hints() -> KeyHintRow {
    KeyHintRow::new()
        .key("A", "attach")
        .key("X", "cancel")
        .key(">", "re-run")
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
