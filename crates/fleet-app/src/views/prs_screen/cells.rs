//! The words a pull request's row and detail read: its review state and its checks.
//!
//! Both are derived once, where the rows are prepared (`build_rows`), so a frame only copies
//! them into elements. The row's state chip is the **review** state alone; checks have a column
//! of their own, so a failing check no longer hides an approval behind `CI fail` (UX-SPEC §3.5).

use fleet_core::github::{PrChecks, PrReviewDecision};
use fleet_ui_kit::{Icon, PrBadgeState, Tone};

/// Where the review stands: the row's state chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    /// Not ready for review yet.
    Draft,
    /// A reviewer asked for changes.
    ChangesRequested,
    /// Approved.
    Approved,
    /// Open and waiting for (or under) review.
    InReview,
}

impl ReviewState {
    /// Draft first — a draft is not asking anyone for anything — then the review decision.
    #[must_use]
    pub fn of(is_draft: bool, decision: PrReviewDecision) -> Self {
        if is_draft {
            return Self::Draft;
        }
        match decision {
            PrReviewDecision::ChangesRequested => Self::ChangesRequested,
            PrReviewDecision::Approved => Self::Approved,
            PrReviewDecision::ReviewRequired | PrReviewDecision::None => Self::InReview,
        }
    }

    /// The kit state the chip draws: its word, glyph and tone come from [`PrBadgeState`], so
    /// this chip and the worktrees page's PR chip cannot disagree.
    #[must_use]
    pub fn badge(self) -> PrBadgeState {
        match self {
            Self::Draft => PrBadgeState::Draft,
            Self::ChangesRequested => PrBadgeState::Changes,
            Self::Approved => PrBadgeState::Approved,
            Self::InReview => PrBadgeState::Review,
        }
    }

    /// The detail panel's `Review` fact.
    #[must_use]
    pub fn sentence(decision: PrReviewDecision) -> &'static str {
        match decision {
            PrReviewDecision::Approved => "Approved",
            PrReviewDecision::ChangesRequested => "Changes requested",
            PrReviewDecision::ReviewRequired => "Waiting for review",
            PrReviewDecision::None => "No review yet",
        }
    }
}

/// How the checks column draws its cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksKind {
    /// Every check passed.
    Passing,
    /// At least one failed.
    Failing,
    /// Still running: the cell spins.
    Running,
    /// The PR has no checks at all.
    None,
}

/// The prepared checks cell: `6 / 6`, `1 failing`, `running`, or a faint `—`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    /// Which of the four the cell draws.
    pub kind: ChecksKind,
    /// The cell's word, short enough for its 10 ch column.
    pub label: String,
    /// The detail panel's sentence (`All 6 passing`, `1 of 6 failing`).
    pub sentence: String,
}

impl Checks {
    /// Words for GitHub's rolled-up check state and, when it reported them, its counts.
    #[must_use]
    pub fn of(checks: PrChecks, passed: Option<u32>, total: Option<u32>) -> Self {
        let counts = passed.zip(total).filter(|(_, total)| *total > 0);
        match checks {
            PrChecks::Pass => Self {
                kind: ChecksKind::Passing,
                label: counts.map_or_else(
                    || "passing".to_owned(),
                    |(passed, total)| format!("{passed} / {total}"),
                ),
                sentence: counts.map_or_else(
                    || "Passing".to_owned(),
                    |(_, total)| format!("All {total} passing"),
                ),
            },
            PrChecks::Fail => {
                let failing = counts
                    .map(|(passed, total)| (total.saturating_sub(passed), total))
                    .filter(|(failing, _)| *failing > 0);
                Self {
                    kind: ChecksKind::Failing,
                    label: failing.map_or_else(
                        || "failing".to_owned(),
                        |(failing, _)| format!("{failing} failing"),
                    ),
                    sentence: failing.map_or_else(
                        || "Failing".to_owned(),
                        |(failing, total)| format!("{failing} of {total} failing"),
                    ),
                }
            }
            PrChecks::Pending => Self {
                kind: ChecksKind::Running,
                label: "running".to_owned(),
                sentence: counts.map_or_else(
                    || "Running".to_owned(),
                    |(passed, total)| format!("Running · {passed} of {total} passed"),
                ),
            },
            PrChecks::None => Self {
                kind: ChecksKind::None,
                label: "\u{2014}".to_owned(),
                sentence: "No checks".to_owned(),
            },
        }
    }

    /// The cell's tone.
    #[must_use]
    pub fn tone(&self) -> Tone {
        match self.kind {
            ChecksKind::Passing => Tone::Success,
            ChecksKind::Failing => Tone::Danger,
            ChecksKind::Running => Tone::Secondary,
            ChecksKind::None => Tone::Muted,
        }
    }

    /// The cell's glyph; a running cell draws a spinner instead.
    #[must_use]
    pub fn icon(&self) -> Option<Icon> {
        match self.kind {
            ChecksKind::Passing => Some(Icon::CircleCheck),
            ChecksKind::Failing => Some(Icon::CircleX),
            ChecksKind::Running | ChecksKind::None => None,
        }
    }
}
