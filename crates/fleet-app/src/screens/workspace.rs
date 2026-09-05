//! The Workspace: one session's terminals, its header, tab strip and grid (UX-SPEC §3.6).
//!
//! # What this screen owns
//!
//! Everything drawn between the shell's context bar and its status bar while a session is open,
//! and — because no other module can — the two behaviours `docs/APP-CONTRACTS.md` §6 assigns to
//! it by name:
//!
//! * **Keys reach the PTY only in Terminal mode.** Clipboard, viewport and prefix bindings dispatch actions
//!   first; every other keystroke falls through gpui's binding pass into this element's
//!   `on_key_down`, becomes a [`KeyEvent`], and reaches the daemon's mode-aware encoder. A key
//!   typed in `Prefix` or `Scroll` is *dropped here*, never forwarded: those modes exist precisely
//!   so that `x` closes a tab instead of typing an `x`.
//! * **Keys are dropped, never buffered, while the daemon is gone** (§3.12 C). A buffer that
//!   replayed twenty keystrokes into a shell the moment it reconnected would be a hazard, not a
//!   convenience. The one buffer that does exist is the opposite case: keys and pastes produced
//!   *while attaching* are held in order until the first frame lands, because that PTY is alive.
//!
//! # The attach lifecycle
//!
//! Attachment follows the rendered terminal, not the user's intent: whenever the active terminal
//! of the active session changes, the previous one is detached and the new one attached with the
//! size the grid was last laid out into. Leaving for the Hub detaches too — and the session
//! keeps running, because it belongs to `fleetd` and never to this window (ARCHITECTURE §1).
//!
//! # Geometry
//!
//! The PTY size is a function of the painted area. [`crate::terminal_element::measure`] reports
//! the terminal area's bounds every frame and a change is forwarded as `ResizeTerminal`; nothing
//! guesses from the window size, so the chrome above and below the grid cannot desynchronise it.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::{
    config::Agent,
    github::{PrChecks, PrReviewDecision, PrState, PrTab, derive_pr_state},
    ids::{RepoId, SessionId, TerminalId, WorktreeId},
    model::Worktree,
    sessions::{Session, SessionKind, SessionState, Terminal, TerminalStatus},
};
use fleet_lazygit::root::{Lazygit, LazygitEvent};
use fleet_proto::{
    job::{JobRecord, JobStatus},
    request::RequestBody,
    response::ResponseBody,
    terminal::{Key, KeyAction, KeyEvent, Modifiers, ScrollCommand, WheelEvent},
};
use fleet_ui_kit::{
    ActiveTheme, CellMetrics, ExitStrip, Icon, KeyHintRow, PrBadgeState, PrefixHint, ScrollPill,
    SplitLayout, StatusKind, TerminalGrid, TerminalMode as KitTerminalMode, TerminalTabStrip, Text,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Div, Entity, FocusHandle, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, SharedString, Size,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::{
    actions::{fleet, prefix, scroll},
    bridge::Bridge,
    dialogs::{self, Dialogs},
    state::{AppState, MirrorGrid, Overlay, Screen, TerminalMode},
    terminal_element::{
        AbsoluteCellPoint, AbsoluteCellSelection, CachedGridRow, CellPoint, GRID_PADDING,
        SelectionGranularity, WheelAccumulator, absolute_selection_at, absolute_selection_text,
        cached_grid_row, cell_at_position, cell_size, extend_absolute_selection, grid_cursor,
        grid_modes, grid_rows, grid_size, line_selection, measure, selection_text, viewport_base,
        viewport_cell_selection, viewport_last, zoom_bar,
    },
    views::{workspace_header::WorkspaceHeader, workspace_tabs},
};

/// The size a terminal is attached at before the grid has ever been laid out.
///
/// The first measured frame replaces it, so this is only ever the argument of the very first
/// `AttachTerminal`; 80 × 24 is the size every program already copes with.
const FALLBACK_GRID: (u16, u16) = (80, 24);

/// The read-only trailing region follows the current window width.
#[must_use]
pub fn watch_width(window_width: f32) -> f32 {
    (window_width * 0.4).clamp(360.0, 640.0)
}

/// What `ctrl-s c` and the `+` tab ask fleetd to type into a fresh login shell.
///
/// The PTY is always the user's `$SHELL -l` in the worktree path; the request's `command` is
/// what fleetd types at its first prompt, and it refuses an empty one. A shell tab wants no
/// program, so it asks for the one thing that leaves a clean prompt behind.
const SHELL_TAB_COMMAND: &str = "clear";

/// How many scrollback lines one selection may remember.
///
/// A selection is bounded by what the user scrolled over, so this only exists so a held `k` on
/// a million-line scrollback cannot grow the map without limit.
const SELECTION_LINE_CAP: usize = 100_000;

/// Maximum viewport rows retained for mouse-selection copy after they leave the mirror grid.
const MOUSE_ROW_CACHE_CAP: usize = 5_000;

/// The six prefix keys the delayed hint strip lists (§3.6).
fn prefix_hints() -> KeyHintRow {
    KeyHintRow::new()
        .key("s", "hub")
        .key("1-9", "tab")
        .key("c", "new")
        .key("x", "close")
        .key("[", "scroll")
        .key("w", "last session")
}

/// Everything the screen remembers between frames.
///
/// It lives behind an [`Rc`] because every listener the screen installs must be `'static` and
/// therefore cannot borrow the screen itself.
#[derive(Debug, Default)]
struct Local {
    /// The terminal this client is attached to.
    attached: Option<TerminalId>,
    /// The [`AppState::link_generation`] that attachment was made under.
    ///
    /// A reconnect replaces both ends of the socket, so the daemon's `attached` set is empty
    /// again and `TerminalFrame`s for this terminal are dropped on its side — while
    /// `TerminalKey` still reaches the PTY, which is what made the grid look frozen but alive.
    /// Comparing generations is what re-issues the attach.
    attached_generation: u64,
    /// The `cols × rows` the daemon was last told about, per terminal.
    sizes: HashMap<TerminalId, (u16, u16)>,
    /// The pixel area the grid was last laid out into.
    area: Bounds<Pixels>,
    /// Measured inner grid bounds and cells, keyed to their terminal.
    geometry: Option<(TerminalId, Bounds<Pixels>, CellMetrics)>,
    wheel: WheelAccumulator,
    /// Terminal input produced before the first frame landed, retained in exact input order.
    pending: Vec<PendingInput>,
    /// The anchor of a scroll-mode selection, as an **absolute** scrollback line.
    ///
    /// Viewport rows are not stable: a page of scrolling replaces every row's content while
    /// leaving its number alone, so an anchor kept that way silently re-points at whatever
    /// moved under it. See [`crate::terminal_element::viewport_base`].
    anchor: Option<u64>,
    /// The scroll-mode caret, as an absolute scrollback line. `j` / `k` move it and a
    /// selection extends to it.
    caret: u64,
    /// Scrollback identity epoch in which the current scroll-mode selection was created.
    anchor_history_epoch: Option<u64>,
    /// Every line this client has painted since the selection was anchored, by absolute
    /// scrollback line.
    ///
    /// The daemon mirrors only the viewport, so this is the only place a multi-page selection
    /// can be assembled from. It is dropped the moment the selection ends.
    history: BTreeMap<u64, String>,
    /// The app-owned mouse selection. It exists independently of terminal modes, so a plain
    /// left drag still selects while an alternate-screen program has mouse reporting enabled.
    mouse_selection: Option<MouseSelection>,
    /// Recent rows keyed by terminal and absolute scrollback line for mouse-selection copy.
    row_caches: HashMap<TerminalId, TerminalRowCache>,
    /// Whether the 400 ms prefix-hint timer is already running for this prefix.
    hint_armed: bool,
    /// Whether that timer has fired.
    hint_visible: bool,
    /// Repositories whose pull requests have already been asked for.
    pr_requested: Vec<RepoId>,
    /// Whether the Fleet-drawn pane of the active tab owns the keyboard this frame.
    ///
    /// The shell reads it through [`WorkspaceScreen::pane_owns_keyboard`]: its own root
    /// re-focuses itself whenever it is not focused, and that would fight the pane every frame.
    pane_focused: bool,
    /// Panes that asked to be left and must be dropped on the next frame.
    ///
    /// Leaving shuts the pane's `git` worker down, so the entity cannot be reused: the next
    /// activation has to build a fresh one. It is recorded rather than dropped on the spot
    /// because the request arrives from inside the pane's own event subscription.
    pane_quit: Vec<WorktreeId>,
    /// Read-only log following position, independent from terminal scrolling.
    watch_scroll: UniformListScrollHandle,
}

impl Local {
    /// The size to attach `terminal` at: the measured one, else the last one, else the fallback.
    fn size_for(&self, terminal: TerminalId, cell: Size<Pixels>) -> (u16, u16) {
        if self.area.size.width <= px(0.0) || self.area.size.height <= px(0.0) {
            return self.sizes.get(&terminal).copied().unwrap_or(FALLBACK_GRID);
        }
        grid_size(self.area.size, cell)
    }

    /// Clears both mouse and Scroll-mode selections, returning whether anything changed.
    fn clear_selections(&mut self) -> bool {
        let mouse_changed = self.mouse_selection.take().is_some();
        let changed = self.anchor.take().is_some() || mouse_changed;
        self.anchor_history_epoch = None;
        self.history.clear();
        self.row_caches.clear();
        changed
    }
}

/// A mouse drag anchored in absolute scrollback coordinates.
#[derive(Clone, Copy, Debug)]
struct MouseSelection {
    anchor: AbsoluteCellPoint,
    head: AbsoluteCellPoint,
    initial: AbsoluteCellSelection,
    initiating: AbsoluteCellPoint,
    granularity: SelectionGranularity,
    history_epoch: u64,
    cols: u16,
    alt_screen: bool,
    dragging: bool,
    selected: bool,
}

/// A terminal's bounded cache of rows that have appeared in its mirrored viewport.
#[derive(Debug)]
struct TerminalRowCache {
    cols: u16,
    alt_screen: bool,
    history_epoch: u64,
    last_seq: u64,
    last_viewport_base: u64,
    rows: BTreeMap<u64, CachedGridRow>,
}

/// One ordered input operation waiting for the terminal's first frame.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingInput {
    Key(KeyEvent),
    Paste(String),
}

impl PendingInput {
    fn into_request(self, terminal: TerminalId) -> RequestBody {
        match self {
            Self::Key(key) => RequestBody::TerminalKey { terminal, key },
            Self::Paste(text) => RequestBody::PasteTerminal { terminal, text },
        }
    }
}

fn drain_pending_requests(
    pending: &mut Vec<PendingInput>,
    terminal: TerminalId,
) -> Vec<RequestBody> {
    pending
        .drain(..)
        .map(|input| input.into_request(terminal))
        .collect()
}

fn history_epoch_changed(selection_epoch: Option<u64>, current_epoch: Option<u64>) -> bool {
    selection_epoch != current_epoch
}

fn invalidate_history_epoch(local: &mut Local, current_epoch: Option<u64>) -> bool {
    let mut changed = false;
    if local.mouse_selection.is_some_and(|selection| {
        history_epoch_changed(Some(selection.history_epoch), current_epoch)
    }) {
        local.mouse_selection = None;
        changed = true;
    }
    if local.anchor.is_some() && history_epoch_changed(local.anchor_history_epoch, current_epoch) {
        local.anchor = None;
        local.anchor_history_epoch = None;
        local.history.clear();
        changed = true;
    }
    if local
        .row_caches
        .values()
        .any(|cache| Some(cache.history_epoch) != current_epoch)
    {
        local.row_caches.clear();
        changed = true;
    }
    changed
}

fn visible_selected_rows(
    viewport_base: u64,
    viewport_rows: u16,
    selection: Option<(u64, u64)>,
) -> Vec<(usize, u64)> {
    let Some((first, last)) = selection else {
        return Vec::new();
    };
    if viewport_rows == 0 {
        return Vec::new();
    }
    let (first, last) = (first.min(last), first.max(last));
    let viewport_last = viewport_base + u64::from(viewport_rows - 1);
    let first = first.max(viewport_base);
    let last = last.min(viewport_last);
    if first > last {
        return Vec::new();
    }
    (first..=last)
        .filter_map(|line| {
            usize::try_from(line - viewport_base)
                .ok()
                .map(|row| (row, line))
        })
        .collect()
}

