//! `NumberField` — an integer with a unit suffix and a clamp.
//!
//! Settings uses it for grace, TTLs, intervals and the pool size. The clamp is part of the
//! contract because §3.8.6 states minimums (`grace ≥ 0`, status refresh `≥ 500 ms`) and an
//! out-of-range value must be refused at the field, not at save time.

use gpui::{App, SharedString, Window, div, prelude::*, px};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

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

    /// An explicit validation message.
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
}

impl RenderOnce for NumberField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let valid = self.is_valid() && self.invalid.is_none();
        let border = if !valid {
            theme.colors.danger
        } else if self.focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        };
        let text = match &self.unit {
            Some(unit) => SharedString::from(format!("{} {}", self.value, unit)),
            None => SharedString::from(self.value.to_string()),
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.row_h)
            .children(self.label.map(|label| Text::ui(label).muted()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w(px(96.0))
                    .px(theme.space.sm)
                    .py(px(2.0))
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(border)
                    .child(Text::data(text)),
            )
            .children(
                self.invalid
                    .map(|message| Text::hint(message).tone(Tone::Danger)),
            )
    }
}
