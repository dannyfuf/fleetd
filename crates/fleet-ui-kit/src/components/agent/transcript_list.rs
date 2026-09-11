//! Variable-height, bottom-anchored native-agent transcript.
//!
//! §5 and §11 of `docs/NATIVE-AGENTS.md` pin the primitive: GPUI `list`, **not**
//! `uniform_list`, because an assistant paragraph, a diff and a 30 px tool row are not the same
//! height. `list` caches the height it measured for every row, so [`TranscriptList::set_rows`]
//! diffs the projection and splices only what really changed: a streaming turn re-measures its
//! last row and nothing else, which is what keeps a fast model from re-laying-out the whole
//! thread per token.
//!
//! It is one of the kit's few entities, and it earns it three times over: `ListState` caches
//! measured heights, the scroll machine (`spec-B` §B7) is state that must outlive a frame, and
//! the row focus needs a focus handle. Three rules the rewrite exists to keep:
//!
//! - **`ListState::reset` is never called from `render`.** `reset` discards every measured
//!   height; [`TranscriptList::set_thread`] is the only caller, because a thread switch is the
//!   only moment those heights are worthless.
//! - **Follow is a generation, not a flag.** Every manual navigation bumps a counter and
//!   follow acts only while the armed generation matches, so a stale callback is harmless with
//!   no cancellation token (`super::scroll`).
//! - **Nothing ticks but the row that has a clock.** The working row's label is recomputed by a
//!   1 Hz task that touches one string, never by `render`.

use std::{ops::Range, rc::Rc, time::Duration, time::Instant};

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, FollowMode, KeyDownEvent,
    ListAlignment, ListScrollEvent, ListState, Pixels, SharedString, Task, Window, div, list,
    prelude::*, px,
};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

use super::{
    format::format_working,
    metrics::{AGENT_LIST_OVERDRAW, AGENT_SCROLLBAR_INSET},
    rows::{RowContext, TranscriptRow, TranscriptRowKind, diff_rows, row_element},
    scroll::{FollowState, Gesture, ScrollMode, is_at_end},
};

#[cfg(test)]
mod tests;

/// What the transcript asks its owner to do. It never acts on a thread itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// `⏎`, or a click on a row's header line: expand or collapse that row.
    Toggle(SharedString),
    /// A row-focus key fired on the focused row (`AgentNativeScroll > AgentRow`).
    RowAction {
        /// Which row.
        row: SharedString,
        /// What was asked of it.
        action: RowAction,
    },
    /// The reader scrolled within [`OLDEST_PREFETCH_ROWS`] of the top of the rows it holds.
    ///
    /// The transcript does not know whether older history exists — the owner does, from the page
    /// cursor the daemon sent — so this reports the *gesture*, not a decision. It fires once per
    /// row set: the owner's answer is to prepend older rows, which is itself a new row set and
    /// re-arms it.
    ReachedOldest,
}

/// How close to the top of the held rows counts as "the reader reached the oldest row".
///
/// Not zero: a page read takes a round trip, and firing only at the very first row means the
/// reader watches a hard stop while it happens. Small enough that a flick to the top of a long
/// thread asks for one page rather than three.
pub const OLDEST_PREFETCH_ROWS: usize = 3;

/// The row-focus verbs of `docs/KEYMAP.md` § *Native agent thread*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    /// `u` — revert this edit, or this turn on a footer.
    Revert,
    /// `o` — open the row's file in the editor.
    Open,
    /// `y` — copy the row's payload.
    Copy,
    /// `d` — open the diff of an edit row or a turn footer.
    Diff,
}

/// Builds the owner-supplied body of one expandable row — a `DiffView`, an inline image, an
/// MCP payload — or declines and lets the row draw its own text.
///
/// `Rc` because every row in a frame gets a handle to the same renderer. The kit cannot reach
/// `fleet_lazygit::diff_view::DiffView` (ADR 0010), so the owner hands the element back per
/// frame instead.
pub type RowBodyRenderer = Rc<dyn Fn(&TranscriptRow, &mut App) -> Option<AnyElement>>;