fn cache_selected_grid_rows(
    cache: &mut TerminalRowCache,
    grid: &MirrorGrid,
    viewport_base: u64,
    selection: Option<(u64, u64)>,
) {
    let selected_rows = visible_selected_rows(viewport_base, grid.rows, selection);
    if selected_rows.is_empty() {
        return;
    }
    let (first, last) =
        selection.map_or((0, 0), |(first, last)| (first.min(last), first.max(last)));
    cache.rows.retain(|line, _| *line >= first && *line <= last);
    let frame_changed = cache.last_seq != grid.seq || cache.last_viewport_base != viewport_base;
    for (row, line) in selected_rows {
        if !frame_changed && cache.rows.contains_key(&line) {
            continue;
        }
        if let Some(cached) = cached_grid_row(grid, row) {
            cache.rows.insert(line, cached);
        }
    }
    while cache.rows.len() > MOUSE_ROW_CACHE_CAP {
        cache.rows.pop_first();
    }
    cache.last_seq = grid.seq;
    cache.last_viewport_base = viewport_base;
}

/// One Fleet-drawn tab, kept alive across frames.
///
/// The view holds a `fleet-git` worker thread and a cached diff model, so it is created once
/// per worktree and reused: rebuilding it on every activation would re-run `git status`,
/// `git log` and `git branch` and lose the cursor the user left in the Files pane.
struct Pane {
    view: Entity<Lazygit>,
    /// Repaint on notify and the `Quit` handler. Dropping these ends both.
    _subscriptions: Vec<Subscription>,
}

/// The Workspace screen.
pub struct WorkspaceScreen {
    local: Rc<RefCell<Local>>,
    /// One live pane per worktree whose `fleet://` tab has been visited.
    ///
    /// Keyed by worktree because that is what the pane is *about*: a session comes and goes
    /// with sleep and wake, and re-opening the same worktree should find the same git view.
    /// Evicted in [`WorkspaceScreen::sync_panes`] when the daemon stops listing the worktree.
    panes: HashMap<WorktreeId, Pane>,
}

