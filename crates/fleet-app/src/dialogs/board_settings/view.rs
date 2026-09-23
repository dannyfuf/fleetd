use super::*;

/// Renders the rail, the divider and whichever pane the rail has selected (§5.4).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let theme = cx.theme();
        (theme.space.md, theme.space.xs)
    };
    adopt_late_schema(state, cx);
    let (draft, input) = read_host(state, cx, |host, _| {
        (
            host.board_settings.clone(),
            host.board_settings_input.clone(),
        )
    });
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

    let rail = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(RAIL_WIDTH))
        .gap(tight)
        .children(BoardSection::ALL.iter().map(|section| {
            let selected = *section == draft.section;
            Row::new()
                .selected(selected)
                .cursor(selected)
                .column(RowColumn::flex(Text::ui(section.title())))
        }));

    let rows = match draft.section {
        BoardSection::General => general_rows(state, &draft, input.as_ref(), focused, cx),
        BoardSection::Backend => backend_pane(state, &draft, input.as_ref(), focused, cx),
        BoardSection::Columns => columns_pane(&draft, input.as_ref(), tight),
    };
    let pane = div()
        .id("board-settings-pane")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .gap(tight)
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .child(rows)
        .children(
            draft
                .notice
                .clone()
                .map(|notice| Text::hint(notice).muted()),
        )
        // §5.4: the trailer is stated once, under the rows, and only where it explains
        // something the user can see — a form full of rows they cannot reach.
        .children(
            (draft.automation_locked && draft.section == BoardSection::Columns)
                .then(|| Text::hint("Automation is available on worktree boards").muted()),
        );

    let body = div()
        .flex()
        .flex_row()
        .gap(gap)
        .size_full()
        .child(rail)
        .child(Divider::vertical())
        .child(pane);

    let mut card = Dialog::new("Board settings")
        .dismiss_action(crate::dialogs::Dialogs::BoardSettings.dismiss_action())
        .icon(Icon::Settings2)
        .width(Dialogs::BoardSettings.width(cx))
        .when_some(Dialogs::BoardSettings.height(), Dialog::height)
        .body(body)
        .hint_row(hints(&draft))
        .primary("^s Save");
    if let Some(message) = draft.error.clone().or_else(|| draft.validate()) {
        card = card.error(message);
    }

    actions(root(focus), state, bridge, focus)
        .child(card)
        .into_any_element()
}

/// The hint row of the open pane.
///
/// The Columns list states its own five keys verbatim (§5.4); every other pane states the
/// vocabulary the global Settings dialog states, plus the save this one moved to `^s`.
#[must_use]
fn hints(draft: &BoardSettingsState) -> KeyHintRow {
    if draft.in_column_list() {
        return KeyHintRow::new()
            .key("n", "new")
            .key("d", "delete")
            .key("J/K", "reorder")
            .key("P", "preset")
            .key("\u{23ce}", "open");
    }
    if draft.opened_column.is_some() {
        return KeyHintRow::new()
            .key("j/k", "row")
            .key("h/l", "cycle")
            .key("\u{23ce}", "edit")
            .key("^s", "save")
            .key("esc", "back");
    }
    KeyHintRow::new()
        .key("\u{21e5}", "section")
        .key("j/k", "row")
        .key("space", "toggle")
        .key("h/l", "cycle")
        .key("^s", "save")
}

