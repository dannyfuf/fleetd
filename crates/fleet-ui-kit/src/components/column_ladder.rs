//! `ColumnLadder` — resolve a `ch`-based responsive column set for the current pane width.
//!
//! §2.9 is authoritative and is stated in `ch` of the **pane that owns the columns**, not of
//! the window; that is what makes the port from the swarm TUI verifiable. One `ch` is
//! [`crate::theme::CH`] = 7.5 px at the data type size, and [`ColumnLadder::pane_ch`] converts
//! a measured pane width into that unit, padding included.
//!
//! ```ignore
//! let pane_ch = ColumnLadder::pane_ch(bounds.size.width, cx);
//! let ladder = ColumnLadder::worktrees_in_scope(scope_is_all);
//! let columns = ladder.resolve(pane_ch);
//! // then feed each ResolvedColumn to RowColumn::resolved(..)
//! ```

use gpui::{Pixels, SharedString};

use crate::{
    components::ColumnAlign,
    theme::{ActiveTheme, CH, ch},
};

/// How wide a column is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// A fixed budget in `ch`.
    Ch(f32),
    /// Takes the remaining width, never below `min_ch`.
    Flex {
        /// Minimum width in `ch`.
        min_ch: f32,
    },
    /// A step ladder of `(pane_ch_at_least, width_ch)`, highest threshold first. A step whose
    /// width is `0` drops the column.
    Ladder(&'static [(f32, f32)]),
}

/// One column of a list.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnSpec {
    /// A stable key the view uses to look the resolved column up.
    pub key: SharedString,
    /// How wide it is.
    pub width: ColumnWidth,
    /// How its content sits.
    pub align: ColumnAlign,
    /// The pane width, in `ch`, below which the column is dropped entirely.
    pub min_pane_ch: f32,
    /// Show the column at any pane width, whatever `min_pane_ch` says.
    ///
    /// §2.9 has exactly two columns whose visibility depends on something other than the pane:
    /// the worktrees `repo` column, shown at any width in `All` scope, and the PR `repo`
    /// column, shown only when the scope is multi-repo. This flag is how a view states that
    /// non-geometric half without re-implementing the ladder.
    pub forced: bool,
}

impl ColumnSpec {
    /// A fixed `ch` column.
    pub fn fixed(key: impl Into<SharedString>, width_ch: f32) -> Self {
        Self {
            key: key.into(),
            width: ColumnWidth::Ch(width_ch),
            align: ColumnAlign::Left,
            min_pane_ch: 0.0,
            forced: false,
        }
    }

    /// The flex column of the list.
    pub fn flex(key: impl Into<SharedString>, min_ch: f32) -> Self {
        Self {
            key: key.into(),
            width: ColumnWidth::Flex { min_ch },
            align: ColumnAlign::Left,
            min_pane_ch: 0.0,
            forced: false,
        }
    }

    /// A column whose width steps down with the pane.
    pub fn ladder(key: impl Into<SharedString>, steps: &'static [(f32, f32)]) -> Self {
        Self {
            key: key.into(),
            width: ColumnWidth::Ladder(steps),
            align: ColumnAlign::Left,
            min_pane_ch: 0.0,
            forced: false,
        }
    }

    /// Set the alignment.
    pub fn align(mut self, align: ColumnAlign) -> Self {
        self.align = align;
        self
    }

    /// Drop the column below this pane width, in `ch`.
    pub fn shown_from(mut self, pane_ch: f32) -> Self {
        self.min_pane_ch = pane_ch;
        self
    }

    /// Show the column regardless of the pane width.
    pub fn forced(mut self, forced: bool) -> Self {
        self.forced = forced;
        self
    }

    fn resolve(&self, pane_ch: f32) -> Option<ResolvedColumn> {
        if !self.is_shown(pane_ch) {
            return None;
        }
        let (width, min_width) = match self.width {
            ColumnWidth::Ch(width) => (Some(ch(width)), None),
            ColumnWidth::Flex { min_ch } => (None, Some(ch(min_ch))),
            ColumnWidth::Ladder(steps) => (Some(ch(resolve_steps(steps, pane_ch)?)), None),
        };
        Some(ResolvedColumn {
            key: self.key.clone(),
            width,
            min_width,
            align: self.align,
        })
    }

