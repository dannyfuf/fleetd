//! Terminal attachment, ordered input, geometry, and selection state shared by both surfaces.
//!
//! Each screen resolves its own target and owns the queue input waits in while that terminal
//! attaches; `route_key`, `route_paste` and `route_copy` decide what follows from a resolved
//! target, so both surfaces answer the same gesture the same way.

use super::*;
use crate::{bridge::Bridge, state::AppState};
use fleet_core::ids::TerminalId;
use fleet_proto::{
    request::RequestBody,
    terminal::{Key, KeyAction, KeyEvent, Modifiers, WheelEvent},
};
use fleet_ui_kit::{ActiveTheme, CellMetrics, Icon};
use gpui::{App, Entity, Keystroke, ScrollWheelEvent, Task};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    future::{Future, poll_fn},
    pin::pin,
    rc::Rc,
    task::Poll,
    time::{Duration, Instant},
};

pub(crate) const FALLBACK_GRID: (u16, u16) = (80, 24);
const MOUSE_ROW_CACHE_CAP: usize = 5_000;
const SELECTION_LINE_CAP: usize = MOUSE_ROW_CACHE_CAP;
pub(crate) const PENDING_INPUT_EVENT_CAP: usize = 1_024;
pub(crate) const PENDING_INPUT_BYTE_CAP: usize = 1024 * 1024;
const INPUT_REJECTION_NOTICE_INTERVAL: Duration = Duration::from_secs(1);
const INPUT_REJECTION_NOTICE: &str = "input dropped while attaching";
const ATTACH_TIMEOUT: Duration = Duration::from_secs(6);
const ATTACH_RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
pub(crate) struct MouseSelection {
    pub(crate) anchor: AbsoluteCellPoint,
    pub(crate) head: AbsoluteCellPoint,
    pub(crate) initial: AbsoluteCellSelection,
    pub(crate) initiating: AbsoluteCellPoint,
    pub(crate) granularity: SelectionGranularity,
    pub(crate) history_epoch: u64,
    pub(crate) cols: u16,
    pub(crate) alt_screen: bool,
    pub(crate) dragging: bool,
    pub(crate) selected: bool,
}

impl MouseSelection {
    pub(crate) fn extend(
        &mut self,
        next: AbsoluteCellSelection,
        position: AbsoluteCellPoint,
    ) -> bool {
        let selected =
            self.granularity != SelectionGranularity::Cell || position != self.initiating;
        let changed =
            self.anchor != next.start || self.head != next.end || self.selected != selected;
        self.anchor = next.start;
        self.head = next.end;
        self.selected = selected;
        changed
    }
}

#[derive(Debug)]
pub(crate) struct TerminalRowCache {
    pub(crate) cols: u16,
    pub(crate) alt_screen: bool,
    pub(crate) history_epoch: u64,
    pub(crate) last_seq: u64,
    pub(crate) last_viewport_base: u64,
    pub(crate) rows: BTreeMap<u64, CachedGridRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingInput {
    Key(KeyEvent),
    Paste(String),
}
impl PendingInput {
    pub(crate) fn into_request(self, terminal: TerminalId) -> RequestBody {
        match self {
            Self::Key(key) => RequestBody::TerminalKey { terminal, key },
            Self::Paste(text) => RequestBody::PasteTerminal { terminal, text },
        }
    }

