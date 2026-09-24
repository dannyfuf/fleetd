//! `Button` and `IconButton` — the pointer's way to run an action, each showing its key.
//!
//! The default wiring is [`Button::action`]: the click dispatches the action to the focused
//! element, exactly as its key would, and the button shows the key the live keymap gives that
//! action as a [`Kbd`] chip. Behaviour and chip therefore cannot drift apart (DESIGN-SYSTEM §4).
//! [`Button::on_click`] is for the rare control that is not an action.
//!
//! Buttons are **not focusable** (ADR 0023): no tab stop, no focus ring. The keyboard path is the
//! key the chip shows, and `Tab`, `j`/`k` and pane focus keep their meaning. An action invalid
//! on a surface is hidden, not disabled; [`Button::disabled`] is for "valid soon on this same
//! surface", such as Save before anything changed.
//!
//! Use an [`IconButton`] when a glyph alone names the action in a dense toolbar or header; its
//! label becomes the tooltip and the accessible name. Use a clickable `Row` or `Chip`, not a
//! button, when the thing clicked is a piece of data rather than a verb.
//!
//! Two chrome shapes share the same frame: a [`StatusButton`] is a live count that opens what it
//! counts (`1 needs you`, `2 jobs`, `fleetd down`), and a [`SwitcherButton`] names the current
//! choice and opens a menu of the others (the title bar's context switcher, a breadcrumb's
//! worktree). Both keep their key in the tooltip rather than on the face, because the chrome they
//! sit in is read at a glance and a row of chips would drown the counts.

use gpui::{
    Action, App, ClickEvent, Div, ElementId, Hsla, SharedString, Stateful, Toggled, Window, div,
    prelude::*,
};

use super::{
    control,
    kbd::{Kbd, KbdSize, KbdTone},
    tooltip::{Tooltip, WithTooltip},
};
use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// How loudly a button asks to be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonStyle {
    /// `accent_fill`: the surface's one primary action.
    Primary,
    /// `control` fill inside a `control_border` hairline: every other action.
    #[default]
    Secondary,
    /// No fill until hovered: toolbar and header actions, and every [`IconButton`] by default.
    Ghost,
    /// `danger` fill: the strong form of a destructive action (the `Y` of a confirmation).
    Danger,
    /// A [`ButtonStyle::Ghost`] whose label is `danger`: a destructive action that is not the
    /// surface's main one, set apart from its neighbours rather than shouted (an approval's
    /// "Deny and stop").
    GhostDanger,
}

/// Button height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// `button_h`: dialog footers, empty states, page headers.
    #[default]
    Default,
    /// `button_h_compact`: inside a row, a pane header or a toolbar.
    Compact,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// What [`Button`] and [`IconButton`] share: identity, look, and what a click does.
struct ButtonBase {
    id: ElementId,
    style: ButtonStyle,
    size: ButtonSize,
    kbd: Option<Kbd>,
    preferred_key: Option<&'static str>,
    key_action: Option<Box<dyn Action>>,
    action: Option<Box<dyn Action>>,
    on_click: Option<ClickHandler>,
    disabled: bool,
    selected: Option<bool>,
}

/// The colours one button paints with, resolved from its style and state.
struct Paint {
    bg: Hsla,
    hover: Hsla,
    active: Hsla,
    border: Hsla,
    fg: Hsla,
    kbd: KbdTone,
}

impl ButtonBase {
    fn new(id: ElementId, style: ButtonStyle) -> Self {
        Self {
            id,
            style,
            size: ButtonSize::Default,
            kbd: None,
            preferred_key: None,
            key_action: None,
            action: None,
            on_click: None,
            disabled: false,
            selected: None,
        }
    }

    fn paint(&self, theme: &Theme) -> Paint {
        let c = &theme.colors;
        let clear = gpui::transparent_black();
        let selected = self.selected == Some(true);
        match self.style {
            ButtonStyle::Primary => Paint {
                bg: c.accent_fill,
                hover: c.accent_fill_hover,
                active: c.accent_fill_active,
                border: c.accent_fill,
                fg: c.accent_fill_text,
                kbd: KbdTone::OnAccent,
            },
            ButtonStyle::Danger => Paint {
                bg: c.danger,
                hover: c.danger_fill_hover,
                active: c.danger_fill_active,
                border: c.danger,
                fg: c.text_inverse,
                kbd: KbdTone::OnDanger,
            },
            // Selection is a background (DESIGN-SYSTEM §3), the same one a selected row has.
            ButtonStyle::Secondary => Paint {
                bg: if selected { c.row_selected } else { c.control },
                hover: c.control_hover,
                active: c.control_active,
                border: c.control_border,
                fg: c.text,
                kbd: KbdTone::Default,
            },
            ButtonStyle::Ghost => Paint {
                bg: if selected { c.row_selected } else { clear },
                hover: c.control_hover,
                active: c.control_active,
                border: clear,
                fg: if selected { c.text } else { c.text_secondary },
                kbd: KbdTone::Default,
            },
            ButtonStyle::GhostDanger => Paint {
                bg: clear,
                hover: c.control_hover,
                active: c.control_active,
                border: clear,
                fg: c.danger,
                kbd: KbdTone::Default,
            },
        }
    }

