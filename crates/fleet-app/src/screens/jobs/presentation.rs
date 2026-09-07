use super::*;

impl JobsPanel {
    /// Renders the panel into the frame's overlay layer.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.bind(state, cx);
        let daemon_lost = state.read(cx).daemon.is_lost();
        let stale_age = state.read(cx).snapshot_age(std::time::Instant::now());
        let (filter, cursor, expanded, log, following, log_offset, confirming) =
            self.state.read_with(cx, |panel, _| {
                (
                    panel.filter,
                    panel.cursor,
                    panel.expanded.clone(),
                    panel.log.clone(),
                    panel.following,
                    panel.log_offset,
                    panel.confirming_cancel_all,
                )
            });

        let prepared = &self.state.read(cx).prepared;
        let visible = prepared.rows.clone();
        let shown = visible.len();
        let total = prepared.total;
        let counts = prepared.counts;
        let cancellable = prepared.cancellable;
        let log_path = prepared.log_paths.get(cursor).cloned();
        let body = if expanded.is_some() {
            self.log_body(log, following, log_offset, state, cx)
        } else {
            presentation::list_body(visible, cursor, filter, &self.list_scroll, now_unix())
        };
        let requests = actions::JobsRequests::bridge(bridge.clone());

        let header = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(jobs_panel::header(counts, filter, shown, total, cx))
            .child(jobs_panel::log_path_row(log_path, cx))
            // §3.7 "States": daemon down → an amber strip at the top, rows still readable.
            .children(
                daemon_lost.then(|| jobs_panel::daemon_down_strip(stale_age.unwrap_or(0), cx)),
            )
            .children(confirming.then(|| jobs_panel::cancel_all_confirm(cancellable, cx)));

        div()
            .track_focus(focus)
            // The shell hands the panel to `AppFrame::body_overlay`, whose band already ends
            // at the two bars (§3.7), so the sheet simply fills it.
            .absolute()
            .inset_0()
            .on_action(self.on_move_down(state))
            .on_action(self.on_move_up(state))
            .on_action(self.on_top(state))
            .on_action(self.on_bottom(state))
            .on_action(self.on_toggle_log(state, bridge))
            .on_action(self.on_cancel(state, requests.clone()))
            .on_action(self.on_cancel_all(state, requests.clone()))
            .on_action(self.on_retry(state, requests.clone()))
            .on_action(self.on_copy_log_path(state))
            .on_action(self.on_dismiss(state, requests))
            .on_action(self.on_cycle_filter(state))
            .on_action(self.on_collapse_log(state))
            .on_action(self.on_close(state))
            .when(expanded.is_some(), |el| el.key_context("Log"))
            .child(
                Sheet::new(true)
                    .expanded(expanded.is_some())
                    .header(header)
                    .body(body)
                    .footer(jobs_panel::footer(expanded.is_some(), cx)),
            )
            .into_any_element()
    }

    /// The expanded log: the last [`LOG_TAIL_LINES`] lines of `logs/jobs/<id>.log`.
    fn log_body(
        &self,
        log: Arc<[SharedString]>,
        following: bool,
        log_offset: usize,
        state: &Entity<AppState>,
        cx: &App,
    ) -> AnyElement {
        if log.is_empty() {
            let theme = cx.theme();
            return div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(fleet_ui_kit::Text::ui("The log is empty so far.").tone(Tone::Muted))
                .p(theme.space.lg)
                .into_any_element();
        }
        let panel = self.state.clone();
        let state = state.clone();
        LogView::from_shared("jobs-panel-log", log)
            .following(following)
            .top(log_offset)
            .track_scroll(&self.log_scroll)
            .on_command(move |command, _, cx| {
                panel.update(cx, |panel, _| panel.apply_log_command(command));
                notify(&state, cx);
            })
            .into_any_element()
    }
}

#[derive(Default)]
pub(super) struct PreparedJobs {
    source: Vec<JobRecord>,
    filter: JobFilter,
    pub(super) rows: Rc<[Rc<crate::presentation::JobDisplay>]>,
    pub(super) log_paths: Vec<SharedString>,
    pub(super) total: usize,
    pub(super) counts: jobs_panel::JobCounts,
    pub(super) cancellable: usize,
}

impl PreparedJobs {
    pub(super) fn update(
        &mut self,
        jobs: &[JobRecord],
        filter: JobFilter,
        home: Option<&std::path::Path>,
        scroll: &ListState,
    ) -> bool {
        let source_changed = self.source != jobs;
        if !source_changed && self.filter == filter {
            return false;
        }
        let previous: std::collections::HashMap<_, _> = self
            .source
            .iter()
            .filter(|job| self.filter.matches(job))
            .zip(self.rows.iter())
            .map(|(job, row)| (&job.id, (job, row)))
            .collect();
        let visible: Vec<&JobRecord> = jobs.iter().filter(|job| filter.matches(job)).collect();
        let rows: Rc<[_]> = visible
            .iter()
            .map(|job| {
                previous
                    .get(&job.id)
                    .filter(|(old, _)| old == job)
                    .map_or_else(
                        || Rc::new(crate::presentation::JobDisplay::new(job)),
                        |(_, row)| (*row).clone(),
                    )
            })
            .collect();
        let mut start = 0;
        while start < rows.len().min(self.rows.len()) && Rc::ptr_eq(&rows[start], &self.rows[start])
        {
            start += 1;
        }
        let mut suffix = 0;
        while suffix < rows.len().min(self.rows.len()).saturating_sub(start)
            && Rc::ptr_eq(
                &rows[rows.len() - suffix - 1],
                &self.rows[self.rows.len() - suffix - 1],
            )
        {
            suffix += 1;
        }
        scroll.splice(start..self.rows.len() - suffix, rows.len() - start - suffix);
        self.rows = rows;
        self.log_paths = visible
            .iter()
            .map(|job| {
                crate::presentation::tilde(&job.log_path, home)
                    .into_owned()
                    .into()
            })
            .collect();
        self.total = jobs.len();
        self.counts = jobs_panel::job_counts(jobs);
        self.cancellable = jobs.iter().filter(|job| can_cancel(job)).count();
        if source_changed {
            self.source = jobs.to_vec();
        }
        self.filter = filter;
        true
    }
}

pub(super) fn synchronize(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    scroll: &ListState,
    home: Option<&std::path::Path>,
    cx: &mut App,
) {
    let changed = panel.update(cx, |panel, cx| {
        let state = state.read(cx);
        if !matches!(state.overlay, Some(Overlay::Jobs)) {
            panel.close();
            return false;
        }
        let jobs = state
            .snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.jobs.as_slice());
        let changed = panel.prepared.update(jobs, panel.filter, home, scroll);
        panel.clamp(panel.prepared.rows.len());
        if !panel.opened {
            panel.opened = true;
            if let Some(job) = &state.jobs_focus {
                panel.focus_job(jobs, job);
            }
        }
        changed
    });
    if changed {
        notify(state, cx);
    }
}

pub(super) fn list_body(
    rows: Rc<[Rc<crate::presentation::JobDisplay>]>,
    cursor: usize,
    filter: JobFilter,
    scroll: &ListState,
    now: i64,
) -> AnyElement {
    if rows.is_empty() {
        return div()
            .size_full()
            .child(jobs_panel::empty_state(filter))
            .into_any_element();
    }
    gpui::list(scroll.clone(), move |index, _, _| {
        rows.get(index).map_or_else(
            || div().into_any_element(),
            |job| jobs_panel::prepared_job_row(job, index == cursor, now),
        )
    })
    .size_full()
    .into_any_element()
}
