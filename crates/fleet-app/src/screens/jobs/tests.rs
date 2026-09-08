use async_channel::Sender;
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    job::{JobKind, JobStatus},
    snapshot::{DaemonInfo, Snapshot},
};
use std::{collections::VecDeque, sync::Mutex};

use super::*;

fn job(id: &str, status: JobStatus, cancellable: bool, retryable: bool) -> JobRecord {
    JobRecord {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::Clone,
        target: "nixos".to_owned(),
        title: "Clone nixos".to_owned(),
        status,
        progress: None,
        log_path: "/tmp/j.log".to_owned(),
        started_at: "2026-09-04T12:00:00Z".to_owned(),
        finished_at: None,
        cancellable,
        retryable,
    }
}

fn panel(filter: JobFilter) -> PanelState {
    PanelState {
        filter,
        following: true,
        ..PanelState::default()
    }
}

fn app_with_jobs(home: &str, jobs: Vec<JobRecord>) -> AppState {
    let mut app = AppState::new(home, std::time::Instant::now());
    app.snapshot = Some(Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        contexts: vec![],
        repos: vec![],
        clones: vec![],
        worktrees: vec![],
        active_context: None,
        sessions: vec![],
        agent_threads: Vec::new(),
        statuses: vec![],
        pools: vec![],
        hosts: vec![],
        jobs,
        daemon: DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    });
    app
}

struct PendingRequest {
    body: RequestBody,
    reply: Sender<Result<ResponseBody, ProtoError>>,
}

#[derive(Clone, Default)]
struct RequestHarness(Arc<Mutex<VecDeque<PendingRequest>>>);

impl RequestHarness {
    fn requests(&self) -> actions::JobsRequests {
        let pending = Arc::clone(&self.0);
        actions::JobsRequests::from_fn(Arc::new(move |body| {
            let (reply, response) = async_channel::bounded(1);
            pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_back(PendingRequest { body, reply });
            response
        }))
    }

    fn len(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    fn respond(&self, response: Result<ResponseBody, ProtoError>) -> RequestBody {
        let request = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or_else(|| panic!("expected a pending jobs request"));
        request
            .reply
            .try_send(response)
            .unwrap_or_else(|_| panic!("jobs reply receiver should still be live"));
        request.body
    }
}

fn refusal(message: &str) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Conflict,
        message: message.into(),
    }
}

struct ActionHarness;

