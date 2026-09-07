//! Floating agent terminal with independent input ownership and popup resize precedence.
//! Hiding releases only popup-exclusive attachments; daemon-owned sessions stay alive.

use crate::terminal::surface::*;

mod actions;
mod chrome;
mod lifecycle;
mod model;
mod terminal;
#[cfg(test)]
mod tests;

use actions::*;
use lifecycle::*;
use model::*;
use terminal::*;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

use fleet_core::{
    config::Agent,
    ids::{SessionId, TerminalId},
    sessions::{AgentActivity, TerminalStatus, agent_session_id},
};
use fleet_proto::{
    request::RequestBody,
    terminal::{Key, KeyAction, KeyEvent, Modifiers, ScrollCommand},
};
use fleet_ui_kit::{
    ActiveTheme, ExitStrip, KeyHintRow, Overlay as FloatingOverlay, OverlayLayer, PrefixHint,
    ScrollPill, StatusDot, Text, Tone, Veil,
};
use gpui::{
    AnyElement, App, ClipboardItem, Div, Entity, FocusHandle, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, Size, Window, div, prelude::*, px,
};

use crate::{
    actions::{agent, prefix, scroll},
    bridge::Bridge,
    state::{AgentPopupMode, AppState, Screen},
    terminal::{
        MouseCell, absolute_selection_at, cell_size, grid_size, measure, surface,
        try_selection_text, viewport_base,
    },
};

/// How much of the window the floating card covers, leaving the Workspace visible around it.
const CARD_WIDTH_FRACTION: f32 = 0.9;
/// See [`CARD_WIDTH_FRACTION`]. The remaining height is split evenly above and below the card.
const CARD_HEIGHT_FRACTION: f32 = 0.85;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingOwner {
    agent: Agent,
    generation: u64,
}

impl TerminalSurface<PopupState> {
    fn discard_pending(&mut self) {
        self.state.pending_owner = None;
        self.pending.clear();
    }

    #[must_use = "rejected input must be surfaced to the user"]
    fn queue_input(&mut self, owner: PendingOwner, input: PendingInput) -> bool {
        if self.state.pending_owner != Some(owner) {
            self.discard_pending();
            self.state.pending_owner = Some(owner);
        }
        self.queue_pending(input)
    }

    fn take_pending(&mut self, owner: PendingOwner) -> Vec<PendingInput> {
        if self.state.pending_owner != Some(owner) {
            return Vec::new();
        }
        self.state.pending_owner = None;
        std::mem::take(&mut self.pending)
    }
}

/// The window-wide floating agent surface.
pub(crate) struct AgentPopup {
    model: Option<Model>,
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
    pub(crate) fn new(_cx: &mut App) -> Self {
        Self {
            local: Rc::new(RefCell::new(Local::default())),
            model: None,
        }
    }

    /// Releases this client's terminal attachment without touching the daemon session.
    pub(crate) fn detach(&self, bridge: &Bridge, preserve: Option<TerminalId>) {
        detach_local(&self.local, bridge, preserve);
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

    /// Composes the last synchronized surface.
    ///
    /// Preparation belongs to [`AgentPopup::synchronize`]: this issues no request and reconciles
    /// no resource.
    pub(crate) fn render_prepared(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(model) = self.model.as_ref() else {
            return div().into_any_element();
        };
        let viewport = window.viewport_size();
        let width = px(f32::from(viewport.width) * CARD_WIDTH_FRACTION);
        let height = px(f32::from(viewport.height) * CARD_HEIGHT_FRACTION);
        let top = px(f32::from(viewport.height) * (1.0 - CARD_HEIGHT_FRACTION) / 2.0);
        let header = self.header(model, cx);
        let terminal = Veil::new(!model.reachable)
            .content(self.terminal_area(model, state, bridge, focus, window, cx));
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
            .content(self.with_keys(card, state, bridge))
            .into_any_element()
    }
}

#[derive(Default)]
struct PopupState {
    pending_owner: Option<PendingOwner>,
    size: Option<(u16, u16)>,
}
type Local = TerminalSurface<PopupState>;
