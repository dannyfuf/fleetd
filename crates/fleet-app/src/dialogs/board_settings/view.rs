use super::*;

/// Everything a click on this dialog needs, cloned into each handler that needs it.
#[derive(Clone)]
pub(super) struct Wire {
    pub(super) state: Entity<AppState>,
    pub(super) bridge: Bridge,
    pub(super) focus: FocusHandle,
}

impl Wire {
    /// Row `row` of the open pane, made clickable: a press puts the cursor there.
    pub(super) fn row(&self, row: usize, element: impl IntoElement) -> AnyElement {
        let wire = self.clone();
        div()
            .id(("board-settings-row", row))
            .w_full()
            .on_click(move |_, window, cx| {
                select_row(&wire.state, row, &wire.focus, window, cx);
            })
            .child(element)
            .into_any_element()
    }

    /// A closed choice's click, for the row showing option `current`.
    pub(super) fn pick(
        &self,
        row: usize,
        current: Option<usize>,
    ) -> impl Fn(usize, &mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |option, window, cx| {
            pick(&wire.state, row, option, current, &wire.focus, window, cx);
        }
    }

    /// A flag's switch.
    pub(super) fn switch(&self, row: usize) -> impl Fn(bool, &mut Window, &mut App) + 'static {
        let wire = self.clone();
        move |on, window, cx| switch(&wire.state, row, on, &wire.focus, window, cx)
    }
}

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
    let wire = Wire {
        state: state.clone(),
        bridge: bridge.clone(),
        focus: focus.clone(),
    };

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
        .children(draft.sections().iter().enumerate().map(|(index, section)| {
            let selected = *section == draft.section;
            let wire = wire.clone();
            Row::with_id(("board-settings-section", index))
                .selected(selected)
                .cursor(selected)
                .column(RowColumn::flex(Text::ui(section.title())))
                .on_click(move |_, window, cx| {
                    select_section(&wire.state, &wire.bridge, index, &wire.focus, window, cx);
                })
        }));

    let rows = match draft.section {
        BoardSection::General => general_rows(&wire, &draft, input.as_ref(), focused, cx),
        BoardSection::Backend => backend_pane(&wire, &draft, input.as_ref(), focused, cx),
        BoardSection::Columns => columns_pane(&wire, &draft, input.as_ref(), tight),
        BoardSection::Schedules => schedules_pane(&wire, &draft, input.as_ref(), cx),
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
        .dismiss_action(Dialogs::BoardSettings.dismiss_action())
        .icon(Icon::Settings2)
        .width(Dialogs::BoardSettings.width(cx))
        .when_some(Dialogs::BoardSettings.height(), Dialog::height)
        .body(body)
        .when_some(footer_start(&draft), Dialog::footer_start)
        .actions(vec![
            footer::cancel(&Dialogs::BoardSettings),
            footer::primary(
                "board-settings-save",
                "Save",
                Box::new(board_settings_actions::Save),
            )
            // With a schedule's form open, `^s` saves that schedule, and so does this button.
            .disabled(
                if draft.schedules.form.is_some() && draft.section == BoardSection::Schedules {
                    draft.schedules.busy
                } else {
                    draft.saving || !draft.dirty()
                },
            ),
        ]);
    if let Some(message) = draft.error.clone().or_else(|| draft.validate()) {
        card = card.error(message);
    }

    actions(root(focus), state, bridge, focus)
        .child(card)
        .into_any_element()
}

