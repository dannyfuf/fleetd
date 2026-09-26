//! The box [`TextInput`] draws around its value: border ladder, label, icon and status line.

use gpui::{App, div, prelude::*};

use super::{InputMode, TextInput};
use crate::{
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

impl TextInput {
    fn border_color(&self, focused: bool, cx: &App) -> gpui::Hsla {
        let theme = cx.theme();
        if self.invalid.is_some() {
            theme.colors.danger
        } else if focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        }
    }

    /// The face the value is drawn in. A placeholder says what empty means and is a sentence
    /// (`Type the next command…`), so it is the UI face whatever face the value takes.
    pub(super) fn value_role(&self) -> TextRole {
        if self.mono && !self.buffer.is_empty() {
            TextRole::Data
        } else {
            TextRole::Ui
        }
    }

    pub(super) fn render_chrome(
        &self,
        focused: bool,
        value: impl IntoElement,
        cx: &App,
    ) -> gpui::Div {
        let theme = cx.theme();
        let role = self.value_role();
        let icon_color = if focused {
            theme.colors.text_secondary
        } else {
            theme.colors.text_muted
        };
        let box_element = div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .w_full()
            .px(theme.space.md)
            .rounded(theme.radii.sm)
            .bg(theme.colors.bg)
            .border(theme.metrics.hairline)
            .border_color(self.border_color(focused, cx))
            .overflow_hidden()
            .children(
                self.icon
                    .map(|icon| icon.el().size(crate::IconSize::Medium).color(icon_color)),
            )
            .child(
                styled_with(div(), role.style(theme), theme)
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h(role.style(theme).line_height)
                    .when(
                        matches!(self.mode(), InputMode::Multiline { .. }),
                        |element| element.h_auto(),
                    )
                    .overflow_hidden()
                    .text_color(theme.colors.text)
                    .child(value),
            )
            .when_else(
                matches!(self.mode(), InputMode::SingleLine),
                |element| element.h(theme.metrics.text_field_h),
                |element| element.min_h(theme.metrics.text_field_h).py(theme.space.sm),
            );

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(self.label.clone().map(Text::label))
            .child(box_element)
            .when(
                matches!(self.mode(), InputMode::SingleLine) && !self.hide_status_line,
                |element| {
                    element.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(theme.metrics.field_status_h)
                            .w_full()
                            .overflow_hidden()
                            .child(match self.invalid.clone() {
                                Some(message) => {
                                    Text::caption(message).tone(Tone::Danger).ellipsize()
                                }
                                None => Text::caption(self.preview.clone().unwrap_or_default())
                                    .faint()
                                    .ellipsize(),
                            }),
                    )
                },
            )
    }
}