impl WorkspaceScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self {
            local: Rc::new(RefCell::new(Local::default())),
            panes: HashMap::new(),
        }
    }

    /// Whether the active tab's Fleet-drawn pane holds the keyboard.
    ///
    /// The shell asks before taking focus back for its own body element.
    #[must_use]
    pub fn pane_owns_keyboard(&self) -> bool {
        self.local.borrow().pane_focused
    }

    /// Renders the Workspace into the frame's body.
    ///
    /// The root element tracks `focus`, which is what puts the `on_action` listeners below on
    /// gpui's dispatch path under the `Workspace > <mode>` contexts the shell has applied above.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        // Cleared before anything can set it: the screen with no session behind it draws no
        // pane, so the shell must own the keyboard again.
        self.local.borrow_mut().pane_focused = false;
        let Some(session) = state.read(cx).active_session().cloned() else {
            self.local.borrow_mut().clear_selections();
            return self.empty(focus, cx);
        };
        let model = Model::build(state.read(cx), &session);
        let cell = cell_size(cx.theme());

        self.reconcile(&model, bridge, cell);
        self.cache_viewport(state, cx);
        self.track_selection(state, cx);
        self.arm_prefix_hint(&model, state, cx);
        self.lookup_pr(&model, bridge, state, cx);
        self.sync_panes(&model, bridge, state, window, cx);

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
        let header = (!model.zoomed).then(|| self.header(&model, pr, cx));
        let tabs = (!model.zoomed).then(|| self.tab_strip(&model, &session, bridge, state, cx));
        let terminal = self.terminal_area(&model, bridge, state, focus, focused, cx);
        let watch = crate::views::watch_pane::render(
            &session.id,
            state,
            bridge,
            &self.local.borrow().watch_scroll,
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

    /// The screen with no session behind it, which only happens between two snapshots.
    fn empty(&self, focus: &FocusHandle, cx: &mut App) -> AnyElement {
        let theme = cx.theme();
        div()
            .track_focus(focus)
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.colors.bg)
            .child(Text::ui("no session").muted())
            .into_any_element()
    }

    // ------------------------------------------------------------------ lifecycle

    /// Attaches, detaches and flushes so the daemon always mirrors what is on screen.
    fn reconcile(&self, model: &Model, bridge: &Bridge, cell: Size<Pixels>) {
        let mut local = self.local.borrow_mut();
        local.wheel.reconcile(model.terminal);
        // A new link means the daemon forgot every attachment, so the terminal on screen has
        // to be claimed again even though it did not change.
        let relinked = local.attached_generation != model.link_generation;
        // A Fleet-drawn tab has no PTY on the daemon side: attaching to it would answer
        // `NotFound`, and detaching from the previous tab still has to happen.
        let target = model.attach_target();
        let terminal_changed = local.attached != target;
        if terminal_changed || relinked {
            if let Some(previous) = local.attached.take()
                && !relinked
            {
                // After a relink the old connection — and its attachment — is already gone;
                // detaching would name a terminal this connection never claimed.
                bridge.send(RequestBody::DetachTerminal { terminal: previous });
            }
            local.pending.clear();
            local.anchor = None;
            local.anchor_history_epoch = None;
            local.history.clear();
            local.row_caches.clear();
            if terminal_changed {
                local.mouse_selection = None;
            }
            if let Some(terminal) = target {
                let (cols, rows) = local.size_for(terminal, cell);
                local.sizes.insert(terminal, (cols, rows));
                bridge.send(RequestBody::AttachTerminal {
                    terminal,
                    cols,
                    rows,
                });
            }
            local.attached = target;
            local.attached_generation = model.link_generation;
        }

        // Primary and alternate screens do not share coordinates, and changing the column count
        // changes the meaning of a cell address. The daemon advances `history_epoch` whenever
        // retained absolute row identities may have rebased.
        if model.primed {
            if local.mouse_selection.is_some_and(|selection| {
                selection.alt_screen != model.alt_screen || Some(selection.cols) != model.grid_cols
            }) {
                local.mouse_selection = None;
                local.row_caches.clear();
            }
            invalidate_history_epoch(&mut local, model.history_epoch);
            if local.row_caches.values().any(|cache| {
                Some(cache.cols) != model.grid_cols || cache.alt_screen != model.alt_screen
            }) {
                local.row_caches.clear();
            }
        }

        // §3.6 "Attaching": input produced before the first frame is flushed in order once the
        // mirror is primed, and dropped if the terminal went away in the meantime.
        if model.primed && !local.pending.is_empty() {
            match target {
                Some(terminal) => {
                    for request in drain_pending_requests(&mut local.pending, terminal) {
                        bridge.send(request);
                    }
                }
                None => local.pending.clear(),
            }
        }
    }

    /// Retains the visible portion of an active selection by absolute line.
    ///
    /// The daemon mirror only contains rows currently on-screen. Rows are cloned only while a
    /// mouse or Scroll-mode selection exists, only when covered by that selection, and only when
    /// the frame/viewport changed or the selection newly covers an uncached row. Repeated renders
    /// of the same state do no cell cloning. [`Self::reconcile`] clears the cache on an epoch or
    /// coordinate-space change before this method can insert the current rows.
    fn cache_viewport(&self, state: &Entity<AppState>, cx: &App) {
        let app = state.read(cx);
        let Some(terminal) = app
            .active_session()
            .and_then(|session| session.active_terminal)
        else {
            return;
        };
        let Some(grid) = app.grids.get(&terminal).filter(|grid| grid.primed) else {
            return;
        };
        let base = viewport_base(grid);
        let cols = grid.cols;
        let alt_screen = grid.modes.alt_screen;
        let history_epoch = grid.viewport.history_epoch;

        let mut local = self.local.borrow_mut();
        let selection = local.mouse_selection.map_or_else(
            || local.anchor.map(|anchor| (anchor, local.caret)),
            |selection| Some((selection.anchor.line, selection.head.line)),
        );
        if visible_selected_rows(base, grid.rows, selection).is_empty() {
            return;
        }
        let reset = local.row_caches.get(&terminal).is_some_and(|cache| {
            cache.cols != cols
                || cache.alt_screen != alt_screen
                || cache.history_epoch != history_epoch
        });
        if reset {
            local.row_caches.remove(&terminal);
        }
        let cache = local
            .row_caches
            .entry(terminal)
            .or_insert_with(|| TerminalRowCache {
                cols,
                alt_screen,
                history_epoch,
                last_seq: 0,
                last_viewport_base: u64::MAX,
                rows: BTreeMap::new(),
            });
        cache_selected_grid_rows(cache, grid, base, selection);
    }

    /// Keeps the Scroll-mode caret inside the viewport and records what a live selection covers.
    ///
    /// The recording is what makes a selection survive scrolling: the daemon mirrors only the
    /// rows on screen, so the lines that scroll out of the viewport exist nowhere else once the
    /// next frame replaces them.
    fn track_selection(&self, state: &Entity<AppState>, cx: &App) {
        let app = state.read(cx);
        let Some(grid) = app.active_grid() else {
            return;
        };
        let (Some(base), Some(bottom)) = (Some(viewport_base(grid)), viewport_last(grid)) else {
            return;
        };
        let mut local = self.local.borrow_mut();
        local.caret = local.caret.clamp(base, bottom);
        let Some(anchor) = local.anchor else {
            local.history.clear();
            return;
        };
        let (first, last) = if anchor <= local.caret {
            (anchor, local.caret)
        } else {
            (local.caret, anchor)
        };
        for row in 0..grid.rows {
            let line = base + u64::from(row);
            if line < first || line > last || local.history.len() >= SELECTION_LINE_CAP {
                continue;
            }
            local
                .history
                .insert(line, grid.row_text(row).trim_end().to_owned());
        }
    }

    /// Starts the 400 ms timer that reveals the prefix hint, once per prefix (§3.6).
    fn arm_prefix_hint(&self, model: &Model, state: &Entity<AppState>, cx: &mut App) {
        if model.mode != TerminalMode::Prefix {
            let mut local = self.local.borrow_mut();
            local.hint_armed = false;
            local.hint_visible = false;
            return;
        }
        if self.local.borrow().hint_armed {
            return;
        }
        self.local.borrow_mut().hint_armed = true;
        let delay = Duration::from_millis(cx.theme().motion.prefix_hint_delay);
        let local = Rc::clone(&self.local);
        let state = state.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            let reveal = {
                let mut local = local.borrow_mut();
                let armed = local.hint_armed;
                local.hint_visible = armed;
                armed
            };
            if reveal {
                state.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }

    /// Fills the shared branch → pull-request cache once per repository when Hub has not.
    fn lookup_pr(&self, model: &Model, bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
        let (Some(repo), Some(branch)) = (model.repo.clone(), model.branch_key.clone()) else {
            return;
        };
        {
            let local = self.local.borrow();
            if state
                .read(cx)
                .pr_badges
                .contains_key(&(repo.clone(), branch.clone()))
                || local.pr_requested.contains(&repo)
            {
                return;
            }
        }
        self.local.borrow_mut().pr_requested.push(repo.clone());
        let reply = bridge.request(RequestBody::ListPullRequests {
            repo: Some(repo),
            context: None,
            tab: PrTab::Mine,
            force: false,
        });
        let state = state.clone();
        cx.spawn(async move |cx| {
            let Ok(Ok(ResponseBody::PullRequests(slices))) = reply.recv().await else {
                return;
            };
            state.update(cx, |app, cx| {
                for slice in &slices {
                    for pr in &slice.prs {
                        app.pr_badges.insert(
                            (pr.repo_id.clone(), pr.head_ref_name.clone()),
                            (
                                pr.number,
                                badge_state(pr.is_draft, pr.checks, pr.review_decision),
                            ),
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------ Fleet-drawn tabs

    /// Creates, focuses, releases and evicts the Fleet-drawn panes.
    ///
    /// Creation is lazy — the first time a `fleet://` tab is actually selected — because a pane
    /// starts a `git` worker thread and immediately runs a full snapshot of the repository.
    /// Opening a worktree must not pay for a tab the user never looks at.
    fn sync_panes(
        &mut self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // A pane that emitted `Quit` shut its own `git` worker down and can never come back to
        // life, so it is dropped and rebuilt on the next activation.
        let left: Vec<WorktreeId> = std::mem::take(&mut self.local.borrow_mut().pane_quit);
        for worktree in left {
            self.panes.remove(&worktree);
        }
        // The daemon's snapshot is authoritative: a worktree it no longer lists can never be
        // painted again, so its pane — and the git worker behind it — goes with it.
        if let Some(snapshot) = state.read(cx).snapshot.as_ref() {
            let live: std::collections::HashSet<&WorktreeId> =
                snapshot.worktrees.iter().map(|entry| &entry.id).collect();
            self.panes.retain(|worktree, _| live.contains(worktree));
        }

        let active = model.native.then(|| model.worktree.clone()).flatten();
        let Some((worktree, path)) = active else {
            // Not on a Fleet-drawn tab: nothing owns the keyboard on our behalf, and every
            // pane that exists is idle in the background.
            self.local.borrow_mut().pane_focused = false;
            for pane in self.panes.values() {
                let view = pane.view.clone();
                view.update(cx, |pane, cx| pane.set_active(false, window, cx));
            }
            return;
        };

        if !self.panes.contains_key(&worktree) {
            let view = cx.new(|cx| Lazygit::embedded(path, cx));
            let mut subscriptions = Vec::new();
            // The pane is a separate entity, so the shell has to be told to repaint when its
            // own state moves — a `git` result arriving, a cursor moving, an overlay opening.
            let repaint = state.clone();
            subscriptions.push(cx.observe(&view, move |_view, cx| {
                repaint.update(cx, |_, cx| cx.notify());
            }));
            // `q` inside the pane means "leave this tab", not "quit Fleet": the tab the user
            // came from is the one they want back.
            let (quit_local, quit_bridge, quit_state) = self.handles(bridge, state);
            let quit_worktree = worktree.clone();
            subscriptions.push(cx.subscribe(&view, move |_view, event, cx| match event {
                LazygitEvent::Quit => {
                    quit_local
                        .borrow_mut()
                        .pane_quit
                        .push(quit_worktree.clone());
                    let previous = neighbour_terminal(&quit_state, -1, cx);
                    select_terminal(&quit_local, &quit_bridge, &quit_state, previous, cx);
                }
            }));
            self.panes.insert(
                worktree.clone(),
                Pane {
                    view,
                    _subscriptions: subscriptions,
                },
            );
        }

        // A Fleet overlay — a dialog, the palette, the filter, the jobs sheet — owns the
        // keyboard over every screen (§2.8), so the pane must let go while one is open and
        // take the keyboard back, unasked, when it closes.
        let owns = !model.overlay_open;
        self.local.borrow_mut().pane_focused = owns;
        for (candidate, pane) in &self.panes {
            let active = owns && candidate == &worktree;
            let view = pane.view.clone();
            view.update(cx, |pane, cx| pane.set_active(active, window, cx));
        }
    }

    // ------------------------------------------------------------------ pieces

    fn header(&self, model: &Model, pr: Option<(u64, PrBadgeState)>, cx: &mut App) -> AnyElement {
        let mut header = WorkspaceHeader::new(model.title.clone())
            .status(model.status)
            .keep_alive(model.keep_alive.iter().cloned())
            .jobs(model.running_jobs, model.failed_jobs)
            // §3.6: the frame's VT modes are the only thing that explains why a documented key
            // behaves differently — no scrollback in alt-screen, the app owning drag-select
            // under mouse reporting. They are badged here, in reserved chrome, and never over
            // the grid, whose cells are live output. Zero-suppressed.
            .modes(model.modes.iter().copied())
            .waking(model.waking);
        if let Some(repo) = &model.repo {
            header = header.repo(SharedString::from(repo.to_string()));
        }
        if let Some((host, reachable)) = &model.host {
            header = header.host(host.clone(), *reachable);
        }
        if let Some((number, badge)) = pr {
            header = header.pr(number, badge);
        }
        let _unused = cx;
        header.into_any_element()
    }

    fn tab_strip(
        &self,
        model: &Model,
        session: &Session,
        bridge: &Bridge,
        state: &Entity<AppState>,
        cx: &mut App,
    ) -> AnyElement {
        let tabs = workspace_tabs::tabs(session, model.terminal, &state.read(cx).renamed_terminals);
        let active = model
            .terminal
            .and_then(|terminal| workspace_tabs::position_of(session, terminal))
            .unwrap_or(0);
        let _unused = cx;
        let (new_session, new_bridge, new_state) = (session.clone(), bridge.clone(), state.clone());
        let new_local = Rc::clone(&self.local);
        let (select_local, select_bridge, select_state) =
            (Rc::clone(&self.local), bridge.clone(), state.clone());
        TerminalTabStrip::new(tabs)
            .active(active)
            // Mouse parity for `ctrl-s 1`-`9` (§3.6). It is also the only way a mouse-first
            // user reaches a Fleet-drawn tab, which is why the strip is finally wired.
            .on_select(move |position, _window, cx| {
                let Some(terminal) = select_state
                    .read(cx)
                    .active_session()
                    .and_then(|session| workspace_tabs::terminal_at(session, position))
                else {
                    return;
                };
                select_terminal(
                    &select_local,
                    &select_bridge,
                    &select_state,
                    Some(terminal),
                    cx,
                );
            })
            // Mouse parity for `ctrl-s c` (§3.6): the `+` is the same request.
            .on_new(move |_window, cx| {
                request_shell_tab(&new_local, &new_session, &new_bridge, &new_state, cx);
            })
            .into_any_element()
    }

    /// The grid, its overlays, and the invisible element that measures it.
    ///
    /// A Fleet-drawn tab replaces the whole band: no grid, no scroll pill, no measuring
    /// element, because none of them describe a pane that is not a terminal.
    fn terminal_area(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        focus: &FocusHandle,
        focused: bool,
        cx: &App,
    ) -> AnyElement {
        if model.native {
            return self.pane_area(model, cx);
        }
        let theme = cx.theme();
        let app = state.read(cx);
        let mirror = app.active_grid();
        let selection = mirror.and_then(|grid| {
            let local = self.local.borrow();
            local
                .mouse_selection
                .filter(|selection| selection.selected)
                .and_then(|selection| {
                    viewport_cell_selection(grid, selection.anchor, selection.head)
                })
                .or_else(|| {
                    local
                        .anchor
                        .and_then(|anchor| line_selection(grid, anchor, local.caret))
                })
        });
        let hint_visible = self.local.borrow().hint_visible;

        let grid: AnyElement = match mirror.filter(|grid| grid.primed) {
            Some(grid) => {
                let resize_local = Rc::clone(&self.local);
                let geometry_local = Rc::clone(&self.local);
                let resize_bridge = bridge.clone();
                let resize_state = state.clone();
                let resize_terminal = model.terminal;
                let mut painted = TerminalGrid::new(grid_rows(grid, theme))
                    .id("workspace-terminal-grid")
                    .cursor(grid_cursor(grid, focused))
                    .focused(focused)
                    // Only so the grid can suppress its scroll overlays in alt-screen: the
                    // badges themselves are drawn in the session header, never over the cells.
                    .modes(grid_modes(&grid.modes))
                    .padding(px(GRID_PADDING))
                    .scrollback(grid.viewport.offset, grid.viewport.scrollback_len)
                    .frame_size(usize::from(grid.cols), usize::from(grid.rows))
                    .on_geometry(move |bounds, metrics| {
                        if let Some(terminal) = resize_terminal {
                            geometry_local.borrow_mut().geometry =
                                Some((terminal, bounds, metrics));
                        }
                    })
                    .on_resize(move |cols, rows, _window, cx| {
                        let Some(terminal) = resize_terminal else {
                            return;
                        };
                        let cols = u16::try_from(cols).unwrap_or(u16::MAX);
                        let rows = u16::try_from(rows).unwrap_or(u16::MAX);
                        let mut local = resize_local.borrow_mut();
                        let selection_cleared = local
                            .mouse_selection
                            .is_some_and(|selection| selection.cols != cols);
                        if selection_cleared {
                            local.mouse_selection = None;
                            local.row_caches.clear();
                        }
                        if local.sizes.get(&terminal) != Some(&(cols, rows)) {
                            local.sizes.insert(terminal, (cols, rows));
                            resize_bridge.send(RequestBody::ResizeTerminal {
                                terminal,
                                cols,
                                rows,
                            });
                        }
                        drop(local);
                        if selection_cleared {
                            resize_state.update(cx, |_, cx| cx.notify());
                        }
                    });
                if let Some(selection) = selection {
                    painted = painted.selection(selection);
                }
                painted.into_any_element()
            }
            // §3.6 "Attaching": one dim centered line, with input meanwhile held in order.
            None => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(theme.terminal.background)
                .child(Text::ui("attaching\u{2026}").muted())
                .into_any_element(),
        };

        // The pixel area the grid is laid out into, remembered for the *next* attach: without
        // it every new or newly selected terminal was attached at 80 × 24 and then resized,
        // which costs a full-screen redraw at the wrong size before the right one arrives.
        let area_local = Rc::clone(&self.local);
        let wheel_local = Rc::clone(&self.local);
        let wheel_bridge = bridge.clone();
        let wheel_state = state.clone();
        let wheel_terminal = model.terminal;
        let area = div()
            .id("terminal-wheel-area")
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _, cx| {
                let Some(terminal) = wheel_terminal else {
                    return;
                };
                let app = wheel_state.read(cx);
                if app.drops_terminal_keys() {
                    return;
                }
                let Some(grid) = app.grids.get(&terminal).filter(|grid| grid.primed) else {
                    return;
                };
                let lines = app.terminal_config.scroll_lines_per_step;
                let mut local = wheel_local.borrow_mut();
                let Some((id, bounds, metrics)) =
                    local.geometry.filter(|(id, _, _)| *id == terminal)
                else {
                    return;
                };
                // Geometry is the actual painter bounds, already inset by GRID_PADDING.
                let col = ((event.position.x - bounds.origin.x) / metrics.width)
                    .floor()
                    .max(0.0) as u16;
                let row = ((event.position.y - bounds.origin.y) / metrics.height)
                    .floor()
                    .max(0.0) as u16;
                let steps =
                    local
                        .wheel
                        .steps(id, event.delta, metrics.height, lines, event.touch_phase);
                let mut mods = Modifiers::empty();
                mods.set(Modifiers::SHIFT, event.modifiers.shift);
                mods.set(Modifiers::CTRL, event.modifiers.control);
                mods.set(Modifiers::ALT, event.modifiers.alt);
                mods.set(Modifiers::SUPER, event.modifiers.platform);
                if steps != 0 {
                    wheel_bridge.send(RequestBody::WheelTerminal {
                        terminal,
                        wheel: WheelEvent {
                            steps,
                            col: col.min(grid.cols.saturating_sub(1)),
                            row: row.min(grid.rows.saturating_sub(1)),
                            mods,
                        },
                    });
                }
                cx.stop_propagation();
            })
            .relative()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(measure(move |bounds| area_local.borrow_mut().area = bounds))
            .child(grid)
            .children((model.mode == TerminalMode::Scroll).then(|| {
                ScrollPill::new(model.scroll_offset, model.scrollback_len)
                    .selecting(selection.is_some())
                    .alt_screen(model.alt_screen)
            }))
            .child(
                PrefixHint::new(model.mode == TerminalMode::Prefix && hint_visible)
                    .hints(prefix_hints()),
            );
        self.with_mouse_selection(area, state, focus)
            .into_any_element()
    }

    /// The band a Fleet-drawn tab fills.
    ///
    /// The element id is per worktree so gpui keeps each pane's hover, scroll and animation
    /// state apart when the user moves between worktrees.
    fn pane_area(&self, model: &Model, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let pane = model
            .worktree
            .as_ref()
            .and_then(|(worktree, _)| self.panes.get(worktree));
        let body: AnyElement = match pane {
            Some(pane) => pane.view.clone().into_any_element(),
            // One frame at most: `sync_panes` creates the view before this runs, unless the
            // snapshot has no worktree for the session (an agent session, or a race with a
            // deletion), in which case the tab has nothing to show and says so.
            None => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Text::ui("no worktree for this tab").muted())
                .into_any_element(),
        };
        let id = model.worktree.as_ref().map_or_else(
            || "workspace-native-pane".to_owned(),
            |(worktree, _)| format!("workspace-native-pane-{worktree}"),
        );
        div()
            .id(SharedString::from(id))
            .relative()
            .flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.colors.bg)
            .child(body)
            .into_any_element()
    }

    // ------------------------------------------------------------------ keys and actions

    /// Installs every listener the Workspace owns on the focused element.
    fn with_keys(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let root = self.with_key_forwarding(root, bridge, state);
        let root = self.with_clipboard_actions(root, bridge, state);
        let root = self.with_prefix_actions(root, bridge, state);
        self.with_scroll_actions(root, bridge, state)
    }

    /// Terminal-mode key forwarding, the inline rename editor and the close confirmation.
    fn with_key_forwarding(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_key_down(move |event: &KeyDownEvent, _window, cx| {
            let (mode, terminal, drops, primed) = {
                let app = state.read(cx);
                if !matches!(app.screen, Screen::Workspace { .. }) {
                    return;
                }
                let terminal = app
                    .active_session()
                    .and_then(|session| session.active_terminal);
                (
                    app.terminal_mode,
                    terminal,
                    app.drops_terminal_keys(),
                    terminal.is_some_and(|id| app.grids.get(&id).is_some_and(|grid| grid.primed)),
                )
            };

            // A key in Prefix or Scroll belongs to that mode, never to the PTY. Prefix keys are
            // normally consumed by the shell's live-state interceptor before this listener;
            // this guard is the final backstop while the rendered tree catches up.
            if mode != TerminalMode::Terminal {
                return;
            }
            let Some(terminal) = terminal else {
                return;
            };
            // §3.12 C: a veiled terminal drops keys. Buffering them would replay a burst into a
            // live shell the instant the daemon came back.
            if drops {
                cx.stop_propagation();
                return;
            }
            let Some(key) = key_event(&event.keystroke, event.is_held) else {
                return;
            };
            if local.borrow_mut().clear_selections() {
                state.update(cx, |_, cx| cx.notify());
            }
            send_or_queue_input(&local, &bridge, terminal, primed, PendingInput::Key(key));
            cx.stop_propagation();
        })
    }

    /// Standard clipboard actions while the terminal owns focus.
    fn with_clipboard_actions(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(
                move |_: &crate::actions::workspace::PasteClipboard, _window, cx| {
                    paste_clipboard(&local, &bridge, &state, cx);
                },
            )
        };
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_action(
            move |_: &crate::actions::workspace::CopySelection, _window, cx| {
                match current_selection_text(&local, &state, cx) {
                    CurrentSelectionText::Text(text) => {
                        if !text.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            state.update(cx, |app, cx| {
                                app.toast_short("copied", Icon::ClipboardCheck, Instant::now());
                                cx.notify();
                            });
                        }
                        return;
                    }
                    CurrentSelectionText::Missing => {
                        selection_scrolled_away(&local, &state, cx);
                        return;
                    }
                    CurrentSelectionText::None => {}
                }

                // A binding normally prevents `on_key_down` from seeing the keystroke. Forward the
                // unmatched cmd-c explicitly so Fleet never steals a terminal program's shortcut.
                if state.read(cx).terminal_mode == TerminalMode::Terminal
                    && let Some((terminal, primed)) = terminal_input_target(&state, cx)
                {
                    send_or_queue_input(
                        &local,
                        &bridge,
                        terminal,
                        primed,
                        PendingInput::Key(KeyEvent {
                            key: Key::Char('c'),
                            mods: Modifiers::SUPER,
                            text: None,
                            action: KeyAction::Press,
                        }),
                    );
                }
            },
        )
    }

    /// Plain left-drag selection, independent of the program running in the PTY.
    fn with_mouse_selection(
        &self,
        area: gpui::Stateful<Div>,
        state: &Entity<AppState>,
        focus: &FocusHandle,
    ) -> gpui::Stateful<Div> {
        let down_local = Rc::clone(&self.local);
        let down_state = state.clone();
        let down_focus = focus.clone();
        let area = area.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                window.focus(&down_focus, cx);
                let Some(cell) = mouse_cell(&down_local, &down_state, event.position, window, cx)
                else {
                    return;
                };
                let granularity = if event.click_count >= 3 {
                    SelectionGranularity::Line
                } else if event.click_count == 2 {
                    SelectionGranularity::Word
                } else {
                    SelectionGranularity::Cell
                };
                let initial = {
                    let app = down_state.read(cx);
                    app.active_grid().and_then(|grid| {
                        absolute_selection_at(grid, cell.viewport, granularity).or_else(|| {
                            absolute_selection_at(grid, cell.viewport, SelectionGranularity::Cell)
                        })
                    })
                };
                let Some(initial) = initial else {
                    return;
                };
                {
                    let mut local = down_local.borrow_mut();
                    local.anchor = None;
                    local.anchor_history_epoch = None;
                    local.history.clear();
                    local.row_caches.clear();
                    local.mouse_selection = Some(MouseSelection {
                        anchor: initial.start,
                        head: initial.end,
                        initial,
                        initiating: cell.absolute,
                        granularity,
                        history_epoch: cell.history_epoch,
                        cols: cell.cols,
                        alt_screen: cell.alt_screen,
                        dragging: true,
                        selected: granularity != SelectionGranularity::Cell,
                    });
                }
                down_state.update(cx, |_, cx| cx.notify());
                cx.stop_propagation();
            },
        );

        let move_local = Rc::clone(&self.local);
        let move_state = state.clone();
        let area = area.on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
            if let Some(terminal) = move_state
                .read(cx)
                .active_session()
                .and_then(|session| session.active_terminal)
            {
                move_local.borrow_mut().wheel.point_at(terminal);
            }

            if event.pressed_button != Some(MouseButton::Left)
                || !move_local
                    .borrow()
                    .mouse_selection
                    .is_some_and(|selection| selection.dragging)
            {
                return;
            }
            let Some(cell) = mouse_cell(&move_local, &move_state, event.position, window, cx)
            else {
                return;
            };
            let next = {
                let app = move_state.read(cx);
                app.active_grid().and_then(|grid| {
                    let selection = move_local.borrow().mouse_selection?;
                    extend_absolute_selection(
                        grid,
                        selection.initial,
                        selection.initiating,
                        cell.viewport,
                        selection.granularity,
                    )
                })
            };
            let changed = {
                let mut local = move_local.borrow_mut();
                let Some(selection) = local.mouse_selection.as_mut() else {
                    return;
                };
                if selection.cols != cell.cols
                    || selection.alt_screen != cell.alt_screen
                    || history_epoch_changed(
                        Some(selection.history_epoch),
                        Some(cell.history_epoch),
                    )
                {
                    local.mouse_selection = None;
                    local.row_caches.clear();
                    true
                } else if let Some(next) = next {
                    let selected = selection.granularity != SelectionGranularity::Cell
                        || cell.absolute != selection.initiating;
                    let changed = selection.anchor != next.start
                        || selection.head != next.end
                        || selection.selected != selected;
                    selection.anchor = next.start;
                    selection.head = next.end;
                    selection.selected = selected;
                    changed
                } else {
                    false
                }
            };
            if changed {
                move_state.update(cx, |_, cx| cx.notify());
            }
            cx.stop_propagation();
        });

        let up_local = Rc::clone(&self.local);
        let up_state = state.clone();
        let area = area.on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
            if finish_mouse_selection(&up_local, &up_state, cx) {
                cx.stop_propagation();
            }
        });
        let out_local = Rc::clone(&self.local);
        let out_state = state.clone();
        area.on_mouse_up_out(MouseButton::Left, move |_event, _window, cx| {
            // GPUI runs mouse-up-out in capture, before sibling on_click handlers.
            // Only consume a release that belongs to an active terminal drag.
            if finish_mouse_selection(&out_local, &out_state, cx) {
                cx.stop_propagation();
            }
        })
    }

    /// Every `ctrl-s <key>` binding the shell does not already own.
    fn with_prefix_actions(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SendLiteral, _window, cx| {
                if let Some((terminal, primed)) = terminal_input_target(&state, cx) {
                    if local.borrow_mut().clear_selections() {
                        state.update(cx, |_, cx| cx.notify());
                    }
                    send_or_queue_input(
                        &local,
                        &bridge,
                        terminal,
                        primed,
                        PendingInput::Key(KeyEvent {
                            key: Key::Char('s'),
                            mods: Modifiers::CTRL,
                            text: None,
                            action: KeyAction::Press,
                        }),
                    );
                }
            })
        };
        let root = {
            let (local, bridge, _) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::GoHub, _window, cx| {
                // The session keeps running in fleetd; only this client's attachment ends.
                detach(&local, &bridge);
                // The shell owns the screen change, so this listener hands the action back.
                cx.propagate();
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SleepAndGoHub, _window, cx| {
                let session = state.read(cx).active_session().map(|s| s.id.clone());
                detach(&local, &bridge);
                if let Some(session) = session {
                    bridge.send(RequestBody::SleepSession { session });
                }
                state.update(cx, |app, cx| {
                    app.leave_prefix();
                    app.screen = Screen::hub();
                    cx.notify();
                });
            })
        };
        let root = {
            let state = state.clone();
            root.on_action(move |_: &prefix::ToggleWatchPane, _, cx| {
                crate::views::watch_pane::toggle(&state, cx)
            })
        };
        let root = {
            let state = state.clone();
            let bridge = bridge.clone();
            root.on_action(move |_: &prefix::DismissWatch, _, cx| {
                crate::views::watch_pane::dismiss_selected(&state, &bridge, cx)
            })
        };
        let root = {
            let state = state.clone();
            root.on_action(move |_: &prefix::NextWatch, _, cx| {
                state.update(cx, |app, cx| {
                    app.cycle_watch(true, Instant::now());
                    cx.notify();
                });
            })
        };
        let root = {
            let state = state.clone();
            root.on_action(move |_: &prefix::PrevWatch, _, cx| {
                state.update(cx, |app, cx| {
                    app.cycle_watch(false, Instant::now());
                    cx.notify();
                });
            })
        };
        let root = self.tab_actions(root, bridge, state);
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::NewTerminal, _window, cx| {
                let Some(session) = state.read(cx).active_session().cloned() else {
                    return;
                };
                request_shell_tab(&local, &session, &bridge, &state, cx);
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::CloseTerminal, _window, cx| {
                let Some(terminal) = active_terminal_record(&state, cx) else {
                    return;
                };
                if terminal.keep_alive.is_empty() {
                    bridge.send(RequestBody::CloseTerminal {
                        terminal: terminal.id,
                    });
                } else {
                    let index = state
                        .read(cx)
                        .active_session()
                        .and_then(|session| {
                            session
                                .terminals
                                .iter()
                                .position(|candidate| candidate.id == terminal.id)
                        })
                        .map_or(1, |index| index + 1);
                    dialogs::request_confirm(
                        cx,
                        dialogs::ConfirmRequest::CloseTerminal {
                            terminal: terminal.id,
                            index,
                            name: terminal.name,
                            running: terminal.foreground_command,
                        },
                    );
                    state.update(cx, |app, cx| {
                        app.leave_prefix();
                        app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
                        cx.notify();
                    });
                }
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::RestartCommand, _window, cx| {
                if let Some(terminal) = active_terminal(&state, cx) {
                    bridge.send(RequestBody::RestartTerminal { terminal });
                }
            })
        };
        let root = {
            let (_, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::RenameTerminal, _window, cx| {
                state.update(cx, |app, cx| {
                    app.leave_prefix();
                    app.open_overlay(Overlay::Dialog(Dialogs::RenameTerminal));
                    cx.notify();
                });
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::Paste, _window, cx| {
                paste_clipboard(&local, &bridge, &state, cx);
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::CopyWorktreePath, _window, cx| {
                let Some(worktree) = active_worktree_id(&state, cx) else {
                    return;
                };
                let reply = bridge.request(RequestBody::WorktreePath { id: worktree });
                let state = state.clone();
                cx.spawn(async move |cx| {
                    let Ok(Ok(ResponseBody::Path(path))) = reply.recv().await else {
                        return;
                    };
                    cx.update(|cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path));
                        state.update(cx, |app, cx| {
                            app.toast_short("path copied", Icon::ClipboardCheck, Instant::now());
                            cx.notify();
                        });
                    });
                })
                .detach();
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::LastSession, _window, cx| {
                let alternate = state.read(cx).session_mru.alternate().cloned();
                match alternate {
                    Some(session) => {
                        detach(&local, &bridge);
                        open_session(&state, session, cx);
                    }
                    None => state.update(cx, |app, cx| {
                        app.toast_short("no other session", Icon::Info, Instant::now());
                        cx.notify();
                    }),
                }
            })
        };
        let root = {
            let (_, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SessionSwitcher, _window, cx| {
                state.update(cx, |app, cx| {
                    app.leave_prefix();
                    app.palette_seed = Some("sessions".to_owned());
                    app.open_overlay(Overlay::Palette);
                    cx.notify();
                });
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &fleet::OpenAgentClaude, _window, cx| {
                open_agent(&local, &bridge, &state, Agent::Claude, cx);
            })
        };
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_action(move |_: &fleet::OpenAgentOpencode, _window, cx| {
            open_agent(&local, &bridge, &state, Agent::Opencode, cx);
        })
    }

    /// `ctrl-s 1`–`9`, `h` / `l`, `p` / `n` and `Tab`.
    fn tab_actions(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        macro_rules! select_tab {
            ($root:expr, $action:ty, $position:expr) => {{
                let (local, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _window, cx| {
                    let terminal = state
                        .read(cx)
                        .active_session()
                        .and_then(|session| workspace_tabs::terminal_at(session, $position));
                    select_terminal(&local, &bridge, &state, terminal, cx);
                })
            }};
        }
        let root = select_tab!(root, prefix::SelectTab1, 0);
        let root = select_tab!(root, prefix::SelectTab2, 1);
        let root = select_tab!(root, prefix::SelectTab3, 2);
        let root = select_tab!(root, prefix::SelectTab4, 3);
        let root = select_tab!(root, prefix::SelectTab5, 4);
        let root = select_tab!(root, prefix::SelectTab6, 5);
        let root = select_tab!(root, prefix::SelectTab7, 6);
        let root = select_tab!(root, prefix::SelectTab8, 7);
        let root = select_tab!(root, prefix::SelectTab9, 8);

        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::PrevTab, _window, cx| {
                let terminal = neighbour_terminal(&state, -1, cx);
                select_terminal(&local, &bridge, &state, terminal, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::NextTab, _window, cx| {
                let terminal = neighbour_terminal(&state, 1, cx);
                select_terminal(&local, &bridge, &state, terminal, cx);
            })
        };
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_action(move |_: &prefix::LastTab, _window, cx| {
            let terminal = {
                let app = state.read(cx);
                app.active_session().and_then(|session| {
                    app.terminal_mru
                        .get(&session.id)
                        .and_then(|mru| mru.alternate().copied())
                })
            };
            select_terminal(&local, &bridge, &state, terminal, cx);
        })
    }

    /// Scroll mode: viewport movement, the line-wise selection and the yank.
    fn with_scroll_actions(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        macro_rules! terminal_scroll {
            ($root:expr, $action:ty, $command:expr, $key:expr, $mods:expr) => {{
                let (_, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _, cx| {
                    let app = state.read(cx);
                    if app.terminal_mode != TerminalMode::Terminal || app.drops_terminal_keys() {
                        return;
                    }
                    let Some(terminal) = app
                        .active_session()
                        .and_then(|session| session.active_terminal)
                    else {
                        return;
                    };
                    bridge.send(RequestBody::ScrollOrKeyTerminal {
                        terminal,
                        scroll: $command,
                        key: KeyEvent {
                            key: $key,
                            mods: $mods,
                            text: None,
                            action: KeyAction::Press,
                        },
                    });
                })
            }};
        }
        let root = terminal_scroll!(
            root,
            scroll::TerminalPageUp,
            ScrollCommand::Pages(-1),
            Key::PageUp,
            Modifiers::SHIFT
        );
        let root = terminal_scroll!(
            root,
            scroll::TerminalPageDown,
            ScrollCommand::Pages(1),
            Key::PageDown,
            Modifiers::SHIFT
        );
        let root = terminal_scroll!(
            root,
            scroll::TerminalTop,
            ScrollCommand::Top,
            Key::Home,
            Modifiers::SUPER
        );
        let root = terminal_scroll!(
            root,
            scroll::TerminalBottom,
            ScrollCommand::Bottom,
            Key::End,
            Modifiers::SUPER
        );
        // `j` and `k` move the caret first and only scroll once it is against an edge, which is
        // what lets a selection be extended without the text moving under the eyes.
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::LineDown, _window, cx| {
                step_line(&local, &bridge, &state, 1, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::LineUp, _window, cx| {
                step_line(&local, &bridge, &state, -1, cx);
            })
        };
        // Every page motion moves the caret by the same number of lines. Without that the
        // viewport slides out from under a live selection and the anchor stops being reachable,
        // which is what made a multi-page selection impossible.
        macro_rules! scroll_pages {
            ($root:expr, $action:ty, $down:expr) => {{
                let (local, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _window, cx| {
                    let page = i32::from(visible_rows(&state, cx)).max(1);
                    let lines = if $down { page } else { -page };
                    scroll_lines(&local, &bridge, &state, lines, cx);
                })
            }};
        }
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::HalfPageDown, _window, cx| {
                let half = half_page(&state, cx);
                scroll_lines(&local, &bridge, &state, half, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::HalfPageUp, _window, cx| {
                let half = half_page(&state, cx);
                scroll_lines(&local, &bridge, &state, -half, cx);
            })
        };
        let root = scroll_pages!(root, scroll::PageDown, true);
        let root = scroll_pages!(root, scroll::PageUp, false);
        // `g` and `G` jump somewhere the caret cannot be derived from a delta. Parking it at
        // either extreme lets the next frame clamp it onto the new viewport.
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Top, _window, cx| {
                local.borrow_mut().caret = 0;
                scroll_viewport(&bridge, &state, ScrollCommand::Top, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Bottom, _window, cx| {
                local.borrow_mut().caret = u64::MAX;
                scroll_viewport(&bridge, &state, ScrollCommand::Bottom, cx);
            })
        };

        let root = {
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::StartSelection, _window, cx| {
                let (base, rows) = viewport_span(&state, cx);
                let history_epoch = state
                    .read(cx)
                    .active_grid()
                    .map(|grid| grid.viewport.history_epoch);
                let mut borrowed = local.borrow_mut();
                // A caret that has never moved is still at line 0; anchoring there would
                // anchor the oldest line in the scrollback rather than the one on screen.
                if rows > 0 {
                    borrowed.caret = borrowed.caret.clamp(base, base + u64::from(rows - 1));
                }
                borrowed.anchor = Some(borrowed.caret);
                borrowed.anchor_history_epoch = history_epoch;
                borrowed.history.clear();
                borrowed.row_caches.clear();
                drop(borrowed);
                state.update(cx, |_, cx| cx.notify());
            })
        };
        let root = {
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Yank, _window, cx| {
                let text = {
                    let borrowed = local.borrow();
                    borrowed
                        .anchor
                        .map(|anchor| selection_text(&borrowed.history, anchor, borrowed.caret))
                };
                let Some(text) = text else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                {
                    let mut borrowed = local.borrow_mut();
                    borrowed.anchor = None;
                    borrowed.anchor_history_epoch = None;
                    borrowed.history.clear();
                    borrowed.row_caches.clear();
                }
                state.update(cx, |app, cx| {
                    app.toast_short("copied", Icon::ClipboardCheck, Instant::now());
                    cx.notify();
                });
            })
        };
        // `Esc` clears a selection first and only then leaves the mode, so a mistyped `v` costs
        // one key rather than a re-entry into Scroll.
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Escape, _window, cx| {
                {
                    let mut borrowed = local.borrow_mut();
                    if borrowed.anchor.take().is_some() {
                        borrowed.anchor_history_epoch = None;
                        borrowed.history.clear();
                        borrowed.row_caches.clear();
                        drop(borrowed);
                        state.update(cx, |_, cx| cx.notify());
                        return;
                    }
                }
                exit_scroll(&local, &bridge, &state, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Exit, _window, cx| {
                exit_scroll(&local, &bridge, &state, cx);
            })
        };
        // `/`, `n` and `N` are reserved by KEYMAP for scrollback search, which lands after v1.
        // They stay bound so the keys cannot be reused, and say so rather than doing nothing.
        macro_rules! reserved {
            ($root:expr, $action:ty) => {{
                let (_, _, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _window, cx| {
                    state.update(cx, |app, cx| {
                        app.toast_short(
                            "scrollback search is not available yet",
                            Icon::Search,
                            Instant::now(),
                        );
                        cx.notify();
                    });
                })
            }};
        }
        let root = reserved!(root, scroll::Search);
        let root = reserved!(root, scroll::SearchNext);
        reserved!(root, scroll::SearchPrev)
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

