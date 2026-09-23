use super::*;

/// Renders the query field over the offered values.
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (draft, input) = read_host(state, cx, |host, _| {
        (host.card_picker.clone(), host.card_picker_input.clone())
    });
    let Some(input) = input else {
        return root(focus).into_any_element();
    };
    // Composed from rows the draft already holds: `prepared` derives them again only when the
    // query, the kind or the board behind them moved, never once per frame.
    let rows = prepared(state, cx);
    let schema = property_kind(state.read(cx), &draft.kind);
    let multi = draft.kind.is_multi_select(schema);

    let label = picker_label(state.read(cx), &draft.kind);
    let selected = draft.selected.clone();
    let accent = Tone::Accent.color(cx.theme());
    let list = FuzzyList::new(
        "card-picker-list",
        rows.iter().map(|option| {
            // A row that cannot be taken is drawn faint and never as the selection, so the reason
            // in its trailing detail — `would cycle` — is the only thing left to read.
            let mut item = FuzzyItem::new(option.label.clone()).disabled(option.disabled);
            if let Some(detail) = option.detail.clone() {
                item = item.trailing(detail);
            }
            if multi && selected.contains(&option.value) {
                item = item.leading(Icon::Check.el().size(IconSize::Small).color(accent));
            }
            item
        }),
    )
    .cursor(draft.cursor)
    .visible_rows(PICKER_ROWS)
    .track_scroll(&draft.scroll)
    .on_click({
        let state = state.clone();
        let bridge = bridge.clone();
        move |index, _window, cx| click_row(&state, &bridge, index, cx)
    })
    .harness_rows("dialog.row", 0)
    .under_text_field(true)
    // Telling a fixed-list kind to type one is telling it to do the thing `apply` answers with
    // "No match — pick a value": only a kind that takes free text can be typed into.
    .empty(
        Text::ui(if accepts_free_text(&draft.kind, schema) {
            "No values \u{2014} type one."
        } else {
            "No values to pick."
        })
        .muted(),
    );

    let mut card = Dialog::new("Card property")
        .dismiss_action(crate::dialogs::Dialogs::CardPicker.dismiss_action())
        .icon(Icon::ArrowRightLeft)
        .width(Dialogs::CardPicker.width(cx))
        .subtitle(label)
        // The query is this dialog's whole tab cycle, so it is `dialog.field[0]` — the field
        // `dialogs::dialog_fields` reports at the same index (`docs/TESTING-HARNESS.md` §3).
        .body(
            div()
                .flex()
                .flex_col()
                .child(input.harness_target_indexed("dialog.field", 0))
                .child(list),
        )
        .actions(vec![
            footer::cancel(&Dialogs::CardPicker),
            footer::primary("card-picker-apply", "Apply", Box::new(dialog::Confirm)),
        ]);
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let apply_state = state.clone();
    let apply_bridge = bridge.clone();

    root(focus)
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_cursor(&state, -1, cx)
        })
        // `tab` moves the highlight here too, as KEYMAP §Board says the generic Dialog context
        // binds it to: bare `j` and `k` type into the query, so a dead `tab` left `ctrl-n` as
        // the only way to move.
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| move_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| move_cursor(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::Toggle, _window, cx| toggle(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Cancel, _window, cx| {
                close(&state, cx);
                cx.stop_propagation();
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            apply(&apply_state, &apply_bridge, cx);
            cx.stop_propagation();
        })
        .child(card)
        .into_any_element()
}
