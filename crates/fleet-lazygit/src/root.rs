//! The coordinating Git pane entity.

mod commits_actions;
mod composition;
mod conflicts;
mod diff_view;
mod events;
mod files_actions;
mod mutations;
mod navigation;
mod overlays;
mod refs;
mod staging_actions;
#[cfg(test)]
mod tests;

use diff_view::DiffView;

use std::cell::Cell;
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
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Pixels,
    Render, Size, Task, UniformListScrollHandle, Window, canvas, div,
};

use crate::actions::{
    branches, commitfiles, commits, conflict, diff as diff_actions, files, global, lg_confirm,
    lg_help, list, menu, prompt, remotes, staging, stash, subcommits, tags,
};
use crate::bridge::{GitBridge, GitEvent, GitRequest, Mutation};
use crate::keymap::ROOT_CONTEXT;
use crate::state::{
    BranchTab, CommitTab, Confirm, ConfirmOutcome, GitUiState, MainContent, Menu, MenuAction,
    MenuItem, Overlay, PanelId, Prompt, PromptKind, Staging, has_unstaged,
};
use crate::views::diff_model::{DiffModel, DiffViewMode, ModelKey};

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
    pub(crate) state: GitUiState,
    /// The git worker.
    pub(crate) bridge: GitBridge,
    /// Focused while a panel owns the keyboard.
    focus: FocusHandle,
    /// Focused while an overlay owns the keyboard.
    overlay_focus: FocusHandle,
    /// One scroll handle per list, owned by the view and passed by reference each render.
    pub(crate) scroll_files: UniformListScrollHandle,
    /// Branches list scroll.
    pub(crate) scroll_branches: UniformListScrollHandle,
    /// Remotes list scroll.
    pub(crate) scroll_remotes: UniformListScrollHandle,
    /// Remote-branches list scroll.
    pub(crate) scroll_remote_branches: UniformListScrollHandle,
    /// Tags list scroll.
    pub(crate) scroll_tags: UniformListScrollHandle,
    /// Commits list scroll.
    pub(crate) scroll_commits: UniformListScrollHandle,
    /// Reflog list scroll.
    pub(crate) scroll_reflog: UniformListScrollHandle,
    /// Stash list scroll.
    pub(crate) scroll_stashes: UniformListScrollHandle,
    /// Main panel scroll.
    pub(crate) scroll_main: UniformListScrollHandle,
    /// Secondary panel scroll.
    pub(crate) scroll_secondary: UniformListScrollHandle,
    conflict_view: Option<conflicts::ConflictView>,
    pub(crate) scroll_conflict_ours: UniformListScrollHandle,
    pub(crate) scroll_conflict_theirs: UniformListScrollHandle,
    pub(crate) scroll_editor: UniformListScrollHandle,
    editor_caret: Option<(usize, usize)>,
    models: HashMap<&'static str, DiffView>,
    empty_models: [Rc<DiffModel>; 2],
    _model_subscription: gpui::Subscription,
    _theme_subscription: gpui::Subscription,
    _focus_subscription: Option<gpui::Subscription>,
    overlay_focused: bool,
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
    /// Whether the view draws its own [`AppFrame`] and owns the window.
    ///
    /// `false` when a host application ([`fleet-app`](https://github.com/dannyfuf/fleetd))
    /// renders this view as one pane among others: the host owns the window chrome, the
    /// keyboard and the quit decision, so the frame, the unconditional focus grab and
    /// `cx.quit()` all have to come out. See [`Lazygit::embedded`].
    embedded: bool,
    /// Whether the host says this pane currently owns the keyboard. Always `true` standalone.
    active: bool,
    /// The pixel size the embedded pane was last laid out into.
    ///
    /// Standalone the window *is* the pane, so [`Window::viewport_size`] is exact; embedded it
    /// is far too tall (the host's bars and tab strip sit inside it) and every row budget
    /// derived from it would page past the end of the list.
    pane_size: Rc<Cell<Option<Size<Pixels>>>>,
    /// Whether the window is active, which sets the refresh-fallback interval.
    window_active: bool,
    /// When the last periodic refresh went out.
    last_refresh: Instant,
    snapshot_dirty: bool,
    _event_task: Task<()>,
    _ticker: Option<Task<()>>,
}

