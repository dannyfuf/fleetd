//! The root view: focus routing, the key-context chain, every action handler and the frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fleet_git::{
    CommandEvent, CommandOutcome, CommitOptions, ConflictChoice, DiffSide, FetchRequest,
    MoveDirection, OperationState, PatchAction, PatchSelection, PullRequest, PushRequest, Ref,
    ResetMode, StashOptions,
};
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::{AppFrame, Toast};
use gpui::{
    AnyElement, App, Context, Div, FocusHandle, Focusable, KeyDownEvent, Render, Task,
    UniformListScrollHandle, Window, div,
};

use crate::actions::{
    branches, commitfiles, commits, confirm, conflict, diff as diff_actions, files, global, help,
    list, menu, prompt, remotes, staging, stash, subcommits, tags,
};
use crate::bridge::{GitBridge, GitEvent, GitRequest, Mutation};
use crate::keymap::ROOT_CONTEXT;
use crate::state::{
    BranchTab, CommitTab, Confirm, ConfirmOutcome, GitUiState, MainContent, Menu, MenuAction,
    MenuItem, Overlay, PanelId, Prompt, PromptKind, Staging, has_unstaged,
};
use crate::views::diff_model::{DiffModel, DiffViewMode, ModelKey};

/// How long one background syntax pass may take before it is worth a log line.
const SYNTAX_BUDGET: Duration = Duration::from_millis(250);

/// One frame at 60 Hz. Foreground diff work that overruns it is logged.
const FRAME_BUDGET: Duration = Duration::from_millis(16);

/// The three diff models a frame can need: the main panel, the second half of the staging split,
/// and the patch under a drill-down list.
pub(crate) const SLOT_MAIN: &str = "main";
/// The staged/unstaged half the main panel is not showing.
pub(crate) const SLOT_SECONDARY: &str = "secondary";
/// The patch under a sub-commits or commit-files list.
pub(crate) const SLOT_PATCH: &str = "patch";

/// How often the UI ticks (toast expiry and the refresh fallback).
const TICK: Duration = Duration::from_millis(500);
/// The periodic snapshot fallback while the window is focused.
const REFRESH_ACTIVE: Duration = Duration::from_secs(2);
/// The periodic snapshot fallback while the window is not focused.
const REFRESH_IDLE: Duration = Duration::from_secs(5);
/// How long a toast stays up.
const TOAST_DWELL: Duration = Duration::from_millis(3_200);
/// How many commits a sub-commits drill-down loads.
const SUB_COMMIT_LIMIT: usize = 300;

/// The root view.
pub struct Lazygit {
    /// All UI state.
    pub state: GitUiState,
    /// The git worker.
    pub bridge: GitBridge,
    /// Focused while a panel owns the keyboard.
    focus: FocusHandle,
    /// Focused while an overlay owns the keyboard.
    overlay_focus: FocusHandle,
    /// One scroll handle per list, owned by the view and passed by reference each render.
    pub scroll_files: UniformListScrollHandle,
    /// Branches list scroll.
    pub scroll_branches: UniformListScrollHandle,
    /// Remotes list scroll.
    pub scroll_remotes: UniformListScrollHandle,
    /// Remote-branches list scroll.
    pub scroll_remote_branches: UniformListScrollHandle,
    /// Tags list scroll.
    pub scroll_tags: UniformListScrollHandle,
    /// Commits list scroll.
    pub scroll_commits: UniformListScrollHandle,
    /// Reflog list scroll.
    pub scroll_reflog: UniformListScrollHandle,
    /// Stash list scroll.
    pub scroll_stashes: UniformListScrollHandle,
    /// Main panel scroll.
    pub scroll_main: UniformListScrollHandle,
    /// Secondary panel scroll.
    pub scroll_secondary: UniformListScrollHandle,
    /// One cached [`DiffModel`] per rendered diff, keyed by slot.
    ///
    /// `RefCell` because the model is built and read from `render`, which only has `&self` by
    /// the time it reaches the panel helpers. Rebuilt only when the underlying `Arc<Diff>`, the
    /// view mode or the context width changes — see [`ModelKey`].
    models: RefCell<HashMap<&'static str, (ModelKey, Rc<DiffModel>)>>,
    /// The in-flight background syntax pass per slot. Dropping the task cancels it.
    syntax_tasks: RefCell<HashMap<&'static str, Task<()>>>,
    /// How many whole rows a flexible side pane can show, measured each frame.
    pub(crate) rows_side: usize,
    /// How many whole rows the Stash pane can show while it keeps its fixed height.
    pub(crate) rows_stash: usize,
    /// How many rows the main panel can show.
    pub(crate) rows_main: usize,
    /// How many characters fit across the side column, for the ellipsis budgets.
    pub(crate) side_ch: usize,
    /// How wide the main panel's payload column is, for the horizontal-scroll clamp.
    pub(crate) main_px_w: f32,
    /// Whether the window is active, which sets the refresh-fallback interval.
    window_active: bool,
    /// When the last periodic refresh went out.
    last_refresh: Instant,
    _tasks: Vec<Task<()>>,
}

