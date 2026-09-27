//! The Columns pane: the column list and the drilled-into column's form (§5.4).

use super::*;

/// A column row's verbs: its menu lists all four, its hover actions the three with a harness
/// name. `Open` is the double-click's and `⏎`'s.
pub(in crate::dialogs::board_settings) const COLUMN_VERBS: [Verb; 4] = [
    Verb {
        label: "Open",
        icon: Icon::ChevronRight,
        action: || Box::new(dialog::Confirm),
        harness: None,
        destructive: false,
    },
    Verb {
        label: "Move up",
        icon: Icon::CircleArrowUp,
        action: || Box::new(board_settings_actions::MoveColumnUp),
        harness: Some("board_settings.up"),
        destructive: false,
    },
    Verb {
        label: "Move down",
        icon: Icon::CircleArrowDown,
        action: || Box::new(board_settings_actions::MoveColumnDown),
        harness: Some("board_settings.down"),
        destructive: false,
    },
    Verb {
        label: "Delete column",
        icon: Icon::Trash2,
        action: || Box::new(board_settings_actions::DeleteColumn),
        harness: Some("board_settings.delete"),
        destructive: true,
    },
];

/// The Columns pane: the column list, or the column the list drilled into.
pub(in crate::dialogs::board_settings) fn columns_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    cx: &App,
) -> PaneBlocks {
    match draft.opened_column {
        Some(opened) => column_form(wire, draft, opened, input),
        None => {
            let rows = draft
                .prepared
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    wire.track(
                        index,
                        column_list_row(wire, draft, row, index, index == draft.row, cx),
                    )
                })
                .collect();
            let mut pane = PaneBlocks::new();
            pane.push(
                card("board-settings-columns", Some("Columns"), rows)
                    .note("in board order \u{b7} a card can only route forward"),
                draft.prepared.len(),
            );
            pane
        }
    }
}

/// The glyph a column's category leads its list row with.
const fn category_icon(category: StatusCategory) -> Icon {
    match category {
        StatusCategory::Backlog => Icon::Dot,
        StatusCategory::Unstarted => Icon::Circle,
        StatusCategory::Started => Icon::CircleDot,
        StatusCategory::Completed => Icon::CircleCheck,
        StatusCategory::Canceled => Icon::CircleSlash,
    }
}

/// One column of the list: a click selects it, a double-click opens it, its hover actions
/// move or delete it, and a right-click offers the same verbs as a menu (ADR 0023).
///
/// While a delete waits for a target the list *is* the choice: the doomed column says so, and a
/// click on any other column moves the cards there, as `⏎` on it would.
fn column_list_row(
    wire: &Wire,
    draft: &BoardSettingsState,
    row: &ColumnRow,
    index: usize,
    cursor: bool,
    cx: &App,
) -> AnyElement {
    let ColumnValue::Column {
        category, helper, ..
    } = &row.value
    else {
        return div().into_any_element();
    };
    let theme = cx.theme();
    let tone = if cursor { Tone::Secondary } else { Tone::Muted };
    let settings_row = SettingsRow::new(("board-settings-row", index))
        .cursor(cursor)
        .leading(
            category_icon(*category)
                .el()
                .size(IconSize::Medium)
                .tone(tone),
        )
        .label(row.label.clone())
        .helper(helper.clone())
        .trailing(Icon::ChevronRight.el().size(IconSize::Small).tone(tone));
    if let Some(doomed) = draft.pending_delete {
        if doomed == index {
            return settings_row
                .disabled(true)
                .label_badge(Badge::new("deleting").style(BadgeStyle::Filled))
                .harness_target_indexed("board_settings.row", index)
                .into_any_element();
        }
        let target = wire.clone();
        return settings_row
            .hover_actions(Text::caption("Move cards here").muted())
            .on_click(move |_, window, cx| {
                choose_target(
                    &target.state,
                    &target.bridge,
                    index,
                    &target.focus,
                    window,
                    cx,
                );
            })
            .harness_target_indexed("board_settings.row", index)
            .into_any_element();
    }
    let last = draft.columns.len().saturating_sub(1);
    let hover = COLUMN_VERBS
        .iter()
        .filter(|verb| verb.harness.is_some())
        .map(|verb| {
            let wire = wire.clone();
            // Moving past an end does nothing, so the button says so.
            let disabled = match verb.harness {
                Some("board_settings.up") => index == 0,
                Some("board_settings.down") => index >= last,
                _ => false,
            };
            IconButton::new(
                (
                    "board-settings-column-verb",
                    index * COLUMN_VERBS.len() + hover_slot(verb),
                ),
                verb.icon,
                verb.label,
            )
            .size(ButtonSize::Compact)
            .disabled(disabled)
            // The button acts on the cursor row, so a click lands the cursor first.
            .on_click(move |_, window, cx| {
                select_row(&wire.state, index, &wire.focus, window, cx);
            })
            .action((verb.action)())
            .harness_target_named(verb.harness.filter(|_| cursor))
        });
    let settings_row = settings_row
        .hover_actions(
            div()
                .flex()
                .items_center()
                .gap(theme.space.xxs)
                .children(hover),
        )
        .on_click(wire.select(index))
        .on_double_click(wire.open(index))
        // The menu acts on the cursor row, so a right-click lands the cursor first.
        .on_secondary_click(wire.select(index))
        .harness_target_indexed("board_settings.row", index);
    ContextMenu::new(("board-settings-column-menu", index), settings_row)
        .menu(|menu, _, _| verb_menu(menu, &COLUMN_VERBS))
        .into_any_element()
}