/// `(top, height)` for the scroll thumb as fractions of the track, or `None` when the
/// transcript fits and needs none.
///
/// Both fractions are of the whole content, which is what [`crate::components::Pane`] draws a
/// thumb from, so the transcript's thumb behaves exactly like every other thumb in Fleet.
/// `min_h` — `metrics.diff_thumb_min_h` — keeps a very long transcript's thumb grabbable.
#[must_use]
pub fn scroll_thumb(
    offset: Pixels,
    max: Pixels,
    viewport: Pixels,
    min_h: Pixels,
) -> Option<(f32, f32)> {
    if max <= px(0.0) || viewport <= px(0.0) {
        return None;
    }
    let (offset, max, viewport) = (f32::from(offset), f32::from(max), f32::from(viewport));
    let content = viewport + max;
    let height = (viewport / content).clamp(f32::from(min_h) / content, 1.0);
    let top = (offset / content).clamp(0.0, 1.0 - height);
    Some((top, height))
}

/// Stateful variable-height transcript surface.
pub struct TranscriptList {
    rows: Vec<TranscriptRow>,
    state: ListState,
    focus_handle: FocusHandle,
    follow: FollowState,
    scroll_mode: bool,
    visible: Range<usize>,
    focused_row: Option<usize>,
    row_body: Option<RowBodyRenderer>,
    /// The working row's clock, recomputed by [`Self::sync_working_clock`] and read by `render`.
    working_label: Option<SharedString>,
    _working_tick: Option<Task<()>>,
    jump_visible: bool,
    _jump_debounce: Option<Task<()>>,
    /// Whether [`TranscriptEvent::ReachedOldest`] has already fired for the rows in hand.
    ///
    /// Re-armed by the identity of the first row changing, which is exactly what prepending an
    /// older page does — so one page is asked for per page delivered, and a reader parked at the
    /// top of a fully loaded thread asks for nothing.
    asked_for_older: bool,
}

