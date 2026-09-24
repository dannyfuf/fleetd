//! Settings' card: the search header, the section rail, the pane of controls and the footer.
//!
//! Composes what the draft prepared (`SettingsState::prepared`, the search hits); it builds no
//! rows and reads no configuration itself.

use fleet_ui_kit::{
    Button, ButtonSize, ButtonStyle, Dialog, EmptyState, FactRow, FactValue, IconButton, IconSize,
    Kbd, Row, RowColumn, Text, Tone, ValueField,
};
use gpui::{MouseButton, SharedString};

use super::*;
use crate::dialogs::{Dialogs, SETTINGS_SEARCH_W};

/// The footer's dirty status: one word beside the amber strip that spells out what it means.
const UNSAVED: &str = "Unsaved";

/// What a row's controls call back into: the draft and the dialog's own focus handle.
#[derive(Clone)]
struct Handlers {
    state: Entity<AppState>,
    focus: FocusHandle,
}

/// Renders the dialog (§3.8.6).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let handlers = Handlers {
        state: state.clone(),
        focus: focus.clone(),
    };
    let cx: &App = cx;
    let theme = cx.theme();
    let host_ref = host.read(cx);
    let draft = &host_ref.settings;
    let dirty = draft.dirty();
    let load_failed = draft.config_error.is_some();
    let viewport = window.viewport_size();
    let width = Dialogs::Settings
        .width(cx)
        .min(viewport.width - theme.space.xl * 2.0);
    let height = Dialogs::Settings
        .height()
        .unwrap_or(viewport.height)
        .min(viewport.height - theme.space.xl * 2.0);
    // Keys are resolved against the dialog's own handle. While the search field or a row
    // editor holds the keyboard, `E` and `D` type instead, so their tooltips drop the key; the
    // buttons still work by pointer.
    let chip = |action: &dyn gpui::Action| Kbd::for_action_in(action, focus, window);

    let search = div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .w(SETTINGS_SEARCH_W)
        .children(host_ref.settings_search.clone().map(|input| {
            div()
                .flex_1()
                .min_w_0()
                .child(input.harness_target("settings.search"))
        }));

    let pane = match draft.search.as_ref().filter(|search| search.active()) {
        Some(search) => hits_pane(search, &draft.hit_scroll, &handlers, cx).into_any_element(),
        None => {
            section_pane(draft, host_ref.settings_input.clone(), &handlers, cx).into_any_element()
        }
    };

    let body = div()
        .flex()
        .size_full()
        .min_h_0()
        .child(rail(draft, &handlers, cx))
        .child(pane);

    let config_button = Button::new("settings-config", "Open config.json")
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .icon(Icon::ExternalLink)
        .action(Box::new(settings_actions::OpenConfigFile))
        .map(|button| match chip(&settings_actions::OpenConfigFile) {
            Some(kbd) => button.kbd(kbd),
            None => button,
        });
    let doctor_button = Button::new("settings-doctor", "Run doctor")
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .icon(Icon::Wrench)
        .action(Box::new(settings_actions::RunDoctor))
        .map(|button| match chip(&settings_actions::RunDoctor) {
            Some(kbd) => button.kbd(kbd),
            None => button,
        });
    let footer_start = div()
        .flex()
        .flex_1()
        .min_w_0()
        // The footer's own `md` gap keeps this group off Cancel only while nothing in it paints
        // past its box: the status is one word so it fits beside the two buttons at the
        // dialog's width, and on a narrower one it gives way (ellipsised) instead of running
        // into Cancel. The amber strip above says the rest.
        .overflow_hidden()
        .items_center()
        .gap(theme.space.sm)
        .child(config_button.harness_target("settings.config"))
        .child(doctor_button.harness_target("settings.doctor"))
        .child(div().flex_1())
        .children(dirty.then(|| {
            div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .items_center()
                .gap(theme.space.xs)
                .child(Icon::Dot.el().size(IconSize::Small).tone(Tone::Warning))
                .child(Text::ui(UNSAVED).tone(Tone::Warning).ellipsize())
        }));

    let cancel = Button::new("settings-cancel", "Cancel")
        .action(Dialogs::Settings.dismiss_action())
        .map(|button| match chip(&dialog::Cancel) {
            Some(kbd) => button.kbd(kbd),
            None => button,
        });
    let save_state = state.clone();
    let save_bridge = bridge.clone();
    // Save is the primary once there is something to save; a failed load turns it into the
    // retry `Enter` already is.
    let save_button = Button::new("settings-save", if load_failed { "Retry" } else { "Save" })
        .style(ButtonStyle::Primary)
        .disabled(!(dirty || load_failed) || draft.save_in_flight)
        .on_click(move |_, _, cx| save(&save_state, &save_bridge, cx))
        .map(|button| match chip(&dialog::Confirm) {
            Some(kbd) => button.kbd(kbd),
            None => button,
        });

    let mut card = Dialog::new("Settings")
        .dismiss_action(Dialogs::Settings.dismiss_action())
        .icon(Icon::Settings2)
        .width(width)
        .height(height)
        .header_actions(search)
        .flush_body(true)
        .body(body)
        .footer_start(footer_start)
        .actions(vec![cancel, save_button]);
    if let Some(message) = draft.error.clone().or_else(|| draft.load_error()) {
        card = card.error(message);
    } else if dirty {
        // `Esc` keeps discarding, as it always has; the strip says so before it happens.
        card = card.warning("Esc or Cancel discards the unsaved changes.");
    }

    let save_state = state.clone();
    let save_bridge = bridge.clone();
    let doctor_bridge = bridge.clone();

    input_actions(root(focus), state, focus)
        .on_action({
            let focus = focus.clone();
            move |_: &dialog::Confirm, window, cx| {
                // In the search field `Enter` jumps to the selected hit.
                if read_host(&save_state, cx, |host, _| host.settings.search_focused) {
                    jump_to_hit(&save_state, None, &focus, window, cx);
                    return;
                }
                // §3.8.6: a text or number row is opened for editing by `Enter`; once its
                // editor owns the keyboard the dialog publishes `SettingsEditing`, whose own
                // `Enter` row reaches this handler again and saves. Every other row saves.
                if confirm_opens_editing(&save_state, window, cx) {
                    return;
                }
                save(&save_state, &save_bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &settings_actions::OpenConfigFile, _window, cx| {
                open_config_file(&state, &bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::RunDoctor, _window, cx| {
                run_doctor(&state, &doctor_bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &settings_actions::Search, window, cx| {
                begin_search(&state, &focus, window, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &settings_actions::EndSearch, window, cx| {
                end_search(&state, &focus, window, cx);
            }
        })
        .child(card)
        .into_any_element()
}

/// The section rail: one row per section, its glyph and its name.
fn rail(draft: &SettingsState, handlers: &Handlers, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(RAIL_WIDTH))
        .h_full()
        .gap(theme.space.xxs)
        .p(theme.space.sm)
        .bg(theme.colors.surface)
        .border_r(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .children(Section::ALL.iter().enumerate().map(|(index, entry)| {
            let selected = index == draft.section;
            let handlers = handlers.clone();
            Row::with_id(("settings-section", index))
                .selected(selected)
                .leading(entry.icon().el().size(IconSize::Small).tone(if selected {
                    Tone::Default
                } else {
                    Tone::Secondary
                }))
                .column(RowColumn::flex(Text::ui(entry.title()).tone(if selected {
                    Tone::Default
                } else {
                    Tone::Secondary
                })))
                .on_click(move |_, window, cx| {
                    goto_row(&handlers.state, index, 0, &handlers.focus, window, cx);
                })
                .harness_target_indexed("settings.section", index)
        }))
}

/// The pane for the section the rail highlights: its rows, a card per group of them.
fn section_pane(
    draft: &SettingsState,
    editing: Option<Entity<TextInput>>,
    handlers: &Handlers,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let rows = &draft.prepared;
    let section = draft.current_section();
    let mut blocks: Vec<AnyElement> = Vec::new();
    let mut index = 0;
    while let Some(row) = rows.get(index) {
        let element = |index: usize, row: &SettingRow| {
            let focused = index == draft.row;
            row_element(
                row,
                index,
                focused,
                focused.then(|| editing.clone()).flatten(),
                handlers,
                cx,
            )
        };
        match row.card {
            Some(title) => {
                let start = index;
                while rows.get(index).is_some_and(|next| next.card == Some(title)) {
                    index += 1;
                }
                blocks.push(card(
                    title,
                    rows.iter()
                        .enumerate()
                        .skip(start)
                        .take(index - start)
                        .map(|(index, row)| element(index, row)),
                    cx,
                ));
            }
            None => {
                blocks.push(element(index, row));
                index += 1;
            }
        }
    }

    div()
        .id("settings-pane")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .gap(theme.space.md)
        .p(theme.space.lg)
        .children(blocks)
        .children((!section.editable()).then(|| {
            Text::caption("Change these in config.json.")
                .faint()
                .into_any_element()
        }))
}

/// A group of rows under a heading (`Claude`), drawn as one bordered card.
fn card(title: &'static str, rows: impl Iterator<Item = AnyElement>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(theme.space.xxs)
        .py(theme.space.sm)
        .rounded(theme.radii.card)
        .border(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .child(
            div()
                .px(theme.space.md)
                .pb(theme.space.xs)
                .child(Text::ui_strong(title)),
        )
        .children(rows)
        .into_any_element()
}

/// Draws one row with the control its kind calls for, every control wired to the pointer.
///
/// `editing` is the live editor of §3.8.6, present only on the row that `Enter` (or a click on
/// its box) opened; text and number rows draw it inside their own box.
fn row_element(
    row: &SettingRow,
    index: usize,
    focused: bool,
    editing: Option<Entity<TextInput>>,
    handlers: &Handlers,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    // The controls of the row under the cursor carry the harness names a scenario clicks.
    let named = focused;
    let control = match &row.kind {
        RowKind::Toggle(checked) => {
            let handlers = handlers.clone();
            let mut toggle = Toggle::labeled(row.label.clone(), *checked)
                .id(("settings-switch", index))
                .focused(focused)
                .on_toggle(move |on, window, cx| {
                    set_row_switch(&handlers.state, index, on, &handlers.focus, window, cx);
                });
            // A switch's live count and a broken rule sit beside its label, where the kit puts
            // them; a helper sentence goes under the row like every other one.
            if let Some(live) = row.invalid.clone().or_else(|| {
                matches!(row.id, RowId::KeepAliveRule(_))
                    .then(|| row.detail.clone())
                    .flatten()
            }) {
                toggle = toggle.detail(live);
            }
            if named {
                toggle = toggle.harness_switch("settings.switch");
            }
            toggle.into_any_element()
        }
        RowKind::Choice {
            value,
            has_prev,
            has_next,
            off_grid,
            options,
        } => {
            let handlers = handlers.clone();
            let mut cycler = Cycler::labeled(row.label.clone(), value.clone())
                .id(("settings-choice", index))
                .options(options.iter().cloned())
                .has_prev(*has_prev)
                .has_next(*has_next)
                .off_grid(*off_grid)
                .focused(focused)
                .on_select(move |option, window, cx| {
                    select_option(&handlers.state, index, option, &handlers.focus, window, cx);
                });
            if named {
                cycler = cycler.harness("settings.option", "settings.dropdown");
            }
            cycler.into_any_element()
        }
        RowKind::Number { value, min, unit } => {
            let mut field = NumberField::labeled(row.label.clone(), *value)
                .min(*min)
                .end_aligned(true)
                .focused(focused && editing.is_none());
            if let Some(unit) = unit {
                field = field.unit(unit.clone());
            }
            if let Some(input) = editing {
                field = field.editor(input);
            }
            field.into_any_element()
        }
        RowKind::Text(value) => {
            let handlers = handlers.clone();
            let mut field = ValueField::new(("settings-text", index), value.clone())
                .label(row.label.clone())
                .label_width(px(LABEL_WIDTH))
                .mono(true)
                .focused(focused)
                .on_click(move |window, cx| {
                    click_row(&handlers.state, index, &handlers.focus, window, cx);
                });
            if let Some(placeholder) = row.placeholder {
                field = field.placeholder(placeholder);
            }
            if let Some(input) = editing {
                field = field.editor(input);
            }
            field.into_any_element()
        }
        RowKind::Fact(value) => div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .px(theme.space.md)
            .child(
                div().flex_1().min_w_0().child(
                    FactRow::new(row.label.clone(), FactValue::known(value.clone()))
                        .label_width(px(LABEL_WIDTH)),
                ),
            )
            .children(row.copy.clone().map(|text| {
                IconButton::new(("settings-copy", index), Icon::Copy, "Copy")
                    .size(ButtonSize::Compact)
                    .on_click(move |_, _, cx| copy_value(&text, cx))
                    .harness_target_indexed("settings.copy", index)
            }))
            .into_any_element(),
    };

    let helper = match &row.kind {
        // A keep-alive rule's detail is its live count, already beside the label.
        RowKind::Toggle(_) if matches!(row.id, RowId::KeepAliveRule(_)) => None,
        _ => row.detail.clone(),
    };
    let handlers = handlers.clone();
    div()
        .id(("settings-row", index))
        .flex()
        .flex_col()
        .flex_none()
        .rounded(theme.radii.sm)
        .when(focused, |el| el.bg(theme.colors.row_selected))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            click_row(&handlers.state, index, &handlers.focus, window, cx);
        })
        .child(control)
        .children(helper.map(|helper| {
            div()
                .px(theme.space.md)
                .pb(theme.space.xs)
                .child(Text::caption(helper).faint())
        }))
        .harness_target_indexed("settings.row", index)
        .into_any_element()
}

/// The pane while a search is typed: every matching row, with its section, in rail order.
fn hits_pane(
    search: &SearchState,
    scroll: &gpui::ScrollHandle,
    handlers: &Handlers,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id("settings-hits")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .gap(theme.space.xxs)
        .p(theme.space.lg)
        .when(search.hits.is_empty(), |el| {
            el.child(EmptyState::new(SharedString::from(format!(
                "No setting matches \u{201c}{}\u{201d}.",
                search.query.trim()
            ))))
        })
        .children(search.hits.iter().enumerate().map(|(index, hit)| {
            let selected = index == search.cursor;
            let handlers = handlers.clone();
            Row::with_id(("settings-hit", index))
                .selected(selected)
                .cursor(selected)
                .leading(
                    hit.section
                        .icon()
                        .el()
                        .size(IconSize::Small)
                        .tone(Tone::Secondary),
                )
                .column(RowColumn::flex(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(Text::ui(hit.label.clone()).ellipsize())
                        .children(
                            hit.detail
                                .clone()
                                .map(|detail| Text::caption(detail).faint().ellipsize()),
                        ),
                ))
                .column(RowColumn::auto(
                    Text::caption(hit.section.title()).tone(Tone::Secondary),
                ))
                .height(theme.metrics.row_h_comfortable)
                .on_click(move |_, window, cx| {
                    jump_to_hit(&handlers.state, Some(index), &handlers.focus, window, cx);
                })
                .harness_target_indexed("settings.hit", index)
        }))
}

pub(super) fn input_actions(
    root: gpui::Div,
    state: &Entity<AppState>,
    focus: &FocusHandle,
) -> gpui::Div {
    root.on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &settings_actions::MoveDown, window, cx| move_row(&state, 1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &settings_actions::MoveUp, window, cx| move_row(&state, -1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::CursorDown, window, cx| {
            if searching(&state, cx) {
                move_hit(&state, 1, cx);
            } else {
                move_row(&state, 1, &focus, window, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::CursorUp, window, cx| {
            if searching(&state, cx) {
                move_hit(&state, -1, cx);
            } else {
                move_row(&state, -1, &focus, window, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::NextField, window, cx| move_section(&state, 1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::PrevField, window, cx| move_section(&state, -1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CyclePrev, _window, cx| cycle_row(&state, -1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CycleNext, _window, cx| cycle_row(&state, 1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::Toggle, _window, cx| toggle_row(&state, cx)
    })
}
