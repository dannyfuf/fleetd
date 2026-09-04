//! The Workspace: one session's terminals, its header, tab strip and grid (UX-SPEC §3.6).
//!
//! # What this screen owns
//!
//! Everything drawn between the shell's context bar and its status bar while a session is open,
//! and — because no other module can — the two behaviours `docs/APP-CONTRACTS.md` §6 assigns to
//! it by name:
//!
//! * **Keys reach the PTY only in Terminal mode.** `Workspace > Terminal` binds exactly one key,
//!   `ctrl-s`, so every other keystroke falls through gpui's binding pass into this element's
//!   `on_key_down`, is turned into a [`KeyEvent`] and is sent to the daemon, which owns the
//!   mode-aware encoder. A key typed in `Prefix` or `Scroll` is *dropped here*, never forwarded:
//!   those modes exist precisely so that `x` closes a tab instead of typing an `x`.
//! * **Keys are dropped, never buffered, while the daemon is gone** (§3.12 C). A buffer that
//!   replayed twenty keystrokes into a shell the moment it reconnected would be a hazard, not a
//!   convenience. The one buffer that does exist is the opposite case: keys typed *while
//!   attaching* are held until the first frame lands, because that PTY is alive and listening.
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
    collections::HashMap,
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
use fleet_proto::{
    job::{JobRecord, JobStatus},
    request::RequestBody,
    response::ResponseBody,
    terminal::{Key, KeyAction, KeyEvent, Modifiers, ScrollCommand},
};
use fleet_ui_kit::{
    ActiveTheme, ExitStrip, Icon, IconSize, KeyHintRow, PrBadgeState, PrefixHint, ScrollPill,
    StatusKind, TerminalGrid, TerminalTabStrip, Text, Tone,
};
use gpui::{
    AnyElement, App, ClipboardItem, Div, Entity, FocusHandle, KeyDownEvent, Keystroke, Pixels,
    SharedString, Size, Window, div, prelude::*, px,
};

use crate::{
    actions::{fleet, prefix, scroll},
    bridge::Bridge,
    state::{AppState, Overlay, Screen, TerminalMode},
    terminal_element::{
        GRID_PADDING, cell_size, grid_cursor, grid_rows, grid_size, line_selection, measure,
        selection_text, zoom_bar,
    },
    views::{workspace_header::WorkspaceHeader, workspace_tabs},
};

/// The size a terminal is attached at before the grid has ever been laid out.
///
/// The first measured frame replaces it, so this is only ever the argument of the very first
/// `AttachTerminal`; 80 × 24 is the size every program already copes with.
const FALLBACK_GRID: (u16, u16) = (80, 24);

/// How long a `ctrl-s x` on a keep-alive terminal stays armed before it forgets the request.
const CLOSE_CONFIRM_DWELL: Duration = Duration::from_secs(6);

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
    /// The `cols × rows` the daemon was last told about, per terminal.
    sizes: HashMap<TerminalId, (u16, u16)>,
    /// The pixel area the grid was last laid out into.
    area: Size<Pixels>,
    /// Keys typed before the first frame landed. Flushed in order, then never used again.
    pending: Vec<KeyEvent>,
    /// The anchor row of a scroll-mode selection, in viewport coordinates.
    anchor: Option<u16>,
    /// The scroll-mode caret row, which `j` / `k` move and a selection extends to.
    caret: u16,
    /// The in-progress `ctrl-s ,` rename, and the terminal it renames.
    rename: Option<(TerminalId, String)>,
    /// A `ctrl-s x` waiting for its confirmation, and when it was armed.
    close_armed: Option<(TerminalId, Instant)>,
    /// Whether the 400 ms prefix-hint timer is already running for this prefix.
    hint_armed: bool,
    /// Whether that timer has fired.
    hint_visible: bool,
    /// The pull request of a branch, once looked up. A `None` value means "no pull request".
    prs: HashMap<String, Option<(u64, PrBadgeState)>>,
    /// Repositories whose pull requests have already been asked for.
    pr_requested: Vec<RepoId>,
}

