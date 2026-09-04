//! `ExitStrip` — `⚠ process exited (<code>)` and the prefixed recovery keys.
//!
//! §3.6: tmux's `remain-on-exit` made a crashed dev server recoverable, and Fleet must not
//! silently swallow one. Every key on this strip carries its `^s` prefix ([D-8]).

use gpui::{App, Window, div, prelude::*, px};

use crate::{
    components::KeyHintRow,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The 22 px strip under a terminal whose command exited.
#[derive(IntoElement)]
pub struct ExitStrip {
    code: i32,
    hints: Option<KeyHintRow>,
}

impl ExitStrip {
    /// A strip for an exit code.
    pub fn new(code: i32) -> Self {
        Self { code, hints: None }
    }

    /// The recovery keys. Defaults to `^s r restart · ^s x close · ^s c new`.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.hints = Some(hints);
        self
    }
}

impl RenderOnce for ExitStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let hints = self.hints.unwrap_or_else(|| {
            KeyHintRow::new()
                .key("^s r", "restart")
                .key("^s x", "close")
                .key("^s c", "new")
        });
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.strip_h)
            .w_full()
            .px(theme.space.md)
            .bg(Tone::Warning.fill(theme))
            .border_t(px(1.0))
            .border_color(theme.colors.border)
            .child(
                Icon::TriangleAlert
                    .el()
                    .size(IconSize::Small)
                    .color(theme.colors.warning),
            )
            .child(
                Text::ui(format!("process exited ({})", self.code)).tone(Tone::Warning),
            )
            .child(hints)
    }
}
