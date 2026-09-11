//! The transcript's scroll machine, as pure functions over geometry.
//!
//! `spec-B` §B7 is the specification and every rule in it is a decision about *when the
//! viewport may stop following the live edge*. None of it needs a window, so none of it lives
//! in the entity: the list owns a [`FollowState`] and asks these functions.
//!
//! The load-bearing idea is the **user-scroll generation**. Follow is armed at a generation;
//! every manual navigation bumps the counter; follow acts only while the two match. That makes
//! every stale async callback harmless without a single cancellation token.

use gpui::Pixels;

use super::metrics::AGENT_FOLLOW_REARM_PX;

/// Where the viewport is, relative to the live edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollMode {
    /// Pinned to the newest row; new content pulls the viewport with it.
    #[default]
    FollowingEnd,
    /// The first user message of a thread landed near the top and the view owns the streaming
    /// scrolls until real tool activity starts.
    AnchoringNewTurn,
    /// The reader is somewhere else and nothing may move the viewport.
    FreeScrolling,
}

/// A gesture that might break follow.
///
/// The rule, and it is the whole design: **a gesture may break follow only when it can actually
/// move the viewport away from the live edge.** Follow gates the list's own auto-pin, so a
/// spurious break while pinned at the end produces no scroll event, never re-arms, and
/// streaming silently stops following.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// A wheel or trackpad scroll towards older content.
    WheelUp,
    /// A wheel or trackpad scroll towards the live edge.
    WheelDown,
    /// A drag of the scrollbar thumb.
    ScrollbarDrag,
    /// A click on the transcript's content.
    ClickContent,
    /// `ctrl-u`, `ctrl-b`, `gg` — a jump towards older content.
    PageBack,
    /// `ctrl-d`, `ctrl-f`, `G` — a jump towards the live edge.
    PageForward,
}

/// Whether the viewport is inside the re-arm band at the live edge.
///
/// Strict on purpose. A list's own "near end" heuristic typically fires within half a
/// viewport, which re-arms follow while the user is reading history and yanks them back down
/// on the next chunk.
#[must_use]
pub fn is_at_end(content_len: Pixels, scroll: Pixels, viewport: Pixels) -> bool {
    content_len - scroll - viewport <= AGENT_FOLLOW_REARM_PX
}

/// Whether `gesture` may break follow, given whether the content overflows the viewport and
/// whether the viewport is at the live edge.
///
/// Content that underflows the viewport cannot scroll at all, so nothing there may break
/// follow; and clicking near the live edge keeps following, because the click cannot have moved
/// the viewport away from it.
#[must_use]
pub const fn breaks_follow(gesture: Gesture, overflows: bool, at_end: bool) -> bool {
    match gesture {
        Gesture::WheelDown => false,
        Gesture::WheelUp | Gesture::ScrollbarDrag | Gesture::PageBack => overflows,
        Gesture::ClickContent | Gesture::PageForward => !at_end,
    }
}

/// The transcript's follow state: a mode plus the generation bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowState {
    mode: ScrollMode,
    generation: u32,
    armed_at: u32,
}

impl Default for FollowState {
    fn default() -> Self {
        Self {
            mode: ScrollMode::FollowingEnd,
            generation: 0,
            armed_at: 0,
        }
    }
}

impl FollowState {
    /// A transcript that starts out following its tail.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Which mode the transcript is in.
    #[must_use]
    pub const fn mode(&self) -> ScrollMode {
        self.mode
    }

    /// Whether follow is armed *and* current: a callback from an older generation is stale and
    /// answers `false` without any cancellation.
    #[must_use]
    pub const fn is_following(&self) -> bool {
        matches!(self.mode, ScrollMode::FollowingEnd) && self.generation == self.armed_at
    }

    /// Whether the view owns the streaming scrolls because the first turn is anchored.
    #[must_use]
    pub const fn is_anchoring(&self) -> bool {
        matches!(self.mode, ScrollMode::AnchoringNewTurn)
    }

    /// The generation an async callback should carry, so a later manual navigation invalidates
    /// it without a token.
    #[must_use]
    pub const fn generation(&self) -> u32 {
        self.generation
    }

    /// Whether a callback minted at `generation` is still current.
    #[must_use]
    pub const fn is_current(&self, generation: u32) -> bool {
        self.generation == generation
    }

    /// Arm follow: jump-to-latest, a thread switch, a send on a started thread, or the viewport
    /// re-entering the re-arm band.
    pub const fn arm(&mut self) {
        self.mode = ScrollMode::FollowingEnd;
        self.armed_at = self.generation;
    }

