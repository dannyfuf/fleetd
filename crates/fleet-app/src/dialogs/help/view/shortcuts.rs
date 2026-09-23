//! Help's All shortcuts tab: the Where list with its legend, and the searchable table.

use std::rc::Rc;

use fleet_ui_kit::{
    Chip, EmptyState, Icon, IconSize, Kbd, KbdSize, ListView, Row, RowColumn, Text, Tone,
    prelude::*,
};
use gpui::{AnyElement, App, Entity, div};

use super::{edit, heading, open_guide};
use crate::{
    action_catalogue::Place,
    dialogs::{
        HELP_KEYS_COL_W, HELP_WHERE_COL_W,
        help::{
            guides::GUIDES,
            model::{HelpState, shortcuts},
            run,
        },
        notify, with_host,
    },
    state::AppState,
};

/// The left column of All shortcuts: every place with its match count, then the legend.
pub(super) fn places_sidebar(help: &HelpState, state: &Entity<AppState>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let all_state = state.clone();
    let all = Row::new()
        .selected(help.place.is_none())
        .column(RowColumn::flex(Text::ui("All places")))
        .column(RowColumn::auto(Text::caption(
            help.results
                .counts
                .first()
                .copied()
                .unwrap_or(0)
                .to_string(),
        )))
        .on_click(move |_, _, cx| edit(&all_state, cx, |help| help.set_place(None)))
        .harness_target_indexed("help.place", 0);
    let places = Place::ALL.iter().enumerate().map(|(ix, place)| {
        let place = *place;
        let place_state = state.clone();
        let here = help.here.place == Some(place);
        Row::new()
            .selected(help.place == Some(place))
            .column(RowColumn::flex(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .min_w_0()
                    .child(Text::ui(place.title()).ellipsize())
                    .children(here.then(|| Chip::new().text("you are here").tone(Tone::Accent))),
            ))
            .column(RowColumn::auto(Text::caption(
                help.results
                    .counts
                    .get(ix + 1)
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
            )))
            .on_click(move |_, _, cx| {
                edit(&place_state, cx, |help| help.set_place(Some(place)));
            })
            .harness_target_indexed("help.place", ix + 1)
    });
    let legend = crate::dialogs::help::model::legend()
        .iter()
        .map(|(kbd, text)| {
            div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(kbd.clone().size(KbdSize::Small))
                .child(Text::caption(text.clone()))
        });
    div()
        .id("help-places")
        .flex()
        .flex_col()
        .flex_none()
        .w(theme.metrics.sidebar_w)
        .h_full()
        .overflow_y_scroll()
        .px(theme.space.md)
        .pb(theme.space.md)
        .bg(theme.colors.surface)
        .border_r(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(heading("Where", None, cx))
        .child(all)
        .children(places)
        .child(div().flex_1().min_h(theme.space.md))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.space.xs)
                .p(theme.space.sm)
                .rounded(theme.radii.card)
                .border(theme.metrics.hairline)
                .border_color(theme.colors.border)
                .bg(theme.colors.elevated)
                .child(Text::ui_strong("Reading the keys"))
                .children(legend),
        )
        .into_any_element()
}

/// The right column of All shortcuts: the table, and the guide a search points at.
pub(super) fn shortcuts_content(
    help: &HelpState,
    state: &Entity<AppState>,
    run_chip: Option<Kbd>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let header = Row::new()
        .hoverable(false)
        .column(RowColumn::flex(Text::caption("Action")))
        .column(RowColumn::fixed(HELP_WHERE_COL_W, Text::caption("Where")))
        .column(
            RowColumn::fixed(HELP_KEYS_COL_W, Text::caption("Keys"))
                .align(fleet_ui_kit::ColumnAlign::Right),
        );
    let rows: Rc<[usize]> = help.results.shortcuts.as_slice().into();
    let here = Rc::clone(&help.here);
    let select_rows = Rc::clone(&rows);
    let select_state = state.clone();
    let list = ListView::new("help-shortcuts", rows.len(), move |ix, is_cursor, _, cx| {
        let Some(row) = rows.get(ix).and_then(|shortcut| shortcuts().get(*shortcut)) else {
            return div().into_any_element();
        };
        let gap = cx.theme().space.xs;
        let runnable = row.runnable(&here).is_some();
        let keys = div()
            .flex()
            .items_center()
            .gap(gap)
            .children(row.kbd.clone().map(|kbd| kbd.size(KbdSize::Small)))
            .children(row.range_end.clone().map(|end| {
                div()
                    .flex()
                    .items_center()
                    .child(Text::caption("\u{2013}"))
                    .child(end.size(KbdSize::Small))
            }));
        let mut line = Row::new()
            .selected(is_cursor)
            .column(RowColumn::flex(Text::ui(row.label.clone()).ellipsize()));
        if runnable && let Some(chip) = run_chip.clone() {
            line = line.column(
                RowColumn::auto(
                    div()
                        .flex()
                        .items_center()
                        .gap(gap)
                        .child(Text::caption("Run").tone(Tone::Accent))
                        .child(chip.size(KbdSize::Small)),
                )
                .hover_only(),
            );
        }
        line.column(RowColumn::fixed(
            HELP_WHERE_COL_W,
            Text::caption(row.place_title.clone()),
        ))
        .column(RowColumn::fixed(HELP_KEYS_COL_W, keys).align(fleet_ui_kit::ColumnAlign::Right))
        .harness_target_indexed("help.shortcut", ix)
        .into_any_element()
    })
    .cursor(help.cursor)
    .track_scroll(&help.table_scroll)
    .on_select(move |ix, _, cx| {
        let Some(shortcut) = select_rows.get(ix).copied() else {
            return;
        };
        let action = with_host(&select_state, cx, |host| {
            let help = host.help.as_mut()?;
            help.set_cursor(ix);
            shortcuts().get(shortcut)?.runnable(&help.here)
        });
        match action {
            // A row that works where Help was opened runs on the click, like `⏎` on it.
            Some(action) => run(&select_state, action, cx),
            None => notify(&select_state, cx),
        }
    })
    .empty(EmptyState::new(format!(
        "Nothing matches \u{201c}{}\u{201d} here.",
        help.query.trim()
    )));

    let related = help.related_guide().and_then(|guide| {
        let entry = GUIDES.get(guide)?;
        let open_state = state.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(theme.space.sm)
                .pt(theme.space.md)
                .border_t(theme.metrics.hairline)
                .border_color(theme.colors.border)
                .child(Text::sentence_label("Related guide"))
                .child(
                    Row::with_id("help-related")
                        .comfortable()
                        .leading(
                            entry
                                .icon
                                .el()
                                .size(IconSize::Medium)
                                .color(theme.colors.accent),
                        )
                        .column(RowColumn::flex(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(theme.space.sm)
                                .min_w_0()
                                .child(Text::ui_strong(entry.title))
                                .child(Text::caption(entry.summary).ellipsize()),
                        ))
                        .column(RowColumn::auto(
                            Icon::ChevronRight.el().size(IconSize::Small),
                        ))
                        .on_click(move |_, _, cx| open_guide(&open_state, guide, cx))
                        .harness_target("help.related"),
                ),
        )
    });

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .p(theme.space.md)
        .gap(theme.space.xs)
        .child(header)
        .child(div().flex_1().min_h_0().child(list))
        .children(related)
        .into_any_element()
}
