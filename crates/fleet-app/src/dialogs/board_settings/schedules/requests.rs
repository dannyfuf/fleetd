//! The Schedules section's entry points and requests: the loads that fill the mirror, and the
//! one request each list key and the form's `^s` send.

use super::*;

/// Opens the Board settings dialog on its Schedules section.
///
/// `starter` opens the new-schedule form pre-filled as `n` would (on a Reviews board, the
/// GitHub review starter): the Review board's empty state `⏎` asks for exactly that. The shell
/// calls this right after opening the dialog, before the host has seeded it, so the request
/// waits for the seed; on a dialog already showing the board it lands at once.
pub(crate) fn open_schedules_section(state: &Entity<AppState>, starter: bool, cx: &mut App) {
    let seeded = read_host(state, cx, |host, _| {
        host.open == Some(Dialogs::BoardSettings) && host.board_settings.board_id.is_some()
    });
    if !seeded {
        PENDING_OPEN.set(Some(starter));
        if state.read(cx).board_takes_schedules() {
            open_on_section(state, BoardSection::Schedules, cx);
        }
        return;
    }
    with_host(state, cx, |host| {
        host.board_settings.open_section(starter, Local::now());
    });
    if state.read(cx).board_takes_schedules() {
        open_on_section(state, BoardSection::Schedules, cx);
    }
    materialize_input(state, None, None, cx);
}

/// The open dialog's section and the name its schedule form holds, if one is open — what a
/// test outside the dialog asserts after an entry point asked for the section.
#[cfg(test)]
pub(crate) fn schedules_form_probe(
    state: &Entity<AppState>,
    cx: &mut App,
) -> (BoardSection, Option<String>) {
    read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        let name = draft
            .schedules
            .form
            .as_ref()
            .map(|form| form.fields.name.clone());
        (draft.section, name)
    })
}

/// Loads the open board's schedules when the mirror has none, or only stale or failed ones.
///
/// Called when the dialog opens and whenever the rail lands on Schedules; a load already in
/// flight is not repeated.
pub(crate) fn load_board_schedules(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some(board) = read_host(state, cx, |host, _| host.board_settings.board_id.clone()) else {
        return;
    };
    // A worktree board stored on another host is one this daemon refuses to schedule.
    if !state.read(cx).board_takes_schedules() {
        return;
    }
    let wanted = state
        .read(cx)
        .schedules
        .entry(&board)
        .is_none_or(|entry| entry.stale || entry.error.is_some());
    if wanted {
        load_schedules(board, state, bridge, cx);
    }
}

/// Sends `ListSchedules { board_id }` and adopts the answer into the mirror.
///
/// Sends nothing on a daemon without `schedules`, or while a load for the board is in flight.
pub(crate) fn load_schedules(
    board: BoardId,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    if !state.read(cx).supports_schedules() {
        return;
    }
    let started = state.update(cx, |app, cx| {
        let started = app.schedules.begin_load(&board);
        if started {
            cx.notify();
        }
        started
    });
    if !started {
        return;
    }
    let reply = bridge.request(RequestBody::ListSchedules {
        board_id: Some(board.clone()),
    });
    sync_open_list(state, cx);
    let bridge = bridge.clone();
    complete_request(state, cx, async move |state, cx| {
        let answer = match reply.recv().await {
            Ok(Ok(ResponseBody::Schedules(schedules))) => Ok(schedules),
            Ok(Err(error)) => Err(error.message),
            Ok(Ok(_)) => Err("Unexpected schedules response".to_owned()),
            Err(_) => Err("Daemon disconnected before replying".to_owned()),
        };
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            adopt_load(&state, &bridge, &board, answer, cx);
        });
    });
}

