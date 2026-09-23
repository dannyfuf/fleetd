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
    let (mut draft, edit_input) = read_host(state, cx, |host, _| {
        (host.card_detail.clone(), host.card_detail_input.clone())
    });
    let Some((board, card)) = state.read(cx).board().and_then(|view| {
        card(state.read(cx), &draft).map(|card| (view.board.clone(), card.clone()))
    }) else {
        return missing(state, focus, cx);
    };
    let (board, card) = (&board, &card);
    let now = now_unix();
    let theme = cx.theme().clone();
    // The run row's facts are read here, where the whole state is in hand: the mark is the
    // board's own fold, and the live status is the delegation mirror's, which is newer than
    // the view's join between two board responses.
    let run = run_line(state.read(cx), card, now);
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
        edit_input
            .clone()
            .map_or_else(|| div().into_any_element(), Entity::into_any_element)
    } else if card.description.trim().is_empty() {
        Text::ui("No description \u{2014} d writes one.")
            .faint()
            .into_any_element()
    } else {
        MarkdownText::new(card.description.clone()).into_any_element()
    };

    let title: AnyElement = if draft.edit == Some(CardEdit::Title) {
        edit_input
            .clone()
            .map_or_else(|| div().into_any_element(), Entity::into_any_element)
    } else {
        detail::title_line(board, card, cx)
    };

    let comment_editor = (draft.edit == Some(CardEdit::Comment))
        .then(|| edit_input.clone())
        .flatten();

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
        .children(run.as_ref().map(|run| detail::run_row(run, cx)))
        // The run's own keys, beside the run they act on (contracts §5.3).
        .children(run.as_ref().map(|_| detail::run_hints().into_any_element()))
        .child(description)
        .child(detail::comments(card, now, &draft.expanded_reports, cx))
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
            // The run keys, which this dialog owns whether or not the card has run yet
            // (contracts §5.5): `>` starts the column's action, and a card with no run says so
            // rather than doing nothing.
            .key("A", "attach")
            .key("X", "cancel")
            .key(">", "run")
            .key("esc", "close")
    };

    let mut dialog_card = Dialog::new("Card detail")
        .dismiss_action(crate::dialogs::Dialogs::CardDetail.dismiss_action())
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
        .on_action({
            let state = state.clone();
            let title = card.title.clone();
            move |_: &card_actions::EditTitle, window, cx| {
                begin(&state, CardEdit::Title, title.clone(), window, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let description = card.description.clone();
            move |_: &card_actions::EditDescription, window, cx| {
                begin(
                    &state,
                    CardEdit::Description,
                    description.clone(),
                    window,
                    cx,
                );
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::AddComment, window, cx| {
                begin(&state, CardEdit::Comment, String::new(), window, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::NextProperty, _window, cx| move_row(&state, 1, rows_len, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &card_actions::PrevProperty, _window, cx| move_row(&state, -1, rows_len, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| {
                let input = read_host(&state, cx, |host, _| host.card_detail_input.clone());
                if let Some(input) = input
                    && matches!(input.read(cx).mode(), InputMode::Multiline { .. })
                {
                    input.update(cx, |input, cx| input.insert("\t", cx));
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
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
            move |_: &card_actions::CreateWorktree, _window, cx| start_worktree(&state, &bridge, cx)
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::OpenRemote, _window, cx| {
                open_remote(&state, &bridge, cx);
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::KeepLocal, _window, cx| {
                resolve(&state, &bridge, ConflictResolution::KeepLocal, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &card_actions::TakeRemote, _window, cx| {
                resolve(&state, &bridge, ConflictResolution::TakeRemote, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            let focus = focus.clone();
            move |_: &card_actions::Close, window, cx| {
                if close(&state, &bridge, cx) {
                    window.focus(&focus, cx);
                }
                cx.stop_propagation();
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            let focus = focus.clone();
            move |_: &dialog::Cancel, window, cx| {
                if close(&state, &bridge, cx) {
                    window.focus(&focus, cx);
                }
                cx.stop_propagation();
            }
        })
        .child(dialog_card)
        .into_any_element()
}

/// What the run row states, from the card and the two places a live run is known.
///
/// A run the card calls live is asked of the delegation mirror first and of the board view's
/// join second: `live_runs` is only as fresh as the last board response, while
/// `DelegationChanged` keeps arriving between them.
fn run_line(app: &AppState, card: &Card, now: i64) -> Option<detail::RunLine> {
    let mark = app
        .board
        .marks
        .by_card
        .get(&card.id)
        .and_then(|marks| marks.run);
    let live = card
        .runs
        .last()
        .filter(|run| run.is_live())
        .and_then(|run| {
            app.agents
                .delegation(run.id)
                .map(|delegation| delegation.status)
                .or_else(|| {
                    app.board()?
                        .live_runs
                        .iter()
                        .find(|live| live.run == run.id)
                        .map(|live| live.status)
                })
        });
    detail::run_line(card, mark, live, now)
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
                .dismiss_action(crate::dialogs::Dialogs::CardDetail.dismiss_action())
                .icon(Icon::FilePen)
                .width(Dialogs::CardDetail.width(cx))
                .body(EmptyState::new("That card is no longer on this board."))
                .hint_row(KeyHintRow::new().key("esc", "close")),
        )
        .into_any_element()
}