    fn retained_bytes(&self) -> usize {
        match self {
            Self::Key(key) => key.text.as_ref().map_or(1, String::len),
            Self::Paste(text) => text.len(),
        }
    }
}

#[derive(Default)]
pub(crate) struct TerminalSurface<S> {
    pub(crate) state: S,
    pub(crate) attached: Option<TerminalId>,
    pub(crate) attached_generation: u64,
    attachment_attempt: u64,
    attach_retry_at: Option<Instant>,
    pub(crate) sizes: HashMap<TerminalId, (u16, u16)>,
    pub(crate) area: Bounds<Pixels>,
    pub(crate) padding: Option<Pixels>,
    pub(crate) geometry: Option<(TerminalId, Bounds<Pixels>, CellMetrics)>,
    pub(crate) wheel: WheelAccumulator,
    pub(crate) pending: Vec<PendingInput>,
    pub(crate) anchor: Option<u64>,
    pub(crate) caret: u64,
    pub(crate) anchor_history_epoch: Option<u64>,
    pub(crate) anchor_cols: Option<u16>,
    pub(crate) anchor_alt_screen: Option<bool>,
    pub(crate) history: BTreeMap<u64, CachedGridRow>,
    pub(crate) history_frame: Option<(u64, u64, u64, u16, u16)>,
    pub(crate) mouse_selection: Option<MouseSelection>,
    pub(crate) row_caches: HashMap<TerminalId, TerminalRowCache>,
    pub(crate) hint: PrefixHintState,
    pub(crate) presentation: TerminalPresentation,
}

impl<S> TerminalSurface<S> {
    pub(crate) fn grid_padding(&self) -> Pixels {
        self.padding
            .unwrap_or_else(|| fleet_ui_kit::theme::Spacing::default().sm)
    }

    pub(crate) fn selection(&self, grid: &MirrorGrid) -> Option<GridSelection> {
        self.mouse_selection
            .filter(|selection| selection.selected)
            .and_then(|selection| viewport_cell_selection(grid, selection.anchor, selection.head))
            .or_else(|| {
                self.anchor
                    .and_then(|anchor| line_selection(grid, anchor, self.caret))
            })
    }

    pub(crate) fn grid(
        &self,
        id: &'static str,
        grid: &MirrorGrid,
        focused: bool,
    ) -> fleet_ui_kit::TerminalGrid {
        fleet_ui_kit::TerminalGrid::from_shared(self.presentation.snapshot())
            .cache(&self.presentation.paint)
            .id(id)
            .cursor(grid_cursor(grid, focused))
            .focused(focused)
            .modes(grid_modes(&grid.modes))
            .padding(self.grid_padding())
            .scrollback(grid.viewport.offset, grid.viewport.scrollback_len)
            .frame_size(usize::from(grid.cols), usize::from(grid.rows))
    }

    pub(crate) fn size_for(&self, terminal: TerminalId, cell: Size<Pixels>) -> (u16, u16) {
        if self.area.size.width <= px(0.0) || self.area.size.height <= px(0.0) {
            return self.sizes.get(&terminal).copied().unwrap_or(FALLBACK_GRID);
        }
        grid_size(self.area.size, cell, self.grid_padding())
    }

    pub(crate) fn clear_selections(&mut self) -> bool {
        let mouse = self.mouse_selection.take().is_some();
        self.clear_line_selection() || mouse
    }

    /// Drops the Scroll-mode selection and its retained text, leaving a mouse selection alone.
    pub(crate) fn clear_line_selection(&mut self) -> bool {
        let anchored = self.anchor.take().is_some();
        self.anchor_history_epoch = None;
        self.anchor_cols = None;
        self.anchor_alt_screen = None;
        self.history.clear();
        self.history_frame = None;
        self.row_caches.clear();
        anchored
    }

    /// Anchors a Scroll-mode selection on the caret, clamped onto the visible viewport.
    ///
    /// A caret that has never moved is still at line 0; anchoring there would anchor the oldest
    /// line in the scrollback rather than the one on screen.
    pub(crate) fn start_line_selection(&mut self, grid: &MirrorGrid) {
        let base = viewport_base(grid);
        if grid.rows > 0 {
            self.caret = self.caret.clamp(base, base + u64::from(grid.rows - 1));
        }
        self.anchor = Some(self.caret);
        self.anchor_history_epoch = Some(grid.viewport.history_epoch);
        self.anchor_cols = Some(grid.cols);
        self.anchor_alt_screen = Some(grid.modes.alt_screen);
        self.history.clear();
    }