/// Puts a `ListSchedules` answer into the mirror and the open dialog's list.
///
/// A `SchedulesChanged` that landed while the load was in flight left the entry stale, and
/// the batch that carried it skipped a board already loading; the answer may predate the
/// change, so the board is read once more.
pub(in crate::dialogs::board_settings) fn adopt_load(
    state: &Entity<AppState>,
    bridge: &Bridge,
    board: &BoardId,
    answer: Result<Vec<Schedule>, String>,
    cx: &mut App,
) {
    let stale = state.update(cx, |app, cx| {
        match answer {
            Ok(schedules) => app.schedules.apply(board, schedules),
            Err(message) => app.schedules.load_failed(board, message),
        }
        cx.notify();
        app.schedules.entry(board).is_some_and(|entry| entry.stale)
    });
    sync_open_list(state, cx);
    notify(state, cx);
    if stale {
        load_schedules(board.clone(), state, bridge, cx);
    }
}

/// Re-reads every board whose schedules a `SchedulesChanged` marked stale.
///
/// Called once per applied event batch, so a burst of events costs one request per board.
pub(crate) fn refresh_stale_schedules(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let stale = state.read(cx).schedules.stale_boards();
    for board in stale {
        load_schedules(board, state, bridge, cx);
    }
}

/// Re-prepares the dialog's list from the mirror, when the mirror moved.
///
/// The host's observation of `AppState` calls this whenever the dialog is open, so a
/// `SchedulesChanged`, a load answering or a reconnect clearing the mirror lands in the list
/// without the render preparing anything (`docs/APP-CONTRACTS.md`); a notification that moved
/// nothing costs one revision compare.
pub(crate) fn sync_open_list(state: &Entity<AppState>, cx: &mut App) {
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, cx| {
        let mirror = &state.read(cx).schedules;
        if host.board_settings.sync_schedules(mirror, Local::now()) {
            cx.notify();
        }
    });
}

/// What a schedule request was for, which decides what its answer does to the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::dialogs::board_settings) enum Sent {
    /// `^s` on the form.
    Saved,
    /// `space` on a list row.
    Toggled,
    /// The second `d` on a list row.
    Deleted(ScheduleId),
    /// `r`.
    Ran,
}

/// `n` in the Schedules list: the new-schedule form.
pub(in crate::dialogs::board_settings) fn new_schedule(
    state: &Entity<AppState>,
    cx: &mut App,
) -> bool {
    let handled = with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if !draft.in_schedule_list() || draft.schedules.busy {
            return draft.section == BoardSection::Schedules;
        }
        draft.open_new_schedule(Local::now());
        true
    });
    if handled {
        notify(state, cx);
    }
    handled
}

/// `⏎` in the Schedules section: drill into a row (or open a new form on an empty list), or
/// open and commit a form row's editor.
///
/// Returns whether the key was this section's.
pub(in crate::dialogs::board_settings) fn confirm_schedule(
    state: &Entity<AppState>,
    window: &mut Window,
    focus: &FocusHandle,
    cx: &mut App,
) -> bool {
    let handled = with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.section != BoardSection::Schedules {
            return false;
        }
        if draft.schedules.busy {
            return true;
        }
        draft.discard_armed = false;
        match draft.focused() {
            SettingRow::Schedule(index) => {
                if let Some(schedule) = draft.schedules.schedule(index).cloned() {
                    draft.open_schedule(&schedule, Local::now());
                }
            }
            SettingRow::ScheduleField(field) => {
                draft.editing = !draft.editing && field.is_text();
            }
            // An empty list: `⏎` is the empty state's "add one".
            _ if draft.schedules.form.is_none() => draft.open_new_schedule(Local::now()),
            _ => {}
        }
        true
    });
    if handled {
        materialize_input(state, Some(window), Some(focus), cx);
        notify(state, cx);
    }
    handled
}

/// `space` on a Schedules list row: `UpdateSchedule { enabled: !enabled }`. Returns whether
/// the key was the list's.
pub(in crate::dialogs::board_settings) fn toggle_listed(
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) -> bool {
    let target = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        if !draft.in_schedule_list() {
            return None;
        }
        Some(match draft.focused() {
            SettingRow::Schedule(index) => draft.schedules.schedule(index).cloned(),
            _ => None,
        })
    });
    let Some(target) = target else {
        return false;
    };
    if let Some(schedule) = target {
        disarm_delete(state, cx);
        send(
            state,
            bridge,
            RequestBody::UpdateSchedule {
                id: schedule.id.clone(),
                patch: SchedulePatch {
                    enabled: Some(!schedule.enabled),
                    ..SchedulePatch::default()
                },
            },
            Sent::Toggled,
            cx,
        );
    }
    true
}

