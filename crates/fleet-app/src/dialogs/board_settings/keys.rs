//! What the Board settings keys do to the draft (contracts §5.4, `docs/KEYMAP.md`).
//!
//! Split from [`super::draft`] when that file passed the file-size rule: the draft is the
//! *state* — what a board being edited looks like — and this is the *verbs*, every one of them
//! a `with_host` mutation followed by a `notify`. Keeping them apart is what makes the state
//! readable as a data model and the verbs readable as a key table.

use super::*;

use std::cell::Cell;

thread_local! {
    /// The section the dialog was last on, for the life of this app session.
    ///
    /// `,` opens the dialog where the user left it and `C` opens it on Columns (contracts
    /// §5.4). The draft cannot hold that: `persistence::seed` rebuilds it from the board on
    /// every opening, which is exactly what makes a fresh draft read this instead.
    static LAST_SECTION: Cell<BoardSection> = const { Cell::new(BoardSection::General) };
}

/// The section a freshly seeded draft opens on.
pub(super) fn remembered_section() -> BoardSection {
    LAST_SECTION.with(Cell::get)
}

/// Opens the board settings dialog on `section`, and remembers it for this session.
///
/// The caller has already opened the dialog — `C` goes through the same guards `,` does, so a
/// board that is not loaded refuses before anything is remembered.
pub(crate) fn open_on_section(state: &Entity<AppState>, section: BoardSection, cx: &mut App) {
    LAST_SECTION.set(section);
    with_host(state, cx, |host| host.board_settings.section = section);
    notify(state, cx);
}

/// `\u{21e5}` on the rail: move to the next section and remember it.
///
/// The cursor goes back to the first row and any drill-in is left, because a row index means
/// something different in every pane and a section arrived at mid-form is a form the user did
/// not open.
///
/// Returns whether the section changed: an unsaved schedule form holds the cursor until the
/// question it raises has been asked once (§5.4), the same question `esc` asks.
pub(super) fn cycle_section(state: &Entity<AppState>, delta: isize, cx: &mut App) -> bool {
    let moved = with_host(state, cx, |host| {
        if host.board_settings.holds_unsaved_schedule() {
            return false;
        }
        // The rail the user can see: Schedules is not on it without the capability, so it is
        // not a step `⇥` can land on either.
        let sections = host.board_settings.sections();
        let index = step(
            sections
                .iter()
                .position(|section| *section == host.board_settings.section)
                .unwrap_or(0),
            delta,
            sections.len(),
        );
        let section = sections.get(index).copied().unwrap_or_default();
        host.board_settings.section = section;
        host.board_settings.row = 0;
        host.board_settings.opened_column = None;
        host.board_settings.editing = false;
        host.board_settings.discard_armed = false;
        host.board_settings.schedules.leave();
        host.board_settings.prepare();
        LAST_SECTION.set(section);
        true
    });
    notify(state, cx);
    moved
}

/// `j` / `k`: move the cursor while no text row owns the keyboard.
///
/// Moving is also how a column editor is left: the editor belongs to the row it was opened on,
/// and a cursor one row down with the old editor still mounted would type into the wrong field.
pub(super) fn move_row(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        let len = host.board_settings.rows().len();
        host.board_settings.row = step(host.board_settings.row, delta, len);
        host.board_settings.editing = false;
        host.board_settings.discard_armed = false;
        host.board_settings.notice = None;
        // The notice was the armed delete's question; with it gone the arm goes too, so a
        // later `d` asks again instead of deleting with nothing on screen.
        host.board_settings.schedules.pending_delete = None;
    });
    cx.stop_propagation();
}

