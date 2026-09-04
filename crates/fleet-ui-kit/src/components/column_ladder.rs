//! `ColumnLadder` — resolve a `ch`-based responsive column set for the current pane width.
//!
//! §2.9 is authoritative and is stated in `ch` of the **pane that owns the columns**, not of
//! the window; that is what makes the port from the swarm TUI verifiable. One `ch` is
//! [`crate::theme::CH`] = 7.5 px at the data type size.

use gpui::{Pixels, SharedString};

use crate::{components::ColumnAlign, theme::ch};

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
    /// A step ladder of `(pane_ch_at_least, width_ch)`, highest threshold first.
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
}

impl ColumnSpec {
    /// A fixed `ch` column.
    pub fn fixed(key: impl Into<SharedString>, width_ch: f32) -> Self {
        Self {
            key: key.into(),
            width: ColumnWidth::Ch(width_ch),
            align: ColumnAlign::Left,
            min_pane_ch: 0.0,
        }
    }

    /// The flex column of the list.
    pub fn flex(key: impl Into<SharedString>, min_ch: f32) -> Self {
        Self {
            key: key.into(),
            width: ColumnWidth::Flex { min_ch },
            align: ColumnAlign::Left,
            min_pane_ch: 0.0,
        }
    }

    /// A column whose width steps down with the pane.
    pub fn ladder(key: impl Into<SharedString>, steps: &'static [(f32, f32)]) -> Self {
        Self {
            key: key.into(),
            width: ColumnWidth::Ladder(steps),
            align: ColumnAlign::Left,
            min_pane_ch: 0.0,
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

    /// The columns that survive at `pane_ch`, in order.
    pub fn resolve(&self, pane_ch: f32) -> Vec<ResolvedColumn> {
        self.specs
            .iter()
            .filter(|spec| pane_ch >= spec.min_pane_ch)
            .filter_map(|spec| match spec.width {
                ColumnWidth::Ch(width) => Some(ResolvedColumn {
                    key: spec.key.clone(),
                    width: Some(ch(width)),
                    min_width: None,
                    align: spec.align,
                }),
                ColumnWidth::Flex { min_ch } => Some(ResolvedColumn {
                    key: spec.key.clone(),
                    width: None,
                    min_width: Some(ch(min_ch)),
                    align: spec.align,
                }),
                ColumnWidth::Ladder(steps) => steps
                    .iter()
                    .find(|(threshold, _)| pane_ch >= *threshold)
                    .and_then(|(_, width)| {
                        (*width > 0.0).then(|| ResolvedColumn {
                            key: spec.key.clone(),
                            width: Some(ch(*width)),
                            min_width: None,
                            align: spec.align,
                        })
                    }),
            })
            .collect()
    }

    /// Whether a key survives at `pane_ch`.
    pub fn shows(&self, key: &str, pane_ch: f32) -> bool {
        self.resolve(pane_ch).iter().any(|c| c.key.as_ref() == key)
    }

    /// The worktrees list ladder of §2.9, keyed
    /// `glyph`, `branch`, `repo`, `keepalive`, `pr`, `age`.
    pub fn worktrees() -> Self {
        const KEEP_ALIVE: &[(f32, f32)] = &[(104.0, 18.0), (88.0, 14.0), (72.0, 10.0), (0.0, 0.0)];
        Self::new([
            ColumnSpec::fixed("glyph", 2.0).align(ColumnAlign::Center),
            ColumnSpec::flex("branch", 24.0),
            ColumnSpec::fixed("repo", 14.0).shown_from(110.0),
            ColumnSpec::ladder("keepalive", KEEP_ALIVE),
            ColumnSpec::fixed("pr", 15.0).shown_from(60.0),
            ColumnSpec::fixed("age", 7.0)
                .align(ColumnAlign::Right)
                .shown_from(52.0),
        ])
    }

    /// The PR list ladder of §2.9, keyed
    /// `presence`, `number`, `title`, `author`, `head`, `repo`, `state`, `age`.
    ///
    /// The two-step author breakpoint (12 ch at 70 ch, 16 ch at 130 ch) is [D-5] and is the one
    /// ladder the pixel-only port lost.
    pub fn pull_requests() -> Self {
        const AUTHOR: &[(f32, f32)] = &[(130.0, 16.0), (70.0, 12.0), (0.0, 0.0)];
        Self::new([
            ColumnSpec::fixed("presence", 2.0).align(ColumnAlign::Center),
            ColumnSpec::fixed("number", 6.0).align(ColumnAlign::Right),
            ColumnSpec::flex("title", 32.0),
            ColumnSpec::ladder("author", AUTHOR),
            ColumnSpec::fixed("head", 12.0).shown_from(90.0),
            ColumnSpec::fixed("repo", 10.0).shown_from(110.0),
            ColumnSpec::fixed("state", 8.0),
            ColumnSpec::fixed("age", 7.0)
                .align(ColumnAlign::Right)
                .shown_from(52.0),
        ])
    }
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
    fn author_has_two_steps() {
        let ladder = ColumnLadder::pull_requests();
        let wide = ladder.resolve(140.0);
        let author = wide.iter().find(|c| c.key.as_ref() == "author").unwrap();
        assert_eq!(author.width, Some(crate::theme::ch(16.0)));
        let narrow = ladder.resolve(80.0);
        let author = narrow.iter().find(|c| c.key.as_ref() == "author").unwrap();
        assert_eq!(author.width, Some(crate::theme::ch(12.0)));
        assert!(!ladder.shows("author", 60.0));
    }
}