/// `r` in the Schedules section — `RunScheduleNow` for the focused row, or for the schedule
/// whose form is open. Refusals go to the dialog's red footer line.
pub(crate) fn run_focused_schedule(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let id = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        if draft.section != BoardSection::Schedules {
            return None;
        }
        if let Some(form) = draft.schedules.form.as_ref() {
            return form.id.clone();
        }
        match draft.focused() {
            SettingRow::Schedule(index) => draft
                .schedules
                .schedule(index)
                .map(|schedule| schedule.id.clone()),
            _ => None,
        }
    });
    if let Some(id) = id {
        disarm_delete(state, cx);
        send(
            state,
            bridge,
            RequestBody::RunScheduleNow { id },
            Sent::Ran,
            cx,
        );
    }
}

/// Takes back an armed delete before another request replaces its question.
///
/// The question is the notice under the list, and every answer rewrites that notice; an arm
/// left behind would let the next `d` delete with nothing on screen.
fn disarm_delete(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.schedules.pending_delete.take().is_some() {
            draft.notice = None;
        }
    });
}

/// `d` on a Schedules list row: the first press asks, the second deletes.
///
/// A schedule is not a draft row that `esc` can bring back: the delete is one request and its
/// run logs go with it, so the key asks once in the amber footer strip, the way `esc` asks
/// about an unsaved draft, and `esc` takes the question back. Returns whether the key was this
/// section's.
pub(in crate::dialogs::board_settings) fn delete_schedule(
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) -> bool {
    let target = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        if draft.section != BoardSection::Schedules {
            return None;
        }
        Some(match draft.focused() {
            SettingRow::Schedule(index) if draft.schedules.form.is_none() => draft
                .schedules
                .schedule(index)
                .map(|schedule| (schedule.id.clone(), schedule.name.clone())),
            _ => None,
        })
    });
    let Some(target) = target else {
        return false;
    };
    let Some((id, name)) = target else {
        return true;
    };
    let confirmed = with_host(state, cx, |host| {
        let pane = &mut host.board_settings.schedules;
        if pane.busy {
            return false;
        }
        if pane.pending_delete.as_ref() == Some(&id) {
            return true;
        }
        pane.pending_delete = Some(id.clone());
        host.board_settings.notice = Some(format!(
            "delete {name} and its run logs? d again to delete, esc to keep it"
        ));
        false
    });
    if confirmed {
        send(
            state,
            bridge,
            RequestBody::DeleteSchedule { id: id.clone() },
            Sent::Deleted(id),
            cx,
        );
    } else {
        notify(state, cx);
    }
    true
}

/// `^s` with a schedule's form open: `CreateSchedule` or `UpdateSchedule`. Returns whether the
/// key was the form's; anywhere else `^s` saves the board.
pub(in crate::dialogs::board_settings) fn save_schedule(
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) -> bool {
    let request = read_host(state, cx, |host, _| {
        let draft = &host.board_settings;
        if draft.section != BoardSection::Schedules {
            return None;
        }
        let form = draft.schedules.form.as_ref()?;
        Some(match &form.id {
            Some(id) => Some(RequestBody::UpdateSchedule {
                id: id.clone(),
                patch: form.patch(),
            }),
            None => draft
                .board_id
                .clone()
                .map(|board| RequestBody::CreateSchedule {
                    draft: form.draft(board),
                }),
        })
    });
    let Some(request) = request else {
        return false;
    };
    if let Some(request) = request {
        send(state, bridge, request, Sent::Saved, cx);
    }
    true
}