    /// Anchor the **first** user message of a thread near the top.
    ///
    /// Releasing the anchor must drop the reserved tail space as well as re-arming follow,
    /// otherwise the timeline settles into "following end" with nothing following anything.
    pub const fn anchor_new_turn(&mut self) {
        self.mode = ScrollMode::AnchoringNewTurn;
        self.generation += 1;
    }

    /// Real tool activity started in the running turn: release the anchor.
    pub const fn release_anchor(&mut self) {
        if self.is_anchoring() {
            self.arm();
        }
    }

    /// Break follow: a manual navigation the reader made.
    pub const fn break_follow(&mut self) {
        self.mode = ScrollMode::FreeScrolling;
        self.generation += 1;
    }

    /// Apply a gesture, and report whether it broke follow.
    ///
    /// Callers short-circuit before this for a modifier-held gesture, an active IME
    /// composition, an open picker, and any gesture inside an expanded body that can still
    /// scroll on its own.
    pub const fn gesture(&mut self, gesture: Gesture, overflows: bool, at_end: bool) -> bool {
        if !breaks_follow(gesture, overflows, at_end) {
            return false;
        }
        self.break_follow();
        true
    }

    /// The viewport moved; re-arm follow when it re-entered the band.
    ///
    /// The list's own `is_at_end` flag is a fallback, **never** a short-circuit: a gap of
    /// 100 px with the flag set is still `false`, which is why `at_end` is computed from
    /// geometry by [`is_at_end`] and only then handed here.
    pub const fn scrolled(&mut self, at_end: bool) {
        if at_end && !self.is_anchoring() {
            self.arm();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    /// §B7.3: the band is 40 px, not half a viewport and not one pixel.
    #[test]
    fn the_rearm_band_is_strict() {
        // 30 px of content below the fold is inside the band.
        assert!(is_at_end(px(1_000.0), px(670.0), px(300.0)));
        // 100 px is not, even though a list's own heuristic would call it "near the end".
        assert!(!is_at_end(px(1_000.0), px(600.0), px(300.0)));
        // 300 px is plainly not.
        assert!(!is_at_end(px(1_000.0), px(400.0), px(300.0)));
        // Exactly at the edge of the band still counts.
        assert!(is_at_end(px(1_000.0), px(660.0), px(300.0)));
    }

    #[test]
    fn content_that_cannot_scroll_never_breaks_follow() {
        for gesture in [Gesture::WheelUp, Gesture::ScrollbarDrag, Gesture::PageBack] {
            assert!(!breaks_follow(gesture, false, true));
            assert!(breaks_follow(gesture, true, true));
        }
    }

    #[test]
    fn a_gesture_towards_the_live_edge_only_breaks_from_away() {
        assert!(!breaks_follow(Gesture::WheelDown, true, false));
        assert!(!breaks_follow(Gesture::ClickContent, true, true));
        assert!(breaks_follow(Gesture::ClickContent, true, false));
        assert!(!breaks_follow(Gesture::PageForward, true, true));
        assert!(breaks_follow(Gesture::PageForward, true, false));
    }

    #[test]
    fn a_stale_callback_answers_false_without_a_cancellation_token() {
        let mut state = FollowState::new();
        let generation = state.generation();
        assert!(state.is_following());
        assert!(state.is_current(generation));

        state.gesture(Gesture::WheelUp, true, true);
        assert_eq!(state.mode(), ScrollMode::FreeScrolling);
        assert!(!state.is_following());
        assert!(!state.is_current(generation));
    }

    #[test]
    fn re_entering_the_band_re_arms_follow() {
        let mut state = FollowState::new();
        state.break_follow();
        state.scrolled(false);
        assert!(!state.is_following());
        state.scrolled(true);
        assert!(state.is_following());
    }

    /// The first send anchors near the top; only real activity releases it. A scroll into the
    /// band must not release the anchor, or the reserved tail collapses under the reader.
    #[test]
    fn the_first_turn_stays_anchored_until_activity_releases_it() {
        let mut state = FollowState::new();
        state.anchor_new_turn();
        assert!(state.is_anchoring());
        assert!(!state.is_following());
        state.scrolled(true);
        assert!(state.is_anchoring(), "a scroll must not release the anchor");
        state.release_anchor();
        assert!(state.is_following());
        // Releasing twice is harmless.
        state.release_anchor();
        assert!(state.is_following());
    }

    #[test]
    fn arming_after_a_break_makes_follow_current_again() {
        let mut state = FollowState::new();
        state.break_follow();
        let generation = state.generation();
        state.arm();
        assert!(state.is_following());
        assert!(state.is_current(generation));
    }
}
