use super::*;

struct ExpandedLog {
    lines: Arc<[SharedString]>,
    following: bool,
    offset: usize,
    job: JobId,
}

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
        let (filter, cursor, expanded, log, following, log_offset, confirming, log_title) =
            self.state.read_with(cx, |panel, _| {
                (
                    panel.filter,
                    panel.cursor,
                    panel.expanded.clone(),
                    panel.log.clone(),
                    panel.following,
                    panel.log_offset,
                    panel.confirming_cancel_all,
                    panel.log_title.clone(),
                )
            });

        let prepared = &self.state.read(cx).prepared;
        let visible = prepared.rows.clone();
        let counts = prepared.counts;
        let cancellable = prepared.cancellable;
        let requests = actions::JobsRequests::bridge(bridge.clone());
        let select = self.select_row(state);
        let body = if let Some(job) = expanded.clone() {
            self.log_body(
                ExpandedLog {
                    lines: log,
                    following,
                    offset: log_offset,
                    job,
                },
                state,
                requests.clone(),
                cx,
            )
        } else {
            list_body(ListBody {
                rows: visible,
                cursor,
                filter,
                scroll: &self.list_scroll,
                now: now_unix(),
                pointer: row_pointer(&select),
                select,
            })
        };

        let title = match (&expanded, log_title) {
            (Some(_), Some(title)) => jobs_panel::log_header(&title, following, cx),
            _ => jobs_panel::header(
                jobs_panel::HeaderProps {
                    counts,
                    filter,
                    cancellable,
                    on_filter: self.pick_filter(state),
                },
                cx,
            ),
        };
        let header = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(title)
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
            .on_action(self.on_bottom(state, requests.clone()))
            .on_action(self.on_toggle_log(state, requests.clone()))
            .on_action(self.on_cancel(state, requests.clone()))
            .on_action(self.on_cancel_all(state, requests.clone()))
            .on_action(self.on_retry(state, requests.clone()))
            .on_action(self.on_copy_log_path(state))
            .on_action(self.on_dismiss(state, requests.clone()))
            .on_action(self.on_cycle_filter(state, requests))
            .on_action(self.on_collapse_log(state))
            .on_action(self.on_close(state))
            .when(expanded.is_some(), |el| el.key_context("Log"))
            .child(
                Sheet::new(true)
                    .dismiss_action(Box::new(jobs_actions::Close))
                    .expanded(expanded.is_some())
                    .header(header)
                    .body(body)
                    .footer(jobs_panel::footer(cx)),
            )
            .into_any_element()
    }

    /// Moves the cursor to a row the pointer pressed: what `j`/`k` do, minus the motion.
    pub(super) fn select_row(&self, state: &Entity<AppState>) -> jobs_panel::SelectRow {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        Rc::new(move |ix, _, cx| {
            panel.update(cx, |panel, cx| {
                let len = panel.visible_len(snapshot_jobs(&state, cx));
                panel.cursor = ix.min(len.saturating_sub(1));
                scroll.scroll_to_reveal_item(panel.cursor);
                mirror_panel(panel, &state, cx);
            });
            notify(&state, cx);
        })
    }

    /// Picks a filter from its segment: what `f` does, straight to the position.
    pub(super) fn pick_filter(&self, state: &Entity<AppState>) -> jobs_panel::PickFilter {
        let panel = self.state.clone();
        let state = state.clone();
        Rc::new(move |filter, _, cx| {
            panel.update(cx, |panel, cx| {
                panel.filter = filter;
                let len = panel.visible_len(snapshot_jobs(&state, cx));
                panel.clamp(len);
                mirror_panel(panel, &state, cx);
            });
            notify(&state, cx);
        })
    }

    /// The expanded log: the last [`LOG_TAIL_LINES`] lines of `logs/jobs/<id>.log`.
    fn log_body(
        &self,
        log: ExpandedLog,
        state: &Entity<AppState>,
        requests: actions::JobsRequests,
        cx: &App,
    ) -> AnyElement {
        if log.lines.is_empty() {
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
        let log_scroll = self.log_scroll.clone();
        LogView::from_shared("jobs-panel-log", log.lines)
            .following(log.following)
            .top(log.offset)
            .track_scroll(&self.log_scroll)
            .on_command(move |command, _, cx| {
                let resume = panel.update(cx, |panel, _| {
                    let was_following = panel.following;
                    panel.apply_log_command(command);
                    !was_following && panel.following
                });
                if resume {
                    start_tail(
                        panel.clone(),
                        state.clone(),
                        requests.clone(),
                        log_scroll.clone(),
                        log.job.clone(),
                        cx,
                    );
                }
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
    pub(super) counts: jobs_panel::JobCounts,
    pub(super) cancellable: usize,
}

impl PreparedJobs {
    pub(super) fn update(
        &mut self,
        jobs: &[JobRecord],
        filter: JobFilter,
        scroll: &ListState,
    ) -> bool {
        let source_changed = self.source != jobs;
        if !source_changed && self.filter == filter {
            return false;
        }
        let previous: std::collections::HashMap<_, _> = self
            .filter
            .visible(&self.source)
            .zip(self.rows.iter())
            .map(|(job, row)| (&job.id, (job, row)))
            .collect();
        let visible: Vec<&JobRecord> = filter.visible(jobs).collect();
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
    cx: &mut App,
) {
    let changed = panel.update(cx, |panel, cx| {
        let app = state.read(cx);
        if !matches!(app.overlay, Some(Overlay::Jobs)) {
            panel.close();
            return false;
        }
        let jobs = app
            .snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.jobs.as_slice());
        let changed = panel.prepared.update(jobs, panel.filter, scroll);
        panel.clamp(panel.prepared.rows.len());
        if !panel.opened {
            panel.opened = true;
            if let Some(job) = &app.jobs_focus {
                panel.focus_job(jobs, job);
            }
        }
        // Publish even when opening did not focus a sticky-error job: the first harness
        // projection must see the panel's seeded cursor and filter.
        mirror_panel(panel, state, cx);
        changed
    });
    if changed {
        notify(state, cx);
    }
}

/// What the job list draws, and how its rows answer the pointer.
pub(super) struct ListBody<'a> {
    pub(super) rows: Rc<[Rc<crate::presentation::JobDisplay>]>,
    pub(super) cursor: usize,
    pub(super) filter: JobFilter,
    pub(super) scroll: &'a ListState,
    pub(super) now: i64,
    pub(super) pointer: ListPointer,
    pub(super) select: jobs_panel::SelectRow,
}

/// The rows' click / double-click / right-click contract (UX-SPEC §5.1): a press selects, a
/// double-click opens the log as `⏎` does, and a right click selects before the row's
/// `ContextMenu` opens its menu.
pub(super) fn row_pointer(select: &jobs_panel::SelectRow) -> ListPointer {
    let select = select.clone();
    ListPointer::new()
        .on_select(move |ix, window, cx| select(ix, window, cx))
        .on_open(|_, window, cx| window.dispatch_action(Box::new(jobs_actions::ToggleLog), cx))
        // The row's `ContextMenu` shows the menu; this handler exists so the right click is
        // routed through `on_select` first and the menu acts on the row under the pointer.
        .on_menu(|_, _, _, _| {})
}

pub(super) fn list_body(body: ListBody<'_>) -> AnyElement {
    let ListBody {
        rows,
        cursor,
        filter,
        scroll,
        now,
        pointer,
        select,
    } = body;
    if rows.is_empty() {
        return div()
            .size_full()
            .child(jobs_panel::empty_state(filter))
            .into_any_element();
    }
    // The "Finished" label only means something under rows of another kind: a list that is all
    // finished work, or the Done segment, has nothing to separate it from.
    let grouped = filter != JobFilter::Done;
    gpui::list(scroll.clone(), move |index, _, cx| {
        rows.get(index).map_or_else(
            || div().into_any_element(),
            |job| {
                let first_finished = grouped
                    && job.is_finished()
                    && index
                        .checked_sub(1)
                        .and_then(|previous| rows.get(previous))
                        .is_some_and(|previous| !previous.is_finished());
                jobs_panel::job_row(
                    job,
                    &jobs_panel::RowContext {
                        ix: index,
                        cursor: index == cursor,
                        first_finished,
                        now,
                        pointer: &pointer,
                        select: &select,
                    },
                    cx,
                )
            },
        )
    })
    .size_full()
    .into_any_element()
}