impl gpui::Render for ActionHarness {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

#[test]
fn the_cursor_indexes_the_visible_rows_not_the_daemons_list() {
    let jobs = vec![
        job("job-a", JobStatus::Succeeded, false, false),
        job(
            "job-b",
            JobStatus::Failed {
                error: "boom".to_owned(),
            },
            false,
            true,
        ),
        job("job-c", JobStatus::Running, true, false),
    ];
    let mut panel = panel(JobFilter::Failed);
    panel.cursor = 0;
    assert_eq!(
        panel.selected(&jobs).map(|job| job.id.as_str().to_owned()),
        Some("job-b".to_owned())
    );
    assert_eq!(panel.visible_len(&jobs), 1);
}

#[test]
fn the_cursor_is_clamped_when_the_filter_shrinks_the_list() {
    let jobs = [
        job("job-a", JobStatus::Running, true, false),
        job("job-b", JobStatus::Running, true, false),
        job("job-c", JobStatus::Succeeded, false, false),
    ];
    let mut panel = panel(JobFilter::All);
    panel.cursor = 2;
    panel.filter = JobFilter::Running;
    panel.clamp(panel.visible_len(&jobs));
    assert_eq!(panel.cursor, 1);
}

#[test]
fn the_sticky_error_opens_the_panel_on_its_own_job() {
    let jobs = vec![
        job("job-a", JobStatus::Running, true, false),
        job(
            "job-b",
            JobStatus::Failed {
                error: "gh: HTTP 502".to_owned(),
            },
            false,
            true,
        ),
    ];
    let mut panel = panel(JobFilter::All);
    let failed = jobs[1].id.clone();
    assert!(panel.focus_job(&jobs, &failed));
    assert_eq!(panel.cursor, 1);

    // A job the filter hides cannot take the cursor.
    panel.filter = JobFilter::Running;
    panel.cursor = 0;
    assert!(!panel.focus_job(&jobs, &failed));
    assert_eq!(panel.cursor, 0);
}

#[test]
fn collapsing_a_log_clears_it_and_reports_whether_it_did_anything() {
    let mut panel = panel(JobFilter::All);
    assert!(!panel.collapse(), "collapsing a collapsed panel is a no-op");
    panel.expanded = Some("job-a".parse().unwrap_or_else(|error| panic!("{error}")));
    panel.log = Arc::from([SharedString::new_static("line")]);
    panel.log_offset = 7;
    assert!(panel.collapse());
    assert!(panel.log.is_empty());
    assert_eq!(panel.log_offset, 0);
    assert_eq!(panel.expanded, None);
}

#[test]
fn log_commands_persist_wheel_position_and_follow_state() {
    let mut panel = panel(JobFilter::All);
    panel.log = Arc::from([
        SharedString::new_static("first"),
        SharedString::new_static("second"),
        SharedString::new_static("third"),
    ]);

    panel.apply_log_command(LogCommand::ScrollTo(1));
    assert!(!panel.following);
    assert_eq!(panel.log_offset, 1);

    panel.apply_log_command(LogCommand::Follow);
    assert!(panel.following);
    assert_eq!(panel.log_offset, 2);
}

#[test]
fn cancel_and_retry_are_offered_only_when_they_can_work() {
    assert!(can_cancel(&job("job-a", JobStatus::Running, true, false)));
    assert!(!can_cancel(&job("job-b", JobStatus::Running, false, false)));
    assert!(!can_cancel(&job(
        "job-c",
        JobStatus::Succeeded,
        true,
        false
    )));
}

#[test]
fn repeated_tails_retain_the_same_shared_lines() {
    let mut panel = panel(JobFilter::All);
    let scroll = UniformListScrollHandle::new();
    let lines: Vec<String> = (0..1000).map(|index| format!("line {index}")).collect();
    assert!(panel.apply_tail(lines.clone(), &scroll));
    let before = panel.log.clone();
    for _ in 0..20 {
        assert!(!panel.apply_tail(lines.clone(), &scroll));
    }
    assert!(Arc::ptr_eq(&before, &panel.log));
    let mut changed = lines;
    changed[500] = "updated".into();
    assert!(panel.apply_tail(changed, &scroll));
    assert!(!Arc::ptr_eq(&before, &panel.log));
}

#[test]
fn paused_log_is_not_replaced_by_tail_poll() {
    let mut panel = panel(JobFilter::All);
    let scroll = UniformListScrollHandle::new();
    assert!(panel.apply_tail(vec!["first".into(), "inspected".into()], &scroll));
    panel.following = false;
    panel.log_offset = 0;

    assert!(!panel.apply_tail(vec!["newest".into()], &scroll));
    assert_eq!(
        panel.log.as_ref(),
        &[
            SharedString::new_static("first"),
            SharedString::new_static("inspected")
        ]
    );
    assert_eq!(panel.log_offset, 0);
}

#[test]
fn expanded_actions_keep_expanded_job_identity() {
    let jobs = vec![
        job("job-a", JobStatus::Running, true, false),
        job("job-b", JobStatus::Running, true, false),
    ];
    let mut panel = panel(JobFilter::All);
    panel.cursor = 1;
    panel.expanded = Some(jobs[0].id.clone());

    assert_eq!(
        panel.action_job(&jobs).map(|job| &job.id),
        Some(&jobs[0].id)
    );
}

#[test]
fn dismiss_updates_only_acknowledged_jobs() {
    let dismissed = job("job-a", JobStatus::Succeeded, false, false);
    let retained = job("job-b", JobStatus::Running, true, false);
    let mut app = app_with_jobs(
        "/tmp/fleet-jobs-dismiss",
        vec![dismissed.clone(), retained.clone()],
    );
    app.sticky_error = Some(StickyError {
        text: "unrelated".into(),
        job: Some(retained.id.clone()),
        retryable: false,
    });
    let gone = [dismissed.id.clone()];

    let refusal = actions::mutation_failure(
        Ok(Err(fleet_proto::error::ProtoError {
            kind: fleet_proto::error::ErrorKind::Conflict,
            message: "still running".into(),
        })),
        actions::ExpectedMutation::Dismiss,
        "dismiss jobs",
    );
    assert_eq!(refusal.as_deref(), Some("still running"));
    assert_eq!(app.snapshot.as_ref().expect("snapshot").jobs.len(), 2);
    assert!(app.seen_failed.is_empty());

    actions::acknowledge_dismissal(&mut app, &gone);
    assert_eq!(
        app.snapshot
            .as_ref()
            .expect("snapshot")
            .jobs
            .iter()
            .map(|job| job.id.as_str())
            .collect::<Vec<_>>(),
        vec![retained.id.as_str()]
    );
    assert!(app.seen_failed.contains(&dismissed.id));
    assert_eq!(
        app.sticky_error.as_ref().map(|error| error.text.as_str()),
        Some("unrelated")
    );
}

#[test]
fn job_mutation_refusals_are_visible() {
    let updated = job("job-a", JobStatus::Running, true, false);
    let selected = updated.id.clone();
    let failure = actions::mutation_failure(
        Ok(Err(refusal("job has already finished"))),
        actions::ExpectedMutation::Cancel(&selected),
        "cancel job",
    )
    .expect("refusal");
    let mut app = app_with_jobs(
        "/tmp/fleet-jobs-mutation",
        vec![job("job-a", JobStatus::Queued, true, false)],
    );
    actions::record_mutation_failure(&mut app, failure, None, false);
    app.apply_job(updated, std::time::Instant::now());

    let error = app.sticky_error.as_ref().expect("sticky refusal");
    assert_eq!(error.text, "job has already finished");
    assert_eq!(error.job, None);
    assert!(app.toasts.is_empty());
}

#[gpui::test]
fn cancel_refusal_from_request_is_visible(cx: &mut gpui::TestAppContext) {
    let harness = RequestHarness::default();
    let requests = harness.requests();
    let state = cx.new(|_| {
        app_with_jobs(
            "/tmp/fleet-jobs-cancel",
            vec![job("job-a", JobStatus::Running, true, false)],
        )
    });
    let jobs = cx.update(JobsPanel::new);
    let handler = jobs.on_cancel(&state, requests);
    let window = cx.add_window(|_, _| ActionHarness);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);

