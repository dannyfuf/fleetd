//! The screen-independent floating agent terminal.
//!
//! The popup adds its terminal to this client's attachment set: a Workspace underneath remains
//! rendered and keeps its own terminal in that set. Hiding releases only a popup-exclusive
//! attachment; the fixed daemon agent session and its scrollback remain alive for the next open.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::{
    config::Agent,
    ids::{SessionId, TerminalId},
    sessions::{AgentActivity, TerminalStatus, agent_session_id},
};
use fleet_proto::{
    request::RequestBody,
    terminal::{Key, KeyAction, KeyEvent, Modifiers, ScrollCommand, WheelEvent},
};
use fleet_ui_kit::{
    ActiveTheme, CellMetrics, ExitStrip, KeyHintRow, Overlay as FloatingOverlay, OverlayLayer,
    PrefixHint, ScrollPill, StatusDot, TerminalGrid, Text, Tone, Veil,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Div, Entity, FocusHandle, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, Size, Window, div,
    prelude::*, px,
};

use crate::{
    actions::{agent, prefix, scroll},
    bridge::Bridge,
    screens::workspace::key_event,
    state::{AgentPopupMode, AppState, Screen},
    terminal_element::{
        AbsoluteCellPoint, AbsoluteCellSelection, CachedGridRow, CellPoint, GRID_PADDING,
        SelectionGranularity, WheelAccumulator, absolute_selection_at, absolute_selection_text,
        cached_grid_row, cell_at_position, cell_size, extend_absolute_selection, grid_cursor,
        grid_modes, grid_rows, grid_size, line_selection, measure, selection_text, viewport_base,
        viewport_cell_selection, viewport_last,
    },
};

