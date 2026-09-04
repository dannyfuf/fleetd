//! `FreshnessStamp` — `checked 14s ago · I re-check`, with the §2.6 contrast ladder.
//!
//! A fact without an age is not a fact. Every job-derived value (inspect, prune dry-run, PR
//! fetch) carries one of these, and the confirm dialogs are the one place where the age
//! actually decides an outcome.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::{KeyHint, age_label::format_age},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The four rungs of the freshness ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// <= 60 s. Normal contrast.
    Fresh,
    /// 60 s - 10 min. Secondary contrast.
    Aging,
    /// > 10 min. Amber; derived marks elsewhere drop to 55 %.
    Stale,
    /// The producing job errored. Red; the value is not drawn at all.
    Errored,
}

impl Freshness {
    /// Classify an age in seconds.
    pub fn from_secs(seconds: i64) -> Self {
        match seconds {
            s if s <= 60 => Freshness::Fresh,
            s if s <= 600 => Freshness::Aging,
            _ => Freshness::Stale,
        }
    }

    /// The tone for this rung.
    pub fn tone(self) -> Tone {
        match self {
            Freshness::Fresh => Tone::Secondary,
            Freshness::Aging => Tone::Secondary,
            Freshness::Stale => Tone::Warning,
            Freshness::Errored => Tone::Danger,
        }
    }

    /// The opacity a mark derived from a fact of this age must render at.
    pub fn derived_opacity(self) -> f32 {
        match self {
            Freshness::Stale => 0.55,
            _ => 1.0,
        }
    }
}

/// `<verb> <age> ago · <key> <action>`
#[derive(IntoElement)]
pub struct FreshnessStamp {
    verb: SharedString,
    age_secs: i64,
    freshness: Freshness,
    action: Option<(SharedString, SharedString)>,
    error: Option<SharedString>,
}

impl FreshnessStamp {
    /// `checked`, `fetched`, `inspected`, `dry run`.
    pub fn new(verb: impl Into<SharedString>, age_secs: i64) -> Self {
        Self {
            verb: verb.into(),
            age_secs,
            freshness: Freshness::from_secs(age_secs),
            action: None,
            error: None,
        }
    }

    /// The re-check affordance, e.g. `("I", "re-check")`.
    pub fn action(mut self, key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        self.action = Some((key.into(), label.into()));
        self
    }

    /// Mark the producing job as errored: the stamp turns red and shows the message verbatim.
    pub fn error(mut self, message: impl Into<SharedString>) -> Self {
        self.error = Some(message.into());
        self.freshness = Freshness::Errored;
        self
    }

    /// The rung this stamp resolved to.
    pub fn freshness(&self) -> Freshness {
        self.freshness
    }
}

impl RenderOnce for FreshnessStamp {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = self.freshness.tone();
        let body = match &self.error {
            Some(message) => SharedString::from(format!("error: {message}")),
            None => SharedString::from(format!(
                "{} {} ago",
                self.verb,
                format_age(self.age_secs)
            )),
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .child(Text::hint(body).tone(tone))
            .children(
                self.action
                    .map(|(key, label)| KeyHint::labeled(key, label)),
            )
    }
}