    fn height(&self, theme: &Theme) -> gpui::Pixels {
        match self.size {
            ButtonSize::Default => theme.metrics.button_h,
            ButtonSize::Compact => theme.metrics.button_h_compact,
        }
    }

    /// The chip: the one the caller gave, else the live binding of the action, else none.
    fn resolve_kbd(&mut self, window: &Window, cx: &App) -> Option<Kbd> {
        self.kbd.take().or_else(|| {
            let action = self.key_action.as_deref().or(self.action.as_deref())?;
            match self.preferred_key {
                Some(keys) => Kbd::for_action_preferring(action, keys, window, cx),
                None => Kbd::for_action(action, window, cx),
            }
        })
    }

    /// The pressable frame: fill, hairline, pointer states and the click. The caller adds the
    /// padding and the content.
    fn frame(
        self,
        name: SharedString,
        kbd: Option<&Kbd>,
        paint: &Paint,
        theme: &Theme,
    ) -> Stateful<Div> {
        let (hover, active) = (paint.hover, paint.active);
        let height = self.height(theme);
        let frame = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.space.sm)
            .h(height)
            .rounded(theme.radii.control)
            .bg(paint.bg)
            .border(theme.metrics.hairline)
            .border_color(paint.border)
            .when_some(kbd.and_then(Kbd::aria_shortcut), |el, shortcut| {
                el.aria_keyshortcuts(shortcut)
            })
            .when_some(self.selected, |el, selected| {
                el.aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
            });
        if self.disabled {
            return frame
                .opacity(theme.metrics.dimmed_opacity)
                .role(gpui::Role::Button)
                .aria_label(name);
        }
        let on_click = self.on_click;
        let action = self.action;
        control::on_click_named(
            frame
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(active)),
            name,
            move |event, window, cx| {
                if let Some(on_click) = &on_click {
                    on_click(event, window, cx);
                }
                if let Some(action) = &action {
                    window.dispatch_action(action.boxed_clone(), cx);
                }
            },
        )
    }
}

/// Builder methods [`Button`] and [`IconButton`] both expose, spelled once.
macro_rules! button_builders {
    () => {
        /// Set the style.
        pub fn style(mut self, style: ButtonStyle) -> Self {
            self.base.style = style;
            self
        }

        /// Set the height.
        pub fn size(mut self, size: ButtonSize) -> Self {
            self.base.size = size;
            self
        }

        /// Show this key instead of the one [`Self::action`] resolves. For a control whose key
        /// is not an action binding; never a hand-typed string for a key the keymap owns.
        pub fn kbd(mut self, kbd: Kbd) -> Self {
            self.base.kbd = Some(kbd);
            self
        }

        /// Of [`Self::action`]'s live bindings, show `keys` when it is one of them; see
        /// [`Kbd::for_action_preferring`].
        pub fn prefer_key(mut self, keys: &'static str) -> Self {
            self.base.preferred_key = Some(keys);
            self
        }

        /// Show the live key of `action` instead of [`Self::action`]'s, without dispatching it:
        /// for a control that does in one click what that key does from where the keyboard is
        /// (`Clear filter` shows the `esc` that clears once the filter input is left). Resolved
        /// like [`Self::action`]'s, so the chip disappears wherever that key does nothing.
        pub fn key_of(mut self, action: Box<dyn Action>) -> Self {
            self.base.key_action = Some(action);
            self
        }

        /// Dispatch `action` to the focused element on click, and show its live key binding.
        pub fn action(mut self, action: Box<dyn Action>) -> Self {
            self.base.action = Some(action);
            self
        }

        /// Run `handler` on click. With [`Self::action`] as well, the handler runs first.
        pub fn on_click(
            mut self,
            handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        ) -> Self {
            self.base.on_click = Some(Box::new(handler));
            self
        }

        /// Draw the button at 40 % with no hover and no click: an action that becomes valid on
        /// this same surface. An action invalid here is hidden instead.
        pub fn disabled(mut self, disabled: bool) -> Self {
            self.base.disabled = disabled;
            self
        }

        /// Make this a toggle and say whether it is on. A selected button paints the
        /// `row_selected` background and announces itself as pressed.
        pub fn selected(mut self, selected: bool) -> Self {
            self.base.selected = Some(selected);
            self
        }
    };
}

