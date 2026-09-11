//! `MetadataRow` — the composer's measured, ordered strip of collapsible blocks.
//!
//! `spec-B` §B5.1: the composer's bottom line states what the harness reports and **invents no
//! segment** — a fresh Claude tab that has not published an effort has three segments, not
//! four. When the window is narrow the strip collapses **from the right** into an overflow
//! count rather than wrapping or clipping.
//!
//! Two rules make it cheap and stable:
//!
//! - **The hidden count is memoised per width**, in a [`MetadataFit`] the owner holds across
//!   frames. `render` reads a memo; it never runs the fit loop per frame.
//! - **The model segment never collapses; it truncates.** Losing which model is answering is
//!   worse than losing its name's tail, so the leading segment is the one that shrinks.

use std::cell::Cell;

use gpui::{App, Pixels, SharedString, Window, div, prelude::*, px};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
};

/// One block of the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataSegment {
    /// What the block says.
    pub text: SharedString,
    /// How wide it wants to be. Estimated from its length in display columns at construction,
    /// which is what keeps the fit a pure function of data the caller already has; a caller
    /// that measured for real overrides it with [`MetadataSegment::width`].
    pub width: Pixels,
    /// Whether the block may be hidden into the overflow menu.
    pub collapsible: bool,
}

impl MetadataSegment {
    /// A collapsible block, sized from its own text.
    #[must_use]
    pub fn new(text: impl Into<SharedString>) -> Self {
        let text = text.into();
        let width = ch(text.chars().count() as f32);
        Self {
            text,
            width,
            collapsible: true,
        }
    }

    /// A block that truncates instead of collapsing — the model segment.
    #[must_use]
    pub fn pinned(text: impl Into<SharedString>) -> Self {
        Self {
            collapsible: false,
            ..Self::new(text)
        }
    }

    /// Override the measured width.
    #[must_use]
    pub const fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }
}

/// What fits at one width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataFitResult {
    /// How many leading segments are drawn.
    pub visible: usize,
    /// How many trailing segments went into the overflow count.
    pub hidden: usize,
}

/// The per-width memo of a strip's fit.
///
/// The owner holds one of these for the life of the composer and bumps `revision` whenever the
/// segment list changes. A frame at an unchanged width is a [`Cell`] read.
#[derive(Debug, Default)]
pub struct MetadataFit {
    memo: Cell<Option<(f32, u32, MetadataFitResult)>>,
}

impl MetadataFit {
    /// An empty memo.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How much of `segments` fits in `available`, memoised per `(width, revision)`.
    ///
    /// `gap` is the space between two blocks and `overflow` the width of the overflow chip,
    /// both from the theme, so the fit never guesses at geometry the renderer will use.
    pub fn fit(
        &self,
        available: Pixels,
        revision: u32,
        segments: &[MetadataSegment],
        gap: Pixels,
        overflow: Pixels,
    ) -> MetadataFitResult {
        let key = f32::from(available);
        if let Some((cached_width, cached_revision, result)) = self.memo.get()
            && cached_width == key
            && cached_revision == revision
        {
            return result;
        }
        let result = fit(available, segments, gap, overflow);
        self.memo.set(Some((key, revision, result)));
        result
    }

    /// Forget the memo, for a caller that changes geometry without changing the segments.
    pub fn invalidate(&self) {
        self.memo.set(None);
    }
}

/// The fit itself: drop collapsible segments from the right until the rest fits.
///
/// A pinned segment is never dropped — it truncates — so a strip of pinned segments alone
/// always reports every one of them visible.
#[must_use]
pub fn fit(
    available: Pixels,
    segments: &[MetadataSegment],
    gap: Pixels,
    overflow: Pixels,
) -> MetadataFitResult {
    let total = segments.len();
    if total == 0 {
        return MetadataFitResult {
            visible: 0,
            hidden: 0,
        };
    }
    let mut visible = total;
    loop {
        let hidden = total - visible;
        let mut width = px(0.0);
        for (index, segment) in segments.iter().take(visible).enumerate() {
            if index > 0 {
                width += gap;
            }
            width += segment.width;
        }
        if hidden > 0 {
            width += gap + overflow;
        }
        if width <= available || visible == 0 {
            return MetadataFitResult { visible, hidden };
        }
        // A pinned trailing block cannot be dropped: it truncates, and the strip stops
        // collapsing there.
        if !segments[visible - 1].collapsible {
            return MetadataFitResult { visible, hidden };
        }
        visible -= 1;
    }
}

/// The composer's metadata strip.
#[derive(IntoElement)]
pub struct MetadataRow {
    segments: Vec<MetadataSegment>,
    fit: MetadataFitResult,
    trailing: Vec<MetadataSegment>,
}

