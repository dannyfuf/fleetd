//! Jobs panel state, keyboard actions, and owned log following.

use std::{rc::Rc, sync::Arc, time::Duration};

use fleet_core::ids::JobId;
use fleet_proto::{job::JobRecord, request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{
    ActiveTheme, Icon, LOG_TAIL_LINES, LogCommand, LogView, Sheet, Tone, prelude::*,
};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, FocusHandle, ListAlignment, ListState, ScrollStrategy,
    SharedString, Subscription, Task, UniformListScrollHandle, Window, div,
};

use crate::{
    actions::jobs as jobs_actions,
    bridge::Bridge,
    presentation::{is_active, is_dismissable, now_unix},
    state::{AppState, Overlay, StickyError},
    views::jobs_panel::{self, JobFilter},
};

mod actions;
mod log_follow;
mod presentation;
#[cfg(test)]
mod tests;
use log_follow::spawn_tail;

/// How often an expanded log re-asks the daemon for its tail while following.
///
/// The daemon exposes the log as a `TailJob` request rather than a stream, so "follow" is a
/// poll. A quarter of a second is under the 400 ms the spec already spends debouncing an
/// inspect, and it costs one small request per tick on exactly one job.
pub(crate) const TAIL_INTERVAL: Duration = Duration::from_millis(250);

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
    pub log: Arc<[SharedString]>,
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
    prepared: presentation::PreparedJobs,
}

impl PanelState {
    /// How many rows the filter leaves for the cursor to move over.
    fn visible_len(&self, jobs: &[JobRecord]) -> usize {
        jobs.iter().filter(|job| self.filter.matches(job)).count()
    }

    /// The job under the cursor.
    #[must_use]
    pub fn selected<'a>(&self, jobs: &'a [JobRecord]) -> Option<&'a JobRecord> {
        jobs.iter()
            .filter(|job| self.filter.matches(job))
            .nth(self.cursor)
    }

    /// The job keyboard actions address. Once a log is expanded, its stable identity wins over
    /// the mutable list cursor until the log is collapsed.
    #[must_use]
    fn action_job<'a>(&self, jobs: &'a [JobRecord]) -> Option<&'a JobRecord> {
        if let Some(expanded) = &self.expanded {
            return jobs.iter().find(|job| &job.id == expanded);
        }
        self.selected(jobs)
    }

    /// Keeps the cursor inside the list after the data or the filter changed.
    pub fn clamp(&mut self, len: usize) {
        self.cursor = self.cursor.min(len.saturating_sub(1));
    }

    /// Puts the cursor on `job` when it is visible, which is what `!` promises: the sticky
    /// error slot opens this panel *on the failure it names* (§1.8).
    pub fn focus_job(&mut self, jobs: &[JobRecord], job: &JobId) -> bool {
        match jobs
            .iter()
            .filter(|job| self.filter.matches(job))
            .position(|record| &record.id == job)
        {
            Some(index) => {
                self.cursor = index;
                true
            }
            None => false,
        }
    }

    fn close(&mut self) {
        self.collapse();
        self.opened = false;
        self.confirming_cancel_all = false;
    }

    /// Collapses an expanded log and stops its follow loop.
    pub fn collapse(&mut self) -> bool {
        self.tail = None;
        self.log = Arc::default();
        self.log_offset = 0;
        self.expanded.take().is_some()
    }

    fn apply_log_command(&mut self, command: LogCommand) {
        match command {
            LogCommand::ToggleFollow => self.following = !self.following,
            LogCommand::Follow => {
                self.following = true;
                self.log_offset = self.log.len().saturating_sub(1);
            }
            LogCommand::ScrollTo(top) => {
                self.following = false;
                self.log_offset = top.min(self.log.len().saturating_sub(1));
            }
        }
    }
}

/// The jobs panel.
pub struct JobsPanel {
    state: Entity<PanelState>,
    list_scroll: ListState,
    log_scroll: UniformListScrollHandle,
    observation: Option<Subscription>,
    home: Option<std::path::PathBuf>,
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
            list_scroll: ListState::new(0, ListAlignment::Top, gpui::px(0.0)),
            log_scroll: UniformListScrollHandle::new(),
            observation: None,
            home: crate::presentation::home_dir(),
        }
    }

    /// Observe visibility even when the Shell no longer renders this panel.
    pub fn bind(&mut self, state: &Entity<AppState>, cx: &mut App) {
        if self.observation.is_some() {
            return;
        }
        let panel = self.state.downgrade();
        let scroll = self.list_scroll.clone();
        let home = self.home.clone();
        self.observation = Some(cx.observe(state, move |state, cx| {
            let Some(panel) = panel.upgrade() else {
                return;
            };
            presentation::synchronize(&panel, &state, &scroll, home.as_deref(), cx);
        }));
        let panel = self.state.downgrade();
        let state = state.downgrade();
        let scroll = self.list_scroll.clone();
        let home = self.home.clone();
        cx.defer(move |cx| {
            if let (Some(panel), Some(state)) = (panel.upgrade(), state.upgrade()) {
                presentation::synchronize(&panel, &state, &scroll, home.as_deref(), cx);
            }
        });
    }
}

/// The daemon's job list, or an empty one before the first snapshot lands.
fn snapshot_jobs<'a>(state: &Entity<AppState>, cx: &'a App) -> &'a [JobRecord] {
    state
        .read(cx)
        .snapshot
        .as_ref()
        .map_or(&[], |snapshot| &snapshot.jobs)
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

/// Whether a job's status still allows `c` to do anything, for the footer's key hints.
#[must_use]
pub(crate) fn can_cancel(job: &JobRecord) -> bool {
    job.cancellable && is_active(&job.status)
}