    /// Moves the caret by whole lines, saturating at both ends of the scrollback.
    pub(crate) fn shift_caret(&mut self, lines: i32) {
        self.caret = if lines >= 0 {
            self.caret.saturating_add(lines.unsigned_abs().into())
        } else {
            self.caret.saturating_sub(lines.unsigned_abs().into())
        };
    }

    /// Replaces any selection with a fresh drag anchored on `initial`.
    pub(crate) fn begin_mouse_selection(
        &mut self,
        initial: AbsoluteCellSelection,
        cell: MouseCell,
        granularity: SelectionGranularity,
    ) {
        self.clear_line_selection();
        self.mouse_selection = Some(MouseSelection {
            anchor: initial.start,
            head: initial.end,
            initial,
            initiating: cell.absolute,
            granularity,
            history_epoch: cell.history_epoch,
            cols: cell.cols,
            alt_screen: cell.alt_screen,
            dragging: true,
            selected: granularity != SelectionGranularity::Cell,
        });
    }

    /// Extends the drag to `cell`, returning whether the surface needs repainting.
    ///
    /// A drag is dropped rather than retargeted when the terminal's coordinate space moved
    /// underneath it: a resize, a screen switch, or a rebased history epoch.
    pub(crate) fn extend_mouse_selection(&mut self, grid: &MirrorGrid, cell: MouseCell) -> bool {
        let Some(selection) = self.mouse_selection else {
            return false;
        };
        if selection.cols != cell.cols
            || selection.alt_screen != cell.alt_screen
            || selection.history_epoch != cell.history_epoch
        {
            self.mouse_selection = None;
            self.row_caches.clear();
            return true;
        }
        let Some(next) = extend_absolute_selection(
            grid,
            selection.initial,
            selection.initiating,
            cell.viewport,
            selection.granularity,
        ) else {
            return false;
        };
        self.mouse_selection
            .as_mut()
            .is_some_and(|selection| selection.extend(next, cell.absolute))
    }

    /// Moves the caret inside the viewport, returning whether it moved at all.
    ///
    /// The caret is an absolute scrollback line, so the viewport it must stay inside is passed
    /// in: `base` is the line the top row shows and `rows` how many are on screen. Returning
    /// `false` at either edge is what makes `j` / `k` scroll the viewport instead.
    pub(crate) fn move_caret_within(&mut self, delta: i32, base: u64, rows: u16) -> bool {
        if rows == 0 {
            return false;
        }
        let bottom = base + u64::from(rows - 1);
        let current = self.caret.clamp(base, bottom);
        self.caret = current;
        self.shift_caret(delta);
        self.caret = self.caret.clamp(base, bottom);
        self.caret != current
    }

    /// Ends a drag and reports whether it left text selected; `None` when there is no selection.
    pub(crate) fn settle_selection(&mut self) -> Option<bool> {
        let selection = self.mouse_selection.as_mut()?;
        selection.dragging = false;
        let selected = selection.selected;
        if !selected {
            self.mouse_selection = None;
            self.row_caches.clear();
        }
        Some(selected)
    }

    /// Drops a drag still in progress, which is how a stale surface gives up its release.
    pub(crate) fn cancel_drag(&mut self) -> bool {
        if !self
            .mouse_selection
            .is_some_and(|selection| selection.dragging)
        {
            return false;
        }
        self.mouse_selection = None;
        self.row_caches.clear();
        true
    }

    pub(crate) fn detach(&mut self, bridge: &Bridge, preserve: Option<TerminalId>) {
        self.attachment_attempt = self.attachment_attempt.wrapping_add(1);
        self.attach_retry_at = None;
        if let Some(terminal) = self.attached.take()
            && Some(terminal) != preserve
        {
            bridge.send(RequestBody::DetachTerminal { terminal });
        }
    }