impl MetadataRow {
    /// A strip whose fit the owner already resolved through its [`MetadataFit`].
    ///
    /// The fit is an argument rather than something the row computes, because a `RenderOnce`
    /// component is rebuilt every frame and the memo must outlive it.
    #[must_use]
    pub fn new(segments: Vec<MetadataSegment>, fit: MetadataFitResult) -> Self {
        Self {
            segments,
            fit,
            trailing: Vec::new(),
        }
    }

    /// The right-hand, turn-derived blocks. They stay empty until a turn has run, and they are
    /// never part of the collapse: there are at most three of them.
    #[must_use]
    pub fn trailing(mut self, trailing: Vec<MetadataSegment>) -> Self {
        self.trailing = trailing;
        self
    }
}

impl RenderOnce for MetadataRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let visible = self.fit.visible.min(self.segments.len());
        let hidden = self.segments.len() - visible;
        let block = |segment: &MetadataSegment| {
            let text = Text::hint(segment.text.clone()).tone(Tone::Secondary);
            if segment.collapsible {
                div().flex_none().child(text)
            } else {
                // The pinned block is the one that shrinks, and it needs `min_w_0` or a flex
                // child never shrinks and no ellipsis ever appears.
                div().flex_1().min_w_0().child(text.ellipsize())
            }
        };
        div()
            .w_full()
            .h(theme.metrics.strip_h)
            .flex()
            .items_center()
            .gap(theme.space.md)
            .children(self.segments.iter().take(visible).map(block))
            .children((hidden > 0).then(|| {
                div()
                    .id("metadata-overflow")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(theme.space.xxs)
                    .child(Icon::Ellipsis.el().size(IconSize::Small).tone(Tone::Muted))
                    .child(
                        Text::hint(SharedString::from(format!("{hidden}")))
                            .faint()
                            .flex_none(),
                    )
            }))
            .child(div().flex_1().min_w_0())
            .children(self.trailing.iter().map(|segment| {
                div()
                    .flex_none()
                    .child(Text::hint(segment.text.clone()).faint())
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments() -> Vec<MetadataSegment> {
        vec![
            MetadataSegment::pinned("claude-opus-5"),
            MetadataSegment::new("high"),
            MetadataSegment::new("supervised"),
            MetadataSegment::new("build"),
        ]
    }

    const GAP: Pixels = px(8.0);
    const OVERFLOW: Pixels = px(24.0);

    #[test]
    fn everything_fits_when_there_is_room() {
        let result = fit(px(1_000.0), &segments(), GAP, OVERFLOW);
        assert_eq!(result.visible, 4);
        assert_eq!(result.hidden, 0);
    }

    /// Collapse comes off the right, one block at a time.
    #[test]
    fn narrowing_collapses_from_the_right() {
        let segments = segments();
        let wide = fit(px(1_000.0), &segments, GAP, OVERFLOW);
        let mid = fit(px(190.0), &segments, GAP, OVERFLOW);
        let narrow = fit(px(150.0), &segments, GAP, OVERFLOW);
        assert!(mid.visible < wide.visible, "{mid:?} vs {wide:?}");
        assert!(narrow.visible <= mid.visible, "{narrow:?} vs {mid:?}");
        assert_eq!(narrow.visible + narrow.hidden, segments.len());
    }

    /// The model segment never collapses. At any width the pinned leading block survives and
    /// truncates instead.
    #[test]
    fn the_pinned_segment_never_collapses() {
        let segments = segments();
        for width in [px(0.0), px(10.0), px(60.0), px(120.0)] {
            let result = fit(width, &segments, GAP, OVERFLOW);
            assert_eq!(result.visible, 1, "at {width:?}");
            assert_eq!(result.hidden, 3, "at {width:?}");
        }
    }

    #[test]
    fn an_empty_strip_reports_nothing() {
        let result = fit(px(100.0), &[], GAP, OVERFLOW);
        assert_eq!(result.visible, 0);
        assert_eq!(result.hidden, 0);
    }

    /// The memo is per width *and* per revision: the same width is a `Cell` read, a new width
    /// or a new segment list recomputes.
    #[test]
    fn the_fit_is_memoised_per_width() {
        let memo = MetadataFit::new();
        let segments = segments();
        let wide = memo.fit(px(1_000.0), 0, &segments, GAP, OVERFLOW);
        assert_eq!(memo.fit(px(1_000.0), 0, &segments, GAP, OVERFLOW), wide);
        let narrow = memo.fit(px(150.0), 0, &segments, GAP, OVERFLOW);
        assert_ne!(narrow, wide);
        // A stale memo cannot survive a segment change.
        let shorter = vec![MetadataSegment::pinned("claude-opus-5")];
        let after = memo.fit(px(150.0), 1, &shorter, GAP, OVERFLOW);
        assert_eq!(after.hidden, 0);
        memo.invalidate();
        assert_eq!(memo.fit(px(150.0), 1, &shorter, GAP, OVERFLOW), after);
    }
}