// ---------------------------------------------------------------------------- helpers

/// Detaches from whatever terminal this client holds.
/// Asks fleetd for a plain shell tab in the session's worktree path.
///
/// `ctrl-s c` and the `+` at the end of the strip are the same request, so they share this.
fn request_shell_tab(
    local: &Rc<RefCell<Local>>,
    session: &Session,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::NewTerminal {
        session: session.id.clone(),
        name: workspace_tabs::new_terminal_name(session),
        command: SHELL_TAB_COMMAND.to_owned(),
        cwd: session.cwd.clone(),
    });
    let state = state.clone();
    let bridge = bridge.clone();
    let local = Rc::clone(local);
    cx.spawn(async move |cx| {
        match reply.recv().await {
            // §3.6: the tab the user just asked for is the tab they are about to type into.
            // Selecting it is what moves the blue underline *and* what makes the daemon clear
            // its unseen-output dot; without it the next keystrokes go to the previous PTY.
            Ok(Ok(ResponseBody::Terminal(terminal))) => cx.update(|cx| {
                select_terminal(&local, &bridge, &state, Some(terminal.id), cx);
            }),
            // A refused request is sticky, never silent (§1.8) — this key used to fail quietly.
            Ok(Err(error)) => cx.update(|cx| {
                state.update(cx, |app, cx| {
                    app.sticky_error = Some(crate::state::StickyError {
                        text: error.message,
                        job: None,
                        retryable: false,
                    });
                    cx.notify();
                });
            }),
            _ => {}
        }
    })
    .detach();
}

