//! Draws Board settings in the shared settings shell (§3.8.6): the header, the section rail, a
//! pane of cards and the footer. The General, Backend and Columns panes live here; the
//! Schedules pane is `schedules/view.rs`.
//!
//! Everything a row states was prepared when the draft changed (`ColumnRow`, the schedule rows)
//! or is one field of the draft read as it is; nothing here formats more than a label.

mod column_pane;
mod row;

use gpui::{Bounds, Pixels};

use super::*;
use crate::dialogs::SETTINGS_RAIL_W;
use column_pane::*;
#[cfg(test)]
pub(super) use column_pane::{COLUMN_VERBS, column_breadcrumb};
use row::sentence;
pub(super) use row::{
    BoxValue, Control, PaneBlocks, RowSpec, Verb, Wire, card, settings_row, verb_menu,
};

/// Renders the rail, the pane of cards the rail has selected and the footer (§3.8.6).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _host: &Entity<DialogHost>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    adopt_late_schema(state, cx);
    let (draft, input) = read_host(state, cx, |host, _| {
        (
            host.board_settings.clone(),
            host.board_settings_input.clone(),
        )
    });
    // The live editor belongs to the cursor row only while it is open.
    let input = input.filter(|_| draft.editing);
    let wire = Wire {
        state: state.clone(),
        bridge: bridge.clone(),
        focus: focus.clone(),
        bounds: draft.row_bounds.clone(),
    };

    let pane = match draft.section {
        BoardSection::General => general_pane(&wire, &draft, input.as_ref(), cx),
        BoardSection::Backend => backend_pane(&wire, &draft, input.as_ref(), cx),
        BoardSection::Columns => columns_pane(&wire, &draft, input.as_ref(), cx),
        BoardSection::Schedules => schedules_pane(&wire, &draft, input.as_ref(), cx),
    };

    // The cursor's row is brought into view when the cursor *changes*: `j` past the fold has
    // to land somewhere the user can see. Only when it changes, for the reason the board screen
    // gates the same call: an unconditional reveal re-anchors the pane on every render and takes
    // the wheel away from the user. The row, not its card: a card taller than the pane (a
    // column's *When a card enters*) is "revealed" at its top with its last rows still hidden.
    // The rows record where they are painted as this frame prepaints, so the offset is settled
    // at the next frame's start from bounds that match the frame on screen; a row not painted
    // yet falls back to its card.
    if let Some(row) = with_host(state, cx, |host| {
        (host.board_settings.revealed != Some(host.board_settings.row)).then(|| {
            host.board_settings.revealed = Some(host.board_settings.row);
            host.board_settings.row
        })
    }) {
        let scroll = draft.scroll.clone();
        let bounds = draft.row_bounds.clone();
        let block = pane.row_block.get(row).copied();
        window.on_next_frame(
            move |_, _| match bounds.borrow().get(row).copied().flatten() {
                Some(row_bounds) => {
                    let mut offset = scroll.offset();
                    offset.y += reveal_delta(scroll.bounds(), row_bounds);
                    offset.y = offset.y.max(-scroll.max_offset().y).min(Pixels::ZERO);
                    scroll.set_offset(offset);
                }
                None => {
                    if let Some(block) = block {
                        scroll.scroll_to_item(block);
                    }
                }
            },
        );
    }

    let theme = cx.theme();
    let caption = draft.pane_notice().map(ToOwned::to_owned).or_else(|| {
        // The drill-ins explain themselves with their breadcrumb; the caption is the list's.
        let list = draft.opened_column.is_none() && draft.schedules.form.is_none();
        list.then(|| draft.section.caption().map(ToOwned::to_owned))
            .flatten()
    });
    let pane = div()
        .id("board-settings-pane")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&draft.scroll)
        .p(theme.space.lg)
        .gap(theme.space.md)
        .children(pane.blocks)
        .children(caption.map(|caption| {
            div()
                .flex_none()
                .px(theme.space.xs)
                .child(Text::caption(caption).muted())
        }));

    let body = div()
        .flex()
        .size_full()
        .min_h_0()
        .child(rail(&wire, &draft, cx))
        .child(pane);

    let viewport = window.viewport_size();
    let width = Dialogs::BoardSettings
        .width(cx)
        .min(viewport.width - theme.space.xl * 2.0);
    let height = Dialogs::BoardSettings
        .height()
        .unwrap_or(viewport.height)
        .min(viewport.height - theme.space.xl * 2.0);

    let mut card = Dialog::new("Board settings")
        .dismiss_action(Dialogs::BoardSettings.dismiss_action())
        .icon(Icon::SquareKanban)
        .subtitle(draft.subtitle())
        .width(width)
        .height(height)
        .flush_body(true)
        .body(body)
        .footer_start(footer_start(&draft, cx))
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
    card = match draft.footer_strip() {
        Some(FooterStrip::Error(message)) => card.error(message),
        Some(FooterStrip::Warning(question)) => card.warning(question),
        None => card,
    };

    actions(root(focus), state, bridge, focus)
        .child(card)
        .into_any_element()
}

