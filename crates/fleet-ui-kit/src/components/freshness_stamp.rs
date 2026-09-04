//! `FreshnessStamp` — `checked 14s ago · I re-check`, with the §2.6 contrast ladder.
//!
//! A fact without an age is not a fact. Every job-derived value (inspect, prune dry-run, PR
//! fetch) carries one of these, and the confirm dialogs are the one place where the age
//! actually decides an outcome.
//!
//! The ladder, verbatim from §2.6:
//!
//! | age | stamp | marks derived from the fact |
//! | --- | --- | --- |
//! | ≤ 60 s | normal contrast | full opacity |
//! | ≤ 10 min | secondary | full opacity |
//! | > 10 min | **amber** | 55 % opacity |
//! | errored | **red**, `error: <message>` verbatim | not drawn at all |

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};

use crate::{
    components::{KeyHint, Spinner, age_label::format_age},
    icons::IconSize,
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The upper bound of [`Freshness::Fresh`], in seconds.
pub const FRESH_SECS: i64 = 60;

/// The upper bound of [`Freshness::Aging`], in seconds.
pub const AGING_SECS: i64 = 600;

/// The opacity a mark derived from a stale fact drops to (§2.6).
pub const STALE_OPACITY: f32 = 0.55;

/// The four rungs of the freshness ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// <= 60 s. Normal contrast.
    Fresh,
    /// 60 s - 10 min. Secondary contrast.
    Aging,
    /// > 10 min. Amber; derived marks elsewhere drop to 55 %.
    Stale,
    /// The producing job errored. Red; the derived mark is not drawn at all.
    Errored,
}

impl Freshness {
    /// Classify an age in seconds.
    pub fn from_secs(seconds: i64) -> Self {
        match seconds {
            s if s <= FRESH_SECS => Freshness::Fresh,
            s if s <= AGING_SECS => Freshness::Aging,
            _ => Freshness::Stale,
        }
    }

    /// The tone for this rung.
    pub fn tone(self) -> Tone {
        match self {
            Freshness::Fresh => Tone::Default,
            Freshness::Aging => Tone::Secondary,
            Freshness::Stale => Tone::Warning,
            Freshness::Errored => Tone::Danger,
        }
    }

    /// The opacity a mark derived from a fact of this age must render at.
    ///
    /// Pass it to [`super::PrBadge::stale`] or to `Text::opacity` for the `✎` dirty mark: the
    /// mark fades, the stamp turns amber, and the two together say "this was true ten minutes
    /// ago" without spending a word on it.
    pub fn derived_opacity(self) -> f32 {
        match self {
            Freshness::Stale => STALE_OPACITY,
            _ => 1.0,
        }
    }

    /// Whether a mark derived from a fact of this age may be drawn at all. An errored job has
    /// no fact, so it has no mark (§2.6) — the error goes in the stamp instead.
    pub fn draws_derived_mark(self) -> bool {
        !matches!(self, Freshness::Errored)
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
    refreshing: Option<ElementId>,
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
            refreshing: None,
        }
    }

    /// The re-check affordance, e.g. `("I", "re-check")`.
    pub fn action(mut self, key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        self.action = Some((key.into(), label.into()));
        self
    }

    /// Mark the producing job as errored: the stamp turns red and shows the message verbatim.
    ///
    /// Warnings from the daemon are greppable diagnostics; they are never paraphrased and never
    /// summarised into "something went wrong".
    pub fn error(mut self, message: impl Into<SharedString>) -> Self {
        self.error = Some(message.into());
        self.freshness = Freshness::Errored;
        self
    }

    /// A refresh is in flight: prefix a spinner, and keep the previous age visible.
    ///
    /// §3.5 "loading with cache": cached values never blank while a fetch runs — the stamp says
    /// `⟳ fetched 4m ago`, so the user knows both what they are looking at and that it is being
    /// replaced. The id must be stable across frames or the spin restarts every frame.
    pub fn refreshing(mut self, id: impl Into<ElementId>) -> Self {
        self.refreshing = Some(id.into());
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
            None => SharedString::from(format!("{} {} ago", self.verb, format_age(self.age_secs))),
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .children(
                self.refreshing
                    .map(|id| Spinner::new(id).size(IconSize::Small)),
            )
            .child(Text::hint(body).tone(tone))
            .children(self.action.map(|(key, label)| KeyHint::labeled(key, label)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_boundaries_match_the_spec() {
        assert_eq!(Freshness::from_secs(0), Freshness::Fresh);
        assert_eq!(Freshness::from_secs(60), Freshness::Fresh);
        assert_eq!(Freshness::from_secs(61), Freshness::Aging);
        assert_eq!(Freshness::from_secs(600), Freshness::Aging);
        assert_eq!(Freshness::from_secs(601), Freshness::Stale);
    }

    #[test]
    fn only_stale_fades_derived_marks() {
        assert_eq!(Freshness::Fresh.derived_opacity(), 1.0);
        assert_eq!(Freshness::Aging.derived_opacity(), 1.0);
        assert_eq!(Freshness::Stale.derived_opacity(), STALE_OPACITY);
        assert!(Freshness::Stale.draws_derived_mark());
        assert!(!Freshness::Errored.draws_derived_mark());
    }

    #[test]
    fn an_error_forces_the_errored_rung() {
        let stamp = FreshnessStamp::new("checked", 3).error("gh: HTTP 502");
        assert_eq!(stamp.freshness(), Freshness::Errored);
        assert_eq!(stamp.freshness().tone(), Tone::Danger);
    }
}