    #[must_use = "rejected input must be surfaced to the user"]
    pub(crate) fn send_or_queue(
        &mut self,
        bridge: &Bridge,
        terminal: Option<TerminalId>,
        primed: bool,
        input: PendingInput,
    ) -> bool {
        if primed && let Some(terminal) = terminal {
            bridge.send(input.into_request(terminal));
            true
        } else {
            self.queue_pending(input)
        }
    }

    /// Retains pre-frame input up to fixed event and byte bounds.
    ///
    /// Once either bound would be exceeded, the newest event is rejected. Earlier input stays in
    /// order so attachment can still flush a coherent prefix instead of an arbitrary suffix.
    #[must_use = "rejected input must be surfaced to the user"]
    pub(crate) fn queue_pending(&mut self, input: PendingInput) -> bool {
        if self.pending.len() >= PENDING_INPUT_EVENT_CAP {
            return false;
        }
        let retained = self
            .pending
            .iter()
            .map(PendingInput::retained_bytes)
            .sum::<usize>();
        if retained.saturating_add(input.retained_bytes()) > PENDING_INPUT_BYTE_CAP {
            return false;
        }
        self.pending.push(input);
        true
    }

    /// The wheel request a scroll over a live, primed terminal produces.
    pub(crate) fn wheel_request(
        &mut self,
        app: &AppState,
        terminal: TerminalId,
        event: &ScrollWheelEvent,
    ) -> Option<RequestBody> {
        if app.drops_terminal_keys() {
            return None;
        }
        let grid = app.grids.get(&terminal).filter(|grid| grid.primed)?;
        let wheel = self.wheel_event(
            terminal,
            grid,
            event,
            app.terminal_config.scroll_lines_per_step,
        )?;
        Some(RequestBody::WheelTerminal { terminal, wheel })
    }

    fn wheel_event(
        &mut self,
        terminal: TerminalId,
        grid: &MirrorGrid,
        event: &ScrollWheelEvent,
        lines: u32,
    ) -> Option<WheelEvent> {
        let (id, bounds, metrics) = self.geometry.filter(|(id, _, _)| *id == terminal)?;
        let col = ((event.position.x - bounds.origin.x) / metrics.width)
            .floor()
            .max(0.0) as u16;
        let row = ((event.position.y - bounds.origin.y) / metrics.height)
            .floor()
            .max(0.0) as u16;
        let steps = self
            .wheel
            .steps(id, event.delta, metrics.height, lines, event.touch_phase);
        (steps != 0).then_some(WheelEvent {
            steps,
            col: col.min(grid.cols.saturating_sub(1)),
            row: row.min(grid.rows.saturating_sub(1)),
            mods: modifiers(event.modifiers),
        })
    }

    pub(crate) fn cache_viewport(
        &mut self,
        terminal: TerminalId,
        grid: &MirrorGrid,
        include_scroll: bool,
    ) {
        let selection = self
            .mouse_selection
            .map(|s| (s.anchor.line, s.head.line))
            .or_else(|| {
                include_scroll
                    .then_some(self.anchor)
                    .flatten()
                    .map(|anchor| (anchor, self.caret))
            });
        let base = viewport_base(grid);
        if visible_selected_rows(base, grid.rows, selection)
            .next()
            .is_none()
        {
            return;
        }
        if self.row_caches.get(&terminal).is_some_and(|cache| {
            cache.cols != grid.cols
                || cache.alt_screen != grid.modes.alt_screen
                || cache.history_epoch != grid.viewport.history_epoch
        }) {
            self.row_caches.remove(&terminal);
        }
        let cache = self
            .row_caches
            .entry(terminal)
            .or_insert_with(|| TerminalRowCache {
                cols: grid.cols,
                alt_screen: grid.modes.alt_screen,
                history_epoch: grid.viewport.history_epoch,
                last_seq: 0,
                last_viewport_base: u64::MAX,
                rows: BTreeMap::new(),
            });
        cache_selected_grid_rows(cache, grid, base, selection);
    }

