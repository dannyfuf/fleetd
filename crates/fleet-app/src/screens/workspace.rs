//! Workspace tabs, native panes, and watches around a shared terminal controller.
//!
//! Input is retained only while attaching a live terminal; disconnection drops it.
//! PTYs remain daemon-owned when this surface detaches.

use crate::terminal::surface::*;

mod actions;
mod agent;
mod changes;
mod chrome;
mod lifecycle;
mod model;
mod native;
mod terminal;
#[cfg(test)]
mod tests;

use actions::*;
pub(crate) use agent::requests::reopen_agent_tab;
use agent::*;
use chrome::*;
use model::*;
use native::*;
use terminal::*;

pub use model::Location;
pub(crate) use model::{host_unreachable, status_kind};

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc, time::Instant};

use fleet_core::{
    agents::{ThreadId, ThreadProjection},
    config::{NATIVE_BOARD, NATIVE_LAZYGIT},
    github::{PrChecks, PrReviewDecision, PrTab, derive_pr_state},
    ids::{HostId, RepoId, SessionId, TerminalId, WorktreeId},
    model::Worktree,
    sessions::{AgentActivity, Session, SessionKind, SessionState, Terminal, TerminalStatus},
};
use fleet_lazygit::root::{Lazygit, LazygitEvent};
use fleet_proto::{
    request::RequestBody,
    response::ResponseBody,
    snapshot::{HostStatus, LinkState},
    terminal::{Key, KeyAction, KeyEvent, Modifiers, ScrollCommand},
};
use fleet_ui_kit::{
    ActiveTheme, CellMetrics, ExitStrip, Icon, PrBadgeState, ScrollPill, SplitLayout, StatusKind,
    TerminalTabStrip, Text, Toast, ToastDuration,
};
use gpui::{
    AnyElement, App, ClipboardItem, Div, Entity, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, SharedString, Size, Subscription,
    Task, UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::{
    actions::{prefix, scroll},
    bridge::Bridge,
    dialogs::{self, Dialogs},
    presentation::session_glyph,
    screens::board::BoardScreen,
    state::{AppState, BoardScope, Overlay, Screen, TerminalMode, dwell_for},
    terminal::{
        MouseCell, SelectionGranularity, absolute_selection_at, cell_size, measure, surface,
        try_selection_text, viewport_base, zoom_bar,
    },
    views::{prefix_menu::PrefixSurface, workspace_tabs},
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
    /// One live git pane per worktree whose `fleet://lazygit` tab has been visited.
    ///
    /// Keyed by worktree because that is what the pane is *about*: a session comes and goes
    /// with sleep and wake, and re-opening the same worktree should find the same git view.
    /// Evicted in [`WorkspaceScreen::sync_panes`] when the daemon stops listing the worktree.
    panes: HashMap<WorktreeId, Pane>,
    /// One live view per opened agent thread, shared with the root's action listeners.
    agent_views: Rc<RefCell<AgentViews>>,
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
    /// The root element tracks `focus` — unless the board pane is drawing, which brings a
    /// focus-tracking root of its own — which is what puts the `on_action` listeners below on
    /// gpui's dispatch path under the `Workspace > <mode>` contexts the shell has applied above.
    /// Preparation belongs to [`WorkspaceScreen::synchronize`]: this issues no request and
    /// reconciles no resource.
    ///
    /// `board` is the shell's one board screen, lent for the frame: a `fleet://board` tab draws
    /// the very same view the Hub's board tab does, and the two are never on screen together.
    pub(crate) fn render_prepared(
        &mut self,
        board: &mut BoardScreen,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(model) = self.model.as_ref() else {
            // The shell can still enter `Workspace > Prefix` while a snapshot temporarily has
            // no active session. Keep the native-agent listeners mounted so `ctrl-s a` reaches
            // `create_thread`, which explains the refusal instead of consuming the action.
            return self
                .with_agent_actions(self.empty(focus, cx), bridge, state)
                .into_any_element();
        };
        let focused = focus.is_focused(window);
        // The session's identity, git state and pull request are the title bar's breadcrumb
        // now (§3.6); the Workspace itself starts at its tab strip.
        let tabs = (!model.zoomed).then(|| self.tab_strip(model, bridge, state, cx));
        // The board pane brings its own focus-tracking root, exactly as the Hub's board tab
        // does: tracking the same handle twice would put two nodes in gpui's dispatch tree for
        // one focus id. The prefix listeners below stay reachable either way — every element
        // is a dispatch node, focus-tracking or not, so `ctrl-s` still leaves the pane.
        let board_pane = self.draws_board(model);
        let terminal = if model.agent.is_some() {
            self.agent_area(model, cx)
        } else {
            self.terminal_area(
                model,
                PaneCtx {
                    board,
                    state,
                    bridge,
                    focus,
                },
                focused,
                window,
                cx,
            )
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
        let body = match self.changes_panel(model, bridge, state, cx) {
            Some(panel) => div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(body),
                )
                .child(panel)
                .into_any_element(),
            None => body,
        };
        let theme = cx.theme().clone();

        let mut root = div()
            .when(!board_pane, |root| root.track_focus(focus))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.colors.bg);
        if model.zoomed {
            root = root.child(zoom_bar(&theme));
        }
        // The ⌃S command menu floats over the bottom of the screen, just above the status bar,
        // whichever tab — terminal, Fleet-drawn pane or agent thread — holds the prefix.
        let prefix_menu = self.local.borrow().prefix_menu.render(state, cx);
        root = root
            .relative()
            .children(tabs)
            .child(body)
            .children(model.exit_code.map(ExitStrip::new))
            .children(prefix_menu);

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
    /// The worktree whose git state was last asked for; cleared when the refresh is due.
    git_inspected: Option<WorktreeId>,
    /// The inspection in flight, then its refresh timer.
    git_task: Option<gpui::Task<()>>,
    pane_focused: bool,
    pane_quit: Vec<WorktreeId>,
    board_claim: Option<BoardClaim>,
    pending_selection_scroll: Option<PendingSelectionScroll>,
    watch_scroll: UniformListScrollHandle,
    /// The git worker reading the open Changes panel's worktree, and the loop draining it.
    changes: Option<changes::ChangesSource>,
    changes_scroll: UniformListScrollHandle,
}
type Local = TerminalSurface<WorkspaceState>;