/// `h` / `l`: cycle a closed choice while no text row owns the keyboard.
pub(super) fn cycle(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    let app = state.read(cx);
    let repos = repo_choices(app);
    let selected = read_host(state, cx, |host, _| {
        host.board_settings.backend_kind.clone()
    });
    let kinds = backend_kinds(state.read(cx), &selected);
    let next_kind = kinds
        .iter()
        .position(|kind| *kind == selected)
        .map(|index| kinds[step(index, delta, kinds.len())].clone())
        .unwrap_or(selected);
    let schema = schema_for(state.read(cx), &next_kind);
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.saving {
            return;
        }
        match draft.focused() {
            SettingRow::DefaultRepo => {
                // A value the list does not carry has no position to step from: wrapping out
                // of an invented `0` sent `h` to the *last* repository. From off the grid the
                // step lands on the neighbour it names — `l` on the first repository, `h` on
                // "none" — and never anywhere the arrows did not point.
                if !repo_listed(&repos, draft.default_repo_id.as_ref()) {
                    draft.default_repo_id = (delta > 0).then(|| repos.first().cloned()).flatten();
                } else {
                    let index = repo_position(&repos, draft.default_repo_id.as_ref());
                    let next = step(index, delta, repos.len() + 1);
                    draft.default_repo_id =
                        next.checked_sub(1).and_then(|at| repos.get(at).cloned());
                }
            }
            SettingRow::ConflictPolicy => {
                if draft.backend_kind == BackendRef::LOCAL {
                    return;
                }
                let index = POLICIES
                    .iter()
                    .position(|policy| *policy == draft.conflict_policy)
                    .unwrap_or(0);
                draft.conflict_policy = POLICIES[step(index, delta, POLICIES.len())];
                return;
            }
            // `h` is off and `l` is on, never a flip: every other cycler row on this dialog
            // steps by `delta`, and a row that flipped on both made `h l` land somewhere other
            // than where it started.
            SettingRow::StartOnWorktree => draft.start_on_worktree = delta > 0,
            SettingRow::PushNewCards => {
                if draft.backend_kind == BackendRef::LOCAL {
                    return;
                }
                draft.push_new_cards = delta > 0;
            }
            SettingRow::Backend => {
                draft.select_backend(&next_kind, &schema);
                return;
            }
            SettingRow::BackendSetting(index) => {
                cycle_backend_row(draft, index, delta);
                return;
            }
            // Clamped rather than wrapped: `h` on one run must not land on eight, which is the
            // difference between "one run at a time" and eight agents in one checkout.
            SettingRow::MaxLiveRuns => draft.step_live_runs(delta),
            SettingRow::ColumnField(field) => {
                let Some(index) = draft.opened_column else {
                    return;
                };
                let locked = draft.automation_locked;
                if cycle_field(&mut draft.columns, index, field, delta, locked) {
                    draft.error = None;
                    draft.notice = None;
                    draft.discard_armed = false;
                    draft.prepare();
                }
                return;
            }
            SettingRow::ScheduleField(field) => {
                if draft.cycle_schedule(field, delta) {
                    draft.error = None;
                    draft.notice = None;
                }
                return;
            }
            // A column list row cycles nothing: `J` and `K` are what move one, and they say so
            // in the hint row. A schedule row is acted on by `space`, `r` and `d`.
            SettingRow::Column(_)
            | SettingRow::Schedule(_)
            | SettingRow::NoRow
            | SettingRow::Name
            | SettingRow::Prefix => {}
        }
        draft.error = None;
    });
    notify(state, cx);
    cx.stop_propagation();
}

/// `h` / `l` inside a backend row: step a number, cycle a select, flip a flag.
pub(super) fn cycle_backend_row(draft: &mut BoardSettingsState, index: usize, delta: isize) {
    let Some(row) = draft.rows.get_mut(index) else {
        return;
    };
    match row.kind {
        // `h` is off and `l` is on, for the reason the fixed toggle rows are: a flip on both
        // makes `h l` land on the opposite of where it started. `space` is the toggle.
        PropertyKind::Bool => {
            row.value = (delta > 0).to_string();
            row.present = true;
        }
        PropertyKind::Number => {
            // Stepping *down* from an unset row would write `0` — a value the daemon is not
            // using, that `backend_element` refuses to draw, that `ctrl-u` is the only way out
            // of, and that on two of Jira's three number rows either fails the save or turns
            // every pull into a full one. An empty row has nothing below it.
            if row.value.trim().is_empty() && delta < 0 {
                return;
            }
            let current: i64 = row.value.trim().parse().unwrap_or(0);
            row.value = current
                .saturating_add(i64::try_from(delta).unwrap_or(0))
                .max(0)
                .to_string();
        }
        PropertyKind::Select if !row.options.is_empty() => {
            let at = row
                .options
                .iter()
                .position(|option| option.value == row.value)
                .unwrap_or(0);
            row.value = row.options[step(at, delta, row.options.len())]
                .value
                .clone();
        }
        _ => return,
    }
    draft.error = None;
}

/// `space`: toggle the focused flag row; anywhere else it is a space.
pub(super) fn toggle(state: &Entity<AppState>, cx: &mut App) {
    let changed = with_host(state, cx, |host| {
        if host.board_settings.saving {
            return false;
        }
        match host.board_settings.focused() {
            SettingRow::StartOnWorktree => {
                host.board_settings.start_on_worktree = !host.board_settings.start_on_worktree;
                true
            }
            SettingRow::PushNewCards => {
                if host.board_settings.backend_kind == BackendRef::LOCAL {
                    return false;
                }
                host.board_settings.push_new_cards = !host.board_settings.push_new_cards;
                true
            }
            SettingRow::BackendSetting(index) => {
                let Some(row) = host.board_settings.rows.get_mut(index) else {
                    return false;
                };
                if !row.is_flag() {
                    return false;
                }
                row.value = (!row.flag()).to_string();
                row.present = true;
                true
            }
            SettingRow::ScheduleField(ScheduleField::Enabled) => {
                host.board_settings.toggle_schedule_flag()
            }
            _ => false,
        }
    });
    if changed {
        notify(state, cx);
    }
    cx.stop_propagation();
}