    pub(crate) fn track_selection(&mut self, grid: &MirrorGrid, clear_unanchored: bool) {
        let base = viewport_base(grid);
        let Some(bottom) = viewport_last(grid) else {
            return;
        };
        self.caret = self.caret.clamp(base, bottom);
        let Some(anchor) = self.anchor else {
            if clear_unanchored {
                self.history.clear();
            }
            return;
        };
        let (first, last) = (anchor.min(self.caret), anchor.max(self.caret));
        let frame = (
            grid.seq,
            base,
            grid.viewport.history_epoch,
            grid.cols,
            grid.rows,
        );
        let changed = self.history_frame != Some(frame);
        self.history_frame = Some(frame);
        for row in 0..grid.rows {
            let line = base + u64::from(row);
            if line >= first
                && line <= last
                && self.history.len() < SELECTION_LINE_CAP
                && (changed || !self.history.contains_key(&line))
                && let Some(row) = cached_grid_row(grid, usize::from(row))
            {
                self.history.insert(line, row);
            }
        }
    }

    pub(crate) fn selection_text(
        &self,
        terminal: TerminalId,
        grid: &MirrorGrid,
    ) -> CurrentSelectionText {
        if let Some(selection) = self.mouse_selection.filter(|selection| selection.selected) {
            let empty = BTreeMap::new();
            let rows = self
                .row_caches
                .get(&terminal)
                .map_or(&empty, |cache| &cache.rows);
            return absolute_selection_text(
                grid,
                rows,
                AbsoluteCellSelection::new(selection.anchor, selection.head),
            )
            .map_or(CurrentSelectionText::Missing, CurrentSelectionText::Text);
        }
        self.anchor.map_or(CurrentSelectionText::None, |anchor| {
            try_selection_text(&self.history, anchor, self.caret)
                .map_or(CurrentSelectionText::Missing, CurrentSelectionText::Text)
        })
    }
    /// The selected text of `terminal`, or `None` when it has no mirror or no selection.
    pub(crate) fn selection_text_for(
        &self,
        app: &AppState,
        terminal: Option<TerminalId>,
    ) -> CurrentSelectionText {
        let Some(terminal) = terminal else {
            return CurrentSelectionText::None;
        };
        let Some(grid) = app.grids.get(&terminal) else {
            return CurrentSelectionText::None;
        };
        self.selection_text(terminal, grid)
    }
}

/// Reconciles a surface attachment through a correlated reply.
///
/// The caller supplies the other surface's retained attachment because daemon attachments are
/// set-valued. A reconnect already released the previous connection's attachment.
pub(crate) struct AttachmentSpec {
    pub(crate) target: Option<TerminalId>,
    pub(crate) generation: u64,
    pub(crate) preserve: Option<TerminalId>,
    pub(crate) size: (u16, u16),
}

pub(crate) fn reconcile_attachment<S: 'static>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    attachment: AttachmentSpec,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    let AttachmentSpec {
        target,
        generation,
        preserve,
        size,
    } = attachment;
    let mut local = surface.borrow_mut();
    let relinked = local.attached_generation != generation;
    if local.attached == target && !relinked {
        return;
    }
    if local
        .attach_retry_at
        .is_some_and(|retry_at| retry_at > Instant::now())
    {
        return;
    }
    if !relinked {
        local.detach(bridge, preserve);
    } else {
        local.attachment_attempt = local.attachment_attempt.wrapping_add(1);
        local.attach_retry_at = None;
    }
    local.attached = target;
    local.attached_generation = generation;
    let Some(terminal) = target else {
        return;
    };
    local.attachment_attempt = local.attachment_attempt.wrapping_add(1);
    let attempt = local.attachment_attempt;
    drop(local);

    let reply = bridge.request(RequestBody::AttachTerminal {
        terminal,
        cols: size.0,
        rows: size.1,
    });
    monitor_attachment(surface, state, terminal, generation, attempt, reply, cx);
}

