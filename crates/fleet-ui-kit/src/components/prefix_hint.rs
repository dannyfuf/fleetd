//! `PrefixHint` — the `^S` pill and its six keys, 400 ms after the prefix.
//!
//! §3.6: the expert types the second key in under 200 ms and never sees this; the returning
//! user gets it exactly when they hesitate. That is 0 px and 0 frames of permanent cost, which
//! is why the delay is part of the contract and not a preference.
//!
//! **The delay is the caller's timer.** A component cannot own a one-shot timer without owning
//! state, and Prefix mode already lives in the app's mode machine; the caller spawns
//! `Timer::after(theme.motion.prefix_hint_delay)` and flips `visible`.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::KeyHintRow,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The six keys §3.6 puts on the strip, in the order the spec lists them.
///
/// They are the six that move you somewhere: everything else on the prefix is either rare
/// (`,` rename, `!` errors) or destructive (`x` close), and a cheat strip that lists a
/// destructive key next to a navigation key trains the wrong muscle.
const DEFAULT_HINTS: [(&str, &str); 6] = [
    ("s", "hub"),
    ("1-9", "tab"),
    ("c", "new"),
    ("x", "close"),
    ("[", "scroll"),
    ("w", "last session"),
];

/// The prefix cheat strip.
#[derive(IntoElement)]
pub struct PrefixHint {
    visible: bool,
    prefix: SharedString,
    hints: Option<KeyHintRow>,
}

impl PrefixHint {
    /// A hint strip. `visible` is false until the caller's 400 ms timer fires.
    pub fn new(visible: bool) -> Self {
        Self {
            visible,
            prefix: SharedString::new_static("^S"),
            hints: None,
        }
    }

    /// Override the prefix pill's label.
    pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// The six most-used prefix keys. Defaults to §3.6's
    /// `s hub · 1-9 tab · c new · x close · [ scroll · w last session`.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.hints = Some(hints);
        self
    }

    /// Whether the strip draws anything.
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

impl RenderOnce for PrefixHint {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let hints = self.hints.unwrap_or_else(|| {
            DEFAULT_HINTS
                .iter()
                .fold(KeyHintRow::new(), |row, (key, label)| row.key(*key, *label))
        });
        div()
            // Bottom-left inside the terminal area, 12 px inset: the prompt lives at the
            // bottom-left too, but the strip only exists while the prefix is pending and the
            // user is by definition not typing into the shell.
            .absolute()
            .left(theme.space.md)
            .bottom(theme.space.md)
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.chip_h)
            .px(theme.space.sm)
            .rounded(theme.radii.md)
            // The floating layer plus a shadow: the strip is drawn over live terminal output
            // and has to be readable on top of any color the shell just painted.
            .bg(theme.colors.elevated)
            .shadow(theme.sheet_shadow())
            .border_1()
            .border_color(theme.colors.border_strong)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.xxs)
                    .px(theme.space.xs)
                    .rounded(theme.radii.sm)
                    // Amber, like the mode word: Prefix is the one mode that expires on its
                    // own, and the pill has to read as "this is temporary".
                    .bg(Tone::Warning.fill(theme))
                    .child(
                        Icon::Command
                            .el()
                            .size(IconSize::Small)
                            .color(theme.colors.warning),
                    )
                    .child(Text::hint(self.prefix).tone(Tone::Warning)),
            )
            .child(hints)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_costs_nothing_until_the_timer_fires() {
        assert!(!PrefixHint::new(false).is_visible());
        assert!(PrefixHint::new(true).is_visible());
    }

    #[test]
    fn the_default_strip_is_the_six_keys_the_spec_lists() {
        assert_eq!(DEFAULT_HINTS.len(), 6);
        assert_eq!(DEFAULT_HINTS[0], ("s", "hub"));
        assert_eq!(DEFAULT_HINTS[5], ("w", "last session"));
    }
}
