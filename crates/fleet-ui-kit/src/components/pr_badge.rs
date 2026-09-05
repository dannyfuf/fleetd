//! `PrBadge` — `#n` plus one icon and one word (max 8 characters).
//!
//! The badge collapses three raw GitHub fields into one glance. The priority order is fixed
//! (§3.5): draft, ci_fail, changes, ci_pending, approved, review; a merged PR overrides all of
//! them. The caller resolves the priority; this component only renders the chosen state, so
//! the kit stays free of GitHub types.

use gpui::{App, Window, div, prelude::*};

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
}

/// `#412  CI fail`
#[derive(IntoElement)]
pub struct PrBadge {
    number: Option<u64>,
    state: PrBadgeState,
    stale: bool,
}

impl PrBadge {
    /// A badge for a numbered PR.
    pub fn new(number: u64, state: PrBadgeState) -> Self {
        Self {
            number: Some(number),
            state,
            stale: false,
        }
    }

    /// A badge with no number, for the Workspace header and detail panels.
    pub fn state_only(state: PrBadgeState) -> Self {
        Self {
            number: None,
            state,
            stale: false,
        }
    }

    /// Drop to 55 % — the §2.6 rendering for a derived mark older than 10 minutes.
    pub fn stale(mut self, stale: bool) -> Self {
        self.stale = stale;
        self
    }
}

impl RenderOnce for PrBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let color = self.state.tone().color(theme);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .when(self.stale, |el| el.opacity(0.55))
            .children(
                self.number
                    .map(|number| Text::data(format!("#{number}")).muted()),
            )
            .child(self.state.icon().el().size(IconSize::Medium).color(color))
            .child(Text::ui(self.state.word()).color(color))
    }
}
