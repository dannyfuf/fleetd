//! Draws the Schedules pane from the prepared list and form; nothing here formats a value.

use gpui::ClipboardItem;

use super::*;
use crate::dialogs::board_settings::view::{
    BoxValue, Control, PaneBlocks, RowSpec, Verb, Wire, card, settings_row, verb_menu,
};

/// A schedule row's verbs: its menu lists all three, and *Run now* is also its hover action.
/// `Open` is the double-click's and `⏎`'s; *Delete* is `d`, and the footer's.
pub(in crate::dialogs::board_settings) const SCHEDULE_VERBS: [Verb; 3] = [
    Verb {
        label: "Open",
        icon: Icon::ChevronRight,
        action: || Box::new(dialog::Confirm),
        harness: None,
        destructive: false,
    },
    Verb {
        label: "Run now",
        icon: Icon::Zap,
        action: || Box::new(board_settings_actions::RunScheduleNow),
        harness: Some("board_settings.run"),
        destructive: false,
    },
    Verb {
        label: "Delete",
        icon: Icon::Trash2,
        action: || Box::new(board_settings_actions::DeleteColumn),
        harness: None,
        destructive: true,
    },
];

/// The Schedules pane: the board's schedules, or the one the list drilled into (§5.4).
pub(in crate::dialogs::board_settings) fn schedules_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    cx: &App,
) -> PaneBlocks {
    let pane = &draft.schedules;
    if let Some(form) = pane.form.as_ref() {
        return form_pane(wire, draft, form, input, cx);
    }
    let mut blocks = PaneBlocks::new();
    if let Some(error) = pane.load_error.clone() {
        blocks.push(Callout::new(Tone::Danger, Icon::TriangleAlert, error), 0);
    }
    let list = SettingsCard::new("board-settings-schedules")
        .title("Schedules")
        .note(pane.note.clone());
    if pane.list.is_empty() {
        // The card's caption line is where an empty list says so, and what adds one.
        blocks.push(
            list.caption(
                Text::caption(if pane.loading {
                    "loading schedules\u{2026}"
                } else {
                    "no schedules yet \u{2014} n or \u{23ce} adds one"
                })
                .muted(),
            ),
            0,
        );
        return blocks;
    }
    let rows = pane
        .list
        .iter()
        .enumerate()
        .map(|(index, row)| {
            wire.track(
                index,
                list_row(
                    wire,
                    row,
                    index,
                    index == draft.row,
                    pane.pending_delete.as_ref() == Some(&row.schedule.id),
                    cx,
                ),
            )
        })
        .collect::<Vec<_>>();
    blocks.push(list.rows(rows), pane.list.len());
    blocks
}

/// One schedule of the list: its switch, its name over `every 15 min · claude · full access`,
/// and the two-line status column — `next 14:05` over the last run's outcome.
///
/// *Run now* is the row's hover action, and the right-click menu offers it beside *Open* and
/// *Delete*, so nothing lives only behind hover (ADR 0023).
fn list_row(
    wire: &Wire,
    row: &ScheduleListRow,
    index: usize,
    cursor: bool,
    doomed: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let enabled = row.schedule.enabled;
    let chevron = if cursor { Tone::Secondary } else { Tone::Muted };
    let toggle = wire.clone();
    let switch = Switch::new(("board-settings-schedule-switch", index), enabled)
        .name(row.schedule.name.clone())
        .on_toggle(move |_, window, cx| {
            select_row(&toggle.state, index, &toggle.focus, window, cx);
            toggle_listed(&toggle.state, &toggle.bridge, cx);
        })
        .harness_target_named(cursor.then_some("board_settings.switch"));
    let status = div()
        .flex()
        .flex_col()
        .items_end()
        .gap(theme.space.xxs)
        .child(Text::caption(row.status.clone()))
        .children(row.last.clone().map(|(outcome, summary)| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(outcome.icon.el().size(IconSize::Small).tone(outcome.tone))
                .child(Text::caption(summary).muted())
        }));
    let hover = SCHEDULE_VERBS
        .iter()
        .enumerate()
        .filter(|(_, verb)| verb.harness.is_some())
        .map(|(slot, verb)| {
            let wire = wire.clone();
            Button::new(
                (
                    "board-settings-schedule-verb",
                    index * SCHEDULE_VERBS.len() + slot,
                ),
                verb.label,
            )
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .icon(verb.icon)
            // The button acts on the cursor row, so a click lands the cursor first.
            .on_click(move |_, window, cx| {
                select_row(&wire.state, index, &wire.focus, window, cx);
            })
            .action((verb.action)())
            .harness_target_named(verb.harness.filter(|_| cursor))
        });
    let settings_row = SettingsRow::new(("board-settings-row", index))
        .cursor(cursor)
        // A disabled schedule dims; the row still answers the pointer below, because the switch
        // that turns it back on and the menu that opens it are both on this row.
        .disabled(!enabled)
        .leading(switch)
        .label(row.schedule.name.clone())
        .when(doomed, |settings_row| {
            settings_row.label_badge(Badge::new("deleting").style(BadgeStyle::Filled))
        })
        .helper(row.helper.clone())
        .hover_actions(div().flex().items_center().children(hover))
        .control(status)
        .trailing(Icon::ChevronRight.el().size(IconSize::Small).tone(chevron));
    let select = wire.select(index);
    let open = wire.open(index);
    let menu_select = wire.select(index);
    // The handlers sit on a wrapper rather than on the row, which drops them while disabled:
    // a disabled schedule is still one the user opens, runs or deletes.
    let pressable = div()
        .id(("board-settings-schedule", index))
        .w_full()
        .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
            if event.click_count >= 2 {
                open(event, window, cx);
            } else {
                select(event, window, cx);
            }
        })
        // The menu acts on the cursor row, so a right-click lands the cursor first.
        .on_mouse_down(gpui::MouseButton::Right, move |event, window, cx| {
            menu_select(event, window, cx);
        })
        .child(settings_row.harness_target_indexed("board_settings.row", index));
    ContextMenu::new(("board-settings-schedule-menu", index), pressable)
        .menu(|menu, _, _| verb_menu(menu, &SCHEDULE_VERBS))
        .into_any_element()
}