/// `\u{23ce}` in the Columns pane: drill into a column, or open and commit a row's editor.
///
/// Returns whether the key was the Columns pane's; General and Backend keep §3.8.6's `\u{23ce}`,
/// which saves.
pub(super) fn confirm_column(
    state: &Entity<AppState>,
    window: &mut Window,
    focus: &FocusHandle,
    cx: &mut App,
) -> bool {
    let handled = with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.section != BoardSection::Columns || draft.saving {
            return false;
        }
        draft.discard_armed = false;
        match draft.focused() {
            SettingRow::Column(index) => {
                // A delete waiting for a target is what `\u{23ce}` answers first: the row the
                // cursor is on *is* the answer, and drilling into it instead would lose it.
                if draft.pending_delete.is_some() {
                    return true;
                }
                draft.opened_column = Some(index);
                draft.row = 0;
                draft.editing = false;
                draft.notice = None;
                draft.prepare();
                true
            }
            SettingRow::ColumnField(field) => {
                let locked = draft.automation_locked && field.is_automation();
                draft.editing = !draft.editing && field.is_text() && !locked;
                true
            }
            _ => false,
        }
    });
    if handled {
        materialize_input(state, Some(window), Some(focus), cx);
        notify(state, cx);
    }
    handled
}

/// `esc`: leave the editor, then the column, then the dialog — asking once on the way out.
///
/// Returns whether the dialog keeps the key. Only the last step lets it through to the shell's
/// own `dialog::Cancel`, which is the single path that closes an overlay.
pub(super) fn cancel(
    state: &Entity<AppState>,
    window: &mut Window,
    focus: &FocusHandle,
    cx: &mut App,
) -> bool {
    let step = with_host(state, cx, |host| host.board_settings.escape());
    let kept = step != EscapeStep::Close;
    if matches!(
        step,
        EscapeStep::Editor | EscapeStep::Column | EscapeStep::Schedule
    ) {
        materialize_input(state, Some(window), Some(focus), cx);
    }
    if kept {
        notify(state, cx);
    }
    kept
}

/// `n`: append a column and put the cursor on it, ready to be renamed.
pub(super) fn add_column(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if !draft.in_column_list() {
            return;
        }
        let Some(column) = new_column(&draft.columns) else {
            draft.notice = Some("every generated column name is taken".to_owned());
            return;
        };
        draft.columns.push(column);
        draft.prepare();
        draft.row = draft.columns.len().saturating_sub(1);
        draft.notice = None;
        draft.error = None;
        draft.discard_armed = false;
    });
    notify(state, cx);
}

/// `J` / `K`: move the focused column past its neighbour, cursor following it.
pub(super) fn move_column(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if !draft.in_column_list() {
            return;
        }
        let SettingRow::Column(index) = draft.focused() else {
            return;
        };
        if let Some(moved) = reorder(&mut draft.columns, index, delta) {
            draft.row = moved;
            draft.notice = None;
            draft.error = None;
            draft.discard_armed = false;
            draft.prepare();
        }
    });
    notify(state, cx);
}

/// `P`: add the workflow preset's missing columns, by id, in preset order.
pub(super) fn preset(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if !draft.in_column_list() {
            return;
        }
        let Some(template) = draft.board.clone() else {
            return;
        };
        draft.notice = Some(if apply_preset(&template, &mut draft.columns) {
            draft.error = None;
            draft.discard_armed = false;
            "the workflow preset added its missing columns".to_owned()
        } else {
            "this board already has every preset column".to_owned()
        });
        draft.prepare();
    });
    notify(state, cx);
}

/// `d`: delete the focused column, or ask where its cards go first.
///
/// A column nothing sits in leaves the draft at once. One holding cards arms instead: the
/// notice names the count, the next `\u{23ce}` on another column is the target, and only then
/// do the moves go out (§5.4).
pub(super) fn arm_delete(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if !draft.in_column_list() || draft.saving {
            return;
        }
        let SettingRow::Column(index) = draft.focused() else {
            return;
        };
        if draft.columns.len() <= 1 {
            draft.notice = Some("a board needs at least one column".to_owned());
            return;
        }
        let cards = draft.cards_in(index).len();
        if cards == 0 {
            draft.columns.remove(index);
            draft.pending_delete = None;
            draft.notice = None;
            draft.error = None;
            draft.discard_armed = false;
            draft.prepare();
            return;
        }
        let name = draft.columns[index].status.name.clone();
        draft.pending_delete = Some(index);
        draft.notice = Some(format!(
            "{name} holds {cards} card{} \u{2014} choose the column they move to",
            if cards == 1 { "" } else { "s" }
        ));
    });
    notify(state, cx);
}

