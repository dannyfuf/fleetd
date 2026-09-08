//! `KeyHint` — mono 11 key plus its action, in the lowest contrast the theme has.
//!
//! Every affordance in Fleet is a key, so this is the most repeated component in the app.
//! Inside the Workspace the key must carry its prefix (`^s r`, never `r`) — §3.6 [D-8]; the
//! component does not enforce that, the caller passes the full chord.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// `⏎ run`, `esc cancel`, `^s x close`.
#[derive(IntoElement)]
pub struct KeyHint {
    keys: SharedString,
    label: Option<SharedString>,
    tone: Tone,
    key_tone: Tone,
}

impl KeyHint {
    /// A bare key with no label (palette rows right-align these).
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self {
            keys: keys.into(),
            label: None,
            tone: Tone::Muted,
            key_tone: Tone::Muted,
        }
    }

    /// A key and what it does.
    pub fn labeled(keys: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self::new(keys).label(label)
    }

    /// Set the action label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Tone of the action label.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Tone of the key itself. Raise it to `Default` for the primary action of a dialog footer.
    pub fn key_tone(mut self, tone: Tone) -> Self {
        self.key_tone = tone;
        self
    }
}

impl RenderOnce for KeyHint {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let gap = cx.theme().space.xs;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(gap)
            .child(Text::hint(self.keys).tone(self.key_tone))
            .children(self.label.map(|label| Text::hint(label).tone(self.tone)))
    }
}

/// A row of hints separated by `·`, the shape every dialog footer and empty state uses.
#[derive(IntoElement)]
pub struct KeyHintRow {
    hints: Vec<KeyHint>,
}

impl KeyHintRow {
    /// An empty row.
    pub fn new() -> Self {
        Self { hints: Vec::new() }
    }

    /// Append a hint.
    pub fn hint(mut self, hint: KeyHint) -> Self {
        self.hints.push(hint);
        self
    }

    /// Append `keys label`.
    pub fn key(self, keys: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        self.hint(KeyHint::labeled(keys, label))
    }

    /// Append every hint in `other`, preserving both rows' order.
    pub fn merge(mut self, other: KeyHintRow) -> Self {
        self.hints.extend(other.hints);
        self
    }

    /// The `(keys, label)` pairs this row will draw, in order.
    ///
    /// Two surfaces advertise the same set — a status bar mirrors a card or a popup header
    /// (`NATIVE-AGENTS.md` §9) — and the only way to hold them to it is to compare the rows
    /// rather than the code that built them.
    #[must_use]
    pub fn pairs(&self) -> Vec<(SharedString, Option<SharedString>)> {
        self.hints
            .iter()
            .map(|hint| (hint.keys.clone(), hint.label.clone()))
            .collect()
    }
}

impl Default for KeyHintRow {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for KeyHintRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let gap = cx.theme().space.sm;
        let last = self.hints.len().saturating_sub(1);
        div().flex().items_center().gap(gap).children(
            self.hints
                .into_iter()
                .enumerate()
                .map(|(ix, hint)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(gap)
                        .child(hint)
                        .when(ix != last, |el| el.child(Text::hint("\u{b7}").faint()))
                })
                .collect::<Vec<_>>(),
        )
    }
}
