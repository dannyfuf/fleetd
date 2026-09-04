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
    code: Option<i32>,
    hints: Option<KeyHintRow>,
}

impl ExitStrip {
    /// A strip for an exit code. Takes `1` or `None`: a process killed by a signal (`^s x`
    /// sends `SIGKILL`) has **no** exit code, and the strip says `killed` rather than
    /// inventing `128 + signo`.
    pub fn new(code: impl Into<Option<i32>>) -> Self {
        Self {
            code: code.into(),
            hints: None,
        }
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
                Text::ui(match self.code {
                    Some(code) => format!("process exited ({code})"),
                    None => "process exited (killed)".to_string(),
                })
                .tone(Tone::Warning),
            )
            .child(hints)
    }
}
