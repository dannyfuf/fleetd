//! `FactRow` — `label   value`, with the null rule.
//!
//! §1.3: a nullable inspection fact renders `—` in the faintest tone, **never `0`**, and the
//! verbatim warning that explains it rides underneath as a `Warning` variant. That rule is the
//! whole reason this component exists instead of two divs.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The three things a fact's value can be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactValue {
    /// A known value, rendered at normal contrast.
    Known(SharedString),
    /// Unknown. Renders `—` faint. Never `0`, never blank.
    Null,
    /// A verbatim warning string from the daemon, rendered amber with `triangle-alert`.
    Warning(SharedString),
}

impl FactValue {
    /// A known value.
    pub fn known(value: impl Into<SharedString>) -> Self {
        FactValue::Known(value.into())
    }

    /// A verbatim warning.
    pub fn warning(message: impl Into<SharedString>) -> Self {
        FactValue::Warning(message.into())
    }

    /// `Some(v)` -> known, `None` -> null. The correct way to render a nullable count.
    pub fn from_option(value: Option<impl Into<SharedString>>) -> Self {
        match value {
            Some(value) => FactValue::Known(value.into()),
            None => FactValue::Null,
        }
    }
}

/// One `label  value` line in a detail panel or a confirm.
#[derive(IntoElement)]
pub struct FactRow {
    label: Option<SharedString>,
    value: FactValue,
    label_width: Pixels,
    mono: bool,
}

impl FactRow {
    /// A labelled fact.
    pub fn new(label: impl Into<SharedString>, value: FactValue) -> Self {
        Self {
            label: Some(label.into()),
            value,
            label_width: px(96.0),
            mono: false,
        }
    }

    /// A warning with no label, e.g. the `warnings[]` lines under SAFETY.
    pub fn warning(message: impl Into<SharedString>) -> Self {
        Self {
            label: None,
            value: FactValue::warning(message),
            label_width: px(96.0),
            mono: false,
        }
    }

    /// Width of the label column. 96 px in the 340 px detail panel.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = width;
        self
    }

    /// Render the value in the data face (paths, shas, urls).
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }
}

impl RenderOnce for FactRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let value: gpui::AnyElement = match self.value {
            FactValue::Known(value) => {
                if self.mono {
                    Text::data(value).into_any_element()
                } else {
                    Text::ui(value).into_any_element()
                }
            }
            FactValue::Null => Text::ui("\u{2014}").faint().into_any_element(),
            FactValue::Warning(message) => div()
                .flex()
                .items_center()
                .gap(theme.space.xs)
                .child(
                    Icon::TriangleAlert
                        .el()
                        .size(IconSize::Small)
                        .color(theme.colors.warning),
                )
                .child(Text::ui(message).tone(Tone::Warning))
                .into_any_element(),
        };

        div()
            .flex()
            .items_start()
            .w_full()
            .gap(theme.space.sm)
            .children(
                self.label
                    .map(|label| Text::ui(label).muted().w(self.label_width)),
            )
            .child(div().flex_1().min_w_0().child(value))
    }
}
