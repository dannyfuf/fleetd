//! The Jobs panel: the right-docked sheet of UX-SPEC §3.7.
//!
//! *What is the daemon doing for me, is it stuck, what failed, and what can I do about it?*
//!
//! The panel is an **overlay**: the shell renders it into the frame's overlay layer with the
//! `Jobs` key context, so the list behind stays fully visible and readable — a centered modal
//! would hide exactly the rows the jobs are about. Closing it restores the exact prior focus,
//! which the shell does for free because the pane, row and mode all live in [`AppState`].
//!
//! # What lives where
//!
//! Everything that is a pure function of the job list — the `f` filter, the header counts, the
//! elapsed spelling, which rows carry a sub-line — is in [`crate::views::jobs_panel`] and is
//! unit tested there. This file owns the three things that need the app: the cursor, the log
//! tail, and the daemon calls behind `c` / `X` / `R` / `y` / `D`.
//!
//! # Deliberate deviations, and why
//!
//! * **`X` confirms inside the panel.** §3.7 asks for a confirm; routing it through
//!   `Dialogs::Confirm` would replace the `Jobs` overlay and close the panel underneath the
//!   question. The first `X` arms an amber strip, the second one cancels; `Esc` disarms.
//! * **`D` dismisses through fleetd.** Finished records and their persisted logs are removed by
//!   the daemon, so they stay gone after reconnecting.
//! * **`Esc` collapses an expanded log before closing the panel.** `J` always closes outright.

use std::{collections::HashSet, time::Duration};

use fleet_core::ids::JobId;
use fleet_proto::{
    error::ProtoError,
    job::{JobRecord, JobStatus},
    request::RequestBody,
    response::ResponseBody,
};
use fleet_ui_kit::{ActiveTheme, Icon, LOG_TAIL_LINES, ListView, LogView, Sheet, Tone, prelude::*};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, FocusHandle, ScrollStrategy, SharedString, Task,
    UniformListScrollHandle, Window, div,
};

use crate::{
    actions::jobs as jobs_actions,
    bridge::Bridge,
    state::{AppState, StickyError},
    views::jobs_panel::{
        self, JobFilter, elapsed_label, home_dir, is_dismissable, job_counts, now_unix, tilde,
        visible_jobs,
    },
};

/// How often an expanded log re-asks the daemon for its tail while following.
///
/// The daemon exposes the log as a `TailJob` request rather than a stream, so "follow" is a
/// poll. A quarter of a second is under the 400 ms the spec already spends debouncing an
/// inspect, and it costs one small request per tick on exactly one job.
pub const TAIL_INTERVAL: Duration = Duration::from_millis(250);

/// Mutable panel state. It is a gpui entity so the panel's `on_action` listeners — which only
/// ever get `&mut App` — can reach it; a plain field on [`JobsPanel`] could not be mutated from
/// inside a listener closure.
#[derive(Default)]
pub struct PanelState {
    /// Where `f` is in its cycle.
    pub filter: JobFilter,
    /// The cursor, as an index into the **visible** rows.
    pub cursor: usize,
    /// The job whose log is expanded, if any. Widens the sheet 440 → 640 px.
    pub expanded: Option<JobId>,
    /// The tail of that log, newest last.
    pub log: Vec<SharedString>,
    /// Whether the log view is pinned to the tail (`f` toggles, `G` re-enables).
    pub following: bool,
    /// The first log line the view is scrolled to, when not following.
    pub log_offset: usize,
    /// Whether `X` is armed and waiting for its second press.
    pub confirming_cancel_all: bool,
    /// Whether the panel was already open on the previous render, so the cursor is seeded
    /// exactly once per opening.
    pub opened: bool,
    /// Keeps the follow loop alive; dropping it stops the poll.
    tail: Option<Task<()>>,
}

impl PanelState {
    /// The ids the panel is currently drawing, in order.
    #[must_use]
    pub fn visible_ids(&self, jobs: &[JobRecord]) -> Vec<JobId> {
        visible_jobs(jobs, self.filter, &HashSet::new())
            .into_iter()
            .map(|job| job.id.clone())
            .collect()
    }

