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

    let editing = host.read(cx).settings_input.clone();
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
            row_element(row, focused, focused.then(|| editing.clone()).flatten())
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

    input_actions(root(focus), state, focus)
        .on_action(move |_: &dialog::Confirm, window, cx| {
            // §3.8.6: a text or number row is opened for editing by `Enter`; once its editor
            // owns the keyboard the dialog publishes `SettingsEditing`, whose own `Enter` row
            // reaches this handler again and saves. Every other row saves straight away.
            if confirm_opens_editing(&save_state, window, cx) {
                return;
            }
            save(&save_state, &save_bridge, cx);
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
        .child(card)
        .into_any_element()
}

/// Draws one row with the kit control its kind calls for.
///
/// `editing` is the live editor of §3.8.6, present only on the row that `Enter` opened. A text
/// row *is* that editor; a number row keeps its `NumberField` chrome around it so the label and
/// the unit stay where they were.
pub(super) fn row_element(
    row: &SettingRow,
    focused: bool,
    editing: Option<Entity<TextInput>>,
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
            let mut field = NumberField::labeled(row.label.clone(), *value)
                .min(*min)
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
            if let Some(input) = editing {
                return input.into_any_element();
            }
            let mut field = TextField::new(value.clone())
                .label(row.label.clone())
                .focused(false)
                .mono(true);
            // The 18 px slot §3.8.1 reserves: the sub-label lives in the preview line, and an
            // `invalid` message replaces it there, which is the zero-shift rule already.
            if let Some(detail) = row.detail.clone() {
                field = field.preview(detail);
            }
            if let Some(invalid) = row.invalid.clone() {
                field = field.invalid(invalid);
            }
            field.into_any_element()
        }
        RowKind::Fact(value) => KeyValueList::new()
            .row(row.label.clone(), FactValue::known(value.clone()))
            .into_any_element(),
    }
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
        move |_: &dialog::CursorDown, window, cx| move_row(&state, 1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::CursorUp, window, cx| move_row(&state, -1, &focus, window, cx)
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
