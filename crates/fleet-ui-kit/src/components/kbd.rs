//! `Kbd` — a key chip: one small outlined chip per keystroke, spelled with the platform's glyphs.
//!
//! This is how a control shows its key (DESIGN-SYSTEM §4): inside a [`super::Button`], at the
//! right of a menu item, in a [`super::Tooltip`]. The chip is resolved from the live keymap with
//! [`Kbd::for_action`] or [`Kbd::for_action_in`], so what it says is what the key does; a key
//! is never typed into a chip by hand in `fleet-app`. [`Kbd::parse`] exists for the gallery and
//! tests.
//!
//! Use a [`super::KeyHint`] instead only on the two surfaces with no control to carry the key
//! (the terminal exit strip and the scroll pill).

use gpui::{
    Action, App, FocusHandle, InvalidKeystrokeError, KeyBinding, Keystroke, SharedString, Window,
    div, prelude::*,
};

use crate::{text::Text, theme::ActiveTheme};

/// How a chip sits on its ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KbdTone {
    /// `kbd_bg` fill and `kbd_border` outline, on any neutral ground.
    #[default]
    Default,
    /// On an `accent_fill` button: a translucent wash of the button's label colour.
    OnAccent,
    /// On a `danger` fill: the same wash, in the danger button's label colour.
    OnDanger,
    /// A held key that is waiting for the next one: a `warning` wash with `warning` text. Only
    /// the ⌃S command menu's own prefix chip uses it, because the prefix is the one mode that
    /// expires on its own.
    Warning,
}

/// Chip height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KbdSize {
    /// `kbd_h`: beside a default-size button label or in a tooltip.
    #[default]
    Default,
    /// `kbd_h_small`: inside a compact button or a menu item.
    Small,
}

/// Which glyph set a chip is spelled in. Private: the kit follows the platform it runs on, and
/// tests pin one explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spelling {
    /// `⌃ ⌥ ⇧ ⌘` stacked in front of the key: `⌃S`.
    Mac,
    /// Words joined by `+`: `Ctrl+S`.
    Words,
}

impl Spelling {
    fn native() -> Self {
        if cfg!(target_os = "macos") {
            Spelling::Mac
        } else {
            Spelling::Words
        }
    }
}

/// A keystroke sequence as chips: `⌃S` for a chord, `⌃S` `a` or `g` `b` for a sequence.
#[derive(IntoElement, Clone, Debug, PartialEq)]
pub struct Kbd {
    strokes: Vec<Keystroke>,
    tone: KbdTone,
    size: KbdSize,
}

impl Kbd {
    /// Chips for an explicit sequence, one chip per keystroke.
    pub fn new(strokes: &[Keystroke]) -> Self {
        Self {
            strokes: strokes.to_vec(),
            tone: KbdTone::Default,
            size: KbdSize::Default,
        }
    }

    /// Chips for a keymap-syntax sequence such as `"ctrl-s a"`. For the gallery and tests: a
    /// view resolves its chip from the keymap with [`Kbd::for_action`] instead.
    pub fn parse(source: &str) -> Result<Self, InvalidKeystrokeError> {
        let strokes = source
            .split_whitespace()
            .map(Keystroke::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(&strokes))
    }

    /// Chips for the keystrokes of one binding.
    pub fn from_binding(binding: &KeyBinding) -> Self {
        let strokes: Vec<Keystroke> = binding
            .keystrokes()
            .iter()
            .map(|stroke| stroke.inner().clone())
            .collect();
        Self::new(&strokes)
    }

    /// The highest-precedence binding for `action` in the focused element's context, as the
    /// last painted frame laid it out; `None` when nothing is focused or nothing binds the action
    /// there — in which case the control shows no chip.
    ///
    /// Resolved through the focused handle rather than gpui's
    /// `Window::highest_precedence_binding_for_action`: at v1.18.1 that one reads whatever
    /// context stack the dispatch tree was left holding when painting ended, not the focused
    /// element's path, so in a real window it can miss a binding the focused context has.
    pub fn for_action(action: &dyn Action, window: &Window, cx: &App) -> Option<Self> {
        let focused = window.focused(cx)?;
        Self::for_action_in(action, &focused, window)
    }

    /// The binding `action` would have if `focus` held the focus: for a control that belongs to
    /// a pane other than the focused one.
    pub fn for_action_in(
        action: &dyn Action,
        focus: &FocusHandle,
        window: &Window,
    ) -> Option<Self> {
        window
            .highest_precedence_binding_for_action_in(action, focus)
            .map(|binding| Self::from_binding(&binding))
    }