    window
        .update(&mut visual, |_, window, cx| {
            handler(&jobs_actions::CancelJob, window, cx)
        })
        .unwrap_or_else(|error| panic!("dispatch cancel: {error}"));
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(refusal("job has already finished"))),
        RequestBody::CancelJob { job } if job.as_str() == "job-a"
    ));
    visual.run_until_parked();

    visual.update(|_, cx| {
        let app = state.read(cx);
        let error = app.sticky_error.as_ref().expect("sticky refusal");
        assert_eq!(error.text, "job has already finished");
        assert_eq!(error.job, None);
    });
}

#[gpui::test]
fn cancel_all_refusal_from_request_is_visible(cx: &mut gpui::TestAppContext) {
    let harness = RequestHarness::default();
    let requests = harness.requests();
    let state = cx.new(|_| {
        app_with_jobs(
            "/tmp/fleet-jobs-cancel-all",
            vec![job("job-a", JobStatus::Running, true, false)],
        )
    });
    let jobs = cx.update(JobsPanel::new);
    let handler = jobs.on_cancel_all(&state, requests);
    let window = cx.add_window(|_, _| ActionHarness);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);

    window
        .update(&mut visual, |_, window, cx| {
            handler(&jobs_actions::CancelAll, window, cx);
            handler(&jobs_actions::CancelAll, window, cx);
        })
        .unwrap_or_else(|error| panic!("dispatch cancel all: {error}"));
    assert_eq!(harness.len(), 1);
    assert!(matches!(
        harness.respond(Err(refusal("cancel all refused"))),
        RequestBody::CancelJob { job } if job.as_str() == "job-a"
    ));
    visual.run_until_parked();

    visual.update(|_, cx| {
        let app = state.read(cx);
        assert_eq!(
            app.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("cancel all refused")
        );
    });
}