const FALLBACK_GRID: (u16, u16) = (80, 24);
const CACHE_CAP: usize = 5_000;
const SELECTION_LINE_CAP: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingOwner {
    agent: Agent,
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
struct MouseSelection {
    terminal: TerminalId,
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

#[derive(Debug, Default)]
struct Local {
    attached: Option<TerminalId>,
    attached_generation: u64,
    size: Option<(u16, u16)>,
    area: Bounds<Pixels>,
    geometry: Option<(TerminalId, Bounds<Pixels>, CellMetrics)>,
    wheel: WheelAccumulator,
    pending_owner: Option<PendingOwner>,
    pending: Vec<PendingInput>,
    mouse_selection: Option<MouseSelection>,
    rows: BTreeMap<u64, CachedGridRow>,
    anchor: Option<u64>,
    caret: u64,
    anchor_history_epoch: Option<u64>,
    anchor_cols: Option<u16>,
    anchor_alt_screen: Option<bool>,
    history: BTreeMap<u64, String>,
    hint_armed: bool,
    hint_visible: bool,
}

impl Local {
    fn clear_selections(&mut self) -> bool {
        let changed = self.mouse_selection.take().is_some() || self.anchor.take().is_some();
        self.rows.clear();
        self.history.clear();
        self.anchor_history_epoch = None;
        self.anchor_cols = None;
        self.anchor_alt_screen = None;
        changed
    }

    fn discard_pending(&mut self) {
        self.pending_owner = None;
        self.pending.clear();
    }

    fn queue_input(&mut self, owner: PendingOwner, input: PendingInput) {
        if self.pending_owner != Some(owner) {
            self.discard_pending();
            self.pending_owner = Some(owner);
        }
        self.pending.push(input);
    }

    fn take_pending(&mut self, owner: PendingOwner) -> Vec<PendingInput> {
        if self.pending_owner != Some(owner) {
            return Vec::new();
        }
        self.pending_owner = None;
        std::mem::take(&mut self.pending)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingInput {
    Key(KeyEvent),
    Paste(String),
}

impl PendingInput {
    fn request(self, terminal: TerminalId) -> RequestBody {
        match self {
            Self::Key(key) => RequestBody::TerminalKey { terminal, key },
            Self::Paste(text) => RequestBody::PasteTerminal { terminal, text },
        }
    }
}

#[derive(Clone)]
struct Model {
    agent: Agent,
    session: SessionId,
    mode: AgentPopupMode,
    terminal: Option<TerminalId>,
    base_terminal: Option<TerminalId>,
    generation: u64,
    primed: bool,
    reachable: bool,
    cols: Option<u16>,
    history_epoch: Option<u64>,
    alt_screen: bool,
    scroll_offset: usize,
    scrollback_len: usize,
    exit_code: Option<Option<i32>>,
    activity: AgentActivity,
}

impl Model {
    fn build(app: &AppState) -> Option<Self> {
        let popup = app.agent_popup?;
        let session = agent_session_id(popup.agent).ok()?;
        let record = app
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.sessions.iter().find(|entry| entry.id == session));
        let terminal_record = record.and_then(|session| session.terminals.first());
        let terminal = terminal_record.map(|terminal| terminal.id);
        let grid = terminal.and_then(|terminal| app.grids.get(&terminal));
        let base_terminal = match &app.screen {
            Screen::Workspace { session } => app.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .sessions
                    .iter()
                    .find(|entry| &entry.id == session)
                    .and_then(|entry| entry.active_terminal)
            }),
            Screen::Hub { .. } => None,
        };
        let exit_code = terminal_record.and_then(|terminal| match terminal.status {
            TerminalStatus::Exited { code } => Some(code),
            TerminalStatus::Starting | TerminalStatus::Running => None,
        });
        let activity = app.session_agent_activity(&session);
        Some(Self {
            agent: popup.agent,
            session,
            mode: popup.mode,
            terminal,
            base_terminal,
            generation: app.link_generation,
            primed: grid.is_some_and(|grid| grid.primed),
            reachable: app.daemon.is_connected(),
            cols: grid.map(|grid| grid.cols),
            history_epoch: grid.map(|grid| grid.viewport.history_epoch),
            alt_screen: grid.is_some_and(|grid| grid.modes.alt_screen),
            scroll_offset: grid.map_or(0, |grid| grid.viewport.offset),
            scrollback_len: grid.map_or(0, |grid| grid.viewport.scrollback_len),
            exit_code,
            activity,
        })
    }
}

/// The window-wide floating agent surface.
pub struct AgentPopup {
    local: Rc<RefCell<Local>>,
}

/// Cloneable popup control used only by the shell's independent `ctrl-q` safety guard.
#[derive(Clone)]
pub(crate) struct AgentPopupInput {
    local: Rc<RefCell<Local>>,
}

impl AgentPopupInput {
    /// Hides and detaches the popup without consulting GPUI's rendered focus path.
    pub(crate) fn hide(&self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
        hide(&self.local, state, bridge, cx);
    }
}

impl AgentPopup {
    #[must_use]
    pub fn new(_cx: &mut App) -> Self {
        Self {
            local: Rc::new(RefCell::new(Local::default())),
        }
    }

    /// Releases this client's terminal attachment without touching the daemon session.
    pub fn detach(&self, bridge: &Bridge, preserve: Option<TerminalId>) {
        if let Some(terminal) = self.local.borrow_mut().attached.take()
            && Some(terminal) != preserve
        {
            bridge.send(RequestBody::DetachTerminal { terminal });
        }
        let mut local = self.local.borrow_mut();
        local.clear_selections();
        local.discard_pending();
    }

    /// A cloneable handle used by the shell's popup `ctrl-q` guard.
    #[must_use]
    pub(crate) fn input(&self) -> AgentPopupInput {
        AgentPopupInput {
            local: Rc::clone(&self.local),
        }
    }

    /// Drops creation-time input when the daemon link is lost.
    pub(crate) fn discard_pending(&self) {
        self.local.borrow_mut().discard_pending();
    }

    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(model) = Model::build(state.read(cx)) else {
            return div().into_any_element();
        };
        self.reconcile(&model, bridge, cell_size(cx.theme()));
        self.cache_selection(&model, state, cx);
        self.track_selection(&model, state, cx);
        self.arm_prefix_hint(&model, state, cx);

        let width = px(f32::from(window.viewport_size().width) * 0.9);
        let height = px(f32::from(window.viewport_size().height) * 0.85);
        let top = px(f32::from(window.viewport_size().height) * 0.075);
        let header = self.header(&model, cx);
        let terminal = Veil::new(!model.reachable)
            .child(self.terminal_area(&model, state, bridge, focus, window, cx));
        let exit = model
            .exit_code
            .map(|code| ExitStrip::new(code).hints(KeyHintRow::new().key("^s r", "restart")));
        let theme = cx.theme().clone();
        let card = div()
            .track_focus(focus)
            .flex()
            .flex_col()
            .w_full()
            .h(height)
            .min_h_0()
            .bg(theme.colors.elevated)
            .child(header)
            .child(terminal)
            .children(exit);

        FloatingOverlay::new()
            .top(top)
            .width(width)
            .scrim(true)
            .layer(OverlayLayer::Dialog)
            .child(self.with_keys(card, state, bridge))
            .into_any_element()
    }

    fn header(&self, model: &Model, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let label = match model.agent {
            Agent::Claude => "claude",
            Agent::Opencode => "opencode",
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(theme.metrics.dialog_header_h)
            .px(theme.space.md)
            .gap(theme.space.sm)
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .child(StatusDot::new(header_status_tone(model)))
            .child(Text::ui_strong(label))
            .child(Text::data(model.session.to_string()).muted().ellipsize())
            .child(div().flex_1())
            .child(
                KeyHintRow::new()
                    .key("^s q", "hide")
                    .key("^s a/A", "switch")
                    .key("ctrl-q", "hide"),
            )
            .into_any_element()
    }

    fn reconcile(&self, model: &Model, bridge: &Bridge, cell: Size<Pixels>) {
        let mut local = self.local.borrow_mut();
        let owner = PendingOwner {
            agent: model.agent,
            generation: model.generation,
        };
        if !model.reachable || local.pending_owner.is_some_and(|pending| pending != owner) {
            local.discard_pending();
        }
        local.wheel.reconcile(model.terminal);
        let relinked = local.attached_generation != model.generation;
        if local.attached != model.terminal || relinked {
            if let Some(previous) = local.attached.take()
                && !relinked
                && Some(previous) != model.base_terminal
            {
                bridge.send(RequestBody::DetachTerminal { terminal: previous });
            }
            local.clear_selections();
            if let Some(terminal) = model.terminal {
                let (cols, rows) =
                    if local.area.size.width > px(0.0) && local.area.size.height > px(0.0) {
                        grid_size(local.area.size, cell)
                    } else {
                        local.size.unwrap_or(FALLBACK_GRID)
                    };
                local.size = Some((cols, rows));
                bridge.send(RequestBody::AttachTerminal {
                    terminal,
                    cols,
                    rows,
                });
            }
            local.attached = model.terminal;
            local.attached_generation = model.generation;
        }
        if model.primed
            && (local.mouse_selection.is_some_and(|selection| {
                Some(selection.cols) != model.cols
                    || selection.alt_screen != model.alt_screen
                    || Some(selection.history_epoch) != model.history_epoch
            }) || (local.anchor.is_some()
                && (local.anchor_history_epoch != model.history_epoch
                    || local.anchor_cols != model.cols
                    || local.anchor_alt_screen != Some(model.alt_screen))))
        {
            local.clear_selections();
        }
        if model.reachable
            && model.primed
            && let Some(terminal) = model.terminal
            && local.pending_owner == Some(owner)
        {
            for input in local.take_pending(owner) {
                bridge.send(input.request(terminal));
            }
        }
    }

    fn terminal_area(
        &self,
        model: &Model,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let owner = PendingOwner {
            agent: model.agent,
            generation: model.generation,
        };
        let app = state.read(cx);
        let mirror = model.terminal.and_then(|terminal| app.grids.get(&terminal));
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
        let focused = focus.is_focused(window);
        let hint_visible = self.local.borrow().hint_visible;
        let grid: AnyElement = match mirror.filter(|grid| grid.primed) {
            Some(grid) => {
                let terminal = model.terminal;
                let geometry = Rc::clone(&self.local);
                let resize = Rc::clone(&self.local);
                let resize_bridge = bridge.clone();
                let mut painted = TerminalGrid::new(grid_rows(grid, theme))
                    .id("agent-popup-terminal-grid")
                    .cursor(grid_cursor(grid, focused))
                    .focused(focused)
                    .modes(grid_modes(&grid.modes))
                    .padding(px(GRID_PADDING))
                    .scrollback(grid.viewport.offset, grid.viewport.scrollback_len)
                    .frame_size(usize::from(grid.cols), usize::from(grid.rows))
                    .on_geometry(move |bounds, metrics| {
                        if let Some(terminal) = terminal {
                            geometry.borrow_mut().geometry = Some((terminal, bounds, metrics));
                        }
                    })
                    .on_resize(move |cols, rows, _window, _cx| {
                        let Some(terminal) = terminal else { return };
                        let size = (
                            u16::try_from(cols).unwrap_or(u16::MAX),
                            u16::try_from(rows).unwrap_or(u16::MAX),
                        );
                        let mut local = resize.borrow_mut();
                        if local.size != Some(size) {
                            local.size = Some(size);
                            resize_bridge.send(RequestBody::ResizeTerminal {
                                terminal,
                                cols: size.0,
                                rows: size.1,
                            });
                        }
                    });
                if let Some(selection) = selection {
                    painted = painted.selection(selection);
                }
                painted.into_any_element()
            }
            None => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(theme.terminal.background)
                .child(Text::ui("attaching\u{2026}").muted())
                .into_any_element(),
        };
        let area_local = Rc::clone(&self.local);
        let wheel_local = Rc::clone(&self.local);
        let wheel_bridge = bridge.clone();
        let wheel_state = state.clone();
        let terminal = model.terminal;
        let area = div()
            .id("agent-popup-terminal-area")
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _, cx| {
                let Some(terminal) = terminal else { return };
                let app = wheel_state.read(cx);
                if !agent_terminal_is_live_owner(app, owner, Some(terminal)) {
                    cx.stop_propagation();
                    return;
                }
                if app.drops_terminal_keys() {
                    return;
                }
                let Some(grid) = app.grids.get(&terminal).filter(|grid| grid.primed) else {
                    return;
                };
                let mut local = wheel_local.borrow_mut();
                let Some((id, bounds, metrics)) =
                    local.geometry.filter(|(id, _, _)| *id == terminal)
                else {
                    return;
                };
                let col = ((event.position.x - bounds.origin.x) / metrics.width)
                    .floor()
                    .max(0.0) as u16;
                let row = ((event.position.y - bounds.origin.y) / metrics.height)
                    .floor()
                    .max(0.0) as u16;
                let steps = local.wheel.steps(
                    id,
                    event.delta,
                    metrics.height,
                    app.terminal_config.scroll_lines_per_step,
                    event.touch_phase,
                );
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
            .flex()
            // `Veil` is a block container, so this nested area is no longer a flex item of the
            // card. Give its grid a definite height to fill instead.
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(measure(move |bounds| area_local.borrow_mut().area = bounds))
            .child(grid)
            .children((model.mode == AgentPopupMode::Scroll).then(|| {
                ScrollPill::new(model.scroll_offset, model.scrollback_len)
                    .selecting(selection.is_some())
                    .alt_screen(model.alt_screen)
            }))
            .child(
                PrefixHint::new(model.mode == AgentPopupMode::Prefix && hint_visible).hints(
                    KeyHintRow::new()
                        .key("q", "hide")
                        .key("a/A", "switch")
                        .key("[", "scroll")
                        .key("]", "paste")
                        .key("?", "help")
                        .key("r", "restart"),
                ),
            );
        self.with_mouse(area, state, focus, owner, model.terminal)
            .into_any_element()
    }

    fn with_keys(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let local = Rc::clone(&self.local);
        let key_state = state.clone();
        let key_bridge = bridge.clone();
        let root = root.on_key_down(move |event: &KeyDownEvent, _window, cx| {
            if forward_terminal_key(
                &local,
                &key_state,
                &key_bridge,
                &event.keystroke,
                event.is_held,
                cx,
            ) {
                cx.stop_propagation();
            }
        });

        let enter_state = state.clone();
        let root = root.on_action(move |_: &agent::EnterPrefix, _window, cx| {
            enter_state.update(cx, |app, cx| {
                app.enter_agent_prefix();
                cx.notify();
            });
        });
        let hide_local = Rc::clone(&self.local);
        let hide_state = state.clone();
        let hide_bridge = bridge.clone();
        let root = root.on_action(move |_: &agent::Hide, _window, cx| {
            hide(&hide_local, &hide_state, &hide_bridge, cx);
        });
        let root = self.with_clipboard(root, state, bridge);
        let root = self.with_prefix(root, state, bridge);
        self.with_scroll(root, state, bridge)
    }

    fn with_clipboard(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let copy_local = Rc::clone(&self.local);
        let copy_state = state.clone();
        let copy_bridge = bridge.clone();
        let root = root.on_action(move |_: &agent::CopySelection, _window, cx| {
            copy_selection(&copy_local, &copy_state, &copy_bridge, cx);
        });
        let paste_local = Rc::clone(&self.local);
        let paste_state = state.clone();
        let paste_bridge = bridge.clone();
        root.on_action(move |_: &agent::PasteClipboard, _window, cx| {
            paste(&paste_local, &paste_state, &paste_bridge, cx);
        })
    }

    fn with_prefix(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let literal_local = Rc::clone(&self.local);
        let literal_state = state.clone();
        let literal_bridge = bridge.clone();
        let root = root.on_action(move |_: &prefix::SendLiteral, _window, cx| {
            send_literal(&literal_local, &literal_state, &literal_bridge, cx);
        });
        let paste_local = Rc::clone(&self.local);
        let paste_state = state.clone();
        let paste_bridge = bridge.clone();
        let root = root.on_action(move |_: &prefix::Paste, _window, cx| {
            paste(&paste_local, &paste_state, &paste_bridge, cx);
        });
        let scroll_state = state.clone();
        let root = root.on_action(move |_: &prefix::EnterScroll, _window, cx| {
            enter_scroll(&scroll_state, cx);
        });
        let restart_state = state.clone();
        let restart_bridge = bridge.clone();
        let root = root.on_action(move |_: &prefix::RestartCommand, _window, cx| {
            restart(&restart_state, &restart_bridge, cx);
        });
        let cancel_state = state.clone();
        root.on_action(move |_: &prefix::Cancel, _window, cx| {
            cancel_state.update(cx, |app, cx| {
                app.leave_agent_prefix();
                cx.notify();
            });
        })
    }

    fn with_scroll(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let local = Rc::clone(&self.local);
        let action_state = state.clone();
        let action_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::LineDown, _, cx| {
            step_line(&local, &action_state, &action_bridge, 1, cx)
        });
        let local = Rc::clone(&self.local);
        let action_state = state.clone();
        let action_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::LineUp, _, cx| {
            step_line(&local, &action_state, &action_bridge, -1, cx)
        });
        let root = {
            let local = Rc::clone(&self.local);
            let state = state.clone();
            let bridge = bridge.clone();
            root.on_action(move |_: &scroll::HalfPageDown, _, cx| {
                let lines = popup_rows(state.read(cx)).max(2) / 2;
                scroll_selection_lines(&local, &state, &bridge, lines, cx)
            })
        };
        let root = {
            let local = Rc::clone(&self.local);
            let state = state.clone();
            let bridge = bridge.clone();
            root.on_action(move |_: &scroll::HalfPageUp, _, cx| {
                let lines = -(popup_rows(state.read(cx)).max(2) / 2);
                scroll_selection_lines(&local, &state, &bridge, lines, cx)
            })
        };
        let root = {
            let local = Rc::clone(&self.local);
            let state = state.clone();
            let bridge = bridge.clone();
            root.on_action(move |_: &scroll::PageDown, _, cx| {
                let lines = popup_rows(state.read(cx)).max(1);
                scroll_selection_lines(&local, &state, &bridge, lines, cx)
            })
        };
        let root = {
            let local = Rc::clone(&self.local);
            let state = state.clone();
            let bridge = bridge.clone();
            root.on_action(move |_: &scroll::PageUp, _, cx| {
                let lines = -popup_rows(state.read(cx)).max(1);
                scroll_selection_lines(&local, &state, &bridge, lines, cx)
            })
        };
        let local = Rc::clone(&self.local);
        let root =
            scroll_command_action::<scroll::Top>(root, state, bridge, local, ScrollCommand::Top);
        let local = Rc::clone(&self.local);
        let root = scroll_command_action::<scroll::Bottom>(
            root,
            state,
            bridge,
            local,
            ScrollCommand::Bottom,
        );
        let select_local = Rc::clone(&self.local);
        let select_state = state.clone();
        let root = root.on_action(move |_: &scroll::StartSelection, _, cx| {
            let (base, rows, epoch, cols, alt_screen) =
                popup_grid(&select_state, cx).map_or((0, 0, 0, 0, false), |grid| {
                    (
                        viewport_base(grid),
                        grid.rows,
                        grid.viewport.history_epoch,
                        grid.cols,
                        grid.modes.alt_screen,
                    )
                });
            if rows == 0 {
                return;
            }
            let mut local = select_local.borrow_mut();
            local.caret = local.caret.clamp(base, base + u64::from(rows - 1));
            local.anchor = Some(local.caret);
            local.anchor_history_epoch = Some(epoch);
            local.anchor_cols = Some(cols);
            local.anchor_alt_screen = Some(alt_screen);
            local.history.clear();
            drop(local);
            select_state.update(cx, |_, cx| cx.notify());
        });
        let yank_local = Rc::clone(&self.local);
        let yank_state = state.clone();
        let root = root.on_action(move |_: &scroll::Yank, _, cx| {
            let text = {
                let local = yank_local.borrow();
                local
                    .anchor
                    .map(|anchor| selection_text(&local.history, anchor, local.caret))
            };
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                yank_local.borrow_mut().clear_selections();
                yank_state.update(cx, |app, cx| {
                    app.toast_short("copied", fleet_ui_kit::Icon::ClipboardCheck, Instant::now());
                    cx.notify();
                });
            }
        });
        let exit_local = Rc::clone(&self.local);
        let exit_state = state.clone();
        let exit_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::Exit, _, cx| {
            exit_scroll(&exit_local, &exit_state, &exit_bridge, cx)
        });
        let escape_local = Rc::clone(&self.local);
        let escape_state = state.clone();
        let escape_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::Escape, _, cx| {
            if escape_local.borrow_mut().clear_selections() {
                escape_state.update(cx, |_, cx| cx.notify());
            } else {
                exit_scroll(&escape_local, &escape_state, &escape_bridge, cx);
            }
        });
        // Search is reserved in the authoritative table, matching Workspace's current v1 behavior.
        let root = reserved::<scroll::Search>(root, state);
        let root = reserved::<scroll::SearchNext>(root, state);
        reserved::<scroll::SearchPrev>(root, state)
    }

    fn with_mouse(
        &self,
        area: gpui::Stateful<Div>,
        state: &Entity<AppState>,
        focus: &FocusHandle,
        owner: PendingOwner,
        terminal: Option<TerminalId>,
    ) -> gpui::Stateful<Div> {
        let down_local = Rc::clone(&self.local);
        let down_state = state.clone();
        let down_focus = focus.clone();
        let area = area.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                let Some(terminal) = terminal else {
                    cx.stop_propagation();
                    return;
                };
                if !agent_terminal_is_live_owner(down_state.read(cx), owner, Some(terminal)) {
                    cx.stop_propagation();
                    return;
                }
                window.focus(&down_focus, cx);
                let Some(cell) = mouse_cell(&down_local, &down_state, event.position, cx) else {
                    return;
                };
                let granularity = if event.click_count >= 3 {
                    SelectionGranularity::Line
                } else if event.click_count == 2 {
                    SelectionGranularity::Word
                } else {
                    SelectionGranularity::Cell
                };
                let Some(initial) = popup_grid(&down_state, cx)
                    .and_then(|grid| absolute_selection_at(grid, cell.viewport, granularity))
                else {
                    return;
                };
                let mut local = down_local.borrow_mut();
                local.clear_selections();
                local.mouse_selection = Some(MouseSelection {
                    terminal,
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
                drop(local);
                down_state.update(cx, |_, cx| cx.notify());
                cx.stop_propagation();
            },
        );
        let move_local = Rc::clone(&self.local);
        let move_state = state.clone();
        let area = area.on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let Some(selection_terminal) = move_local
                .borrow()
                .mouse_selection
                .filter(|selection| selection.dragging)
                .map(|selection| selection.terminal)
            else {
                return;
            };
            if !agent_terminal_is_live_owner(move_state.read(cx), owner, Some(selection_terminal)) {
                cx.stop_propagation();
                return;
            }
            let Some(cell) = mouse_cell(&move_local, &move_state, event.position, cx) else {
                return;
            };
            let next = popup_grid(&move_state, cx).and_then(|grid| {
                let selection = move_local.borrow().mouse_selection?;
                extend_absolute_selection(
                    grid,
                    selection.initial,
                    selection.initiating,
                    cell.viewport,
                    selection.granularity,
                )
            });
            let mut local = move_local.borrow_mut();
            let Some(selection) = local.mouse_selection.as_mut() else {
                return;
            };
            if selection.cols != cell.cols
                || selection.alt_screen != cell.alt_screen
                || selection.history_epoch != cell.history_epoch
            {
                local.clear_selections();
            } else if let Some(next) = next {
                selection.anchor = next.start;
                selection.head = next.end;
                selection.selected = selection.granularity != SelectionGranularity::Cell
                    || cell.absolute != selection.initiating;
            }
            drop(local);
            move_state.update(cx, |_, cx| cx.notify());
            cx.stop_propagation();
        });
        let up_local = Rc::clone(&self.local);
        let up_state = state.clone();
        let area = area.on_mouse_up(MouseButton::Left, move |_, _, cx| {
            let Some(selection_terminal) = up_local
                .borrow()
                .mouse_selection
                .map(|selection| selection.terminal)
            else {
                return;
            };
            if !agent_terminal_is_live_owner(up_state.read(cx), owner, Some(selection_terminal)) {
                cx.stop_propagation();
                return;
            }
            let selected = {
                let mut local = up_local.borrow_mut();
                let Some(selection) = local.mouse_selection.as_mut() else {
                    return;
                };
                selection.dragging = false;
                let selected = selection.selected;
                if !selected {
                    local.mouse_selection = None;
                    local.rows.clear();
                }
                selected
            };
            if selected {
                match current_selection_text(&up_local, &up_state, cx) {
                    CurrentSelectionText::Text(text) if !text.is_empty() => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    CurrentSelectionText::Missing => {
                        selection_scrolled_away(&up_local, &up_state, cx);
                        return;
                    }
                    CurrentSelectionText::None | CurrentSelectionText::Text(_) => {}
                }
            }
            up_state.update(cx, |_, cx| cx.notify());
            cx.stop_propagation();
        });
        let out_local = Rc::clone(&self.local);
        let out_state = state.clone();
        area.on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            let Some(selection_terminal) = out_local
                .borrow()
                .mouse_selection
                .map(|selection| selection.terminal)
            else {
                return;
            };
            if !agent_terminal_is_live_owner(out_state.read(cx), owner, Some(selection_terminal)) {
                cx.stop_propagation();
                return;
            }
            let selected = {
                let mut local = out_local.borrow_mut();
                let Some(selection) = local.mouse_selection.as_mut() else {
                    return;
                };
                selection.dragging = false;
                let selected = selection.selected;
                if !selected {
                    local.mouse_selection = None;
                    local.rows.clear();
                }
                selected
            };
            if selected {
                match current_selection_text(&out_local, &out_state, cx) {
                    CurrentSelectionText::Text(text) if !text.is_empty() => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    CurrentSelectionText::Missing => {
                        selection_scrolled_away(&out_local, &out_state, cx);
                        return;
                    }
                    CurrentSelectionText::None | CurrentSelectionText::Text(_) => {}
                }
            }
            out_state.update(cx, |_, cx| cx.notify());
            cx.stop_propagation();
        })
    }

    fn cache_selection(&self, model: &Model, state: &Entity<AppState>, cx: &App) {
        let Some(grid) = model.terminal.and_then(|id| state.read(cx).grids.get(&id)) else {
            return;
        };
        let mut local = self.local.borrow_mut();
        let Some(selection) = local.mouse_selection else {
            return;
        };
        let selection = AbsoluteCellSelection::new(selection.anchor, selection.head);
        let base = viewport_base(grid);
        for row in 0..grid.rows {
            let line = base + u64::from(row);
            if line >= selection.start.line
                && line <= selection.end.line
                && let Some(row) = cached_grid_row(grid, usize::from(row))
            {
                local.rows.insert(line, row);
            }
        }
        while local.rows.len() > CACHE_CAP {
            local.rows.pop_first();
        }
    }

    fn track_selection(&self, model: &Model, state: &Entity<AppState>, cx: &App) {
        let Some(grid) = model.terminal.and_then(|id| state.read(cx).grids.get(&id)) else {
            return;
        };
        let base = viewport_base(grid);
        let Some(bottom) = viewport_last(grid) else {
            return;
        };
        let mut local = self.local.borrow_mut();
        local.caret = local.caret.clamp(base, bottom);
        let Some(anchor) = local.anchor else { return };
        let (first, last) = (anchor.min(local.caret), anchor.max(local.caret));
        for row in 0..grid.rows {
            let line = base + u64::from(row);
            if line >= first && line <= last && local.history.len() < SELECTION_LINE_CAP {
                local
                    .history
                    .insert(line, grid.row_text(row).trim_end().to_owned());
            }
        }
    }

    fn arm_prefix_hint(&self, model: &Model, state: &Entity<AppState>, cx: &mut App) {
        if model.mode != AgentPopupMode::Prefix {
            let mut local = self.local.borrow_mut();
            local.hint_armed = false;
            local.hint_visible = false;
            return;
        }
        if self.local.borrow().hint_armed {
            return;
        }
        self.local.borrow_mut().hint_armed = true;
        let local = Rc::clone(&self.local);
        let state = state.clone();
        let delay = Duration::from_millis(cx.theme().motion.prefix_hint_delay);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            if local.borrow().hint_armed {
                local.borrow_mut().hint_visible = true;
                state.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }
}

