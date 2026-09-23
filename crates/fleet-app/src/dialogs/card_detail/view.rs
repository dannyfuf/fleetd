use std::rc::Rc;

use fleet_ui_kit::harness::HarnessTargetExt as _;

use super::*;
use crate::{
    actions::board as board_actions,
    dialogs::card_picker::PickerKind,
    presentation::age_label,
    views::board_screen::{CardMenu, category_accent},
};

/// Renders the sheet and wires every §8 key.
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    window: &mut Window,
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
    let mark = state
        .read(cx)
        .board
        .marks
        .by_card
        .get(&card.id)
        .and_then(|marks| marks.run);
    let menu = CardMenu::of(board, card, mark);
    let backend = state.read(cx).backend_label(&board.backend.kind);
    // Borrowed, never cloned: this runs on every frame, and the card set of a real board
    // carries every comment and activity entry on it.
    let rows = state.read(cx).board().map_or_else(Vec::new, |view| {
        detail::property_rows(&view.board, &view.cards, card, now)
    });
    draft.property_row = draft.property_row.min(rows.len().saturating_sub(1));
    with_host(state, cx, |host| {
        host.card_detail.property_row = draft.property_row
    });
    let editor = |surface: CardEdit| {
        (draft.edit == Some(surface))
            .then(|| edit_input.clone())
            .flatten()
    };

    let header = header(board, card, &backend, &menu, &theme);

    let title: AnyElement = match editor(CardEdit::Title) {
        Some(input) => editing(input, CardEdit::Title, &theme),
        None => div()
            .id("card-detail-title")
            .w_full()
            .cursor_pointer()
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(card_actions::EditTitle), cx);
            })
            .child(Text::page_title(card.title.clone()))
            .harness_target("card_detail.title")
            .into_any_element(),
    };

    let description = description(card, editor(CardEdit::Description), &theme);

    let composer: AnyElement = match editor(CardEdit::Comment) {
        Some(input) => editing(input, CardEdit::Comment, &theme),
        None => Button::new("card-detail-comment", "Add a comment\u{2026}")
            .icon(Icon::SquarePen)
            .full_width()
            .action(Box::new(card_actions::AddComment))
            .harness_target("card_detail.comment")
            .into_any_element(),
    };

    let buttons = detail::RunButtons {
        attach: menu.attach,
        rerun: menu.run_now,
        cancel: menu.cancel,
    };
    // The order `lifecycle::begin` counts on to scroll the comment editor into view:
    // conflict (only when there is one), title, run (only beside one), description, comments.
    let prose = div()
        .id("card-detail-left")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .w_full()
        .gap(theme.space.lg)
        .px(theme.space.xl)
        .py(theme.space.lg)
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .children(detail::conflict_banner(card, &theme))
        .child(title)
        .children(run.as_ref().map(|run| detail::run_row(run, buttons, cx)))
        .child(description)
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(theme.space.md)
                .child(detail::comments(card, now, &draft.expanded_reports, cx))
                .child(composer),
        );

    let left = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .children(draft.error.clone().map(|message| {
            div()
                .flex_none()
                .px(theme.space.xl)
                .pt(theme.space.md)
                .child(Callout::new(Tone::Danger, Icon::TriangleAlert, message))
        }))
        .child(prose);

    let activity = {
        let state = state.clone();
        let toggle = move |_: &mut Window, cx: &mut App| {
            with_host(&state, cx, |host| {
                host.card_detail.activity_open = !host.card_detail.activity_open;
            });
            notify(&state, cx);
        };
        detail::activity(card, now, draft.activity_open, toggle, cx)
    };
    let properties = properties(
        state,
        bridge,
        &rows,
        &draft,
        PropertyKeys::resolve(&rows, window, cx),
        activity,
        cx,
    );

    let body = div()
        .flex()
        .flex_row()
        .size_full()
        .min_h_0()
        .child(left)
        .child(properties);

    let sheet = Sheet::new(true)
        .side(SheetSide::Right)
        .scrim(true)
        .width(theme.metrics.sheet_w_detail)
        .dismiss_action(Dialogs::CardDetail.dismiss_action())
        .close_target("card_detail.close")
        .header(header)
        .body(body);

    let rows_len = rows.len();
    let targets: Rc<Vec<detail::PropertyRow>> = Rc::new(rows);
    let card_id = card.id.clone();

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
            let targets = targets.clone();
            move |_: &card_actions::EditProperty, _window, cx| {
                enter(&state, &bridge, &targets, cx);
            }
        })
        // The board's field keys, answered here for the card on show: each selects the row it
        // edits and runs what `⏎` runs on it. A row this card does not have (no agent rows
        // outside an automated column, no link rows on an unlinked board) hands the key on to
        // the board, whose own answer names why.
        .on_action(pick_key::<board_actions::PickStatus>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::Status),
        ))
        .on_action(pick_key::<board_actions::PickPriority>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::Priority),
        ))
        .on_action(pick_key::<board_actions::PickAssignee>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::Assignee),
        ))
        .on_action(pick_key::<board_actions::PickLabels>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::Labels),
        ))
        .on_action(pick_key::<board_actions::PickEstimate>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::Estimate),
        ))
        .on_action(pick_key::<board_actions::PickBlockedBy>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::BlockedBy),
        ))
        .on_action(pick_key::<board_actions::PickAgent>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Pick(PickerKind::Provider),
        ))
        .on_action(pick_key::<board_actions::OpenWorktree>(
            state,
            bridge,
            &targets,
            detail::PropertyTarget::Worktree,
        ))
        .on_action({
            // The ⋯ menu's Delete: the board's own `d`, aimed at the card on show rather than
            // wherever a refresh left the board's cursor.
            let state = state.clone();
            move |_: &board_actions::DeleteCard, _window, cx| {
                state.update(cx, |app, _| board::focus_card(app, &card_id));
                cx.propagate();
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
        .child(sheet)
        .into_any_element()
}

/// A board field key answered on the detail: select the row `target` names and open it.
fn pick_key<A: gpui::Action>(
    state: &Entity<AppState>,
    bridge: &Bridge,
    targets: &Rc<Vec<detail::PropertyRow>>,
    target: detail::PropertyTarget,
) -> impl Fn(&A, &mut Window, &mut App) + 'static {
    let state = state.clone();
    let bridge = bridge.clone();
    let targets = targets.clone();
    move |_: &A, _window, cx| match targets.iter().position(|row| row.target == target) {
        Some(index) => open_row(&state, &bridge, &targets, index, cx),
        None => cx.propagate(),
    }
}

/// The sheet's header: key, status, where the card is mirrored, and the card's verbs.
fn header(
    board: &fleet_core::board::Board,
    card: &Card,
    backend: &str,
    menu: &CardMenu,
    theme: &Theme,
) -> AnyElement {
    let status = board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id);
    let mut status_button = Button::new(
        "card-detail-status",
        status.map_or_else(|| "No status".to_owned(), |status| status.name.clone()),
    )
    .size(ButtonSize::Compact)
    .action(Box::new(board_actions::PickStatus));
    if let Some(status) = status {
        status_button = status_button.dot(category_accent(status, theme));
    }
    let synced = card.remote.as_ref().map(|remote| {
        Text::hint(format!(
            "{backend} {} \u{00b7} synced {}",
            remote.key,
            age_label(&remote.synced_at, now_unix())
        ))
    });
    let open_remote = menu.open_remote.then(|| {
        Button::new("card-detail-open-remote", format!("Open in {backend}"))
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .action(Box::new(card_actions::OpenRemote))
    });
    let conflicted = card.conflict.is_some();
    let delete = menu.delete;
    let more = PopoverMenu::new("card-detail-more")
        .anchor(MenuAnchor::BottomRight)
        .trigger_with(|open, _, _| {
            IconButton::new(
                "card-detail-more-trigger",
                Icon::Ellipsis,
                "More card actions",
            )
            .selected(open)
            .harness_target("card_detail.menu")
        })
        .menu(move |menu, _, _| {
            let mut menu = menu.item(menu_item(Box::new(card_actions::CreateWorktree)));
            if conflicted {
                menu = menu
                    .separator()
                    .item(menu_item(Box::new(card_actions::KeepLocal)))
                    .item(menu_item(Box::new(card_actions::TakeRemote)));
            }
            if delete {
                menu = menu
                    .separator()
                    .item(menu_item(Box::new(board_actions::DeleteCard)));
            }
            menu
        });
    div()
        .flex()
        .items_center()
        .w_full()
        .h(theme.metrics.button_h)
        .my(theme.space.sm)
        .pl(theme.space.xl)
        .gap(theme.space.sm)
        .child(
            Text::data_small(card.display_key(board))
                .faint()
                .flex_none(),
        )
        .child(status_button)
        .children(synced.map(|synced| synced.ellipsize()))
        .child(div().flex_1())
        .children(open_remote)
        .child(more)
        .into_any_element()
}