    /// The job under the cursor.
    #[must_use]
    pub fn selected<'a>(&self, jobs: &'a [JobRecord]) -> Option<&'a JobRecord> {
        visible_jobs(jobs, self.filter, &HashSet::new())
            .into_iter()
            .nth(self.cursor)
    }

    /// Keeps the cursor inside the list after the data or the filter changed.
    pub fn clamp(&mut self, len: usize) {
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }

    /// Puts the cursor on `job` when it is visible, which is what `!` promises: the sticky
    /// error slot opens this panel *on the failure it names* (§1.8).
    pub fn focus_job(&mut self, jobs: &[JobRecord], job: &JobId) -> bool {
        match self.visible_ids(jobs).iter().position(|id| id == job) {
            Some(index) => {
                self.cursor = index;
                true
            }
            None => false,
        }
    }

    /// Collapses an expanded log and stops its follow loop.
    pub fn collapse(&mut self) -> bool {
        self.tail = None;
        self.log.clear();
        self.log_offset = 0;
        self.expanded.take().is_some()
    }
}

/// The jobs panel.
pub struct JobsPanel {
    state: Entity<PanelState>,
    list_scroll: UniformListScrollHandle,
    log_scroll: UniformListScrollHandle,
}

impl JobsPanel {
    /// Builds the panel. Called once, while the shell is being built.
    #[must_use]
    pub fn new(cx: &mut App) -> Self {
        Self {
            state: cx.new(|_| PanelState {
                following: true,
                ..PanelState::default()
            }),
            list_scroll: UniformListScrollHandle::new(),
            log_scroll: UniformListScrollHandle::new(),
        }
    }

    /// The panel's own state, for tests and for the shell's diagnostics.
    #[must_use]
    pub fn state(&self) -> &Entity<PanelState> {
        &self.state
    }

