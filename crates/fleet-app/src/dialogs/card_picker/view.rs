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
    let draft = read_host(state, cx, |host, _| host.card_picker.clone());
    // Composed from rows the draft already holds: `prepared` derives them again only when the
    // query, the kind or the board behind them moved, never once per frame.
    let rows = prepared(state, cx);
    let schema = property_kind(state.read(cx), &draft.kind);
    let invalid = free_text_error(&draft.kind, draft.query.trim(), schema);
    let multi = draft.kind.is_multi_select(schema);

    let label = picker_label(state.read(cx), &draft.kind);
    let mut field = TextField::new(draft.query.clone())
        .label(label.clone())
        .placeholder(if multi {
            "filter values"
        } else {
            "type to filter or set"
        })
        .caret(draft.caret)
        .focused(true);
    if let Some(message) = invalid.clone() {
        field = field.invalid(message);
    }

    let selected = draft.selected.clone();
    let accent = Tone::Accent.color(cx.theme());
    let list = FuzzyList::new(rows.iter().map(|option| {
        let mut item = FuzzyItem::new(option.label.clone());
        if let Some(detail) = option.detail.clone() {
            item = item.trailing(detail);
        }
        if multi && selected.contains(&option.value) {
            item = item.leading(Icon::Check.el().size(IconSize::Small).color(accent));
        }
        item
    }))
    .cursor(draft.cursor)
    .cap(PICKER_ROWS)
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

    // `ctrl-n` / `ctrl-p` are the only way to move the highlight on either branch — `j` and
    // `k` type into the query — so the multi-select row names them too. Without it a Labels
    // picker offered no discoverable way to reach its second row.
    let hints = if multi {
        KeyHintRow::new()
            .key("\u{2303}n/\u{2303}p", "move")
            .key("space", "toggle")
            .key("\u{23ce}", "apply")
            .key("esc", "cancel")
    } else {
        KeyHintRow::new()
            .key("\u{2303}n/\u{2303}p", "move")
            .key("\u{23ce}", "set")
            .key("esc", "cancel")
    };

    let mut card = Dialog::new("Card property")
        .icon(Icon::ArrowRightLeft)
        .width(Dialogs::CardPicker.width(cx))
        .subtitle(format!("\u{00b7} {label}"))
        .body(div().flex().flex_col().child(field).child(list))
        .hint_row(hints)
        .primary("\u{23ce} Apply");
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let apply_state = state.clone();
    let apply_bridge = bridge.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let typed = with_host(&state, cx, |host| {
                    let mut input = host.card_picker.input();
                    if !type_into(&mut input, event) {
                        return false;
                    }
                    host.card_picker.set_input(&input);
                    host.card_picker.cursor = 0;
                    host.card_picker.error = None;
                    true
                });
                if typed {
                    notify(&state, cx);
                    cx.stop_propagation();
                }
            }
        })
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
            move |_: &dialog::CursorLeft, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_left();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_right();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit_query(&state, cx, |input| {
                    input.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit_query(&state, cx, |input| {
                    input.delete_word_before();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                edit_query(&state, cx, |input| {
                    input.clear();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_to_start();
                })
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                edit_query(&state, cx, |input| {
                    let _moved = input.move_to_end();
                })
            }
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
