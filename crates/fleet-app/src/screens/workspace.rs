//! Workspace tabs, native panes, and watches around a shared terminal controller.
//!
//! Input is retained only while attaching a live terminal; disconnection drops it.
//! PTYs remain daemon-owned when this surface detaches.

use crate::terminal::surface::*;

mod actions;
mod agent;
mod chrome;
mod lifecycle;
mod model;
mod native;
mod terminal;
#[cfg(test)]
mod tests;

use actions::*;
use agent::*;
use chrome::*;
use model::*;
use native::*;
use terminal::*;

pub(crate) use model::status_kind;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use fleet_core::{
    agents::{ThreadId, ThreadProjection},
    github::{PrChecks, PrReviewDecision, PrTab, derive_pr_state},
    ids::{RepoId, SessionId, TerminalId, WorktreeId},
    model::Worktree,
    sessions::{AgentActivity, Session, SessionKind, SessionState, Terminal, TerminalStatus},
};
use fleet_lazygit::root::{Lazygit, LazygitEvent};
use fleet_proto::{
    job::{JobRecord, JobStatus},
    request::RequestBody,
    response::ResponseBody,
    terminal::{Key, KeyAction, KeyEvent, Modifiers, ScrollCommand},
};
use fleet_ui_kit::{
    ActiveTheme, CellMetrics, ExitStrip, Icon, KeyHintRow, PrBadgeState, PrefixHint, ScrollPill,
    SplitLayout, StatusKind, TerminalMode as KitTerminalMode, TerminalTabStrip, Text,
};
use gpui::{
    AnyElement, App, ClipboardItem, Div, Entity, FocusHandle, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, SharedString, Size, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::{
    actions::{prefix, scroll},
    bridge::Bridge,
    dialogs::{self, Dialogs},
    presentation::session_glyph,
    state::{AppState, Overlay, Screen, TerminalMode},
    terminal::{
        MouseCell, SelectionGranularity, absolute_selection_at, cell_size, grid_modes, measure,
        surface, try_selection_text, viewport_base, zoom_bar,
    },
    views::{workspace_header::WorkspaceHeader, workspace_tabs},
};

/// What `ctrl-s c` and the `+` tab ask fleetd to type into a fresh login shell.
///
/// The PTY is always the user's `$SHELL -l` in the worktree path; the request's `command` is
/// what fleetd types at its first prompt, and it refuses an empty one. A shell tab wants no
/// program, so it asks for the one thing that leaves a clean prompt behind.
const SHELL_TAB_COMMAND: &str = "clear";

/// The Workspace screen.
pub(crate) struct WorkspaceScreen {
    model: Option<Model>,
    local: Rc<RefCell<Local>>,
    /// One live pane per worktree whose `fleet://` tab has been visited.
    ///
    /// Keyed by worktree because that is what the pane is *about*: a session comes and goes
    /// with sleep and wake, and re-opening the same worktree should find the same git view.
    /// Evicted in [`WorkspaceScreen::sync_panes`] when the daemon stops listing the worktree.
    panes: HashMap<WorktreeId, Pane>,
    /// One live view per opened agent thread, shared with the root's action listeners.
    agent_views: Rc<RefCell<AgentViews>>,
    /// Threads whose `@` completion listing has already been requested.
    agent_files: Rc<RefCell<HashSet<ThreadId>>>,
}

impl WorkspaceScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub(crate) fn new(_cx: &mut App) -> Self {
        Self {
            local: Rc::new(RefCell::new(Local::default())),
            model: None,
            panes: HashMap::new(),
            agent_views: Rc::new(RefCell::new(AgentViews::new())),
            agent_files: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    /// Whether the active tab's Fleet-drawn pane holds the keyboard.
    ///
    /// The shell asks before taking focus back for its own body element.
    #[must_use]
    pub(crate) fn pane_owns_keyboard(&self) -> bool {
        self.local.borrow().state.pane_focused
    }

    /// Composes the last synchronized surface into the frame's body.
    ///
    /// The root element tracks `focus`, which is what puts the `on_action` listeners below on
    /// gpui's dispatch path under the `Workspace > <mode>` contexts the shell has applied above.
    /// Preparation belongs to [`WorkspaceScreen::synchronize`]: this issues no request and
    /// reconciles no resource.
    pub(crate) fn render_prepared(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(model) = self.model.as_ref() else {
            return self.empty(focus, cx);
        };
        let focused = focus.is_focused(window);
        let pr = model.repo.as_ref().and_then(|repo| {
            model.branch_key.as_ref().and_then(|branch| {
                state
                    .read(cx)
                    .pr_badges
                    .get(&(repo.clone(), branch.clone()))
                    .copied()
            })
        });
        let agent_word = self.agent_header_word(state.read(cx), model);
        let header = (!model.zoomed).then(|| self.header(model, pr, agent_word));
        let tabs = (!model.zoomed).then(|| self.tab_strip(model, bridge, state, cx));
        let terminal = if model.agent.is_some() {
            self.agent_area(model, cx)
        } else {
            self.terminal_area(model, bridge, state, focus, focused, cx)
        };
        let watch = crate::views::watch_pane::render(
            &model.session,
            state,
            bridge,
            &self.local.borrow().state.watch_scroll,
            cx,
        );
        let body = if let Some(watch) = watch {
            let width = watch_width(f32::from(window.viewport_size().width));
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    SplitLayout::horizontal()
                        .leading(terminal)
                        .trailing(watch)
                        .trailing_size(px(width)),
                )
                .into_any_element()
        } else {
            terminal
        };
        let theme = cx.theme().clone();

        let mut root = div()
            .track_focus(focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.colors.bg);
        if model.zoomed {
            root = root.child(zoom_bar(&theme));
        }
        root = root
            .children(header)
            .children(tabs)
            .child(body)
            .children(model.exit_code.map(ExitStrip::new));

        self.with_keys(root, bridge, state).into_any_element()
    }

    /// The three handles every listener needs, cloned once per listener.
    fn handles(
        &self,
        bridge: &Bridge,
        state: &Entity<AppState>,
    ) -> (Rc<RefCell<Local>>, Bridge, Entity<AppState>) {
        (Rc::clone(&self.local), bridge.clone(), state.clone())
    }
}

// Workspace-only state stays outside the terminal controller.
#[derive(Default)]
struct WorkspaceState {
    tab_labels: workspace_tabs::TabLabels,
    popup_owned_size: bool,
    pr_requested: Vec<RepoId>,
    pr_tasks: HashMap<RepoId, gpui::Task<()>>,
    pane_focused: bool,
    pane_quit: Vec<WorktreeId>,
    pending_selection_scroll: Option<PendingSelectionScroll>,
    watch_scroll: UniformListScrollHandle,
}
type Local = TerminalSurface<WorkspaceState>;
