//! Variable-height, bottom-anchored native-agent transcript.
//!
//! §8 of `NATIVE-AGENTS.md` pins the primitive: GPUI `list`, **not** `uniform_list`, because an
//! assistant paragraph, a diff and a 30 px tool row are not the same height. `list` caches the
//! height it measured for every row, so [`TranscriptList::set_rows`] diffs the projection and
//! splices only what really changed: a streaming turn re-measures its last row and nothing
//! else, which is what keeps a fast model from re-laying-out the whole thread per token.

use std::{ops::Range, rc::Rc};

use gpui::{
    AnyElement, App, Context, ElementId, EventEmitter, FocusHandle, Focusable, FollowMode,
    KeyDownEvent, ListAlignment, ListScrollEvent, ListState, Pixels, SharedString, Window, div,
    list, prelude::*, px,
};

use crate::{
    components::{KeyHint, KeyHintRow, Spinner, markdown},
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

use super::{
    decision_card::{DecisionAction, DecisionCard, DecisionCardElement},
    format::format_thinking,
    metrics::{
        AGENT_BODY_MAX_H, AGENT_CARET_H, AGENT_CONTENT_W, AGENT_LIST_OVERDRAW,
        AGENT_SCROLLBAR_INSET,
    },
    tool_row::{ToolRow, ToolRowElement, expand_hint},
};

use super::super::MarkdownDocument;

/// One projected native-agent transcript row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptRow {
    /// The only transcript block with a surface background.
    UserBlock {
        /// User text.
        text: SharedString,
        /// Attachment display names, drawn as pills under the text.
        attachments: Vec<SharedString>,
    },
    /// Visible assistant Markdown on the ground.
    AssistantText {
        /// Parsed Markdown document.
        markdown: MarkdownDocument,
    },
    /// Collapsible reasoning line.
    Thinking {
        /// Reasoning text, shown only while the row is expanded.
        text: SharedString,
        /// How long the model reasoned, which is what the collapsed line states (§2).
        duration_ms: u64,
        /// Whether the reasoning body is exposed.
        expanded: bool,
    },
    /// Tool call with optionally nested subagent/tool rows.
    ToolRow {
        /// Tool row presentation.
        row: ToolRow,
        /// Nested children in display order.
        children: Vec<TranscriptRow>,
    },
    /// Fold replacing successful completed work.
    WorkedFold {
        /// Duration/count summary.
        text: SharedString,
        /// Whether successful rows are exposed.
        expanded: bool,
    },
    /// Right-aligned completed-turn metadata.
    TurnFooter {
        /// Duration, tokens, and file summary.
        text: SharedString,
    },
    /// Inline human decision card.
    DecisionCard(DecisionCard),
    /// Runtime or turn failure.
    ErrorCard {
        /// Failure detail.
        message: SharedString,
        /// A backoff is counting down, which is progress rather than a failure to act on.
        retrying: bool,
    },
    /// Compaction or resume boundary.
    CheckpointLine {
        /// Boundary copy.
        text: SharedString,
    },
    /// A user-facing provider notice: a config warning, a deprecation, a harness message.
    Notice {
        /// What the provider said.
        text: SharedString,
    },
    /// User message waiting behind active work.
    QueuedMessage {
        /// Queued text.
        text: SharedString,
    },
    /// Empty-thread instruction.
    EmptyState {
        /// Empty-state copy.
        message: SharedString,
    },
}

impl TranscriptRow {
    /// The stable key of the rows that carry one.
    ///
    /// Only tool rows and decision cards are addressable on their own — everything else is
    /// identified by its position and its content, which is exactly what [`diff_rows`] compares.
    #[must_use]
    pub fn id(&self) -> Option<SharedString> {
        match self {
            TranscriptRow::ToolRow { row, .. } => Some(row.id.clone()),
            TranscriptRow::DecisionCard(card) => Some(card.id.clone()),
            _ => None,
        }
    }

    /// Whether `⏎` on this row expands or collapses something.
    #[must_use]
    pub fn is_expandable(&self) -> bool {
        match self {
            TranscriptRow::ToolRow { row, children } => row.has_body() || !children.is_empty(),
            TranscriptRow::Thinking { .. } | TranscriptRow::WorkedFold { .. } => true,
            _ => false,
        }
    }
}

/// The single [`ListState::splice`] that turns one row projection into the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSplice {
    /// The replaced range of old rows.
    pub old_range: Range<usize>,
    /// How many new rows take its place.
    pub count: usize,
}

