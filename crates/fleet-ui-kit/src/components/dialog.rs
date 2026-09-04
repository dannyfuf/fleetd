//! `Dialog` — scrim + card + 44 px header + 44 px footer.
//!
//! §3.8: no close button (`Esc` closes), and **no OK/Cancel button pair anywhere** — the
//! footer hint row states the keys, because this is a keyboard app. The primary action is a
//! label on the right of the footer, not a button.

use gpui::{AnyElement, App, Pixels, SharedString, Window, deferred, div, prelude::*, px};

use crate::{
    components::KeyHintRow,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A centered modal card.
#[derive(IntoElement)]
pub struct Dialog {
    icon: Option<Icon>,
    title: SharedString,
    subtitle: Option<SharedString>,
    width: Pixels,
    height: Option<Pixels>,
    tone: Tone,
    body: Option<AnyElement>,
    hints: Option<AnyElement>,
    primary: Option<SharedString>,
    footer_error: Option<SharedString>,
}

impl Dialog {
    /// A dialog with a title.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            subtitle: None,
            width: px(560.0),
            height: None,
            tone: Tone::Default,
            body: None,
            hints: None,
            primary: None,
            footer_error: None,
        }
    }

    /// The header glyph (§3.8 lists one per dialog).
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A muted suffix on the title row, e.g. `· buk/payroll`.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// The card width. §3.8 fixes one per dialog: 460 / 480 / 520 / 560 / 720 / 880.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// A fixed height. Omit for `auto`.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Tint the header glyph and title, e.g. amber for an expanded confirm.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// The body.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The footer's left side: contextual key hints.
    pub fn hints(mut self, hints: impl IntoElement) -> Self {
        self.hints = Some(hints.into_any_element());
        self
    }

    /// Convenience for a [`KeyHintRow`] footer.
    pub fn hint_row(self, hints: KeyHintRow) -> Self {
        self.hints(hints)
    }

    /// The footer's right side: the primary action label only, e.g. `⏎ Create`.
    pub fn primary(mut self, primary: impl Into<SharedString>) -> Self {
        self.primary = Some(primary.into());
        self
    }

    /// A red footer line: an exact conflict or write error. The dialog stays open.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.footer_error = Some(error.into());
        self
    }
}

impl RenderOnce for Dialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone_color = self.tone.color(theme);
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.dialog_header_h)
            .px(theme.space.lg)
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .children(
                self.icon
                    .map(|icon| icon.el().size(IconSize::Large).color(tone_color)),
            )
            .child(Text::title(self.title).color(tone_color))
            .children(self.subtitle.map(|s| Text::ui(s).muted()));

        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(theme.metrics.dialog_footer_h)
            .px(theme.space.lg)
            .gap(theme.space.md)
            .border_t(px(1.0))
            .border_color(theme.colors.border)
            .child(div().flex().flex_1().min_w_0().children(self.hints))
            .children(
                self.footer_error
                    .map(|e| Text::ui(e).tone(Tone::Danger).ellipsize()),
            )
            .children(self.primary.map(Text::ui_strong));

        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.colors.overlay)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(self.width)
                        .when_some(self.height, |el, h| el.h(h))
                        .rounded(theme.radii.lg)
                        .bg(theme.colors.elevated)
                        .border_1()
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.dialog_shadow())
                        .overflow_hidden()
                        .child(header)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .p(theme.space.lg)
                                .gap(theme.space.md)
                                .children(self.body),
                        )
                        .child(footer),
                ),
        )
    }
}