/// A labelled button, optionally with a leading icon and a trailing key chip.
#[derive(IntoElement)]
pub struct Button {
    base: ButtonBase,
    label: SharedString,
    icon: Option<Icon>,
    dot: Option<Hsla>,
    full_width: bool,
    tooltip: Option<SharedString>,
}

impl Button {
    /// A secondary button reading `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            base: ButtonBase::new(id.into(), ButtonStyle::Secondary),
            label: label.into(),
            icon: None,
            dot: None,
            full_width: false,
            tooltip: None,
        }
    }

    button_builders!();

    /// Lead the label with a glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Lead the label with a small dot of `color`: a value that has a colour of its own, such
    /// as a board card's status. Pass a theme token, never a literal.
    pub fn dot(mut self, color: Hsla) -> Self {
        self.dot = Some(color);
        self
    }

    /// Stretch across the container, label centred.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Explain the button when the pointer rests on it. The key is already on the button, so
    /// the tooltip carries only `text`.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

impl RenderOnce for Button {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kbd = self.base.resolve_kbd(window, cx);
        let theme = cx.theme();
        let paint = self.base.paint(theme);
        let (kbd_size, icon_size, pad) = match self.base.size {
            ButtonSize::Default => (KbdSize::Default, IconSize::Medium, theme.space.md),
            ButtonSize::Compact => (KbdSize::Small, IconSize::Small, theme.space.sm),
        };
        let chip = kbd.clone().map(|kbd| kbd.tone(paint.kbd).size(kbd_size));
        let icon = self
            .icon
            .map(|icon| icon.el().size(icon_size).color(paint.fg));
        let dot = self.dot.map(|color| {
            div()
                .flex_none()
                .size(theme.metrics.dot_size_small)
                .rounded(theme.radii.full)
                .bg(color)
        });
        let label = Text::ui_strong(self.label.clone()).color(paint.fg);
        let tooltip = self.tooltip.map(Tooltip::new);
        let full_width = self.full_width;
        self.base
            .frame(self.label, kbd.as_ref(), &paint, theme)
            .px(pad)
            .map(|el| {
                if full_width {
                    el.w_full()
                } else {
                    el.flex_none()
                }
            })
            .children(icon)
            .children(dot)
            .child(label)
            .children(chip)
            .when_some(tooltip, |el, tooltip| el.with_tooltip(tooltip, cx))
    }
}

/// A square, icon-only button. The label is required: it is the tooltip and the accessible
/// name, and the tooltip also carries the key.
#[derive(IntoElement)]
pub struct IconButton {
    base: ButtonBase,
    icon: Icon,
    label: SharedString,
}

impl IconButton {
    /// A ghost icon button showing `icon`, named `label`.
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>) -> Self {
        Self {
            base: ButtonBase::new(id.into(), ButtonStyle::Ghost),
            icon,
            label: label.into(),
        }
    }

    button_builders!();
}

impl RenderOnce for IconButton {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kbd = self.base.resolve_kbd(window, cx);
        let theme = cx.theme();
        let paint = self.base.paint(theme);
        let side = self.base.height(theme);
        let icon_size = match self.base.size {
            ButtonSize::Default => IconSize::Medium,
            ButtonSize::Compact => IconSize::Small,
        };
        let tooltip = Tooltip::new(self.label.clone()).kbd(kbd.clone());
        self.base
            .frame(self.label, kbd.as_ref(), &paint, theme)
            .flex_none()
            .w(side)
            .child(self.icon.el().size(icon_size).color(paint.fg))
            .with_tooltip(tooltip, cx)
    }
}

/// What leads a [`StatusButton`]'s label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMark {
    /// A small dot in the button's tone: something is waiting for a person.
    Dot,
    /// A glyph in the button's tone.
    Icon(Icon),
    /// A spinning `loader-circle`: something is running.
    Spinner,
}