impl TranscriptList {
    /// Creates an empty transcript entity.
    #[must_use]
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = ListState::new(0, ListAlignment::Bottom, AGENT_LIST_OVERDRAW);
        state.set_follow_mode(FollowMode::Tail);
        state.set_scroll_handler(cx.listener(
            |this: &mut Self, event: &ListScrollEvent, _window, cx| {
                this.on_scroll(event, cx);
            },
        ));
        Self {
            rows: Vec::new(),
            state,
            focus_handle: cx.focus_handle(),
            follow: FollowState::new(),
            scroll_mode: false,
            visible: 0..0,
            focused_row: None,
            row_body: None,
            working_label: None,
            _working_tick: None,
            jump_visible: false,
            _jump_debounce: None,
            asked_for_older: false,
        }
    }

    /// Installs the renderer for an expandable row's owner-supplied body.
    pub fn set_row_body(
        &mut self,
        render: impl Fn(&TranscriptRow, &mut App) -> Option<AnyElement> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.row_body = Some(Rc::new(render));
        cx.notify();
    }

    /// Replaces the keyed row projection.
    ///
    /// Only the rows that changed are spliced into the list state, so every untouched row keeps
    /// the height it was measured at.
    pub fn set_rows(&mut self, rows: Vec<TranscriptRow>, cx: &mut Context<Self>) {
        if let Some(splice) = diff_rows(&self.rows, &rows) {
            // A splice that touches the front means the oldest end of the transcript moved —
            // which is what answering [`TranscriptEvent::ReachedOldest`] with a page does. The
            // question is re-armed, not re-fired: it fires again only when the reader is near
            // the top of the *new* rows, and prepending a page moves the viewport away from it.
            if splice.old_range.start == 0 {
                self.asked_for_older = false;
            }
            self.state.splice(splice.old_range, splice.count);
        }
        self.rows = rows;
        if let Some(focused) = self.focused_row
            && focused >= self.rows.len()
        {
            self.focused_row = None;
        }
        self.sync_working_clock(cx);
        cx.notify();
    }

    /// Switches to a different thread's rows.
    ///
    /// This is the **only** caller of [`ListState::reset`]: every measured height belongs to the
    /// thread being left, and the new thread starts at its own live edge.
    pub fn set_thread(&mut self, rows: Vec<TranscriptRow>, cx: &mut Context<Self>) {
        self.state.reset(rows.len());
        self.rows = rows;
        self.focused_row = None;
        self.scroll_mode = false;
        self.follow = FollowState::new();
        self.state.set_follow_mode(FollowMode::Tail);
        self.jump_visible = false;
        self._jump_debounce = None;
        self.asked_for_older = false;
        self.sync_working_clock(cx);
        cx.notify();
    }

    /// The current presentation rows.
    #[must_use]
    pub fn rows(&self) -> &[TranscriptRow] {
        &self.rows
    }

    /// The focus handle the transcript routes bare keys through.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Which mode the scroll machine is in.
    #[must_use]
    pub fn scroll_state(&self) -> ScrollMode {
        self.follow.mode()
    }

    /// Whether the transcript is following its live edge.
    #[must_use]
    pub fn is_following(&self) -> bool {
        self.follow.is_following()
    }

    /// Whether the jump-to-latest chip is showing.
    #[must_use]
    pub fn shows_jump_to_latest(&self) -> bool {
        self.jump_visible
    }

    // -----------------------------------------------------------------------------------------
    // The scroll machine
    // -----------------------------------------------------------------------------------------

    /// Jumps to the newest row and re-arms follow (the chip, a thread switch, a send).
    pub fn scroll_to_latest(&mut self, cx: &mut Context<Self>) {
        self.state.scroll_to_end();
        self.follow.arm();
        self.state.set_follow_mode(FollowMode::Tail);
        self.set_jump_visible(false, cx);
        cx.notify();
    }

    /// Jumps to the newest row **without** thawing a frozen tail (`G` in scroll mode).
    ///
    /// `G` is documented as "newest", not as a way out of scroll mode: the status bar's `SCROLL`
    /// word stays on until `q` / `i` / `esc`, and so must the frozen tail.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.state.scroll_to_end();
        if !self.scroll_mode {
            self.follow.arm();
        }
        self.set_jump_visible(false, cx);
        cx.notify();
    }

    /// Anchors the **first** user message of a thread near the top.
    pub fn anchor_new_turn(&mut self, cx: &mut Context<Self>) {
        self.follow.anchor_new_turn();
        self.state.pause_following_tail();
        cx.notify();
    }

    /// Real tool activity started in the running turn: release the anchor and follow again.
    pub fn release_anchor(&mut self, cx: &mut Context<Self>) {
        if self.follow.is_anchoring() {
            self.follow.release_anchor();
            self.state.set_follow_mode(FollowMode::Tail);
            cx.notify();
        }
    }

    /// Reports a gesture. Returns whether it broke follow.
    ///
    /// The owner short-circuits before calling: a modifier-held gesture, an active IME
    /// composition, an open picker and any gesture inside an expanded body that can still
    /// scroll never reach here.
    pub fn gesture(&mut self, gesture: Gesture, cx: &mut Context<Self>) -> bool {
        let (overflows, at_end) = self.geometry();
        let broke = self.follow.gesture(gesture, overflows, at_end);
        if broke {
            self.state.pause_following_tail();
            self.set_jump_visible(true, cx);
            cx.notify();
        }
        broke
    }

    /// Enables or disables transcript scroll mode.
    ///
    /// Entering freezes the tail so new output cannot pull the viewport out from under the
    /// reader, and focuses the row nearest the bottom of the viewport; leaving re-arms follow.
    pub fn scroll_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.scroll_mode = enabled;
        if enabled {
            self.follow.break_follow();
            self.state.pause_following_tail();
            if self.focused_row.is_none() {
                self.focused_row = self
                    .visible
                    .end
                    .checked_sub(1)
                    .filter(|_| !self.rows.is_empty());
            }
            cx.notify();
        } else {
            self.focused_row = None;
            self.scroll_to_latest(cx);
        }
    }

    /// Whether scroll mode is on.
    #[must_use]
    pub fn is_scroll_mode(&self) -> bool {
        self.scroll_mode
    }

    /// Moves the frozen viewport by whole rows (`j` / `k` without a row focus).
    pub fn scroll_rows(&mut self, rows: f32, cx: &mut Context<Self>) {
        let row = cx.theme().metrics.row_h;
        self.state.scroll_by(row * rows);
        cx.notify();
    }

    /// Moves the frozen viewport by a fraction of its own height (half page, page).
    pub fn scroll_viewports(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let height = self.state.viewport_bounds().size.height;
        self.state.scroll_by(height * fraction);
        cx.notify();
    }

    /// Jumps to the oldest row (`gg`).
    pub fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.state.scroll_to(gpui::ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        self.set_jump_visible(true, cx);
        // `gg` lands on item zero by definition, so the question is answerable here rather than
        // one repaint later — which is also what makes it reachable without a laid-out window.
        self.reached_oldest(0, cx);
        cx.notify();
    }

    /// Moves the row focus and scrolls to it (`j` / `k` in scroll mode).
    pub fn focus_row(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        self.focused_row = row.filter(|index| *index < self.rows.len());
        if let Some(index) = self.focused_row {
            self.state.scroll_to_reveal_item(index);
        }
        cx.notify();
    }

    /// Moves the row focus by `delta` rows, clamped to the transcript.
    pub fn move_row_focus(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let current = self.focused_row.unwrap_or(last);
        let next = current.saturating_add_signed(delta).min(last);
        self.focus_row(Some(next), cx);
    }

    /// Which row currently carries the focus ring.
    #[must_use]
    pub fn focused_row(&self) -> Option<usize> {
        self.focused_row
    }

    /// Resolves one bare key against the focused row.
    ///
    /// Row focus lives **inside** scroll mode, which is what finally makes `⏎` / `u` / `o`
    /// fire; outside it there is no focused row and no key is claimed.
    #[must_use]
    pub fn event_for_key(&self, key: &str) -> Option<TranscriptEvent> {
        let index = self.focused_row?;
        let row = self.rows.get(index)?;
        let id = row.id.key();
        let action = match key {
            "enter" => {
                return row
                    .is_expandable()
                    .then(|| TranscriptEvent::Toggle(id.clone()));
            }
            "u" => RowAction::Revert,
            "o" => RowAction::Open,
            "y" => RowAction::Copy,
            "d" => RowAction::Diff,
            _ => return None,
        };
        Some(TranscriptEvent::RowAction { row: id, action })
    }

    // -----------------------------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------------------------

    /// Whether the content overflows the viewport, and whether the viewport is at the live edge.
    ///
    /// The list's own `is_scrolled_to_end` flag is a **fallback, never a short-circuit**: a
    /// 100 px gap with the flag set is still `false`, so geometry decides whenever it is known.
    fn geometry(&self) -> (bool, bool) {
        let viewport = self.state.viewport_bounds().size.height;
        let max = self.state.max_offset_for_scrollbar().y;
        let scroll = -self.state.scroll_px_offset_for_scrollbar().y;
        let overflows = max > px(0.0);
        let at_end = if overflows {
            is_at_end(viewport + max, scroll, viewport)
        } else {
            self.state.is_scrolled_to_end().unwrap_or(true)
        };
        (overflows, at_end)
    }

    fn on_scroll(&mut self, event: &ListScrollEvent, cx: &mut Context<Self>) {
        self.visible = event.visible_range.clone();
        let (_, at_end) = self.geometry();
        self.follow.scrolled(at_end && !self.scroll_mode);
        let following = self.follow.is_following() || self.follow.is_anchoring();
        self.set_jump_visible(!following, cx);
        // Only once the list has actually measured something: an empty or not-yet-laid-out
        // transcript reports `0..0`, and treating that as "the reader is at the top" would ask
        // for a page before the first one is on screen.
        if !self.visible.is_empty() {
            self.reached_oldest(self.visible.start, cx);
        }
        cx.notify();
    }

    /// Asks the owner for older history, at most once per row set.
    ///
    /// Two callers, one rule: the scroll handler, which is how a wheel or a trackpad gets here,
    /// and [`TranscriptList::scroll_to_top`], which lands on item zero by definition and must
    /// not wait for a repaint to say so.
    fn reached_oldest(&mut self, first_visible: usize, cx: &mut Context<Self>) {
        if self.asked_for_older || self.rows.is_empty() || first_visible > OLDEST_PREFETCH_ROWS {
            return;
        }
        self.asked_for_older = true;
        cx.emit(TranscriptEvent::ReachedOldest);
    }

    /// Shows or hides the jump-to-latest chip.
    ///
    /// **Debounced on show only** (`motion.jump_chip_delay`), immediate on hide: a thread
    /// switch fires scroll events with `is_at_end = false` while the initial position settles,
    /// and an undebounced chip flashes through that window.
    fn set_jump_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.jump_visible == visible {
            self._jump_debounce = None;
            return;
        }
        if !visible {
            self.jump_visible = false;
            self._jump_debounce = None;
            cx.notify();
            return;
        }
        let delay = Duration::from_millis(cx.theme().motion.jump_chip_delay);
        self._jump_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            // The update fails only when the transcript itself is gone, which is the one
            // outcome a debounce timer has nothing left to do about.
            this.update(cx, |this, cx| {
                if !this.follow.is_following() && !this.follow.is_anchoring() {
                    this.jump_visible = true;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Starts or stops the 1 Hz clock, and recomputes its label now.
    ///
    /// The clock is not in the model: the row carries `started_at`, the task writes one
    /// [`SharedString`], and `render` reads it. One element repaints per second instead of the
    /// whole tree.
    fn sync_working_clock(&mut self, cx: &mut Context<Self>) {
        let started = self.rows.iter().find_map(|row| match &row.kind {
            TranscriptRowKind::Working(working) => working.started_at,
            _ => None,
        });
        let Some(started) = started else {
            self.working_label = None;
            self._working_tick = None;
            return;
        };
        self.working_label = Some(working_label(started));
        if self._working_tick.is_some() {
            return;
        }
        let tick = Duration::from_millis(cx.theme().motion.working_tick);
        self._working_tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(tick).await;
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(started) = this.rows.iter().find_map(|row| match &row.kind {
                            TranscriptRowKind::Working(working) => working.started_at,
                            _ => None,
                        }) else {
                            this.working_label = None;
                            return false;
                        };
                        let label = working_label(started);
                        if this.working_label.as_ref() != Some(&label) {
                            this.working_label = Some(label);
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(transcript_event) = self.event_for_key(&event.keystroke.key) {
            cx.emit(transcript_event);
            cx.stop_propagation();
        }
    }

    fn render_row(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(index).cloned() else {
            return div().into_any_element();
        };
        let body = self.row_body.clone().and_then(|render| render(&row, cx));
        let this = cx.weak_entity();
        let ctx = RowContext {
            index,
            focused: self.focused_row == Some(index),
            // Before the first scroll event the visible range is empty and every row counts
            // as on screen: the alternative is a first frame with no shimmer anywhere.
            visible: self.visible.is_empty() || self.visible.contains(&index),
            working_label: self.working_label.clone(),
            body,
            toggle: Some(Rc::new(move |key: SharedString, _window, cx| {
                // A click can only arrive while the row is on screen, so the weak handle is
                // live; it fails only if the transcript was dropped in the same frame.
                this.update(cx, |_, cx| cx.emit(TranscriptEvent::Toggle(key)))
                    .ok();
            })),
        };
        row_element(&row, ctx, cx)
    }
}

/// The working row's label, from a start instant. Not called in `render`.
fn working_label(started: Instant) -> SharedString {
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    format_working(elapsed)
}

impl EventEmitter<TranscriptEvent> for TranscriptList {}

impl Focusable for TranscriptList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TranscriptList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let thumb = scroll_thumb(
            -self.state.scroll_px_offset_for_scrollbar().y,
            self.state.max_offset_for_scrollbar().y,
            self.state.viewport_bounds().size.height,
            theme.metrics.diff_thumb_min_h,
        );
        let show_jump = self.jump_visible;

        div()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                // The 16 px inset belongs to the thread column that sets the 760 px measure;
                // adding it again here would indent the transcript out of line with the
                // composer and shrink every docked decision below its fixed width.
                div().size_full().py(theme.space.md).child(
                    list(
                        self.state.clone(),
                        cx.processor(|this: &mut Self, index: usize, window, cx| {
                            this.render_row(index, window, cx)
                        }),
                    )
                    .size_full(),
                ),
            )
            .when_some(thumb, |el, (top, height)| {
                if height >= 1.0 {
                    return el;
                }
                el.child(
                    div()
                        .absolute()
                        .right(AGENT_SCROLLBAR_INSET)
                        .top(gpui::relative(top))
                        .h(gpui::relative(height))
                        .w(theme.metrics.scroll_thumb_w)
                        .bg(theme.colors.scroll_thumb),
                )
            })
            .when(show_jump, |el| {
                el.child(
                    div()
                        .id("transcript-jump-to-latest")
                        .absolute()
                        .bottom(theme.space.md)
                        .right(theme.space.lg)
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _window, cx| this.scroll_to_latest(cx)))
                        .gap(theme.space.xs)
                        .h(theme.metrics.chip_h)
                        .px(theme.space.sm)
                        .rounded(theme.radii.full)
                        .bg(theme.colors.elevated)
                        .shadow(theme.sheet_shadow())
                        .child(
                            Icon::CircleArrowDown
                                .el()
                                .size(IconSize::Small)
                                .color(theme.colors.text_secondary),
                        )
                        .child(Text::hint("jump to latest").tone(Tone::Secondary)),
                )
            })
    }
}
