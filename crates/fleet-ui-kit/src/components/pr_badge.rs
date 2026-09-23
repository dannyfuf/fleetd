//! `PrBadge` — `#n` plus one icon and one word (max 8 characters).
//!
//! The badge collapses three raw GitHub fields into one glance. The priority order is fixed
//! (§3.5): draft, ci_fail, changes, ci_pending, approved, review; a merged PR overrides all of
//! them. The caller resolves the priority; this component only renders the chosen state, so
//! the kit stays free of GitHub types.
//!
//! [`PrBadge::chip`] draws the same state as a tinted pill with the longer sentence word
//! (`#4 In review`, `#9 CI failing`): the comfortable Hub rows and the detail panel, where the
//! badge sits beside prose rather than in a dense column.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The seven renderings a PR can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrBadgeState {
    /// `git-pull-request-draft`, faint, `Draft`.
    Draft,
    /// `circle-x`, red, `CI fail`.
    CiFail,
    /// `message-square-warning`, amber, `Changes`.
    Changes,
    /// `clock`, amber, `CI ···`.
    CiPending,
    /// `circle-check`, green, `Approved`.
    Approved,
    /// `eye`, secondary, `Review`.
    Review,
    /// `git-merge`, green, `Merged`.
    Merged,
}

impl PrBadgeState {
    /// The glyph.
    pub fn icon(self) -> Icon {
        match self {
            PrBadgeState::Draft => Icon::GitPullRequestDraft,
            PrBadgeState::CiFail => Icon::CircleX,
            PrBadgeState::Changes => Icon::MessageSquareWarning,
            PrBadgeState::CiPending => Icon::Clock,
            PrBadgeState::Approved => Icon::CircleCheck,
            PrBadgeState::Review => Icon::Eye,
            PrBadgeState::Merged => Icon::GitMerge,
        }
    }

    /// The tone.
    pub fn tone(self) -> Tone {
        match self {
            PrBadgeState::Draft => Tone::Muted,
            PrBadgeState::CiFail => Tone::Danger,
            PrBadgeState::Changes | PrBadgeState::CiPending => Tone::Warning,
            PrBadgeState::Approved | PrBadgeState::Merged => Tone::Success,
            PrBadgeState::Review => Tone::Secondary,
        }
    }

    /// The word, at most 8 characters.
    pub fn word(self) -> &'static str {
        match self {
            PrBadgeState::Draft => "Draft",
            PrBadgeState::CiFail => "CI fail",
            PrBadgeState::Changes => "Changes",
            PrBadgeState::CiPending => "CI \u{b7}\u{b7}\u{b7}",
            PrBadgeState::Approved => "Approved",
            PrBadgeState::Review => "Review",
            PrBadgeState::Merged => "Merged",
        }
    }

    /// The word a [`PrBadge::chip`] reads: a short phrase, not the ≤ 8-character column word.
    pub fn chip_word(self) -> &'static str {
        match self {
            PrBadgeState::Draft => "Draft",
            PrBadgeState::CiFail => "CI failing",
            PrBadgeState::Changes => "Needs changes",
            PrBadgeState::CiPending => "CI running",
            PrBadgeState::Approved => "Approved",
            PrBadgeState::Review => "In review",
            PrBadgeState::Merged => "Merged",
        }
    }

    /// The tone a [`PrBadge::chip`] tints with. A PR waiting for review is on its way, so the
    /// chip reads it as information (`Accent`) rather than the column's quiet `Secondary`.
    pub fn chip_tone(self) -> Tone {
        match self {
            PrBadgeState::Review => Tone::Accent,
            other => other.tone(),
        }
    }
}

/// `#412  CI fail`
#[derive(IntoElement)]
pub struct PrBadge {
    number: Option<u64>,
    state: PrBadgeState,
    stale: bool,
    chip: bool,
}

impl PrBadge {
    /// A badge for a numbered PR.
    pub fn new(number: u64, state: PrBadgeState) -> Self {
        Self {
            number: Some(number),
            state,
            stale: false,
            chip: false,
        }
    }

    /// A badge with no number, for the Workspace header and detail panels.
    pub fn state_only(state: PrBadgeState) -> Self {
        Self {
            number: None,
            state,
            stale: false,
            chip: false,
        }
    }

    /// Drop to 55 % — the §2.6 rendering for a derived mark older than 10 minutes.
    pub fn stale(mut self, stale: bool) -> Self {
        self.stale = stale;
        self
    }

    /// Draw as a tinted pill reading `#n` and [`PrBadgeState::chip_word`], for a comfortable
    /// row or a detail panel. The bare badge stays the dense-column form.
    pub fn chip(mut self) -> Self {
        self.chip = true;
        self
    }
}

impl RenderOnce for PrBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        if self.chip {
            let tone = self.state.chip_tone();
            let color = tone.color(theme);
            let word = self.state.chip_word();
            let label = match self.number {
                Some(number) => SharedString::from(format!("#{number} {word}")),
                None => SharedString::new_static(word),
            };
            return div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.xxs)
                .h(theme.metrics.chip_h)
                .px(theme.space.sm)
                .rounded(theme.radii.pill)
                .bg(tone.fill(theme))
                .when(self.stale, |el| el.opacity(theme.metrics.stale_opacity))
                .child(self.state.icon().el().size(IconSize::Small).color(color))
                .child(Text::caption(label).color(color))
                .into_any_element();
        }
        let color = self.state.tone().color(theme);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .when(self.stale, |el| el.opacity(theme.metrics.stale_opacity))
            .children(
                self.number
                    .map(|number| Text::data(format!("#{number}")).muted()),
            )
            .child(self.state.icon().el().size(IconSize::Medium).color(color))
            .child(Text::ui(self.state.word()).color(color))
            .into_any_element()
    }
}