impl Lazygit {
    /// Builds the view, starts the git bridge and asks for the first snapshot.
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = GitBridge::start(path.clone());
        let event_task = Self::spawn_event_loop(&bridge, cx);
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
            _focus_subscription: None,
            overlay_focused: false,
            conflict_view: None,
            scroll_conflict_ours: UniformListScrollHandle::new(),
            scroll_conflict_theirs: UniformListScrollHandle::new(),
            scroll_editor: UniformListScrollHandle::new(),
            editor_caret: None,
            models: HashMap::new(),
            empty_models: [
                Rc::new(DiffModel::empty(DiffViewMode::Unified)),
                Rc::new(DiffModel::empty(DiffViewMode::Split)),
            ],
            _model_subscription: cx.observe_self(|this, cx| this.prepare_models(cx)),
            _theme_subscription: cx.observe_global::<fleet_ui_kit::Theme>(|this, cx| {
                this.prepare_models(cx);
                cx.notify();
            }),
            main_px_w: 600.0,
            rows_side: fleet_ui_kit::DEFAULT_PAGE * 2,
            rows_stash: 2,
            rows_main: fleet_ui_kit::DEFAULT_PAGE * 2,
            side_ch: 40,
            embedded: false,
            active: true,
            pane_size: Rc::new(Cell::new(None)),
            window_active: true,
            last_refresh: Instant::now(),
            snapshot_dirty: false,
            _event_task: event_task,
            _ticker: Some(Self::spawn_ticker(cx)),
        }
    }

    /// The same view, rendered as one pane of a host application.
    ///
    /// The difference from [`Lazygit::new`] is entirely in `render`: no [`AppFrame`], no
    /// per-frame focus grab, and `q` emits [`LazygitEvent::Quit`] instead of quitting the
    /// process. lazygit's own one-row key-hint / mode bar stays inside the pane, because it is
    /// the crate's discoverability contract and not window chrome.
    pub fn embedded(path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            embedded: true,
            active: false,
            _ticker: None,
            ..Self::new(path, cx)
        }
    }

    /// Tells the pane whether it currently owns the keyboard.
    ///
    /// Focus is taken exactly once per activation: taking it every frame would fight the host,
    /// whose own root element also focuses itself whenever it is not focused.
    pub fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self._focus_subscription.is_none() {
            self._focus_subscription =
                Some(cx.observe_in(&cx.entity(), window, |this, _, window, cx| {
                    let overlay = this.state.overlay().is_some();
                    if overlay != this.overlay_focused {
                        this.overlay_focused = overlay;
                        if this.active && this.owns_keyboard(window) {
                            window.focus(this.wanted_focus(), cx);
                        }
                    }
                }));
        }
        let was = self.active;
        self.active = active;
        if active && !self.owns_keyboard(window) {
            window.focus(self.wanted_focus(), cx);
        }
        if was != active {
            self._ticker = active.then(|| Self::spawn_ticker(cx));
            self.prepare_models(cx);
            if active && !self.state.refreshing {
                self.snapshot_dirty = false;
                self.state.refreshing = true;
                self.bridge.send(GitRequest::Snapshot);
            }
            cx.notify();
        }
    }

    /// The handle that must hold the keyboard for the rendered context chain to answer keys.
    fn wanted_focus(&self) -> &FocusHandle {
        if self.state.overlay().is_some() {
            &self.overlay_focus
        } else {
            &self.focus
        }
    }

    /// Whether one of this view's two handles is the focused element.
    fn owns_keyboard(&self, window: &Window) -> bool {
        self.focus.is_focused(window) || self.overlay_focus.is_focused(window)
    }
}

/// What the view tells a host application about.
///
/// Standalone there is nobody to tell, so [`crate::run`] subscribes and maps `Quit` onto
/// `cx.quit()`; embedded the host decides what closing the pane means.
/// Deliberately exhaustive: every embedder in this workspace should stop compiling when a new
/// variant lands rather than silently ignoring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LazygitEvent {
    /// `q`, or the confirmation behind it, asked to leave.
    Quit,
}

impl EventEmitter<LazygitEvent> for Lazygit {}

impl Focusable for Lazygit {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
