//! Harness targets — "this painted rect is reachable by name".
//!
//! The external GUI test harness (`docs/TESTING-HARNESS.md`) drives Fleet by *name*:
//! a scenario says `click worktrees.row[2]`, never `click 412 338`. That only works if the
//! application publishes where `worktrees.row[2]` was last painted, so this module gives the
//! whole kit one way to say it:
//!
//! ```no_run
//! use fleet_ui_kit::prelude::*;
//!
//! # fn demo() -> impl IntoElement {
//! div().w(px(200.0)).h(px(28.0)).harness_target("sticky_error.retry")
//! # }
//! ```
//!
//! ## What it costs when the harness is off
//!
//! Nothing that a profile can see. [`HarnessTargetExt::harness_target`] wraps the child in a
//! generic [`HarnessTarget`] that adds **no layout node** — it forwards `request_layout`,
//! `prepaint` and `paint` straight through — and the only extra work in `paint` is
//! [`is_recording`], one thread-local `bool` read and one branch. Recording is off unless
//! [`set_recording`] has been called, and the harness is the only caller.
//!
//! When recording *is* on, one frame's targets live in a per-window [`Vec`] that is cleared —
//! never reallocated — at the start of every frame, so a steady-state frame pushes into
//! capacity it already has.
//!
//! `docs/APP-CONTRACTS.md` forbids IO, requests, `cx.notify` and heavy CPU in a render path.
//! Recording a rect the frame already computed is none of those; it is bookkeeping of a value
//! GPUI hands to `paint` anyway. That tension is deliberate and is recorded in the phase plan:
//! bounds are simply not known at update time.
//!
//! ## Naming
//!
//! Names are the stable identifiers scenarios depend on. `docs/TESTING-HARNESS.md` §3 fixes
//! their form as `<surface>.<part>[<index>]` — `worktrees.row[2]`, `dialog.button[0]`,
//! `sticky_error.retry`. Prefer a `&'static str` for a fixed name and
//! [`HarnessTargetExt::harness_target_indexed`] for an indexed one: the index is formatted
//! only while recording, so a list of rows allocates nothing in production.
//!
//! A name built any other way must already be a `SharedString` owned by a memoised projection
//! (see `gpui-performance`), never a `format!` evaluated per row per frame.
//!
//! ## The frame boundary
//!
//! [`begin_frame`] clears a window's table and bumps its frame number. [`super::AppFrame`] —
//! the fixed chrome of every Fleet window, rendered once per frame at the window root — calls
//! it, so nothing else has to. A window that never renders an `AppFrame` never establishes a
//! table and therefore records nothing; that is what keeps the table from growing without a
//! frame boundary to clear it.
//!
//! [`frame`] is the authority for the frame number the harness snapshot reports, so a target
//! whose `frame` is behind `window.frame` is provably stale.
//!
//! ## Threading
//!
//! The table is thread-local, because it is written from `paint` and read from the snapshot
//! builder, both of which run on the foreground (window) thread. [`set_recording`] must be
//! called on that thread — inside the application's run closure — or the flag the paint path
//! reads stays false.

use std::cell::{Cell, RefCell};

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, SharedString, Window, WindowId,
};

thread_local! {
    /// The already-loaded flag every recording site branches on first.
    static RECORDING: Cell<bool> = const { Cell::new(false) };

    /// One frame of painted targets per window.
    static TABLE: RefCell<Table> = const { RefCell::new(Table::new()) };
}

/// Where a named element was painted, in logical window coordinates.
///
/// The field names and order are the ones `docs/TESTING-HARNESS.md` §3 pins for a
/// `targets` entry, so the snapshot builder can copy them across without a translation table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetRect {
    /// Distance from the window's left edge.
    pub x: f32,
    /// Distance from the window's top edge.
    pub y: f32,
    /// Painted width.
    pub w: f32,
    /// Painted height.
    pub h: f32,
    /// The frame this rect was painted in; compare it with [`frame`] to detect a stale read.
    pub frame: u64,
}

/// One target painted during a window's last frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedTarget {
    /// The scenario-facing name, for example `worktrees.row[2]`.
    pub name: SharedString,
    /// Where it was painted.
    pub rect: TargetRect,
}

/// Whether this thread is recording harness targets.
#[must_use]
pub fn is_recording() -> bool {
    RECORDING.with(Cell::get)
}

