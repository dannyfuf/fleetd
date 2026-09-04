//! `DaemonDot` — 8 px liveness dot that expands into a labelled pill when degraded.
//!
//! §2.2 and §3.12: the dot is always present because it reports the liveness of the process
//! that owns every job and every PTY. Healthy is a dot and nothing else; degraded and lost
//! grow a word, because good news must not cost pixels but bad news must be readable.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::StatusDot, text::Text, theme::ActiveTheme, tone::Tone,
};

/// The three daemon situations of §3.12.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonState {
    /// Connected and responding. A green dot, no word.
    Healthy,
    /// Connected but slow or reconnecting. Amber pill.
    Degraded,
    /// The daemon died or the socket is gone. Red pill.
    Lost,
}

impl DaemonState {
    /// The tone for this state.
    pub fn tone(self) -> Tone {
        match self {
            DaemonState::Healthy => Tone::Success,
            DaemonState::Degraded => Tone::Warning,
            DaemonState::Lost => Tone::Danger,
        }
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

    /// The word shown next to a degraded or lost dot, e.g. `fleetd stopped`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl RenderOnce for DaemonDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = self.state.tone();
        let show_label = self.state != DaemonState::Healthy && self.label.is_some();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .h(theme.metrics.chip_h)
            .when(show_label, |el| {
                el.px(theme.space.sm)
                    .rounded(theme.radii.full)
                    .bg(tone.fill(theme))
            })
            .child(StatusDot::new(tone))
            .children(
                show_label
                    .then(|| self.label.map(|label| Text::ui(label).tone(tone)))
                    .flatten(),
            )
    }
}