/// One menu entry running `action`, labelled from the catalogue, its key from the live keymap.
fn menu_item(action: Box<dyn gpui::Action>) -> MenuItem {
    let info = crate::action_catalogue::info(action.name());
    MenuItem::new(info.map_or("", |info| info.short_label))
        .destructive(info.is_some_and(|info| info.destructive))
        .action(action)
}

/// The description: the Markdown with an `Edit` button, the empty state, or the editor.
fn description(card: &Card, editor: Option<Entity<TextInput>>, theme: &Theme) -> AnyElement {
    let empty = card.description.trim().is_empty();
    let edit = (editor.is_none() && !empty).then(|| {
        Button::new("card-detail-edit-description", "Edit")
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .action(Box::new(card_actions::EditDescription))
    });
    let body: AnyElement = match editor {
        Some(input) => editing(input, CardEdit::Description, theme),
        None if empty => div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .child(Text::ui("No description").faint())
            .child(
                Button::new("card-detail-write-description", "Write one")
                    .style(ButtonStyle::Ghost)
                    .size(ButtonSize::Compact)
                    .action(Box::new(card_actions::EditDescription)),
            )
            .into_any_element(),
        None => MarkdownText::new(card.description.clone()).into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.xs)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(theme.metrics.button_h_compact)
                .child(Text::sentence_label("Description"))
                .children(edit),
        )
        .child(body)
        .into_any_element()
}