    /// Renders the panel into the frame's overlay layer.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let jobs: Vec<JobRecord> = state
            .read(cx)
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.jobs.clone())
            .unwrap_or_default();
        let daemon_lost = state.read(cx).daemon.is_lost();
        let stale_age = state.read(cx).snapshot_age(std::time::Instant::now());
        let focus_job = state.read(cx).jobs_focus.clone();

        // Seed the cursor once per opening: `!` promises the panel opens on the failure the
        // sticky slot names, and `J` is unharmed by landing on the same row.
        self.state.update(cx, |panel, _| {
            let len = panel.visible_ids(&jobs).len();
            panel.clamp(len);
            if !panel.opened {
                panel.opened = true;
                if let Some(job) = focus_job.as_ref() {
                    panel.focus_job(&jobs, job);
                }
            }
        });

        let (filter, cursor, expanded, log, following, confirming) =
            self.state.read_with(cx, |panel, _| {
                (
                    panel.filter,
                    panel.cursor,
                    panel.expanded.clone(),
                    panel.log.clone(),
                    panel.following,
                    panel.confirming_cancel_all,
                )
            });

        let visible: Vec<JobRecord> = visible_jobs(&jobs, filter, &HashSet::new())
            .into_iter()
            .cloned()
            .collect();
        let now = now_unix();
        let home = home_dir();
        let log_path = visible
            .get(cursor)
            .map(|job| SharedString::from(tilde(&job.log_path, home.as_deref())));
        let counts = job_counts(&jobs);
        let cancellable = jobs
            .iter()
            .filter(|job| job.cancellable && jobs_panel::is_active(&job.status))
            .count();

        let body: AnyElement = if expanded.is_some() {
            self.log_body(&log, following, cx)
        } else if visible.is_empty() {
            div()
                .size_full()
                .child(jobs_panel::empty_state(filter))
                .into_any_element()
        } else {
            let rows: Vec<JobRecord> = visible.clone();
            ListView::new(
                "jobs-panel-list",
                rows.len(),
                move |index, is_cursor, _, _| {
                    rows.get(index).map_or_else(
                        || div().into_any_element(),
                        |job| jobs_panel::job_row(job, is_cursor, now),
                    )
                },
            )
            .cursor(cursor)
            .track_scroll(&self.list_scroll)
            .into_any_element()
        };

        let header = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(jobs_panel::header(
                counts,
                filter,
                visible.len(),
                jobs.len(),
                cx,
            ))
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
            .on_action(self.on_move_down(state, cx))
            .on_action(self.on_move_up(state, cx))
            .on_action(self.on_top(state, cx))
            .on_action(self.on_bottom(state, cx))
            .on_action(self.on_toggle_log(state, bridge, cx))
            .on_action(self.on_cancel(state, bridge, cx))
            .on_action(self.on_cancel_all(state, bridge, cx))
            .on_action(self.on_retry(state, bridge, cx))
            .on_action(self.on_copy_log_path(state, cx))
            .on_action(self.on_dismiss(state, bridge, cx))
            .on_action(self.on_cycle_filter(state, cx))
            .on_action(self.on_collapse_log(state, cx))
            .on_action(self.on_close(state, cx))
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
    fn log_body(&self, log: &[SharedString], following: bool, cx: &App) -> AnyElement {
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
        LogView::new("jobs-panel-log", log.iter().cloned())
            .following(following)
            .track_scroll(&self.log_scroll)
            .into_any_element()
    }

    // ------------------------------------------------------------------ listeners

    fn on_move_down(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::MoveDown, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            panel.update(cx, |panel, _| {
                if panel.expanded.is_some() {
                    panel.following = false;
                    panel.log_offset = panel
                        .log_offset
                        .saturating_add(1)
                        .min(panel.log.len().saturating_sub(1));
                    scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Top);
                } else {
                    let len = panel.visible_ids(&jobs).len();
                    panel.cursor = (panel.cursor + 1).min(len.saturating_sub(1));
                    scroll.scroll_to_item(panel.cursor, ScrollStrategy::Center);
                }
            });
            notify(&state, cx);
        }
    }

    fn on_move_up(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::MoveUp, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, _| {
                if panel.expanded.is_some() {
                    panel.following = false;
                    panel.log_offset = panel.log_offset.saturating_sub(1);
                    scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Top);
                } else {
                    panel.cursor = panel.cursor.saturating_sub(1);
                    scroll.scroll_to_item(panel.cursor, ScrollStrategy::Center);
                }
            });
            notify(&state, cx);
        }
    }

    fn on_top(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::Top, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, _| {
                if panel.expanded.is_some() {
                    panel.following = false;
                    panel.log_offset = 0;
                } else {
                    panel.cursor = 0;
                }
                scroll.scroll_to_item(0, ScrollStrategy::Top);
            });
            notify(&state, cx);
        }
    }

    fn on_bottom(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::Bottom, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            panel.update(cx, |panel, _| {
                if panel.expanded.is_some() {
                    // `G` re-enables follow (§3.7), it does not merely scroll.
                    panel.following = true;
                    panel.log_offset = panel.log.len().saturating_sub(1);
                    log_scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Bottom);
                } else {
                    let len = panel.visible_ids(&jobs).len();
                    panel.cursor = len.saturating_sub(1);
                    scroll.scroll_to_item(panel.cursor, ScrollStrategy::Center);
                }
            });
            notify(&state, cx);
        }
    }

    fn on_cycle_filter(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::CycleFilter, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            panel.update(cx, |panel, _| {
                // One binding, two meanings: `f` follows inside an expanded log and cycles the
                // filter in the list (`docs/APP-CONTRACTS.md` §3).
                if panel.expanded.is_some() {
                    panel.following = !panel.following;
                } else {
                    panel.filter = panel.filter.next();
                    let len = panel.visible_ids(&jobs).len();
                    panel.clamp(len);
                }
            });
            notify(&state, cx);
        }
    }

    fn on_close(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::Close, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            let consumed = panel.update(cx, |panel, _| {
                if panel.confirming_cancel_all {
                    panel.confirming_cancel_all = false;
                    return true;
                }
                false
            });
            if consumed {
                cx.stop_propagation();
                notify(&state, cx);
                return;
            }
            // The shell closes the overlay; the panel forgets its transient state so the next
            // opening seeds its cursor again. gpui stops an action at the first bubble-phase
            // listener, and this one is the innermost, so `Shell::close_jobs` is only reached
            // by handing the action back explicitly.
            panel.update(cx, |panel, _| {
                panel.opened = false;
                panel.confirming_cancel_all = false;
            });
            cx.propagate();
        }
    }

    fn on_collapse_log(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::CollapseLog, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, _| {
                panel.collapse();
            });
            cx.stop_propagation();
            notify(&state, cx);
        }
    }

    fn on_toggle_log(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::ToggleLog, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let selected = panel.read_with(cx, |panel, _| {
                panel.selected(&jobs).map(|job| job.id.clone())
            });
            let Some(job) = selected else {
                return;
            };
            let already = panel.read_with(cx, |panel, _| panel.expanded.as_ref() == Some(&job));
            if already {
                panel.update(cx, |panel, _| {
                    panel.collapse();
                });
                notify(&state, cx);
                return;
            }
            panel.update(cx, |panel, _| {
                panel.collapse();
                panel.expanded = Some(job.clone());
                panel.following = true;
            });
            let task = spawn_tail(
                panel.clone(),
                state.clone(),
                bridge.clone(),
                log_scroll.clone(),
                job,
                cx,
            );
            panel.update(cx, |panel, _| panel.tail = Some(task));
            notify(&state, cx);
        }
    }

    fn on_cancel(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::CancelJob, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let selected = panel.read_with(cx, |panel, _| panel.selected(&jobs).cloned());
            let Some(job) = selected else {
                return;
            };
            if !job.cancellable || !jobs_panel::is_active(&job.status) {
                refuse(&state, "Job is not cancellable", cx);
                return;
            }
            bridge.send(RequestBody::CancelJob { job: job.id });
            notify(&state, cx);
        }
    }

    fn on_cancel_all(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::CancelAll, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let targets: Vec<JobId> = jobs
                .iter()
                .filter(|job| job.cancellable && jobs_panel::is_active(&job.status))
                .map(|job| job.id.clone())
                .collect();
            if targets.is_empty() {
                refuse(&state, "Nothing to cancel", cx);
                return;
            }
            let armed = panel.read_with(cx, |panel, _| panel.confirming_cancel_all);
            if !armed {
                panel.update(cx, |panel, _| panel.confirming_cancel_all = true);
                notify(&state, cx);
                return;
            }
            panel.update(cx, |panel, _| panel.confirming_cancel_all = false);
            for job in targets {
                bridge.send(RequestBody::CancelJob { job });
            }
            notify(&state, cx);
        }
    }

    fn on_retry(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::Retry, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let selected = panel.read_with(cx, |panel, _| panel.selected(&jobs).cloned());
            let Some(job) = selected else {
                return;
            };
            if !job.retryable {
                refuse(&state, "This job cannot be retried", cx);
                return;
            }
            // The retried job comes back as a fresh record, so the failed one stops owning the
            // sticky slot the moment the daemon answers.
            bridge.send(RequestBody::RetryJob { job: job.id });
            notify(&state, cx);
        }
    }

    fn on_copy_log_path(
        &self,
        state: &Entity<AppState>,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::CopyLogPath, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let selected = panel.read_with(cx, |panel, _| panel.selected(&jobs).cloned());
            let Some(job) = selected else {
                return;
            };
            cx.write_to_clipboard(ClipboardItem::new_string(job.log_path));
            state.update(cx, |state, cx| {
                state.toast_short(
                    "Log path copied",
                    Icon::ClipboardCheck,
                    std::time::Instant::now(),
                );
                cx.notify();
            });
        }
    }

    fn on_dismiss(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        _cx: &App,
    ) -> impl Fn(&jobs_actions::DismissFinished, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let gone: Vec<JobId> = jobs
                .iter()
                .filter(|job| is_dismissable(&job.status))
                .map(|job| job.id.clone())
                .collect();
            if gone.is_empty() {
                return;
            }
            panel.update(cx, |panel, _| {
                if panel
                    .expanded
                    .as_ref()
                    .is_some_and(|job| gone.contains(job))
                {
                    panel.collapse();
                }
                let len = jobs.len().saturating_sub(gone.len());
                panel.clamp(len);
            });
            bridge.send(RequestBody::DismissJobs { jobs: gone.clone() });
            state.update(cx, |state, cx| {
                if let Some(snapshot) = state.snapshot.as_mut() {
                    snapshot.jobs.retain(|job| !gone.contains(&job.id));
                }
                state.seen_failed.extend(gone.iter().cloned());
                state.sticky_error = None;
                cx.notify();
            });
        }
    }
}