/// A ghost button that is a live count and opens what it counts: `1 needs you`, `2 jobs`,
/// `1 failed`, `fleetd down`.
///
/// The label is drawn in the button's [`Tone`], because the colour *is* the state (amber waits for
/// a person, red failed). The key goes in the tooltip beside the label, not on the face: a status
/// cluster is read at a glance, and chips between the counts would bury them. Show one only when
/// its count is non-zero; the caller zero-suppresses, as with every count (§1.2).
#[derive(IntoElement)]
pub struct StatusButton {
    base: ButtonBase,
    label: SharedString,
    mark: Option<StatusMark>,
    tone: Tone,
    tooltip: Option<SharedString>,
}

impl StatusButton {
    /// A compact ghost status button reading `label` in the secondary tone.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let mut base = ButtonBase::new(id.into(), ButtonStyle::Ghost);
        base.size = ButtonSize::Compact;
        Self {
            base,
            label: label.into(),
            mark: None,
            tone: Tone::Secondary,
            tooltip: None,
        }
    }

    button_builders!();

    /// Lead the label with a dot, a glyph or a spinner.
    pub fn mark(mut self, mark: StatusMark) -> Self {
        self.mark = Some(mark);
        self
    }

    /// Paint the label and its mark in `tone`.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// What the tooltip says the click does, e.g. `Open the agent that needs you`. Without it the
    /// tooltip repeats the label. The key is added beside it either way.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

impl RenderOnce for StatusButton {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kbd = self.base.resolve_kbd(window, cx);
        let theme = cx.theme();
        let paint = self.base.paint(theme);
        let color = self.tone.color(theme);
        let mark = self.mark.map(|mark| match mark {
            StatusMark::Dot => div()
                .flex_none()
                .size(theme.metrics.dot_size_small)
                .rounded(theme.radii.full)
                .bg(color)
                .into_any_element(),
            StatusMark::Icon(icon) => icon
                .el()
                .size(IconSize::Small)
                .color(color)
                .into_any_element(),
            StatusMark::Spinner => Icon::LoaderCircle
                .el()
                .size(IconSize::Small)
                .color(color)
                .spinning(true)
                .id("status-button-spinner")
                .into_any_element(),
        });
        let tooltip =
            Tooltip::new(self.tooltip.unwrap_or_else(|| self.label.clone())).kbd(kbd.clone());
        let label = Text::ui(self.label.clone()).color(color);
        self.base
            .frame(self.label, kbd.as_ref(), &paint, theme)
            .flex_none()
            .px(theme.space.sm)
            .gap(theme.space.xs)
            .children(mark)
            .child(label)
            .with_tooltip(tooltip, cx)
    }
}

/// A ghost button that names the current choice and opens a menu of the others: the title bar's
/// context switcher (`[A] Acme ⌄`), a breadcrumb's worktree (`⎇ agent ⌄`).
///
/// It is always a [`super::PopoverMenu`] trigger with no action of its own; pass the popover's
/// `open` to [`Self::selected`] so an open menu's trigger stays pressed. The trailing chevron says
/// a menu opens, which is why the switcher needs no key chip on its face; what each choice's key
/// is, the menu shows.
#[derive(IntoElement)]
pub struct SwitcherButton {
    base: ButtonBase,
    label: SharedString,
    monogram: bool,
    icon: Option<Icon>,
    tooltip: Option<SharedString>,
}

impl SwitcherButton {
    /// A compact ghost switcher reading `label`.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let mut base = ButtonBase::new(id.into(), ButtonStyle::Ghost);
        base.size = ButtonSize::Compact;
        Self {
            base,
            label: label.into(),
            monogram: false,
            icon: None,
            tooltip: None,
        }
    }

    button_builders!();

    /// Lead the label with a monogram tile: the label's first letter or digit, upper-cased.
    pub fn monogram(mut self) -> Self {
        self.monogram = true;
        self
    }

    /// Lead the label with a glyph instead of a monogram.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// What the tooltip says, e.g. `Switch context`.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

/// The monogram of `label`: its first letter or digit, upper-cased, or nothing for a label with
/// neither.
fn monogram_of(label: &str) -> Option<SharedString> {
    label
        .chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| SharedString::from(character.to_uppercase().collect::<String>()))
}

