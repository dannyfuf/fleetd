//! `Cycler` — `◂ value ▸`, changed with `←` / `→`.
//!
//! Used for the host selector and every Settings choice. It is a cycler and not a dropdown
//! because the option sets are two to five items long and a dropdown would cost a second key.
//!
//! Zero-suppression applies to the control itself: a set with fewer than two members has no
//! choice in it, so [`Cycler::is_visible`] is false and the host cycler disappears when no
//! hosts are configured.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A left/right value cycler.
#[derive(IntoElement)]
pub struct Cycler {
    label: Option<SharedString>,
    value: SharedString,
    has_prev: bool,
    has_next: bool,
    focused: bool,
    disabled: bool,
    off_grid: bool,
    label_width: Option<gpui::Pixels>,
}

impl Cycler {
    /// A cycler showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            label: None,
            value: value.into(),
            has_prev: true,
            has_next: true,
            focused: false,
            disabled: false,
            off_grid: false,
            label_width: None,
        }
    }

    /// A labelled cycler.
    pub fn labeled(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self::new(value).label(label)
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Fix the label column so a stack of settings rows aligns on one gutter.
    pub fn label_width(mut self, width: gpui::Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// Whether `←` does anything. Off dims the arrow.
    pub fn has_prev(mut self, has_prev: bool) -> Self {
        self.has_prev = has_prev;
        self
    }

    /// Whether `→` does anything.
    pub fn has_next(mut self, has_next: bool) -> Self {
        self.has_next = has_next;
        self
    }

    /// Whether the row carries the cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the choice can be changed at all (a locked setting, a single-host config).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mark a persisted value that is outside the configured steps.
    ///
    /// Both arrows remain available so the next move returns to a known step.
    pub fn off_grid(mut self, off_grid: bool) -> Self {
        self.off_grid = off_grid;
        self
    }

    /// Whether the control has anything to cycle. A set with one member renders nothing.
    pub fn is_visible(&self) -> bool {
        self.off_grid || self.has_prev || self.has_next
    }
}

impl RenderOnce for Cycler {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        if !self.is_visible() && self.label.is_none() {
            return div().into_any_element();
        }

        let disabled = self.disabled;
        let arrow = |enabled: bool, icon: Icon| {
            icon.el()
                .size(IconSize::Small)
                .color(if enabled && !disabled {
                    theme.colors.text_secondary
                } else {
                    theme.colors.text_muted
                })
                // A dead arrow stays in place at low contrast: the control must not resize
                // when the value reaches an end of the set.
                .opacity(if enabled && !disabled {
                    1.0
                } else {
                    theme.metrics.dimmed_opacity
                })
        };

        let value_tone = if disabled { Tone::Muted } else { Tone::Default };

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .h_full()
            .w_full()
            .px(theme.space.md)
            .children(self.label.map(|label| {
                let text = Text::ui(label).tone(if disabled {
                    Tone::Muted
                } else {
                    Tone::Secondary
                });
                match self.label_width {
                    Some(width) => text.w(width),
                    None => text,
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(arrow(self.off_grid || self.has_prev, Icon::ChevronLeft))
                    .child(Text::ui(self.value).tone(value_tone))
                    .child(arrow(self.off_grid || self.has_next, Icon::ChevronRight)),
            );

        super::control::cursor_row(theme, self.focused && !disabled, disabled, body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_member_set_is_zero_suppressed() {
        let one = Cycler::new("local").has_prev(false).has_next(false);
        assert!(!one.is_visible());
        assert!(Cycler::new("local").is_visible());
    }
}