impl Lazygit {
    /// Builds the view, starts the git bridge and asks for the first snapshot.
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = GitBridge::start(path.clone());
        let tasks = vec![Self::spawn_event_loop(&bridge, cx), Self::spawn_ticker(cx)];
        bridge.send(GitRequest::Snapshot);
        Self {
            state: GitUiState::new(path),
            bridge,
            focus: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            scroll_files: UniformListScrollHandle::new(),
            scroll_branches: UniformListScrollHandle::new(),
            scroll_remotes: UniformListScrollHandle::new(),
            scroll_remote_branches: UniformListScrollHandle::new(),
            scroll_tags: UniformListScrollHandle::new(),
            scroll_commits: UniformListScrollHandle::new(),
            scroll_reflog: UniformListScrollHandle::new(),
            scroll_stashes: UniformListScrollHandle::new(),
            scroll_main: UniformListScrollHandle::new(),
            scroll_secondary: UniformListScrollHandle::new(),
            models: RefCell::new(HashMap::new()),
            syntax_tasks: RefCell::new(HashMap::new()),
            main_px_w: 600.0,
            rows_side: fleet_ui_kit::DEFAULT_PAGE * 2,
            rows_stash: 2,
            rows_main: fleet_ui_kit::DEFAULT_PAGE * 2,
            side_ch: 40,
            window_active: true,
            last_refresh: Instant::now(),
            _tasks: tasks,
        }
    }

    /// Drains the bridge's event stream into the state. Stored, never detached: dropping the task
    /// would cancel the future.
    fn spawn_event_loop(bridge: &GitBridge, cx: &mut Context<Self>) -> Task<()> {
        let events = bridge.events();
        cx.spawn(async move |root, cx| {
            while let Ok(event) = events.recv().await {
                let updated = root.update(cx, |root, cx| {
                    root.apply_event(event, Instant::now());
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
        })
    }

    /// Expires toasts and issues the periodic snapshot fallback.
    fn spawn_ticker(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |root, cx| {
            loop {
                let timer = cx.background_executor().timer(TICK);
                timer.await;
                let updated = root.update(cx, |root, cx| {
                    let now = Instant::now();
                    let mut dirty = root.state.expire_toasts(now);
                    let interval = if root.window_active {
                        REFRESH_ACTIVE
                    } else {
                        REFRESH_IDLE
                    };
                    if !root.state.refreshing
                        && root.state.fatal.is_none()
                        && now.duration_since(root.last_refresh) >= interval
                    {
                        root.last_refresh = now;
                        root.state.refreshing = true;
                        root.bridge.send(GitRequest::Snapshot);
                        dirty = true;
                    }
                    if dirty {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        })
    }

    // ---------------------------------------------------------------- bridge events

    fn apply_event(&mut self, event: GitEvent, now: Instant) {
        match event {
            GitEvent::Opened { root } => {
                self.state.root = root;
            }
            GitEvent::OpenFailed { message } => {
                self.state.fatal = Some(message);
                self.state.refreshing = false;
            }
            GitEvent::Snapshot(snapshot) => {
                self.last_refresh = now;
                let requests = self.state.apply_snapshot(snapshot);
                for request in requests {
                    self.send(request);
                }
                self.sync_main_len();
            }
            GitEvent::FileDiff {
                path,
                unstaged,
                staged,
            } => {
                self.state.finish_read();
                if self.state.apply_file_diff(&path, unstaged, staged) {
                    self.retarget_staging_side();
                    self.sync_main_len();
                    // The diff usually arrives after `enter` opened staging mode, so this is
                    // where the cursor first lands on something selectable.
                    self.snap_to_change();
                }
            }
            GitEvent::CommitDiff {
                oid,
                diff,
                files: changed,
            } => {
                self.state.finish_read();
                match &mut self.state.main {
                    MainContent::CommitDiff {
                        oid: current,
                        diff: slot,
                        files,
                    } if *current == oid => {
                        crate::state::keep_or_replace(slot, diff);
                        *files = changed;
                        self.sync_main_len();
                    }
                    // The commit-files drill-down is filled by the same read: it needs the file
                    // list, and the whole patch is what the header row shows.
                    MainContent::CommitFiles {
                        oid: current,
                        files,
                        whole,
                        shown,
                        diff: slot,
                        ..
                    } if *current == oid => {
                        *whole = Some(diff);
                        *files = changed;
                        if shown.is_none() {
                            *slot = whole.clone();
                        }
                        self.sync_main_len();
                        self.request_main_selection();
                    }
                    // A sub-commit's patch, requested by `enter` in the sub-commits view.
                    MainContent::SubCommits {
                        shown: Some(current),
                        diff: slot,
                        ..
                    } if *current == oid => {
                        *slot = Some(diff);
                    }
                    _ => {}
                }
            }
            GitEvent::BranchDiff { name, diff } => {
                self.state.finish_read();
                if let crate::state::MainContent::BranchDiff {
                    name: current,
                    diff: slot,
                } = &mut self.state.main
                    && *current == name
                {
                    crate::state::keep_or_replace(slot, diff);
                    self.sync_main_len();
                }
            }
            GitEvent::StashDiff { index, diff } => {
                self.state.finish_read();
                if let crate::state::MainContent::StashDiff {
                    index: current,
                    diff: slot,
                } = &mut self.state.main
                    && *current == index
                {
                    *slot = Some(diff);
                    self.sync_main_len();
                }
            }
            GitEvent::Conflict { path, file } => {
                self.state.finish_read();
                if let crate::state::MainContent::Conflict {
                    path: current,
                    file: slot,
                    ..
                } = &mut self.state.main
                    && *current == path
                {
                    *slot = Some(file);
                }
            }
            GitEvent::RefCommits {
                reference,
                commits: loaded,
            } => {
                self.state.finish_read();
                if let MainContent::SubCommits {
                    reference: current,
                    commits,
                    ..
                } = &mut self.state.main
                    && *current == reference
                {
                    *commits = loaded;
                    self.sync_main_len();
                }
            }
            GitEvent::CommitFileDiff { oid, path, diff } => {
                self.state.finish_read();
                if let MainContent::CommitFiles {
                    oid: current,
                    shown: Some(shown),
                    diff: slot,
                    ..
                } = &mut self.state.main
                    && *current == oid
                    && *shown == path
                {
                    *slot = Some(diff);
                }
            }
            GitEvent::ReadFailed { label, message } => {
                // A read owns no in-progress guard and arms no escalation dialog.
                self.state.finish_read();
                self.state.last_error =
                    Some(format!("{label}: {}", crate::state::error_line(&message)));
            }
            GitEvent::Mutated { label, warning } => {
                self.state.finish_mutation();
                self.state.last_error = None;
                self.disarm_escalation(&label);
                if let Some(warning) = warning {
                    self.state.toast(
                        Toast::new(format!("{label}: {warning}")).icon(Icon::TriangleAlert),
                        now,
                        TOAST_DWELL,
                    );
                }
            }
            GitEvent::Failed { label, message } => {
                self.state.finish_mutation();
                // A safe attempt that Git refused becomes the "are you sure?" dialog rather than
                // an error the user cannot act on.
                if let Some(escalation) = self.take_escalation(&label) {
                    self.open_confirm(*escalation);
                    return;
                }
                self.state.last_error =
                    Some(format!("{label}: {}", crate::state::error_line(&message)));
            }
            GitEvent::Command(event) => {
                if let Some(line) = command_line(&event) {
                    self.state.log_command(line);
                }
            }
            GitEvent::CommandsReseeded(records) => {
                self.state.command_log.clear();
                for record in records {
                    if let Some(line) = record_line(&record) {
                        self.state.log_command(line);
                    }
                }
            }
            GitEvent::Changed(_) => {
                // fleet-git already debounces; one change event is one snapshot request.
                if !self.state.refreshing {
                    self.state.refreshing = true;
                    self.bridge.send(GitRequest::Snapshot);
                }
            }
        }
    }

    /// Forgets an armed escalation once its mutation succeeded.
    fn disarm_escalation(&mut self, label: &str) {
        if self
            .state
            .escalation
            .as_ref()
            .is_some_and(|(armed, _)| armed == label)
        {
            self.state.escalation = None;
        }
    }

    /// Takes the escalation armed for `label`, if the failure that just arrived is its.
    fn take_escalation(&mut self, label: &str) -> Option<Box<Confirm>> {
        match &self.state.escalation {
            Some((armed, _)) if armed == label => {
                self.state.escalation.take().map(|(_, confirm)| confirm)
            }
            _ => None,
        }
    }

    /// Never leaves staging mode looking at an empty side.
    ///
    /// Staging the last hunk of the unstaged half empties it; lazygit moves the selection to the
    /// other half rather than showing "no changes" over a file that still has staged work.
    fn retarget_staging_side(&mut self) {
        let Some(side) = self.state.staging.as_ref().map(|staging| staging.side) else {
            return;
        };
        if !self.main_model().is_empty() {
            return;
        }
        let flipped = match side {
            DiffSide::Unstaged => DiffSide::Staged,
            DiffSide::Staged => DiffSide::Unstaged,
        };
        if let Some(staging) = &mut self.state.staging {
            staging.side = flipped;
            staging.cursor = 0;
            staging.anchor = None;
            staging.snapped = false;
        }
        // Both halves empty: the file has no changes left, so keep the side the user chose and
        // let the next snapshot move the Files cursor off it.
        if self.main_model().is_empty()
            && let Some(staging) = &mut self.state.staging
        {
            staging.side = side;
        }
    }

    /// Keeps the main panel's cursor length in step with the list it renders.
    fn sync_main_len(&mut self) {
        let rows = self.main_len();
        self.state.cursors.main.set_len(rows);
        if let Some(staging) = &mut self.state.staging {
            staging.cursor = staging.cursor.min(rows.saturating_sub(1));
        }
    }

    /// How many items the main panel's cursor moves over.
    ///
    /// Usually one per diff row, but the two drill-down views put a list there instead: the
    /// sub-commits view one row per commit, the commit-files view a header row plus one per file.
    pub(crate) fn main_len(&self) -> usize {
        match &self.state.main {
            MainContent::SubCommits { commits, .. } => commits.len(),
            MainContent::CommitFiles { files, .. } => files.len() + 1,
            _ => self.main_model().len(),
        }
    }

    /// The patch the main panel's primary list is showing, if any.
    pub(crate) fn main_diff(&self) -> Option<Arc<fleet_git::Diff>> {
        match &self.state.main {
            MainContent::FileDiff {
                unstaged, staged, ..
            } => match self.state.staging.as_ref().map(|staging| staging.side) {
                Some(DiffSide::Staged) => staged.clone(),
                Some(DiffSide::Unstaged) => unstaged.clone(),
                None => match unstaged {
                    Some(diff) if !diff.files.is_empty() => unstaged.clone(),
                    _ => staged.clone(),
                },
            },
            MainContent::CommitDiff { diff, .. }
            | MainContent::BranchDiff { diff, .. }
            | MainContent::StashDiff { diff, .. } => diff.clone(),
            // The drill-downs cursor over their list, not over the patch they render below it.
            _ => None,
        }
    }

    /// Which layout the main panel's primary list uses.
    ///
    /// Staging always renders unified: `Staging::cursor` is an index into the *unified* rows,
    /// which is also what `state::hunk_range` and `state::selection_hunks` key on, so a split
    /// layout there would silently stage the wrong lines.
    pub(crate) fn main_mode(&self) -> DiffViewMode {
        if self.state.staging.is_some() {
            DiffViewMode::Unified
        } else {
            self.state.diff_mode
        }
    }

    /// The cached model for one slot, rebuilt only when its key changes.
    ///
    /// `cx` is `Some` on the render path, which is the only place allowed to start the
    /// background syntax pass; action handlers pass `None` and read whatever is cached.
    pub(crate) fn slot_model(
        &self,
        slot: &'static str,
        diff: Option<&Arc<fleet_git::Diff>>,
        mode: DiffViewMode,
        cx: Option<&Context<Self>>,
    ) -> Rc<DiffModel> {
        let key = diff.map(|diff| ModelKey::new(diff, mode));
        if let Some(key) = key
            && let Some((cached, model)) = self.models.borrow().get(slot)
            && *cached == key
        {
            let model = model.clone();
            if let Some(cx) = cx {
                self.start_syntax(slot, &model, cx);
            }
            return model;
        }
        let started = Instant::now();
        let model = Rc::new(match diff {
            Some(diff) => DiffModel::build(diff, mode),
            None => DiffModel::empty(mode),
        });
        let elapsed = started.elapsed();
        if elapsed > FRAME_BUDGET {
            tracing::warn!(
                slot,
                rows = model.rows.len(),
                elapsed_ms = elapsed.as_millis(),
                "diff: flattening the model overran one frame"
            );
        } else {
            tracing::debug!(
                slot,
                rows = model.rows.len(),
                elapsed_us = elapsed.as_micros(),
                "diff: model built"
            );
        }
        match key {
            Some(key) => {
                self.models.borrow_mut().insert(slot, (key, model.clone()));
            }
            None => {
                self.models.borrow_mut().remove(slot);
                self.syntax_tasks.borrow_mut().remove(slot);
            }
        }
        if let Some(cx) = cx {
            self.start_syntax(slot, &model, cx);
        }
        model
    }

    /// Highlights a model on the background executor, once.
    ///
    /// syntect at `regex-fancy` costs ~60 µs a line here, so a 10 000-line patch is over half a
    /// second: far too much for a frame. The whole pass therefore runs off the foreground thread
    /// and the rows render plain until it lands, at which point one `notify` repaints them.
    fn start_syntax(&self, slot: &'static str, model: &Rc<DiffModel>, cx: &Context<Self>) {
        if !model.claim_syntax() {
            return;
        }
        let jobs = model.syntax_jobs();
        if jobs.is_empty() {
            model.apply_syntax(Vec::new());
            return;
        }
        let lines: usize = jobs.iter().map(|job| job.lines.len()).sum();
        let target = model.clone();
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let runs = cx
                .background_spawn(async move { crate::views::syntax::run(&jobs) })
                .await;
            let elapsed = started.elapsed();
            if elapsed > SYNTAX_BUDGET {
                tracing::warn!(
                    lines,
                    elapsed_ms = elapsed.as_millis(),
                    "diff: syntax pass was slow; rows rendered plain until now"
                );
            } else {
                tracing::debug!(lines, elapsed_ms = elapsed.as_millis(), "diff: syntax pass");
            }
            target.apply_syntax(runs);
            let _ignored = this.update(cx, |_this, cx| cx.notify());
        });
        self.syntax_tasks.borrow_mut().insert(slot, task);
    }

    /// The model the main panel's primary list renders.
    pub fn main_model(&self) -> Rc<DiffModel> {
        self.slot_model(SLOT_MAIN, self.main_diff().as_ref(), self.main_mode(), None)
    }

    // ---------------------------------------------------------------- helpers

    /// Dispatches one request and books it.
    ///
    /// Mutations and reads are counted separately: `pending` is the guard `q` asks about, and a
    /// read that fails must never touch it.
    fn send(&mut self, request: GitRequest) {
        if matches!(request, GitRequest::Mutate { .. }) {
            self.state.begin_mutation();
            self.state.refreshing = true;
        } else if !matches!(
            request,
            GitRequest::Snapshot | GitRequest::Shutdown | GitRequest::SetDiffContext(_)
        ) {
            self.state.begin_read();
        }
        self.bridge.send(request);
    }

    fn mutate(&mut self, label: &str, mutation: Mutation) {
        self.send(GitRequest::Mutate {
            label: label.to_owned(),
            mutation: Box::new(mutation),
        });
    }

    fn refresh_main(&mut self) {
        for request in self.state.refresh_main() {
            self.send(request);
        }
        self.state.cursors.main.set(0);
        self.state.main_h_scroll = 0.0;
        self.sync_main_len();
    }

    fn scroll_handle(&self) -> &UniformListScrollHandle {
        match self.state.focused {
            PanelId::Status => &self.scroll_main,
            PanelId::Files => &self.scroll_files,
            PanelId::Branches => match self.state.branch_tab {
                BranchTab::Local => &self.scroll_branches,
                BranchTab::Remotes if self.state.remote_drill.is_some() => {
                    &self.scroll_remote_branches
                }
                BranchTab::Remotes => &self.scroll_remotes,
                BranchTab::Tags => &self.scroll_tags,
            },
            PanelId::Commits => match self.state.commit_tab {
                CommitTab::Commits => &self.scroll_commits,
                CommitTab::Reflog => &self.scroll_reflog,
            },
            PanelId::Stash => &self.scroll_stashes,
            PanelId::Main => &self.scroll_main,
        }
    }

    fn toast(&mut self, text: impl Into<gpui::SharedString>, icon: Icon) {
        self.state
            .toast(Toast::new(text).icon(icon), Instant::now(), TOAST_DWELL);
    }

    fn open_confirm(&mut self, confirm: Confirm) {
        self.state.push_overlay(Overlay::Confirm(confirm));
    }

    fn open_prompt(&mut self, prompt: Prompt) {
        self.state.push_overlay(Overlay::Prompt(prompt));
    }

    fn open_menu(&mut self, menu: Menu) {
        self.state.push_overlay(Overlay::Menu(menu));
    }

    // ---------------------------------------------------------------- global actions

    fn quit(&mut self, _: &global::Quit, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.mutation_in_flight() || self.state.operation() != OperationState::None {
            self.open_confirm(Confirm {
                title: "Quit".to_owned(),
                target: self.state.root.display().to_string(),
                facts: vec!["An operation is still in progress.".to_owned()],
                danger: false,
                outcome: ConfirmOutcome::Request(Box::new(GitRequest::Shutdown)),
            });
            cx.notify();
            return;
        }
        self.bridge.send(GitRequest::Shutdown);
        cx.quit();
    }

    fn refresh(&mut self, _: &global::Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.state.refreshing = true;
        self.bridge.send(GitRequest::Snapshot);
        cx.notify();
    }

    fn open_help(&mut self, _: &global::OpenHelp, _: &mut Window, cx: &mut Context<Self>) {
        self.state.push_overlay(Overlay::Help { top: 0 });
        cx.notify();
    }

    fn next_screen_mode(
        &mut self,
        _: &global::NextScreenMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.screen_mode = self.state.screen_mode.next();
        cx.notify();
    }

    fn prev_screen_mode(
        &mut self,
        _: &global::PrevScreenMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.screen_mode = self.state.screen_mode.prev();
        cx.notify();
    }

    fn toggle_command_log(
        &mut self,
        _: &global::ToggleCommandLog,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.show_command_log = !self.state.show_command_log;
        cx.notify();
    }

    fn cancel(&mut self, _: &global::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.pop_overlay() {
            cx.notify();
            return;
        }
        if self.state.staging.is_some() {
            self.state.staging = None;
            self.state.focused = self.state.previous_panel;
            self.refresh_main();
            cx.notify();
            return;
        }
        if self.state.focused == PanelId::Main {
            self.state.focused = self.state.previous_panel;
            // A drill-down is not re-derivable from the side panel, so leaving it has to rebuild
            // the main panel; an ordinary diff keeps its scroll, as lazygit's `esc` does.
            if matches!(
                self.state.main,
                MainContent::SubCommits { .. } | MainContent::CommitFiles { .. }
            ) {
                self.refresh_main();
            }
            cx.notify();
            return;
        }
        if self.state.remote_drill.is_some() && self.state.branch_tab == BranchTab::Remotes {
            self.state.remote_drill = None;
            self.refresh_main();
            cx.notify();
            return;
        }
        if self.state.last_error.take().is_some() {
            cx.notify();
        }
    }

    fn focus_panel(&mut self, panel: PanelId, cx: &mut Context<Self>) {
        if self.state.focused == panel {
            return;
        }
        if panel == PanelId::Main {
            self.state.previous_panel = if self.state.focused == PanelId::Main {
                self.state.previous_panel
            } else {
                self.state.focused
            };
        } else {
            self.state.previous_panel = panel;
        }
        self.state.focused = panel;
        if panel != PanelId::Main {
            self.state.staging = None;
            self.refresh_main();
        }
        cx.notify();
    }

    fn focus_status(&mut self, _: &global::FocusStatus, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Status, cx);
    }

    fn focus_files(&mut self, _: &global::FocusFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Files, cx);
    }

    fn focus_branches(
        &mut self,
        _: &global::FocusBranches,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Branches, cx);
    }

    fn focus_commits(&mut self, _: &global::FocusCommits, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Commits, cx);
    }

    fn focus_stash(&mut self, _: &global::FocusStash, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Stash, cx);
    }

    fn focus_main(&mut self, _: &global::FocusMain, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Main, cx);
    }

    fn next_panel(&mut self, _: &global::NextPanel, _: &mut Window, cx: &mut Context<Self>) {
        let panel = if self.state.focused == PanelId::Main {
            self.state.previous_panel
        } else {
            self.state.focused.next_side()
        };
        self.focus_panel(panel, cx);
    }

    fn prev_panel(&mut self, _: &global::PrevPanel, _: &mut Window, cx: &mut Context<Self>) {
        let panel = if self.state.focused == PanelId::Main {
            self.state.previous_panel
        } else {
            self.state.focused.prev_side()
        };
        self.focus_panel(panel, cx);
    }

    fn cycle_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.state.focused {
            PanelId::Branches => {
                self.state.branch_tab = match (self.state.branch_tab, forward) {
                    (BranchTab::Local, true) => BranchTab::Remotes,
                    (BranchTab::Remotes, true) => BranchTab::Tags,
                    (BranchTab::Tags, true) => BranchTab::Local,
                    (BranchTab::Local, false) => BranchTab::Tags,
                    (BranchTab::Remotes, false) => BranchTab::Local,
                    (BranchTab::Tags, false) => BranchTab::Remotes,
                };
                self.state.remote_drill = None;
                self.refresh_main();
            }
            PanelId::Commits => {
                self.state.commit_tab = match self.state.commit_tab {
                    CommitTab::Commits => CommitTab::Reflog,
                    CommitTab::Reflog => CommitTab::Commits,
                };
                self.refresh_main();
            }
            _ => {}
        }
        cx.notify();
    }

    fn next_tab(&mut self, _: &global::NextTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(true, cx);
    }

    fn prev_tab(&mut self, _: &global::PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(false, cx);
    }

    fn pull(&mut self, _: &global::Pull, _: &mut Window, cx: &mut Context<Self>) {
        self.mutate("pull", Mutation::Pull(PullRequest::default()));
        cx.notify();
    }

    fn push(&mut self, _: &global::Push, _: &mut Window, cx: &mut Context<Self>) {
        let branch = self.state.head_branch().map(str::to_owned);
        let upstream = self
            .state
            .branches()
            .iter()
            .find(|candidate| candidate.is_head)
            .and_then(|candidate| candidate.upstream.clone());
        match (branch, upstream) {
            (Some(_), Some(_)) => {
                self.mutate("push", Mutation::Push(PushRequest::default()));
            }
            (Some(branch), None) => {
                self.open_prompt(Prompt {
                    title: "Push and set upstream".to_owned(),
                    subtitle: Some(format!("`{branch}` has no upstream. Name the remote.")),
                    buffer: crate::state::Buffer::single_line().with_text("origin"),
                    kind: PromptKind::PushSetUpstream,
                });
            }
            (None, _) => self.toast("HEAD is detached; nothing to push.", Icon::TriangleAlert),
        }
        cx.notify();
    }

    fn fetch(&mut self, _: &global::Fetch, _: &mut Window, cx: &mut Context<Self>) {
        self.mutate(
            "fetch",
            Mutation::Fetch(FetchRequest {
                remote: None,
                prune: true,
                all: false,
            }),
        );
        cx.notify();
    }

    fn operation_menu(
        &mut self,
        _: &global::OperationMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let operation = self.state.operation();
        let items = match operation {
            OperationState::None => {
                self.toast("Nothing in progress.", Icon::CircleDot);
                cx.notify();
                return;
            }
            OperationState::Rebasing { .. } => vec![
                menu_request(
                    "c",
                    "continue rebase",
                    "rebase continue",
                    Mutation::RebaseContinue,
                ),
                menu_request("s", "skip commit", "rebase skip", Mutation::RebaseSkip),
                menu_request("a", "abort rebase", "rebase abort", Mutation::RebaseAbort),
            ],
            OperationState::Merging => vec![
                menu_request(
                    "c",
                    "continue merge",
                    "merge continue",
                    Mutation::MergeContinue,
                ),
                menu_request("a", "abort merge", "merge abort", Mutation::MergeAbort),
            ],
            OperationState::CherryPicking | OperationState::Reverting => vec![
                menu_request(
                    "c",
                    "continue cherry-pick",
                    "cherry-pick continue",
                    Mutation::CherryPickContinue,
                ),
                menu_request(
                    "a",
                    "abort cherry-pick",
                    "cherry-pick abort",
                    Mutation::CherryPickAbort,
                ),
            ],
            OperationState::Bisecting => {
                self.toast("Bisect is not supported yet.", Icon::TriangleAlert);
                cx.notify();
                return;
            }
        };
        self.open_menu(Menu::new("Merge / rebase options", items));
        cx.notify();
    }

    // ---------------------------------------------------------------- list motion

    fn motion(&mut self, motion: ListMotion, cx: &mut Context<Self>) {
        if self.state.focused == PanelId::Main {
            let rows = self.main_len();
            // `ctrl-d` / `ctrl-u` move half a *measured* viewport, never a hardcoded ten rows.
            let page = self.state.cursors.main.page_rows();
            if let Some(staging) = &mut self.state.staging {
                let cursor = staging.cursor;
                let next = match motion {
                    ListMotion::Down => (cursor + 1).min(rows.saturating_sub(1)),
                    ListMotion::Up => cursor.saturating_sub(1),
                    ListMotion::First => 0,
                    ListMotion::Last => rows.saturating_sub(1),
                    ListMotion::PageDown => (cursor + page).min(rows.saturating_sub(1)),
                    ListMotion::PageUp => cursor.saturating_sub(page),
                };
                staging.cursor = next;
                self.state.cursors.main.set_len(rows);
                self.state.cursors.main.set(next);
            } else {
                self.state.cursors.main.set_len(rows);
                self.state.cursors.main.motion(motion);
            }
            let moving_down = motion.moves_down();
            ListView::reveal(&self.scroll_main, &self.state.cursors.main, moving_down);
            self.request_main_selection();
            cx.notify();
            return;
        }
        let moving_down = self.state.focused_cursor_mut().motion(motion);
        let cursor = *self.state.focused_cursor();
        ListView::reveal(self.scroll_handle(), &cursor, moving_down);
        self.state.remember_selection();
        self.refresh_main();
        cx.notify();
    }

    fn move_down(&mut self, _: &list::MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::Down, cx);
    }

    fn move_up(&mut self, _: &list::MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::Up, cx);
    }

    fn go_top(&mut self, _: &list::GoTop, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::First, cx);
    }

    fn go_bottom(&mut self, _: &list::GoBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::Last, cx);
    }

    fn page_down(&mut self, _: &list::PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::PageDown, cx);
    }

    fn page_up(&mut self, _: &list::PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::PageUp, cx);
    }

    fn scroll_left(&mut self, _: &list::ScrollLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.state.main_h_scroll = (self.state.main_h_scroll - crate::views::diff::H_STEP).max(0.0);
        cx.notify();
    }

    /// `L`: pan right, clamped to the widest line — scrolling past it used to yield blank rows.
    fn scroll_right(&mut self, _: &list::ScrollRight, _: &mut Window, cx: &mut Context<Self>) {
        let limit = crate::views::diff::max_h_scroll(&self.main_model(), self.main_px_w);
        self.state.main_h_scroll =
            (self.state.main_h_scroll + crate::views::diff::H_STEP).min(limit);
        cx.notify();
    }

    // ---------------------------------------------------------------- diff view

    /// Pans the diff payload sideways by a wheel delta, clamped to the longest line.
    ///
    /// `uniform_list` only ever declares `overflow.y = Scroll`, so a horizontal delta — a
    /// two-finger sideways swipe, or `shift` plus a wheel on a platform that swaps the axis —
    /// reaches the list and is dropped. Handling it here is purely additive: nothing else in the
    /// chain consumes `delta.x`, so there is no double-scroll to guard against.
    pub(crate) fn pan_diff(&mut self, delta_x: f32, cx: &mut Context<Self>) {
        if delta_x == 0.0 {
            return;
        }
        let limit = crate::views::diff::max_h_scroll(&self.main_model(), self.main_px_w);
        let next = (self.state.main_h_scroll - delta_x).clamp(0.0, limit);
        if next != self.state.main_h_scroll {
            self.state.main_h_scroll = next;
            cx.notify();
        }
    }

    /// `|`: switch the diff between unified and side by side.
    ///
    /// Staging mode stays unified whatever this says (see [`Self::main_mode`]); the toggle still
    /// takes effect, it just does not apply until staging is left.
    fn toggle_split(
        &mut self,
        _: &diff_actions::ToggleSplit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.diff_mode = self.state.diff_mode.toggled();
        self.state.main_h_scroll = 0.0;
        self.sync_main_len();
        cx.notify();
    }

    /// `{` / `}`: narrow or widen the context git prints around every hunk.
    ///
    /// The width lives on the repository, so the patch a staging selection builds is read back
    /// with the same `-U` it was displayed with.
    fn set_context(&mut self, context: u32, cx: &mut Context<Self>) {
        let context = context.min(fleet_git::MAX_DIFF_CONTEXT);
        if context == self.state.diff_context {
            return;
        }
        self.state.diff_context = context;
        self.send(GitRequest::SetDiffContext(context));
        self.refresh_main();
        self.toast(format!("Diff context: {context}"), Icon::FileDiff);
        cx.notify();
    }

    fn less_context(
        &mut self,
        _: &diff_actions::LessContext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.state.diff_context.saturating_sub(1);
        self.set_context(context, cx);
    }

    fn more_context(
        &mut self,
        _: &diff_actions::MoreContext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.state.diff_context.saturating_add(1);
        self.set_context(context, cx);
    }

    // ---------------------------------------------------------------- Files

    /// `space`: on a file, stage or unstage it; on a directory, every file under it.
    ///
    /// lazygit asks the *node* whether anything below it is unstaged, so a directory with one
    /// unstaged file stages the whole directory, and a fully staged one unstages it.
    fn toggle_staged(&mut self, _: &files::ToggleStaged, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.state.file_row().cloned() else {
            return;
        };
        if row.children.is_empty() {
            return;
        }
        if row.unstaged {
            self.mutate("stage", Mutation::Stage(row.children));
        } else {
            self.mutate("unstage", Mutation::Unstage(row.children));
        }
        cx.notify();
    }

    fn toggle_staged_all(
        &mut self,
        _: &files::ToggleStagedAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_unstaged = self.state.files().iter().any(has_unstaged);
        if any_unstaged {
            self.mutate("stage all", Mutation::StageAll);
        } else {
            self.mutate("unstage all", Mutation::UnstageAll);
        }
        cx.notify();
    }

    /// `d`: discard the row's changes — one file, or every file under a directory.
    fn discard_file(&mut self, _: &files::Discard, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.state.file_row().cloned() else {
            return;
        };
        if row.children.is_empty() {
            return;
        }
        let mut facts = vec!["The working-tree changes cannot be recovered.".to_owned()];
        if row.is_dir {
            facts.insert(
                0,
                format!(
                    "{} file{} under this directory.",
                    row.children.len(),
                    if row.children.len() == 1 { "" } else { "s" }
                ),
            );
        }
        self.open_confirm(Confirm {
            title: "Discard changes".to_owned(),
            target: row.path.display().to_string(),
            facts,
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "discard".to_owned(),
                mutation: Box::new(Mutation::Discard(row.children)),
            })),
        });
        cx.notify();
    }

    fn commit(&mut self, _: &files::Commit, _: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(Prompt {
            title: "Commit".to_owned(),
            subtitle: Some("⌘⏎ commits, ⏎ starts a new line".to_owned()),
            buffer: crate::state::Buffer::multi_line(),
            kind: PromptKind::Commit { amend: false },
        });
        cx.notify();
    }

    fn amend(&mut self, _: &files::Amend, _: &mut Window, cx: &mut Context<Self>) {
        self.open_amend_confirm();
        cx.notify();
    }

    fn open_amend_confirm(&mut self) {
        self.open_confirm(Confirm {
            title: "Amend last commit".to_owned(),
            target: self
                .state
                .commits()
                .first()
                .map(|commit| commit.subject.clone())
                .unwrap_or_default(),
            facts: vec!["The staged changes are folded into the last commit.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "amend".to_owned(),
                mutation: Box::new(Mutation::Commit {
                    message: String::new(),
                    options: CommitOptions {
                        amend: true,
                        allow_empty: true,
                        ..CommitOptions::default()
                    },
                }),
            })),
        });
    }

    fn stash_menu(&mut self, _: &files::StashMenu, _: &mut Window, cx: &mut Context<Self>) {
        let items = vec![
            MenuItem {
                key: "a".to_owned(),
                label: "stash all changes".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Stash all changes".to_owned(),
                    subtitle: Some("Message".to_owned()),
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::Stash(StashOptions::default()),
                })),
            },
            MenuItem {
                key: "s".to_owned(),
                label: "stash staged changes".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Stash staged changes".to_owned(),
                    subtitle: Some("Message".to_owned()),
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::Stash(StashOptions {
                        staged_only: true,
                        ..StashOptions::default()
                    }),
                })),
            },
            MenuItem {
                key: "u".to_owned(),
                label: "stash all, including untracked".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Stash including untracked".to_owned(),
                    subtitle: Some("Message".to_owned()),
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::Stash(StashOptions {
                        include_untracked: true,
                        ..StashOptions::default()
                    }),
                })),
            },
        ];
        self.open_menu(Menu::new("Stash options", items));
        cx.notify();
    }

    /// `` ` `` / `~` — the tree/flat switch.
    fn toggle_file_tree(&mut self, _: &files::ToggleTree, _: &mut Window, cx: &mut Context<Self>) {
        self.state.toggle_file_tree_mode();
        self.refresh_main();
        cx.notify();
    }

    /// `-` — collapse every directory.
    fn collapse_all_files(
        &mut self,
        _: &files::CollapseAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.collapse_all_files();
        self.refresh_main();
        cx.notify();
    }

    /// `=` — expand every directory.
    fn expand_all_files(&mut self, _: &files::ExpandAll, _: &mut Window, cx: &mut Context<Self>) {
        self.state.expand_all_files();
        self.refresh_main();
        cx.notify();
    }

    /// `enter`: staging mode on a file, the conflict view on a conflicted one, and — lazygit's
    /// "Stage lines / Collapse directory" — a collapse toggle on a directory row.
    fn enter_file(&mut self, _: &files::Enter, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.state.file_row().cloned()
            && row.is_dir
        {
            self.state.toggle_file_collapsed(&row.path);
            self.refresh_main();
            cx.notify();
            return;
        }
        let Some(file) = self.state.selected_file().cloned() else {
            return;
        };
        self.state.previous_panel = PanelId::Files;
        self.state.focused = PanelId::Main;
        if file.conflict.is_some() {
            self.state.staging = None;
            self.refresh_main();
        } else {
            let side = if has_unstaged(&file) {
                DiffSide::Unstaged
            } else {
                DiffSide::Staged
            };
            self.state.staging = Some(Staging {
                path: file.path.clone(),
                side,
                cursor: 0,
                anchor: None,
                line_mode: false,
                snapped: false,
            });
            self.sync_main_len();
            self.snap_to_change();
        }
        cx.notify();
    }

    /// Puts the staging cursor on the first added or removed line, unless it is on one already.
    ///
    /// lazygit snaps to a change line whenever it enters or re-modes the staging view; a context
    /// line is never a useful selection. It does **not** re-snap on a background refresh, which
    /// is what `Staging::snapped` guards: the periodic re-read of the file diff used to drag the
    /// selection back to the first hunk every two seconds.
    fn snap_to_change(&mut self) {
        if self
            .state
            .staging
            .as_ref()
            .is_some_and(|staging| staging.snapped)
        {
            return;
        }
        let model = self.main_model();
        let rows = &model.rows;
        let already = self
            .state
            .staging
            .as_ref()
            .and_then(|staging| rows.get(staging.cursor))
            .is_some_and(crate::views::diff_model::DiffRow::is_change);
        if already {
            if let Some(staging) = &mut self.state.staging {
                staging.snapped = true;
            }
            return;
        }
        if let Some(index) = rows
            .iter()
            .position(crate::views::diff_model::DiffRow::is_change)
            && let Some(staging) = &mut self.state.staging
        {
            staging.cursor = index;
            staging.snapped = true;
            self.state.cursors.main.set_len(rows.len());
            self.state.cursors.main.set(index);
            ListView::reveal(&self.scroll_main, &self.state.cursors.main, true);
        }
    }

    // ---------------------------------------------------------------- Branches

    fn checkout_branch(&mut self, _: &branches::Checkout, _: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        self.mutate("checkout", Mutation::Checkout(Ref::from(branch.name)));
        cx.notify();
    }

    fn new_branch(&mut self, _: &branches::New, _: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(Prompt {
            title: "New branch".to_owned(),
            subtitle: Some("Branch name".to_owned()),
            buffer: crate::state::Buffer::single_line(),
            kind: PromptKind::NewBranch { start_point: None },
        });
        cx.notify();
    }

    fn delete_branch(&mut self, _: &branches::Delete, _: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let name = branch.name.clone();
        let forced = Confirm {
            title: "Force delete branch".to_owned(),
            target: name.clone(),
            facts: vec!["The branch is not merged. Its commits become unreachable.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "delete branch".to_owned(),
                mutation: Box::new(Mutation::DeleteBranch {
                    name: name.clone(),
                    force: true,
                }),
            })),
        };
        self.open_confirm(Confirm {
            title: "Delete branch".to_owned(),
            target: name.clone(),
            facts: vec!["A branch that is not merged asks again before it is forced.".to_owned()],
            danger: false,
            // The safe `git branch -d` runs first; the force dialog appears only if Git refuses.
            outcome: ConfirmOutcome::RequestOrEscalate {
                request: Box::new(GitRequest::Mutate {
                    label: "delete branch".to_owned(),
                    mutation: Box::new(Mutation::DeleteBranch {
                        name: name.clone(),
                        force: false,
                    }),
                }),
                escalate: Box::new(forced),
            },
        });
        cx.notify();
    }

    fn rename_branch(&mut self, _: &branches::Rename, _: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        self.open_prompt(Prompt {
            title: "Rename branch".to_owned(),
            subtitle: Some(format!("Renaming `{}`", branch.name)),
            buffer: crate::state::Buffer::single_line().with_text(branch.name.clone()),
            kind: PromptKind::RenameBranch(branch.name),
        });
        cx.notify();
    }

    fn merge_branch(&mut self, _: &branches::Merge, _: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let head = self.state.head_branch().unwrap_or("HEAD").to_owned();
        self.open_confirm(Confirm {
            title: "Merge".to_owned(),
            target: format!("{} → {head}", branch.name),
            facts: vec!["A conflict pauses the merge; `m` then offers continue/abort.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "merge".to_owned(),
                mutation: Box::new(Mutation::Merge(Ref::from(branch.name))),
            })),
        });
        cx.notify();
    }

    fn rebase_branch(&mut self, _: &branches::Rebase, _: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let head = self.state.head_branch().unwrap_or("HEAD").to_owned();
        self.open_confirm(Confirm {
            title: "Rebase".to_owned(),
            target: format!("{head} onto {}", branch.name),
            facts: vec!["Rewrites the checked-out branch's commits.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "rebase".to_owned(),
                mutation: Box::new(Mutation::RebaseOnto(Ref::from(branch.name))),
            })),
        });
        cx.notify();
    }

    fn upstream_menu(
        &mut self,
        _: &branches::UpstreamMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let name = branch.name.clone();
        let items = vec![
            MenuItem {
                key: "s".to_owned(),
                label: "set upstream".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Set upstream".to_owned(),
                    subtitle: Some("remote/branch".to_owned()),
                    buffer: crate::state::Buffer::single_line().with_text(format!("origin/{name}")),
                    kind: PromptKind::SetUpstream(name.clone()),
                })),
            },
            MenuItem {
                key: "u".to_owned(),
                label: "unset upstream".to_owned(),
                action: MenuAction::Request(Box::new(GitRequest::Mutate {
                    label: "unset upstream".to_owned(),
                    mutation: Box::new(Mutation::UnsetUpstream(name.clone())),
                })),
            },
        ];
        self.open_menu(Menu::new("Upstream options", items));
        cx.notify();
    }

    fn tag_branch(&mut self, _: &branches::Tag, _: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .state
            .selected_branch()
            .map(|branch| branch.name.clone())
            .unwrap_or_else(|| "HEAD".to_owned());
        self.open_prompt(Prompt {
            title: "New tag".to_owned(),
            subtitle: Some(format!("Tag name for `{target}`")),
            buffer: crate::state::Buffer::single_line(),
            kind: PromptKind::NewTag(target),
        });
        cx.notify();
    }

    fn enter_branch(&mut self, _: &branches::Enter, _: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let upstream = branch
            .upstream
            .as_ref()
            .map(|upstream| upstream.name.clone());
        self.open_sub_commits(branch.name, upstream, PanelId::Branches, cx);
    }

    /// Opens lazygit's sub-commits view on a ref: its log in the main panel, the selected
    /// commit's patch below it.
    fn open_sub_commits(
        &mut self,
        reference: String,
        upstream: Option<String>,
        from: PanelId,
        cx: &mut Context<Self>,
    ) {
        self.state.previous_panel = from;
        self.state.focused = PanelId::Main;
        self.state.staging = None;
        self.state.main = MainContent::SubCommits {
            reference: reference.clone(),
            commits: Vec::new(),
            shown: None,
            diff: None,
        };
        self.state.cursors.main.set_len(0);
        self.state.main_h_scroll = 0.0;
        self.send(GitRequest::RefCommits {
            reference,
            upstream,
            limit: SUB_COMMIT_LIMIT,
        });
        cx.notify();
    }

    /// `enter` / `space` in the sub-commits view: read the selected commit's patch.
    fn sub_commit_show_diff(
        &mut self,
        _: &subcommits::ShowDiff,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.state.cursors.main.index();
        let Some(oid) = (match &self.state.main {
            MainContent::SubCommits { commits, .. } => {
                commits.get(index).map(|commit| commit.oid.clone())
            }
            _ => None,
        }) else {
            return;
        };
        if let MainContent::SubCommits { shown, diff, .. } = &mut self.state.main {
            *shown = Some(oid.clone());
            *diff = None;
        }
        self.send(GitRequest::CommitDiff(oid));
        cx.notify();
    }

    // ---------------------------------------------------------------- Remotes and tags

    fn enter_remote(&mut self, _: &remotes::Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.remote_drill.is_some() {
            let Some(branch) = self.state.selected_remote_branch().cloned() else {
                return;
            };
            // A remote branch is its own upstream, so every commit on it counts as pushed.
            let upstream = Some(branch.name.clone());
            self.open_sub_commits(branch.name, upstream, PanelId::Branches, cx);
            return;
        }
        if let Some(remote) = self.state.selected_remote().cloned() {
            self.state.remote_drill = Some(remote.name);
            self.state.cursors.remote_branches.set(0);
            self.refresh_main();
        }
        cx.notify();
    }

    fn checkout_remote(&mut self, _: &remotes::Checkout, _: &mut Window, cx: &mut Context<Self>) {
        let Some(remote) = self.state.remote_drill.clone() else {
            self.toast("Enter a remote first.", Icon::CircleDot);
            cx.notify();
            return;
        };
        let Some(branch) = self.state.selected_remote_branch().cloned() else {
            return;
        };
        self.mutate(
            "checkout remote branch",
            Mutation::CheckoutRemoteBranch {
                remote,
                name: branch.branch,
            },
        );
        cx.notify();
    }

    fn new_tag(&mut self, _: &tags::New, _: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(Prompt {
            title: "New tag".to_owned(),
            subtitle: Some("Tag name for HEAD".to_owned()),
            buffer: crate::state::Buffer::single_line(),
            kind: PromptKind::NewTag("HEAD".to_owned()),
        });
        cx.notify();
    }

    fn delete_tag(&mut self, _: &tags::Delete, _: &mut Window, cx: &mut Context<Self>) {
        let Some(tag) = self.state.selected_tag().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Delete tag".to_owned(),
            target: tag.name.clone(),
            facts: vec!["The tag is removed locally only.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "delete tag".to_owned(),
                mutation: Box::new(Mutation::DeleteTag(tag.name)),
            })),
        });
        cx.notify();
    }

    fn checkout_tag(&mut self, _: &tags::Checkout, _: &mut Window, cx: &mut Context<Self>) {
        let Some(tag) = self.state.selected_tag().cloned() else {
            return;
        };
        self.mutate("checkout tag", Mutation::Checkout(Ref::from(tag.name)));
        cx.notify();
    }

    // ---------------------------------------------------------------- Commits

    fn selected_oid(&self) -> Option<fleet_git::ObjectId> {
        // While the sub-commits view owns the keyboard, `c` / `v` / `g` act on *its* cursor.
        if self.state.focused == PanelId::Main
            && let MainContent::SubCommits { commits, .. } = &self.state.main
        {
            return commits
                .get(self.state.cursors.main.index())
                .map(|commit| commit.oid.clone());
        }
        match self.state.commit_tab {
            CommitTab::Commits => self.state.selected_commit().map(|c| c.oid.clone()),
            CommitTab::Reflog => self.state.selected_reflog().map(|e| e.oid.clone()),
        }
    }

    fn enter_commit(&mut self, _: &commits::Enter, _: &mut Window, cx: &mut Context<Self>) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        let subject = match self.state.commit_tab {
            CommitTab::Commits => self
                .state
                .selected_commit()
                .map(|commit| commit.subject.clone()),
            CommitTab::Reflog => self
                .state
                .selected_reflog()
                .map(|entry| entry.subject.clone()),
        }
        .unwrap_or_default();
        let from = if self.state.focused == PanelId::Main {
            self.state.previous_panel
        } else {
            self.state.focused
        };
        self.open_commit_files(oid, subject, from, cx);
    }

    /// Opens lazygit's commit-files view: a header row plus the commit's changed files, over the
    /// patch of whichever row is selected. The header row keeps the whole-commit patch reachable.
    fn open_commit_files(
        &mut self,
        oid: fleet_git::ObjectId,
        subject: String,
        from: PanelId,
        cx: &mut Context<Self>,
    ) {
        // The commit's diff and file list are usually already in hand: the side panel's selection
        // read them when the cursor landed on the commit.
        let (files, whole) = match &self.state.main {
            MainContent::CommitDiff {
                oid: current,
                diff,
                files,
            } if *current == oid => (files.clone(), diff.clone()),
            _ => (Vec::new(), None),
        };
        let need_read = whole.is_none() || files.is_empty();
        self.state.previous_panel = from;
        self.state.focused = PanelId::Main;
        self.state.staging = None;
        self.state.main = MainContent::CommitFiles {
            oid: oid.clone(),
            subject,
            files,
            whole: whole.clone(),
            shown: None,
            diff: whole,
        };
        self.state.main_h_scroll = 0.0;
        self.sync_main_len();
        self.state.cursors.main.set(0);
        if need_read {
            self.send(GitRequest::CommitDiff(oid));
        }
        cx.notify();
    }

    /// `enter` / `space` in the commit-files view: re-read the selected row's patch.
    ///
    /// The lower half already follows the cursor, so this only has to force the read the cursor
    /// move skipped — which is what makes `enter` on the header row a way back to the whole patch.
    fn commit_file_show_patch(
        &mut self,
        _: &commitfiles::ShowPatch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.state.cursors.main.index();
        let request = match &mut self.state.main {
            MainContent::CommitFiles {
                oid, files, shown, ..
            } => match index.checked_sub(1).and_then(|row| files.get(row)) {
                Some(file) => {
                    *shown = Some(file.path.clone());
                    Some(GitRequest::CommitFileDiff {
                        oid: oid.clone(),
                        path: file.path.clone(),
                    })
                }
                None => {
                    *shown = None;
                    Some(GitRequest::CommitDiff(oid.clone()))
                }
            },
            _ => None,
        };
        if let Some(request) = request {
            self.send(request);
        }
        cx.notify();
    }

    /// Loads whatever the main panel's cursor now points at, for the views whose lower half
    /// follows the selection. A no-op everywhere else.
    fn request_main_selection(&mut self) {
        let index = self.state.cursors.main.index();
        let request = match &mut self.state.main {
            MainContent::CommitFiles {
                oid,
                files,
                whole,
                shown,
                diff,
                ..
            } => {
                // Row 0 is the commit header: it shows the whole patch, every later row one file.
                match index.checked_sub(1).and_then(|row| files.get(row)) {
                    Some(file) => {
                        let path = file.path.clone();
                        if shown.as_deref() == Some(path.as_path()) {
                            None
                        } else {
                            *shown = Some(path.clone());
                            *diff = None;
                            Some(GitRequest::CommitFileDiff {
                                oid: oid.clone(),
                                path,
                            })
                        }
                    }
                    None => {
                        *shown = None;
                        *diff = whole.clone();
                        None
                    }
                }
            }
            _ => None,
        };
        if let Some(request) = request {
            self.send(request);
        }
    }

    fn checkout_commit(&mut self, _: &commits::Checkout, _: &mut Window, cx: &mut Context<Self>) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Checkout commit".to_owned(),
            target: crate::state::short_oid(&oid),
            facts: vec!["HEAD becomes detached.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "checkout".to_owned(),
                mutation: Box::new(Mutation::Checkout(Ref::from(oid.0))),
            })),
        });
        cx.notify();
    }

    fn reword_commit(&mut self, _: &commits::Reword, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_prompt(Prompt {
            title: "Reword commit".to_owned(),
            subtitle: Some("⌘⏎ rewords, ⏎ starts a new line".to_owned()),
            buffer: crate::state::Buffer::multi_line().with_text(commit.subject.clone()),
            kind: PromptKind::Reword(commit.oid),
        });
        cx.notify();
    }

    fn squash_commit(&mut self, _: &commits::Squash, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Squash".to_owned(),
            target: format!(
                "{} {}",
                crate::state::short_oid(&commit.oid),
                commit.subject
            ),
            facts: vec!["Folds the commit into the one below it via a rebase.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "squash".to_owned(),
                mutation: Box::new(Mutation::Squash(commit.oid)),
            })),
        });
        cx.notify();
    }

    fn fixup_commit(&mut self, _: &commits::Fixup, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Fixup".to_owned(),
            target: format!(
                "{} {}",
                crate::state::short_oid(&commit.oid),
                commit.subject
            ),
            facts: vec!["Melds the commit into the one below it and drops its message.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "fixup".to_owned(),
                mutation: Box::new(Mutation::Fixup(commit.oid)),
            })),
        });
        cx.notify();
    }

    fn drop_commit(&mut self, _: &commits::Drop, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Drop commit".to_owned(),
            target: format!(
                "{} {}",
                crate::state::short_oid(&commit.oid),
                commit.subject
            ),
            facts: vec!["The commit is removed from the branch via a rebase.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "drop".to_owned(),
                mutation: Box::new(Mutation::DropCommit(commit.oid)),
            })),
        });
        cx.notify();
    }

    fn edit_commit(&mut self, _: &commits::Edit, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.mutate("edit commit", Mutation::EditCommit(commit.oid));
        cx.notify();
    }

    fn move_commit_down(&mut self, _: &commits::MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.mutate(
            "move commit down",
            Mutation::MoveCommit {
                oid: commit.oid,
                direction: MoveDirection::Down,
            },
        );
        cx.notify();
    }

    fn move_commit_up(&mut self, _: &commits::MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.mutate(
            "move commit up",
            Mutation::MoveCommit {
                oid: commit.oid,
                direction: MoveDirection::Up,
            },
        );
        cx.notify();
    }

    fn reset_menu(&mut self, _: &commits::ResetMenu, _: &mut Window, cx: &mut Context<Self>) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        let short = crate::state::short_oid(&oid);
        let item = |key: &str, label: &str, mode: ResetMode, danger: bool| MenuItem {
            key: key.to_owned(),
            label: label.to_owned(),
            action: MenuAction::Confirm(Box::new(Confirm {
                title: format!("Reset {label}"),
                target: short.clone(),
                facts: vec![
                    match mode {
                        ResetMode::Soft => "Moves HEAD; index and worktree keep their content.",
                        ResetMode::Mixed => {
                            "Moves HEAD and resets the index; the worktree is kept."
                        }
                        ResetMode::Hard => "Moves HEAD and discards the index and the worktree.",
                    }
                    .to_owned(),
                ],
                danger,
                outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                    label: "reset".to_owned(),
                    mutation: Box::new(Mutation::Reset {
                        to: Ref::from(oid.0.clone()),
                        mode,
                    }),
                })),
            })),
        };
        self.open_menu(Menu::new(
            "Reset options",
            vec![
                item("s", "soft", ResetMode::Soft, false),
                item("m", "mixed", ResetMode::Mixed, false),
                item("h", "hard", ResetMode::Hard, true),
            ],
        ));
        cx.notify();
    }

    fn copy_commit(&mut self, _: &commits::Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        if let Some(index) = self.state.copied.iter().position(|copied| *copied == oid) {
            self.state.copied.remove(index);
        } else {
            self.state.copied.push(oid);
        }
        let count = self.state.copied.len();
        let noun = if count == 1 { "commit" } else { "commits" };
        self.toast(format!("{count} {noun} copied"), Icon::Check);
        cx.notify();
    }

    fn paste_commits(&mut self, _: &commits::Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.copied.is_empty() {
            self.toast(
                "Nothing copied. Press `c` on a commit first.",
                Icon::CircleDot,
            );
            cx.notify();
            return;
        }
        let copied = self.state.copied.clone();
        self.open_confirm(Confirm {
            title: "Paste commits".to_owned(),
            target: copied
                .iter()
                .map(crate::state::short_oid)
                .collect::<Vec<_>>()
                .join(" "),
            facts: vec!["Cherry-picks the copied commits onto the current branch.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "cherry-pick".to_owned(),
                mutation: Box::new(Mutation::CherryPick(copied)),
            })),
        });
        cx.notify();
    }

    fn revert_commit(&mut self, _: &commits::Revert, _: &mut Window, cx: &mut Context<Self>) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Revert commit".to_owned(),
            target: crate::state::short_oid(&oid),
            facts: vec!["Creates a commit that applies the changes in reverse.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "revert".to_owned(),
                mutation: Box::new(Mutation::Revert(oid)),
            })),
        });
        cx.notify();
    }

    fn tag_commit(&mut self, _: &commits::Tag, _: &mut Window, cx: &mut Context<Self>) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_prompt(Prompt {
            title: "Tag commit".to_owned(),
            subtitle: Some(format!("Tag name for {}", crate::state::short_oid(&oid))),
            buffer: crate::state::Buffer::single_line(),
            kind: PromptKind::NewTag(oid.0),
        });
        cx.notify();
    }

    fn amend_commit(&mut self, _: &commits::Amend, _: &mut Window, cx: &mut Context<Self>) {
        self.open_amend_confirm();
        cx.notify();
    }

    fn branch_from_commit(
        &mut self,
        _: &commits::NewBranch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_prompt(Prompt {
            title: "New branch".to_owned(),
            subtitle: Some(format!("Starting at {}", crate::state::short_oid(&oid))),
            buffer: crate::state::Buffer::single_line(),
            kind: PromptKind::NewBranch {
                start_point: Some(oid.0),
            },
        });
        cx.notify();
    }

    // ---------------------------------------------------------------- Stash

    fn stash_apply(&mut self, _: &stash::Apply, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.mutate("stash apply", Mutation::StashApply(entry.index));
        cx.notify();
    }

    fn stash_pop(&mut self, _: &stash::Pop, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.mutate("stash pop", Mutation::StashPop(entry.index));
        cx.notify();
    }

    fn stash_drop(&mut self, _: &stash::Drop, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Drop stash entry".to_owned(),
            target: format!("stash@{{{}}}: {}", entry.index, entry.subject),
            facts: vec!["The entry cannot be recovered.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "stash drop".to_owned(),
                mutation: Box::new(Mutation::StashDrop(entry.index)),
            })),
        });
        cx.notify();
    }

    fn stash_branch(&mut self, _: &stash::NewBranch, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.open_prompt(Prompt {
            title: "Branch from stash".to_owned(),
            subtitle: Some(format!("Branch name for stash@{{{}}}", entry.index)),
            buffer: crate::state::Buffer::single_line(),
            kind: PromptKind::BranchFromStash(entry.index),
        });
        cx.notify();
    }

    fn enter_stash(&mut self, _: &stash::Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Main, cx);
    }

    // ---------------------------------------------------------------- Staging

    /// The rows the current selection covers, as indexes into the main model's rows.
    pub(crate) fn staging_range(&self) -> Option<(usize, usize)> {
        let staging = self.state.staging.as_ref()?;
        let model = self.main_model();
        let rows = &model.rows;
        rows.get(staging.cursor)?;
        if let Some(anchor) = staging.anchor {
            let (start, end) = if anchor <= staging.cursor {
                (anchor, staging.cursor)
            } else {
                (staging.cursor, anchor)
            };
            return Some((start, end));
        }
        if staging.line_mode {
            return Some((staging.cursor, staging.cursor));
        }
        // Hunk mode: the whole `@@` hunk the cursor sits in, keyed by (file, hunk).
        crate::state::hunk_range(rows, staging.cursor)
    }

    fn patch_selection(&self) -> Option<PatchSelection> {
        let staging = self.state.staging.as_ref()?;
        let (start, end) = self.staging_range()?;
        let model = self.main_model();
        let hunks = crate::state::selection_hunks(&model.rows, start, end);
        if hunks.is_empty() {
            return None;
        }
        Some(PatchSelection {
            path: staging.path.clone(),
            side: staging.side,
            hunks,
        })
    }

    fn staging_apply(&mut self, _: &staging::Apply, _: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.patch_selection() else {
            self.toast("Nothing selected.", Icon::CircleDot);
            cx.notify();
            return;
        };
        let action = match selection.side {
            DiffSide::Unstaged => PatchAction::Stage,
            DiffSide::Staged => PatchAction::Unstage,
        };
        let label = if action == PatchAction::Stage {
            "stage selection"
        } else {
            "unstage selection"
        };
        self.mutate(label, Mutation::Patch { selection, action });
        if let Some(staging) = &mut self.state.staging {
            staging.anchor = None;
        }
        cx.notify();
    }

    fn staging_discard(&mut self, _: &staging::Discard, _: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.patch_selection() else {
            return;
        };
        if selection.side == DiffSide::Staged {
            self.mutate(
                "unstage selection",
                Mutation::Patch {
                    selection,
                    action: PatchAction::Unstage,
                },
            );
            cx.notify();
            return;
        }
        self.open_confirm(Confirm {
            title: "Discard selection".to_owned(),
            target: selection.path.display().to_string(),
            facts: vec!["The selected lines are reverted in the working tree.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "discard selection".to_owned(),
                mutation: Box::new(Mutation::Patch {
                    selection,
                    action: PatchAction::Discard,
                }),
            })),
        });
        cx.notify();
    }

    fn staging_toggle_range(
        &mut self,
        _: &staging::ToggleRange,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(staging) = &mut self.state.staging {
            staging.anchor = match staging.anchor {
                Some(_) => None,
                None => {
                    staging.line_mode = true;
                    Some(staging.cursor)
                }
            };
        }
        cx.notify();
    }

    fn staging_toggle_line_mode(
        &mut self,
        _: &staging::ToggleLineMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(staging) = &mut self.state.staging {
            staging.line_mode = !staging.line_mode;
            staging.anchor = None;
            staging.snapped = false;
        }
        self.snap_to_change();
        cx.notify();
    }

    fn staging_switch_side(
        &mut self,
        _: &staging::SwitchSide,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(staging) = &mut self.state.staging {
            staging.side = match staging.side {
                DiffSide::Unstaged => DiffSide::Staged,
                DiffSide::Staged => DiffSide::Unstaged,
            };
            staging.cursor = 0;
            staging.anchor = None;
            staging.snapped = false;
        }
        self.sync_main_len();
        self.snap_to_change();
        cx.notify();
    }

    fn staging_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        let model = self.main_model();
        let rows = &model.rows;
        let Some(staging) = &mut self.state.staging else {
            return;
        };
        let file = rows.get(staging.cursor).map(|row| row.file);
        let current = rows.get(staging.cursor).and_then(|row| row.hunk);
        let same_file = |row: &crate::views::diff_model::DiffRow| Some(row.file) == file;
        let target = if forward {
            rows.iter()
                .enumerate()
                .find(|(_, row)| row.line.is_some() && same_file(row) && row.hunk > current)
        } else {
            // Both directions land on the *top* of the hunk they move to, as lazygit's `h`/`l` do,
            // so the nearest previous hunk has to be found before its first line.
            let previous = rows
                .iter()
                .filter(|row| {
                    row.line.is_some()
                        && same_file(row)
                        && current.is_some_and(|c| row.hunk < Some(c))
                })
                .filter_map(|row| row.hunk)
                .max();
            previous.and_then(|hunk| {
                rows.iter()
                    .enumerate()
                    .find(|(_, row)| row.line.is_some() && same_file(row) && row.hunk == Some(hunk))
            })
        };
        if let Some((index, _)) = target {
            staging.cursor = index;
            staging.anchor = None;
            self.state.cursors.main.set_len(rows.len());
            self.state.cursors.main.set(index);
            ListView::reveal(&self.scroll_main, &self.state.cursors.main, forward);
        }
        cx.notify();
    }

    fn staging_prev_hunk(&mut self, _: &staging::PrevHunk, _: &mut Window, cx: &mut Context<Self>) {
        self.staging_hunk(false, cx);
    }

    fn staging_next_hunk(&mut self, _: &staging::NextHunk, _: &mut Window, cx: &mut Context<Self>) {
        self.staging_hunk(true, cx);
    }

    // ---------------------------------------------------------------- Conflicts

    fn resolve(&mut self, choice: ConflictChoice, label: &str, cx: &mut Context<Self>) {
        let crate::state::MainContent::Conflict { path, .. } = &self.state.main else {
            return;
        };
        let path = path.clone();
        self.mutate(label, Mutation::ResolveConflict { path, choice });
        cx.notify();
    }

    fn take_ours(&mut self, _: &conflict::TakeOurs, _: &mut Window, cx: &mut Context<Self>) {
        self.resolve(ConflictChoice::Ours, "resolve ours", cx);
    }

    fn take_theirs(&mut self, _: &conflict::TakeTheirs, _: &mut Window, cx: &mut Context<Self>) {
        self.resolve(ConflictChoice::Theirs, "resolve theirs", cx);
    }

    fn take_both(&mut self, _: &conflict::TakeBoth, _: &mut Window, cx: &mut Context<Self>) {
        self.resolve(ConflictChoice::Both, "resolve both", cx);
    }

    fn conflict_section(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let crate::state::MainContent::Conflict { file, section, .. } = &mut self.state.main {
            let total = file.as_ref().map_or(0, |file| file.conflicts.len());
            if total > 0 {
                *section = if forward {
                    (*section + 1) % total
                } else {
                    (*section + total - 1) % total
                };
            }
        }
        cx.notify();
    }

    fn next_section(&mut self, _: &conflict::NextSection, _: &mut Window, cx: &mut Context<Self>) {
        self.conflict_section(true, cx);
    }

    fn prev_section(&mut self, _: &conflict::PrevSection, _: &mut Window, cx: &mut Context<Self>) {
        self.conflict_section(false, cx);
    }

    // ---------------------------------------------------------------- Overlays

    fn confirm_accept(&mut self, _: &confirm::Accept, _: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Confirm(confirm)) = self.state.overlays.pop() else {
            return;
        };
        match confirm.outcome {
            ConfirmOutcome::Request(request) => match *request {
                GitRequest::Shutdown => {
                    self.bridge.send(GitRequest::Shutdown);
                    cx.quit();
                }
                request => self.send(request),
            },
            ConfirmOutcome::RequestOrEscalate { request, escalate } => {
                if let GitRequest::Mutate { label, .. } = request.as_ref() {
                    self.state.escalation = Some((label.clone(), escalate));
                }
                self.send(*request);
            }
            ConfirmOutcome::Prompt(prompt) => self.open_prompt(*prompt),
        }
        cx.notify();
    }

    fn confirm_cancel(&mut self, _: &confirm::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.state.pop_overlay();
        cx.notify();
    }

    fn prompt_accept(&mut self, _: &prompt::Accept, _: &mut Window, cx: &mut Context<Self>) {
        let multiline = matches!(self.state.overlay(), Some(Overlay::Prompt(prompt)) if prompt.buffer.is_multiline());
        if multiline {
            if let Some(Overlay::Prompt(prompt)) = self.state.overlay_mut() {
                prompt.buffer.insert("\n");
            }
            cx.notify();
            return;
        }
        self.submit_prompt(cx);
    }

    fn prompt_submit(&mut self, _: &prompt::Submit, _: &mut Window, cx: &mut Context<Self>) {
        self.submit_prompt(cx);
    }

    fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Prompt(prompt)) = self.state.overlays.pop() else {
            return;
        };
        let value = prompt.buffer.value().trim().to_owned();
        match prompt.kind {
            PromptKind::Commit { amend } => {
                if value.is_empty() {
                    self.toast("A commit needs a message.", Icon::TriangleAlert);
                } else {
                    self.mutate(
                        "commit",
                        Mutation::Commit {
                            message: value,
                            options: CommitOptions {
                                amend,
                                ..CommitOptions::default()
                            },
                        },
                    );
                }
            }
            PromptKind::NewBranch { start_point } => {
                if !value.is_empty() {
                    self.mutate(
                        "new branch",
                        Mutation::CheckoutNewBranch {
                            name: value,
                            start_point: start_point.map(Ref::from),
                        },
                    );
                }
            }
            PromptKind::RenameBranch(old) => {
                if !value.is_empty() && value != old {
                    self.mutate("rename branch", Mutation::RenameBranch { old, new: value });
                }
            }
            PromptKind::NewTag(target) => {
                if !value.is_empty() {
                    self.mutate(
                        "new tag",
                        Mutation::CreateTag {
                            name: value,
                            target: Ref::from(target),
                            message: None,
                        },
                    );
                }
            }
            PromptKind::Reword(oid) => {
                if !value.is_empty() {
                    self.mutate(
                        "reword",
                        Mutation::Reword {
                            oid,
                            message: value,
                        },
                    );
                }
            }
            PromptKind::SetUpstream(branch) => match value.split_once('/') {
                Some((remote, remote_branch)) => self.mutate(
                    "set upstream",
                    Mutation::SetUpstream {
                        branch,
                        remote: remote.to_owned(),
                        remote_branch: remote_branch.to_owned(),
                    },
                ),
                None => self.toast("Type `remote/branch`.", Icon::TriangleAlert),
            },
            PromptKind::Stash(options) => {
                let message = (!value.is_empty()).then_some(value);
                self.mutate(
                    "stash",
                    Mutation::StashPush(StashOptions { message, ..options }),
                );
            }
            PromptKind::BranchFromStash(index) => {
                if !value.is_empty() {
                    self.mutate("stash branch", Mutation::StashBranch { name: value, index });
                }
            }
            PromptKind::PushSetUpstream => {
                let branch = self.state.head_branch().map(str::to_owned);
                if let Some(branch) = branch {
                    self.mutate(
                        "push",
                        Mutation::Push(PushRequest {
                            remote: Some(if value.is_empty() {
                                "origin".to_owned()
                            } else {
                                value
                            }),
                            branch: Some(branch),
                            set_upstream: true,
                            ..PushRequest::default()
                        }),
                    );
                }
            }
        }
        cx.notify();
    }

    fn prompt_cancel(&mut self, _: &prompt::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.state.pop_overlay();
        cx.notify();
    }

    fn with_buffer(&mut self, edit: impl FnOnce(&mut crate::state::Buffer)) {
        match self.state.overlay_mut() {
            Some(Overlay::Prompt(prompt)) => edit(&mut prompt.buffer),
            Some(Overlay::Menu(menu)) => {
                if let Some(filter) = &mut menu.filter {
                    edit(filter);
                }
                menu.cursor = 0;
            }
            _ => {}
        }
    }

    fn prompt_backspace(&mut self, _: &prompt::Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(|buffer| {
            buffer.backspace();
        });
        cx.notify();
    }

    fn prompt_delete_word(
        &mut self,
        _: &prompt::DeleteWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_buffer(|buffer| {
            buffer.delete_word();
        });
        cx.notify();
    }

    fn prompt_delete_to_start(
        &mut self,
        _: &prompt::DeleteToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_buffer(|buffer| {
            buffer.delete_to_line_start();
        });
        cx.notify();
    }

    fn prompt_left(&mut self, _: &prompt::Left, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::left);
        cx.notify();
    }

    fn prompt_right(&mut self, _: &prompt::Right, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::right);
        cx.notify();
    }

    fn prompt_home(&mut self, _: &prompt::Home, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::home);
        cx.notify();
    }

    fn prompt_end(&mut self, _: &prompt::End, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::end);
        cx.notify();
    }

    fn prompt_paste(&mut self, _: &prompt::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        if !text.is_empty() {
            self.with_buffer(|buffer| buffer.insert(&text));
        }
        cx.notify();
    }

    fn menu_accept(&mut self, _: &menu::Accept, _: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Menu(menu)) = self.state.overlay() else {
            return;
        };
        let Some(action) = menu.selected().map(|item| item.action.clone()) else {
            return;
        };
        self.run_menu_action(action, cx);
    }

    /// Closes the menu and performs one row's action.
    fn run_menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        self.state.pop_overlay();
        match action {
            MenuAction::Request(request) => self.send(*request),
            MenuAction::Confirm(confirm) => self.open_confirm(*confirm),
            MenuAction::Prompt(prompt) => self.open_prompt(*prompt),
        }
        cx.notify();
    }

    fn menu_cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut()
            && menu.filter.is_some()
        {
            menu.filter = None;
            menu.cursor = 0;
            cx.notify();
            return;
        }
        self.state.pop_overlay();
        cx.notify();
    }

    fn menu_down(&mut self, _: &menu::Down, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut() {
            let len = menu.visible().len();
            if len > 0 {
                menu.cursor = (menu.cursor + 1) % len;
            }
        }
        cx.notify();
    }

    fn menu_up(&mut self, _: &menu::Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut() {
            let len = menu.visible().len();
            if len > 0 {
                menu.cursor = (menu.cursor + len - 1) % len;
            }
        }
        cx.notify();
    }

    fn menu_start_filter(&mut self, _: &menu::StartFilter, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut() {
            menu.filter = Some(crate::state::Buffer::single_line());
            menu.cursor = 0;
        }
        cx.notify();
    }

    fn help_close(&mut self, _: &help::Close, _: &mut Window, cx: &mut Context<Self>) {
        self.state.pop_overlay();
        cx.notify();
    }

    fn help_down(&mut self, _: &help::Down, _: &mut Window, cx: &mut Context<Self>) {
        // The last row is the last thing the overlay scrolls to; past it there is nothing to see.
        let last = crate::overlays::help_last_top(self);
        if let Some(Overlay::Help { top }) = self.state.overlay_mut() {
            *top = (*top + 1).min(last);
        }
        cx.notify();
    }

    fn help_up(&mut self, _: &help::Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Help { top }) = self.state.overlay_mut() {
            *top = top.saturating_sub(1);
        }
        cx.notify();
    }

    // ---------------------------------------------------------------- typing

    /// Types a printable character into the focused buffer.
    ///
    /// gpui dispatches key **bindings** before `on_key_down`, and no text context binds a
    /// single-character key, so exactly the printable set reaches here.
    fn typed(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        let Some(text) = event.keystroke.key_char.as_deref() else {
            return;
        };
        if text.is_empty() || text.chars().any(char::is_control) {
            return;
        }
        // lazygit's menus dispatch on the letter printed beside each row. It only applies while
        // no filter prompt is open: a printable key then belongs to the filter.
        if let Some(Overlay::Menu(menu)) = self.state.overlay()
            && menu.filter.is_none()
            && let Some(action) = menu.item_for_key(text).map(|item| item.action.clone())
        {
            self.run_menu_action(action, cx);
            return;
        }
        let typing = matches!(
            self.state.overlay(),
            Some(Overlay::Prompt(_))
                | Some(Overlay::Menu(Menu {
                    filter: Some(_),
                    ..
                }))
        );
        if !typing {
            return;
        }
        let text = text.to_owned();
        self.with_buffer(|buffer| buffer.insert(&text));
        cx.notify();
    }

    // ---------------------------------------------------------------- measurement

    /// Recomputes how many whole rows each pane can show, and how wide the side column is.
    ///
    /// gpui gives an element its size during layout, which is too late for a key handler, so the
    /// frame's own arithmetic is repeated here from the window size: the bands and the two fixed
    /// pane heights are constants, and every other side pane is one equal share of what is left.
    /// The numbers feed two things: `ctrl-d` / `ctrl-u` page by half a real viewport, and each
    /// side list is clipped to a whole number of rows so no row is ever half painted.
    fn measure(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (status_bar_h, banner_h, header_h, row_h, hairline) = {
            let metrics = &cx.theme().metrics;
            (
                f32::from(metrics.status_bar_h),
                f32::from(metrics.banner_h),
                f32::from(metrics.pane_header_h),
                f32::from(metrics.row_h),
                f32::from(metrics.hairline),
            )
        };
        let viewport = window.viewport_size();
        let banner = if self.state.operation() == OperationState::None {
            0.0
        } else {
            banner_h
        };
        let body_h = (f32::from(viewport.height) - status_bar_h - banner).max(0.0);
        let rows_in = |height: f32, row: f32| {
            if row <= 0.0 {
                return 1;
            }
            (((height - header_h) / row).floor().max(1.0)) as usize
        };

        let normal = self.state.screen_mode == crate::state::ScreenMode::Normal;
        let stash_focused = self.state.focused == PanelId::Stash;
        let (side_h, stash_h) = if normal {
            let fixed = crate::panels::STATUS_H
                + 4.0 * hairline
                + if stash_focused {
                    0.0
                } else {
                    crate::panels::STASH_H
                };
            let shares = if stash_focused { 4.0 } else { 3.0 };
            let flex = (body_h - fixed).max(0.0) / shares;
            (
                flex,
                if stash_focused {
                    flex
                } else {
                    crate::panels::STASH_H
                },
            )
        } else {
            (body_h, body_h)
        };
        self.rows_side = rows_in(side_h, row_h);
        self.rows_stash = rows_in(stash_h, row_h);

        let main_h = body_h
            - if self.state.show_command_log {
                crate::panels::LOG_H
            } else {
                0.0
            };
        self.rows_main = rows_in(main_h, crate::views::diff::ROW_H);

        let ratio = crate::panels::side_ratio(self.state.screen_mode, self.state.focused);
        let column = f32::from(viewport.width) * ratio;
        let one_ch = f32::from(fleet_ui_kit::theme::ch(1.0)).max(1.0);
        self.side_ch = (column / one_ch).floor().max(8.0) as usize;
        // What is left of the window once the side column and the gutters are taken: the
        // horizontal-scroll clamp measures against this.
        self.main_px_w = (f32::from(viewport.width)
            - column
            - crate::views::diff::gutter_width(&self.main_model()))
        .max(80.0);

        // Every list pages by half its own viewport (`ListCursor::set_page_from_visible`).
        for cursor in [
            &mut self.state.cursors.files,
            &mut self.state.cursors.branches,
            &mut self.state.cursors.remotes,
            &mut self.state.cursors.remote_branches,
            &mut self.state.cursors.tags,
            &mut self.state.cursors.commits,
            &mut self.state.cursors.reflog,
        ] {
            cursor.set_page_from_visible(self.rows_side);
        }
        self.state
            .cursors
            .stashes
            .set_page_from_visible(self.rows_stash);
        let main_rows = match self.state.main {
            // The two drill-downs split the pane between a list and a patch.
            MainContent::SubCommits { .. } | MainContent::CommitFiles { .. } => {
                (self.rows_main / 2).max(1)
            }
            _ => self.rows_main,
        };
        self.state.cursors.main.set_page_from_visible(main_rows);
    }

    // ---------------------------------------------------------------- rendering

    /// Wraps `child` in one div per key-context word, outermost first.
    fn contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .size_full()
                .min_h_0()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }

    /// The same nesting as a layer rather than a flex child, so the frame's bands never move.
    fn overlay_contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .absolute()
                .inset_0()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }

    /// Installs every action listener. They live on the root div, above the focused element, so
    /// gpui's upward dispatch reaches them from any context.
    fn with_actions(root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::refresh))
            .on_action(cx.listener(Self::open_help))
            .on_action(cx.listener(Self::next_screen_mode))
            .on_action(cx.listener(Self::prev_screen_mode))
            .on_action(cx.listener(Self::toggle_command_log))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_status))
            .on_action(cx.listener(Self::focus_files))
            .on_action(cx.listener(Self::focus_branches))
            .on_action(cx.listener(Self::focus_commits))
            .on_action(cx.listener(Self::focus_stash))
            .on_action(cx.listener(Self::focus_main))
            .on_action(cx.listener(Self::next_panel))
            .on_action(cx.listener(Self::prev_panel))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::pull))
            .on_action(cx.listener(Self::push))
            .on_action(cx.listener(Self::fetch))
            .on_action(cx.listener(Self::operation_menu))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::go_top))
            .on_action(cx.listener(Self::go_bottom))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::scroll_left))
            .on_action(cx.listener(Self::scroll_right))
            .on_action(cx.listener(Self::toggle_split))
            .on_action(cx.listener(Self::less_context))
            .on_action(cx.listener(Self::more_context))
            .on_action(cx.listener(Self::toggle_staged))
            .on_action(cx.listener(Self::toggle_staged_all))
            .on_action(cx.listener(Self::discard_file))
            .on_action(cx.listener(Self::commit))
            .on_action(cx.listener(Self::amend))
            .on_action(cx.listener(Self::stash_menu))
            .on_action(cx.listener(Self::toggle_file_tree))
            .on_action(cx.listener(Self::collapse_all_files))
            .on_action(cx.listener(Self::expand_all_files))
            .on_action(cx.listener(Self::enter_file))
            .on_action(cx.listener(Self::checkout_branch))
            .on_action(cx.listener(Self::new_branch))
            .on_action(cx.listener(Self::delete_branch))
            .on_action(cx.listener(Self::rename_branch))
            .on_action(cx.listener(Self::merge_branch))
            .on_action(cx.listener(Self::rebase_branch))
            .on_action(cx.listener(Self::upstream_menu))
            .on_action(cx.listener(Self::tag_branch))
            .on_action(cx.listener(Self::enter_branch))
            .on_action(cx.listener(Self::enter_remote))
            .on_action(cx.listener(Self::checkout_remote))
            .on_action(cx.listener(Self::new_tag))
            .on_action(cx.listener(Self::delete_tag))
            .on_action(cx.listener(Self::checkout_tag))
            .on_action(cx.listener(Self::enter_commit))
            .on_action(cx.listener(Self::checkout_commit))
            .on_action(cx.listener(Self::reword_commit))
            .on_action(cx.listener(Self::squash_commit))
            .on_action(cx.listener(Self::fixup_commit))
            .on_action(cx.listener(Self::drop_commit))
            .on_action(cx.listener(Self::edit_commit))
            .on_action(cx.listener(Self::move_commit_down))
            .on_action(cx.listener(Self::move_commit_up))
            .on_action(cx.listener(Self::reset_menu))
            .on_action(cx.listener(Self::copy_commit))
            .on_action(cx.listener(Self::paste_commits))
            .on_action(cx.listener(Self::revert_commit))
            .on_action(cx.listener(Self::tag_commit))
            .on_action(cx.listener(Self::amend_commit))
            .on_action(cx.listener(Self::branch_from_commit))
            .on_action(cx.listener(Self::stash_apply))
            .on_action(cx.listener(Self::stash_pop))
            .on_action(cx.listener(Self::stash_drop))
            .on_action(cx.listener(Self::stash_branch))
            .on_action(cx.listener(Self::enter_stash))
            .on_action(cx.listener(Self::sub_commit_show_diff))
            .on_action(cx.listener(Self::commit_file_show_patch))
            .on_action(cx.listener(Self::staging_apply))
            .on_action(cx.listener(Self::staging_discard))
            .on_action(cx.listener(Self::staging_toggle_range))
            .on_action(cx.listener(Self::staging_toggle_line_mode))
            .on_action(cx.listener(Self::staging_switch_side))
            .on_action(cx.listener(Self::staging_prev_hunk))
            .on_action(cx.listener(Self::staging_next_hunk))
            .on_action(cx.listener(Self::take_ours))
            .on_action(cx.listener(Self::take_theirs))
            .on_action(cx.listener(Self::take_both))
            .on_action(cx.listener(Self::next_section))
            .on_action(cx.listener(Self::prev_section))
            .on_action(cx.listener(Self::confirm_accept))
            .on_action(cx.listener(Self::confirm_cancel))
            .on_action(cx.listener(Self::prompt_accept))
            .on_action(cx.listener(Self::prompt_submit))
            .on_action(cx.listener(Self::prompt_cancel))
            .on_action(cx.listener(Self::prompt_backspace))
            .on_action(cx.listener(Self::prompt_delete_word))
            .on_action(cx.listener(Self::prompt_delete_to_start))
            .on_action(cx.listener(Self::prompt_left))
            .on_action(cx.listener(Self::prompt_right))
            .on_action(cx.listener(Self::prompt_home))
            .on_action(cx.listener(Self::prompt_end))
            .on_action(cx.listener(Self::prompt_paste))
            .on_action(cx.listener(Self::menu_accept))
            .on_action(cx.listener(Self::menu_cancel))
            .on_action(cx.listener(Self::menu_down))
            .on_action(cx.listener(Self::menu_up))
            .on_action(cx.listener(Self::menu_start_filter))
            .on_action(cx.listener(Self::help_close))
            .on_action(cx.listener(Self::help_down))
            .on_action(cx.listener(Self::help_up))
    }
}