#[gpui::test]
fn dismiss_waits_for_daemon_acknowledgement(cx: &mut gpui::TestAppContext) {
    let harness = RequestHarness::default();
    let requests = harness.requests();
    let state = cx.new(|_| {
        app_with_jobs(
            "/tmp/fleet-jobs-dismiss-await",
            vec![
                job("job-a", JobStatus::Succeeded, false, false),
                job("job-b", JobStatus::Running, true, false),
            ],
        )
    });
    let jobs = cx.update(JobsPanel::new);
    let handler = jobs.on_dismiss(&state, requests);
    let window = cx.add_window(|_, _| ActionHarness);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);

    window
        .update(&mut visual, |_, window, cx| {
            handler(&jobs_actions::DismissFinished, window, cx)
        })
        .unwrap_or_else(|error| panic!("dispatch dismissal: {error}"));
    assert_eq!(harness.len(), 1);
    visual.update(|_, cx| {
        let app = state.read(cx);
        assert_eq!(app.snapshot.as_ref().expect("snapshot").jobs.len(), 2);
        assert!(app.seen_failed.is_empty());
    });
    assert!(matches!(
        harness.respond(Ok(ResponseBody::Ack)),
        RequestBody::DismissJobs { jobs } if jobs.iter().any(|job| job.as_str() == "job-a")
    ));
    visual.run_until_parked();

    visual.update(|_, cx| {
        let app = state.read(cx);
        let retained = &app.snapshot.as_ref().expect("snapshot").jobs;
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].id.as_str(), "job-b");
    });
}

#[gpui::test]
fn dismiss_not_found_is_acknowledged(cx: &mut gpui::TestAppContext) {
    let harness = RequestHarness::default();
    let requests = harness.requests();
    let state = cx.new(|_| {
        app_with_jobs(
            "/tmp/fleet-jobs-dismiss-not-found",
            vec![job("job-a", JobStatus::Succeeded, false, false)],
        )
    });
    let jobs = cx.update(JobsPanel::new);
    let handler = jobs.on_dismiss(&state, requests);
    let window = cx.add_window(|_, _| ActionHarness);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);

    window
        .update(&mut visual, |_, window, cx| {
            handler(&jobs_actions::DismissFinished, window, cx)
        })
        .unwrap_or_else(|error| panic!("dispatch dismissal: {error}"));
    assert!(matches!(
        harness.respond(Err(ProtoError {
            kind: ErrorKind::NotFound,
            message: "job was already pruned".into(),
        })),
        RequestBody::DismissJobs { .. }
    ));
    visual.run_until_parked();

    visual.update(|_, cx| {
        let app = state.read(cx);
        assert!(app.snapshot.as_ref().expect("snapshot").jobs.is_empty());
        assert!(app.sticky_error.is_none());
    });
}

