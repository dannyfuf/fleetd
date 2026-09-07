use fleet_proto::job::{JobKind, JobStatus};

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
