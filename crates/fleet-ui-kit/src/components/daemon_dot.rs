//! `DaemonDot` — 8 px liveness dot that grows a word when the daemon is not healthy.
//!
//! §2.2 and §3.12: the dot is always present because it reports the liveness of the process
//! that owns every job and every PTY — closing this window is safe *because* that process is
//! somewhere else, and the dot is the only place the app says so.
//!
//! Healthy is a dot and nothing else: good news must not cost pixels. Degraded and lost grow a
//! labelled pill, because bad news must be readable from across the room. A degraded dot with
//! no caller-supplied label still shows [`DaemonState::word`], so the pill can never be a
//! wordless colour change — a red dot alone is exactly the signal §1.3 says must not exist.
//!
//! ## States
//!
//! healthy (green dot) · degraded (amber pill) · lost (red pill).

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{components::StatusDot, text::Text, theme::ActiveTheme, tone::Tone};

/// The three daemon situations of §3.12.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum DaemonState {
    /// Connected and responding. A green dot, no word.
    #[default]
    Healthy,
    /// Connected but slow, or reconnecting. Amber pill.
    Degraded,
    /// The daemon died or the socket is gone. Red pill.
    Lost,
}

impl DaemonState {
    /// Every state, in escalation order. Used by the gallery.
    pub const ALL: &'static [DaemonState] = &[
        DaemonState::Healthy,
        DaemonState::Degraded,
        DaemonState::Lost,
    ];

    /// The tone for this state.
    pub const fn tone(self) -> Tone {
        match self {
            DaemonState::Healthy => Tone::Success,
            DaemonState::Degraded => Tone::Warning,
            DaemonState::Lost => Tone::Danger,
        }
    }

    /// The fallback word for a state that must be readable, used when the caller supplies
    /// none. `Healthy` has none: it never grows a word.
    pub const fn word(self) -> Option<&'static str> {
        match self {
            DaemonState::Healthy => None,
            DaemonState::Degraded => Some("degraded"),
            DaemonState::Lost => Some("lost"),
        }
    }

    /// Whether this state renders as a labelled pill rather than a bare dot.
    pub const fn is_labelled(self) -> bool {
        self.word().is_some()
    }
}

/// The daemon liveness indicator.
#[derive(IntoElement)]
pub struct DaemonDot {
    state: DaemonState,
    label: Option<SharedString>,
}

impl DaemonDot {
    /// A dot for a state.
    pub fn new(state: DaemonState) -> Self {
        Self { state, label: None }
    }

    /// The word shown next to a degraded or lost dot, e.g. `fleetd stopped`. Ignored while
    /// the daemon is healthy.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The word this dot will render, `None` when it is a bare dot.
    pub fn resolved_label(&self) -> Option<SharedString> {
        if !self.state.is_labelled() {
            return None;
        }
        self.label
            .clone()
            .or_else(|| self.state.word().map(SharedString::new_static))
    }
}

impl RenderOnce for DaemonDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = self.state.tone();
        let label = self.resolved_label();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .h(theme.metrics.chip_h)
            .when(label.is_some(), |el| {
                el.px(theme.space.sm)
                    .rounded(theme.radii.full)
                    .bg(tone.fill(theme))
            })
            .child(StatusDot::new(tone))
            .children(label.map(|label| Text::ui(label).tone(tone)))
    }
}
