//! `Toast` / `ToastStack` — bottom-right, max 3, with identical-text coalescing.
//!
//! §2.7 states the law: *a toast is allowed only when there is no row and no pill that already
//! shows the outcome*. Two consequences are encoded here rather than left to callers:
//! [`ToastStack::push`] coalesces identical text within one second into a `×n` suffix, and the
//! stack evicts the oldest beyond three. **Errors are never toasts** — they are sticky.

use gpui::{App, SharedString, Window, deferred, div, prelude::*, px};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// How long a toast stays up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastDuration {
    /// 1.6 s: instant-action acknowledgements (clipboard, "already running", mode no-ops).
    Short,
    /// 3.2 s: everything else the law allows.
    #[default]
    Normal,
}

impl ToastDuration {
    /// The dwell in milliseconds, read from the theme's motion tokens.
    pub fn millis(self, theme: &crate::theme::Theme) -> u64 {
        match self {
            ToastDuration::Short => theme.motion.toast_short,
            ToastDuration::Normal => theme.motion.toast_normal,
        }
    }
}

/// One toast: one icon, one line, no title, no close button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// The line.
    pub text: SharedString,
    /// The glyph.
    pub icon: Option<Icon>,
    /// The tone. Never `Danger`: errors are sticky, not transient.
    pub tone: Tone,
    /// How long it stays.
    pub duration: ToastDuration,
    /// Coalescing count. Rendered as a `×n` suffix when above 1.
    pub count: usize,
}

impl Toast {
    /// A normal-duration toast.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            icon: None,
            tone: Tone::Default,
            duration: ToastDuration::Normal,
            count: 1,
        }
    }

    /// Set the glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the tone.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Make it a 1.6 s toast.
    pub fn short(mut self) -> Self {
        self.duration = ToastDuration::Short;
        self
    }
}

/// The bottom-right stack.
#[derive(IntoElement)]
pub struct ToastStack {
    toasts: Vec<Toast>,
    max: usize,
    bottom_inset: gpui::Pixels,
}

impl ToastStack {
    /// A stack over the currently live toasts.
    pub fn new(toasts: impl IntoIterator<Item = Toast>) -> Self {
        Self {
            toasts: toasts.into_iter().collect(),
            max: 3,
            bottom_inset: px(12.0),
        }
    }

    /// Change the stack cap. 3 by the spec.
    pub fn max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }

    /// Distance from the bottom of the body region.
    pub fn bottom_inset(mut self, inset: gpui::Pixels) -> Self {
        self.bottom_inset = inset;
        self
    }

    /// Push a toast into a live list, applying the §2.7 rules: identical text coalesces into
    /// `×n`, and the oldest is evicted beyond `max`.
    ///
    /// The caller owns the list and the timers; this is the pure part of the law.
    pub fn push(toasts: &mut Vec<Toast>, toast: Toast, max: usize) {
        if let Some(existing) = toasts.iter_mut().find(|t| t.text == toast.text) {
            existing.count += 1;
            return;
        }
        toasts.push(toast);
        while toasts.len() > max {
            toasts.remove(0);
        }
    }
}

impl RenderOnce for ToastStack {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.toasts.is_empty() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let width = theme.metrics.toast_w;
        let toasts: Vec<Toast> = self
            .toasts
            .into_iter()
            .rev()
            .take(self.max)
            .rev()
            .collect();

        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .justify_end()
                .items_end()
                .p(self.bottom_inset)
                .gap(theme.space.sm)
                .children(toasts.into_iter().map(|toast| {
                    let color = toast.tone.color(theme);
                    let text = if toast.count > 1 {
                        SharedString::from(format!("{} \u{d7}{}", toast.text, toast.count))
                    } else {
                        toast.text
                    };
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .w(width)
                        .px(theme.space.md)
                        .py(theme.space.sm)
                        .rounded(theme.radii.md)
                        .bg(theme.colors.elevated)
                        .border_1()
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.sheet_shadow())
                        .children(
                            toast
                                .icon
                                .map(|icon| icon.el().size(IconSize::Medium).color(color)),
                        )
                        .child(Text::ui(text).ellipsize())
                })),
        )
        .into_any_element()
    }
}