#[gpui::test]
fn closing_or_replacing_the_overlay_cancels_following(cx: &mut gpui::TestAppContext) {
    use std::cell::Cell;
    struct OnDrop(Rc<Cell<bool>>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let state = cx.update(|cx| {
        cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet-jobs-test", std::time::Instant::now());
            state.overlay = Some(Overlay::Jobs);
            state
        })
    });
    let mut jobs = cx.update(JobsPanel::new);
    cx.update(|cx| jobs.bind(&state, cx));
    cx.run_until_parked();
    let cancelled = Rc::new(Cell::new(false));
    let guard = OnDrop(cancelled.clone());
    cx.update(|cx| {
        let task = cx.spawn(async move |cx| {
            let _guard = guard;
            cx.background_executor()
                .timer(Duration::from_secs(60))
                .await;
        });
        jobs.state.update(cx, |panel, _| {
            panel.expanded = Some("job-a".parse().expect("job id"));
            panel.tail = Some(task);
            panel.log = Arc::from([SharedString::new_static("retained")]);
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        state.update(cx, |state, cx| {
            state.overlay = None;
            cx.notify();
        })
    });
    cx.run_until_parked();
    assert!(cancelled.get());
    cx.read(|cx| {
        let panel = jobs.state.read(cx);
        assert!(panel.tail.is_none());
        assert!(panel.expanded.is_none());
        assert!(!panel.opened);
        assert!(panel.log.is_empty());
    });
}

#[gpui::test]
fn job_list_measures_mixed_heights_and_reuses_unchanged_rows(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| fleet_ui_kit::Theme::init(fleet_ui_kit::ThemeMode::Dark, cx));
    let mut jobs: Vec<_> = (0..10_000)
        .map(|index| job(&format!("job-{index}"), JobStatus::Succeeded, false, false))
        .collect();
    jobs[1].status = JobStatus::Failed {
        error: "failed command".into(),
    };
    let scroll = ListState::new(0, ListAlignment::Top, gpui::px(0.0));
    let mut prepared = presentation::PreparedJobs::default();
    assert!(prepared.update(&jobs, JobFilter::All, None, &scroll));
    let rows = prepared.rows.clone();
    assert!(!prepared.update(&jobs, JobFilter::All, None, &scroll));
    assert!(Rc::ptr_eq(&rows, &prepared.rows));
    let cx = cx.add_empty_window();
    let harness = cx.new(|_| JobListHarness {
        rows: rows.clone(),
        scroll: scroll.clone(),
    });
    JobListHarness::draw(&harness, cx);
    assert_eq!(
        scroll
            .bounds_for_item(0)
            .expect("first row laid out")
            .size
            .height,
        gpui::px(30.0)
    );
    assert_eq!(
        scroll
            .bounds_for_item(1)
            .expect("failure laid out")
            .size
            .height,
        gpui::px(44.0)
    );
    assert!(
        scroll.bounds_for_item(5000).is_none(),
        "offscreen rows must not be laid out"
    );
    jobs[1].status = JobStatus::Succeeded;
    assert!(prepared.update(&jobs, JobFilter::All, None, &scroll));
    assert!(Rc::ptr_eq(&rows[0], &prepared.rows[0]));
    assert!(Rc::ptr_eq(&rows[9999], &prepared.rows[9999]));
    assert!(!Rc::ptr_eq(&rows[1], &prepared.rows[1]));
    harness.update(cx, |harness, _| harness.rows = prepared.rows.clone());
    JobListHarness::draw(&harness, cx);
    assert_eq!(
        scroll
            .bounds_for_item(1)
            .expect("changed row laid out")
            .size
            .height,
        gpui::px(30.0)
    );
}

/// A root view for the list measurement test: `gpui::list` needs a rendered entity, which
/// `VisualTestContext::draw` only provides through a view.
struct JobListHarness {
    rows: Rc<[Rc<crate::presentation::JobDisplay>]>,
    scroll: ListState,
}

impl gpui::Render for JobListHarness {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        presentation::list_body(
            self.rows.clone(),
            0,
            JobFilter::All,
            &self.scroll,
            1_788_523_230,
        )
    }
}

impl JobListHarness {
    /// Lays the list out in a 440x140 viewport, the size the panel gets in the shell.
    fn draw(this: &Entity<Self>, cx: &mut gpui::VisualTestContext) {
        let view = this.clone();
        cx.draw(
            gpui::Point::default(),
            gpui::size(gpui::px(440.0), gpui::px(140.0)),
            |_, _| view.into_any_element(),
        );
    }
}