fn monitor_attachment<S: 'static>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    terminal: TerminalId,
    generation: u64,
    attempt: u64,
    reply: async_channel::Receiver<
        Result<fleet_proto::response::ResponseBody, fleet_proto::error::ProtoError>,
    >,
    cx: &mut App,
) {
    let surface = Rc::clone(surface);
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        let answer =
            before_timeout(reply.recv(), cx.background_executor().timer(ATTACH_TIMEOUT)).await;
        let failure = match answer {
            Some(Ok(Ok(fleet_proto::response::ResponseBody::Ack))) => None,
            Some(Ok(Ok(_))) => Some("daemon returned an unexpected response".to_owned()),
            Some(Ok(Err(error))) => Some(error.message),
            Some(Err(_)) => Some("daemon reply was lost".to_owned()),
            None => Some("request timed out".to_owned()),
        };
        let Some(failure) = failure else {
            return;
        };
        let retry_at = Instant::now() + ATTACH_RETRY_DELAY;
        {
            let mut local = surface.borrow_mut();
            if local.attachment_attempt != attempt
                || local.attached != Some(terminal)
                || local.attached_generation != generation
            {
                return;
            }
            local.attached = None;
            local.attached_generation = 0;
            local.attach_retry_at = Some(retry_at);
        }
        let Some(state) = state.upgrade() else {
            return;
        };
        let message = format!(
            "could not attach terminal: {failure}; retrying. Reconnect Fleet if it persists"
        );
        state.update(cx, |app, cx| {
            app.sticky_error = Some(crate::state::StickyError {
                text: message,
                job: None,
                retryable: false,
            });
            cx.notify();
        });
        cx.background_executor().timer(ATTACH_RETRY_DELAY).await;
        if surface.borrow().attach_retry_at == Some(retry_at) {
            surface.borrow_mut().attach_retry_at = None;
            state.update(cx, |_, cx| cx.notify());
        }
    })
    .detach();
}

async fn before_timeout<T>(
    future: impl Future<Output = T>,
    timeout: impl Future<Output = ()>,
) -> Option<T> {
    let mut future = pin!(future);
    let mut timeout = pin!(timeout);
    poll_fn(|cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        if timeout.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

pub(crate) fn report_input_delivery(accepted: bool, state: &Entity<AppState>, cx: &mut App) {
    if accepted {
        return;
    }
    let now = Instant::now();
    let recently_notified = state.read(cx).toasts.iter().any(|live| {
        live.toast.text.as_ref() == INPUT_REJECTION_NOTICE
            && now.saturating_duration_since(live.shown_at) < INPUT_REJECTION_NOTICE_INTERVAL
    });
    if recently_notified {
        return;
    }
    state.update(cx, |app, cx| {
        app.toast_short(INPUT_REJECTION_NOTICE, Icon::Info, now);
        cx.notify();
    });
}

pub(crate) enum CurrentSelectionText {
    None,
    Text(String),
    Missing,
}

/// What a click count selects: one cell, the word under it, or the whole line.
pub(crate) const fn click_granularity(clicks: usize) -> SelectionGranularity {
    match clicks {
        0 | 1 => SelectionGranularity::Cell,
        2 => SelectionGranularity::Word,
        _ => SelectionGranularity::Line,
    }
}

/// §3.6 "Attaching": one dim centered line, with input meanwhile held in order.
pub(crate) fn attaching(theme: &Theme) -> impl IntoElement {
    gpui::div()
        .flex()
        .size_full()
        .items_center()
        .justify_center()
        .bg(theme.terminal.background)
        .child(fleet_ui_kit::Text::ui("attaching\u{2026}").muted())
}

/// The `cmd-c` a surface forwards when there is nothing of its own to copy.
pub(crate) fn copy_keystroke() -> KeyEvent {
    KeyEvent {
        key: Key::Char('c'),
        mods: Modifiers::SUPER,
        text: None,
        action: KeyAction::Press,
    }
}

/// Writes a finished selection to the clipboard, reporting whether it owned the gesture.
///
/// `false` means the surface had nothing selected and its caller should forward the keystroke
/// to the program instead. A selection whose rows are gone toasts rather than copying a
/// truncated version.
pub(crate) fn copy_to_clipboard<S>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    text: CurrentSelectionText,
    cx: &mut App,
) -> bool {
    match text {
        CurrentSelectionText::Text(text) => {
            if !text.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                state.update(cx, |app, cx| {
                    app.toast_short("copied", Icon::ClipboardCheck, Instant::now());
                    cx.notify();
                });
            }
            true
        }
        CurrentSelectionText::Missing => {
            selection_scrolled_away(surface, state, cx);
            true
        }
        CurrentSelectionText::None => false,
    }
}