fn popup_target(app: &AppState) -> Option<(TerminalId, bool)> {
    if app.overlay.is_some() || app.drops_terminal_keys() {
        return None;
    }
    let terminal = app.agent_popup_session()?.terminals.first()?.id;
    Some((
        terminal,
        app.grids.get(&terminal).is_some_and(|grid| grid.primed),
    ))
}

#[derive(Clone, Copy, Debug)]
struct PopupInputTarget {
    owner: PendingOwner,
    terminal: Option<TerminalId>,
    primed: bool,
}

fn popup_input_target(app: &AppState) -> Option<PopupInputTarget> {
    if app.overlay.is_some() || app.drops_terminal_keys() {
        return None;
    }
    let popup = app.agent_popup?;
    let terminal = app
        .agent_popup_session()
        .and_then(|session| session.terminals.first())
        .map(|terminal| terminal.id);
    Some(PopupInputTarget {
        owner: PendingOwner {
            agent: popup.agent,
            generation: app.link_generation,
        },
        terminal,
        primed: terminal.is_some_and(|id| app.grids.get(&id).is_some_and(|grid| grid.primed)),
    })
}

/// Whether this rendered popup instance is still the authoritative topmost pointer owner.
fn agent_surface_is_live_owner(app: &AppState, owner: PendingOwner) -> bool {
    app.overlay.is_none()
        && app
            .agent_popup
            .is_some_and(|popup| popup.agent == owner.agent)
        && app.link_generation == owner.generation
}

