//! `Dialog` — scrim + card + 44 px header + 44 px footer.
//!
//! §3.8: no close button (`Esc` closes), and **no OK/Cancel button pair anywhere** — the
//! footer hint row states the keys, because this is a keyboard app. The primary action is a
//! label on the right of the footer, not a button.
//!
//! The dialog is the only surface that ghosts the base screen: its scrim paints
//! `colors.overlay` over the whole frame and swallows pointer events, so the screen behind is
//! visibly frozen rather than merely covered. A palette uses [`super::Overlay`] instead.
//!
//! ## Keyboard
//!
//! The dialog owns no keys — it cannot, because [`gpui::RenderOnce`] holds no state. The view
//! binds `Esc` to close, `Enter` to the primary action, and the keys its own
//! [`super::KeyHintRow`] advertises. What the component guarantees is that every one of those
//! keys is *stated* in the footer.

use gpui::{AnyElement, App, Pixels, SharedString, Window, deferred, div, prelude::*, px};

use crate::{
    components::{KeyHintRow, OverlayLayer},
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
    footer_note: Option<(SharedString, Tone)>,
}

impl Dialog {
    /// The default card width (560 px): create, clone and the expanded confirm.
    pub const DEFAULT_WIDTH: Pixels = px(560.0);

    /// A dialog with a title.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            subtitle: None,
            width: Self::DEFAULT_WIDTH,
            height: None,
            tone: Tone::Default,
            body: None,
            hints: None,
            primary: None,
            footer_note: None,
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

    /// A fixed height. Omit for `auto`; the card never grows past 90 % of the window either
    /// way, and the body scrolls instead.
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

    /// A red footer line: an exact conflict or write error. The dialog **stays open**, the
    /// line sits directly above the hint row, and the hints never move (the error is its own
    /// 22 px strip), so the keys the user was about to press do not shift under their eyes.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.footer_note = Some((error.into(), Tone::Danger));
        self
    }

    /// An amber footer line in the same strip: a consequence worth stating about a setting that
    /// is nonetheless **allowed**.
    ///
    /// §2.4 reserves red for "failed, changes requested, danger". A permitted configuration —
    /// §3.8.4's context with no owners, say — is a warning, not an error, and painting it red
    /// tells the user they did something wrong when they did not.
    pub fn warning(mut self, warning: impl Into<SharedString>) -> Self {
        self.footer_note = Some((warning.into(), Tone::Warning));
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
            .children(self.subtitle.map(|s| Text::ui(s).muted().ellipsize()));

        let error_line = self.footer_note.map(|(note, tone)| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.sm)
                .h(theme.metrics.strip_h)
                .w_full()
                .px(theme.space.lg)
                .bg(tone.fill(theme))
                .child(
                    Icon::TriangleAlert
                        .el()
                        .size(IconSize::Small)
                        .color(tone.color(theme)),
                )
                .child(Text::ui(note).tone(tone).ellipsize())
        });

        let footer = div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .border_t(px(1.0))
            .border_color(theme.colors.border)
            .children(error_line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(theme.metrics.dialog_footer_h)
                    .px(theme.space.lg)
                    .gap(theme.space.md)
                    .child(div().flex().flex_1().min_w_0().children(self.hints))
                    .children(self.primary.map(|p| Text::ui_strong(p).ellipsize())),
            );

        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(theme.space.xl)
                .bg(theme.colors.overlay)
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(self.width)
                        .when_some(self.height, |el, h| el.h(h))
                        .max_h_full()
                        .min_h_0()
                        .rounded(theme.radii.lg)
                        .bg(theme.colors.elevated)
                        .border_1()
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.dialog_shadow())
                        .overflow_hidden()
                        .occlude()
                        .child(header)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .p(theme.space.lg)
                                .gap(theme.space.md)
                                .children(self.body),
                        )
                        .child(footer),
                ),
        )
        .with_priority(OverlayLayer::Dialog.priority())
    }
}