    /// Whether this spec survives at `pane_ch`.
    pub fn is_shown(&self, pane_ch: f32) -> bool {
        self.forced || pane_ch >= self.min_pane_ch
    }
}

/// A column resolved for one pane width.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedColumn {
    /// The spec's key.
    pub key: SharedString,
    /// Width in pixels. `None` for the flex column.
    pub width: Option<Pixels>,
    /// Minimum width in pixels, for the flex column.
    pub min_width: Option<Pixels>,
    /// Alignment.
    pub align: ColumnAlign,
}

/// An ordered set of column specs.
#[derive(Clone, Debug, Default)]
pub struct ColumnLadder {
    specs: Vec<ColumnSpec>,
}

impl ColumnLadder {
    /// A ladder over the given specs, in render order.
    pub fn new(specs: impl IntoIterator<Item = ColumnSpec>) -> Self {
        Self {
            specs: specs.into_iter().collect(),
        }
    }

    /// Append a spec.
    pub fn column(mut self, spec: ColumnSpec) -> Self {
        self.specs.push(spec);
        self
    }

    /// The specs, in render order, whatever the pane width.
    pub fn specs(&self) -> &[ColumnSpec] {
        &self.specs
    }

    /// Convert a measured pane width into the `ch` unit §2.9 speaks, after subtracting the
    /// 16 px pane padding on both edges.
    ///
    /// Measure the **pane**, never the window: the detail panel opening is exactly the case
    /// where the two disagree, and it is what the ladder exists to survive.
    pub fn pane_ch(width: Pixels, cx: &impl ActiveTheme) -> f32 {
        let padding = cx.theme().space.lg * 2.0;
        (width - padding).max(Pixels::ZERO).as_f32() / CH
    }

    /// The columns that survive at `pane_ch`, in order.
    pub fn resolve(&self, pane_ch: f32) -> Vec<ResolvedColumn> {
        self.specs
            .iter()
            .filter_map(|spec| spec.resolve(pane_ch))
            .collect()
    }

    /// Whether a key survives at `pane_ch`.
    pub fn shows(&self, key: &str, pane_ch: f32) -> bool {
        self.specs
            .iter()
            .filter(|spec| spec.key == key)
            .find_map(|spec| spec.resolve(pane_ch))
            .is_some()
    }

    /// The resolved width of one column in `ch`; absent for hidden or flexible columns.
    pub fn width_ch(&self, key: &str, pane_ch: f32) -> Option<f32> {
        self.specs
            .iter()
            .filter(|spec| spec.key == key)
            .find_map(|spec| spec.resolve(pane_ch))
            .and_then(|column| column.width)
            .map(|width| width.as_f32() / CH)
    }

    /// The worktrees list ladder of §2.9, keyed
    /// `glyph`, `branch`, `repo`, `keepalive`, `pr`, `age`.
    pub fn worktrees() -> Self {
        Self::worktrees_in_scope(false)
    }

    /// The worktrees ladder, with the `repo` column forced on in `All` scope.
    ///
    /// §2.9 column 3: `owner/name` is shown when the scope is `All` **or** the pane is at least
    /// 110 ch, because in `All` scope the repo is the only thing that disambiguates two
    /// identically named branches.
    pub fn worktrees_in_scope(all_scope: bool) -> Self {
        Self::new([
            ColumnSpec::fixed("glyph", 2.0).align(ColumnAlign::Center),
            ColumnSpec::flex("branch", 24.0),
            ColumnSpec::fixed("repo", 14.0)
                .shown_from(110.0)
                .forced(all_scope),
            ColumnSpec::ladder("keepalive", KEEP_ALIVE_STEPS),
            ColumnSpec::fixed("pr", 15.0).shown_from(60.0),
            ColumnSpec::fixed("age", 7.0)
                .align(ColumnAlign::Right)
                .shown_from(52.0),
        ])
    }