/// The General pane: the board's own facts, and the throttle its runs share.
fn general_rows(
    state: &Entity<AppState>,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    focused: SettingRow,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    let repos = repo_choices(state.read(cx));
    let repo_index = repo_position(&repos, draft.default_repo_id.as_ref());
    let repo_off_grid = !repo_listed(&repos, draft.default_repo_id.as_ref());
    let policy_index = POLICIES
        .iter()
        .position(|policy| *policy == draft.conflict_policy)
        .unwrap_or(0);
    let limit = draft.live_run_limit();
    div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(if focused == SettingRow::Name {
            editor(input)
        } else {
            // §6.4: a value nobody is editing is a read-only fact, not an empty box. The
            // editor `Enter` opens carries the placeholder and the label.
            FactRow::new(
                SettingRow::Name.label(),
                FactValue::from_option((!draft.name.is_empty()).then(|| draft.name.clone())),
            )
            .label_width(px(LABEL_WIDTH))
            .into_any_element()
        })
        .child(if focused == SettingRow::Prefix {
            editor(input)
        } else {
            FactRow::new(
                SettingRow::Prefix.label(),
                FactValue::from_option((!draft.prefix.is_empty()).then(|| draft.prefix.clone())),
            )
            .label_width(px(LABEL_WIDTH))
            .mono(true)
            .into_any_element()
        })
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
            // The hint is its own line rather than a `detail`, which the cycler has no slot
            // for: it explains the *consequence* of the number, and the number is what the
            // row states.
            div()
                .flex()
                .flex_col()
                .child(
                    Cycler::labeled(SettingRow::MaxLiveRuns.label(), limit.to_string())
                        .label_width(px(LABEL_WIDTH))
                        .has_prev(limit > 1)
                        .has_next(limit < MAX_LIVE_RUNS_PER_BOARD)
                        .focused(focused == SettingRow::MaxLiveRuns),
                )
                .child(Text::hint("runs share one checkout").muted()),
        )
        .into_any_element()
}

/// The Backend pane: which backend mirrors the board, and that backend's own rows.
fn backend_pane(
    state: &Entity<AppState>,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    focused: SettingRow,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    let kinds = backend_kinds(state.read(cx), &draft.backend_kind);
    let kind_index = kinds
        .iter()
        .position(|kind| *kind == draft.backend_kind)
        .unwrap_or(0);
    let backend_label = state.read(cx).backend_label(&draft.backend_kind);
    div()
        .flex()
        .flex_col()
        .gap(gap)
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
                .map(|(index, row)| backend_element(row, draft, input, index)),
        )
        .into_any_element()
}

/// The Columns pane: the column list, or the column the list drilled into.
fn columns_pane(
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    gap: gpui::Pixels,
) -> AnyElement {
    let focused = draft.row;
    div()
        .flex()
        .flex_col()
        .gap(gap)
        .children(
            draft
                .opened()
                .map(|column| Text::hint(format!("Columns \u{203a} {}", column.status.name))),
        )
        .children(
            draft
                .prepared
                .iter()
                .enumerate()
                .map(|(index, row)| column_element(row, index == focused, input)),
        )
        .into_any_element()
}

/// Draws one prepared Columns row as the control its value calls for.
fn column_element(row: &ColumnRow, focused: bool, input: Option<&Entity<TextInput>>) -> AnyElement {
    match &row.value {
        ColumnValue::Column { name, has_action } => {
            Row::new()
                .selected(focused)
                .cursor(focused)
                .column(RowColumn::flex(Text::ui(name.clone())))
                .columns(has_action.then(|| {
                    RowColumn::auto(Icon::Zap.el().size(IconSize::Small).tone(Tone::Muted))
                }))
                .into_any_element()
        }
        ColumnValue::Choice {
            value,
            has_prev,
            has_next,
        } => Cycler::labeled(row.label.clone(), value.clone())
            .label_width(px(LABEL_WIDTH))
            .has_prev(*has_prev)
            .has_next(*has_next)
            .disabled(row.disabled)
            .focused(focused)
            .into_any_element(),
        ColumnValue::Text { value, mono } => {
            if focused
                && !row.disabled
                && let Some(input) = input
            {
                return input.clone().into_any_element();
            }
            // A row this board may not carry is drawn by the kit's disabled row rather than as
            // a fact: a `FactRow` states a value as true, and an automation row on a context
            // board is not a fact about that board at all.
            if row.disabled {
                return Row::new()
                    .disabled(true)
                    .column(RowColumn::fixed(
                        px(LABEL_WIDTH),
                        Text::ui(row.label.clone()),
                    ))
                    .column(RowColumn::flex(Text::ui(value.clone())))
                    .into_any_element();
            }
            FactRow::new(
                row.label.clone(),
                FactValue::from_option((!value.is_empty()).then(|| value.clone())),
            )
            .label_width(px(LABEL_WIDTH))
            .mono(*mono)
            .into_any_element()
        }
    }
}

/// The one row-scoped editor, or nothing when it has not been built yet.
fn editor(input: Option<&Entity<TextInput>>) -> AnyElement {
    input.cloned().map_or_else(
        || div().into_any_element(),
        gpui::IntoElement::into_any_element,
    )
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
        let row = host.board_settings.row;
        host.board_settings.select_backend(&kind, &schema);
        host.board_settings.row = row;
    });
}

