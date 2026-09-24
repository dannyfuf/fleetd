//! The board surfaces' schedule keys (BOARD §11.8): `R` runs every enabled schedule of the
//! shown board now. `T` and the header strip open Board settings on its Schedules section,
//! which the shell does through the dialog's own entry point.

use super::*;

/// Said when `R` finds nothing to run on the shown board.
pub(crate) const NO_ENABLED_SCHEDULE: &str = "No enabled schedule on this board";
/// Said when `R` is pressed before the shown board's schedules have been listed.
pub(crate) const SCHEDULES_LOADING: &str = "schedules are still loading";

/// `R` — one `RunScheduleNow` per enabled schedule of the shown board, from the schedules
/// mirror. A daemon without `schedules` gets its sentence and no request; a board with no
/// enabled schedule says so. Each fire is the daemon's to report: its `SchedulesChanged` marks
/// the mirror stale and the strip reads `running` once the reload lands.
pub(crate) fn run_schedules(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if refuses(state, cx) {
        return;
    }
    let app = state.read(cx);
    if let Some(refusal) = app.board_schedules_refusal() {
        state.update(cx, |app, cx| {
            app.toast_short(refusal, Icon::CloudOff, Instant::now());
            cx.notify();
        });
        return;
    }
    let Some(board) = board_id(app) else {
        state.update(cx, |app, cx| {
            app.toast_short("No board loaded yet", Icon::Boxes, Instant::now());
            cx.notify();
        });
        return;
    };
    // The mirror is what `R` reads, so a board whose schedules are not loaded yet, or whose
    // load failed, says that instead of claiming it has none.
    let unloaded = match app.schedules.entry(&board) {
        None => Some(SCHEDULES_LOADING.to_owned()),
        Some(entry) if entry.loading && entry.schedules.is_empty() => {
            Some(SCHEDULES_LOADING.to_owned())
        }
        Some(entry) => entry.error.clone(),
    };
    if let Some(sentence) = unloaded {
        state.update(cx, |app, cx| {
            app.toast_short(sentence, Icon::Boxes, Instant::now());
            cx.notify();
        });
        return;
    }
    let ids = board_screen::runnable_schedules(app.schedules.for_board(&board));
    if ids.is_empty() {
        state.update(cx, |app, cx| {
            app.toast_short(NO_ENABLED_SCHEDULE, Icon::Boxes, Instant::now());
            cx.notify();
        });
        return;
    }
    for id in ids {
        let reply = bridge.request(RequestBody::RunScheduleNow { id });
        let state = state.clone();
        cx.spawn(async move |cx| {
            let Ok(answer) = reply.recv().await else {
                return;
            };
            cx.update(|cx| run_answered(&state, answer, cx));
        })
        .detach();
    }
}