/// Where a verb sits in its table, for a stable element id per row and verb.
fn hover_slot(verb: &Verb) -> usize {
    COLUMN_VERBS
        .iter()
        .position(|entry| entry.label == verb.label)
        .unwrap_or(0)
}

/// The drilled-into column: a breadcrumb, then its rows in cards (§5.4).
///
/// On a board that may not carry automation the two automation cards are replaced by one
/// callout; the rows are folded away, not greyed one by one.
fn column_form(
    wire: &Wire,
    draft: &BoardSettingsState,
    opened: usize,
    input: Option<&Entity<TextInput>>,
) -> PaneBlocks {
    let mut pane = PaneBlocks::new();
    let Some(column) = draft.columns.get(opened) else {
        return pane;
    };
    pane.push(column_breadcrumb(column, opened, draft.columns.len()), 0);
    let mut start = 0;
    while let Some(first) = draft.prepared.get(start) {
        let SettingRow::ColumnField(field) = first.row else {
            break;
        };
        let group = field.card();
        let end = draft.prepared[start..]
            .iter()
            .position(
                |row| !matches!(row.row, SettingRow::ColumnField(next) if next.card() == group),
            )
            .map_or(draft.prepared.len(), |offset| start + offset);
        let rows = draft.prepared[start..end]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                let index = start + offset;
                settings_row(
                    wire,
                    column_field_row(row, index, index == draft.row),
                    input,
                )
            })
            .collect();
        pane.push(
            card(("board-settings-column-card", start), group.title(), rows),
            end - start,
        );
        start = end;
    }
    if draft.automation_locked {
        let linked = draft
            .board
            .as_ref()
            .is_some_and(|board| board.backend.kind != BackendRef::LOCAL);
        let (headline, detail) = if linked {
            (
                "Automation is available on local boards.",
                "This board's columns follow its backend, so they only name and order cards.",
            )
        } else {
            (
                "Automation is available on worktree boards.",
                "This board runs in the context, so its columns only name and order cards.",
            )
        };
        pane.push(
            div()
                .flex_none()
                .child(Callout::new(Tone::Warning, Icon::Lock, headline).detail(detail)),
            0,
        );
    }
    pane
}

/// The column form's breadcrumb: `‹ Columns  In review [started]  4 of 6` (§5.4).
#[must_use]
pub(in crate::dialogs::board_settings) fn column_breadcrumb(
    column: &ColumnDraft,
    opened: usize,
    total: usize,
) -> Breadcrumb {
    Breadcrumb::new(
        "board-settings-crumb",
        "Columns",
        column.status.name.clone(),
    )
    .back_action(Box::new(dialog::Cancel))
    .badge(Badge::new(category_word(column.status.category)).style(BadgeStyle::Filled))
    .trailing(format!("{} of {total}", opened + 1))
    .harness_back("board_settings.back")
}

/// One row of the column form, as the control its prepared value calls for.
fn column_field_row(row: &ColumnRow, index: usize, cursor: bool) -> RowSpec {
    let SettingRow::ColumnField(field) = row.row else {
        return RowSpec::new(
            index,
            cursor,
            row.label.clone(),
            Control::Fact(String::new()),
        );
    };
    let control = match &row.value {
        ColumnValue::Choice {
            value,
            options,
            details,
            at,
            other,
        } => Control::Choice {
            value: value.clone(),
            options: options.clone(),
            details: details.clone(),
            at: *at,
            other: *other,
            dropdown: false,
        },
        ColumnValue::Text {
            value,
            mono,
            multiline,
        } => Control::Box(
            BoxValue::text(value.clone(), field.placeholder())
                .mono(*mono)
                .multiline(*multiline),
        ),
        ColumnValue::Column { .. } => Control::Fact(String::new()),
    };
    RowSpec::new(index, cursor, row.label.clone(), control).helper(field.helper())
}