fn detach(local: &Rc<RefCell<Local>>, bridge: &Bridge) {
    let attached = local.borrow_mut().attached.take();
    if let Some(terminal) = attached {
        bridge.send(RequestBody::DetachTerminal { terminal });
    }
}

/// Sends clipboard text through the same ordered attach gate as terminal keys.
fn paste_clipboard(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    let Some((terminal, primed)) = terminal_input_target(state, cx) else {
        return;
    };
    let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
        return;
    };
    if local.borrow_mut().clear_selections() {
        state.update(cx, |_, cx| cx.notify());
    }
    send_or_queue_input(local, bridge, terminal, primed, PendingInput::Paste(text));
}

fn terminal_input_target(state: &Entity<AppState>, cx: &App) -> Option<(TerminalId, bool)> {
    let app = state.read(cx);
    if app.drops_terminal_keys() {
        return None;
    }
    let terminal = app
        .active_session()
        .and_then(|session| session.active_terminal)?;
    let primed = app.grids.get(&terminal).is_some_and(|grid| grid.primed);
    Some((terminal, primed))
}

fn send_or_queue_input(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    terminal: TerminalId,
    primed: bool,
    input: PendingInput,
) {
    if primed {
        bridge.send(input.into_request(terminal));
    } else {
        local.borrow_mut().pending.push(input);
    }
}

