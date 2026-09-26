//! Settings' card: the search header, the section rail, the pane of cards and the footer.
//!
//! Composes what the draft prepared (`SettingsState::prepared`, the search hits); it builds no
//! rows and reads no configuration itself.

use fleet_ui_kit::{
    Badge, BadgeStyle, Button, ButtonSize, ButtonStyle, Chip, Dialog, Dropdown, EmptyState,
    IconButton, IconSize, Kbd, MenuItem, Row, RowColumn, SearchField, SettingsCard, SettingsRow,
    StatusDot, Switch, Text, Tone, ValueBox, ValueBoxWidth,
};
use gpui::SharedString;

use super::*;
use crate::dialogs::{Dialogs, SETTINGS_RAIL_W, SETTINGS_SEARCH_W};

/// The footer's dirty status: one word; the strip above it speaks only when `Esc` asks.
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
    let searching = draft.search.as_ref().filter(|search| search.active());

    let search = host_ref.settings_search.clone().map(|input| {
        let clear = handlers.clone();
        let mut field = SearchField::new("settings-search", input)
            .width(SETTINGS_SEARCH_W)
            .kbd(chip(&settings_actions::Search))
            .on_clear(move |window, cx| {
                end_search(&clear.state, &clear.focus, window, cx);
            })
            .harness_clear("settings.clear");
        if let Some(search) = searching {
            field = field.count(search.hits.len(), search.total);
        }
        field.harness_target("settings.search")
    });

    let pane =
        match searching {
            Some(search) => hits_pane(search, &draft.hit_scroll, &handlers, focus, window, cx)
                .into_any_element(),
            None => section_pane(draft, host_ref.settings_input.clone(), &handlers, cx)
                .into_any_element(),
        };

    let body = div()
        .flex()
        .size_full()
        .min_h_0()
        .child(rail(draft, searching.is_some(), &handlers, cx))
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
        // into Cancel.
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
                .child(StatusDot::small(Tone::Warning))
                .child(Text::ui(UNSAVED).tone(Tone::Warning).ellipsize())
        }));

    let cancel_button = Button::new("settings-cancel", "Cancel")
        .action(Dialogs::Settings.dismiss_action())
        .map(|button| match chip(&dialog::Cancel) {
            Some(kbd) => button.kbd(kbd),
            None => button,
        });
    let save_state = state.clone();
    let save_bridge = bridge.clone();
    // Save is the primary once there is something to save; a failed load turns it into the
    // retry `Enter` already is. A number the open editor holds out of range makes it wait.
    let save_button = Button::new("settings-save", if load_failed { "Retry" } else { "Save" })
        .style(ButtonStyle::Primary)
        .disabled(!(dirty || load_failed) || draft.save_in_flight || draft.edit_rule.is_some())
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
        .flush_body(true)
        .body(body)
        .footer_start(footer_start)
        .actions(vec![cancel_button, save_button]);
    if let Some(search) = search {
        card = card.header_actions(search);
    }
    // The strip speaks only when it has something to say: a refusal or a failed load in red,
    // or the one question the first `Esc` on a dirty draft asks, in amber.
    if let Some(message) = draft.error.clone().or_else(|| draft.load_error()) {
        card = card.error(message);
    } else if let Some(warning) = draft.discard_warning() {
        card = card.warning(warning);
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
                // §3.8.6: a text, number or model row is opened for editing by `Enter`; once
                // its editor owns the keyboard the dialog publishes `SettingsEditing`, whose own
                // `Enter` row reaches this handler again and keeps the value and closes the box.
                // Every other row saves.
                if confirm_opens_editing(&save_state, &focus, window, cx) {
                    return;
                }
                save(&save_state, &save_bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &dialog::Cancel, window, cx| {
                // `Esc` reverts an open editor, then asks once about a dirty draft; only when
                // it has nothing left to do does it reach the shell, which closes the overlay.
                if cancel(&state, &focus, window, cx) {
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
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
///
/// While a search is typed the rail selects nothing and dims: `⏎` and `esc` act on the hits,
/// and a section left lit would claim them (§3.8.6). A click on a section still jumps to it.
fn rail(draft: &SettingsState, searching: bool, handlers: &Handlers, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(SETTINGS_RAIL_W))
        .h_full()
        .gap(theme.space.xxs)
        .p(theme.space.sm)
        .bg(theme.colors.surface)
        .border_r(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .when(searching, |el| el.opacity(theme.metrics.dimmed_opacity))
        .children(Section::ALL.iter().enumerate().map(|(index, entry)| {
            let selected = !searching && index == draft.section;
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

/// The pane for the section the rail highlights: its rows, a card per run of rows that share a
/// heading (an untitled card for rows with none), then the section's one caption.
fn section_pane(
    draft: &SettingsState,
    editing: Option<Entity<TextInput>>,
    handlers: &Handlers,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let rows = &draft.prepared;
    let section = draft.current_section();
    let mut cards: Vec<AnyElement> = Vec::new();
    let mut start = 0;
    while let Some(first) = rows.get(start) {
        let heading = first.card;
        let end = rows
            .iter()
            .skip(start)
            .position(|row| row.card != heading)
            .map_or(rows.len(), |len| start + len);
        let run = rows.get(start..end).unwrap_or_default();
        let elements = run.iter().enumerate().map(|(offset, row)| {
            let index = start + offset;
            let cursor = index == draft.row;
            row_element(
                row,
                index,
                cursor,
                cursor.then(|| editing.clone()).flatten(),
                draft.edit_rule.as_deref().filter(|_| cursor),
                handlers,
                cx,
            )
        });
        // A keep-alive rule whose pattern does not compile says so once, under its card.
        let broken: Vec<AnyElement> = card_captions(run)
            .into_iter()
            .map(|sentence| {
                Text::caption(sentence)
                    .tone(Tone::Danger)
                    .into_any_element()
            })
            .collect();
        let card = SettingsCard::new(("settings-card", start))
            .rows(elements.collect::<Vec<_>>())
            .when_some(heading, |card, title| {
                let card = card.title(title);
                match card_note(title) {
                    Some(note) => card.note(note),
                    None => card,
                }
            })
            .when(!broken.is_empty(), |card| {
                card.caption(
                    div()
                        .flex()
                        .flex_col()
                        .gap(theme.space.xxs)
                        .children(broken),
                )
            });
        cards.push(card.into_any_element());
        start = end;
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
        .children(cards)
        .children(section.caption().map(|caption| {
            div()
                .px(theme.space.xs)
                .child(Text::caption(caption).muted())
        }))
}

/// Draws one row with the control its kind calls for, every control wired to the pointer.
///
/// `editing` is the live editor, present only on the cursor row that `Enter` (or a click on its
/// box) opened; the row's `ValueBox` draws it in place of the value. `rule` is what the typed
/// number breaks, which replaces the helper.
fn row_element(
    row: &SettingRow,
    index: usize,
    cursor: bool,
    editing: Option<Entity<TextInput>>,
    rule: Option<&str>,
    handlers: &Handlers,
    cx: &App,
) -> AnyElement {
    let open = {
        let handlers = handlers.clone();
        move |window: &mut Window, cx: &mut App| {
            open_row(&handlers.state, index, &handlers.focus, window, cx);
        }
    };
    let mut settings_row = SettingsRow::new(("settings-row", index))
        .label(row.label.clone())
        .cursor(cursor)
        .on_click({
            let handlers = handlers.clone();
            move |_, window, cx| click_row(&handlers.state, index, &handlers.focus, window, cx)
        });
    if row.kind.opens_editor() {
        let open = open.clone();
        settings_row = settings_row.on_double_click(move |_, window, cx| open(window, cx));
    }
    if let Some(detail) = row.detail.clone() {
        settings_row = settings_row.helper(detail);
    }
    if let Some(rule) = rule {
        settings_row = settings_row.invalid(rule.to_owned());
    }

    let settings_row = match &row.kind {
        RowKind::Toggle(on) => {
            let handlers = handlers.clone();
            let switch = Switch::new(("settings-switch", index), *on)
                .name(row.label.clone())
                .on_toggle(move |on, window, cx| {
                    set_row_switch(&handlers.state, index, on, &handlers.focus, window, cx);
                });
            settings_row.control(switch.harness_target_named(cursor.then_some("settings.switch")))
        }
        RowKind::Rule {
            on,
            badge,
            pattern,
            running,
            broken,
        } => {
            let handlers = handlers.clone();
            let switch = Switch::new(("settings-switch", index), *on)
                .name(row.label.clone())
                .on_toggle(move |on, window, cx| {
                    set_row_switch(&handlers.state, index, on, &handlers.focus, window, cx);
                });
            let settings_row = settings_row
                .leading(switch.harness_target_named(cursor.then_some("settings.switch")))
                .label_badge(Badge::new(*badge).style(BadgeStyle::Filled))
                .disabled(*broken);
            let settings_row = if pattern.is_empty() {
                settings_row
            } else {
                settings_row.helper_mono(pattern.clone())
            };
            match running {
                Some(0) => settings_row.control(Text::caption("none running").muted()),
                Some(count) => settings_row.control(
                    Chip::labeled(Icon::Zap, format!("{count} running"))
                        .tone(Tone::Success)
                        .filled(true),
                ),
                None => settings_row,
            }
        }
        RowKind::Choice {
            value,
            has_prev,
            has_next,
            off_grid,
            options,
        } => {
            let handlers = handlers.clone();
            let mut cycler = Cycler::new(value.clone())
                .id(("settings-choice", index))
                .inline(true)
                .options(options.iter().cloned())
                .has_prev(*has_prev)
                .has_next(*has_next)
                .off_grid(*off_grid)
                .on_select(move |option, window, cx| {
                    select_option(&handlers.state, index, option, &handlers.focus, window, cx);
                });
            if cursor {
                cycler = cycler.harness("settings.option", "settings.dropdown");
            }
            settings_row.control(cycler)
        }
        RowKind::Number { value, unit, .. } => {
            let mut value_box = ValueBox::new(("settings-box", index), value.to_string())
                .width(ValueBoxWidth::Number)
                .mono(true)
                .invalid(rule.is_some())
                .on_click(open);
            if let Some(unit) = unit {
                value_box = value_box.unit(unit.clone());
            }
            if let Some(input) = editing {
                value_box = value_box.editor(input);
            }
            settings_row.control(value_box.harness_target_named(cursor.then_some("settings.box")))
        }
        RowKind::Text(value) => settings_row.control(
            text_box(row, index, value, editing, open)
                .harness_target_named(cursor.then_some("settings.box")),
        ),
        RowKind::Model {
            value,
            shown,
            options,
            harness,
        } => {
            if row.kind.draws_model_dropdown(editing.is_some()) {
                settings_row.control(
                    model_dropdown(index, value, shown, options, harness, open, handlers)
                        .harness_target_named(cursor.then_some("settings.dropdown")),
                )
            } else {
                // No catalogue, or an id being typed: the same text box every text row draws.
                settings_row.control(
                    text_box(row, index, value, editing, open)
                        .harness_target_named(cursor.then_some("settings.box")),
                )
            }
        }
        RowKind::Fact(value) => {
            let theme = cx.theme();
            let text = if row.mono {
                Text::data(value.clone())
            } else {
                Text::ui(value.clone())
            };
            settings_row
                .control(
                    div()
                        .max_w(theme.metrics.value_box_w)
                        .min_w_0()
                        .overflow_hidden()
                        .child(text.ellipsize()),
                )
                .when_some(row.copy.clone(), |settings_row, copy| {
                    settings_row.trailing(
                        IconButton::new(("settings-copy", index), Icon::Copy, "Copy")
                            .size(ButtonSize::Compact)
                            .on_click(move |_, _, cx| copy_value(&copy, cx))
                            .harness_target_indexed("settings.copy", index),
                    )
                })
        }
    };
    settings_row
        .harness_target_indexed("settings.row", index)
        .into_any_element()
}

/// A text or model row's box: mono, 300 px, the row's placeholder when empty.
fn text_box(
    row: &SettingRow,
    index: usize,
    value: &str,
    editing: Option<Entity<TextInput>>,
    open: impl Fn(&mut Window, &mut App) + 'static,
) -> ValueBox {
    let mut value_box = ValueBox::new(("settings-box", index), value.to_owned())
        .width(ValueBoxWidth::Text)
        .mono(true)
        .on_click(open);
    if let Some(placeholder) = row.placeholder {
        value_box = value_box.placeholder(placeholder);
    }
    if let Some(input) = editing {
        value_box = value_box.editor(input);
    }
    value_box
}

/// The Default model dropdown: Harness default, the models the harness reported, then
/// `Other model id…`, which opens the box for an id the list lacks.
fn model_dropdown(
    index: usize,
    value: &str,
    shown: &str,
    options: &[ModelOption],
    harness: &'static str,
    open: impl Fn(&mut Window, &mut App) + 'static,
    handlers: &Handlers,
) -> Dropdown {
    let options: std::rc::Rc<[ModelOption]> = options.into();
    let value: SharedString = value.to_owned().into();
    let handlers = handlers.clone();
    let open = std::rc::Rc::new(open);
    Dropdown::new(("settings-model", index), shown.to_owned())
        .compact()
        .menu(move |menu, _window, _cx| {
            let pick = |choice: Option<usize>| {
                let handlers = handlers.clone();
                move |window: &mut Window, cx: &mut App| {
                    pick_model(&handlers.state, index, choice, &handlers.focus, window, cx);
                }
            };
            let menu = menu
                .item(
                    MenuItem::new(MODEL_DEFAULT)
                        .detail(format!("whatever {harness} starts with"))
                        .checked(value.is_empty())
                        .on_select(pick(None)),
                )
                .separator();
            let menu = options.iter().enumerate().fold(menu, |menu, (ix, option)| {
                let item = MenuItem::new(option.name.clone())
                    .checked(option.id == value.as_ref())
                    .on_select(pick(Some(ix)));
                menu.item(if option.name == option.id {
                    item
                } else {
                    item.detail(option.id.clone())
                })
            });
            let open = open.clone();
            menu.separator().item(
                MenuItem::new("Other model id\u{2026}")
                    .detail("type it")
                    .on_select(move |window, cx| open(window, cx)),
            )
        })
}

/// The pane while a search is typed: every matching row under its section's heading, in rail
/// order, then the keys that move over them.
fn hits_pane(
    search: &SearchState,
    scroll: &gpui::ScrollHandle,
    handlers: &Handlers,
    focus: &FocusHandle,
    window: &Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let mut children: Vec<AnyElement> = Vec::new();
    let mut shown_section = None;
    for (index, hit) in search.hits.iter().enumerate() {
        if shown_section != Some(hit.section) {
            shown_section = Some(hit.section);
            children.push(
                div()
                    .px(theme.space.md)
                    .pt(theme.space.md)
                    .pb(theme.space.xs)
                    .child(Text::sentence_label(hit.section.title()))
                    .into_any_element(),
            );
        }
        let handlers = handlers.clone();
        let mut row = SettingsRow::new(("settings-hit", index))
            .leading(
                hit.section
                    .icon()
                    .el()
                    .size(IconSize::Small)
                    .tone(Tone::Secondary),
            )
            .label_spans(
                hit.spans
                    .iter()
                    .map(|(text, strong)| (SharedString::from(text.clone()), *strong)),
            )
            .trailing(Text::caption(hit.section.title()).muted())
            .cursor(index == search.cursor)
            .on_click(move |_, window, cx| {
                jump_to_hit(&handlers.state, Some(index), &handlers.focus, window, cx);
            });
        if let Some(detail) = hit.detail.clone() {
            row = row.helper(detail);
        }
        children.push(
            row.harness_target_indexed("settings.hit", index)
                .into_any_element(),
        );
    }

    let key = |action: &dyn gpui::Action| Kbd::for_action_in(action, focus, window);
    let keys_caption = div()
        .flex()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(theme.space.xs)
        .px(theme.space.xs)
        .children(key(&dialog::CursorUp))
        .children(key(&dialog::CursorDown))
        .child(Text::caption("move \u{00b7}").muted())
        .children(key(&dialog::Confirm))
        .child(Text::caption("open the row \u{00b7}").muted())
        .children(key(&settings_actions::EndSearch))
        .child(Text::caption("back to the sections").muted());

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .gap(theme.space.md)
        .p(theme.space.lg)
        .child(
            div()
                .id("settings-hits")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .when(search.hits.is_empty(), |el| {
                    el.child(EmptyState::new(SharedString::from(format!(
                        "No setting matches \u{201c}{}\u{201d}.",
                        search.query.trim()
                    ))))
                })
                .children(children),
        )
        .child(keys_caption)
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
