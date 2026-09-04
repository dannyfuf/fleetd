//! `ConfirmDialog` — compact or expanded, decided by the facts.
//!
//! §1.7 made literal, and §3.8.3's escalation rule [D-10] enforced in one place:
//!
//! * every decisive fact known and benign -> the **compact** form, 480 px, one line of facts,
//!   `y` confirms and `Enter` is accepted;
//! * any risk true, or any decisive fact unknown -> the **expanded** form, 560 px, one line
//!   per fact, and `Y` is required with `Enter` **not** accepted.
//!
//! The freshness stamp is mandatory on every facts confirm: this is the one surface where the
//! age of a fact decides an outcome. There is no "don't ask again" — the compact form is the
//! real answer to confirm fatigue.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{ConfirmKey, Dialog, FactList, FreshnessStamp, KeyHintRow},
    icons::Icon,
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A destructive confirm.
#[derive(IntoElement)]
pub struct ConfirmDialog {
    title: SharedString,
    target: Option<SharedString>,
    facts: FactList,
    consequence: Option<SharedString>,
    stamp: Option<FreshnessStamp>,
    icon: Option<Icon>,
    extra_hints: Option<KeyHintRow>,
    action_label: SharedString,
    width_override: Option<Pixels>,
}

impl ConfirmDialog {
    /// A confirm with a title and its facts.
    pub fn new(title: impl Into<SharedString>, facts: FactList) -> Self {
        Self {
            title: title.into(),
            target: None,
            facts,
            consequence: None,
            stamp: None,
            icon: None,
            extra_hints: None,
            action_label: SharedString::new_static("Delete"),
            width_override: None,
        }
    }

    /// The full id of what is being acted on. Shown on its own line in the expanded form.
    pub fn target(mut self, target: impl Into<SharedString>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// The consequence sentence, in plain future tense. Users confirm the sentence.
    pub fn consequence(mut self, consequence: impl Into<SharedString>) -> Self {
        self.consequence = Some(consequence.into());
        self
    }

    /// The mandatory freshness stamp.
    pub fn stamp(mut self, stamp: FreshnessStamp) -> Self {
        self.stamp = Some(stamp);
        self
    }

    /// The header glyph: `trash`, `scissors`, `power`, `x`, or `triangle-alert` when expanded.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Extra footer keys, e.g. `I re-check` or `s toggle the KEEP list`.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.extra_hints = Some(hints);
        self
    }

    /// The verb on the primary action, e.g. `Delete`, `Prune 3`, `Stop and quit`.
    pub fn action_label(mut self, label: impl Into<SharedString>) -> Self {
        self.action_label = label.into();
        self
    }

    /// Override the width. Defaults to 480 (compact) / 560 (expanded).
    pub fn width(mut self, width: Pixels) -> Self {
        self.width_override = Some(width);
        self
    }

    /// Whether this confirm renders in its compact form.
    pub fn is_compact(&self) -> bool {
        self.facts.is_compact()
    }

    /// The key this confirm binds.
    pub fn confirm_key(&self) -> ConfirmKey {
        self.facts.confirm_key()
    }
}

impl RenderOnce for ConfirmDialog {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let compact = self.is_compact();
        let key = self.confirm_key();
        let width = self
            .width_override
            .unwrap_or(if compact { px(480.0) } else { px(560.0) });
        let icon = self.icon.unwrap_or(if compact {
            Icon::Trash
        } else {
            Icon::TriangleAlert
        });

        let mut hints = KeyHintRow::new();
        if let Some(extra) = self.extra_hints {
            hints = extra;
        }
        hints = hints.key("n", "cancel");

        let body = div()
            .flex()
            .flex_col()
            .gap(theme.space.sm)
            .children(
                self.target
                    .filter(|_| !compact)
                    .map(|target| Text::data(target)),
            )
            .child(self.facts)
            .children(self.stamp)
            .children(
                self.consequence
                    .map(|line| Text::ui(line).muted()),
            );

        Dialog::new(self.title)
            .icon(icon)
            .width(width)
            .tone(if compact { Tone::Default } else { Tone::Warning })
            .body(body)
            .hints(hints)
            .primary(format!("{}  {}", key.label(), self.action_label))
    }
}
