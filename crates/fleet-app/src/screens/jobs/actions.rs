use super::*;
use async_channel::{Receiver, RecvError};
use fleet_proto::error::{ErrorKind, ProtoError};

type MutationReply = Result<Result<ResponseBody, ProtoError>, RecvError>;

#[derive(Clone)]
pub(super) struct JobsRequests(
    Arc<dyn Fn(RequestBody) -> Receiver<Result<ResponseBody, ProtoError>> + Send + Sync>,
);

impl JobsRequests {
    pub(super) fn bridge(bridge: Bridge) -> Self {
        Self(Arc::new(move |body| bridge.request(body)))
    }

    #[cfg(test)]
    pub(super) fn from_fn(
        request: Arc<
            dyn Fn(RequestBody) -> Receiver<Result<ResponseBody, ProtoError>> + Send + Sync,
        >,
    ) -> Self {
        Self(request)
    }

    fn request(&self, body: RequestBody) -> Receiver<Result<ResponseBody, ProtoError>> {
        (self.0)(body)
    }
}

#[derive(Clone, Copy)]
pub(super) enum ExpectedMutation<'a> {
    Cancel(&'a JobId),
    Retry,
    Dismiss,
}

pub(super) fn mutation_failure(
    reply: MutationReply,
    expected: ExpectedMutation<'_>,
    operation: &str,
) -> Option<String> {
    match reply {
        Ok(Err(error))
            if error.kind == ErrorKind::NotFound
                && matches!(expected, ExpectedMutation::Dismiss) =>
        {
            None
        }
        Ok(Err(error)) => Some(error.message),
        Err(_) => Some(format!("could not {operation}: daemon reply was lost")),
        Ok(Ok(ResponseBody::JobCancelled(actual))) if matches!(expected, ExpectedMutation::Cancel(expected) if actual == *expected) => {
            None
        }
        Ok(Ok(ResponseBody::Job(_))) if matches!(expected, ExpectedMutation::Retry) => None,
        Ok(Ok(ResponseBody::Ack)) if matches!(expected, ExpectedMutation::Dismiss) => None,
        Ok(Ok(_)) => Some(format!(
            "could not {operation}: daemon returned an unexpected response"
        )),
    }
}

pub(super) fn record_mutation_failure(
    app: &mut AppState,
    text: String,
    job: Option<JobId>,
    retryable: bool,
) {
    app.sticky_error = Some(StickyError {
        text,
        job,
        retryable,
    });
}

fn await_job_mutation(
    reply: async_channel::Receiver<Result<ResponseBody, ProtoError>>,
    expected_job: JobId,
    retryable: bool,
    retry: bool,
    state: Entity<AppState>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        let expected = if retry {
            ExpectedMutation::Retry
        } else {
            ExpectedMutation::Cancel(&expected_job)
        };
        if let Some(error) = mutation_failure(
            answer,
            expected,
            if retry { "retry job" } else { "cancel job" },
        ) {
            state.update(cx, |app, cx| {
                record_mutation_failure(app, error, None, retryable);
                cx.notify();
            });
        }
    })
    .detach();
}

pub(super) fn acknowledge_dismissal(app: &mut AppState, dismissed: &[JobId]) {
    if let Some(snapshot) = app.snapshot.as_mut() {
        snapshot.jobs.retain(|job| !dismissed.contains(&job.id));
    }
    app.seen_failed.extend(dismissed.iter().cloned());
    if app
        .sticky_error
        .as_ref()
        .and_then(|error| error.job.as_ref())
        .is_some_and(|job| dismissed.contains(job))
    {
        app.sticky_error = None;
    }
}

pub(super) fn reconcile_dismissed_panel(
    panel: &mut PanelState,
    jobs: &[JobRecord],
    dismissed: &[JobId],
) {
    if panel
        .expanded
        .as_ref()
        .is_some_and(|job| dismissed.contains(job))
    {
        panel.collapse();
    }
    panel.clamp(panel.visible_len(jobs));
}