/// The daemon's job list, or an empty one before the first snapshot lands.
fn snapshot_jobs(state: &Entity<AppState>, cx: &App) -> Vec<JobRecord> {
    state
        .read(cx)
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.jobs.clone())
        .unwrap_or_default()
}

/// Repaints the frame. The shell observes [`AppState`] and nothing else, so a panel-only change
/// has to say so through that entity.
fn notify(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |_, cx| cx.notify());
}

/// A refused action, with its reason (§2.7 "Action refused, with the reason").
fn refuse(state: &Entity<AppState>, reason: &str, cx: &mut App) {
    state.update(cx, |state, cx| {
        state.toast_short(reason.to_owned(), Icon::Info, std::time::Instant::now());
        cx.notify();
    });
}

/// Polls `TailJob` while the log stays expanded and following.
///
/// The loop stops on its own as soon as the panel collapses or expands another job, so
/// collapsing does not have to cancel anything for correctness — dropping the task is only an
/// optimisation.
fn spawn_tail(
    panel: Entity<PanelState>,
    state: Entity<AppState>,
    bridge: Bridge,
    log_scroll: UniformListScrollHandle,
    job: JobId,
    cx: &App,
) -> Task<()> {
    cx.spawn(async move |cx| {
        loop {
            let reply = bridge.request(RequestBody::TailJob {
                job: job.clone(),
                lines: LOG_TAIL_LINES,
            });
            match reply.recv().await {
                Ok(Ok(ResponseBody::JobLog(lines))) => {
                    let applied = cx.update(|cx| {
                        let still_open =
                            panel.read_with(cx, |panel, _| panel.expanded.as_ref() == Some(&job));
                        if !still_open {
                            return false;
                        }
                        panel.update(cx, |panel, _| {
                            panel.log = lines.into_iter().map(SharedString::from).collect();
                            if panel.following {
                                panel.log_offset = panel.log.len().saturating_sub(1);
                                log_scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Bottom);
                            }
                        });
                        notify(&state, cx);
                        true
                    });
                    if !applied {
                        return;
                    }
                }
                Ok(Ok(_)) => return,
                Ok(Err(error)) => {
                    report_tail_error(&panel, &state, &job, &error, cx).await;
                    return;
                }
                Err(_) => return,
            }
            cx.background_executor().timer(TAIL_INTERVAL).await;
        }
    })
}

