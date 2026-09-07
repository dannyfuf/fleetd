use super::*;

/// The job the cursor is on, or `None` when the filtered list is empty.
fn selected_job(
    panel: &Entity<PanelState>,
    state: &Entity<AppState>,
    cx: &App,
) -> Option<JobRecord> {
    panel.read(cx).selected(snapshot_jobs(state, cx)).cloned()
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
        bridge: &Bridge,
    ) -> impl Fn(&jobs_actions::CancelJob, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let Some(job) = selected_job(&panel, &state, cx) else {
                return;
            };
            if !can_cancel(&job) {
                refuse(&state, "Job is not cancellable", cx);
                return;
            }
            bridge.send(RequestBody::CancelJob { job: job.id });
            notify(&state, cx);
        }
    }

    pub(super) fn on_cancel_all(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
    ) -> impl Fn(&jobs_actions::CancelAll, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let jobs = snapshot_jobs(&state, cx);
            let targets: Vec<JobId> = jobs
                .iter()
                .filter(|job| can_cancel(job))
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

    pub(super) fn on_retry(
        &self,
        state: &Entity<AppState>,
        bridge: &Bridge,
    ) -> impl Fn(&jobs_actions::Retry, &mut Window, &mut App) + 'static {
        let panel = self.state.clone();
        let state = state.clone();
        let bridge = bridge.clone();
        move |_, _, cx| {
            let Some(job) = selected_job(&panel, &state, cx) else {
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
        bridge: &Bridge,
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
            let total = jobs.len();
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
                let len = total.saturating_sub(gone.len());
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