fn agent_terminal_is_live_owner(
    app: &AppState,
    owner: PendingOwner,
    terminal: Option<TerminalId>,
) -> bool {
    agent_surface_is_live_owner(app, owner)
        && terminal.is_some()
        && app
            .agent_popup_session()
            .and_then(|session| session.terminals.first())
            .map(|terminal| terminal.id)
            == terminal
}

fn forward_terminal_key(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    keystroke: &Keystroke,
    is_held: bool,
    cx: &mut App,
) -> bool {
    let target = {
        let app = state.read(cx);
        let Some(popup) = app.agent_popup else {
            return false;
        };
        if app.overlay.is_some() || popup.mode != AgentPopupMode::Terminal {
            return false;
        }
        if app.drops_terminal_keys() {
            local.borrow_mut().discard_pending();
            return true;
        }
        popup_input_target(app)
    };
    let Some(target) = target else {
        return false;
    };
    let Some(key) = key_event(keystroke, is_held) else {
        return false;
    };
    if local.borrow_mut().clear_selections() {
        state.update(cx, |_, cx| cx.notify());
    }
    send_or_queue(local, bridge, target, PendingInput::Key(key));
    true
}

fn popup_grid<'a>(
    state: &'a Entity<AppState>,
    cx: &'a App,
) -> Option<&'a crate::state::MirrorGrid> {
    state.read(cx).agent_popup_grid()
}

