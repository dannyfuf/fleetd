//! `TextField` — a single-line input with a blue caret and a zero-shift validation line.
//!
//! The module has three layers, and a caller picks exactly one:
//!
//! 1. [`TextField`] — the presentational `RenderOnce` surface. The caller owns the string and
//!    the caret index and passes both down every frame. This is what dialogs that already own
//!    their state (Create, Clone, Context) compose.
//! 2. [`TextFieldState`] — the editing model: an owned string, a byte cursor and the KEYMAP
//!    §3.8 edit set (printable, `Backspace`, `Delete`, `ctrl-w`, `ctrl-u`, `ctrl-k`, `ctrl-a`,
//!    `ctrl-e`, `←` / `→`, `Home` / `End`). It is pure — no gpui element, no theme — so it is
//!    unit-testable and reusable by [`super::FilterBar`] and [`super::Palette`] callers, which
//!    render their own chrome.
//! 3. [`TextInput`] — a gpui entity that owns a [`TextFieldState`], paints a real shaped line
//!    with a caret through a custom element, and installs an [`gpui::ElementInputHandler`] so
//!    dead keys and IME composition (marked text is underlined) work like a native field.
//!
//! The one rule the surface *enforces* is §3.8.1's: the validation line **replaces** the
//! preview line in the same 18 px slot, so a failing branch name causes zero layout shift.

use gpui::{App, Pixels, SharedString, Window, prelude::*};

use crate::{icons::Icon, theme::ActiveTheme};

mod chrome;
mod element;
mod input;
mod state;

use chrome::FieldChrome;
pub(crate) use element::FieldLine;
use element::TextInputElement;
pub use input::{TEXT_FIELD_KEY_CONTEXT, TextInput, TextInputEvent};
pub use state::{EditEffect, TextFieldState};

fn char_offset(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index)
}

/// A single-line text input. The caller owns the string, the caret and the keys.
#[derive(IntoElement)]
pub struct TextField {
    value: SharedString,
    placeholder: Option<SharedString>,
    caret: Option<usize>,
    focused: bool,
    icon: Option<Icon>,
    prefix: Option<SharedString>,
    preview: Option<SharedString>,
    invalid: Option<SharedString>,
    mono: bool,
    label: Option<SharedString>,
    height: Option<Pixels>,
    hide_status_line: bool,
}

impl TextField {
    /// A field showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            placeholder: None,
            caret: None,
            focused: false,
            icon: None,
            prefix: None,
            preview: None,
            invalid: None,
            mono: false,
            label: None,
            height: None,
            hide_status_line: false,
        }
    }

    /// A label above the box.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Text shown when the value is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Caret position in characters. Drawn only while focused.
    pub fn caret(mut self, caret: usize) -> Self {
        self.caret = Some(caret);
        self
    }

    /// Whether the field has keyboard focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// A leading glyph, e.g. `search` in the Clone dialog.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A literal leading character in the data face, e.g. the palette's `:`.
    ///
    /// Use it instead of [`Self::icon`] where the mark is the **key the user pressed**: the
    /// palette opens on `:`, and a `command` glyph there advertises `⌘`, a key Fleet does not
    /// bind.
    pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// The faint derived preview under the box (`→ buk/payroll#feat-rut-validator`).
    pub fn preview(mut self, preview: impl Into<SharedString>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    /// The exact failing rule. **Replaces** the preview in the same slot.
    pub fn invalid(mut self, message: impl Into<SharedString>) -> Self {
        self.invalid = Some(message.into());
        self
    }

    /// Render the value in the data face (branches, paths).
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Override the 36 px box height. The palette's 44 px input is the only caller.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Drop the 18 px preview / validation slot.
    ///
    /// Only for a surface that can carry **neither** a preview nor a validation message — the
    /// palette's query line. Everywhere else the slot must stay, because that is what makes a
    /// preview turning into an error cost zero layout shift.
    pub fn hide_status_line(mut self, hide: bool) -> Self {
        self.hide_status_line = hide;
        self
    }

    /// The chrome this field draws around its value.
    fn chrome(&self) -> FieldChrome {
        FieldChrome {
            label: self.label.clone(),
            icon: self.icon,
            prefix: self.prefix.clone(),
            preview: self.preview.clone(),
            invalid: self.invalid.clone(),
            focused: self.focused,
            mono: self.mono,
            height: self.height,
            hide_status_line: self.hide_status_line,
        }
    }
}

impl RenderOnce for TextField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let chrome = self.chrome();
        let value = FieldLine::new(self.value, self.placeholder, self.caret, self.focused);
        chrome.wrap(cx.theme(), value)
    }
}

#[cfg(test)]
mod tests;
