//! `Toast` / `ToastStack` — bottom-right, max 3, with identical-text coalescing.
//!
//! §2.7 states the law:
//!
//! > **A toast is allowed only when there is no row and no pill that already shows the
//! > outcome.**
//!
//! Agent-finished toasts deliberately make one exception: their session is usually off-screen,
//! so the visible row glyph is not a sufficient completion signal.
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
//! [`ToastDuration::millis`] has elapsed. The stack draws the supplied live entries.
//!
//! ## Pointer (ADR 0023)
//!
//! ```text
//! [ ✓ Created acme/api#spike            View J  ✕ ]
//! ```
//!
//! A toast that points somewhere carries a [`Toast::action`] label: the whole line and a small
//! ghost `View` button both run [`ToastStack::on_activate`], and the button shows the key that
//! does the same from the keyboard ([`ToastStack::action_keys`], resolved by the caller). Every
//! toast gets a ✕ once the caller handles [`ToastStack::on_dismiss`], and
//! [`ToastStack::on_hover`] tells the caller when to pause the decay — the dwell stays the
//! caller's timer, so the stack only reports the pointer. The line, the button and the ✕ are
//! siblings, so a click on one never also runs another.
//!
//! Harness names: `toasts.toast[N]` is the whole toast, `toasts.toast[N].action` its button and
//! `toasts.toast[N].close` its ✕, `N` counting the drawn toasts from the oldest.

use std::rc::Rc;

use gpui::{App, SharedString, Window, deferred, div, prelude::*};

use crate::{
    components::{Button, ButtonSize, ButtonStyle, IconButton, Kbd, OverlayLayer},
    harness::{self, HarnessTargetExt as _},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A handler told which toast, by its index in the list [`ToastStack::new`] was given. `Rc`
/// because one handler is cloned into every drawn toast.
type ToastHandler = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;
/// [`ToastHandler`] plus whether the pointer entered (`true`) or left.
type HoverHandler = Rc<dyn Fn(usize, bool, &mut Window, &mut App) + 'static>;

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

/// One toast: one icon, one line, no title; a `View` button when it points somewhere.
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
    /// The label of the button that goes where the toast points (`View`), when it points
    /// anywhere. What the button does is the stack's [`ToastStack::on_activate`].
    pub action: Option<SharedString>,
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
            action: None,
        }
    }

    /// Make the toast point somewhere: a button reading `label` and a click on the line both
    /// run [`ToastStack::on_activate`].
    pub fn action(mut self, label: impl Into<SharedString>) -> Self {
        self.action = Some(label.into());
        self
    }

    /// Set the glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the tone.
    ///
    /// [`Tone::Danger`] is rejected without panicking and coerced to [`Tone::Warning`]: §2.7
    /// forbids an error toast, while callers still need a recoverable presentation path.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = allowed_tone(tone);
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
    bottom_inset: Option<gpui::Pixels>,
    action_keys: Vec<Option<Kbd>>,
    on_activate: Option<ToastHandler>,
    on_dismiss: Option<ToastHandler>,
    on_hover: Option<HoverHandler>,
}

impl ToastStack {
    /// The §2.7 cap: three stacked, oldest evicted first.
    pub const MAX: usize = 3;

    /// A stack over the currently live toasts.
    pub fn new(toasts: impl IntoIterator<Item = Toast>) -> Self {
        Self {
            toasts: toasts.into_iter().collect(),
            max: Self::MAX,
            bottom_inset: None,
            action_keys: Vec::new(),
            on_activate: None,
            on_dismiss: None,
            on_hover: None,
        }
    }

    /// The key chip each toast's action button shows, in the same order as the toasts: the key
    /// that goes to the same place from the keyboard (`J` for Jobs), or `None` for no chip.
    pub fn action_keys(mut self, keys: impl IntoIterator<Item = Option<Kbd>>) -> Self {
        self.action_keys = keys.into_iter().collect();
        self
    }

    /// Run when a toast with a [`Toast::action`] is clicked, on its line or its button.
    pub fn on_activate(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }

    /// Run when a toast's ✕ is clicked. Setting it draws the ✕ on every toast.
    pub fn on_dismiss(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Run when the pointer enters (`true`) or leaves a toast, so the caller can hold its dwell.
    pub fn on_hover(
        mut self,
        handler: impl Fn(usize, bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }

    /// Change the stack cap. 3 by the spec.
    pub fn max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }

    /// Distance from the *bottom* of the layer the stack is placed in; the other three edges
    /// keep §2.2's 12 px. A surface that docks something along the bottom — the native agent
    /// tab's composer and its metadata row — raises this so a toast never paints over it.
    pub fn bottom_inset(mut self, inset: gpui::Pixels) -> Self {
        self.bottom_inset = Some(inset);
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
        let mut toast = toast;
        toast.raised_at_ms = now_ms;
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

    /// The line a toast renders, `×n` suffix included.
    fn resolved_text(toast: &Toast) -> SharedString {
        if toast.count > 1 {
            SharedString::from(format!("{} \u{d7}{}", toast.text, toast.count))
        } else {
            toast.text.clone()
        }
    }
}

fn allowed_tone(tone: Tone) -> Tone {
    if tone == Tone::Danger {
        Tone::Warning
    } else {
        tone
    }
}

/// `toasts.toast[N].<part>`, formatted only while the harness records.
fn target_name(index: usize, part: &'static str) -> SharedString {
    if harness::is_recording() {
        SharedString::from(format!("toasts.toast[{index}].{part}"))
    } else {
        SharedString::default()
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
        let first = self.toasts.len().saturating_sub(self.max.max(1));
        let mut keys = self.action_keys;
        keys.resize(self.toasts.len(), None);
        let visible: Vec<(Toast, Option<Kbd>)> =
            self.toasts.into_iter().zip(keys).skip(first).collect();
        let (on_activate, on_dismiss, on_hover) =
            (self.on_activate, self.on_dismiss, self.on_hover);
        let hover_bg = theme.colors.control_hover;

        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .justify_end()
                .items_end()
                .p(theme.metrics.toast_inset)
                .pb(self.bottom_inset.unwrap_or(theme.metrics.toast_inset))
                .gap(theme.space.sm)
                .children(
                    visible
                        .into_iter()
                        .enumerate()
                        .map(|(index, (toast, kbd))| {
                            let source = first + index;
                            let tone = allowed_tone(toast.tone);
                            let color = tone.color(theme);
                            let text = ToastStack::resolved_text(&toast);
                            let activate = toast
                                .action
                                .is_some()
                                .then(|| on_activate.clone())
                                .flatten();
                            let line = div()
                                .id(("toast-line", index))
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap(theme.space.sm)
                                .children(
                                    toast
                                        .icon
                                        .map(|icon| icon.el().size(IconSize::Medium).color(color)),
                                )
                                .child(Text::ui(text).ellipsize())
                                .when_some(activate.clone(), |el, activate| {
                                    super::control::on_activate(
                                        el.rounded(theme.radii.sm).hover(move |s| s.bg(hover_bg)),
                                        "View",
                                        move |window, cx| activate(source, window, cx),
                                    )
                                });
                            let action = toast.action.zip(activate).map(|(label, activate)| {
                                Button::new(("toast-action", index), label)
                                    .style(ButtonStyle::Ghost)
                                    .size(ButtonSize::Compact)
                                    .map(|button| match kbd {
                                        Some(kbd) => button.kbd(kbd),
                                        None => button,
                                    })
                                    .on_click(move |_, window, cx| activate(source, window, cx))
                                    .harness_target(target_name(index, "action"))
                            });
                            let close = on_dismiss.clone().map(|dismiss| {
                                IconButton::new(("toast-close", index), Icon::X, "Dismiss")
                                    .size(ButtonSize::Compact)
                                    .on_click(move |_, window, cx| dismiss(source, window, cx))
                                    .harness_target(target_name(index, "close"))
                            });
                            let hover = on_hover.clone();
                            div()
                                .id(("toast", index))
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(theme.space.xs)
                                .w(width)
                                .pl(theme.space.md)
                                .pr(theme.space.xs)
                                .py(theme.space.xs)
                                .rounded(theme.radii.md)
                                .bg(theme.colors.elevated)
                                .border(theme.metrics.hairline)
                                .border_color(theme.colors.border_strong)
                                .shadow(theme.sheet_shadow())
                                .occlude()
                                .when_some(hover, |el, hover| {
                                    el.on_hover(move |hovered, window, cx| {
                                        hover(source, *hovered, window, cx)
                                    })
                                })
                                .child(line)
                                .children(action)
                                .children(close)
                                // `toasts.toast[0]` is the oldest live toast, which is the order
                                // `docs/TESTING-HARNESS.md` §3 gives the `toasts` array as well.
                                .harness_target_indexed("toasts.toast", index)
                        }),
                ),
        )
        .with_priority(OverlayLayer::Toast.priority())
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_at_stamps_new_and_coalesced_toasts() {
        let mut toasts = Vec::new();
        ToastStack::push_at(&mut toasts, Toast::new("Path copied"), ToastStack::MAX, 400);
        assert_eq!(toasts[0].raised_at_ms, 400);

        ToastStack::push_at(&mut toasts, Toast::new("Path copied"), ToastStack::MAX, 900);
        assert_eq!(toasts[0].raised_at_ms, 900);
        assert_eq!(toasts[0].count, 2);

        ToastStack::push_at(
            &mut toasts,
            Toast::new("Path copied"),
            ToastStack::MAX,
            2_000,
        );
        assert_eq!(toasts.len(), 2);
        assert_eq!(toasts[1].raised_at_ms, 2_000);
    }

    #[test]
    fn danger_is_rejected_without_panicking() {
        let toast = Toast::new("failure").tone(Tone::Danger);
        assert_eq!(toast.tone, Tone::Warning);
    }
}
