//! `Dialog` — scrim + card + 44 px header + 44 px footer.
//!
//! ADR 0023: a dialog closes and confirms by pointer as well as by key. The header carries a
//! close ✕ and the footer a row of [`super::Button`]s, secondary first and the one primary
//! last, each showing its key as a chip. A click on the scrim closes the dialog through the same
//! path as the ✕ ([`Dialog::dismiss_action`]), which is the same action `esc` dispatches.
//!
//! The dialog is the only surface that ghosts the base screen: its scrim paints
//! `colors.overlay` over the whole frame and swallows pointer events, so the screen behind is
//! visibly frozen rather than merely covered. A palette uses [`super::Overlay`] instead.
//!
//! ## Keyboard
//!
//! The caller owns dialog actions and focus. The view binds `Esc` to close, `Enter` to the
//! primary action, and whatever else its buttons show. The buttons are not focusable; the key
//! on each chip is the keyboard path to it.
//!
//! ## Harness
//!
//! The ✕ paints `dialog.close` and the footer buttons paint `dialog.button[N]`, `0` leftmost
//! (`docs/TESTING-HARNESS.md` §3).

use gpui::{AnyElement, App, Pixels, SharedString, Window, deferred, div, prelude::*};

use super::{
    button::Button,
    dismiss::{Dismiss, dismiss_builders},
};
use crate::{
    components::{KeyHintRow, OverlayLayer},
    harness::HarnessTargetExt,
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
    width: Option<Pixels>,
    height: Option<Pixels>,
    tone: Tone,
    body: Option<AnyElement>,
    hints: Option<AnyElement>,
    footer_start: Option<AnyElement>,
    header_actions: Option<AnyElement>,
    actions: Vec<Button>,
    primary: Option<SharedString>,
    footer_note: Option<(SharedString, Tone)>,
    dismiss: Option<Dismiss>,
}

impl Dialog {
    /// A dialog with a title.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            title: title.into(),
            subtitle: None,
            width: None,
            height: None,
            tone: Tone::Default,
            body: None,
            hints: None,
            footer_start: None,
            header_actions: None,
            actions: Vec::new(),
            primary: None,
            footer_note: None,
            dismiss: None,
        }
    }

    dismiss_builders!();

    /// Carry a dismiss already chosen by a wrapping component such as
    /// [`super::ConfirmDialog`].
    pub(super) fn dismiss(mut self, dismiss: Option<Dismiss>) -> Self {
        self.dismiss = dismiss;
        self
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
        self.width = Some(width);
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

    /// Controls on the right of the header, before the close ✕: a search field (Settings) or a
    /// segmented control (Help).
    pub fn header_actions(mut self, actions: impl IntoElement) -> Self {
        self.header_actions = Some(actions.into_any_element());
        self
    }

    /// The footer's buttons, right-aligned in the order given: secondary first (`Cancel`), the
    /// one primary last. Each paints `dialog.button[N]`, `0` leftmost.
    pub fn actions(mut self, actions: Vec<Button>) -> Self {
        self.actions = actions;
        self
    }

    /// The footer's left side: a link-like control such as "Open config.json".
    pub fn footer_start(mut self, start: impl IntoElement) -> Self {
        self.footer_start = Some(start.into_any_element());
        self
    }

    /// The footer's left side: contextual key hints. A dialog on the button footer does not
    /// need them, because each button shows its own key (ADR 0023).
    pub fn hints(mut self, hints: impl IntoElement) -> Self {
        self.hints = Some(hints.into_any_element());
        self
    }

    /// Convenience for a [`KeyHintRow`] footer.
    pub fn hint_row(self, hints: KeyHintRow) -> Self {
        self.hints(hints)
    }

    /// The footer's right side as a bold label, e.g. `⏎ Create`.
    ///
    /// **Deprecated:** kept only while dialogs migrate to [`Self::actions`], which draws real
    /// buttons with live key chips. Ignored when `actions` is set.
    pub fn primary(mut self, primary: impl Into<SharedString>) -> Self {
        self.primary = Some(primary.into());
        self
    }

    /// A red footer line: an exact conflict or write error. The dialog **stays open**, the
    /// line sits directly above the button row, and the buttons never move (the error is its
    /// own 22 px strip), so the control the user was about to press does not shift under them.
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
        let on_scrim = self.dismiss.as_ref().map(Dismiss::callback);
        let close = self.dismiss.as_ref().map(|dismiss| {
            dismiss
                .close_button("dialog-close")
                .harness_target("dialog.close")
        });

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.dialog_header_h)
            .pl(theme.space.lg)
            .map(|el| {
                if close.is_some() {
                    el.pr(theme.space.sm)
                } else {
                    el.pr(theme.space.lg)
                }
            })
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .children(
                self.icon
                    .map(|icon| icon.el().size(IconSize::Large).color(tone_color)),
            )
            .child(Text::title(self.title).color(tone_color))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .children(self.subtitle.map(|s| Text::ui(s).muted().ellipsize())),
            )
            .children(self.header_actions)
            .children(close);

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

        let trailing = if self.actions.is_empty() {
            self.primary
                .map(|p| Text::ui_strong(p).ellipsize().into_any_element())
        } else {
            Some(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.sm)
                    .children(
                        self.actions
                            .into_iter()
                            .enumerate()
                            .map(|(ix, button)| button.harness_target_indexed("dialog.button", ix)),
                    )
                    .into_any_element(),
            )
        };

        let footer = div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .bg(theme.colors.surface)
            .border_t(theme.metrics.hairline)
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
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(theme.space.md)
                            .children(self.hints)
                            .children(self.footer_start),
                    )
                    .children(trailing),
            );

        deferred(
            div()
                .id("dialog-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(theme.space.xl)
                .bg(theme.colors.overlay)
                .occlude()
                .when_some(on_scrim, |el, dismiss| {
                    el.on_click(move |_, window, cx| dismiss(window, cx))
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(self.width.unwrap_or(theme.metrics.dialog_w))
                        .when_some(self.height, |el, h| el.h(h))
                        .max_h_full()
                        .min_h_0()
                        .rounded(theme.radii.dialog)
                        .bg(theme.colors.elevated)
                        .border(theme.metrics.hairline)
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

#[cfg(test)]
mod tests;