/// The section rail: one row per section, its glyph and its name (§3.8.6).
fn rail(wire: &Wire, draft: &BoardSettingsState, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(SETTINGS_RAIL_W))
        .h_full()
        .gap(theme.space.xxs)
        .p(theme.space.sm)
        .bg(theme.colors.surface)
        .border_r(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .children(draft.sections().iter().enumerate().map(|(index, section)| {
            let selected = *section == draft.section;
            let tone = if selected {
                Tone::Default
            } else {
                Tone::Secondary
            };
            let wire = wire.clone();
            Row::with_id(("board-settings-section", index))
                .selected(selected)
                .leading(section.icon().el().size(IconSize::Small).tone(tone))
                .column(RowColumn::flex(Text::ui(section.title()).tone(tone)))
                .on_click(move |_, window, cx| {
                    select_section(&wire.state, &wire.bridge, index, &wire.focus, window, cx);
                })
                .harness_target_indexed("board_settings.section", index)
        }))
}

/// *New column* `n` and *Apply preset* `P`: the Columns list's footer. Delete is each row's own.
pub(super) const COLUMNS_FOOTER: [Verb; 2] = [
    Verb {
        label: "New column",
        icon: Icon::Plus,
        action: || Box::new(board_settings_actions::NewColumn),
        harness: Some("board_settings.new"),
        destructive: false,
    },
    Verb {
        label: "Apply preset",
        icon: Icon::Sparkles,
        action: || Box::new(board_settings_actions::ApplyPreset),
        harness: Some("board_settings.preset"),
        destructive: false,
    },
];

/// *New schedule* `n` and *Delete* `d`: the Schedules list's footer.
pub(super) const SCHEDULES_FOOTER: [Verb; 2] = [
    Verb {
        label: "New schedule",
        icon: Icon::Plus,
        action: || Box::new(board_settings_actions::NewColumn),
        harness: Some("board_settings.new"),
        destructive: false,
    },
    Verb {
        label: "Delete",
        icon: Icon::Trash2,
        action: || Box::new(board_settings_actions::DeleteColumn),
        harness: None,
        destructive: true,
    },
];

/// The footer verbs the open pane offers: the list's own, and nothing inside a drill-in, whose
/// way back is its breadcrumb (§3.8.6).
#[must_use]
pub(super) fn footer_verbs(draft: &BoardSettingsState) -> &'static [Verb] {
    if draft.in_schedule_list() && draft.schedules_supported {
        &SCHEDULES_FOOTER
    } else if draft.in_column_list() && draft.pending_delete.is_none() {
        &COLUMNS_FOOTER
    } else {
        &[]
    }
}

/// The footer's left side: the open list's own verbs, then `Unsaved` against the buttons.
///
/// Each verb dispatches the action its key runs, so its chip is that key.
fn footer_start(draft: &BoardSettingsState, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let verbs = footer_verbs(draft).iter().enumerate().map(|(index, verb)| {
        Button::new(("board-settings-footer", index), verb.label)
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .icon(verb.icon)
            .action((verb.action)())
            .harness_target_named(verb.harness)
    });
    div()
        .flex()
        .flex_1()
        .min_w_0()
        // The status is one word so it fits beside the verbs at the dialog's width; on a
        // narrower one it gives way (ellipsised) instead of running into Cancel.
        .overflow_hidden()
        .items_center()
        .gap(theme.space.sm)
        .children(verbs)
        .child(div().flex_1())
        .children(draft.unsaved().then(|| {
            div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .items_center()
                .gap(theme.space.xs)
                .child(StatusDot::small(Tone::Warning))
                .child(Text::ui(UNSAVED).tone(Tone::Warning).ellipsize())
        }))
        .into_any_element()
}

/// The footer's dirty status: one word; the amber strip above says the rest when `esc` asks.
const UNSAVED: &str = "Unsaved";