/// Draws one backend settings row as the control its `PropertyKind` names.
fn backend_element(
    row: &BackendRow,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    index: usize,
) -> AnyElement {
    let focused = draft.focused() == SettingRow::BackendSetting(index);
    let label = if row.required {
        format!("{} \u{2217}", row.name)
    } else {
        row.name.clone()
    };
    if focused && row.is_text() {
        return editor(input);
    }
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
        PropertyKind::Number if !row.value.trim().is_empty() => {
            let mut field = NumberField::labeled(label, row.value.trim().parse().unwrap_or(0))
                .min(0)
                .focused(false);
            if let Some(message) = row.error() {
                field = field.invalid(message);
            }
            field.into_any_element()
        }
        _ => {
            let mut column = div().flex().flex_col().child(
                FactRow::new(
                    label,
                    FactValue::from_option((!row.value.is_empty()).then(|| row.value.clone())),
                )
                .label_width(px(LABEL_WIDTH))
                .mono(row.kind == PropertyKind::Number),
            );
            if let Some(message) = row.error() {
                column = column.child(FactRow::warning(message));
            }
            column.into_any_element()
        }
    }
}

/// Every key the dialog answers, on the element that tracks its focus.
fn actions(
    root: gpui::Div,
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
) -> gpui::Div {
    root.on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &settings_actions::MoveDown, window, cx| move_to_row(&state, 1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &settings_actions::MoveUp, window, cx| move_to_row(&state, -1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::CursorDown, window, cx| move_to_row(&state, 1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::CursorUp, window, cx| move_to_row(&state, -1, &focus, window, cx)
    })
    // §5.4: the rail answers `⇥` here exactly as the global Settings dialog's does, which is
    // why `tab` is a section rather than the next field.
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::NextField, window, cx| move_to_section(&state, 1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::PrevField, window, cx| move_to_section(&state, -1, &focus, window, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CycleNext, _window, cx| cycle(&state, 1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CyclePrev, _window, cx| cycle(&state, -1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::Toggle, _window, cx| toggle(&state, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &board_settings_actions::NewColumn, _window, cx| add_column(&state, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &board_settings_actions::DeleteColumn, _window, cx| arm_delete(&state, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &board_settings_actions::MoveColumnDown, _window, cx| move_column(&state, 1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &board_settings_actions::MoveColumnUp, _window, cx| move_column(&state, -1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &board_settings_actions::ApplyPreset, _window, cx| preset(&state, cx)
    })
    .on_action({
        let state = state.clone();
        let bridge = bridge.clone();
        move |_: &board_settings_actions::Save, _window, cx| {
            save(&state, &bridge, cx);
            cx.stop_propagation();
        }
    })
    .on_action({
        let state = state.clone();
        let bridge = bridge.clone();
        let focus = focus.clone();
        move |_: &dialog::Confirm, window, cx| {
            // A delete waiting for a target is answered by the column the cursor is on; the
            // Columns pane then owns `⏎` for drilling in and editing, and every other pane
            // keeps §3.8.6's meaning, which is to save.
            if delete_armed(&state, cx) {
                delete_with_cards(&state, &bridge, cx);
            } else if !confirm_column(&state, window, &focus, cx) {
                save(&state, &bridge, cx);
            }
            cx.stop_propagation();
        }
    })
    .on_action({
        let state = state.clone();
        let focus = focus.clone();
        move |_: &dialog::Cancel, window, cx| {
            // `esc` leaves the editor, then the column, then asks once — and only when it has
            // nothing left to leave does it reach the shell, which is the one path that closes
            // an overlay.
            if cancel(&state, window, &focus, cx) {
                cx.stop_propagation();
            } else {
                cx.propagate();
            }
        }
    })
}

/// Whether `⏎` is answering a delete that is waiting for a target.
fn delete_armed(state: &Entity<AppState>, cx: &mut App) -> bool {
    read_host(state, cx, |host, _| {
        host.board_settings.pending_delete.is_some()
    })
}

fn move_to_row(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    move_row(state, delta, cx);
    materialize_input(state, Some(window), Some(focus), cx);
}

fn move_to_section(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    cycle_section(state, delta, cx);
    materialize_input(state, Some(window), Some(focus), cx);
}