fn popup_rows(app: &AppState) -> i32 {
    i32::from(app.agent_popup_grid().map_or(0, |grid| grid.rows))
}

fn send_or_queue(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    target: PopupInputTarget,
    input: PendingInput,
) {
    if target.primed
        && let Some(terminal) = target.terminal
    {
        bridge.send(input.request(terminal));
    } else {
        local.borrow_mut().queue_input(target.owner, input);
    }
}

fn send_literal(local: &Rc<RefCell<Local>>, state: &Entity<AppState>, bridge: &Bridge, cx: &App) {
    if let Some(target) = popup_input_target(state.read(cx)) {
        send_or_queue(
            local,
            bridge,
            target,
            PendingInput::Key(KeyEvent {
                key: Key::Char('s'),
                mods: Modifiers::CTRL,
                text: None,
                action: KeyAction::Press,
            }),
        );
    }
}

fn enter_scroll(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        let alt_screen = app
            .agent_popup_grid()
            .is_some_and(|grid| grid.modes.alt_screen);
        if let Some(popup) = &mut app.agent_popup {
            if alt_screen {
                popup.mode = AgentPopupMode::Terminal;
                app.toast_short(
                    "no scrollback in alt-screen",
                    fleet_ui_kit::Icon::ChevronsUp,
                    Instant::now(),
                );
            } else {
                popup.mode = AgentPopupMode::Scroll;
            }
        }
        cx.notify();
    });
}