impl RenderOnce for SwitcherButton {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kbd = self.base.resolve_kbd(window, cx);
        let theme = cx.theme();
        let paint = self.base.paint(theme);
        let monogram = self
            .monogram
            .then(|| monogram_of(&self.label))
            .flatten()
            .map(|letter| {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(theme.metrics.monogram_size)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.accent_subtle)
                    .child(Text::label(letter).color(theme.colors.accent))
            });
        let icon = self
            .icon
            .map(|icon| icon.el().size(IconSize::Small).color(paint.fg));
        let tooltip = self.tooltip.map(|text| Tooltip::new(text).kbd(kbd.clone()));
        let label = Text::ui_strong(self.label.clone())
            .color(theme.colors.text)
            .ellipsize();
        self.base
            .frame(self.label, kbd.as_ref(), &paint, theme)
            .flex_none()
            .min_w_0()
            .px(theme.space.sm)
            .children(monogram)
            .children(icon)
            .child(label)
            .child(
                Icon::ChevronDown
                    .el()
                    .size(IconSize::Small)
                    .color(theme.colors.text_secondary),
            )
            .when_some(tooltip, |el, tooltip| el.with_tooltip(tooltip, cx))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        AppContext, Context, FocusHandle, KeyBinding, Modifiers, Render, VisualTestContext,
        WindowHandle, point, px,
    };

    use super::*;

    gpui::actions!(button_test, [Fire, Unbound]);

    struct Host {
        focus: FocusHandle,
        fired: usize,
        disabled: bool,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context("ButtonTest")
                .track_focus(&self.focus)
                .size_full()
                .on_action(cx.listener(|host, _: &Fire, _, _| host.fired += 1))
                .child(
                    Button::new("fire", "Fire")
                        .action(Box::new(Fire))
                        .disabled(self.disabled),
                )
        }
    }

    fn host(cx: &mut gpui::TestAppContext) -> (WindowHandle<Host>, VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(crate::Theme::dark());
            cx.bind_keys([KeyBinding::new("ctrl-s a", Fire, Some("ButtonTest"))]);
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    Host {
                        focus,
                        fired: 0,
                        disabled: false,
                    }
                })
            })
            .expect("test window")
        });
        let visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        (window, visual)
    }

    #[gpui::test]
    fn clicking_an_action_button_dispatches_the_action(cx: &mut gpui::TestAppContext) {
        let (window, mut cx) = host(cx);
        let view = window.root(&mut cx).expect("test host");
        let on_button = point(px(12.0), px(12.0));

        cx.simulate_mouse_move(on_button, None, Modifiers::none());
        cx.simulate_click(on_button, Modifiers::none());
        cx.run_until_parked();
        view.read_with(&cx, |host, _| assert_eq!(host.fired, 1));

        view.update(&mut cx, |host, cx| {
            host.disabled = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_click(on_button, Modifiers::none());
        cx.run_until_parked();
        view.read_with(&cx, |host, _| {
            assert_eq!(host.fired, 1, "a disabled button does not dispatch")
        });
    }

    #[gpui::test]
    fn the_chip_comes_from_the_live_binding_in_the_focused_context(cx: &mut gpui::TestAppContext) {
        let (_window, mut cx) = host(cx);
        let (bound, unbound) = cx.update(|window, cx| {
            (
                Kbd::for_action(&Fire, window, cx).map(|kbd| kbd.chip_labels()),
                Kbd::for_action(&Unbound, window, cx),
            )
        });
        let expected = Kbd::parse("ctrl-s a")
            .unwrap_or_else(|error| panic!("{error}"))
            .chip_labels();
        assert_eq!(bound, Some(expected));
        assert_eq!(unbound, None, "an unbound action shows no chip");
    }

    #[gpui::test]
    fn key_of_shows_another_actions_key_and_hides_it_where_that_key_is_unbound(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_window, mut cx) = host(cx);
        let (borrowed, dead) = cx.update(|window, cx| {
            let mut borrowed = Button::new("clear", "Clear")
                .action(Box::new(Unbound))
                .key_of(Box::new(Fire));
            let mut dead = Button::new("fire", "Fire")
                .action(Box::new(Fire))
                .key_of(Box::new(Unbound));
            (
                borrowed
                    .base
                    .resolve_kbd(window, cx)
                    .map(|kbd| kbd.chip_labels()),
                dead.base.resolve_kbd(window, cx),
            )
        });
        let expected = Kbd::parse("ctrl-s a")
            .unwrap_or_else(|error| panic!("{error}"))
            .chip_labels();
        assert_eq!(
            borrowed,
            Some(expected),
            "the chip is the borrowed action's key"
        );
        assert_eq!(
            dead, None,
            "a borrowed key that does nothing here shows no chip, whatever the click runs"
        );
    }
}