/// Turn target recording on or off for this thread.
///
/// Call it from the application's run closure when the harness socket is configured. Turning
/// recording off drops every recorded table, so a later read cannot see a stale frame.
pub fn set_recording(recording: bool) {
    RECORDING.with(|flag| flag.set(recording));
    if !recording {
        TABLE.with_borrow_mut(Table::clear);
    }
}

/// Start a new frame for `window`: clear its targets and bump its frame number.
///
/// [`super::AppFrame`] calls this, so application code does not need to. It is a no-op while
/// recording is off.
pub fn begin_frame(window: &Window) {
    if !is_recording() {
        return;
    }
    let id = window.window_handle().window_id();
    TABLE.with_borrow_mut(|table| table.begin_frame(id));
}

/// The number of the frame `window` is currently painting, or `0` before its first one.
///
/// This is the number the harness snapshot reports as `window.frame`.
#[must_use]
pub fn frame(window: &Window) -> u64 {
    let id = window.window_handle().window_id();
    TABLE.with_borrow(|table| table.window(id).map_or(0, |slot| slot.frame))
}

/// Every target painted in `window`'s last frame, in paint order.
///
/// A name painted twice appears twice; the later entry is the one on top, so a consumer
/// building a map should let the last entry win.
#[must_use]
pub fn painted(window: &Window) -> Vec<RecordedTarget> {
    let id = window.window_handle().window_id();
    TABLE.with_borrow(|table| {
        table
            .window(id)
            .map_or_else(Vec::new, |slot| slot.targets.clone())
    })
}

/// A target name, kept in the form that is cheapest to carry until it is actually needed.
#[derive(Debug, Clone)]
enum TargetName {
    /// A complete name the caller already owns.
    Fixed(SharedString),
    /// A `part` plus an `index`, rendered as `part[index]` only while recording.
    Indexed(&'static str, usize),
    /// No name: the wrapper paints its child and records nothing.
    ///
    /// This is what a kit list whose caller supplied no prefix wraps its rows in, so the named
    /// and the unnamed row are the same type and neither needs an `AnyElement` to unify them.
    Unnamed,
}

impl TargetName {
    fn resolve(&self) -> Option<SharedString> {
        match self {
            Self::Fixed(name) => Some(name.clone()),
            Self::Indexed(part, index) => Some(SharedString::from(format!("{part}[{index}]"))),
            Self::Unnamed => None,
        }
    }
}

/// Give any element a harness-addressable name.
///
/// Blanket-implemented for every [`IntoElement`], so it composes at the end of an ordinary
/// styling chain. The wrapper is transparent: it changes neither layout nor paint.
pub trait HarnessTargetExt: IntoElement + Sized {
    /// Record this element's painted bounds under a fixed `name`.
    ///
    /// Pass a `&'static str` wherever the name is fixed; anything else must already be an
    /// owned [`SharedString`] from a memoised projection, never a per-frame `format!`.
    fn harness_target(self, name: impl Into<SharedString>) -> HarnessTarget<Self::Element> {
        HarnessTarget {
            name: TargetName::Fixed(name.into()),
            child: self.into_element(),
        }
    }

    /// Record this element's painted bounds under `part[index]`, for example
    /// `harness_target_indexed("worktrees.row", 2)` → `worktrees.row[2]`.
    ///
    /// The name is formatted only while recording, so naming a list of rows costs nothing in
    /// production.
    fn harness_target_indexed(
        self,
        part: &'static str,
        index: usize,
    ) -> HarnessTarget<Self::Element> {
        HarnessTarget {
            name: TargetName::Indexed(part, index),
            child: self.into_element(),
        }
    }