/// The job the cursor is on, or `None` when the filtered list is empty.
fn selected_job(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    cx: &App,
) -> Option<JobRecord> {
    panel.read(cx).action_job(snapshot_jobs(state, cx)).cloned()
}

pub(super) fn request_cancel(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    requests: &JobsRequests,
    cx: &mut App,
) {
    let Some(job) = selected_job(panel, state, cx) else {
        return;
    };
    if !can_cancel(&job) {
        refuse(state, "Job is not cancellable", cx);
        return;
    }
    let expected = job.id.clone();
    let reply = requests.request(RequestBody::CancelJob { job: job.id });
    await_job_mutation(reply, expected, job.retryable, false, state.clone(), cx);
    notify(state, cx);
}

pub(super) fn request_cancel_all(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    requests: &JobsRequests,
    cx: &mut App,
) {
    let jobs = snapshot_jobs(state, cx);
    let targets: Vec<JobId> = jobs
        .iter()
        .filter(|job| can_cancel(job))
        .map(|job| job.id.clone())
        .collect();
    if targets.is_empty() {
        refuse(state, "Nothing to cancel", cx);
        return;
    }
    let armed = panel.read_with(cx, |panel, _| panel.confirming_cancel_all);
    if !armed {
        panel.update(cx, |panel, _| panel.confirming_cancel_all = true);
        notify(state, cx);
        return;
    }
    panel.update(cx, |panel, _| panel.confirming_cancel_all = false);
    for job in targets {
        let reply = requests.request(RequestBody::CancelJob { job: job.clone() });
        await_job_mutation(reply, job, false, false, state.clone(), cx);
    }
    notify(state, cx);
}

pub(super) fn request_retry(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    requests: &JobsRequests,
    cx: &mut App,
) {
    let Some(job) = selected_job(panel, state, cx) else {
        return;
    };
    if !job.retryable {
        refuse(state, "This job cannot be retried", cx);
        return;
    }
    let expected = job.id.clone();
    let reply = requests.request(RequestBody::RetryJob { job: job.id });
    await_job_mutation(reply, expected, job.retryable, true, state.clone(), cx);
    notify(state, cx);
}

pub(super) fn request_dismissal(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    requests: &JobsRequests,
    cx: &mut App,
) {
    let gone: Vec<JobId> = snapshot_jobs(state, cx)
        .iter()
        .filter(|job| is_dismissable(&job.status))
        .map(|job| job.id.clone())
        .collect();
    if gone.is_empty() {
        return;
    }
    let reply = requests.request(RequestBody::DismissJobs { jobs: gone.clone() });
    let panel = panel.clone();
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            if let Some(error) = mutation_failure(answer, ExpectedMutation::Dismiss, "dismiss jobs")
            {
                state.update(cx, |app, cx| {
                    record_mutation_failure(app, error, None, false);
                    cx.notify();
                });
                return;
            }
            state.update(cx, |app, cx| {
                acknowledge_dismissal(app, &gone);
                cx.notify();
            });
            panel.update(cx, |panel, cx| {
                reconcile_dismissed_panel(panel, snapshot_jobs(&state, cx), &gone);
            });
        });
    })
    .detach();
}