/// Result of resolving the currently active selection for a clipboard operation.
enum CurrentSelectionText {
    None,
    Text(String),
    Missing,
}

/// Text owned by either the mouse selection or the existing Scroll-mode selection.
fn current_selection_text(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    cx: &App,
) -> CurrentSelectionText {
    let app = state.read(cx);
    let Some(terminal) = app
        .active_session()
        .and_then(|session| session.active_terminal)
    else {
        return CurrentSelectionText::None;
    };
    let Some(grid) = app.grids.get(&terminal) else {
        return CurrentSelectionText::None;
    };
    let local = local.borrow();
    if let Some(selection) = local.mouse_selection.filter(|selection| selection.selected) {
        let empty_cache = BTreeMap::new();
        let cache = local
            .row_caches
            .get(&terminal)
            .map_or(&empty_cache, |cache| &cache.rows);
        return absolute_selection_text(
            grid,
            cache,
            AbsoluteCellSelection::new(selection.anchor, selection.head),
        )
        .map_or(CurrentSelectionText::Missing, CurrentSelectionText::Text);
    }
    local.anchor.map_or(CurrentSelectionText::None, |anchor| {
        CurrentSelectionText::Text(selection_text(&local.history, anchor, local.caret))
    })
}

/// Clears a selection whose required rows have fallen outside both mirror and bounded cache.
fn selection_scrolled_away(local: &Rc<RefCell<Local>>, state: &Entity<AppState>, cx: &mut App) {
    local.borrow_mut().clear_selections();
    state.update(cx, |app, cx| {
        app.toast_short("selection scrolled away", Icon::Info, Instant::now());
        cx.notify();
    });
}

/// Maps a mouse position with the same measured metrics the grid painter uses.
fn mouse_cell(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    position: gpui::Point<Pixels>,
    window: &Window,
    cx: &App,
) -> Option<MouseCell> {
    let (cols, rows, viewport_base, alt_screen, history_epoch) = {
        let app = state.read(cx);
        let grid = app.active_grid()?;
        (
            grid.cols,
            grid.rows,
            viewport_base(grid),
            grid.modes.alt_screen,
            grid.viewport.history_epoch,
        )
    };
    let bounds = local.borrow().area;
    let metrics = CellMetrics::measure(cx.theme(), window, cx);
    let point = cell_at_position(
        bounds,
        position,
        gpui::size(metrics.width, metrics.height),
        cols,
        rows,
    )?;
    Some(MouseCell {
        viewport: point,
        absolute: AbsoluteCellPoint::new(viewport_base + point.row as u64, point.col),
        cols,
        alt_screen,
        history_epoch,
    })
}

#[derive(Clone, Copy, Debug)]
struct MouseCell {
    viewport: CellPoint,
    absolute: AbsoluteCellPoint,
    cols: u16,
    alt_screen: bool,
    history_epoch: u64,
}

/// Claims a release only for an active terminal drag; completed selections do not own clicks.
fn end_mouse_drag(local: &mut Local) -> Option<bool> {
    let selection = local.mouse_selection.as_mut()?;
    if !selection.dragging {
        return None;
    }
    selection.dragging = false;
    let selected = selection.selected;
    if !selected {
        local.mouse_selection = None;
        local.row_caches.clear();
    }
    Some(selected)
}

/// Ends and copies a terminal drag, returning whether its release should be consumed.
fn finish_mouse_selection(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    cx: &mut App,
) -> bool {
    let Some(selected) = end_mouse_drag(&mut local.borrow_mut()) else {
        return false;
    };
    if selected {
        match current_selection_text(local, state, cx) {
            CurrentSelectionText::Text(text) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            CurrentSelectionText::Missing => {
                selection_scrolled_away(local, state, cx);
                return true;
            }
            CurrentSelectionText::None | CurrentSelectionText::Text(_) => {}
        }
    }
    state.update(cx, |_, cx| cx.notify());
    true
}

/// Moves the viewport and repaints.
fn scroll_viewport(
    bridge: &Bridge,
    state: &Entity<AppState>,
    command: ScrollCommand,
    cx: &mut App,
) {
    if let Some(terminal) = active_terminal(state, cx) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: command,
        });
    }
    state.update(cx, |_, cx| cx.notify());
}

/// `j` / `k`: move the caret, and scroll only once it is against an edge.
fn step_line(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    delta: isize,
    cx: &mut App,
) {
    let (base, rows) = viewport_span(state, cx);
    if move_caret(local, delta, base, rows) {
        state.update(cx, |_, cx| cx.notify());
        return;
    }
    // The caret is against an edge: the viewport moves under it instead, and the caret goes
    // with it so a selection keeps extending line by line.
    scroll_lines(local, bridge, state, i32::try_from(delta).unwrap_or(0), cx);
}

/// The absolute line the viewport's top row shows, and how many rows it has.
fn viewport_span(state: &Entity<AppState>, cx: &App) -> (u64, u16) {
    state
        .read(cx)
        .active_grid()
        .map_or((0, 0), |grid| (viewport_base(grid), grid.rows))
}

/// Scrolls by whole lines and carries the caret along, so a live selection grows with the view.
fn scroll_lines(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    lines: i32,
    cx: &mut App,
) {
    {
        let mut local = local.borrow_mut();
        let moved = i64::from(lines);
        local.caret = if moved >= 0 {
            local.caret.saturating_add(moved.unsigned_abs())
        } else {
            local.caret.saturating_sub(moved.unsigned_abs())
        };
    }
    scroll_viewport(bridge, state, ScrollCommand::Lines(lines), cx);
}

/// Leaves Scroll mode: the viewport snaps back to the live bottom (KEYMAP §Scroll).
fn exit_scroll(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    {
        let mut borrowed = local.borrow_mut();
        borrowed.anchor = None;
        borrowed.anchor_history_epoch = None;
        borrowed.history.clear();
        borrowed.row_caches.clear();
    }
    if let Some(terminal) = active_terminal(state, cx) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Bottom,
        });
    }
    state.update(cx, |app, cx| {
        if app.terminal_mode == TerminalMode::Scroll {
            app.terminal_mode = app.resting_terminal_mode();
        }
        cx.notify();
    });
}

/// Moves the scroll caret inside the viewport, returning whether it moved at all.
///
/// The caret is an absolute scrollback line, so the viewport it must stay inside is passed in:
/// `base` is the line the top row shows and `rows` how many are on screen. Returning `false`
/// at either edge is what makes `j` / `k` scroll instead.
fn move_caret(local: &Rc<RefCell<Local>>, delta: isize, base: u64, rows: u16) -> bool {
    if rows == 0 {
        return false;
    }
    let mut local = local.borrow_mut();
    let bottom = base + u64::from(rows - 1);
    let current = local.caret.clamp(base, bottom);
    local.caret = current;
    let next = if delta >= 0 {
        current
            .saturating_add(delta.unsigned_abs() as u64)
            .min(bottom)
    } else {
        current
            .saturating_sub(delta.unsigned_abs() as u64)
            .max(base)
    };
    if next == current {
        return false;
    }
    local.caret = next;
    true
}

/// The active terminal of the active session.
fn active_terminal(state: &Entity<AppState>, cx: &App) -> Option<TerminalId> {
    state
        .read(cx)
        .active_session()
        .and_then(|session| session.active_terminal)
}

/// The active terminal's record, cloned so the state borrow ends before the caller mutates.
fn active_terminal_record(state: &Entity<AppState>, cx: &App) -> Option<Terminal> {
    let app = state.read(cx);
    let session = app.active_session()?;
    let active = session.active_terminal?;
    session
        .terminals
        .iter()
        .find(|terminal| terminal.id == active)
        .cloned()
}

/// The worktree the open session belongs to, when it is a worktree session.
fn active_worktree_id(state: &Entity<AppState>, cx: &App) -> Option<WorktreeId> {
    match &state.read(cx).active_session()?.kind {
        SessionKind::Worktree(id) => Some(id.clone()),
        SessionKind::Agent(_) => None,
    }
}

/// How many rows the mirror currently holds.
fn visible_rows(state: &Entity<AppState>, cx: &App) -> u16 {
    state.read(cx).active_grid().map_or(0, |grid| grid.rows)
}

/// Half a page of the current grid, never zero, as `ctrl-d` / `ctrl-u` move.
fn half_page(state: &Entity<AppState>, cx: &App) -> i32 {
    i32::from(visible_rows(state, cx) / 2).max(1)
}

/// The terminal `ctrl-s h` / `ctrl-s l` moves to.
fn neighbour_terminal(state: &Entity<AppState>, delta: isize, cx: &App) -> Option<TerminalId> {
    let app = state.read(cx);
    let session = app.active_session()?;
    workspace_tabs::neighbour(session, session.active_terminal, delta)
}

/// Selects a terminal in the daemon and records it in the session's MRU.
fn select_terminal(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    terminal: Option<TerminalId>,
    cx: &mut App,
) {
    let Some(terminal) = terminal else {
        return;
    };
    let Some((session, native)) = state.read(cx).active_session().map(|session| {
        let native = session
            .terminals
            .iter()
            .any(|entry| entry.id == terminal && entry.is_native());
        (session.id.clone(), native)
    }) else {
        return;
    };
    bridge.send(RequestBody::SelectTerminal {
        session: session.clone(),
        terminal,
    });
    {
        let mut borrowed = local.borrow_mut();
        borrowed.anchor = None;
        borrowed.anchor_history_epoch = None;
        borrowed.history.clear();
        borrowed.row_caches.clear();
    }
    state.update(cx, |app, cx| {
        app.touch_terminal(&session, terminal);
        // The snapshot decides which tab is really active; this only keeps the mode honest
        // while the round trip is in flight. The kind is read from the session record we
        // already have, so switching to a `fleet://` tab does not flash a Terminal frame.
        app.terminal_mode = if native {
            TerminalMode::Native
        } else {
            TerminalMode::Terminal
        };
        cx.notify();
    });
}

/// Switches the Workspace to another session, sleeping nothing.
fn open_session(state: &Entity<AppState>, session: SessionId, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.leave_prefix();
        app.touch_session(session.clone());
        app.screen = Screen::Workspace { session };
        app.terminal_mode = TerminalMode::Terminal;
        // The session we are moving to may rest on a `fleet://` tab; the next snapshot would
        // fix it anyway, this only avoids one frame in the wrong mode.
        app.sync_terminal_mode();
        cx.notify();
    });
}

/// `ctrl-s a` / `ctrl-s A`: ensure the repository-level agent session and go to it.
fn open_agent(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    agent: Agent,
    cx: &mut App,
) {
    detach(local, bridge);
    let reply = bridge.request(RequestBody::EnsureSession {
        worktree: None,
        agent: Some(agent),
        sleep_previous: false,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Session(session))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| open_session(&state, session.id, cx));
    })
    .detach();
}

/// Turns a gpui keystroke into the daemon's semantic key event.
///
/// The daemon owns the encoder, so this only has to be faithful: the semantic key is gpui's
/// *unshifted* key — which is exactly what `fleet-term` sets as the unshifted codepoint — and
/// the composed text, when the platform produced one, rides along as `text`.
#[must_use]
pub fn key_event(keystroke: &Keystroke, is_held: bool) -> Option<KeyEvent> {
    let key = named_key(&keystroke.key)?;
    let mut mods = Modifiers::empty();
    if keystroke.modifiers.shift {
        mods |= Modifiers::SHIFT;
    }
    if keystroke.modifiers.control {
        mods |= Modifiers::CTRL;
    }
    if keystroke.modifiers.alt {
        mods |= Modifiers::ALT;
    }
    if keystroke.modifiers.platform {
        mods |= Modifiers::SUPER;
    }
    // Only a character key carries text: a named key's `key_char` is the platform's control
    // byte, and forwarding it would encode `Enter` twice.
    let text = match key {
        Key::Char(_) => keystroke.key_char.clone(),
        _ => None,
    };
    Some(KeyEvent {
        key,
        mods,
        text,
        action: if is_held {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        },
    })
}