/// The open schedule: a breadcrumb, its rows in cards, then its last runs (§5.4).
fn form_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    form: &ScheduleForm,
    input: Option<&Entity<TextInput>>,
    cx: &App,
) -> PaneBlocks {
    let pane = &draft.schedules;
    let mut blocks = PaneBlocks::new();
    let (name, next) = match form.id.as_ref() {
        Some(id) => (
            form.baseline.name.clone(),
            pane.list
                .iter()
                .find(|row| &row.schedule.id == id)
                .and_then(|row| row.next.clone()),
        ),
        None => (NEW_SCHEDULE.to_owned(), None),
    };
    blocks.push(
        Breadcrumb::new("board-settings-crumb", "Schedules", name)
            .back_action(Box::new(dialog::Cancel))
            .harness_back("board_settings.back")
            .when_some(next, Breadcrumb::trailing),
        0,
    );
    let mut start = 0;
    while let Some(first) = pane.prepared.get(start) {
        let group = first.field.card();
        let end = pane.prepared[start..]
            .iter()
            .position(|row| row.field.card() != group)
            .map_or(pane.prepared.len(), |offset| start + offset);
        let rows = pane.prepared[start..end]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                let index = start + offset;
                let spec = form_row(row, index, index == draft.row, pane.busy);
                settings_row(wire, spec, input)
            })
            .collect();
        blocks.push(
            card(("board-settings-schedule-card", start), group.title(), rows),
            end - start,
        );
        start = end;
    }
    if !form.runs.is_empty() {
        blocks.push(last_runs(form, cx), 0);
    }
    blocks
}

/// What the breadcrumb names a schedule that has not been saved yet.
const NEW_SCHEDULE: &str = "New schedule";

/// One prepared row of the form, as the control its value calls for.
fn form_row(row: &ScheduleFormRow, index: usize, cursor: bool, busy: bool) -> RowSpec {
    let field = row.field;
    let control = match &row.value {
        ScheduleValue::Choice {
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
        ScheduleValue::Flag(on) => Control::Switch(*on),
        ScheduleValue::Text { value } => {
            let value = BoxValue::text(value.clone(), field.placeholder())
                .mono(field.is_mono())
                .multiline(field.is_multiline());
            Control::Box(if field.is_number() {
                value.number(field.unit().map(SharedString::new_static))
            } else {
                value
            })
        }
    };
    RowSpec::new(index, cursor, field.label(), control)
        .helper(field.helper())
        // A request in flight freezes the choices until it answers, as the keys wait for it.
        .disabled(busy && matches!(row.value, ScheduleValue::Choice { .. }))
}

/// The read-only `Last runs` card: each run's outcome, when, summary and log (§5.4).
fn last_runs(form: &ScheduleForm, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let rows = form
        .runs
        .iter()
        .enumerate()
        .map(|(index, run)| {
            let copy = run.log_path.clone().map(|path| {
                IconButton::new(
                    ("board-settings-copy-log", index),
                    Icon::Copy,
                    "Copy log path",
                )
                .size(ButtonSize::Compact)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                })
            });
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .h(theme.metrics.row_h)
                .px(theme.space.md)
                .child(
                    run.outcome
                        .icon
                        .el()
                        .size(IconSize::Small)
                        .tone(run.outcome.tone),
                )
                .child(Text::caption(run.when.clone()).muted().flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::ui(run.summary.clone()).ellipsize()),
                )
                .children(run.log_path.clone().map(|path| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Text::data_small(path).muted().ellipsize())
                }))
                .children(copy)
                .into_any_element()
        })
        .collect::<Vec<_>>();
    SettingsCard::new("board-settings-last-runs")
        .title("Last runs")
        .note(format!(
            "{} kept here, {} in fleetd",
            form.runs.len(),
            form.kept
        ))
        .rows(rows)
        .into_any_element()
}
