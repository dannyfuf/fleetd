//! `NumberField` — an integer with a unit suffix and a clamp.
//!
//! Settings uses it for grace, TTLs, intervals and the pool size. The clamp is part of the
//! contract because §3.8.6 states minimums (`grace ≥ 0`, status refresh `≥ 500 ms`) and an
//! out-of-range value must be refused **at the field**, not at save time — a dialog that
//! accepts `-1` and fails on save teaches the user nothing about the rule.
//!
//! When the value is out of range and the caller supplied no message, the field states the rule
//! itself ([`NumberField::range_message`]), because §3.8's law is that a failure names the
//! exact rule it failed.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{components::FocusRing, text::Text, theme::ActiveTheme, tone::Tone};

/// The minimum width of the value box: wide enough for `100000 ms` without reflowing as the
/// user types.
///
/// TODO(INTEGRATION `metrics.number_field_w`): a `Metrics` entry would be the right home; the
/// token set has no pixel constant for an input box yet.
const VALUE_BOX_W: Pixels = px(96.0);

/// An integer input.
#[derive(IntoElement)]
pub struct NumberField {
    label: Option<SharedString>,
    value: i64,
    unit: Option<SharedString>,
    min: Option<i64>,
    max: Option<i64>,
    focused: bool,
    invalid: Option<SharedString>,
    label_width: Option<Pixels>,
}

impl NumberField {
    /// A field showing `value`.
    pub fn new(value: i64) -> Self {
        Self {
            label: None,
            value,
            unit: None,
            min: None,
            max: None,
            focused: false,
            invalid: None,
            label_width: None,
        }
    }

    /// A labelled field.
    pub fn labeled(label: impl Into<SharedString>, value: i64) -> Self {
        Self::new(value).label(label)
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Fix the label column so a stack of settings rows aligns on one gutter.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// The unit suffix, e.g. `ms`.
    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// The inclusive range.
    pub fn range(mut self, min: i64, max: i64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    /// The inclusive minimum.
    pub fn min(mut self, min: i64) -> Self {
        self.min = Some(min);
        self
    }

    /// Whether the row carries the cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// An explicit validation message. Overrides the derived range message.
    pub fn invalid(mut self, message: impl Into<SharedString>) -> Self {
        self.invalid = Some(message.into());
        self
    }

    /// Clamp a candidate value into the field's range.
    pub fn clamp(&self, value: i64) -> i64 {
        let value = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(value, |max| value.min(max))
    }

    /// Whether the current value is inside the range.
    pub fn is_valid(&self) -> bool {
        self.clamp(self.value) == self.value
    }

    /// The rule the value broke, stated exactly.
    pub fn range_message(&self) -> Option<SharedString> {
        if self.is_valid() {
            return None;
        }
        let unit = self
            .unit
            .as_ref()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        Some(SharedString::from(match (self.min, self.max) {
            (Some(min), Some(max)) => format!("must be between {min} and {max}{unit}"),
            (Some(min), None) => format!("must be at least {min}{unit}"),
            (None, Some(max)) => format!("must be at most {max}{unit}"),
            (None, None) => "out of range".to_string(),
        }))
    }

    /// The message the field will show, explicit or derived.
    pub fn message(&self) -> Option<SharedString> {
        self.invalid.clone().or_else(|| self.range_message())
    }
}

impl RenderOnce for NumberField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let message = self.message();
        let valid = message.is_none();
        let focused = self.focused;
        let border = if !valid {
            theme.colors.danger
        } else if focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        };

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .h_full()
            .w_full()
            .px(theme.space.md)
            .children(self.label.map(|label| {
                let text = Text::ui(label).muted();
                match self.label_width {
                    Some(width) => text.w(width),
                    None => text,
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(theme.space.sm)
                    .min_w(VALUE_BOX_W)
                    .px(theme.space.sm)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(border)
                    .child(Text::data(self.value.to_string()).tone(if valid {
                        Tone::Default
                    } else {
                        Tone::Danger
                    }))
                    // The unit is a label, not a value: it never competes with the number.
                    .children(self.unit.map(|unit| Text::data(unit).faint())),
            )
            .children(message.map(|message| Text::hint(message).tone(Tone::Danger).ellipsize()));

        div()
            .w_full()
            .h(theme.metrics.row_h)
            .when(focused, |el| el.bg(theme.colors.row_selected))
            .child(FocusRing::cursor_row(focused).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_respects_both_ends() {
        let field = NumberField::new(0).range(500, 5_000);
        assert_eq!(field.clamp(10), 500);
        assert_eq!(field.clamp(9_999), 5_000);
        assert_eq!(field.clamp(1_200), 1_200);
    }

    #[test]
    fn an_out_of_range_value_states_the_rule() {
        let field = NumberField::new(100).range(500, 5_000).unit("ms");
        assert!(!field.is_valid());
        assert_eq!(
            field.message().as_deref(),
            Some("must be between 500 and 5000 ms")
        );
    }

    #[test]
    fn a_minimum_only_field_states_the_minimum() {
        let field = NumberField::new(-1).min(0);
        assert_eq!(field.message().as_deref(), Some("must be at least 0"));
    }

    #[test]
    fn an_explicit_message_wins_over_the_derived_one() {
        let field = NumberField::new(-1)
            .min(0)
            .invalid("grace cannot be negative");
        assert_eq!(field.message().as_deref(), Some("grace cannot be negative"));
    }

    #[test]
    fn a_valid_value_has_no_message() {
        assert!(NumberField::new(3).range(1, 8).message().is_none());
    }
}
