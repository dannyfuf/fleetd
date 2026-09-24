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
//!
//! A row that is being typed into hands the field its live editor with [`NumberField::editor`];
//! the field then draws that editor where the number would be and keeps its own label, unit and
//! message around it, so entering and leaving editing never moves the row.

use gpui::{App, Entity, Pixels, SharedString, Window, div, prelude::*};

use crate::{components::input::TextInput, text::Text, theme::ActiveTheme, tone::Tone};

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
    editor: Option<Entity<TextInput>>,
    end_aligned: bool,
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
            editor: None,
            end_aligned: false,
        }
    }

    /// A labelled field.
    pub fn labeled(label: impl Into<SharedString>, value: i64) -> Self {
        Self::new(value).label(label)
    }

    /// Put the value box at the row's end, where a `Cycler` or a `Toggle` puts its control, so a
    /// mixed settings list reads down one right-hand column. The label then reads at full
    /// contrast, as theirs do.
    pub fn end_aligned(mut self, end_aligned: bool) -> Self {
        self.end_aligned = end_aligned;
        self
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

    /// The live editor the row is being typed into, drawn in place of the value.
    ///
    /// The editor owns the text, the caret and the validation of what is typed; the field keeps
    /// only the label, the unit and the row chrome around it.
    pub fn editor(mut self, editor: Entity<TextInput>) -> Self {
        self.editor = Some(editor);
        self
    }

    /// Whether this field is being typed into.
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Clamp a candidate value into the field's range.
    pub fn clamp(&self, value: i64) -> i64 {
        let value = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(value, |max| value.min(max))
    }

    /// Whether the current value is inside the range.
    pub fn is_in_range(&self) -> bool {
        self.clamp(self.value) == self.value
    }

    /// The rule the value broke, stated exactly.
    pub fn range_message(&self) -> Option<SharedString> {
        if self.is_in_range() {
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
        // While an editor owns the row, the value behind it is whatever was last committed and
        // the editor states its own rule; deriving a range message from the stale number would
        // contradict the field the user is typing into.
        let message = if self.editor.is_some() {
            self.invalid.clone()
        } else {
            self.message()
        };
        let valid = message.is_none();
        let focused = self.focused || self.editor.is_some();
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
                // In a list of end-aligned controls the label is the row's name, drawn as a
                // `Cycler`'s or a `Toggle`'s is; beside a free-standing box it recedes.
                let text = if self.end_aligned {
                    Text::ui(label)
                } else {
                    Text::ui(label).muted()
                };
                match self.label_width {
                    Some(width) => text.w(width),
                    None => text,
                }
            }))
            .when(self.end_aligned, |el| el.child(div().flex_1()))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(theme.space.sm)
                    .min_w(theme.metrics.number_field_w)
                    .px(theme.space.sm)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border(theme.metrics.hairline)
                    .border_color(border)
                    .map(|slot| match self.editor {
                        Some(editor) => slot.child(editor),
                        None => slot.child(Text::data(self.value.to_string()).tone(if valid {
                            Tone::Default
                        } else {
                            Tone::Danger
                        })),
                    })
                    // The unit is a label, not a value: it never competes with the number.
                    .children(self.unit.map(|unit| Text::data(unit).faint())),
            )
            .children(message.map(|message| Text::caption(message).tone(Tone::Danger).ellipsize()));

        super::control::cursor_row(theme, focused, false, body)
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
