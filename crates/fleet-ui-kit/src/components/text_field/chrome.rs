use gpui::{Hsla, Pixels, SharedString, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::Theme,
    tone::Tone,
};

/// Everything both the presentational [`TextField`] and the live [`TextInput`] draw around the
/// value: the label, the box, the leading glyph and the 18 px status slot.
pub(super) struct FieldChrome {
    pub(super) label: Option<SharedString>,
    pub(super) icon: Option<Icon>,
    pub(super) prefix: Option<SharedString>,
    pub(super) preview: Option<SharedString>,
    pub(super) invalid: Option<SharedString>,
    pub(super) focused: bool,
    pub(super) mono: bool,
    pub(super) height: Option<Pixels>,
    pub(super) hide_status_line: bool,
}

impl FieldChrome {
    /// The type role the value renders in.
    pub(super) fn role(&self) -> TextRole {
        if self.mono {
            TextRole::Data
        } else {
            TextRole::Ui
        }
    }

    /// The box border: danger beats focus, focus beats rest.
    fn border(&self, theme: &Theme) -> Hsla {
        if self.invalid.is_some() {
            theme.colors.danger
        } else if self.focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        }
    }

    /// Wrap `value` — the caret-bearing content — in the label / box / status-line frame.
    pub(super) fn wrap(self, theme: &Theme, value: impl IntoElement) -> gpui::Div {
        let role = self.role();
        let box_h = self.height.unwrap_or(theme.metrics.text_field_h);
        let border = self.border(theme);
        let icon_color = if self.focused {
            theme.colors.text_secondary
        } else {
            theme.colors.text_muted
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(self.label.map(Text::label))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .h(box_h)
                    .w_full()
                    .px(theme.space.md)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border(theme.metrics.hairline)
                    .border_color(border)
                    .overflow_hidden()
                    .children(
                        self.icon
                            .map(|icon| icon.el().size(IconSize::Medium).color(icon_color)),
                    )
                    // A literal prompt character, for a field whose leading mark *is* the key
                    // the user pressed (§3.9's `:`), where a glyph would name a different key.
                    .children(
                        self.prefix
                            .map(|prefix| Text::data(prefix).color(icon_color)),
                    )
                    .child(
                        // The value area carries the role's font so a shaped caret lines up
                        // with the glyphs around it.
                        styled_with(div(), role.style(theme), theme)
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .h(role.style(theme).line_height)
                            .overflow_hidden()
                            .text_color(theme.colors.text)
                            .child(value),
                    ),
            )
            .when(!self.hide_status_line, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(theme.metrics.field_status_h)
                        .w_full()
                        .overflow_hidden()
                        .child(match self.invalid {
                            Some(message) => Text::hint(message).tone(Tone::Danger).ellipsize(),
                            None => Text::hint(self.preview.unwrap_or_default())
                                .faint()
                                .ellipsize(),
                        }),
                )
            })
    }
}
