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
        let following = matches!(state.read(cx).overlay, Some(Overlay::Jobs))
            && panel.read(cx).expanded.as_ref() == Some(job);
        following.then_some((panel, state))
    })
}

pub(super) fn spawn_tail(
    panel: Entity<PanelState>,
    state: Entity<AppState>,
    bridge: Bridge,
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
            let reply = bridge.request(RequestBody::TailJob {
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
            cx.background_executor().timer(TAIL_INTERVAL).await;
        }
    })
}