    /// Set how the chip sits on its ground. [`super::Button`] sets this for you.
    pub fn tone(mut self, tone: KbdTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the chip height. [`super::Button`] sets this for you.
    pub fn size(mut self, size: KbdSize) -> Self {
        self.size = size;
        self
    }

    /// The keystrokes this chip shows.
    pub fn strokes(&self) -> &[Keystroke] {
        &self.strokes
    }

    /// The text of each chip, in order, spelled for the platform this build targets.
    pub fn chip_labels(&self) -> Vec<SharedString> {
        self.labels_in(Spelling::native())
    }

    /// The shortcut in WAI-ARIA `aria-keyshortcuts` form (`Control+Shift+Y`), for assistive
    /// technology. `None` for a multi-stroke sequence, which that attribute cannot express: it
    /// reads spaces as alternatives, not as a sequence.
    pub fn aria_shortcut(&self) -> Option<SharedString> {
        let [stroke] = self.strokes.as_slice() else {
            return None;
        };
        let modifiers = stroke.modifiers;
        let mut text = String::new();
        for (on, name) in [
            (modifiers.control, "Control"),
            (modifiers.alt, "Alt"),
            (modifiers.shift, "Shift"),
            (modifiers.platform, "Meta"),
        ] {
            if on {
                text.push_str(name);
                text.push('+');
            }
        }
        let key = match stroke.key.as_str() {
            "enter" => "Enter".to_owned(),
            "escape" => "Escape".to_owned(),
            "tab" => "Tab".to_owned(),
            "space" => "Space".to_owned(),
            "backspace" => "Backspace".to_owned(),
            "delete" => "Delete".to_owned(),
            "up" => "ArrowUp".to_owned(),
            "down" => "ArrowDown".to_owned(),
            "left" => "ArrowLeft".to_owned(),
            "right" => "ArrowRight".to_owned(),
            "home" => "Home".to_owned(),
            "end" => "End".to_owned(),
            "pageup" => "PageUp".to_owned(),
            "pagedown" => "PageDown".to_owned(),
            other => capitalize_key(other),
        };
        text.push_str(&key);
        Some(text.into())
    }

    fn labels_in(&self, spelling: Spelling) -> Vec<SharedString> {
        self.strokes
            .iter()
            .map(|stroke| stroke_label(stroke, spelling))
            .collect()
    }
}

/// One chip's text: the modifiers, then the key.
fn stroke_label(stroke: &Keystroke, spelling: Spelling) -> SharedString {
    let modifiers = stroke.modifiers;
    let mut label = String::new();
    // Apple's order is ⌃ ⌥ ⇧ ⌘; the word spelling keeps the same order.
    let held = [
        (modifiers.function, "fn", "Fn"),
        (modifiers.control, "⌃", "Ctrl"),
        (modifiers.alt, "⌥", "Alt"),
        (modifiers.shift, "⇧", "Shift"),
        (modifiers.platform, "⌘", "Super"),
    ];
    for (on, glyph, word) in held {
        if !on {
            continue;
        }
        match spelling {
            Spelling::Mac => label.push_str(glyph),
            Spelling::Words => {
                label.push_str(word);
                label.push('+');
            }
        }
    }
    match named_key(&stroke.key) {
        Some(name) => label.push_str(name),
        // A letter under a modifier reads as the key cap (`⌃S`); a bare letter stays the
        // letter that is typed (`g`).
        None if modifiers.modified() => label.push_str(&capitalize_key(&stroke.key)),
        None => label.push_str(&stroke.key),
    }
    label.into()
}

/// The glyph or short word for a named key; `None` for a printable key.
fn named_key(key: &str) -> Option<&'static str> {
    Some(match key {
        "enter" => "⏎",
        "escape" => "esc",
        "tab" => "⇥",
        "space" => "␣",
        "backspace" => "⌫",
        "delete" => "del",
        "up" => "↑",
        "down" => "↓",
        "left" => "←",
        "right" => "→",
        "home" => "home",
        "end" => "end",
        "pageup" => "pgup",
        "pagedown" => "pgdn",
        _ => return None,
    })
}

fn capitalize_key(key: &str) -> String {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(single), None) => single.to_uppercase().collect(),
        // Function keys and other named keys gpui knows: `f5` → `F5`.
        (Some(first), Some(_)) if key.len() <= 3 && first == 'f' => key.to_uppercase(),
        _ => key.to_owned(),
    }
}

