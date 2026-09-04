//! `FactList` — risks first, safe facts after, and the decision of which confirm to show.
//!
//! §3.8.3 [D-10]: `y` confirms when every decisive fact is known; `Y` (shift) is required when
//! any is unknown or the inspection errored. The list also decides compact vs. expanded, so
//! the confirm dialog does not have to.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// One line of a confirm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    /// The sentence, stated as a fact and never as an adjective.
    pub text: SharedString,
    /// True when this fact is a reason to stop.
    pub risk: bool,
    /// True when the fact could not be determined. Forces the `Y` key.
    pub unknown: bool,
}

impl Fact {
    /// A safe fact: `✓ clean`.
    pub fn safe(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            risk: false,
            unknown: false,
        }
    }

    /// A risk: `⚠ 12 uncommitted files`.
    pub fn risk(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            risk: true,
            unknown: false,
        }
    }

    /// An unknown decisive fact, carrying the daemon's warning string verbatim.
    pub fn unknown(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            risk: true,
            unknown: true,
        }
    }

    /// The glyph for this fact.
    pub fn icon(&self) -> Icon {
        if self.risk {
            Icon::TriangleAlert
        } else {
            Icon::CircleCheck
        }
    }

    /// The tone for this fact.
    pub fn tone(&self) -> Tone {
        if self.risk {
            Tone::Warning
        } else {
            Tone::Success
        }
    }
}

/// Which key the confirm binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKey {
    /// `y`, and `Enter` is accepted too.
    Lower,
    /// `Y`. `Enter` is **not** accepted.
    Upper,
}

impl ConfirmKey {
    /// The literal key to print in the footer.
    pub fn label(self) -> &'static str {
        match self {
            ConfirmKey::Lower => "y",
            ConfirmKey::Upper => "Y",
        }
    }

    /// Whether `Enter` also confirms.
    pub fn accepts_enter(self) -> bool {
        matches!(self, ConfirmKey::Lower)
    }
}

/// An ordered set of facts: risks first, then safe facts.
#[derive(Clone, Debug, Default, IntoElement)]
pub struct FactList {
    facts: Vec<Fact>,
    loading: bool,
}

impl FactList {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// From an unordered set. Order is normalised on render, never by the caller.
    pub fn from_facts(facts: impl IntoIterator<Item = Fact>) -> Self {
        Self {
            facts: facts.into_iter().collect(),
            loading: false,
        }
    }

    /// Append one fact.
    pub fn fact(mut self, fact: Fact) -> Self {
        self.facts.push(fact);
        self
    }

    /// The dry run producing these facts is still running: values render as `…` and the confirm
    /// key escalates.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Risks first, safe facts after; stable within each group.
    pub fn ordered(&self) -> Vec<&Fact> {
        let mut out: Vec<&Fact> = self.facts.iter().filter(|f| f.risk).collect();
        out.extend(self.facts.iter().filter(|f| !f.risk));
        out
    }

    /// True when every fact is known and benign, so the compact confirm applies.
    pub fn is_compact(&self) -> bool {
        !self.loading && self.facts.iter().all(|f| !f.risk && !f.unknown)
    }

    /// `Y` when any decisive fact is unknown or the facts are still loading, else `y`.
    pub fn confirm_key(&self) -> ConfirmKey {
        if self.loading || self.facts.iter().any(|f| f.unknown) {
            ConfirmKey::Upper
        } else {
            ConfirmKey::Lower
        }
    }

    /// How many facts there are.
    pub fn len(&self) -> usize {
        self.facts.len()
    }

    /// Whether there are no facts at all.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
}

impl RenderOnce for FactList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let gap = theme.space.xs;
        let rows: Vec<_> = self
            .ordered()
            .into_iter()
            .map(|fact| {
                let color = fact.tone().color(theme);
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(fact.icon().el().size(IconSize::Medium).color(color))
                    .child(Text::ui(fact.text.clone()))
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap(gap)
            .when(self.loading, |el| {
                el.child(Text::ui("\u{2026}").faint())
            })
            .children(rows)
    }
}
