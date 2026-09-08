use super::*;

/// Renders the board's own rows, the backend cycler, and one row per backend setting.
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    adopt_late_schema(state, cx);
    let draft = read_host(state, cx, |host, _| host.board_settings.clone());
    let repos = repo_choices(state.read(cx));
    let repo_index = repo_position(&repos, draft.default_repo_id.as_ref());
    let repo_off_grid = !repo_listed(&repos, draft.default_repo_id.as_ref());
    let policy_index = POLICIES
        .iter()
        .position(|policy| *policy == draft.conflict_policy)
        .unwrap_or(0);
    let kinds = backend_kinds(state.read(cx), &draft.backend_kind);
    let kind_index = kinds
        .iter()
        .position(|kind| *kind == draft.backend_kind)
        .unwrap_or(0);
    let backend_label = state.read(cx).backend_label(&draft.backend_kind);
    let focused = draft.focused();

    // The focused row is brought into view when it *changes*: `j` past the fold has to land
    // somewhere the user can see, and the card tops out at 90 % of the window however many
    // rows there are. Only when it changes, for the reason the board screen gates the same
    // call: an unconditional reveal re-anchors the list on every render and takes the wheel
    // away from the user.
    if let Some(row) = with_host(state, cx, |host| {
        (host.board_settings.revealed != Some(host.board_settings.row)).then(|| {
            host.board_settings.revealed = Some(host.board_settings.row);
            host.board_settings.row
        })
    }) {
        draft.scroll.scroll_to_item(row);
    }
    let rows = div()
        .id("board-settings-rows")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap(gap)
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .child(
            TextField::new(draft.name.clone())
                .label(SettingRow::Name.label())
                .placeholder("Fleet")
                .caret(if focused == SettingRow::Name {
                    draft.caret
                } else {
                    draft.name.chars().count()
                })
                .focused(focused == SettingRow::Name),
        )
        .child(
            TextField::new(draft.prefix.clone())
                .label(SettingRow::Prefix.label())
                .placeholder("FLT")
                .mono(true)
                .caret(if focused == SettingRow::Prefix {
                    draft.caret
                } else {
                    draft.prefix.chars().count()
                })
                .focused(focused == SettingRow::Prefix),
        )
        .child(
            Cycler::labeled(
                SettingRow::DefaultRepo.label(),
                draft
                    .default_repo_id
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), |repo| repo.as_str().to_owned()),
            )
            .label_width(px(LABEL_WIDTH))
            .has_prev(repo_off_grid || repo_index > 0)
            .has_next(repo_off_grid || repo_index < repos.len())
            // A stored repository this context no longer lists sits on no position of the
            // cycler: drawn as if it were "none" the row said `h` would do nothing and `l`
            // would move one step, while the value on screen was the repository id itself.
            .off_grid(repo_off_grid)
            .focused(focused == SettingRow::DefaultRepo),
        )
        .child(
            Toggle::labeled(SettingRow::StartOnWorktree.label(), draft.start_on_worktree)
                .label_width(px(LABEL_WIDTH))
                .detail("moves a backlog card to the first started column")
                .focused(focused == SettingRow::StartOnWorktree),
        )
        .child(
            Toggle::labeled(SettingRow::PushNewCards.label(), draft.push_new_cards)
                .label_width(px(LABEL_WIDTH))
                .detail("files a card made here as a new issue on the backend")
                .disabled(draft.backend_kind == BackendRef::LOCAL)
                .focused(focused == SettingRow::PushNewCards),
        )
        .child(
            Cycler::labeled(
                SettingRow::ConflictPolicy.label(),
                policy_label(draft.conflict_policy),
            )
            .label_width(px(LABEL_WIDTH))
            .has_prev(policy_index > 0)
            .has_next(policy_index + 1 < POLICIES.len())
            .disabled(draft.backend_kind == BackendRef::LOCAL)
            .focused(focused == SettingRow::ConflictPolicy),
        )
        .child(
            Cycler::labeled(SettingRow::Backend.label(), backend_label)
                .label_width(px(LABEL_WIDTH))
                .has_prev(kind_index > 0)
                .has_next(kind_index + 1 < kinds.len())
                .focused(focused == SettingRow::Backend),
        )
        .children(
            draft
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| backend_element(row, &draft, index)),
        );

    let mut card = Dialog::new("Board settings")
        .icon(Icon::Settings2)
        .width(Dialogs::BoardSettings.width(cx))
        .body(rows)
        .hint_row(
            KeyHintRow::new()
                .key("j/k", "row")
                .key("space", "toggle")
                .key("h/l", "cycle")
                .key("esc", "cancel"),
        )
        .primary("\u{23ce} Save");
    if let Some(message) = draft.error.clone().or_else(|| draft.validate()) {
        card = card.error(message);
    }

    let save_state = state.clone();
    let save_bridge = bridge.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                if insert(&state, text, cx) {
                    cx.stop_propagation();
                }
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
        // KEYMAP §Board says the board dialog rows are "in addition to everything the generic
        // Dialog context binds", and `tab` is one of them: without these two it was a dead key
        // here while every other multi-row dialog moved on it.
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| move_row(&state, 1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| move_row(&state, -1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_row(&state, -1, "", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::CycleNext, _window, cx| cycle(&state, 1, "l", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::CyclePrev, _window, cx| cycle(&state, -1, "h", cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                if !caret(&state, cx, TextFieldState::move_right) {
                    cycle(&state, 1, "", cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                if !caret(&state, cx, TextFieldState::move_left) {
                    cycle(&state, -1, "", cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &settings_actions::Toggle, _window, cx| toggle(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                edit(&state, cx, |input| {
                    input.backspace();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                edit(&state, cx, |input| {
                    input.delete_word_before();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                edit(&state, cx, |input| {
                    input.clear();
                });
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                caret(&state, cx, TextFieldState::move_to_start);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                caret(&state, cx, TextFieldState::move_to_end);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            save(&save_state, &save_bridge, cx);
            cx.stop_propagation();
        })
        .child(card)
        .into_any_element()
}

/// Fills in the backend rows when the registry answers after the dialog was seeded.
///
/// `,` can be pressed on the first frame after a reconnect, before `ListBoardBackends` has
/// come back; without this the dialog would show a `Backend` row and no settings under it for
/// as long as it stayed open. Rows are only ever built when there are none, so nothing the
/// user has typed can be overwritten — and the cursor stays exactly where it was.
fn adopt_late_schema(state: &Entity<AppState>, cx: &mut App) {
    let (kind, empty) = read_host(state, cx, |host, _| {
        (
            host.board_settings.backend_kind.clone(),
            host.board_settings.rows.is_empty(),
        )
    });
    if !empty {
        return;
    }
    let schema = schema_for(state.read(cx), &kind);
    if schema.is_empty() {
        return;
    }
    with_host(state, cx, |host| {
        let (row, caret) = (host.board_settings.row, host.board_settings.caret);
        host.board_settings.select_backend(&kind, &schema);
        host.board_settings.row = row;
        host.board_settings.caret = caret;
    });
}

/// Draws one backend settings row as the control its `PropertyKind` names.
fn backend_element(row: &BackendRow, draft: &BoardSettingsState, index: usize) -> AnyElement {
    let focused = draft.focused() == SettingRow::BackendSetting(index);
    let label = if row.required {
        format!("{} \u{2217}", row.name)
    } else {
        row.name.clone()
    };
    match row.kind {
        PropertyKind::Bool => Toggle::labeled(label, row.flag())
            .label_width(px(LABEL_WIDTH))
            .focused(focused)
            .into_any_element(),
        PropertyKind::Select => {
            let position = row
                .options
                .iter()
                .position(|option| option.value == row.value);
            let shown =
                position.map_or_else(|| row.value.clone(), |at| row.options[at].label.clone());
            Cycler::labeled(
                label,
                if shown.is_empty() {
                    "none".to_owned()
                } else {
                    shown
                },
            )
            .label_width(px(LABEL_WIDTH))
            .has_prev(position.is_some_and(|at| at > 0) || position.is_none())
            .has_next(position.is_none_or(|at| at + 1 < row.options.len()))
            .off_grid(position.is_none() && !row.value.is_empty())
            .focused(focused)
            .into_any_element()
        }
        // An unset optional number is drawn as an empty field with its placeholder, never as
        // `0`: every backend number row here has a non-zero default, and a dialog that shows
        // `0` states a value the daemon is not using.
        // …and only while it is not the row being edited: `BackendRow::is_text` is true for a
        // number, so `left`/`right`/`ctrl-a`/`ctrl-e` move a caret `NumberField` does not draw.
        // On "10" that turned a `left` and a typed `5` into "150" with nothing on screen to
        // explain it. The focused row is drawn as the text field it is actually edited as.
        PropertyKind::Number if !row.value.trim().is_empty() && !focused => {
            let mut field = NumberField::labeled(label, row.value.trim().parse().unwrap_or(0))
                .min(0)
                .focused(false);
            if let Some(message) = row.error() {
                field = field.invalid(message);
            }
            field.into_any_element()
        }
        _ => {
            let mut field = TextField::new(row.value.clone())
                .label(label)
                .placeholder(placeholder(row))
                .mono(row.kind == PropertyKind::Number)
                .caret(if focused {
                    draft.caret
                } else {
                    row.value.chars().count()
                })
                .focused(focused);
            if let Some(message) = row.error() {
                field = field.invalid(message);
            }
            field.into_any_element()
        }
    }
}

/// What an empty backend text row suggests.
#[must_use]
fn placeholder(row: &BackendRow) -> &'static str {
    match row.kind {
        PropertyKind::MultiSelect => "comma, separated, values",
        PropertyKind::Date => "YYYY-MM-DD",
        _ if row.required => "required",
        _ => "optional",
    }
}