impl JobsPanel {
    pub(super) fn on_move_down(
        &self,
        state: &Entity<AppState>,
    ) -> impl Fn(&jobs_actions::MoveDown, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, cx| {
                let jobs = snapshot_jobs(&state, cx);
                if panel.expanded.is_some() {
                    panel.following = false;
                    panel.log_offset = panel
                        .log_offset
                        .saturating_add(1)
                        .min(panel.log.len().saturating_sub(1));
                    log_scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Top);
                } else {
                    let len = panel.visible_len(jobs);
                    panel.cursor = (panel.cursor + 1).min(len.saturating_sub(1));
                    scroll.scroll_to_reveal_item(panel.cursor);
                }
            });
            notify(&state, cx);
        }
    }

    pub(super) fn on_move_up(
        &self,
        state: &Entity<AppState>,
    ) -> impl Fn(&jobs_actions::MoveUp, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, _| {
                if panel.expanded.is_some() {
                    panel.following = false;
                    panel.log_offset = panel.log_offset.saturating_sub(1);
                    log_scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Top);
                } else {
                    panel.cursor = panel.cursor.saturating_sub(1);
                    scroll.scroll_to_reveal_item(panel.cursor);
                }
            });
            notify(&state, cx);
        }
    }

    pub(super) fn on_top(
        &self,
        state: &Entity<AppState>,
    ) -> impl Fn(&jobs_actions::Top, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, _| {
                if panel.expanded.is_some() {
                    panel.following = false;
                    panel.log_offset = 0;
                    log_scroll.scroll_to_item(0, ScrollStrategy::Top);
                } else {
                    panel.cursor = 0;
                    scroll.scroll_to_reveal_item(0);
                }
            });
            notify(&state, cx);
        }
    }

    pub(super) fn on_bottom(
        &self,
        state: &Entity<AppState>,
    ) -> impl Fn(&jobs_actions::Bottom, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let scroll = self.list_scroll.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, cx| {
                let jobs = snapshot_jobs(&state, cx);
                if panel.expanded.is_some() {
                    // `G` re-enables follow (§3.7), it does not merely scroll.
                    panel.following = true;
                    panel.log_offset = panel.log.len().saturating_sub(1);
                    log_scroll.scroll_to_item(panel.log_offset, ScrollStrategy::Bottom);
                } else {
                    panel.cursor = panel.visible_len(jobs).saturating_sub(1);
                    scroll.scroll_to_reveal_item(panel.cursor);
                }
            });
            notify(&state, cx);
        }
    }

    pub(super) fn on_cycle_filter(
        &self,
        state: &Entity<AppState>,
    ) -> impl Fn(&jobs_actions::CycleFilter, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            panel.update(cx, |panel, cx| {
                let jobs = snapshot_jobs(&state, cx);
                // One binding, two meanings: `f` follows inside an expanded log and cycles the
                // filter in the list (`docs/APP-CONTRACTS.md` §3).
                if panel.expanded.is_some() {
                    panel.following = !panel.following;
                } else {
                    panel.filter = panel.filter.next();
                    let len = panel.visible_len(jobs);
                    panel.clamp(len);
                }
            });
            notify(&state, cx);
        }
    }

    pub(super) fn on_close(
        &self,
        state: &Entity<AppState>,
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
                panel.close();
            });
            cx.propagate();
        }
    }

    pub(super) fn on_collapse_log(
        &self,
        state: &Entity<AppState>,
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

    pub(super) fn on_toggle_log(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
    ) -> impl Fn(&jobs_actions::ToggleLog, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        let log_scroll = self.log_scroll.clone();
        move |_, _, cx| {
            let Some(job) = selected_job(&panel, &state, cx).map(|job| job.id) else {
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

    pub(super) fn on_cancel(
        &self,
        state: &Entity<AppState>,
        requests: JobsRequests,
    ) -> impl Fn(&jobs_actions::CancelJob, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            request_cancel(&panel, &state, &requests, cx);
        }
    }

    pub(super) fn on_cancel_all(
        &self,
        state: &Entity<AppState>,
        requests: JobsRequests,
    ) -> impl Fn(&jobs_actions::CancelAll, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            request_cancel_all(&panel, &state, &requests, cx);
        }
    }

    pub(super) fn on_retry(
        &self,
        state: &Entity<AppState>,
        requests: JobsRequests,
    ) -> impl Fn(&jobs_actions::Retry, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            request_retry(&panel, &state, &requests, cx);
        }
    }

    pub(super) fn on_copy_log_path(
        &self,
        state: &Entity<AppState>,
    ) -> impl Fn(&jobs_actions::CopyLogPath, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            let Some(job) = selected_job(&panel, &state, cx) else {
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

    pub(super) fn on_dismiss(
        &self,
        state: &Entity<AppState>,
        requests: JobsRequests,
    ) -> impl Fn(&jobs_actions::DismissFinished, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        move |_, _, cx| {
            request_dismissal(&panel, &state, &requests, cx);
        }
    }
}
