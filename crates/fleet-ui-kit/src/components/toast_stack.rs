//! `Toast` / `ToastStack` — bottom-right, max 3, with identical-text coalescing.
//!
//! §2.7 states the law:
//!
//! > **A toast is allowed only when there is no row and no pill that already shows the
//! > outcome.**
//!
//! That rule is a decision the *caller* makes, but three of its consequences are mechanical
//! and are therefore encoded here rather than left to be re-derived at every call site:
//!
//! 1. **Coalescing.** Identical text within one second becomes one toast with a `×n` suffix
//!    ([`ToastStack::push_at`]). Overlapping operations with no duplicate suppression are what
//!    produced swarm's duplicate spam.
//! 2. **The cap.** Max three stacked, oldest evicted first.
//! 3. **Errors are never toasts.** They are sticky ([`super::StickyErrorSlot`]), so
//!    [`Toast::tone`] refuses [`Tone::Danger`] and renders such a toast amber instead of
//!    letting a transient red line exist.
//!
//! Dwell is the caller's timer: it owns the `Vec<Toast>` and removes an entry when
//! [`ToastDuration::millis`] has elapsed. `RenderOnce` cannot hold a timer.

use gpui::{App, SharedString, Window, deferred, div, prelude::*, px};

use crate::{
    components::OverlayLayer,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The window within which identical toast text coalesces, in milliseconds (§2.7).
pub const COALESCE_WINDOW_MS: u64 = 1_000;

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
    /// The tone. Never [`Tone::Danger`]: errors are sticky, not transient.
    pub tone: Tone,
    /// How long it stays.
    pub duration: ToastDuration,
    /// Coalescing count. Rendered as a `×n` suffix when above 1.
    pub count: usize,
    /// When the toast was raised, in milliseconds on the caller's clock. Only the
    /// *difference* between two toasts matters, so any monotonic millisecond source works.
    /// Left at `0` when the caller does not track time, which makes [`ToastStack::push`]
    /// coalesce identical text unconditionally.
    pub raised_at_ms: u64,
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
            raised_at_ms: 0,
        }
    }

    /// Set the glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the tone.
    ///
    /// [`Tone::Danger`] is coerced to [`Tone::Warning`]: §2.7 forbids an error toast, and a
    /// silently amber toast is a smaller failure than a red one that vanishes after 3.2 s.
    /// Debug builds assert instead, so the mistake is caught in the gallery.
    pub fn tone(mut self, tone: Tone) -> Self {
        debug_assert!(
            tone != Tone::Danger,
            "a toast is never Tone::Danger: errors are sticky (UX spec §2.7 / §1.8)"
        );
        self.tone = if tone == Tone::Danger {
            Tone::Warning
        } else {
            tone
        };
        self
    }

    /// Make it a 1.6 s toast.
    pub fn short(mut self) -> Self {
        self.duration = ToastDuration::Short;
        self
    }

    /// Stamp the toast with the caller's clock, enabling the one-second coalescing window.
    pub fn raised_at(mut self, millis: u64) -> Self {
        self.raised_at_ms = millis;
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
    /// The §2.7 cap: three stacked, oldest evicted first.
    pub const MAX: usize = 3;

    /// A stack over the currently live toasts.
    pub fn new(toasts: impl IntoIterator<Item = Toast>) -> Self {
        Self {
            toasts: toasts.into_iter().collect(),
            max: Self::MAX,
            bottom_inset: px(12.0),
        }
    }

    /// Change the stack cap. 3 by the spec.
    pub fn max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }

    /// Distance from the edges of the layer the stack is placed in. 12 px by §2.2.
    pub fn bottom_inset(mut self, inset: gpui::Pixels) -> Self {
        self.bottom_inset = inset;
        self
    }

    /// Whether the stack renders anything.
    pub fn is_visible(&self) -> bool {
        !self.toasts.is_empty()
    }

    /// Push a toast into a live list, applying the §2.7 rules with no clock: identical text
    /// coalesces into `×n`, and the oldest is evicted beyond `max`.
    ///
    /// The caller owns the list and the dwell timers; this is the pure part of the law. Use
    /// [`ToastStack::push_at`] when the caller has a clock and wants the one-second window
    /// the spec actually states.
    pub fn push(toasts: &mut Vec<Toast>, toast: Toast, max: usize) {
        let now = toast.raised_at_ms;
        Self::push_at(toasts, toast, max, now);
    }

    /// Push a toast, coalescing identical text raised within [`COALESCE_WINDOW_MS`] of `now`.
    ///
    /// A coalesced toast bumps its `count` **and** its `raised_at_ms`, so the dwell restarts
    /// from the last occurrence: three copies of `Path copied` read `Path copied ×3` and stay
    /// up 1.6 s after the third one, not after the first.
    pub fn push_at(toasts: &mut Vec<Toast>, toast: Toast, max: usize, now_ms: u64) {
        if let Some(existing) = toasts.iter_mut().find(|t| {
            t.text == toast.text && now_ms.saturating_sub(t.raised_at_ms) <= COALESCE_WINDOW_MS
        }) {
            existing.count += 1;
            existing.raised_at_ms = now_ms;
            existing.duration = toast.duration;
            return;
        }
        toasts.push(toast);
        while toasts.len() > max.max(1) {
            toasts.remove(0);
        }
    }

    /// The line a toast renders, `×n` suffix included. Exposed for tests.
    pub fn resolved_text(toast: &Toast) -> SharedString {
        if toast.count > 1 {
            SharedString::from(format!("{} \u{d7}{}", toast.text, toast.count))
        } else {
            toast.text.clone()
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
        // Oldest first, newest nearest the corner it grew from; anything past the cap is
        // dropped from the *front*, because §2.7 evicts the oldest.
        let visible: Vec<Toast> = self
            .toasts
            .into_iter()
            .rev()
            .take(self.max.max(1))
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
                .children(visible.into_iter().map(|toast| {
                    let tone = if toast.tone == Tone::Danger {
                        Tone::Warning
                    } else {
                        toast.tone
                    };
                    let color = tone.color(theme);
                    let text = ToastStack::resolved_text(&toast);
                    div()
                        .flex()
                        .flex_none()
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
                        .occlude()
                        .children(
                            toast
                                .icon
                                .map(|icon| icon.el().size(IconSize::Medium).color(color)),
                        )
                        .child(Text::ui(text).ellipsize())
                })),
        )
        .with_priority(OverlayLayer::Toast.priority())
        .into_any_element()
    }
}