/// The footer's left side: the Columns list's own verbs, or the way back out of a column.
///
/// Each button dispatches the action its key runs, so its chip is that key; outside the list
/// the verbs are not offered at all, as their keys do nothing there.
fn footer_start(draft: &BoardSettingsState) -> Option<AnyElement> {
    let ghost =
        |id: &'static str, label: &'static str, icon: Icon, action: Box<dyn gpui::Action>| {
            Button::new(id, label)
                .style(ButtonStyle::Ghost)
                .icon(icon)
                .action(action)
        };
    if draft.in_schedule_list() && draft.schedules_supported {
        return Some(
            div()
                .flex()
                .items_center()
                .children([
                    ghost(
                        "board-settings-new-schedule",
                        "New schedule",
                        Icon::Plus,
                        Box::new(board_settings_actions::NewColumn),
                    ),
                    ghost(
                        "board-settings-run-schedule",
                        "Run now",
                        Icon::Zap,
                        Box::new(board_settings_actions::RunScheduleNow),
                    ),
                    ghost(
                        "board-settings-delete-schedule",
                        "Delete",
                        Icon::Trash2,
                        Box::new(board_settings_actions::DeleteColumn),
                    ),
                ])
                .into_any_element(),
        );
    }
    if draft.section == BoardSection::Schedules {
        return (draft.schedules.form.is_some() && !draft.editing).then(|| {
            ghost(
                "board-settings-back",
                "Schedules",
                Icon::ChevronLeft,
                Box::new(dialog::Cancel),
            )
            .into_any_element()
        });
    }
    if draft.in_column_list() && draft.pending_delete.is_none() {
        return Some(
            div()
                .flex()
                .items_center()
                .children([
                    ghost(
                        "board-settings-new-column",
                        "New column",
                        Icon::Plus,
                        Box::new(board_settings_actions::NewColumn),
                    ),
                    ghost(
                        "board-settings-delete-column",
                        "Delete",
                        Icon::Trash2,
                        Box::new(board_settings_actions::DeleteColumn),
                    ),
                    ghost(
                        "board-settings-preset",
                        "Apply preset",
                        Icon::Sparkles,
                        Box::new(board_settings_actions::ApplyPreset),
                    ),
                ])
                .into_any_element(),
        );
    }
    // `esc` leaves an open column for the list; while an editor is open it leaves the editor
    // first, which is not what this button says, so it waits until the editor is closed.
    (draft.opened_column.is_some() && !draft.editing).then(|| {
        ghost(
            "board-settings-back",
            "Columns",
            Icon::ChevronLeft,
            Box::new(dialog::Cancel),
        )
        .into_any_element()
    })
}