/// Sends one schedule request and routes its answer through [`finish`].
fn send(state: &Entity<AppState>, bridge: &Bridge, body: RequestBody, sent: Sent, cx: &mut App) {
    let Some((board, generation)) = with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.schedules.busy {
            return None;
        }
        let board = draft.board_id.clone()?;
        draft.schedules.busy = true;
        draft.error = None;
        Some((board, draft.generation))
    }) else {
        return;
    };
    let reply = bridge.request(body);
    notify(state, cx);
    complete_request(state, cx, async move |state, cx| {
        let answer = match reply.recv().await {
            Ok(Ok(ResponseBody::Schedule(schedule))) => Ok(Some(schedule)),
            Ok(Ok(ResponseBody::Ack)) => Ok(None),
            // The daemon's own words, verbatim: `invalid cadence: every must be between 5 and
            // 1440 minutes` is a sentence only the rules that refused it can write.
            Ok(Err(error)) => Err(error.message),
            Ok(Ok(_)) => Err("Unexpected schedule response".to_owned()),
            Err(_) => Err("Daemon disconnected before replying".to_owned()),
        };
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            finish(&state, &board, generation, &sent, answer, cx);
        });
    });
}

/// Applies a schedule request's answer: to the mirror always, and to the dialog when it is
/// still the opening that sent it.
///
/// A refusal keeps the form open with the daemon's sentence on the red footer line.
pub(in crate::dialogs::board_settings) fn finish(
    state: &Entity<AppState>,
    board: &BoardId,
    generation: u64,
    sent: &Sent,
    answer: Result<Option<Schedule>, String>,
    cx: &mut App,
) {
    if let Ok(answer) = &answer {
        state.update(cx, |app, cx| {
            let mut schedules = app.schedules.for_board(board).to_vec();
            match (sent, answer) {
                (Sent::Deleted(id), _) => schedules.retain(|schedule| schedule.id != *id),
                (_, Some(schedule)) => {
                    match schedules.iter_mut().find(|held| held.id == schedule.id) {
                        Some(held) => *held = schedule.clone(),
                        None => schedules.push(schedule.clone()),
                    }
                }
                (_, None) => return,
            }
            app.schedules.apply(board, schedules);
            cx.notify();
        });
    }
    let open =
        state.read(cx).overlay == Some(crate::state::Overlay::Dialog(Dialogs::BoardSettings));
    if !open {
        return;
    }
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, cx| {
        let draft = &mut host.board_settings;
        if draft.generation != generation || draft.board_id.as_ref() != Some(board) {
            return;
        }
        draft.schedules.busy = false;
        draft.sync_schedules(&state.read(cx).schedules, Local::now());
        // The cursor and the notice belong to the Schedules section; an answer that lands
        // after the user left it (leaving already closed the form) changes the mirror and
        // nothing on the section now shown. A refusal still reaches the dialog-wide red line,
        // since nothing else would say the request failed.
        if draft.section != BoardSection::Schedules {
            if let Err(message) = answer {
                draft.error = Some(message);
            }
            return;
        }
        let name = answer
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|schedule| schedule.name.clone());
        let skipped = answer
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(skipped_run_notice);
        match answer {
            Err(message) => draft.error = Some(message),
            Ok(_) => {
                draft.error = None;
                match sent {
                    Sent::Saved => {
                        let id = draft
                            .schedules
                            .form
                            .as_ref()
                            .and_then(|form| form.id.clone());
                        draft.schedules.leave();
                        draft.editing = false;
                        let id = id.or_else(|| {
                            draft
                                .schedules
                                .list
                                .last()
                                .map(|row| row.schedule.id.clone())
                        });
                        draft.row = id
                            .and_then(|id| {
                                draft
                                    .schedules
                                    .list
                                    .iter()
                                    .position(|row| row.schedule.id == id)
                            })
                            .unwrap_or(0);
                        draft.notice = name.map(|name| format!("saved {name}"));
                    }
                    Sent::Toggled => draft.notice = None,
                    Sent::Deleted(_) => {
                        draft.schedules.pending_delete = None;
                        draft.notice = Some("the schedule was deleted".to_owned());
                        draft.row = draft.row.min(draft.schedules.list.len().saturating_sub(1));
                    }
                    Sent::Ran => {
                        draft.notice =
                            skipped.or_else(|| name.map(|name| format!("running {name} now")));
                    }
                }
            }
        }
    });
    materialize_input(state, None, None, cx);
    notify(state, cx);
}