/// The retiring Help and Palette spelling of a keymap chord: `^s`, `S-⇥`, `⌥⏎`.
///
/// The substitutions are ordered longest-name-first, so `backspace` becomes `⌫` instead of
/// being eaten by the `space` rule; `cmd-` is left spelled out (Fleet's clipboard rows read
/// `cmd-c`). A surface rebuilt under ADR 0023 shows a [`Kbd`] instead; this stays only until the
/// Help overlay, the palette and the prefix toast move over in their own cards.
pub fn pretty_keys(keys: &str) -> String {
    keys.replace("ctrl-", "^")
        .replace("shift-", "S-")
        .replace("alt-", "⌥")
        .replace("backspace", "⌫")
        .replace("escape", "esc")
        .replace("enter", "⏎")
        .replace("tab", "⇥")
        .replace("space", "␣")
}

impl RenderOnce for Kbd {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let wash = theme.metrics.semantic_fill_opacity;
        let (bg, border, fg) = match self.tone {
            KbdTone::Default => (colors.kbd_bg, colors.kbd_border, colors.text_secondary),
            KbdTone::OnAccent => {
                let fg = colors.accent_fill_text;
                (fg.opacity(wash), fg.opacity(wash), fg)
            }
            KbdTone::OnDanger => {
                let fg = colors.text_inverse;
                (fg.opacity(wash), fg.opacity(wash), fg)
            }
            KbdTone::Warning => {
                let fg = colors.warning;
                (fg.opacity(wash), fg.opacity(wash), fg)
            }
        };
        let height = match self.size {
            KbdSize::Default => theme.metrics.kbd_h,
            KbdSize::Small => theme.metrics.kbd_h_small,
        };
        let chip = |label: SharedString| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .h(height)
                .min_w(height)
                .px(theme.space.xs)
                .rounded(theme.radii.sm)
                .bg(bg)
                .border(theme.metrics.hairline)
                .border_color(border)
                .child(Text::hint(label).color(fg))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .children(self.chip_labels().into_iter().map(chip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(source: &str, spelling: Spelling) -> Vec<SharedString> {
        Kbd::parse(source)
            .unwrap_or_else(|error| panic!("{source:?} parses: {error}"))
            .labels_in(spelling)
    }

    #[test]
    fn a_prefixed_sequence_is_two_chips_with_the_modifier_inside_the_first() {
        assert_eq!(labels("ctrl-s a", Spelling::Mac), ["⌃S", "a"]);
        assert_eq!(labels("ctrl-s a", Spelling::Words), ["Ctrl+S", "a"]);
    }

    #[test]
    fn the_build_spells_chips_for_its_own_platform() {
        let native = Kbd::parse("ctrl-s a")
            .unwrap_or_else(|error| panic!("{error}"))
            .chip_labels();
        #[cfg(target_os = "macos")]
        assert_eq!(native, ["⌃S", "a"]);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(native, ["Ctrl+S", "a"]);
    }

    #[test]
    fn modifiers_follow_apples_order_and_named_keys_use_their_glyphs() {
        assert_eq!(labels("cmd-shift-alt-ctrl-k", Spelling::Mac), ["⌃⌥⇧⌘K"]);
        assert_eq!(
            labels("cmd-shift-alt-ctrl-k", Spelling::Words),
            ["Ctrl+Alt+Shift+Super+K"]
        );
        assert_eq!(
            labels("enter escape tab space backspace up down", Spelling::Mac),
            ["⏎", "esc", "⇥", "␣", "⌫", "↑", "↓"]
        );
        assert_eq!(labels("shift-tab", Spelling::Mac), ["⇧⇥"]);
        assert_eq!(
            labels("Y", Spelling::Mac),
            ["⇧Y"],
            "an uppercase key is shift"
        );
        assert_eq!(labels("g b", Spelling::Words), ["g", "b"]);
        assert_eq!(labels("alt-f5", Spelling::Words), ["Alt+F5"]);
    }

    #[test]
    fn assistive_technology_hears_single_strokes_only() {
        let aria = |source: &str| {
            Kbd::parse(source)
                .unwrap_or_else(|error| panic!("{error}"))
                .aria_shortcut()
        };
        assert_eq!(aria("ctrl-shift-y"), Some("Control+Shift+Y".into()));
        assert_eq!(aria("escape"), Some("Escape".into()));
        assert_eq!(aria("ctrl-s a"), None, "a sequence is not an ARIA shortcut");
        assert!(
            Kbd::parse("ctrl-s a-b").is_err(),
            "a bad stroke fails the parse"
        );
    }

    #[test]
    fn the_retiring_spelling_is_unchanged() {
        assert_eq!(pretty_keys("ctrl-s shift-tab alt-enter"), "^s S-⇥ ⌥⏎");
        assert_eq!(
            pretty_keys("backspace"),
            "⌫",
            "the longer name wins over `space`"
        );
        assert_eq!(pretty_keys("space"), "␣");
        assert_eq!(pretty_keys("cmd-c"), "cmd-c");
    }
}