/// The General pane: the board's own facts, and the throttle its runs share.
fn general_rows(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    focused: SettingRow,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    let repos = repo_choices(wire.state.read(cx));
    let repo_index = repo_position(&repos, draft.default_repo_id.as_ref());
    let repo_off_grid = !repo_listed(&repos, draft.default_repo_id.as_ref());
    let policy_index = POLICIES
        .iter()
        .position(|policy| *policy == draft.conflict_policy)
        .unwrap_or(0);
    let limit = draft.live_run_limit();
    let at = |row: SettingRow| {
        GENERAL_ROWS
            .iter()
            .position(|general| *general == row)
            .unwrap_or(0)
    };
    let (name, prefix, repo, start, push, policy, runs) = (
        at(SettingRow::Name),
        at(SettingRow::Prefix),
        at(SettingRow::DefaultRepo),
        at(SettingRow::StartOnWorktree),
        at(SettingRow::PushNewCards),
        at(SettingRow::ConflictPolicy),
        at(SettingRow::MaxLiveRuns),
    );
    div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(wire.row(
            name,
            if focused == SettingRow::Name {
                editor(input)
            } else {
                // §6.4: a value nobody is editing is a read-only fact, not an empty box. The
                // editor a click or `j` / `k` opens carries the placeholder and the label.
                FactRow::new(
                    SettingRow::Name.label(),
                    FactValue::from_option((!draft.name.is_empty()).then(|| draft.name.clone())),
                )
                .label_width(px(LABEL_WIDTH))
                .into_any_element()
            },
        ))
        .child(wire.row(
            prefix,
            if focused == SettingRow::Prefix {
                editor(input)
            } else {
                FactRow::new(
                    SettingRow::Prefix.label(),
                    FactValue::from_option(
                        (!draft.prefix.is_empty()).then(|| draft.prefix.clone()),
                    ),
                )
                .label_width(px(LABEL_WIDTH))
                .mono(true)
                .into_any_element()
            },
        ))
        .child(
            wire.row(
                repo,
                Cycler::labeled(
                    SettingRow::DefaultRepo.label(),
                    draft
                        .default_repo_id
                        .as_ref()
                        .map_or_else(|| "none".to_owned(), |repo| repo.as_str().to_owned()),
                )
                .id("board-settings-default-repo")
                .options(
                    std::iter::once("none".to_owned())
                        .chain(repos.iter().map(|repo| repo.as_str().to_owned())),
                )
                .on_select(wire.pick(repo, (!repo_off_grid).then_some(repo_index)))
                .label_width(px(LABEL_WIDTH))
                .has_prev(repo_off_grid || repo_index > 0)
                .has_next(repo_off_grid || repo_index < repos.len())
                // A stored repository this context no longer lists sits on no position of the
                // cycler: drawn as if it were "none" the row said `h` would do nothing and `l`
                // would move one step, while the value on screen was the repository id itself.
                .off_grid(repo_off_grid)
                .focused(focused == SettingRow::DefaultRepo),
            ),
        )
        .child(
            wire.row(
                start,
                Toggle::labeled(SettingRow::StartOnWorktree.label(), draft.start_on_worktree)
                    .id("board-settings-start-on-worktree")
                    .label_width(px(LABEL_WIDTH))
                    .detail("moves a backlog card to the first started column")
                    .on_toggle(wire.switch(start))
                    .focused(focused == SettingRow::StartOnWorktree),
            ),
        )
        .child(
            wire.row(
                push,
                Toggle::labeled(SettingRow::PushNewCards.label(), draft.push_new_cards)
                    .id("board-settings-push-new-cards")
                    .label_width(px(LABEL_WIDTH))
                    .detail("files a card made here as a new issue on the backend")
                    .disabled(draft.backend_kind == BackendRef::LOCAL)
                    .on_toggle(wire.switch(push))
                    .focused(focused == SettingRow::PushNewCards),
            ),
        )
        .child(
            wire.row(
                policy,
                Cycler::labeled(
                    SettingRow::ConflictPolicy.label(),
                    policy_label(draft.conflict_policy),
                )
                .id("board-settings-conflict-policy")
                .options(POLICIES.iter().map(|policy| policy_label(*policy)))
                .on_select(wire.pick(policy, Some(policy_index)))
                .label_width(px(LABEL_WIDTH))
                .has_prev(policy_index > 0)
                .has_next(policy_index + 1 < POLICIES.len())
                .disabled(draft.backend_kind == BackendRef::LOCAL)
                .focused(focused == SettingRow::ConflictPolicy),
            ),
        )
        .child(
            // The hint is its own line rather than a `detail`, which the cycler has no slot
            // for: it explains the *consequence* of the number, and the number is what the
            // row states.
            wire.row(
                runs,
                div()
                    .flex()
                    .flex_col()
                    .child(
                        Cycler::labeled(SettingRow::MaxLiveRuns.label(), limit.to_string())
                            .id("board-settings-max-live-runs")
                            .options((1..=MAX_LIVE_RUNS_PER_BOARD).map(|runs| runs.to_string()))
                            .on_select(wire.pick(runs, Some(limit.saturating_sub(1) as usize)))
                            .label_width(px(LABEL_WIDTH))
                            .has_prev(limit > 1)
                            .has_next(limit < MAX_LIVE_RUNS_PER_BOARD)
                            .focused(focused == SettingRow::MaxLiveRuns),
                    )
                    .child(
                        Text::hint(if draft.run_location.is_board_worktree() {
                            "runs share one checkout"
                        } else {
                            "each run works in its card's worktree"
                        })
                        .muted(),
                    ),
            ),
        )
        // A fact, not a row: nothing moves the board between the two in v1, because a change
        // on a board with live runs would strand them (BOARD §11.10).
        .child(
            FactRow::new(
                RUNS_IN_LABEL,
                FactValue::known(run_location_label(draft.run_location)),
            )
            .label_width(px(LABEL_WIDTH)),
        )
        .into_any_element()
}

