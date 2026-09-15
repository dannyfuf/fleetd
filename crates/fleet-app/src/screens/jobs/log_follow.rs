use super::*;

impl PanelState {
    /// Compare raw lines before allocating shared text or asking the Shell to repaint.
    pub(super) fn apply_tail(
        &mut self,
        lines: Vec<String>,
        scroll: &UniformListScrollHandle,
    ) -> bool {
        if !self.following {
            return false;
        }
        if self.log.len() == lines.len()
            && self
                .log
                .iter()
                .zip(&lines)
                .all(|(old, new)| old.as_ref() == new)
        {
            return false;
        }
        self.log = lines.into_iter().map(SharedString::from).collect();
        self.log_offset = self.log.len().saturating_sub(1);
        scroll.scroll_to_item(self.log_offset, ScrollStrategy::Bottom);
        true
    }
}

/// Whether the overlay is still showing this job's log; a follow loop stops the moment it is not.
fn still_following(
    panel: &gpui::WeakEntity<PanelState>,
    state: &gpui::WeakEntity<AppState>,
    job: &JobId,
    cx: &mut gpui::AsyncApp,
) -> Option<(Entity<PanelState>, Entity<AppState>)> {
    cx.update(|cx| {
        let (panel, state) = (panel.upgrade()?, state.upgrade()?);
        let panel_state = panel.read(cx);
        let following = matches!(state.read(cx).overlay, Some(Overlay::Jobs))
            && panel_state.following
            && panel_state.expanded.as_ref() == Some(job);
        following.then_some((panel, state))
    })
}

pub(super) fn start_tail(
    panel: Entity<PanelState>,
    state: Entity<AppState>,
    requests: actions::JobsRequests,
    log_scroll: UniformListScrollHandle,
    job: JobId,
    cx: &mut App,
) {
    let task = spawn_tail_with_requests(panel.clone(), state, requests, log_scroll, job, cx);
    panel.update(cx, |panel, _| panel.tail = Some(task));
}