/// What the one row-scoped editor is built from.
struct InputSpec {
    /// The row it belongs to, so a late `Changed` event for a stale row is dropped.
    row: SettingRow,
    /// The text it opens with.
    text: String,
    /// Its label.
    label: String,
    /// Its placeholder.
    placeholder: String,
    /// Whether it is drawn in the mono face.
    mono: bool,
    /// Whether it takes digits only.
    digits: bool,
    /// Whether it takes more than one line.
    multiline: bool,
}

/// Rebuild the one row-scoped entity after focus moves, dropping it on non-text rows.
pub(super) fn materialize_input(
    state: &Entity<AppState>,
    window: Option<&mut Window>,
    dialog_focus: Option<&FocusHandle>,
    cx: &mut App,
) {
    let spec = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        let row = draft.focused();
        let text = draft.focused_text()?;
        let (label, placeholder, mono, digits, multiline) = match row {
            SettingRow::Name => (
                row.label().to_owned(),
                "Fleet".to_owned(),
                false,
                false,
                false,
            ),
            SettingRow::Prefix => (row.label().to_owned(), "FLT".to_owned(), true, false, false),
            SettingRow::BackendSetting(index) => {
                let backend = draft.rows.get(index)?;
                let label = if backend.required {
                    format!("{} \u{2217}", backend.name)
                } else {
                    backend.name.clone()
                };
                (
                    label,
                    input_placeholder(backend).to_owned(),
                    backend.kind == PropertyKind::Number,
                    backend.kind == PropertyKind::Number,
                    false,
                )
            }
            SettingRow::ColumnField(field) => (
                field.label().to_owned(),
                field.placeholder().to_owned(),
                matches!(field, ColumnField::OnEnter | ColumnField::Env),
                false,
                field.is_multiline(),
            ),
            SettingRow::ScheduleField(field) => (
                field.label().to_owned(),
                field.placeholder().to_owned(),
                field.is_mono(),
                field.is_number(),
                field.is_multiline(),
            ),
            _ => return None,
        };
        Some(InputSpec {
            row,
            text,
            label,
            placeholder,
            mono,
            digits,
            multiline,
        })
    });
    let Some(spec) = spec else {
        with_host(state, cx, |host| {
            host.board_settings_input = None;
            host.board_settings_input_subscription = None;
        });
        if let (Some(window), Some(focus)) = (window, dialog_focus) {
            window.focus(focus, cx);
        }
        notify(state, cx);
        return;
    };
    let row = spec.row;
    let input = cx.new(|cx| {
        let mut input = TextInput::new(
            if spec.multiline {
                // The same eight rows the card's description opens in
                // (`card_detail/draft.rs`): an instruction block is prose, and a four-row box
                // hides the end of every one of them.
                InputMode::Multiline {
                    min_rows: MULTILINE_ROWS,
                    max_rows: MULTILINE_ROWS,
                }
            } else {
                InputMode::SingleLine
            },
            cx,
        );
        input.set_text(spec.text, cx);
        input.set_label(Some(spec.label.into()), cx);
        input.set_placeholder(spec.placeholder, cx);
        input.set_mono(spec.mono, cx);
        if spec.digits {
            input.set_filter(Some(|character| character.is_ascii_digit()), cx);
        }
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    let weak_host = host.downgrade();
    let subscription = cx.subscribe(&input, move |input, event, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(host) = weak_host.upgrade() else {
            return;
        };
        let raw = input.read(cx).text().to_owned();
        let normalized = if row == SettingRow::Prefix {
            raw.to_uppercase()
        } else {
            raw
        };
        // Mirror prefixes in their stored uppercase form, but leave the live editor untouched so
        // selection and undo history remain user edits. Moving away and back materializes that
        // normalized draft value.
        host.update(cx, |host, cx| {
            if host.board_settings.focused() == row {
                host.board_settings.set_focused_text(&normalized);
                cx.notify();
            }
        });
    });
    host.update(cx, |host, _| {
        host.board_settings_input = Some(input.clone());
        host.board_settings_input_subscription = Some(subscription);
    });
    if let Some(window) = window {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
}