/// One `RunScheduleNow` answer from `R`: a refusal is the daemon's sentence, and a fire the
/// daemon recorded as `Skipped` (the previous run was still going) says
/// `skipped {name}: {summary}`, as the CLI does. A started run says nothing here: its
/// `SchedulesChanged` turns the header strip to `running`.
fn run_answered(
    state: &Entity<AppState>,
    answer: Result<ResponseBody, fleet_proto::error::ProtoError>,
    cx: &mut App,
) {
    match answer {
        Err(error) => fail(state, error.message, cx),
        Ok(ResponseBody::Schedule(schedule)) => {
            if let Some(sentence) = crate::dialogs::skipped_run_notice(&schedule) {
                state.update(cx, |app, cx| {
                    app.toast_short(sentence, Icon::Boxes, Instant::now());
                    cx.notify();
                });
            }
        }
        Ok(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::RecordedRequests;
    use fleet_core::{
        board::{BoardView, new_board},
        schedule::{Cadence, Schedule, ScheduleAgent},
    };
    use gpui::TestAppContext;

    fn schedule(id: &str, board: &BoardId, enabled: bool) -> Schedule {
        Schedule {
            id: id.parse().expect("schedule id"),
            board_id: board.clone(),
            name: id.to_owned(),
            prompt: "look".to_owned(),
            cadence: Cadence::Every { minutes: 15 },
            agent: ScheduleAgent::default(),
            enabled,
            timeout_minutes: 20,
            created_at: "2026-09-24T10:00:00Z".to_owned(),
            updated_at: "2026-09-24T10:00:00Z".to_owned(),
            runs: Vec::new(),
            next_run_at: None,
        }
    }

    fn seeded(cx: &mut TestAppContext, capable: bool) -> (Entity<AppState>, BoardId) {
        let state = cx.new(|_| AppState::new("/tmp/fleet-run-schedules", Instant::now()));
        let board = cx.update(|cx| {
            state.update(cx, |app, _| {
                let context = fleet_core::model::Context {
                    id: "work".parse().expect("context id"),
                    name: "Fleet".into(),
                    owners: vec![],
                    created_at: "2026-09-24T10:00:00Z".into(),
                };
                let board = new_board(&context, "2026-09-24T10:00:00Z");
                let id = board.id.clone();
                app.board.scope = Some(BoardScope::Context(context.id));
                app.board.view = Some(BoardView {
                    board,
                    cards: Vec::new(),
                    live_runs: Vec::new(),
                });
                if capable {
                    app.daemon_capabilities
                        .insert(fleet_proto::response::SCHEDULES_CAPABILITY.to_owned());
                }
                app.schedules.begin_load(&id);
                app.schedules.apply(
                    &id,
                    vec![
                        schedule("sch-00000001", &id, true),
                        schedule("sch-00000002", &id, false),
                        schedule("sch-00000003", &id, true),
                    ],
                );
                id
            })
        });
        (state, board)
    }

    fn run_ids(requests: &RecordedRequests) -> Vec<String> {
        requests
            .take()
            .into_iter()
            .filter_map(|body| match body {
                RequestBody::RunScheduleNow { id } => Some(id.to_string()),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    fn r_sends_one_request_per_enabled_schedule(cx: &mut TestAppContext) {
        let (state, _) = seeded(cx, true);
        let (bridge, requests) = Bridge::recording();
        cx.update(|cx| run_schedules(&state, &bridge, cx));

        assert_eq!(run_ids(&requests), vec!["sch-00000001", "sch-00000003"]);
    }

    #[gpui::test]
    fn r_on_a_daemon_without_schedules_sends_nothing(cx: &mut TestAppContext) {
        let (state, _) = seeded(cx, false);
        let (bridge, requests) = Bridge::recording();
        cx.update(|cx| run_schedules(&state, &bridge, cx));

        assert!(run_ids(&requests).is_empty());
    }

    fn toast_texts(state: &Entity<AppState>, cx: &mut TestAppContext) -> Vec<String> {
        state.read_with(cx, |app, _| {
            app.toasts
                .iter()
                .map(|toast| toast.toast.text.to_string())
                .collect()
        })
    }

    #[gpui::test]
    fn r_before_the_schedules_are_listed_says_so_rather_than_none(cx: &mut TestAppContext) {
        let (state, board) = seeded(cx, true);
        state.update(cx, |app, _| app.schedules.clear());
        let (bridge, requests) = Bridge::recording();
        cx.update(|cx| run_schedules(&state, &bridge, cx));
        assert!(run_ids(&requests).is_empty());
        assert_eq!(toast_texts(&state, cx), [SCHEDULES_LOADING]);

        // A failed load says why, not that the board has no schedule.
        state.update(cx, |app, _| {
            app.schedules.begin_load(&board);
            app.schedules
                .load_failed(&board, "schedules.json is unreadable".to_owned());
        });
        cx.update(|cx| run_schedules(&state, &bridge, cx));
        assert!(run_ids(&requests).is_empty());
        assert_eq!(
            toast_texts(&state, cx).last().map(String::as_str),
            Some("schedules.json is unreadable")
        );
    }

    #[gpui::test]
    fn a_skipped_run_now_says_skipped_with_its_summary(cx: &mut TestAppContext) {
        let (state, board) = seeded(cx, true);
        let mut skipped = schedule("sch-00000001", &board, true);
        skipped.name = "GitHub reviews".to_owned();
        skipped.runs.push(fleet_core::schedule::ScheduleRun {
            job_id: None,
            started_at: "2026-09-24T10:05:00Z".to_owned(),
            ended_at: Some("2026-09-24T10:05:00Z".to_owned()),
            outcome: Some(fleet_core::schedule::ScheduleOutcome::Skipped),
            summary: Some("the previous run is still going".to_owned()),
            cost_usd: None,
            log_path: None,
        });
        cx.update(|cx| run_answered(&state, Ok(ResponseBody::Schedule(skipped)), cx));
        assert_eq!(
            toast_texts(&state, cx),
            ["skipped GitHub reviews: the previous run is still going"]
        );

        // A started run is the strip's to report.
        let started = schedule("sch-00000003", &board, true);
        cx.update(|cx| run_answered(&state, Ok(ResponseBody::Schedule(started)), cx));
        assert_eq!(toast_texts(&state, cx).len(), 1);
    }
}
