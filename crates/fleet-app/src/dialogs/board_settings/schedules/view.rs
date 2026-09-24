//! Draws the Schedules pane from the prepared list and form; nothing here formats a value.

use super::*;
use crate::dialogs::board_settings::view::Wire;

/// The Schedules pane: the board's schedules, or the one the list drilled into.
pub(in crate::dialogs::board_settings) fn schedules_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let space = &cx.theme().space;
        (space.sm, space.xs)
    };
    let pane = &draft.schedules;
    let mut column = div().flex().flex_col().gap(gap);
    if let Some(form) = pane.form.as_ref() {
        column =
            column
                .child(Text::hint(if form.id.is_some() {
                    format!("Schedules \u{203a} {}", form.baseline.name)
                } else {
                    "Schedules \u{203a} New schedule".to_owned()
                }))
                .children(pane.prepared.iter().enumerate().map(|(index, row)| {
                    form_row(wire, draft, row, index, index == draft.row, input)
                }))
                .children((!form.runs.is_empty()).then(|| last_runs(&form.runs, gap, tight)));
        return column.into_any_element();
    }
    if let Some(error) = pane.load_error.clone() {
        column = column.child(FactRow::warning(error));
    }
    if pane.list.is_empty() {
        return column
            .child(
                Text::hint(if pane.loading {
                    "loading schedules\u{2026}"
                } else {
                    "no schedules yet \u{2014} n or \u{23ce} adds one"
                })
                .muted(),
            )
            .into_any_element();
    }
    column
        .children(pane.list.iter().enumerate().map(|(index, row)| {
            list_row(
                wire,
                row,
                index,
                index == draft.row,
                pane.pending_delete.as_ref() == Some(&row.schedule.id),
                tight,
            )
        }))
        .into_any_element()
}

/// One schedule of the list: `● name   every 15m   next 14:05   last ✓ summary`.
fn list_row(
    wire: &Wire,
    row: &ScheduleListRow,
    index: usize,
    focused: bool,
    doomed: bool,
    tight: gpui::Pixels,
) -> AnyElement {
    let select = wire.clone();
    let open = wire.clone();
    Row::with_id(("board-settings-schedule", index))
        .selected(focused)
        .cursor(focused)
        .dimmed(doomed || !row.schedule.enabled)
        .column(RowColumn::auto(Text::ui(row.dot).tone(
            if row.schedule.enabled {
                Tone::Accent
            } else {
                Tone::Muted
            },
        )))
        .column(RowColumn::flex(Text::ui(row.schedule.name.clone())))
        .column(RowColumn::auto(Text::hint(row.cadence.clone()).muted()))
        .columns(
            row.next
                .clone()
                .map(|next| RowColumn::auto(Text::hint(next).muted())),
        )
        .columns(row.last.clone().map(|(outcome, summary)| {
            RowColumn::auto(
                div()
                    .flex()
                    .items_center()
                    .gap(tight)
                    .child(Text::hint("last").muted())
                    .child(Text::hint(outcome.glyph).tone(outcome.tone))
                    .child(Text::hint(summary)),
            )
        }))
        .columns(doomed.then(|| RowColumn::auto(Badge::new("deleting"))))
        .on_click(move |_, window, cx| {
            select_row(&select.state, index, &select.focus, window, cx);
        })
        .on_double_click(move |_, window, cx| {
            select_row(&open.state, index, &open.focus, window, cx);
            confirm_schedule(&open.state, window, &open.focus, cx);
        })
        .into_any_element()
}

/// One prepared row of the form, as the control its value calls for.
fn form_row(
    wire: &Wire,
    draft: &BoardSettingsState,
    row: &ScheduleFormRow,
    index: usize,
    focused: bool,
    input: Option<&Entity<TextInput>>,
) -> AnyElement {
    let label = row.field.label();
    match &row.value {
        ScheduleValue::Choice { value, options, at } => wire.row(
            index,
            Cycler::labeled(label, value.clone())
                .id(("board-settings-schedule-field", index))
                .options(options.iter().cloned())
                .on_select(wire.pick(index, Some(*at)))
                .label_width(px(LABEL_WIDTH))
                .has_prev(*at > 0)
                .has_next(*at + 1 < options.len())
                .disabled(draft.schedules.busy)
                .focused(focused),
        ),
        ScheduleValue::Flag(on) => wire.row(
            index,
            Toggle::labeled(label, *on)
                .id(("board-settings-schedule-field", index))
                .label_width(px(LABEL_WIDTH))
                .on_toggle(wire.switch(index))
                .focused(focused),
        ),
        ScheduleValue::Text { value } => {
            if focused
                && draft.editing
                && let Some(input) = input
            {
                return input.clone().into_any_element();
            }
            wire.row(
                index,
                FactRow::new(
                    label,
                    FactValue::from_option((!value.is_empty()).then(|| value.clone())),
                )
                .label_width(px(LABEL_WIDTH))
                .mono(row.field.is_mono()),
            )
        }
    }
}

/// The read-only `Last runs` block: outcome, when and summary, then the log path.
fn last_runs(runs: &[RunLine], gap: gpui::Pixels, tight: gpui::Pixels) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(Text::label("Last runs"))
        .children(runs.iter().map(|run| {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(tight)
                        .child(Text::ui(run.outcome.glyph).tone(run.outcome.tone))
                        .child(Text::ui(run.text.clone())),
                )
                .children(
                    run.log_path
                        .clone()
                        .map(|path| Text::data_small(path).muted()),
                )
        }))
        .into_any_element()
}