/// The semantic key behind one of gpui's key names.
fn named_key(name: &str) -> Option<Key> {
    Some(match name {
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "space" => Key::Char(' '),
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            let mut chars = other.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                // A named key this protocol does not model (`f13`, `back`, a lone modifier) is
                // dropped rather than typed into the shell as literal text.
                return None;
            }
            Key::Char(first)
        }
    })
}

/// The badge a pull request's facts add up to (§3.5 priority order).
#[must_use]
pub fn badge_state(is_draft: bool, checks: PrChecks, review: PrReviewDecision) -> PrBadgeState {
    match derive_pr_state(is_draft, checks, review) {
        PrState::Draft => PrBadgeState::Draft,
        PrState::CiFail => PrBadgeState::CiFail,
        PrState::Changes => PrBadgeState::Changes,
        PrState::CiPending => PrBadgeState::CiPending,
        PrState::Approved => PrBadgeState::Approved,
        PrState::Review => PrBadgeState::Review,
    }
}

/// How many of a snapshot's jobs belong to this session, running and failed (§3.6 `⟳n` / `⚠n`).
#[must_use]
pub fn job_counts(jobs: &[JobRecord], targets: &[String]) -> (usize, usize) {
    let mut running = 0;
    let mut failed = 0;
    for job in jobs.iter().filter(|job| targets.contains(&job.target)) {
        match job.status {
            JobStatus::Queued | JobStatus::Running => running += 1,
            JobStatus::Failed { .. } => failed += 1,
            JobStatus::Cancelling | JobStatus::Succeeded | JobStatus::Cancelled => {}
        }
    }
    (running, failed)
}

/// The status glyph a session shows, identical to the Hub's for the same worktree (§2.5).
#[must_use]
pub fn status_kind(session: SessionState, sleeping: bool, degraded: bool) -> StatusKind {
    if degraded {
        return StatusKind::Degraded;
    }
    match session {
        SessionState::Attached => StatusKind::Attached,
        SessionState::Detached if sleeping => StatusKind::Sleeping,
        SessionState::Detached => StatusKind::DetachedAwake,
        SessionState::Unknown => StatusKind::Unknown,
        SessionState::None => StatusKind::NoSession,
    }
}

// ---------------------------------------------------------------------------- the frame model

/// Everything one frame of the Workspace needs, read from [`AppState`] exactly once.
///
/// It holds only scalars: the mirror grid itself is read straight out of the state entity while
/// the rows are being built, because copying a full terminal grid once per frame is exactly the
/// cost this screen exists to avoid.
struct Model {
    link_generation: u64,
    mode: TerminalMode,
    zoomed: bool,
    terminal: Option<TerminalId>,
    grid_cols: Option<u16>,
    history_epoch: Option<u64>,
    primed: bool,
    alt_screen: bool,
    scroll_offset: usize,
    scrollback_len: usize,
    exit_code: Option<Option<i32>>,
    title: SharedString,
    branch_key: Option<String>,
    repo: Option<RepoId>,
    host: Option<(SharedString, bool)>,
    status: StatusKind,
    keep_alive: Vec<SharedString>,
    running_jobs: usize,
    failed_jobs: usize,
    waking: bool,
    /// The VT modes the active terminal's last frame reported (§3.6: badged in the header).
    modes: Vec<KitTerminalMode>,
    /// Whether the active tab is drawn by Fleet rather than by a PTY.
    native: bool,
    /// The worktree the session belongs to, and its path on disk — what a pane is built from.
    worktree: Option<(WorktreeId, PathBuf)>,
    /// Whether a Fleet overlay owns the keyboard, in which case no pane may hold it.
    overlay_open: bool,
}

impl Model {
    /// The terminal this client attaches to, which is never a Fleet-drawn tab.
    const fn attach_target(&self) -> Option<TerminalId> {
        if self.native { None } else { self.terminal }
    }
}

impl Model {
    fn build(app: &AppState, session: &Session) -> Self {
        let terminal = session.active_terminal;
        let grid = terminal.and_then(|id| app.grids.get(&id));
        let worktree = worktree_of(app, session);
        let (title, branch_key, repo, host) = match worktree {
            Some(worktree) => (
                SharedString::from(worktree.branch.clone()),
                Some(worktree.branch.clone()),
                Some(worktree.repo_id.clone()),
                worktree.host.as_ref().map(|host| {
                    let reachable = app.snapshot.as_ref().is_none_or(|snapshot| {
                        snapshot
                            .hosts
                            .iter()
                            .find(|candidate| &candidate.id == host)
                            .is_none_or(|candidate| candidate.reachable)
                    });
                    (SharedString::from(host.to_string()), reachable)
                }),
            ),
            // An agent session has no worktree: its own name is the only identity it has.
            None => (SharedString::from(session.id.to_string()), None, None, None),
        };

        let status = worktree.map_or(StatusKind::Attached, |worktree| {
            let session_state = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .statuses
                        .iter()
                        .find(|status| status.worktree_id == worktree.id)
                })
                .map_or(SessionState::Attached, |status| status.session);
            status_kind(
                session_state,
                session.slept_at.is_some(),
                worktree.degraded.is_some(),
            )
        });

        let mut keep_alive: Vec<SharedString> = Vec::new();
        for terminal in &session.terminals {
            for label in &terminal.keep_alive {
                let label = SharedString::from(label.clone());
                if !keep_alive.contains(&label) {
                    keep_alive.push(label);
                }
            }
        }

        let targets: Vec<String> = worktree.map_or_else(Vec::new, |worktree| {
            vec![
                worktree.id.to_string(),
                worktree.repo_id.to_string(),
                worktree.slug.clone(),
            ]
        });
        let (running_jobs, failed_jobs) = app
            .snapshot
            .as_ref()
            .map_or((0, 0), |snapshot| job_counts(&snapshot.jobs, &targets));

        let exit_code = session
            .terminals
            .iter()
            .find(|candidate| Some(candidate.id) == terminal)
            .and_then(|candidate| match candidate.status {
                TerminalStatus::Exited { code } => Some(code),
                TerminalStatus::Running | TerminalStatus::Starting => None,
            });

        Self {
            link_generation: app.link_generation,
            mode: app.terminal_mode,
            zoomed: app.zoomed,
            terminal,
            grid_cols: grid.map(|grid| grid.cols),
            history_epoch: grid.map(|grid| grid.viewport.history_epoch),
            primed: grid.is_some_and(|grid| grid.primed),
            alt_screen: grid.is_some_and(|grid| grid.modes.alt_screen),
            scroll_offset: grid.map_or(0, |grid| grid.viewport.offset),
            scrollback_len: grid.map_or(0, |grid| grid.viewport.scrollback_len),
            exit_code,
            title,
            branch_key,
            repo,
            host,
            status,
            keep_alive,
            running_jobs,
            failed_jobs,
            native: session
                .terminals
                .iter()
                .any(|entry| Some(entry.id) == terminal && entry.is_native()),
            worktree: worktree.map(|worktree| (worktree.id.clone(), PathBuf::from(&worktree.path))),
            overlay_open: app.overlay.is_some(),
            waking: session.slept_at.is_some()
                && session
                    .terminals
                    .iter()
                    .any(|terminal| terminal.status == TerminalStatus::Starting),
            modes: grid.map_or_else(Vec::new, |grid| grid_modes(&grid.modes)),
        }
    }
}

/// The worktree a session belongs to, when it is a worktree session.
fn worktree_of<'a>(app: &'a AppState, session: &Session) -> Option<&'a Worktree> {
    let SessionKind::Worktree(id) = &session.kind else {
        return None;
    };
    app.snapshot
        .as_ref()?
        .worktrees
        .iter()
        .find(|worktree| &worktree.id == id)
}

#[cfg(test)]
mod tests {
    use fleet_core::ids::JobId;
    use fleet_proto::job::JobKind;
    use gpui::Modifiers as GpuiModifiers;

    use super::*;