fn restart(state: &Entity<AppState>, bridge: &Bridge, cx: &App) {
    if let Some((terminal, _)) = popup_target(state.read(cx)) {
        bridge.send(RequestBody::RestartTerminal { terminal });
    }
}

fn paste(local: &Rc<RefCell<Local>>, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some(target) = popup_input_target(state.read(cx)) else {
        return;
    };
    let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
        return;
    };
    local.borrow_mut().clear_selections();
    send_or_queue(local, bridge, target, PendingInput::Paste(text));
}

fn hide(local: &Rc<RefCell<Local>>, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let preserve = state
        .read(cx)
        .active_session()
        .and_then(|session| session.active_terminal);
    detach_local(local, bridge, preserve);
    state.update(cx, |app, cx| {
        app.hide_agent_popup();
        cx.notify();
    });
}

enum CurrentSelectionText {
    None,
    Text(String),
    Missing,
}

fn copy_selection(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    match current_selection_text(local, state, cx) {
        CurrentSelectionText::Text(text) => {
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                state.update(cx, |app, cx| {
                    app.toast_short("copied", fleet_ui_kit::Icon::ClipboardCheck, Instant::now());
                    cx.notify();
                });
            }
        }
        CurrentSelectionText::Missing => selection_scrolled_away(local, state, cx),
        CurrentSelectionText::None => {
            if let Some(target) = popup_input_target(state.read(cx)) {
                send_or_queue(
                    local,
                    bridge,
                    target,
                    PendingInput::Key(KeyEvent {
                        key: Key::Char('c'),
                        mods: Modifiers::SUPER,
                        text: None,
                        action: KeyAction::Press,
                    }),
                );
            }
        }
    }
}

