use super::*;

/// Renders the dialog (§3.8.6).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let theme = cx.theme();
        (theme.space.md, theme.space.xs)
    };
    let draft = &host.read(cx).settings;
    let pane_rows = &draft.prepared;
    let section = draft.current_section();
    let dirty = draft.dirty();

    let rail = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(RAIL_WIDTH))
        .gap(tight)
        .children(Section::ALL.iter().enumerate().map(|(index, entry)| {
            let selected = index == draft.section;
            Row::new()
                .selected(selected)
                .cursor(selected)
                .column(RowColumn::flex(Text::ui(entry.title())))
        }));

    let editing = &draft.editing;
    let pane = div()
        .id("settings-pane")
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(tight)
        .children(pane_rows.iter().enumerate().map(|(index, row)| {
            let focused = index == draft.row;
            row_element(row, focused, focused.then_some(editing.as_ref()).flatten())
        }))
        .children(
            section
                .editable()
                .then(|| Text::hint("edit in config.json").tone(Tone::Muted)),
        );

    let body = div()
        .flex()
        .flex_row()
        .gap(gap)
        .size_full()
        .child(rail)
        .child(Divider::vertical())
        .child(pane);

    let mut card = Dialog::new("Settings")
        .icon(Icon::Settings2)
        .width(crate::dialogs::Dialogs::Settings.width(cx))
        .when_some(crate::dialogs::Dialogs::Settings.height(), Dialog::height)
        .body(body)
        .hint_row(if dirty {
            KeyHintRow::new()
                .key("\u{23ce}", "save")
                .key("esc", "discard changes")
                .key("E", "config.json")
                .key("D", "doctor")
        } else {
            KeyHintRow::new()
                .key("\u{21e5}", "section")
                .key("j/k", "row")
                .key("E", "config.json")
                .key("D", "doctor")
        });
    if dirty {
        // §3.8.6: the dirty state marks the title in accent and renames the footer.
        card = card.tone(Tone::Accent).primary("\u{23ce} Save");
    }
    if let Some(message) = draft.error.clone().or_else(|| draft.load_error()) {
        card = card.error(message);
    }

    let save_state = state.clone();
    let save_bridge = bridge.clone();
    let doctor_bridge = bridge.clone();

    input_actions(root(focus), state)
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            save(&save_state, &save_bridge, cx);
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &settings_actions::OpenConfigFile, _window, cx| {
                // §3.8.6 surrenders every bound printable key to a focused input, `E` and `D`
                // included: `Claude command` and `OpenCode command` are free text, and a key
                // that replaced the screen instead of typing dropped the draft silently.
                if insert_literal(&state, "E", cx) {
                    return;
                }
                open_config_file(&state, &bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::RunDoctor, _window, cx| {
                if insert_literal(&state, "D", cx) {
                    return;
                }
                run_doctor(&state, &doctor_bridge, cx);
            }
        })
        .child(card)
        .into_any_element()
}

/// Draws one row with the kit control its kind calls for.
pub(super) fn row_element(
    row: &SettingRow,
    focused: bool,
    editing: Option<&TextFieldState>,
) -> AnyElement {
    match &row.kind {
        RowKind::Toggle(checked) => {
            let mut toggle = Toggle::labeled(row.label.clone(), *checked).focused(focused);
            if let Some(detail) = row.invalid.clone().or_else(|| row.detail.clone()) {
                toggle = toggle.detail(detail);
            }
            toggle.into_any_element()
        }
        RowKind::Choice {
            value,
            has_prev,
            has_next,
            off_grid,
        } => Cycler::labeled(row.label.clone(), value.clone())
            .has_prev(*has_prev)
            .has_next(*has_next)
            .off_grid(*off_grid)
            .focused(focused)
            .into_any_element(),
        RowKind::Number { value, min, unit } => {
            if let Some(input) = editing {
                let valid = input
                    .text()
                    .trim()
                    .parse::<i64>()
                    .is_ok_and(|value| value >= *min);
                let label = unit.as_ref().map_or_else(
                    || row.label.clone(),
                    |unit| format!("{} ({unit})", row.label),
                );
                let mut field = TextField::new(input.text().to_owned())
                    .label(label)
                    .caret(input.caret_chars())
                    .focused(input_is_focused(focused, editing))
                    .mono(true);
                if !valid {
                    field = field.invalid(format!("must be an integer of at least {min}"));
                }
                return field.into_any_element();
            }
            let mut field = NumberField::labeled(row.label.clone(), *value)
                .min(*min)
                .focused(false);
            if let Some(unit) = unit {
                field = field.unit(unit.clone());
            }
            field.into_any_element()
        }
        RowKind::Text(value) => {
            TextField::new(editing.map_or_else(|| value.clone(), |input| input.text().to_owned()))
                .label(row.label.clone())
                .caret(editing.map_or(0, TextFieldState::caret_chars))
                .focused(input_is_focused(focused, editing))
                .mono(true)
                .into_any_element()
        }
        RowKind::Fact(value) => KeyValueList::new()
            .row(row.label.clone(), FactValue::known(value.clone()))
            .into_any_element(),
    }
}

pub(super) fn input_is_focused(focused_row: bool, editing: Option<&TextFieldState>) -> bool {
    focused_row && editing.is_some()
}

pub(super) fn input_actions(root: gpui::Div, state: &Entity<AppState>) -> gpui::Div {
    root.on_key_down({
        let state = state.clone();
        move |event, _window, cx| {
            type_into_row(&state, event, cx);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::MoveDown, _window, cx| move_row(&state, 1, "j", cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::MoveUp, _window, cx| move_row(&state, -1, "k", cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::CursorDown, _window, cx| move_row(&state, 1, "", cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::CursorUp, _window, cx| move_row(&state, -1, "", cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::NextField, _window, cx| move_section(&state, 1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::PrevField, _window, cx| move_section(&state, -1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CyclePrev, _window, cx| cycle_row(&state, -1, "h", cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CycleNext, _window, cx| cycle_row(&state, 1, "l", cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::Toggle, _window, cx| toggle_row(&state, cx)
    })
    // `Backspace` / `ctrl-u` / `ctrl-w` are unambiguous edit intents, so unlike `j` / `k`
    // they focus the row's input themselves.
    .on_action({
        let state = state.clone();
        move |_: &dialog::Backspace, _window, cx| {
            edit_focused(&state, cx, TextFieldState::backspace);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::ClearInput, _window, cx| {
            edit_focused(&state, cx, clear_all);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::DeleteWord, _window, cx| {
            edit_focused(&state, cx, TextFieldState::delete_word_before);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::LineStart, _window, cx| {
            move_caret(&state, cx, TextFieldState::move_to_start);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::LineEnd, _window, cx| {
            move_caret(&state, cx, TextFieldState::move_to_end);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::CursorLeft, _window, cx| {
            if !move_caret(&state, cx, TextFieldState::move_left) {
                cycle_row(&state, -1, "", cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::CursorRight, _window, cx| {
            if !move_caret(&state, cx, TextFieldState::move_right) {
                cycle_row(&state, 1, "", cx);
            }
        }
    })
}