impl Local {
    /// The size to attach `terminal` at: the measured one, else the last one, else the fallback.
    fn size_for(&self, terminal: TerminalId, cell: Size<Pixels>) -> (u16, u16) {
        if self.area.width <= px(0.0) || self.area.height <= px(0.0) {
            return self.sizes.get(&terminal).copied().unwrap_or(FALLBACK_GRID);
        }
        grid_size(self.area, cell)
    }
}

/// The Workspace screen.
pub struct WorkspaceScreen {
    local: Rc<RefCell<Local>>,
}

impl WorkspaceScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self {
            local: Rc::new(RefCell::new(Local::default())),
        }
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
        let Some(session) = state.read(cx).active_session().cloned() else {
            return self.empty(focus, cx);
        };
        let model = Model::build(state.read(cx), &session);
        let cell = cell_size(cx.theme());

        self.reconcile(&model, bridge, cell);
        self.arm_prefix_hint(&model, state, cx);
        self.lookup_pr(&model, bridge, state, cx);

        let focused = focus.is_focused(window);
        let header = (!model.zoomed).then(|| self.header(&model, cx));
        let tabs = (!model.zoomed).then(|| self.tab_strip(&model, &session, cx));
        let body = self.terminal_area(&model, bridge, cell, state, focused, cx);
        let confirm = self.close_confirm_strip(&model, cx);
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
            .children(model.exit_code.map(ExitStrip::new))
            .children(confirm);

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
        if local.attached != model.terminal {
            if let Some(previous) = local.attached.take() {
                bridge.send(RequestBody::DetachTerminal { terminal: previous });
            }
            local.pending.clear();
            local.anchor = None;
            local.rename = None;
            local.close_armed = None;
            if let Some(terminal) = model.terminal {
                let (cols, rows) = local.size_for(terminal, cell);
                local.sizes.insert(terminal, (cols, rows));
                bridge.send(RequestBody::AttachTerminal {
                    terminal,
                    cols,
                    rows,
                });
            }
            local.attached = model.terminal;
        }

        // A `ctrl-s x` that was never confirmed forgets itself rather than staying armed behind
        // the user's back.
        if let Some((_, armed_at)) = local.close_armed
            && Instant::now().saturating_duration_since(armed_at) >= CLOSE_CONFIRM_DWELL
        {
            local.close_armed = None;
        }

        // §3.6 "Attaching": keys typed before the first frame are flushed in order once the
        // mirror is primed, and dropped if the terminal went away in the meantime.
        if model.primed && !local.pending.is_empty() {
            match model.terminal {
                Some(terminal) => {
                    for key in local.pending.drain(..) {
                        bridge.send(RequestBody::TerminalKey { terminal, key });
                    }
                }
                None => local.pending.clear(),
            }
        }

        // The caret cannot survive outside a grid that shrank under it.
        let last_row = model.rows.saturating_sub(1);
        if local.caret > last_row {
            local.caret = last_row;
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

    /// Asks the daemon once per repository which pull request the open branch belongs to.
    ///
    /// TODO(integration): the snapshot carries no branch → pull-request association, so the
    /// header would otherwise have no badge at all. See the integration request in the report.
    fn lookup_pr(&self, model: &Model, bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
        let (Some(repo), Some(branch)) = (model.repo.clone(), model.branch_key.clone()) else {
            return;
        };
        {
            let local = self.local.borrow();
            if local.prs.contains_key(&branch) || local.pr_requested.contains(&repo) {
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
        let local = Rc::clone(&self.local);
        let state = state.clone();
        cx.spawn(async move |cx| {
            let Ok(Ok(ResponseBody::PullRequests(slices))) = reply.recv().await else {
                return;
            };
            {
                let mut local = local.borrow_mut();
                for slice in &slices {
                    for pr in &slice.prs {
                        let badge = badge_state(pr.is_draft, pr.checks, pr.review_decision);
                        local
                            .prs
                            .insert(pr.head_ref_name.clone(), Some((pr.number, badge)));
                    }
                }
                local.prs.entry(branch).or_insert(None);
            }
            state.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    // ------------------------------------------------------------------ pieces

    fn header(&self, model: &Model, cx: &mut App) -> AnyElement {
        let pr = model
            .branch_key
            .as_ref()
            .and_then(|branch| self.local.borrow().prs.get(branch).copied())
            .flatten();
        let mut header = WorkspaceHeader::new(model.title.clone())
            .status(model.status)
            .keep_alive(model.keep_alive.iter().cloned())
            .jobs(model.running_jobs, model.failed_jobs)
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

    fn tab_strip(&self, model: &Model, session: &Session, cx: &mut App) -> AnyElement {
        let renaming = self.local.borrow().rename.clone();
        let mut tabs = workspace_tabs::tabs(session, model.terminal);
        if let Some((terminal, draft)) = &renaming
            && let Some(position) = workspace_tabs::position_of(session, *terminal)
            && let Some(tab) = tabs.get_mut(position)
        {
            // The caret block is the only affordance a rename needs: the strip already says
            // which tab it is and `Esc` / `Enter` are the only two keys that end it.
            tab.name = SharedString::from(format!("{draft}\u{2588}"));
        }
        let active = model
            .terminal
            .and_then(|terminal| workspace_tabs::position_of(session, terminal))
            .unwrap_or(0);
        let _unused = cx;
        TerminalTabStrip::new(tabs)
            .active(active)
            .into_any_element()
    }

    /// The grid, its overlays, and the invisible element that measures it.
    fn terminal_area(
        &self,
        model: &Model,
        bridge: &Bridge,
        cell: Size<Pixels>,
        state: &Entity<AppState>,
        focused: bool,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let app = state.read(cx);
        let mirror = app.active_grid();
        let selection = mirror.and_then(|grid| {
            let local = self.local.borrow();
            local
                .anchor
                .map(|anchor| line_selection(grid, anchor, local.caret))
        });
        let hint_visible = self.local.borrow().hint_visible;

        let grid: AnyElement = match mirror.filter(|grid| grid.primed) {
            Some(grid) => {
                let mut painted = TerminalGrid::new(grid_rows(grid, theme))
                    .cursor(grid_cursor(grid, focused))
                    .focused(focused)
                    .padding(px(GRID_PADDING))
                    .scrollback(grid.viewport.offset, grid.viewport.scrollback_len);
                if let Some(selection) = selection {
                    painted = painted.selection(selection);
                }
                painted.into_any_element()
            }
            // §3.6 "Attaching": one dim centered line, and the keys typed meanwhile are held.
            None => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(theme.terminal.background)
                .child(Text::ui("attaching\u{2026}").muted())
                .into_any_element(),
        };

        let local = Rc::clone(&self.local);
        let bridge = bridge.clone();
        let terminal = model.terminal;
        let measurer = measure(move |size| {
            let mut local = local.borrow_mut();
            local.area = size;
            let Some(terminal) = terminal else {
                return;
            };
            let (cols, rows) = grid_size(size, cell);
            if local.sizes.get(&terminal) != Some(&(cols, rows)) {
                local.sizes.insert(terminal, (cols, rows));
                bridge.send(RequestBody::ResizeTerminal {
                    terminal,
                    cols,
                    rows,
                });
            }
        });

        div()
            .relative()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(grid)
            .children((model.mode == TerminalMode::Scroll).then(|| {
                ScrollPill::new(model.scroll_offset, model.scrollback_len)
                    .selecting(selection.is_some())
                    .alt_screen(model.alt_screen)
            }))
            .child(
                PrefixHint::new(model.mode == TerminalMode::Prefix && hint_visible)
                    .hints(prefix_hints()),
            )
            .child(measurer)
            .into_any_element()
    }

    /// The inline confirmation `ctrl-s x` raises over a terminal that keeps the session awake.
    ///
    /// TODO(integration): §3.8.3 wants this in the shared Confirm dialog, but `Dialogs::Confirm`
    /// carries no payload and its outcome is not routed back to the requester. Until that lands
    /// the Workspace confirms in its own key contract, with `^s`-prefixed affordances per [D-8].
    fn close_confirm_strip(&self, model: &Model, cx: &mut App) -> Option<AnyElement> {
        let armed = self.local.borrow().close_armed?;
        if Some(armed.0) != model.terminal {
            return None;
        }
        let theme = cx.theme();
        let labels: Vec<String> = model
            .keep_alive
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        Some(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .h(theme.metrics.strip_h)
                .w_full()
                .flex_none()
                .px(theme.space.md)
                .bg(Tone::Danger.fill(theme))
                .border_t(px(1.0))
                .border_color(theme.colors.border)
                .child(
                    Icon::TriangleAlert
                        .el()
                        .size(IconSize::Small)
                        .color(theme.colors.danger),
                )
                .child(
                    Text::ui(format!("{} keeps this session awake", labels.join(", ")))
                        .tone(Tone::Danger),
                )
                .child(
                    KeyHintRow::new()
                        .key("^s x", "close anyway")
                        .key("Esc", "cancel"),
                )
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------------ keys and actions

    /// Installs every listener the Workspace owns on the focused element.
    fn with_keys(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let root = self.with_key_forwarding(root, bridge, state);
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

            // An unbound key in Prefix or Scroll belongs to the mode, never to the PTY: it is
            // dropped here. Propagation is deliberately **not** stopped — gpui runs the
            // keystroke observers only while the event still propagates, and the shell's
            // observer is what makes the prefix one-shot, so swallowing the event would leave
            // an unbound `ctrl-s d` stuck in Prefix mode forever.
            if mode != TerminalMode::Terminal {
                return;
            }
            if rename_key(&local, &bridge, &state, event, cx) {
                cx.stop_propagation();
                return;
            }
            if event.keystroke.key == "escape" && local.borrow().close_armed.is_some() {
                local.borrow_mut().close_armed = None;
                state.update(cx, |_, cx| cx.notify());
                cx.stop_propagation();
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
            if primed {
                bridge.send(RequestBody::TerminalKey { terminal, key });
            } else {
                local.borrow_mut().pending.push(key);
            }
            cx.stop_propagation();
        })
    }

    /// Every `ctrl-s <key>` binding the shell does not already own.
    fn with_prefix_actions(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SendLiteral, _window, cx| {
                if let Some(terminal) = active_terminal(&state, cx) {
                    bridge.send(RequestBody::TerminalKey {
                        terminal,
                        key: KeyEvent {
                            key: Key::Char('s'),
                            mods: Modifiers::CTRL,
                            text: None,
                            action: KeyAction::Press,
                        },
                    });
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
        let root = self.tab_actions(root, bridge, state);
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::NewTerminal, _window, cx| {
                if let Some(session) = state.read(cx).active_session() {
                    bridge.send(RequestBody::NewTerminal {
                        session: session.id.clone(),
                        name: workspace_tabs::new_terminal_name(session),
                        command: String::new(),
                        cwd: session.cwd.clone(),
                    });
                }
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::CloseTerminal, _window, cx| {
                let Some(terminal) = active_terminal_record(&state, cx) else {
                    return;
                };
                let armed = local.borrow().close_armed.map(|(id, _)| id);
                if terminal.keep_alive.is_empty() || armed == Some(terminal.id) {
                    local.borrow_mut().close_armed = None;
                    bridge.send(RequestBody::CloseTerminal {
                        terminal: terminal.id,
                    });
                } else {
                    local.borrow_mut().close_armed = Some((terminal.id, Instant::now()));
                }
                state.update(cx, |_, cx| cx.notify());
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
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::RenameTerminal, _window, cx| {
                if let Some(terminal) = active_terminal_record(&state, cx) {
                    local.borrow_mut().rename = Some((terminal.id, terminal.name.clone()));
                    state.update(cx, |_, cx| cx.notify());
                }
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::Paste, _window, cx| {
                let Some(terminal) = active_terminal(&state, cx) else {
                    return;
                };
                // The daemon brackets the paste when the program asked for bracketed paste.
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return;
                };
                bridge.send(RequestBody::PasteTerminal { terminal, text });
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
                // TODO(integration): the palette has no seed field, so `ctrl-s W` opens it
                // unfiltered instead of pre-filtered to `GO`/sessions (KEYMAP A4).
                state.update(cx, |app, cx| {
                    app.leave_prefix();
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
        macro_rules! scroll_by {
            ($root:expr, $action:ty, $command:expr) => {{
                let (_, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _window, cx| {
                    scroll_viewport(&bridge, &state, $command, cx);
                })
            }};
        }
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::HalfPageDown, _window, cx| {
                let half = half_page(&state, cx);
                scroll_viewport(&bridge, &state, ScrollCommand::Lines(half), cx);
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::HalfPageUp, _window, cx| {
                let half = half_page(&state, cx);
                scroll_viewport(&bridge, &state, ScrollCommand::Lines(-half), cx);
            })
        };
        let root = scroll_by!(root, scroll::PageDown, ScrollCommand::Pages(1));
        let root = scroll_by!(root, scroll::PageUp, ScrollCommand::Pages(-1));
        let root = scroll_by!(root, scroll::Top, ScrollCommand::Top);
        let root = scroll_by!(root, scroll::Bottom, ScrollCommand::Bottom);

        let root = {
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::StartSelection, _window, cx| {
                let caret = local.borrow().caret;
                local.borrow_mut().anchor = Some(caret);
                state.update(cx, |_, cx| cx.notify());
            })
        };
        let root = {
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Yank, _window, cx| {
                let text = {
                    let borrowed = local.borrow();
                    let app = state.read(cx);
                    borrowed
                        .anchor
                        .zip(app.active_grid())
                        .map(|(anchor, grid)| {
                            selection_text(grid, line_selection(grid, anchor, borrowed.caret))
                        })
                };
                let Some(text) = text else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                local.borrow_mut().anchor = None;
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
                if local.borrow_mut().anchor.take().is_some() {
                    state.update(cx, |_, cx| cx.notify());
                    return;
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
fn detach(local: &Rc<RefCell<Local>>, bridge: &Bridge) {
    let attached = local.borrow_mut().attached.take();
    if let Some(terminal) = attached {
        bridge.send(RequestBody::DetachTerminal { terminal });
    }
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
    let rows = visible_rows(state, cx);
    if move_caret(local, delta, rows) {
        state.update(cx, |_, cx| cx.notify());
        return;
    }
    scroll_viewport(
        bridge,
        state,
        ScrollCommand::Lines(i32::try_from(delta).unwrap_or(0)),
        cx,
    );
}

/// Leaves Scroll mode: the viewport snaps back to the live bottom (KEYMAP §Scroll).
fn exit_scroll(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    local.borrow_mut().anchor = None;
    if let Some(terminal) = active_terminal(state, cx) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Bottom,
        });
    }
    state.update(cx, |app, cx| {
        if app.terminal_mode == TerminalMode::Scroll {
            app.terminal_mode = TerminalMode::Terminal;
        }
        cx.notify();
    });
}

/// Moves the scroll caret, returning whether it moved at all.
fn move_caret(local: &Rc<RefCell<Local>>, delta: isize, rows: u16) -> bool {
    if rows == 0 {
        return false;
    }
    let mut local = local.borrow_mut();
    let last = rows.saturating_sub(1);
    let current = local.caret.min(last);
    let last = isize::try_from(last).unwrap_or(isize::MAX);
    let next = (isize::try_from(current).unwrap_or(0) + delta).clamp(0, last);
    let next = u16::try_from(next).unwrap_or(0);
    if next == local.caret {
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
    let Some(session) = state.read(cx).active_session().map(|s| s.id.clone()) else {
        return;
    };
    bridge.send(RequestBody::SelectTerminal {
        session: session.clone(),
        terminal,
    });
    local.borrow_mut().anchor = None;
    state.update(cx, |app, cx| {
        app.touch_terminal(&session, terminal);
        // The snapshot decides which tab is really active; this only keeps the mode honest
        // while the round trip is in flight.
        app.terminal_mode = TerminalMode::Terminal;
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

/// Handles one key of the inline `ctrl-s ,` rename, returning whether it was consumed.
///
/// The rename lives in the Workspace's own key contract rather than in a dialog: `Enter` commits,
/// `Esc` abandons and every other printable key edits the draft the tab strip is showing. `ctrl-s`
/// still opens the prefix while a rename is in flight — it is the Workspace's one escape key and
/// stays reachable — so a prefix key typed mid-rename acts and the draft is abandoned by `Esc`.
///
/// TODO(integration): a `Dialogs::RenameTerminal` variant would let this reuse the shared dialog
/// frame of §3.8; the enum is frozen, so the request is filed instead.
fn rename_key(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    event: &KeyDownEvent,
    cx: &mut App,
) -> bool {
    let Some((terminal, mut draft)) = local.borrow().rename.clone() else {
        return false;
    };
    match event.keystroke.key.as_str() {
        "escape" => local.borrow_mut().rename = None,
        "enter" => {
            local.borrow_mut().rename = None;
            let name = draft.trim().to_owned();
            if !name.is_empty() {
                bridge.send(RequestBody::RenameTerminal { terminal, name });
            }
        }
        "backspace" => {
            draft.pop();
            local.borrow_mut().rename = Some((terminal, draft));
        }
        _ => {
            let Some(text) = event.keystroke.key_char.as_deref() else {
                return true;
            };
            if text.chars().any(char::is_control) {
                return true;
            }
            draft.push_str(text);
            local.borrow_mut().rename = Some((terminal, draft));
        }
    }
    state.update(cx, |_, cx| cx.notify());
    true
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
    mode: TerminalMode,
    zoomed: bool,
    terminal: Option<TerminalId>,
    primed: bool,
    alt_screen: bool,
    rows: u16,
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
            mode: app.terminal_mode,
            zoomed: app.zoomed,
            terminal,
            primed: grid.is_some_and(|grid| grid.primed),
            alt_screen: grid.is_some_and(|grid| grid.modes.alt_screen),
            rows: grid.map_or(0, |grid| grid.rows),
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
            waking: session.slept_at.is_some()
                && session
                    .terminals
                    .iter()
                    .any(|terminal| terminal.status == TerminalStatus::Starting),
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
        let local = Rc::new(RefCell::new(Local::default()));
        assert!(move_caret(&local, 1, 3));
        assert_eq!(local.borrow().caret, 1);
        assert!(move_caret(&local, 5, 3));
        assert_eq!(local.borrow().caret, 2);
        // At the bottom edge the caret no longer moves, which is what makes `j` scroll instead.
        assert!(!move_caret(&local, 1, 3));
        assert!(move_caret(&local, -9, 3));
        assert_eq!(local.borrow().caret, 0);
        assert!(!move_caret(&local, -1, 3));
    }

    #[test]
    fn the_caret_cannot_move_in_an_empty_grid() {
        let local = Rc::new(RefCell::new(Local::default()));
        assert!(!move_caret(&local, 1, 0));
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
            area: gpui::size(px(216.0), px(416.0)),
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
}