/// The rows that really changed between two projections, or `None` when nothing did.
///
/// The common head and tail are matched by content, so a row that did not change keeps the
/// height `list` measured for it. Appending one row splices `len..len`; a streaming row
/// splices only itself.
#[must_use]
pub fn diff_rows(old: &[TranscriptRow], new: &[TranscriptRow]) -> Option<RowSplice> {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|(before, after)| before == after)
        .count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let bounded = old.len().min(new.len()) - prefix;
    let suffix = (0..bounded)
        .take_while(|back| old[old.len() - 1 - back] == new[new.len() - 1 - back])
        .count();
    Some(RowSplice {
        old_range: prefix..old.len() - suffix,
        count: new.len() - suffix - prefix,
    })
}

/// `(offset, visible)` for the scroll thumb, or `None` when the transcript fits.
///
/// Both fractions are of the whole content, which is what [`crate::components::Pane`] draws a
/// thumb from, so the transcript's thumb behaves exactly like every other thumb in Fleet.
#[must_use]
pub fn scroll_fraction(offset: Pixels, max: Pixels, viewport: Pixels) -> Option<(f32, f32)> {
    if max <= px(0.0) || viewport <= px(0.0) {
        return None;
    }
    let (offset, max, viewport) = (f32::from(offset), f32::from(max), f32::from(viewport));
    let content = viewport + max;
    Some(((offset / content).clamp(0.0, 1.0), viewport / content))
}

/// What the transcript asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// `⏎` on a focused row: expand or collapse it.
    Toggle(SharedString),
    /// The open decision card claimed a key.
    Decision {
        /// Which card answered.
        card: SharedString,
        /// What it answered with.
        action: DecisionAction,
    },
}

/// Builds the expanded body of one tool row, or declines and lets the row draw its own text.
///
/// `Rc` because every tool row in a frame gets a handle to the same renderer.
pub type ToolBodyRenderer = Rc<dyn Fn(&ToolRow, &mut App) -> Option<AnyElement>>;

/// Stateful variable-height transcript surface.
pub struct TranscriptList {
    rows: Vec<TranscriptRow>,
    state: ListState,
    focus_handle: FocusHandle,
    scroll_mode: bool,
    at_bottom: bool,
    streaming: bool,
    focused_row: Option<usize>,
    tool_body: Option<ToolBodyRenderer>,
}