fn current_selection_text(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    cx: &App,
) -> CurrentSelectionText {
    let app = state.read(cx);
    let Some(grid) = app.agent_popup_grid() else {
        return CurrentSelectionText::None;
    };
    let local = local.borrow();
    if let Some(selection) = local.mouse_selection.filter(|selection| selection.selected) {
        return absolute_selection_text(
            grid,
            &local.rows,
            AbsoluteCellSelection::new(selection.anchor, selection.head),
        )
        .map_or(CurrentSelectionText::Missing, CurrentSelectionText::Text);
    }
    local.anchor.map_or(CurrentSelectionText::None, |anchor| {
        CurrentSelectionText::Text(selection_text(&local.history, anchor, local.caret))
    })
}

fn selection_scrolled_away(local: &Rc<RefCell<Local>>, state: &Entity<AppState>, cx: &mut App) {
    local.borrow_mut().clear_selections();
    state.update(cx, |app, cx| {
        app.toast_short(
            "selection scrolled away",
            fleet_ui_kit::Icon::Info,
            Instant::now(),
        );
        cx.notify();
    });
}

fn detach_local(local: &Rc<RefCell<Local>>, bridge: &Bridge, preserve: Option<TerminalId>) {
    if let Some(terminal) = local.borrow_mut().attached.take()
        && Some(terminal) != preserve
    {
        bridge.send(RequestBody::DetachTerminal { terminal });
    }
    let mut local = local.borrow_mut();
    local.clear_selections();
    local.discard_pending();
}

struct MouseCell {
    viewport: CellPoint,
    absolute: AbsoluteCellPoint,
    cols: u16,
    alt_screen: bool,
    history_epoch: u64,
}