    /// Record this element under `part[index]` only when `name` is `Some`.
    ///
    /// A kit list takes its target prefix from the caller — the same rows are
    /// `palette.row[N]` under the palette and `dialog.row[N]` inside a dialog — and a list with
    /// no prefix must still build the same element type. The wrapper is transparent either way.
    fn harness_target_optional(
        self,
        name: Option<(&'static str, usize)>,
    ) -> HarnessTarget<Self::Element> {
        HarnessTarget {
            name: match name {
                Some((part, index)) => TargetName::Indexed(part, index),
                None => TargetName::Unnamed,
            },
            child: self.into_element(),
        }
    }

    /// Record this element under a fixed `name` only when there is one.
    ///
    /// The unindexed half of [`HarnessTargetExt::harness_target_optional`], for a control that
    /// is named only in some of the states its component renders — an approval's `[y]`, which
    /// is `agents.approval.allow_once`, beside a question's `[⏎]`, which is nothing.
    fn harness_target_named(self, name: Option<&'static str>) -> HarnessTarget<Self::Element> {
        HarnessTarget {
            name: match name {
                Some(name) => TargetName::Fixed(SharedString::new_static(name)),
                None => TargetName::Unnamed,
            },
            child: self.into_element(),
        }
    }
}

impl<E: IntoElement> HarnessTargetExt for E {}

/// A transparent wrapper that records its child's painted bounds under a harness name.
///
/// Built by [`HarnessTargetExt::harness_target`]; there is no reason to name this type in a
/// signature.
pub struct HarnessTarget<E> {
    name: TargetName,
    child: E,
}

impl<E: Element> IntoElement for HarnessTarget<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: Element> Element for HarnessTarget<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.child.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.child.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.child.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        record(&self.name, bounds, window);
    }

    fn a11y_role(&self) -> Option<gpui::accesskit::Role> {
        self.child.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        self.child.write_a11y_info(node);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut gpui::A11ySubtreeBuilder<'_>,
    ) {
        self.child.a11y_synthetic_children(prepaint, builder);
    }
}

/// The whole recording cost when the harness is off: one flag read and one branch.
fn record(name: &TargetName, bounds: Bounds<Pixels>, window: &Window) {
    if !is_recording() {
        return;
    }
    // An unnamed wrapper is a list row whose caller asked for no prefix. It still paints; it
    // just has nothing to put in the table.
    let Some(name) = name.resolve() else {
        return;
    };
    let id = window.window_handle().window_id();
    TABLE.with_borrow_mut(|table| table.record(id, name, bounds));
}

/// One window's targets for the frame it is painting.
struct WindowTable {
    window: WindowId,
    frame: u64,
    targets: Vec<RecordedTarget>,
}

/// Every window that has begun a frame, with the one being painted kept at `active`.
struct Table {
    windows: Vec<WindowTable>,
    active: usize,
}

impl Table {
    const fn new() -> Self {
        Self {
            windows: Vec::new(),
            active: 0,
        }
    }

    fn clear(&mut self) {
        self.windows.clear();
        self.active = 0;
    }

    fn window(&self, window: WindowId) -> Option<&WindowTable> {
        self.windows.iter().find(|slot| slot.window == window)
    }

    fn begin_frame(&mut self, window: WindowId) {
        match self.windows.iter().position(|slot| slot.window == window) {
            Some(index) => {
                self.active = index;
                if let Some(slot) = self.windows.get_mut(index) {
                    slot.frame += 1;
                    // `clear` keeps the capacity, so a steady-state frame never reallocates.
                    slot.targets.clear();
                }
            }
            None => {
                self.windows.push(WindowTable {
                    window,
                    frame: 1,
                    targets: Vec::new(),
                });
                self.active = self.windows.len() - 1;
            }
        }
    }

    /// Append to the window's current frame. A window with no table — one that never rendered
    /// an [`super::AppFrame`], and so has no frame boundary to clear it — records nothing.
    fn record(&mut self, window: WindowId, name: SharedString, bounds: Bounds<Pixels>) {
        let Some(slot) = self.active_slot(window) else {
            return;
        };
        let frame = slot.frame;
        slot.targets.push(RecordedTarget {
            name,
            rect: TargetRect {
                x: bounds.origin.x.as_f32(),
                y: bounds.origin.y.as_f32(),
                w: bounds.size.width.as_f32(),
                h: bounds.size.height.as_f32(),
                frame,
            },
        });
    }

    fn active_slot(&mut self, window: WindowId) -> Option<&mut WindowTable> {
        if self
            .windows
            .get(self.active)
            .is_some_and(|slot| slot.window == window)
        {
            return self.windows.get_mut(self.active);
        }
        let index = self.windows.iter().position(|slot| slot.window == window)?;
        self.active = index;
        self.windows.get_mut(index)
    }
}

#[cfg(test)]
mod tests;
