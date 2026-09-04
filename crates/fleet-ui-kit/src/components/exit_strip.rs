//! `ExitStrip` — `⚠ process exited (<code>)` and the prefixed recovery keys.
//!
//! §3.6: tmux's `remain-on-exit` made a crashed dev server recoverable, and Fleet must not
//! silently swallow one. Every key on this strip carries its `^s` prefix ([D-8]): in Terminal
//! mode a bare `r` goes to the PTY, so a hint that reads `r restart` is a lie.

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

    /// The sentence the strip states.
    ///
    /// A clean exit is still an exit — the terminal is dead either way and the recovery keys
    /// are the same — but the wording separates "your build failed" from "your shell ended",
    /// which is the first thing the user needs to know.
    fn message(&self) -> String {
        match self.code {
            Some(0) => "process exited (0)".to_string(),
            Some(code) => format!("process exited ({code})"),
            None => "process exited (killed)".to_string(),
        }
    }

    /// A clean exit is stated in the neutral secondary tone; anything else is amber, because
    /// amber is Fleet's "needs attention" and a non-zero exit is exactly that.
    fn tone(&self) -> Tone {
        match self.code {
            Some(0) => Tone::Secondary,
            _ => Tone::Warning,
        }
    }
}

impl RenderOnce for ExitStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = self.tone();
        let message = self.message();
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
            .flex_none()
            .px(theme.space.md)
            .bg(tone.fill(theme))
            .border_t(px(1.0))
            .border_color(theme.colors.border)
            .child(
                div().flex_none().child(
                    Icon::TriangleAlert
                        .el()
                        .size(IconSize::Small)
                        .color(tone.color(theme)),
                ),
            )
            .child(div().flex_none().child(Text::ui(message).tone(tone)))
            // The keys are the point of the strip; if anything has to be clipped on a narrow
            // window it is the sentence, not the way out of it.
            .child(div().flex_1().min_w_0())
            .child(div().flex_none().child(hints))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_killed_process_never_gets_an_invented_code() {
        assert_eq!(ExitStrip::new(None).message(), "process exited (killed)");
        assert_eq!(ExitStrip::new(137).message(), "process exited (137)");
    }

    #[test]
    fn a_clean_exit_is_not_amber() {
        assert_eq!(ExitStrip::new(0).tone(), Tone::Secondary);
        assert_eq!(ExitStrip::new(1).tone(), Tone::Warning);
        assert_eq!(ExitStrip::new(None).tone(), Tone::Warning);
    }
}