impl TranscriptList {
    /// Creates an empty transcript entity.
    #[must_use]
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = ListState::new(0, ListAlignment::Bottom, AGENT_LIST_OVERDRAW);
        state.set_follow_mode(FollowMode::Tail);
        state.set_scroll_handler(cx.listener(
            |this: &mut Self, event: &ListScrollEvent, _window, cx| {
                let at_bottom = event.is_following_tail || !event.is_scrolled;
                if this.at_bottom != at_bottom {
                    this.at_bottom = at_bottom;
                    cx.notify();
                }
            },
        ));
        Self {
            rows: Vec::new(),
            state,
            focus_handle: cx.focus_handle(),
            scroll_mode: false,
            at_bottom: true,
            streaming: false,
            focused_row: None,
            tool_body: None,
        }
    }

    /// Installs the renderer for an expanded tool row's body.
    ///
    /// A real inline diff lives in `fleet-lazygit`, which the kit must not depend on (lib.rs
    /// rule 1), so the owner hands the element back per frame instead. Rows the renderer
    /// declines fall back to the row's own diff or output text, which is also what an owner
    /// that never calls this gets.
    pub fn set_tool_body(
        &mut self,
        render: impl Fn(&ToolRow, &mut App) -> Option<AnyElement> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.tool_body = Some(Rc::new(render));
        cx.notify();
    }

    /// Replaces the keyed row projection.
    ///
    /// Only the rows that changed are spliced into the list state, so every untouched row keeps
    /// the height it was measured at.
    pub fn set_rows(&mut self, rows: Vec<TranscriptRow>, cx: &mut Context<Self>) {
        if let Some(splice) = diff_rows(&self.rows, &rows) {
            self.state.splice(splice.old_range, splice.count);
        }
        self.rows = rows;
        if let Some(focused) = self.focused_row
            && focused >= self.rows.len()
        {
            self.focused_row = None;
        }
        cx.notify();
    }

    /// Jumps to the newest bottom-anchored row and resumes following the tail.
    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.scroll_to_end(cx);
        self.state.set_follow_mode(FollowMode::Tail);
    }

    /// Jumps to the newest row without un-freezing the tail (`G` in scroll mode).
    ///
    /// KEYMAP.md documents `G` as "oldest/newest" and `ctrl-s [` as a mode the status bar's
    /// `SCROLL` word tracks "for as long as it is on": re-arming [`FollowMode::Tail`] here made
    /// the transcript follow again while the mode word — and the key context with it — still
    /// said the tail was frozen. Leaving the mode is what resumes the follow.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.state.scroll_to_end();
        self.at_bottom = true;
        cx.notify();
    }

    /// Enables or disables transcript scroll mode.
    ///
    /// Entering the mode freezes the tail so new output cannot pull the viewport out from under
    /// the reader; leaving it returns to the newest row.
    pub fn scroll_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.scroll_mode = enabled;
        if enabled {
            self.state.pause_following_tail();
        } else {
            self.scroll_to_bottom(cx);
        }
        cx.notify();
    }

    /// Whether scroll mode is on.
    #[must_use]
    pub fn is_scroll_mode(&self) -> bool {
        self.scroll_mode
    }

    /// Moves the frozen viewport by whole rows (`j` / `k`).
    ///
    /// Freezing the tail is only half of a scroll mode: §9 gives it the same `j`/`k`, half/page
    /// and `gg`/`G` vocabulary the terminal one has, and none of it can reach the list without
    /// a way to move the viewport from outside.
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
            offset_in_item: gpui::px(0.0),
        });
        cx.notify();
    }

    /// Whether the transcript is showing its newest row.
    #[must_use]
    pub fn is_at_bottom(&self) -> bool {
        self.at_bottom
    }

    /// Draw the streaming caret after the last assistant paragraph.
    pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {
        if self.streaming != streaming {
            self.streaming = streaming;
            cx.notify();
        }
    }

    /// Move the row focus, which is what `⏎` expands.
    pub fn focus_row(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        self.focused_row = row.filter(|index| *index < self.rows.len());
        cx.notify();
    }

    /// Which row currently carries the focus ring.
    #[must_use]
    pub fn focused_row(&self) -> Option<usize> {
        self.focused_row
    }

    /// Returns the current presentation rows.
    #[must_use]
    pub fn rows(&self) -> &[TranscriptRow] {
        &self.rows
    }

    /// Returns the focus handle the transcript routes bare keys through.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The open decision card, which §2 always keeps as the last row.
    #[must_use]
    pub fn open_decision(&self) -> Option<&DecisionCard> {
        match self.rows.last() {
            Some(TranscriptRow::DecisionCard(card)) => Some(card),
            _ => None,
        }
    }

    /// Resolves one bare key against the open card first, then the focused row.
    ///
    /// §2: while a card is open bare keys route to the card, so a `y` never toggles a row
    /// behind an unanswered permission.
    #[must_use]
    pub fn event_for_key(&self, key: &str) -> Option<TranscriptEvent> {
        if let Some(card) = self.open_decision()
            && let Some(action) = card.action_for_key(key)
        {
            return Some(TranscriptEvent::Decision {
                card: card.id.clone(),
                action,
            });
        }
        if key != "enter" {
            return None;
        }
        let index = self.focused_row?;
        let row = self.rows.get(index)?;
        row.is_expandable()
            .then(|| TranscriptEvent::Toggle(row.id().unwrap_or_else(|| row_key(index))))
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
        let focused = self.focused_row == Some(index);
        let streaming = self.streaming && index + 1 == self.rows.len();
        let theme = cx.theme().clone();

        let content = match row {
            TranscriptRow::UserBlock { text, attachments } => {
                user_block_with(&text, &attachments, false, cx)
            }
            TranscriptRow::QueuedMessage { text } => user_block(&text, true, cx),
            TranscriptRow::AssistantText { markdown: document } => div()
                .flex()
                .items_end()
                .gap(theme.space.xs)
                .child(div().flex_1().min_w_0().child(markdown(&document, cx)))
                .when(streaming, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .w(theme.metrics.cell_w)
                            .h(AGENT_CARET_H)
                            .bg(theme.colors.accent),
                    )
                })
                .into_any_element(),
            TranscriptRow::Thinking {
                text,
                duration_ms,
                expanded,
            } => {
                let this = cx.weak_entity();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(ElementId::from(SharedString::from(format!(
                                "thinking-row-{index}"
                            ))))
                            .h(theme.metrics.strip_h)
                            .flex()
                            .items_center()
                            .gap(theme.space.sm)
                            .on_click(move |_, _window, cx| {
                                let key = row_key(index);
                                this.update(cx, |_, cx| cx.emit(TranscriptEvent::Toggle(key)))
                                    .ok();
                            })
                            .child(Text::ui(format_thinking(duration_ms)).muted().ellipsize())
                            .child(expand_hint(expanded)),
                    )
                    .when(expanded, |el| {
                        el.child(
                            div()
                                .id(ElementId::from(SharedString::from(format!(
                                    "thinking-{index}"
                                ))))
                                .max_h(AGENT_BODY_MAX_H)
                                .overflow_y_scroll()
                                .child(Text::data_small(text).muted()),
                        )
                    })
                    .into_any_element()
            }
            TranscriptRow::ToolRow { row, children } => {
                let tool_body = self.tool_body.clone();
                let toggle = cx.weak_entity();
                let rendered = children
                    .iter()
                    .map(|child| render_nested(child, tool_body.as_ref(), &toggle, cx))
                    .collect::<Vec<_>>();
                let id = row.id.clone();
                let this = cx.weak_entity();
                let body = tool_body.and_then(|render| render(&row, cx));
                let mut element = ToolRowElement::new(row)
                    .focused(focused)
                    .children(rendered)
                    .on_toggle(move |_window, cx| {
                        let id = id.clone();
                        this.update(cx, |_, cx| cx.emit(TranscriptEvent::Toggle(id)))
                            .ok();
                    });
                if let Some(body) = body {
                    element = element.body(body);
                }
                element.into_any_element()
            }
            TranscriptRow::WorkedFold { text, expanded } => {
                let this = cx.weak_entity();
                div()
                    .id(ElementId::from(SharedString::from(format!("fold-{index}"))))
                    .h(theme.metrics.strip_h)
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .on_click(move |_, _window, cx| {
                        let key = row_key(index);
                        this.update(cx, |_, cx| cx.emit(TranscriptEvent::Toggle(key)))
                            .ok();
                    })
                    .child(Text::ui(text).muted().ellipsize())
                    .child(expand_hint(expanded))
                    .into_any_element()
            }
            // §10: the turn-level diff surface did not land, and DESIGN-SYSTEM §4 does not list
            // an invalid command — neither `[⏎] diff` nor `[u] revert turn` is drawn. `u` is
            // declared only in `Agent > AgentRow`, a context nothing enters until row focus
            // lands, and the footer has no click handler, so the keycap named a key that could
            // not fire by keyboard or by mouse. Both hints return with the surfaces they name.
            TranscriptRow::TurnFooter { text } => div()
                .flex()
                .items_center()
                .justify_end()
                .gap(theme.space.sm)
                .child(Text::hint(text).faint())
                .into_any_element(),
            TranscriptRow::DecisionCard(card) => {
                let id = card.id.clone();
                let expanded = card.expanded;
                // DESIGN-SYSTEM §3: selection is a background. Without this the options `1`-`4`
                // and `space` choose draw exactly like the ones they did not.
                let selected = card.selected.clone();
                let this = cx.weak_entity();
                DecisionCardElement::new(card)
                    .expanded(expanded)
                    .selected(selected)
                    .on_action(move |action, _window, cx| {
                        let card = id.clone();
                        this.update(cx, |_, cx| {
                            cx.emit(TranscriptEvent::Decision { card, action });
                        })
                        .ok();
                    })
                    .into_any_element()
            }
            TranscriptRow::ErrorCard { message, retrying } => error_card(&message, retrying, cx),
            TranscriptRow::CheckpointLine { text } => div()
                .flex()
                .items_center()
                .gap(theme.space.md)
                .child(hairline(&theme))
                .child(Text::hint(text).faint().flex_none())
                .child(hairline(&theme))
                .into_any_element(),
            // §3.2 makes a notice user-facing and §2 makes amber "needs you or cannot verify",
            // which is exactly what a harness warning is: shown, not shouted.
            TranscriptRow::Notice { text } => div()
                .flex()
                .items_start()
                .gap(theme.space.sm)
                .child(
                    Icon::TriangleAlert
                        .el()
                        .size(IconSize::Small)
                        .tone(Tone::Warning),
                )
                .child(div().flex_1().min_w_0().child(Text::ui(text).muted()))
                .into_any_element(),
            TranscriptRow::EmptyState { message } => div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(theme.space.sm)
                .py(theme.space.xxl)
                .child(Text::ui_strong(message))
                .child(
                    KeyHintRow::new()
                        .key("⏎", "send your first message")
                        .key("⇧⇥", "plan mode first"),
                )
                .child(
                    KeyHintRow::new()
                        .key("@", "mention a file")
                        .key("/", "commands"),
                )
                .into_any_element(),
        };

        div()
            .w_full()
            .flex()
            .py(theme.space.xs)
            .child(div().w_full().max_w(AGENT_CONTENT_W).child(content))
            .into_any_element()
    }
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
        let count = self.rows.len();
        if self.state.item_count() != count {
            self.state.reset(count);
        }

        let thumb = scroll_fraction(
            -self.state.scroll_px_offset_for_scrollbar().y,
            self.state.max_offset_for_scrollbar().y,
            self.state.viewport_bounds().size.height,
        );
        let show_jump = !self.at_bottom;

        div()
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                // §2's 16 px inset belongs to the thread column that sets the 760 px measure;
                // adding it again here would indent the transcript out of line with the
                // composer and shrink every decision card below its fixed width.
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
            .when_some(thumb, |el, (offset, visible)| {
                if visible >= 1.0 {
                    return el;
                }
                let height = visible.max(0.04);
                let top = offset.min(1.0 - height);
                el.child(
                    div()
                        .absolute()
                        .right(AGENT_SCROLLBAR_INSET)
                        .top(gpui::relative(top))
                        .h(gpui::relative(height))
                        .w(theme.metrics.scroll_thumb_w)
                        // DESIGN-SYSTEM §2.1 names `scroll_thumb` for the 3 px pane-edge thumb,
                        // which is what `Pane` paints: a look-alike token drifts the moment a
                        // theme moves one of them.
                        .bg(theme.colors.scroll_thumb),
                )
            })
            .when(show_jump, |el| {
                el.child(
                    // §10 makes the click the interim substitute for row focus: outside scroll
                    // mode `G` is not bound, so without this the chip would state a capability
                    // the reader has no way to invoke (DESIGN-SYSTEM §4).
                    div()
                        .id("transcript-jump-to-latest")
                        .absolute()
                        .bottom(theme.space.md)
                        .right(theme.space.lg)
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _window, cx| this.scroll_to_bottom(cx)))
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

/// A stable synthetic key for the rows that carry no id of their own.
fn row_key(index: usize) -> SharedString {
    SharedString::from(format!("row-{index}"))
}

fn hairline(theme: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .h(theme.metrics.hairline)
        .bg(theme.colors.border)
}

/// Renders one nested row of a subagent call.
///
/// The nested rows carry the same toggle as the top-level ones: §5 gives an `Agent` row a
/// children region, and until row focus lands (§10) a click is the only way to open anything.
fn render_nested(
    row: &TranscriptRow,
    tool_body: Option<&ToolBodyRenderer>,
    toggle: &gpui::WeakEntity<TranscriptList>,
    cx: &mut App,
) -> AnyElement {
    match row {
        TranscriptRow::ToolRow { row, children } => {
            let rendered = children
                .iter()
                .map(|child| render_nested(child, tool_body, toggle, cx))
                .collect::<Vec<_>>();
            let body = tool_body.and_then(|render| render(row, cx));
            let id = row.id.clone();
            let list = toggle.clone();
            let mut element = ToolRowElement::new(row.clone())
                .children(rendered)
                .on_toggle(move |_window, cx| {
                    let id = id.clone();
                    list.update(cx, |_, cx| cx.emit(TranscriptEvent::Toggle(id)))
                        .ok();
                });
            if let Some(body) = body {
                element = element.body(body);
            }
            element.into_any_element()
        }
        TranscriptRow::ErrorCard { message, retrying } => error_card(message, *retrying, cx),
        // A subagent's prose is a document, not a `text` field: reading one off this row leaves
        // an empty line where §5 promised the nested children region would show its work.
        TranscriptRow::AssistantText { markdown: document } => {
            markdown(document, cx).into_any_element()
        }
        TranscriptRow::Thinking { duration_ms, .. } => {
            Text::data_small(format_thinking(*duration_ms))
                .muted()
                .into_any_element()
        }
        other => {
            let text = match other {
                TranscriptRow::UserBlock { text, .. }
                | TranscriptRow::QueuedMessage { text }
                | TranscriptRow::WorkedFold { text, .. }
                | TranscriptRow::TurnFooter { text }
                | TranscriptRow::CheckpointLine { text }
                | TranscriptRow::Notice { text } => text.clone(),
                TranscriptRow::EmptyState { message } => message.clone(),
                _ => SharedString::default(),
            };
            Text::data_small(text).muted().into_any_element()
        }
    }
}

/// The user turn: the only transcript block with a background (§2).
///
/// `queued` is the 60 % variant a message waiting behind active work is drawn in, with the key
/// that takes it back.
#[must_use]
pub fn user_block(text: &SharedString, queued: bool, cx: &App) -> AnyElement {
    user_block_with(text, &[], queued, cx)
}

/// The user turn with the attachments it was sent with, each as a neutral pill (§2).
#[must_use]
pub fn user_block_with(
    text: &SharedString,
    attachments: &[SharedString],
    queued: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .w_full()
        .rounded(theme.radii.md)
        .bg(theme.colors.surface)
        .py(theme.space.sm)
        .px(theme.space.md)
        .flex()
        .flex_col()
        .gap(theme.space.xs)
        .when(queued, |el| el.opacity(theme.metrics.refreshing_opacity))
        .child(Text::ui(text.clone()))
        .when(!attachments.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.space.xs)
                    .children(attachments.iter().map(|name| attachment_pill(name, cx))),
            )
        })
        .when(queued, |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(Text::hint("queued").faint())
                    .child(KeyHint::labeled("esc", "unqueue")),
            )
        })
        .into_any_element()
}