/// `/`, `n` and `N` are reserved by KEYMAP for scrollback search, which lands after v1.
///
/// They stay bound so the keys cannot be reused, and say so rather than doing nothing.
pub(crate) fn reserved_search<A: gpui::Action>(
    root: gpui::Div,
    state: &Entity<AppState>,
) -> gpui::Div {
    let state = state.clone();
    root.on_action(move |_: &A, _window, cx| {
        state.update(cx, |app, cx| {
            app.toast_short(
                "scrollback search is not available yet",
                Icon::Search,
                Instant::now(),
            );
            cx.notify();
        });
    })
}

/// Drops a selection whose rows have left both the mirror and the bounded cache, and says so.
///
/// Both surfaces report the same thing because both bound their retained rows the same way: the
/// text is genuinely unrecoverable, and silently copying a truncated version would be worse.
pub(crate) fn selection_scrolled_away<S>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    surface.borrow_mut().clear_selections();
    state.update(cx, |app, cx| {
        app.toast_short("selection scrolled away", Icon::Info, Instant::now());
        cx.notify();
    });
}

#[derive(Default)]
pub(crate) struct PrefixHintState {
    visible: Rc<Cell<bool>>,
    task: Option<Task<()>>,
}
impl PrefixHintState {
    pub(crate) fn visible(&self) -> bool {
        self.visible.get()
    }
    pub(crate) fn clear(&mut self) {
        self.task = None;
        self.visible.set(false);
    }
    pub(crate) fn reconcile(&mut self, active: bool, state: &Entity<AppState>, cx: &mut App) {
        if !active {
            self.clear();
            return;
        }
        if self.task.is_some() {
            return;
        }
        let visible = Rc::clone(&self.visible);
        let state = state.downgrade();
        let delay = Duration::from_millis(cx.theme().motion.prefix_hint_delay);
        self.task = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            visible.set(true);
            let _ = state.update(cx, |_, cx| cx.notify());
        }));
    }
}

fn modifiers(mods: gpui::Modifiers) -> Modifiers {
    let mut result = Modifiers::empty();
    result.set(Modifiers::SHIFT, mods.shift);
    result.set(Modifiers::CTRL, mods.control);
    result.set(Modifiers::ALT, mods.alt);
    result.set(Modifiers::SUPER, mods.platform);
    result
}

fn visible_selected_rows(
    viewport_base: u64,
    viewport_rows: u16,
    selection: Option<(u64, u64)>,
) -> impl Iterator<Item = (usize, u64)> {
    let range = selection.and_then(|(first, last)| {
        if viewport_rows == 0 {
            return None;
        }
        let (first, last) = (
            first.min(last).max(viewport_base),
            first
                .max(last)
                .min(viewport_base + u64::from(viewport_rows - 1)),
        );
        (first <= last).then_some(first..=last)
    });
    range.into_iter().flatten().filter_map(move |line| {
        usize::try_from(line - viewport_base)
            .ok()
            .map(|row| (row, line))
    })
}