/// Surfaces a tail failure the way §1.8 demands: sticky, never a toast.
async fn report_tail_error(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    job: &JobId,
    error: &ProtoError,
    cx: &mut gpui::AsyncApp,
) {
    let message = format!(
        "could not read the log of {}: {}",
        job.as_str(),
        error.message
    );
    cx.update(|cx| {
        panel.update(cx, |panel, _| {
            panel.collapse();
        });
        state.update(cx, |state, cx| {
            state.sticky_error = Some(StickyError {
                text: message,
                job: Some(job.clone()),
                retryable: false,
            });
            cx.notify();
        });
    });
}

/// Whether a job's status still allows `c` to do anything, for the footer's key hints.
#[must_use]
pub fn can_cancel(job: &JobRecord) -> bool {
    job.cancellable && jobs_panel::is_active(&job.status)
}

/// Whether `R` on this job would do anything (§3.7: retry a failed job).
#[must_use]
pub fn can_retry(job: &JobRecord) -> bool {
    job.retryable && matches!(job.status, JobStatus::Failed { .. } | JobStatus::Cancelled)
}

/// The row the elapsed column would print, exposed so tests can assert §3.7 without a window.
#[must_use]
pub fn elapsed_for(job: &JobRecord, now: i64) -> Option<String> {
    elapsed_label(job, now)
}

#[cfg(test)]
mod tests {
    use fleet_proto::job::JobKind;

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
        assert_eq!(panel.visible_ids(&jobs).len(), 1);
    }

    #[test]
    fn the_cursor_is_clamped_when_the_filter_shrinks_the_list() {
        let jobs = vec![
            job("job-a", JobStatus::Running, true, false),
            job("job-b", JobStatus::Running, true, false),
            job("job-c", JobStatus::Succeeded, false, false),
        ];
        let mut panel = panel(JobFilter::All);
        panel.cursor = 2;
        panel.filter = JobFilter::Running;
        panel.clamp(panel.visible_ids(&jobs).len());
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
    fn only_finished_rows_are_dismissable() {
        let jobs = [
            job("job-a", JobStatus::Running, true, false),
            job("job-b", JobStatus::Succeeded, false, false),
        ];
        assert!(!is_dismissable(&jobs[0].status));
        assert!(is_dismissable(&jobs[1].status));
    }

    #[test]
    fn collapsing_a_log_clears_it_and_reports_whether_it_did_anything() {
        let mut panel = panel(JobFilter::All);
        assert!(!panel.collapse(), "collapsing a collapsed panel is a no-op");
        panel.expanded = Some("job-a".parse().unwrap_or_else(|error| panic!("{error}")));
        panel.log = vec![SharedString::new_static("line")];
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

        assert!(can_retry(&job(
            "job-d",
            JobStatus::Failed {
                error: String::new()
            },
            false,
            true
        )));
        assert!(!can_retry(&job(
            "job-e",
            JobStatus::Failed {
                error: String::new()
            },
            false,
            false
        )));
        assert!(!can_retry(&job("job-f", JobStatus::Running, true, true)));
    }
}