    fn keystroke(key: &str, key_char: Option<&str>, mods: GpuiModifiers) -> Keystroke {
        Keystroke {
            modifiers: mods,
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    fn job(target: &str, status: JobStatus) -> JobRecord {
        JobRecord {
            id: JobId::try_from("job-1").unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::Prune,
            target: target.to_owned(),
            title: "prune".to_owned(),
            status,
            progress: None,
            log_path: "/tmp/job.log".to_owned(),
            started_at: "2026-09-04T00:00:00Z".to_owned(),
            finished_at: None,
            cancellable: false,
            retryable: false,
        }
    }

    fn encoded(key: &str, key_char: Option<&str>, mods: GpuiModifiers) -> KeyEvent {
        key_event(&keystroke(key, key_char, mods), false)
            .unwrap_or_else(|| panic!("`{key}` must encode"))
    }

    #[test]
    fn watch_split_scales_and_the_terminal_uses_its_reduced_measured_area() {
        assert_eq!(watch_width(800.0), 360.0);
        assert_eq!(watch_width(1200.0), 480.0);
        assert_eq!(watch_width(2000.0), 640.0);
        let cell = gpui::size(px(10.0), px(20.0));
        let mut local = Local {
            area: Bounds::new(
                gpui::point(px(0.0), px(0.0)),
                gpui::size(px(1200.0), px(600.0)),
            ),
            ..Local::default()
        };
        let full = local.size_for(TerminalId(1), cell);
        local.area.size.width -= px(watch_width(1200.0) + 1.0);
        let split = local.size_for(TerminalId(1), cell);
        assert!(split.0 < full.0);
        assert_eq!(split.1, full.1);
        assert_eq!(split, grid_size(local.area.size, cell));
    }

    #[test]
    fn a_plain_character_carries_its_composed_text() {
        let event = encoded("a", Some("a"), GpuiModifiers::default());
        assert_eq!(event.key, Key::Char('a'));
        assert_eq!(event.text.as_deref(), Some("a"));
        assert_eq!(event.mods, Modifiers::empty());
        assert_eq!(event.action, KeyAction::Press);
    }

    #[test]
    fn the_semantic_key_stays_unshifted_while_the_text_is_shifted() {
        let mods = GpuiModifiers {
            shift: true,
            ..GpuiModifiers::default()
        };
        let event = encoded("a", Some("A"), mods);
        assert_eq!(event.key, Key::Char('a'));
        assert_eq!(event.text.as_deref(), Some("A"));
        assert!(event.mods.contains(Modifiers::SHIFT));
    }

    #[test]
    fn control_keys_carry_no_text() {
        let mods = GpuiModifiers {
            control: true,
            ..GpuiModifiers::default()
        };
        let event = encoded("c", None, mods);
        assert_eq!(event.key, Key::Char('c'));
        assert_eq!(event.text, None);
        assert!(event.mods.contains(Modifiers::CTRL));
    }

    #[test]
    fn named_keys_never_send_text() {
        for (name, expected) in [
            ("enter", Key::Enter),
            ("escape", Key::Escape),
            ("backspace", Key::Backspace),
            ("tab", Key::Tab),
            ("up", Key::Up),
            ("pagedown", Key::PageDown),
            ("f5", Key::F5),
        ] {
            let event = encoded(name, Some("\r"), GpuiModifiers::default());
            assert_eq!(event.key, expected, "{name}");
            assert_eq!(event.text, None, "{name}");
        }
    }

    #[test]
    fn nvim_mode_keys_survive_the_app_translation() {
        let plain = GpuiModifiers::default();
        assert_eq!(encoded("escape", None, plain).key, Key::Escape);
        assert_eq!(encoded("i", Some("i"), plain).text.as_deref(), Some("i"));
        assert_eq!(encoded("v", Some("v"), plain).text.as_deref(), Some("v"));
        assert_eq!(encoded("up", None, plain).key, Key::Up);
        assert_eq!(encoded("down", None, plain).key, Key::Down);

        let shift = GpuiModifiers {
            shift: true,
            ..GpuiModifiers::default()
        };
        let colon = encoded(";", Some(":"), shift);
        assert_eq!(colon.key, Key::Char(';'));
        assert_eq!(colon.text.as_deref(), Some(":"));
        let shift_up = encoded("up", None, shift);
        assert_eq!(shift_up.key, Key::Up);
        assert!(shift_up.mods.contains(Modifiers::SHIFT));

        let control = GpuiModifiers {
            control: true,
            ..GpuiModifiers::default()
        };
        let ctrl_bracket = encoded("[", None, control);
        assert_eq!(ctrl_bracket.key, Key::Char('['));
        assert_eq!(ctrl_bracket.text, None);
        assert!(ctrl_bracket.mods.contains(Modifiers::CTRL));
        for key in ["c", "w"] {
            let event = encoded(key, None, control);
            assert_eq!(event.key, Key::Char(key.chars().next().unwrap_or_default()));
            assert_eq!(event.text, None);
            assert!(event.mods.contains(Modifiers::CTRL));
        }

        let alt = GpuiModifiers {
            alt: true,
            ..GpuiModifiers::default()
        };
        let alt_x = encoded("x", Some("x"), alt);
        assert_eq!(alt_x.key, Key::Char('x'));
        assert_eq!(alt_x.text.as_deref(), Some("x"));
        assert!(alt_x.mods.contains(Modifiers::ALT));
    }

    #[test]
    fn space_is_a_character_not_a_named_key() {
        assert_eq!(
            encoded("space", Some(" "), GpuiModifiers::default()).key,
            Key::Char(' ')
        );
    }

    #[test]
    fn a_held_key_is_a_repeat() {
        let event = key_event(&keystroke("a", Some("a"), GpuiModifiers::default()), true)
            .unwrap_or_else(|| panic!("printable keys must encode"));
        assert_eq!(event.action, KeyAction::Repeat);
    }

    #[test]
    fn unmodelled_keys_are_dropped_rather_than_typed() {
        for name in ["f13", "back", "", "shift"] {
            assert!(
                key_event(&keystroke(name, None, GpuiModifiers::default()), false).is_none(),
                "`{name}` must not reach the pty"
            );
        }
    }

    #[test]
    fn every_modifier_survives_the_translation() {
        let mods = GpuiModifiers {
            control: true,
            alt: true,
            shift: true,
            platform: true,
            function: true,
        };
        let event = encoded("a", None, mods);
        assert!(event.mods.contains(Modifiers::CTRL));
        assert!(event.mods.contains(Modifiers::ALT));
        assert!(event.mods.contains(Modifiers::SHIFT));
        assert!(event.mods.contains(Modifiers::SUPER));
    }

    #[test]
    fn only_this_sessions_jobs_reach_the_header_chip() {
        let jobs = vec![
            job("acme/api#feature", JobStatus::Running),
            job(
                "acme/api#feature",
                JobStatus::Failed {
                    error: "boom".to_owned(),
                },
            ),
            job("acme/api#other", JobStatus::Running),
            job("acme/api#feature", JobStatus::Succeeded),
        ];
        let targets = vec!["acme/api#feature".to_owned()];
        assert_eq!(job_counts(&jobs, &targets), (1, 1));
        assert_eq!(job_counts(&jobs, &[]), (0, 0));
    }

    #[test]
    fn a_queued_job_already_counts_as_running() {
        let jobs = vec![job("t", JobStatus::Queued)];
        assert_eq!(job_counts(&jobs, &["t".to_owned()]), (1, 0));
    }

    #[test]
    fn the_status_glyph_matches_the_hub_row() {
        assert_eq!(
            status_kind(SessionState::Attached, false, false),
            StatusKind::Attached
        );
        assert_eq!(
            status_kind(SessionState::Detached, true, false),
            StatusKind::Sleeping
        );
        assert_eq!(
            status_kind(SessionState::Detached, false, false),
            StatusKind::DetachedAwake
        );
        assert_eq!(
            status_kind(SessionState::Unknown, false, false),
            StatusKind::Unknown
        );
        assert_eq!(
            status_kind(SessionState::None, false, false),
            StatusKind::NoSession
        );
        // A failed post-create hook outranks every session state (§2.5).
        assert_eq!(
            status_kind(SessionState::Attached, false, true),
            StatusKind::Degraded
        );
    }

    #[test]
    fn the_pr_badge_follows_the_strict_priority_order() {
        assert_eq!(
            badge_state(true, PrChecks::Fail, PrReviewDecision::Approved),
            PrBadgeState::Draft
        );
        assert_eq!(
            badge_state(false, PrChecks::Fail, PrReviewDecision::Approved),
            PrBadgeState::CiFail
        );
        assert_eq!(
            badge_state(false, PrChecks::Pass, PrReviewDecision::ChangesRequested),
            PrBadgeState::Changes
        );
        assert_eq!(
            badge_state(false, PrChecks::Pending, PrReviewDecision::None),
            PrBadgeState::CiPending
        );
        assert_eq!(
            badge_state(false, PrChecks::Pass, PrReviewDecision::Approved),
            PrBadgeState::Approved
        );
        assert_eq!(
            badge_state(false, PrChecks::None, PrReviewDecision::ReviewRequired),
            PrBadgeState::Review
        );
    }

    #[test]
    fn the_scroll_caret_stops_at_both_edges() {
        // The caret is an absolute scrollback line: a viewport of 3 rows starting at line 900
        // confines it to 900..=902, and the edges are where `j` / `k` start scrolling instead.
        // `track_selection` clamps the caret into the viewport every frame, so a key only ever
        // sees one that is already in range; `move_caret` normalizes as a safety net.
        let local = Rc::new(RefCell::new(Local {
            caret: 900,
            ..Local::default()
        }));
        assert!(move_caret(&local, 1, 900, 3));
        assert_eq!(local.borrow().caret, 901);
        assert!(move_caret(&local, 5, 900, 3));
        assert_eq!(local.borrow().caret, 902);
        assert!(!move_caret(&local, 1, 900, 3));
        assert!(move_caret(&local, -9, 900, 3));
        assert_eq!(local.borrow().caret, 900);
        assert!(!move_caret(&local, -1, 900, 3));
    }

    #[test]
    fn the_caret_cannot_move_in_an_empty_grid() {
        let local = Rc::new(RefCell::new(Local::default()));
        assert!(!move_caret(&local, 1, 0, 0));
    }

    #[test]
    fn an_unmeasured_area_falls_back_to_a_conventional_grid() {
        let local = Local::default();
        let cell = gpui::size(px(10.0), px(20.0));
        assert_eq!(local.size_for(TerminalId(1), cell), FALLBACK_GRID);
    }

    #[test]
    fn a_measured_area_decides_the_grid() {
        let local = Local {
            area: Bounds::new(
                gpui::point(px(0.0), px(0.0)),
                gpui::size(px(216.0), px(416.0)),
            ),
            ..Local::default()
        };
        let cell = gpui::size(px(10.0), px(20.0));
        assert_eq!(local.size_for(TerminalId(1), cell), (20, 20));
    }

    #[test]
    fn a_remembered_size_survives_an_unmeasured_frame() {
        let mut local = Local::default();
        local.sizes.insert(TerminalId(7), (100, 30));
        let cell = gpui::size(px(10.0), px(20.0));
        assert_eq!(local.size_for(TerminalId(7), cell), (100, 30));
    }

    #[test]
    fn pre_frame_keys_and_pastes_flush_in_input_order() {
        let key = |character| {
            PendingInput::Key(KeyEvent {
                key: Key::Char(character),
                mods: Modifiers::empty(),
                text: Some(character.to_string()),
                action: KeyAction::Press,
            })
        };
        let mut pending = vec![key('a'), PendingInput::Paste("middle".to_owned()), key('b')];

        let requests = drain_pending_requests(&mut pending, TerminalId(9));

        assert!(pending.is_empty());
        assert!(matches!(
            requests.as_slice(),
            [
                RequestBody::TerminalKey {
                    terminal: TerminalId(9),
                    key: KeyEvent { key: Key::Char('a'), .. },
                },
                RequestBody::PasteTerminal {
                    terminal: TerminalId(9),
                    text,
                },
                RequestBody::TerminalKey {
                    terminal: TerminalId(9),
                    key: KeyEvent { key: Key::Char('b'), .. },
                },
            ] if text == "middle"
        ));
    }

    #[test]
    fn mouse_release_only_consumes_an_active_terminal_drag() {
        let mut local = Local::default();
        // The capture-phase mouse-up-out handler must let watch tab/close clicks bubble.
        assert_eq!(end_mouse_drag(&mut local), None);
        let point = AbsoluteCellPoint::new(3, 1);
        for selected in [false, true] {
            local.mouse_selection = Some(MouseSelection {
                anchor: point,
                head: point,
                initial: AbsoluteCellSelection::new(point, point),
                initiating: point,
                granularity: SelectionGranularity::Cell,
                history_epoch: 7,
                cols: 80,
                alt_screen: false,
                dragging: true,
                selected,
            });
            assert_eq!(end_mouse_drag(&mut local), Some(selected));
            assert_eq!(local.mouse_selection.is_some(), selected);
            // Even a retained selection no longer owns subsequent releases on sibling controls.
            assert_eq!(end_mouse_drag(&mut local), None);
        }
    }

    #[test]
    fn history_epoch_change_clears_selections_and_the_row_cache() {
        let point = AbsoluteCellPoint::new(3, 1);
        let mut local = Local {
            anchor: Some(3),
            anchor_history_epoch: Some(7),
            mouse_selection: Some(MouseSelection {
                anchor: point,
                head: point,
                initial: AbsoluteCellSelection::new(point, point),
                initiating: point,
                granularity: SelectionGranularity::Cell,
                history_epoch: 7,
                cols: 80,
                alt_screen: false,
                dragging: false,
                selected: true,
            }),
            ..Local::default()
        };
        local.row_caches.insert(
            TerminalId(1),
            TerminalRowCache {
                cols: 80,
                alt_screen: false,
                history_epoch: 7,
                last_seq: 4,
                last_viewport_base: 2,
                rows: BTreeMap::new(),
            },
        );

        assert!(invalidate_history_epoch(&mut local, Some(8)));
        assert!(local.mouse_selection.is_none());
        assert!(local.anchor.is_none());
        assert!(local.row_caches.is_empty());
    }

    #[test]
    fn unchanged_history_epoch_keeps_selections_and_the_row_cache() {
        let point = AbsoluteCellPoint::new(3, 1);
        let mut local = Local {
            mouse_selection: Some(MouseSelection {
                anchor: point,
                head: point,
                initial: AbsoluteCellSelection::new(point, point),
                initiating: point,
                granularity: SelectionGranularity::Cell,
                history_epoch: 7,
                cols: 80,
                alt_screen: false,
                dragging: false,
                selected: true,
            }),
            ..Local::default()
        };
        local.row_caches.insert(
            TerminalId(1),
            TerminalRowCache {
                cols: 80,
                alt_screen: false,
                history_epoch: 7,
                last_seq: 4,
                last_viewport_base: 2,
                rows: BTreeMap::new(),
            },
        );

        assert!(!invalidate_history_epoch(&mut local, Some(7)));
        assert!(local.mouse_selection.is_some());
        assert_eq!(local.row_caches.len(), 1);
    }

    #[test]
    fn row_cache_is_untouched_without_a_selection_and_retains_only_selected_rows() {
        let mut grid = MirrorGrid::new(4, 4);
        grid.seq = 3;
        let retained = cached_grid_row(&grid, 0)
            .unwrap_or_else(|| panic!("an initialized mirror row must exist"));
        let mut cache = TerminalRowCache {
            cols: 4,
            alt_screen: false,
            history_epoch: 0,
            last_seq: 1,
            last_viewport_base: 99,
            rows: BTreeMap::from([(99, retained)]),
        };

        cache_selected_grid_rows(&mut cache, &grid, 100, None);
        assert_eq!(cache.rows.keys().copied().collect::<Vec<_>>(), vec![99]);
        assert_eq!((cache.last_seq, cache.last_viewport_base), (1, 99));

        cache_selected_grid_rows(&mut cache, &grid, 100, Some((102, 101)));
        assert_eq!(
            cache.rows.keys().copied().collect::<Vec<_>>(),
            vec![101, 102]
        );
        assert_eq!((cache.last_seq, cache.last_viewport_base), (3, 100));
    }
}