pub(crate) fn cache_selected_grid_rows(
    cache: &mut TerminalRowCache,
    grid: &MirrorGrid,
    viewport_base: u64,
    selection: Option<(u64, u64)>,
) {
    let mut selected_rows = visible_selected_rows(viewport_base, grid.rows, selection).peekable();
    if selected_rows.peek().is_none() {
        return;
    }
    let (first, last) =
        selection.map_or((0, 0), |(first, last)| (first.min(last), first.max(last)));
    cache.rows.retain(|line, _| *line >= first && *line <= last);
    let frame_changed = cache.last_seq != grid.seq || cache.last_viewport_base != viewport_base;
    for (row, line) in selected_rows {
        if cache
            .rows
            .get(&line)
            .is_some_and(|cached| !frame_changed || cached.matches(grid, row))
        {
            continue;
        }
        if let Some(cached) = cached_grid_row(grid, row) {
            cache.rows.insert(line, cached);
        }
    }
    while cache.rows.len() > MOUSE_ROW_CACHE_CAP {
        cache.rows.pop_first();
    }
    cache.last_seq = grid.seq;
    cache.last_viewport_base = viewport_base;
}

pub(crate) fn drain_pending_requests(
    pending: &mut Vec<PendingInput>,
    terminal: TerminalId,
) -> Vec<RequestBody> {
    pending
        .drain(..)
        .map(|input| input.into_request(terminal))
        .collect()
}

pub(crate) fn key_event(keystroke: &Keystroke, is_held: bool) -> Option<KeyEvent> {
    let key = named_key(&keystroke.key)?;
    let mods = modifiers(keystroke.modifiers);
    // Only a character key carries text: a named key's `key_char` is the platform's control
    // byte, and forwarding it would encode `Enter` twice.
    let text = match key {
        Key::Char(_) => keystroke.key_char.clone(),
        _ => None,
    };
    Some(KeyEvent {
        key,
        mods,
        text,
        action: if is_held {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        },
    })
}

fn named_key(name: &str) -> Option<Key> {
    Some(match name {
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "space" => Key::Char(' '),
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            let mut chars = other.chars();
            let first = chars.next()?;
            if chars.next().is_some() {
                // A named key this protocol does not model (`f13`, `back`, a lone modifier) is
                // dropped rather than typed into the shell as literal text.
                return None;
            }
            Key::Char(first)
        }
    })
}

/// Clears the selection and hands one keystroke to the surface's delivery policy.
///
/// `false` means the keystroke carries no terminal input, so the rest of the key handlers
/// still get a chance at it.
pub(crate) fn route_key<S>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    keystroke: &Keystroke,
    is_held: bool,
    cx: &mut App,
    deliver: impl FnOnce(PendingInput),
) -> bool {
    let Some(key) = key_event(keystroke, is_held) else {
        return false;
    };
    if surface.borrow_mut().clear_selections() {
        state.update(cx, |_, cx| cx.notify());
    }
    deliver(PendingInput::Key(key));
    true
}

/// Pastes the clipboard through the surface's delivery policy.
pub(crate) fn route_paste<S>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    cx: &mut App,
    deliver: impl FnOnce(PendingInput),
) {
    let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
        return;
    };
    if surface.borrow_mut().clear_selections() {
        state.update(cx, |_, cx| cx.notify());
    }
    deliver(PendingInput::Paste(text));
}

/// Copies what `terminal` has selected, reporting whether the surface owned the gesture.
///
/// `false` means nothing was selected and the caller should forward the `cmd-c` to the program.
pub(crate) fn route_copy<S>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    terminal: Option<TerminalId>,
    cx: &mut App,
) -> bool {
    let text = surface
        .borrow()
        .selection_text_for(state.read(cx), terminal);
    copy_to_clipboard(surface, state, text, cx)
}

#[cfg(test)]
mod tests;