fn spawn_tail_with_requests(
    panel: Entity<PanelState>,
    state: Entity<AppState>,
    requests: actions::JobsRequests,
    log_scroll: UniformListScrollHandle,
    job: JobId,
    cx: &App,
) -> Task<()> {
    let panel = panel.downgrade();
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        loop {
            if still_following(&panel, &state, &job, cx).is_none() {
                return;
            }
            let reply = requests.request(RequestBody::TailJob {
                job: job.clone(),
                lines: LOG_TAIL_LINES,
            });
            match reply.recv().await {
                Ok(Ok(ResponseBody::JobLog(lines))) => {
                    let Some((panel, state)) = still_following(&panel, &state, &job, cx) else {
                        return;
                    };
                    cx.update(|cx| {
                        if panel.update(cx, |panel, _| panel.apply_tail(lines, &log_scroll)) {
                            notify(&state, cx);
                        }
                    });
                }
                Ok(Err(error)) => {
                    let Some((panel, state)) = still_following(&panel, &state, &job, cx) else {
                        return;
                    };
                    cx.update(|cx| {
                        panel.update(cx, |panel, _| {
                            panel.collapse();
                        });
                        state.update(cx, |state, cx| {
                            state.sticky_error = Some(StickyError {
                                text: format!(
                                    "could not read the log of {}: {}",
                                    job.as_str(),
                                    error.message
                                ),
                                job: Some(job.clone()),
                                retryable: false,
                            });
                            cx.notify();
                        });
                    });
                    return;
                }
                Ok(Ok(_)) | Err(_) => return,
            }
            // The bridge holds the `InFlight` claim until this receiver is gone; `await idle`
            // needs a zero edge between polls, before the next tail interval begins.
            drop(reply);
            cx.background_executor().timer(TAIL_INTERVAL).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        },
    };

    use async_channel::Sender;
    use fleet_proto::error::ProtoError;
    use gpui::{BackgroundExecutor, TestAppContext};

    use super::*;
    use crate::state::{IdleWake, SettleCounter};

    struct ActionHarness;

    impl gpui::Render for ActionHarness {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div()
        }
    }

    type TailReply = Sender<Result<ResponseBody, ProtoError>>;

    #[derive(Clone)]
    struct TailHarness {
        pending: Arc<Mutex<VecDeque<TailReply>>>,
        in_flight: Arc<AtomicU32>,
    }

    impl TailHarness {
        fn new() -> Self {
            Self {
                pending: Arc::new(Mutex::new(VecDeque::new())),
                in_flight: Arc::new(AtomicU32::new(0)),
            }
        }

        fn requests(&self, executor: BackgroundExecutor) -> actions::JobsRequests {
            let pending = Arc::clone(&self.pending);
            let in_flight = Arc::clone(&self.in_flight);
            actions::JobsRequests::from_fn(Arc::new(move |body| {
                assert!(matches!(body, RequestBody::TailJob { .. }));
                let (reply, response) = async_channel::bounded(1);
                in_flight.fetch_add(1, Ordering::AcqRel);
                let held_reply = reply.clone();
                let in_flight = Arc::clone(&in_flight);
                executor
                    .spawn(async move {
                        held_reply.closed().await;
                        in_flight.fetch_sub(1, Ordering::AcqRel);
                    })
                    .detach();
                pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push_back(reply);
                response
            }))
        }

        fn respond(&self, response: ResponseBody) {
            let reply = self
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_else(|| panic!("expected one pending tail request"));
            reply
                .try_send(Ok(response))
                .unwrap_or_else(|_| panic!("tail reply receiver should still be live"));
        }

        fn pending(&self) -> usize {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
        }
    }

    fn following_state(
        harness: &TailHarness,
        job: &JobId,
        cx: &mut TestAppContext,
    ) -> (Entity<AppState>, Entity<PanelState>) {
        let in_flight = Arc::clone(&harness.in_flight);
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet-jobs-tail", std::time::Instant::now());
            state.overlay = Some(Overlay::Jobs);
            state.harness.attach_bridge(
                in_flight,
                Arc::new(SettleCounter::default()),
                IdleWake::new(|| {}),
            );
            state
        });
        let job = job.clone();
        let panel = cx.new(|_| PanelState {
            expanded: Some(job),
            following: true,
            ..PanelState::default()
        });
        (state, panel)
    }

    #[gpui::test]
    fn completed_tail_is_idle_between_polls(cx: &mut TestAppContext) {
        let harness = TailHarness::new();
        let job: JobId = "job-a"
            .parse()
            .unwrap_or_else(|error| panic!("job id: {error}"));
        let (state, panel) = following_state(&harness, &job, cx);
        let requests = harness.requests(cx.executor());
        let tail = cx.update(|cx| {
            spawn_tail_with_requests(
                panel.clone(),
                state.clone(),
                requests,
                UniformListScrollHandle::new(),
                job,
                cx,
            )
        });

        cx.run_until_parked();
        assert_eq!(harness.pending(), 1);
        cx.read(|cx| {
            assert_eq!(state.read(cx).harness_snapshot().idle.in_flight_requests, 1);
        });

        harness.respond(ResponseBody::JobLog(vec!["complete".to_owned()]));
        cx.run_until_parked();
        cx.executor().advance_clock(TAIL_INTERVAL / 2);
        cx.run_until_parked();

        assert_eq!(
            harness.pending(),
            0,
            "the next poll interval has not elapsed"
        );
        cx.read(|cx| {
            assert_eq!(state.read(cx).harness_snapshot().idle.in_flight_requests, 0);
        });
        drop(tail);
    }

    #[gpui::test]
    fn paused_follow_issues_no_further_tail_request(cx: &mut TestAppContext) {
        let harness = TailHarness::new();
        let job: JobId = "job-a"
            .parse()
            .unwrap_or_else(|error| panic!("job id: {error}"));
        let (state, panel) = following_state(&harness, &job, cx);
        let requests = harness.requests(cx.executor());
        let tail = cx.update(|cx| {
            spawn_tail_with_requests(
                panel.clone(),
                state.clone(),
                requests,
                UniformListScrollHandle::new(),
                job,
                cx,
            )
        });

        cx.run_until_parked();
        assert_eq!(harness.pending(), 1);
        harness.respond(ResponseBody::JobLog(vec!["complete".to_owned()]));
        cx.run_until_parked();
        panel.update(cx, |panel, _| panel.following = false);

        cx.executor().advance_clock(TAIL_INTERVAL);
        cx.run_until_parked();

        assert_eq!(harness.pending(), 0);
        drop(tail);
    }

    #[gpui::test]
    fn follow_pause_resume_issues_a_new_tail_request(cx: &mut TestAppContext) {
        let harness = TailHarness::new();
        let job: JobId = "job-a"
            .parse()
            .unwrap_or_else(|error| panic!("job id: {error}"));
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet-jobs-tail-resume", std::time::Instant::now());
            state.overlay = Some(Overlay::Jobs);
            state
        });
        let jobs = cx.update(JobsPanel::new);
        jobs.state.update(cx, |panel, _| {
            panel.expanded = Some(job.clone());
            panel.following = true;
        });
        let requests = harness.requests(cx.executor());
        cx.update(|cx| {
            start_tail(
                jobs.state.clone(),
                state.clone(),
                requests.clone(),
                jobs.log_scroll.clone(),
                job,
                cx,
            );
        });
        let handler = jobs.on_cycle_filter(&state, requests);
        let window = cx.add_window(|_, _| ActionHarness);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);

        visual.run_until_parked();
        assert_eq!(harness.pending(), 1);
        harness.respond(ResponseBody::JobLog(vec!["complete".to_owned()]));
        visual.run_until_parked();

        window
            .update(&mut visual, |_, window, cx| {
                handler(&jobs_actions::CycleFilter, window, cx)
            })
            .unwrap_or_else(|error| panic!("pause job-log follow: {error}"));
        visual.executor().advance_clock(TAIL_INTERVAL);
        visual.run_until_parked();
        assert_eq!(harness.pending(), 0);

        window
            .update(&mut visual, |_, window, cx| {
                handler(&jobs_actions::CycleFilter, window, cx)
            })
            .unwrap_or_else(|error| panic!("resume job-log follow: {error}"));
        visual.run_until_parked();

        assert_eq!(harness.pending(), 1);
        jobs.state.update(&mut visual, |panel, _| panel.collapse());
    }
}