fn mouse_cell(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    position: gpui::Point<Pixels>,
    cx: &App,
) -> Option<MouseCell> {
    let app = state.read(cx);
    let terminal = app.agent_popup_session()?.terminals.first()?.id;
    let grid = app.grids.get(&terminal)?;
    let (_, bounds, metrics) = local
        .borrow()
        .geometry
        .filter(|(id, _, _)| *id == terminal)?;
    let viewport = cell_at_position(
        bounds,
        position,
        gpui::size(metrics.width, metrics.height),
        grid.cols,
        grid.rows,
    )?;
    Some(MouseCell {
        viewport,
        absolute: AbsoluteCellPoint::new(viewport_base(grid) + viewport.row as u64, viewport.col),
        cols: grid.cols,
        alt_screen: grid.modes.alt_screen,
        history_epoch: grid.viewport.history_epoch,
    })
}

fn scroll_lines(state: &Entity<AppState>, bridge: &Bridge, lines: i32, cx: &mut App) {
    if let Some((terminal, _)) = popup_target(state.read(cx)) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Lines(lines),
        });
    }
    state.update(cx, |_, cx| cx.notify());
}

fn step_line(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    delta: i32,
    cx: &mut App,
) {
    let (base, rows) =
        popup_grid(state, cx).map_or((0, 0), |grid| (viewport_base(grid), grid.rows));
    if rows > 0 {
        let bottom = base + u64::from(rows - 1);
        let mut local = local.borrow_mut();
        let current = local.caret.clamp(base, bottom);
        let next = add_signed(current, delta).clamp(base, bottom);
        local.caret = next;
        if next != current {
            drop(local);
            state.update(cx, |_, cx| cx.notify());
            return;
        }
    }
    scroll_selection_lines(local, state, bridge, delta, cx);
}

fn scroll_selection_lines(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    lines: i32,
    cx: &mut App,
) {
    let caret = local.borrow().caret;
    local.borrow_mut().caret = add_signed(caret, lines);
    scroll_lines(state, bridge, lines, cx);
}

fn add_signed(value: u64, delta: i32) -> u64 {
    if delta >= 0 {
        value.saturating_add(delta as u64)
    } else {
        value.saturating_sub(u64::from(delta.unsigned_abs()))
    }
}

fn header_status_tone(model: &Model) -> Tone {
    if !model.reachable || model.exit_code.is_some() || model.terminal.is_none() {
        return Tone::Danger;
    }
    match model.activity {
        AgentActivity::Working => Tone::Warning,
        AgentActivity::Idle | AgentActivity::Unknown => Tone::Success,
    }
}

fn scroll_command_action<A: gpui::Action>(
    root: Div,
    state: &Entity<AppState>,
    bridge: &Bridge,
    local: Rc<RefCell<Local>>,
    command: ScrollCommand,
) -> Div {
    let state = state.clone();
    let bridge = bridge.clone();
    root.on_action(move |_: &A, _, cx| {
        match command {
            ScrollCommand::Top => local.borrow_mut().caret = 0,
            ScrollCommand::Bottom => local.borrow_mut().caret = u64::MAX,
            _ => {}
        }
        if let Some((terminal, _)) = popup_target(state.read(cx)) {
            bridge.send(RequestBody::ScrollTerminal {
                terminal,
                scroll: command,
            });
        }
    })
}

fn exit_scroll(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    local.borrow_mut().clear_selections();
    if let Some((terminal, _)) = popup_target(state.read(cx)) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Bottom,
        });
    }
    state.update(cx, |app, cx| {
        if let Some(popup) = &mut app.agent_popup {
            popup.mode = AgentPopupMode::Terminal;
        }
        cx.notify();
    });
}

fn reserved<A: gpui::Action>(root: Div, state: &Entity<AppState>) -> Div {
    let state = state.clone();
    root.on_action(move |_: &A, _, cx| {
        state.update(cx, |app, cx| {
            app.toast_short(
                "scrollback search is not available yet",
                fleet_ui_kit::Icon::Search,
                Instant::now(),
            );
            cx.notify();
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_input_survives_terminal_discovery_for_the_same_owner() {
        let owner = PendingOwner {
            agent: Agent::Claude,
            generation: 7,
        };
        let mut local = Local::default();
        local.queue_input(owner, PendingInput::Paste("first".to_owned()));
        local.queue_input(owner, PendingInput::Paste("second".to_owned()));

        assert_eq!(
            local.take_pending(owner),
            vec![
                PendingInput::Paste("first".to_owned()),
                PendingInput::Paste("second".to_owned())
            ]
        );
        assert!(local.pending.is_empty());
        assert_eq!(local.pending_owner, None);
    }

    #[test]
    fn creation_input_is_discarded_on_agent_switch_or_generation_change() {
        let claude = PendingOwner {
            agent: Agent::Claude,
            generation: 7,
        };
        let opencode = PendingOwner {
            agent: Agent::Opencode,
            generation: 7,
        };
        let reconnected = PendingOwner {
            agent: Agent::Claude,
            generation: 8,
        };
        let mut local = Local::default();
        local.queue_input(claude, PendingInput::Paste("stale".to_owned()));
        local.queue_input(opencode, PendingInput::Paste("switched".to_owned()));
        assert!(local.take_pending(claude).is_empty());
        assert_eq!(
            local.take_pending(opencode),
            vec![PendingInput::Paste("switched".to_owned())]
        );

        local.queue_input(claude, PendingInput::Paste("old link".to_owned()));
        local.queue_input(reconnected, PendingInput::Paste("new link".to_owned()));
        assert!(local.take_pending(claude).is_empty());
        assert_eq!(
            local.take_pending(reconnected),
            vec![PendingInput::Paste("new link".to_owned())]
        );
    }
}
