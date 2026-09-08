use super::*;

/// Renders the two panes and wires every §8 key.
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let Some((board, card)) = state.read(cx).board().and_then(|view| {
        card(state.read(cx), &draft).map(|card| (view.board.clone(), card.clone()))
    }) else {
        return missing(state, focus, cx);
    };
    let (board, card) = (&board, &card);
    let now = now_unix();
    let theme = cx.theme().clone();
    // Borrowed, never cloned: this runs on every frame, and the card set of a real board
    // carries every comment and activity entry on it.
    let rows = state.read(cx).board().map_or_else(Vec::new, |view| {
        detail::property_rows(&view.board, &view.cards, card, now)
    });
    draft.property_row = draft.property_row.min(rows.len().saturating_sub(1));
    with_host(state, cx, |host| {
        host.card_detail.property_row = draft.property_row
    });

    let description: AnyElement = if draft.edit == Some(CardEdit::Description) {
        TextArea::new(draft.area.text().to_owned())
            .label("Description")
            .placeholder("Markdown.")
            .rows(8)
            .max_rows(8)
            .scroll_row(draft.area.scroll_row())
            .scroll("card-detail-description-scroll", draft.area_scroll.clone())
            .cursor(draft.area.cursor())
            .focused(true)
            .into_any_element()
    } else if card.description.trim().is_empty() {
        Text::ui("No description \u{2014} d writes one.")
            .faint()
            .into_any_element()
    } else {
        MarkdownText::new(card.description.clone()).into_any_element()
    };

    let title: AnyElement = if draft.edit == Some(CardEdit::Title) {
        TextField::new(draft.area.text().to_owned())
            .label("Title")
            .caret(
                draft.area.text()[..draft.area.cursor().min(draft.area.text().len())]
                    .chars()
                    .count(),
            )
            .focused(true)
            .into_any_element()
    } else {
        detail::title_line(board, card, cx)
    };

    let comment_editor = (draft.edit == Some(CardEdit::Comment)).then(|| {
        TextArea::new(draft.area.text().to_owned())
            .label("New comment")
            .placeholder("Markdown.")
            .rows(4)
            .max_rows(4)
            .scroll_row(draft.area.scroll_row())
            .scroll("card-detail-comment-scroll", draft.area_scroll.clone())
            .cursor(draft.area.cursor())
            .focused(true)
    });

    let left = div()
        .id("card-detail-left")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .gap(theme.space.md)
        .pr(theme.space.md)
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .children(detail::conflict_banner(card))
        .child(title)
        .child(description)
        .child(detail::comments(card, now, cx))
        .children(comment_editor)
        .child(detail::activity(card, now, cx));

    let properties = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(PROPERTIES_WIDTH))
        .h_full()
        .gap(theme.space.xxs)
        .border_l(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .pl(theme.space.md)
        .child(SectionHeader::new("Properties"))
        .children(rows.iter().enumerate().map(|(index, row)| {
            detail::property_row(
                row,
                index == draft.property_row,
                !draft.is_editing(),
                &theme,
            )
        }));

    let body = div()
        .flex()
        .flex_row()
        .size_full()
        .min_h_0()
        .child(left)
        .child(properties);

    let hints = if let Some(surface) = draft.edit {
        KeyHintRow::new()
            .key("\u{2303}s", format!("save {}", surface.label()))
            .key("esc", "discard")
    } else {
        KeyHintRow::new()
            .key("i/d/c", "title/desc/comment")
            .key("j/k", "property")
            .key("\u{23ce}", "edit")
            .key("w", "worktree")
            // `x` is otherwise nowhere on this surface: the conflict banner names `K`/`R` when
            // there is a conflict, but nothing ever names the key that opens the issue.
            .key("x", "remote")
            .key("esc", "close")
    };

    let mut dialog_card = Dialog::new("Card detail")
        .icon(Icon::FilePen)
        .width(Dialogs::CardDetail.width(cx))
        .height(px(DETAIL_HEIGHT))
        .subtitle(format!("\u{00b7} {}", card.display_key(board)))
        .body(body)
        .hint_row(hints);
    if let Some(message) = draft.error.clone() {
        dialog_card = dialog_card.error(message);
    }

    let rows_len = rows.len();
    let targets: Vec<detail::PropertyRow> = rows;

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                if type_text(&state, text, cx) {
                    cx.stop_propagation();
                }
            }
        })
        .on_action({
            let state = state.clone();
            let title = card.title.clone();
            move |_: &card_actions::EditTitle, _window, cx| {
                if type_text(&state, "i", cx) {
                    return;
                }
                begin(&state, CardEdit::Title, title.clone(), cx);
            }
        })
        .on_action({
            let state = state.clone();
            let description = card.description.clone();
            move |_: &card_actions::EditDescription, _window, cx| {
                if type_text(&state, "d", cx) {
                    return;
                }
                begin(&state, CardEdit::Description, description.clone(), cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::AddComment, _window, cx| {
                if type_text(&state, "c", cx) {
                    return;
                }
                begin(&state, CardEdit::Comment, String::new(), cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::NextProperty, _window, cx| {
                if type_text(&state, "j", cx) {
                    return;
                }
                move_row(&state, 1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::PrevProperty, _window, cx| {
                if type_text(&state, "k", cx) {
                    return;
                }
                move_row(&state, -1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| {
                if edit_buffer(&state, cx, |area| {
                    area.move_down();
                }) {
                    return;
                }
                move_row(&state, 1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| {
                if edit_buffer(&state, cx, |area| {
                    area.move_up();
                }) {
                    return;
                }
                move_row(&state, -1, rows_len, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| {
                edit_buffer(&state, cx, TextAreaState::insert_tab);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_left();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_right();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.delete_word_before();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                // DESIGN-SYSTEM §TextArea: `ctrl-u` clears the line, not the whole draft.
                edit_buffer(&state, cx, |area| {
                    area.delete_to_line_start();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_to_line_start();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                edit_buffer(&state, cx, |area| {
                    area.move_to_line_end();
                });
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::EditProperty, _window, cx| {
                enter(&state, &bridge, &targets, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::Save, _window, cx| commit_edit(&state, &bridge, cx)
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::CreateWorktree, _window, cx| {
                if type_text(&state, "w", cx) {
                    return;
                }
                start_worktree(&state, &bridge, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::OpenRemote, _window, cx| {
                // The guard belongs to the *key*, not to the command: this handler is what a
                // literal `x` reaches while a text edit is open, and the palette dispatches
                // the same action from a surface where nothing was typed. Held inside
                // `open_remote` it turned the palette's "Open remote issue" row into an `x`
                // inserted in the user's description, with no browser and no message.
                if type_text(&state, "x", cx) {
                    return;
                }
                open_remote(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::KeepLocal, _window, cx| {
                if type_text(&state, "K", cx) {
                    return;
                }
                resolve(&state, &bridge, ConflictResolution::KeepLocal, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::TakeRemote, _window, cx| {
                if type_text(&state, "R", cx) {
                    return;
                }
                resolve(&state, &bridge, ConflictResolution::TakeRemote, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::Close, _window, cx| {
                close(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &dialog::Cancel, _window, cx| {
                close(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .child(dialog_card)
        .into_any_element()
}

/// The card went away while the dialog was open — say so instead of showing an empty card.
pub(super) fn missing(state: &Entity<AppState>, focus: &FocusHandle, cx: &App) -> AnyElement {
    let state = state.clone();
    root(focus)
        .on_action(move |_: &dialog::Cancel, _window, cx| {
            state.update(cx, |app, cx| {
                app.close_overlay();
                cx.notify();
            });
            cx.stop_propagation();
        })
        .child(
            Dialog::new("Card detail")
                .icon(Icon::FilePen)
                .width(Dialogs::CardDetail.width(cx))
                .body(EmptyState::new("That card is no longer on this board."))
                .hint_row(KeyHintRow::new().key("esc", "close")),
        )
        .into_any_element()
}