/// A menu row that fires one mutation.
fn menu_request(key: &str, label: &str, mutation_label: &str, mutation: Mutation) -> MenuItem {
    MenuItem {
        key: key.to_owned(),
        label: label.to_owned(),
        action: MenuAction::Request(Box::new(GitRequest::Mutate {
            label: mutation_label.to_owned(),
            mutation: Box::new(mutation),
        })),
    }
}

/// One command-log line, or `None` for events that add nothing.
fn command_line(event: &CommandEvent) -> Option<String> {
    match event {
        CommandEvent::Started(_) => None,
        CommandEvent::Finished(record) => record_line(record),
    }
}

fn record_line(record: &fleet_git::CommandRecord) -> Option<String> {
    let argv = record.display_argv.join(" ");
    let elapsed = record
        .elapsed
        .map(|elapsed| format!(" ({} ms)", elapsed.as_millis()))
        .unwrap_or_default();
    match &record.outcome {
        CommandOutcome::Running => None,
        CommandOutcome::Success { .. } => Some(format!("✓ {argv}{elapsed}")),
        CommandOutcome::Failed {
            status,
            stderr_preview,
            ..
        } => {
            let stderr = String::from_utf8_lossy(stderr_preview);
            let first = stderr.lines().next().unwrap_or_default();
            Some(format!(
                "✗ {argv}{elapsed} — exit {}{}",
                status.unwrap_or(-1),
                if first.is_empty() {
                    String::new()
                } else {
                    format!(": {first}")
                }
            ))
        }
        CommandOutcome::TimedOut => Some(format!("✗ {argv} — timed out")),
        CommandOutcome::SpawnFailed(message) => Some(format!("✗ {argv} — {message}")),
    }
}