/// The Backend pane: which backend mirrors the board, and that backend's own rows.
fn backend_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    focused: SettingRow,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.sm;
    let app = wire.state.read(cx);
    let kinds = backend_kinds(app, &draft.backend_kind);
    let kind_index = kinds
        .iter()
        .position(|kind| *kind == draft.backend_kind)
        .unwrap_or(0);
    let backend_label = app.backend_label(&draft.backend_kind);
    let kind_labels: Vec<String> = kinds.iter().map(|kind| app.backend_label(kind)).collect();
    div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(
            wire.row(
                0,
                Cycler::labeled(SettingRow::Backend.label(), backend_label)
                    .id("board-settings-backend")
                    .options(kind_labels)
                    .on_select(wire.pick(0, Some(kind_index)))
                    .label_width(px(LABEL_WIDTH))
                    .has_prev(kind_index > 0)
                    .has_next(kind_index + 1 < kinds.len())
                    .focused(focused == SettingRow::Backend),
            ),
        )
        .children(draft.rows.iter().enumerate().map(|(index, row)| {
            wire.row(index + 1, backend_element(wire, row, draft, input, index))
        }))
        .into_any_element()
}

/// The Columns pane: the column list, or the column the list drilled into.
fn columns_pane(
    wire: &Wire,
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
            draft.prepared.iter().enumerate().map(|(index, row)| {
                column_element(wire, draft, row, index, index == focused, input)
            }),
        )
        .into_any_element()
}

/// Draws one prepared Columns row as the control its value calls for.
fn column_element(
    wire: &Wire,
    draft: &BoardSettingsState,
    row: &ColumnRow,
    index: usize,
    focused: bool,
    input: Option<&Entity<TextInput>>,
) -> AnyElement {
    match &row.value {
        ColumnValue::Column { name, has_action } => column_list_row(
            wire,
            draft.pending_delete,
            name,
            *has_action,
            index,
            focused,
        ),
        ColumnValue::Choice {
            value,
            options,
            at,
            has_prev,
            has_next,
        } => wire.row(
            index,
            Cycler::labeled(row.label.clone(), value.clone())
                .id(("board-settings-column-field", index))
                .options(options.iter().cloned())
                .on_select(wire.pick(index, Some(*at)))
                .label_width(px(LABEL_WIDTH))
                .has_prev(*has_prev)
                .has_next(*has_next)
                .disabled(row.disabled)
                .focused(focused),
        ),
        ColumnValue::Action { value, options, at } => {
            if focused
                && draft.editing
                && !row.disabled
                && let Some(input) = input
            {
                return input.clone().into_any_element();
            }
            wire.row(
                index,
                Cycler::labeled(row.label.clone(), value.clone())
                    .id(("board-settings-column-field", index))
                    .options(options.iter().cloned())
                    .on_select(wire.pick(index, Some(*at)))
                    .label_width(px(LABEL_WIDTH))
                    // Three options, so both arrows are live somewhere; the cycle clamps.
                    .has_prev(*at > 0)
                    .has_next(*at + 1 < options.len())
                    .disabled(row.disabled)
                    .focused(focused),
            )
        }
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
            wire.row(
                index,
                FactRow::new(
                    row.label.clone(),
                    FactValue::from_option((!value.is_empty()).then(|| value.clone())),
                )
                .label_width(px(LABEL_WIDTH))
                .mono(*mono),
            )
        }
    }
}