/// How far the pane's scroll offset moves to show `row` inside `viewport`, the least it can:
/// down until the row's bottom shows, up until its top does, nothing while it is in view. A
/// row taller than the pane shows its top. Added to the offset, which grows negative downward.
#[must_use]
pub(super) fn reveal_delta(viewport: Bounds<Pixels>, row: Bounds<Pixels>) -> Pixels {
    if row.top() < viewport.top() {
        viewport.top() - row.top()
    } else if row.bottom() > viewport.bottom() {
        (viewport.bottom() - row.bottom()).max(viewport.top() - row.top())
    } else {
        Pixels::ZERO
    }
}

/// The General pane: three cards — the board, its cards, its runs (§3.8.6).
fn general_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    cx: &App,
) -> PaneBlocks {
    let repos = repo_choices(wire.state.read(cx));
    let local = draft.backend_kind == BackendRef::LOCAL;
    let mut pane = PaneBlocks::new();
    for (title, rows) in GENERAL_CARDS {
        let mut elements: Vec<AnyElement> = rows
            .iter()
            .map(|row| {
                let index = GENERAL_ROWS
                    .iter()
                    .position(|general| general == row)
                    .unwrap_or(0);
                let spec = general_row(*row, index, index == draft.row, draft, &repos, local);
                settings_row(wire, spec, input)
            })
            .collect();
        // A fact, not a cursor row: nothing moves the board between the two in v1, because a
        // change on a board with live runs would strand them (BOARD §11.10).
        if rows.contains(&SettingRow::MaxLiveRuns) {
            elements.push(
                SettingsRow::new("board-settings-runs-in")
                    .label(RUNS_IN_LABEL)
                    .helper(RUNS_IN_HELPER)
                    .control(Text::ui(draft.runs_in()))
                    .into_any_element(),
            );
        }
        pane.push(
            card(
                ("board-settings-general", pane.blocks.len()),
                Some(title),
                elements,
            ),
            rows.len(),
        );
    }
    pane
}

/// One General row, as the control its setting calls for.
fn general_row(
    row: SettingRow,
    index: usize,
    cursor: bool,
    draft: &BoardSettingsState,
    repos: &[RepoId],
    local: bool,
) -> RowSpec {
    let spec = |control| RowSpec::new(index, cursor, row.label(), control).helper(row.helper());
    match row {
        SettingRow::Name => spec(Control::Box(BoxValue::text(draft.name.clone(), "Fleet")))
            .invalid(draft.name_rule().as_deref().map(sentence)),
        SettingRow::Prefix => {
            let shown = if draft.prefix.trim().is_empty() {
                "FLT"
            } else {
                draft.prefix.trim()
            };
            spec(Control::Box(
                BoxValue::text(draft.prefix.clone(), "FLT")
                    .mono(true)
                    .width(ValueBoxWidth::Short),
            ))
            .helper(Some(format!(
                "Up to {MAX_PREFIX} letters or digits. Cards read {shown}-12."
            )))
            .invalid(draft.prefix_rule().as_deref().map(sentence))
        }
        SettingRow::DefaultRepo => {
            let at = repo_listed(repos, draft.default_repo_id.as_ref())
                .then(|| repo_position(repos, draft.default_repo_id.as_ref()));
            let options: Vec<String> = std::iter::once(NONE.to_owned())
                .chain(repos.iter().map(|repo| repo.as_str().to_owned()))
                .collect();
            // A stored repository this context no longer lists sits on no position: it is shown
            // as the id it is, off the grid, and either arrow steps back onto the list.
            let value = draft
                .default_repo_id
                .as_ref()
                .map_or_else(|| NONE.to_owned(), |repo| repo.as_str().to_owned());
            spec(Control::Choice {
                value,
                details: vec![String::new(); options.len()],
                options,
                at,
                other: false,
                // Repository ids read as a list, not as states: two of them beside `none`
                // would segment, and `none │ acme/api` reads as a switch.
                dropdown: true,
            })
        }
        SettingRow::StartOnWorktree => spec(Control::Switch(draft.start_on_worktree)),
        SettingRow::PushNewCards => spec(Control::Switch(draft.push_new_cards))
            .helper(row.helper().map(|helper| {
                if local {
                    format!("{helper} {NO_BACKEND}")
                } else {
                    helper.to_owned()
                }
            }))
            .disabled(local),
        SettingRow::ConflictPolicy => {
            let options: Vec<String> = POLICIES
                .iter()
                .map(|policy| policy_label(*policy).to_owned())
                .collect();
            spec(Control::Choice {
                value: policy_label(draft.conflict_policy).to_owned(),
                details: vec![String::new(); options.len()],
                options,
                at: POLICIES
                    .iter()
                    .position(|policy| *policy == draft.conflict_policy),
                other: false,
                dropdown: false,
            })
            .disabled(local)
        }
        SettingRow::MaxLiveRuns => spec(Control::Box(
            BoxValue::text(draft.live_run_limit().to_string(), "1")
                .number(Some(format!("of {MAX_LIVE_RUNS_PER_BOARD}").into())),
        ))
        .helper(Some(live_runs_helper(draft.run_location)))
        .invalid(draft.live_runs_rule()),
        _ => spec(Control::Fact(String::new())),
    }
}