impl Focusable for Lazygit {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Lazygit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_active = window.is_window_active();
        self.measure(window, cx);
        let chain = self.state.context_chain();
        let overlay_open = self.state.overlay().is_some();

        // Exactly one of the two handles is focused, and it is the one attached to the element
        // carrying the key-context chain, so an overlay really does shadow the panels.
        let wanted = if overlay_open {
            &self.overlay_focus
        } else {
            &self.focus
        };
        if !wanted.is_focused(window) {
            window.focus(wanted, cx);
        }

        let overlay_element = crate::overlays::render(self, cx).map(|element| {
            Self::overlay_contexts(
                &chain,
                div()
                    .absolute()
                    .inset_0()
                    .track_focus(&self.overlay_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        this.typed(event, cx);
                    }))
                    .child(element)
                    .into_any_element(),
            )
        });

        let body = self.body(window, cx);
        let body = if overlay_open {
            body
        } else {
            Self::contexts(
                &chain,
                div()
                    .size_full()
                    .min_h_0()
                    .track_focus(&self.focus)
                    .child(body)
                    .into_any_element(),
            )
        };

        let mut frame = AppFrame::new()
            .body(body)
            .status_bar(self.status_bar(&chain, cx));
        if let Some(banner) = self.banner(cx) {
            frame = frame.banner(banner);
        }
        if let Some(overlay) = overlay_element {
            frame = frame.overlay(overlay);
        }
        if !self.state.toasts.is_empty() {
            frame = frame.body_overlay(ToastStack::new(self.state.toasts.clone()));
        }

        Self::with_actions(div().size_full().key_context(ROOT_CONTEXT), cx).child(frame)
    }
}