/// An open text edit: the shared input, then the buttons that save and discard it.
///
/// They run `ctrl-s` and `esc`, and show those keys, so the edit's way out is on the surface
/// rather than in a footer legend.
fn editing(input: Entity<TextInput>, surface: CardEdit, theme: &Theme) -> AnyElement {
    let save = match surface {
        CardEdit::Comment => "Comment",
        CardEdit::Title | CardEdit::Description => "Save",
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(theme.space.sm)
        .child(input)
        .child(
            div()
                .flex()
                .justify_end()
                .gap(theme.space.xs)
                .child(
                    Button::new("card-detail-edit-cancel", "Cancel")
                        .style(ButtonStyle::Ghost)
                        .size(ButtonSize::Compact)
                        .action(Box::new(card_actions::Close)),
                )
                .child(
                    Button::new("card-detail-edit-save", save)
                        .style(ButtonStyle::Primary)
                        .size(ButtonSize::Compact)
                        .action(Box::new(card_actions::Save))
                        .prefer_key(SAVE_KEY),
                ),
        )
        .into_any_element()
}

/// The save key the edit's primary button teaches, among `Save`'s bindings.
const SAVE_KEY: &str = "ctrl-enter";

/// Each property row's key chip, read from the live keymap once per frame.
struct PropertyKeys(Vec<Option<Kbd>>);

impl PropertyKeys {
    fn resolve(rows: &[detail::PropertyRow], window: &Window, cx: &App) -> Self {
        Self(
            rows.iter()
                .map(|row| {
                    row_action(row).and_then(|action| Kbd::for_action(action.as_ref(), window, cx))
                })
                .collect(),
        )
    }
}

/// The action whose key edits a row: the board's own field key where one exists, `⏎` otherwise.
///
/// Only the first row of a multi-row field names the key; the rows under it (`Blocked by`'s
/// second link) are still selected and opened by `⏎` or a click.
fn row_action(row: &detail::PropertyRow) -> Option<Box<dyn gpui::Action>> {
    use detail::PropertyTarget as T;
    Some(match &row.target {
        T::ReadOnly => return None,
        T::Worktree => Box::new(board_actions::OpenWorktree),
        T::Remote => Box::new(card_actions::OpenRemote),
        T::Pick(_) if row.label.is_empty() => Box::new(card_actions::EditProperty),
        T::Pick(PickerKind::Status) => Box::new(board_actions::PickStatus),
        T::Pick(PickerKind::Priority) => Box::new(board_actions::PickPriority),
        T::Pick(PickerKind::Assignee) => Box::new(board_actions::PickAssignee),
        T::Pick(PickerKind::Labels) => Box::new(board_actions::PickLabels),
        T::Pick(PickerKind::Estimate) => Box::new(board_actions::PickEstimate),
        T::Pick(PickerKind::BlockedBy) => Box::new(board_actions::PickBlockedBy),
        T::Pick(PickerKind::Provider) => Box::new(board_actions::PickAgent),
        T::Pick(_) => Box::new(card_actions::EditProperty),
    })
}

/// The right-hand column: the clickable property rows, then the activity folded under them.
fn properties(
    state: &Entity<AppState>,
    bridge: &Bridge,
    rows: &[detail::PropertyRow],
    draft: &CardDetailState,
    keys: PropertyKeys,
    activity: AnyElement,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    // A click selects the row and runs what `⏎` runs on it. Not while a text edit holds the
    // keyboard: the edit's own buttons are how it ends.
    let on_click: Option<detail::OnPropertyClick> = (!draft.is_editing()).then(|| {
        let state = state.clone();
        let bridge = bridge.clone();
        Rc::new(move |index: usize, _: &mut Window, cx: &mut App| {
            let targets = property_targets(&state, cx);
            open_row(&state, &bridge, &targets, index, cx);
        }) as detail::OnPropertyClick
    });
    // The card's own fields first; where it sits in the repository after a hairline.
    let repo_row = rows.iter().position(|row| row.label.as_ref() == "Repo");
    let list = div()
        .id("card-detail-properties")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .w_full()
        .gap(theme.space.xxs)
        .px(theme.space.sm)
        .py(theme.space.lg)
        .overflow_y_scroll()
        .child(
            div()
                .px(theme.space.sm)
                .pb(theme.space.xs)
                .child(Text::sentence_label("Properties")),
        )
        .children(
            rows.iter()
                .zip(keys.0)
                .enumerate()
                .flat_map(|(index, (row, kbd))| {
                    let divider = (Some(index) == repo_row)
                        .then(|| Divider::horizontal().inset(true).into_any_element());
                    let row = detail::property_row(
                        row,
                        detail::PropertyRowProps {
                            index,
                            selected: index == draft.property_row,
                            focused: !draft.is_editing(),
                            kbd,
                            on_click: on_click.clone(),
                        },
                        theme,
                    );
                    divider.into_iter().chain(std::iter::once(row))
                }),
        );
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(theme.metrics.sheet_detail_props_w)
        .h_full()
        .border_l(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(list)
        .child(
            div()
                .flex_none()
                .w_full()
                .px(theme.space.lg)
                .py(theme.space.md)
                .border_t(theme.metrics.hairline)
                .border_color(theme.colors.border)
                .child(activity),
        )
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

/// The card went away while the sheet was open — say so instead of showing an empty card.
pub(super) fn missing(state: &Entity<AppState>, focus: &FocusHandle, cx: &App) -> AnyElement {
    let state = state.clone();
    let theme = cx.theme();
    root(focus)
        .on_action(move |_: &dialog::Cancel, _window, cx| {
            state.update(cx, |app, cx| {
                app.close_overlay();
                cx.notify();
            });
            cx.stop_propagation();
        })
        .child(
            Sheet::new(true)
                .side(SheetSide::Right)
                .scrim(true)
                .width(theme.metrics.sheet_w_detail)
                .dismiss_action(Dialogs::CardDetail.dismiss_action())
                .close_target("card_detail.close")
                .body(EmptyState::new("That card is no longer on this board.")),
        )
        .into_any_element()
}