    /// The PR list ladder of §2.9, keyed
    /// `presence`, `number`, `title`, `author`, `head`, `repo`, `state`, `age`, for one tab and
    /// one scope.
    ///
    /// The two-step author breakpoint (12 ch at 70 ch, 16 ch at 130 ch) is [D-5] and is the one
    /// ladder the pixel-only port lost. §2.9: the `author` column is meaningful only in the
    /// `REVIEW` tab, and the `repo` column needs both a wide pane **and** a multi-repo scope.
    pub fn pull_requests_for(review_tab: bool, multi_repo: bool) -> Self {
        const AUTHOR: &[(f32, f32)] = &[(130.0, 16.0), (70.0, 12.0), (0.0, 0.0)];
        let mut specs = vec![
            ColumnSpec::fixed("presence", 2.0).align(ColumnAlign::Center),
            ColumnSpec::fixed("number", 6.0).align(ColumnAlign::Right),
            ColumnSpec::flex("title", 32.0),
        ];
        if review_tab {
            specs.push(ColumnSpec::ladder("author", AUTHOR));
        }
        specs.push(ColumnSpec::fixed("head", 12.0).shown_from(90.0));
        if multi_repo {
            specs.push(ColumnSpec::fixed("repo", 10.0).shown_from(110.0));
        }
        specs.push(ColumnSpec::fixed("state", 8.0));
        specs.push(
            ColumnSpec::fixed("age", 7.0)
                .align(ColumnAlign::Right)
                .shown_from(52.0),
        );
        Self::new(specs)
    }
}

pub(super) const KEEP_ALIVE_STEPS: &[(f32, f32)] =
    &[(104.0, 18.0), (88.0, 14.0), (72.0, 10.0), (0.0, 0.0)];

pub(super) fn resolve_steps(steps: &[(f32, f32)], pane_ch: f32) -> Option<f32> {
    steps
        .iter()
        .find(|(threshold, _)| pane_ch >= *threshold)
        .map(|(_, width)| *width)
        .filter(|width| *width > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktrees_drops_columns_with_the_pane() {
        let ladder = ColumnLadder::worktrees();
        assert!(ladder.shows("repo", 138.0));
        assert!(!ladder.shows("repo", 93.0));
        assert!(ladder.shows("keepalive", 104.0));
        assert!(!ladder.shows("keepalive", 71.0));
        assert!(!ladder.shows("age", 40.0));
        assert!(ladder.shows("branch", 40.0));
    }

    #[test]
    fn keep_alive_steps_18_14_10_0() {
        let ladder = ColumnLadder::worktrees();
        assert_eq!(ladder.width_ch("keepalive", 138.0), Some(18.0));
        assert_eq!(ladder.width_ch("keepalive", 90.0), Some(14.0));
        assert_eq!(ladder.width_ch("keepalive", 72.0), Some(10.0));
        assert_eq!(ladder.width_ch("keepalive", 60.0), None);
    }

    #[test]
    fn narrow_pane_keeps_glyph_branch_pr_and_age() {
        // §2.9: "below 72 ch only columns 1, 2, 5, 6 survive".
        let keys: Vec<String> = ColumnLadder::worktrees()
            .resolve(70.0)
            .into_iter()
            .map(|c| c.key.to_string())
            .collect();
        assert_eq!(keys, ["glyph", "branch", "pr", "age"]);
    }

    #[test]
    fn all_scope_forces_the_repo_column() {
        let ladder = ColumnLadder::worktrees_in_scope(true);
        assert!(ladder.shows("repo", 93.0));
        assert!(ladder.shows("repo", 60.0));
    }

    #[test]
    fn author_has_two_steps() {
        let ladder = ColumnLadder::pull_requests_for(true, true);
        assert_eq!(ladder.width_ch("author", 140.0), Some(16.0));
        assert_eq!(ladder.width_ch("author", 80.0), Some(12.0));
        assert!(!ladder.shows("author", 60.0));
    }

    #[test]
    fn author_is_review_only_and_repo_is_multi_repo_only() {
        let mine = ColumnLadder::pull_requests_for(false, false);
        assert!(!mine.shows("author", 140.0));
        assert!(!mine.shows("repo", 140.0));
        let review = ColumnLadder::pull_requests_for(true, true);
        assert!(review.shows("author", 140.0));
        assert!(review.shows("repo", 140.0));
    }

    #[test]
    fn flex_columns_carry_their_minimum() {
        let title = ColumnLadder::pull_requests_for(true, true)
            .resolve(140.0)
            .into_iter()
            .find(|c| c.key.as_ref() == "title")
            .expect("the title column is always shown");
        assert_eq!(title.width, None);
        assert_eq!(title.min_width, Some(ch(32.0)));
    }
}