/// One column of the list: a click selects it, a double-click opens it, and ↑ / ↓ on hover
/// move it past its neighbour (`K` / `J`).
///
/// While a delete waits for a target the list *is* the choice: the doomed column says so, and a
/// click on any other column moves the cards there, as `⏎` on it would.
fn column_list_row(
    wire: &Wire,
    pending_delete: Option<usize>,
    name: &str,
    has_action: bool,
    index: usize,
    focused: bool,
) -> AnyElement {
    let doomed = pending_delete == Some(index);
    let select = wire.clone();
    let open = wire.clone();
    let row = Row::with_id(("board-settings-column", index))
        .selected(focused)
        .cursor(focused)
        .column(RowColumn::flex(Text::ui(name.to_owned())))
        .columns(
            has_action
                .then(|| RowColumn::auto(Icon::Zap.el().size(IconSize::Small).tone(Tone::Muted))),
        );
    if pending_delete.is_some() {
        if doomed {
            return row
                .dimmed(true)
                .column(RowColumn::auto(Badge::new("deleting")))
                .into_any_element();
        }
        return row
            .column(RowColumn::auto(Text::hint("Move cards here").muted()).hover_only())
            .on_click(move |_, window, cx| {
                choose_target(
                    &select.state,
                    &select.bridge,
                    index,
                    &select.focus,
                    window,
                    cx,
                );
            })
            .into_any_element();
    }
    let arrow =
        |id: &'static str, icon: Icon, label: &'static str, action: Box<dyn gpui::Action>| {
            let wire = wire.clone();
            IconButton::new((id, index), icon, label)
                .size(ButtonSize::Compact)
                .on_click(move |_, window, cx| {
                    select_row(&wire.state, index, &wire.focus, window, cx);
                })
                .action(action)
        };
    row.hover_actions(
        div()
            .flex()
            .items_center()
            .child(arrow(
                "board-settings-column-up",
                Icon::CircleArrowUp,
                "Move up",
                Box::new(board_settings_actions::MoveColumnUp),
            ))
            .child(arrow(
                "board-settings-column-down",
                Icon::CircleArrowDown,
                "Move down",
                Box::new(board_settings_actions::MoveColumnDown),
            )),
    )
    .on_click(move |_, window, cx| {
        select_row(&select.state, index, &select.focus, window, cx);
    })
    .on_double_click(move |_, window, cx| {
        open_column(&open.state, index, &open.focus, window, cx);
    })
    .into_any_element()
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
    wire: &Wire,
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
            .id(("board-settings-backend-row", index))
            .label_width(px(LABEL_WIDTH))
            .on_toggle(wire.switch(index + 1))
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
            .id(("board-settings-backend-row", index))
            .options(row.options.iter().map(|option| option.label.clone()))
            .on_select(wire.pick(index + 1, position))
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
        let bridge = bridge.clone();
        let focus = focus.clone();
        move |_: &dialog::NextField, window, cx| {
            move_to_section(&state, &bridge, 1, &focus, window, cx);
        }
    })
    .on_action({
        let state = state.clone();
        let bridge = bridge.clone();
        let focus = focus.clone();
        move |_: &dialog::PrevField, window, cx| {
            move_to_section(&state, &bridge, -1, &focus, window, cx);
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CycleNext, _window, cx| cycle(&state, 1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &settings_actions::CyclePrev, _window, cx| cycle(&state, -1, cx)
    })
    // `space`, `n` and `d` are the Schedules list's verbs there, and the Columns list's (or a
    // flag row's) everywhere else: one key, one meaning per pane.
    .on_action({
        let state = state.clone();
        let bridge = bridge.clone();
        move |_: &settings_actions::Toggle, _window, cx| {
            if toggle_listed(&state, &bridge, cx) {
                cx.stop_propagation();
            } else {
                toggle(&state, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &board_settings_actions::NewColumn, _window, cx| {
            if !new_schedule(&state, cx) {
                add_column(&state, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        let bridge = bridge.clone();
        move |_: &board_settings_actions::DeleteColumn, _window, cx| {
            if !delete_schedule(&state, &bridge, cx) {
                arm_delete(&state, cx);
            }
        }
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
        move |_: &board_settings_actions::RunScheduleNow, _window, cx| {
            run_focused_schedule(&state, &bridge, cx);
        }
    })
    .on_action({
        let state = state.clone();
        let bridge = bridge.clone();
        move |_: &board_settings_actions::Save, _window, cx| {
            if !save_schedule(&state, &bridge, cx) {
                save(&state, &bridge, cx);
            }
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
            } else if !confirm_column(&state, window, &focus, cx)
                && !confirm_schedule(&state, window, &focus, cx)
            {
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
    bridge: &Bridge,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    if !cycle_section(state, delta, cx) {
        return;
    }
    materialize_input(state, Some(window), Some(focus), cx);
    load_board_schedules(state, bridge, cx);
}