/// A neutral attachment pill, drawn next to the user text that carries it.
#[must_use]
pub fn attachment_pill(name: &SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .h(theme.metrics.chip_h)
        .px(theme.space.sm)
        .flex()
        .items_center()
        .rounded(theme.radii.sm)
        .bg(theme
            .colors
            .text
            .opacity(theme.metrics.neutral_fill_opacity))
        .child(Text::hint(name.clone()).tone(Tone::Secondary))
        .into_any_element()
}

/// A 28 px error card: a 2 px red bar, the failure, and the key that retries it.
///
/// `retrying` swaps the red cross for the gray spinner, because a backoff that is still
/// counting down is progress, not a failure the reader has to act on.
#[must_use]
pub fn error_card(message: &SharedString, retrying: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let glyph = if retrying {
        Spinner::new(ElementId::from(SharedString::from(format!(
            "error-retry-{message}"
        ))))
        .size(IconSize::Small)
        .tone(Tone::Secondary)
        .into_any_element()
    } else {
        Icon::CircleX
            .el()
            .size(IconSize::Small)
            .tone(Tone::Danger)
            .into_any_element()
    };
    div()
        .w_full()
        .max_w(AGENT_CONTENT_W)
        .min_h(theme.metrics.banner_h)
        .flex()
        .items_stretch()
        .rounded(theme.radii.sm)
        .overflow_hidden()
        .bg(theme.colors.surface)
        .child(
            div()
                .flex_none()
                .w(theme.metrics.focus_ring_w)
                .bg(theme.colors.danger),
        )
        .child(
            // §5 caps tool bodies, not error cards: a provider error, a turn error message or an
            // OpenCode `session.error` body is the text §11 relies on to surface protocol drift,
            // so the card grows to the body cap and scrolls rather than ellipsizing one line.
            div()
                .id(ElementId::from(SharedString::from(format!(
                    "error-card-{message}"
                ))))
                .flex_1()
                .min_w_0()
                .px(theme.space.md)
                .py(theme.space.xs)
                .max_h(AGENT_BODY_MAX_H)
                .overflow_y_scroll()
                .flex()
                .items_start()
                .gap(theme.space.sm)
                .child(div().flex_none().pt(theme.space.xxs).child(glyph))
                .child(div().flex_1().min_w_0().child(Text::ui(message.clone()))),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::agent::{
        decision_card::{DecisionCardKind, permission_actions},
        tool_row::ToolRowState,
    };

    fn text_row(text: &str) -> TranscriptRow {
        TranscriptRow::UserBlock {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    fn permission_row() -> TranscriptRow {
        TranscriptRow::DecisionCard(
            DecisionCard::new(
                "gate",
                "claude wants to run a command",
                DecisionCardKind::Permission {
                    tool: "bash".into(),
                    payload: "cargo test".into(),
                    rationale: None,
                },
            )
            .actions(permission_actions(
                DecisionAction::AllowSession,
                "allow for this session",
            )),
        )
    }

    fn expandable() -> TranscriptRow {
        TranscriptRow::ToolRow {
            row: ToolRow::new("t1", "bash", "cargo test").output("exit 0"),
            children: Vec::new(),
        }
    }

    fn transcript(
        cx: &mut gpui::TestAppContext,
        rows: Vec<TranscriptRow>,
    ) -> gpui::Entity<TranscriptList> {
        cx.update(|cx| cx.set_global(crate::theme::Theme::dark()));
        let list = cx.update(|cx| cx.new(TranscriptList::new));
        cx.update(|cx| {
            list.update(cx, |list, cx| {
                list.set_rows(rows, cx);
            });
        });
        list
    }

    #[gpui::test]
    fn set_rows_keeps_the_list_state_in_step_with_the_projection(cx: &mut gpui::TestAppContext) {
        let list = transcript(cx, vec![text_row("a"), expandable()]);
        list.read_with(cx, |list, _| {
            assert_eq!(list.rows().len(), 2);
            assert_eq!(list.state.item_count(), 2);
        });
        cx.update(|cx| {
            list.update(cx, |list, cx| {
                list.set_rows(vec![text_row("a")], cx);
            });
        });
        list.read_with(cx, |list, _| {
            assert_eq!(list.rows().len(), 1);
            assert_eq!(list.state.item_count(), 1);
            assert_eq!(list.focused_row(), None);
        });
    }

    /// UX-5: `G` is documented as "newest", not as a way out of scroll mode — the mode word
    /// stays on until `q`/`i`/`esc`, and so does the frozen tail.
    #[gpui::test]
    fn jumping_to_the_newest_row_does_not_thaw_the_frozen_tail(cx: &mut gpui::TestAppContext) {
        let list = transcript(cx, vec![text_row("a"), text_row("b")]);
        cx.update(|cx| {
            list.update(cx, |list, cx| list.scroll_mode(true, cx));
        });
        list.read_with(cx, |list, _| {
            assert!(list.is_scroll_mode());
            assert!(!list.state.is_following_tail());
        });

        cx.update(|cx| {
            list.update(cx, |list, cx| list.scroll_to_end(cx));
        });
        list.read_with(cx, |list, _| {
            assert!(list.is_scroll_mode());
            assert!(
                !list.state.is_following_tail(),
                "`G` re-armed the tail while the status bar still read SCROLL"
            );
        });

        // Leaving the mode is what resumes the follow.
        cx.update(|cx| {
            list.update(cx, |list, cx| list.scroll_mode(false, cx));
        });
        list.read_with(cx, |list, _| {
            assert!(!list.is_scroll_mode());
            assert!(list.state.is_following_tail());
        });
    }

    #[gpui::test]
    fn the_tool_body_renderer_sees_every_row_including_nested_ones(cx: &mut gpui::TestAppContext) {
        let nested = TranscriptRow::ToolRow {
            row: ToolRow::new("child", "edit", "src/lib.rs")
                .diff("@@ -1 +1 @@")
                .expanded(true),
            children: Vec::new(),
        };
        let parent = TranscriptRow::ToolRow {
            row: ToolRow::new("parent", "agent", "explore").expanded(true),
            children: vec![nested],
        };
        cx.update(|cx| cx.set_global(crate::theme::Theme::dark()));
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorder = std::rc::Rc::clone(&seen);
        let window = cx
            .update(|cx| {
                cx.open_window(Default::default(), |_, cx| {
                    cx.new(|cx| {
                        let mut list = TranscriptList::new(cx);
                        list.set_rows(vec![parent], cx);
                        list.set_tool_body(
                            move |row, _cx| {
                                recorder.borrow_mut().push(row.id.clone());
                                // Declining leaves the row's own diff text in place.
                                None
                            },
                            cx,
                        );
                        list
                    })
                })
            })
            .expect("test window");
        let cx = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        // Children are built before the row that nests them.
        assert_eq!(*seen.borrow(), vec!["child", "parent"]);
    }

    #[gpui::test]
    fn an_open_card_takes_every_bare_key_before_the_focused_row(cx: &mut gpui::TestAppContext) {
        let list = transcript(cx, vec![expandable(), permission_row()]);
        cx.update(|cx| {
            list.update(cx, |list, cx| list.focus_row(Some(0), cx));
        });
        list.read_with(cx, |list, _| {
            assert_eq!(
                list.event_for_key("y"),
                Some(TranscriptEvent::Decision {
                    card: "gate".into(),
                    action: DecisionAction::AllowOnce,
                })
            );
            // `⏎` is not a permission key, so it falls through to the focused row.
            assert_eq!(
                list.event_for_key("enter"),
                Some(TranscriptEvent::Toggle("t1".into()))
            );
            assert_eq!(list.event_for_key("z"), None);
        });
    }

    #[gpui::test]
    fn enter_only_toggles_a_row_that_has_something_to_show(cx: &mut gpui::TestAppContext) {
        let list = transcript(cx, vec![text_row("a"), expandable()]);
        cx.update(|cx| {
            list.update(cx, |list, cx| list.focus_row(Some(0), cx));
        });
        list.read_with(cx, |list, _| assert_eq!(list.event_for_key("enter"), None));
        cx.update(|cx| {
            list.update(cx, |list, cx| list.focus_row(Some(1), cx));
        });
        list.read_with(cx, |list, _| {
            assert_eq!(
                list.event_for_key("enter"),
                Some(TranscriptEvent::Toggle("t1".into()))
            );
        });
    }

    fn tool(id: &str, summary: &str) -> TranscriptRow {
        TranscriptRow::ToolRow {
            row: ToolRow::new(id, "bash", summary),
            children: Vec::new(),
        }
    }

    #[test]
    fn an_unchanged_projection_splices_nothing() {
        let rows = vec![text_row("a"), tool("t1", "cargo test")];
        assert_eq!(diff_rows(&rows, &rows), None);
    }

    #[test]
    fn appending_splices_only_the_new_tail() {
        let old = vec![text_row("a"), text_row("b")];
        let mut new = old.clone();
        new.push(text_row("c"));
        assert_eq!(
            diff_rows(&old, &new),
            Some(RowSplice {
                old_range: 2..2,
                count: 1
            })
        );
    }

    #[test]
    fn a_streaming_row_splices_only_itself() {
        let old = vec![text_row("a"), tool("t1", "cargo t")];
        let new = vec![text_row("a"), tool("t1", "cargo test")];
        assert_eq!(
            diff_rows(&old, &new),
            Some(RowSplice {
                old_range: 1..2,
                count: 1
            })
        );
    }

    #[test]
    fn a_middle_change_keeps_the_rows_around_it_measured() {
        let old = vec![text_row("a"), tool("t1", "one"), text_row("c")];
        let new = vec![text_row("a"), tool("t1", "two"), text_row("c")];
        assert_eq!(
            diff_rows(&old, &new),
            Some(RowSplice {
                old_range: 1..2,
                count: 1
            })
        );
    }

    #[test]
    fn removing_a_row_splices_only_that_index() {
        let old = vec![text_row("a"), text_row("b"), text_row("c")];
        let new = vec![text_row("a"), text_row("c")];
        assert_eq!(
            diff_rows(&old, &new),
            Some(RowSplice {
                old_range: 1..2,
                count: 0
            })
        );
    }

    #[test]
    fn a_state_change_on_a_tool_row_keeps_its_id() {
        let running = tool("t1", "cargo test");
        let TranscriptRow::ToolRow { row, children } = running.clone() else {
            unreachable!("constructed a tool row")
        };
        let done = TranscriptRow::ToolRow {
            row: row.state(ToolRowState::Done).result("exit 0 · 1.2s"),
            children,
        };
        assert_eq!(running.id(), done.id());
        assert_eq!(running.id().as_deref(), Some("t1"));
        assert_eq!(
            diff_rows(&[running], &[done]),
            Some(RowSplice {
                old_range: 0..1,
                count: 1
            })
        );
    }

    #[test]
    fn only_rows_with_something_underneath_expand() {
        assert!(!tool("t1", "cargo test").is_expandable());
        assert!(
            TranscriptRow::ToolRow {
                row: ToolRow::new("t1", "bash", "cargo test").output("ok"),
                children: Vec::new(),
            }
            .is_expandable()
        );
        assert!(
            TranscriptRow::Thinking {
                text: "the spec says half-up".into(),
                duration_ms: 6_000,
                expanded: false
            }
            .is_expandable()
        );
        assert!(!text_row("a").is_expandable());
    }

    #[test]
    fn the_thumb_disappears_when_the_transcript_fits() {
        assert_eq!(scroll_fraction(px(0.0), px(0.0), px(400.0)), None);
        assert_eq!(scroll_fraction(px(0.0), px(100.0), px(0.0)), None);
        let (offset, visible) =
            scroll_fraction(px(100.0), px(100.0), px(300.0)).expect("a scrollable transcript");
        assert!((visible - 0.75).abs() < f32::EPSILON);
        assert!((offset - 0.25).abs() < f32::EPSILON);
    }
}