/// What a closed choice with nothing chosen reads: the Default repository row with no
/// repository, a backend select with no value.
const NONE: &str = "none";

/// The Backend pane: which backend mirrors the board, then that backend's own rows.
fn backend_pane(
    wire: &Wire,
    draft: &BoardSettingsState,
    input: Option<&Entity<TextInput>>,
    cx: &App,
) -> PaneBlocks {
    let app = wire.state.read(cx);
    let kinds = backend_kinds(app, &draft.backend_kind);
    let options: Vec<String> = kinds.iter().map(|kind| app.backend_label(kind)).collect();
    let backend_label = app.backend_label(&draft.backend_kind);
    let focused = draft.focused();
    let mut pane = PaneBlocks::new();
    let kind = settings_row(
        wire,
        RowSpec::new(
            0,
            focused == SettingRow::Backend,
            SettingRow::Backend.label(),
            Control::Choice {
                value: backend_label.clone(),
                details: vec![String::new(); options.len()],
                at: kinds.iter().position(|kind| *kind == draft.backend_kind),
                options,
                other: false,
                dropdown: false,
            },
        ),
        input,
    );
    pane.push(card("board-settings-backend-kind", None, vec![kind]), 1);
    if !draft.rows.is_empty() {
        let rows = draft
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let cursor = focused == SettingRow::BackendSetting(index);
                settings_row(wire, backend_row(row, index + 1, cursor), input)
            })
            .collect();
        pane.push(
            card("board-settings-backend-rows", None, rows).title(backend_label),
            draft.rows.len(),
        );
    }
    pane
}

/// One backend settings row, as the control its `PropertyKind` names.
fn backend_row(row: &BackendRow, index: usize, cursor: bool) -> RowSpec {
    let label = if row.required {
        format!("{} \u{2217}", row.name)
    } else {
        row.name.clone()
    };
    let control = match row.kind {
        PropertyKind::Bool => Control::Switch(row.flag()),
        PropertyKind::Select => {
            let at = row
                .options
                .iter()
                .position(|option| option.value == row.value);
            let options: Vec<String> = row
                .options
                .iter()
                .map(|option| option.label.clone())
                .collect();
            let value = at.map_or_else(
                || {
                    if row.value.is_empty() {
                        NONE.to_owned()
                    } else {
                        row.value.clone()
                    }
                },
                |at| options[at].clone(),
            );
            Control::Choice {
                value,
                details: vec![String::new(); options.len()],
                options,
                at,
                other: false,
                dropdown: false,
            }
        }
        // An unset optional number is an empty box with its placeholder, never `0`: every
        // backend number row has a non-zero default, and a `0` states a value the daemon is not
        // using.
        PropertyKind::Number => Control::Box(
            BoxValue::text(row.value.trim(), input_placeholder(row))
                .mono(true)
                .number(None),
        ),
        _ => Control::Box(BoxValue::text(row.value.clone(), input_placeholder(row))),
    };
    RowSpec::new(index, cursor, label, control).invalid(row.error().as_deref().map(sentence))
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
            // A delete waiting for a target is answered by the column the cursor is on; then a
            // list row drills in, a text or number row opens (or keeps) its box, and every other
            // row keeps §3.8.6's meaning, which is to save.
            if delete_armed(&state, cx) {
                delete_with_cards(&state, &bridge, cx);
            } else if !confirm_column(&state, window, &focus, cx)
                && !confirm_schedule(&state, window, &focus, cx)
                && !confirm_row(&state, window, &focus, cx)
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
            // `esc` reverts the editor, then leaves the column, then asks once — and only when
            // it has nothing left to leave does it reach the shell, which is the one path that
            // closes an overlay.
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
