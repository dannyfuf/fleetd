//! Window coordination and ownership. Feature handlers live in continued implementations.

use crate::{
    bridge::Bridge,
    screens::{
        agent_popup::AgentPopup, board::BoardScreen, hub::HubScreen, jobs::JobsPanel,
        workspace::WorkspaceScreen,
    },
    shell::chrome,
    state::AppState,
};
use gpui::{App, AppContext, Context, Entity, FocusHandle, Focusable, Subscription, Task};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Instant};

mod actions;
mod agent;
mod board;
mod bootstrap;
mod daemon_lifecycle;
mod events;
mod focus;
mod observations;
mod quit_actions;
mod render;
mod routing;
#[cfg(test)]
mod tests;

use agent::AgentEnsureFlights;
pub use bootstrap::run;
use focus::{FocusOwnerKeys, focus_owner};

/// Whether the migration action is still valid at the instant its key reaches the root.
fn first_run_import_allowed(is_first_run: bool, state_file_exists: bool) -> bool {
    is_first_run && !state_file_exists
}

fn record_request_failure(state: &mut AppState, message: String) {
    state.sticky_error = Some(crate::state::StickyError {
        text: message,
        job: None,
        retryable: false,
    });
}

/// The root view: frame, routing, chrome, focus and the quit flow.
pub struct Shell {
    window: Option<gpui::AnyWindowHandle>,
    state: Entity<AppState>,
    bridge: Bridge,
    /// Focused while the Hub or the Workspace owns the keyboard.
    body_focus: FocusHandle,
    /// Focused while a dialog, the palette, the filter or the jobs panel is open, so that an
    /// overlay's key context really does shadow the screen behind it.
    overlay_focus: FocusHandle,
    /// Focused while the floating agent terminal is the topmost surface.
    agent_focus: FocusHandle,
    hub: HubScreen,
    /// The one board screen, lent to whichever surface is drawing the board this frame.
    ///
    /// The Hub's tab and the Workspace's `fleet://board` pane are never visible together and
    /// share a single [`crate::state::BoardState`] behind a scope, so they share the column
    /// lists, the scrollers and the filter editor too: two of them would mean two carets, two
    /// measured tile heights and a filter that only half the app can see (BOARD §8).
    board: BoardScreen,
    workspace: WorkspaceScreen,
    agent_popup: AgentPopup,
    /// Single-flight EnsureSession claims retained across popup hide/switch transitions.
    agent_ensures: AgentEnsureFlights,
    /// Generation gate and bounded input queue shared with the application-wide interceptor.
    focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
    local_files: observations::LocalFiles,
    user_home: Option<PathBuf>,
    jobs: JobsPanel,
    dialogs: Entity<crate::dialogs::ActiveDialog>,
    diagnostics: Entity<crate::views::doctor_view::DiagnosticView>,
    context_bar: Entity<chrome::Chrome>,
    status_bar: Entity<chrome::Chrome>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Shell {
    /// Builds the shell and installs the bridge, state observers, and owned background tasks.
    pub fn new(home: PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = Bridge::start(home.clone());
        Self::with_bridge(home, bridge, cx)
    }

    /// Builds the shell around a bridge the caller already holds.
    ///
    /// Tests hand it [`Bridge::closed`], whose refusals are delivered on the GPUI executor, so
    /// no reply depends on a background thread's wall-clock progress.
    fn with_bridge(home: PathBuf, bridge: Bridge, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|_| AppState::new(home, Instant::now()));
        // `idle` reports in-flight requests (`docs/TESTING-HARNESS.md` §2), and only the bridge
        // knows when one ends: it claims a slot on admission and releases it on the runtime
        // thread. Handing the counter to the projection is what makes `await idle` wait for a
        // daemon round trip instead of returning while one is outstanding.
        state.update(cx, |state, _| {
            state.harness.attach_bridge(
                bridge.in_flight_requests(),
                bridge.settle_counter(),
                bridge.idle_wake(),
            );
        });
        crate::views::watch_pane::sync(&state, &bridge, cx);
        let workspace = WorkspaceScreen::new(cx);
        let agent_popup = AgentPopup::new(cx);
        let focus_owner_keys = Rc::new(RefCell::new(FocusOwnerKeys::new(focus_owner(
            state.read(cx),
        ))));

        let subscriptions =
            focus::install_input_gates(&state, &bridge, agent_popup.input(), &focus_owner_keys, cx);

        let tasks = vec![
            Self::spawn_event_loop(&bridge, cx),
            Self::spawn_ticker(cx),
            Self::spawn_file_observer(cx),
        ];

        let mut hub = HubScreen::new(cx);
        hub.bind(&state, &bridge, cx);
        let board = BoardScreen::new(cx);
        let mut jobs = JobsPanel::new(cx);
        jobs.bind(&state, cx);
        let overlay_focus = cx.focus_handle();
        let dialogs = cx.new(|cx| {
            crate::dialogs::ActiveDialog::new(
                state.clone(),
                bridge.clone(),
                overlay_focus.clone(),
                cx,
            )
        });
        let diagnostics = cx.new(|cx| crate::views::doctor_view::DiagnosticView::new(&state, cx));
        let context_bar =
            cx.new(|cx| chrome::Chrome::new(state.clone(), chrome::ChromeKind::Context, cx));
        let status_bar =
            cx.new(|cx| chrome::Chrome::new(state.clone(), chrome::ChromeKind::Status, cx));
        Self {
            window: None,
            dialogs,
            diagnostics,
            local_files: observations::LocalFiles::default(),
            user_home: crate::presentation::home_dir(),
            context_bar,
            status_bar,
            state,
            bridge,
            body_focus: cx.focus_handle(),
            overlay_focus,
            agent_focus: cx.focus_handle(),
            hub,
            board,
            workspace,
            agent_popup,
            agent_ensures: AgentEnsureFlights::default(),
            focus_owner_keys,
            jobs,
            _subscriptions: subscriptions,
            _tasks: tasks,
        }
    }

    fn show_request_failure(&mut self, message: String, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            record_request_failure(state, message);
            cx.notify();
        });
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.body_focus.clone()
    }
}
