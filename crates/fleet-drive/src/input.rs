//! GPUI input dispatch used by both Fleet surfaces.
//!
//! Every command here is *synthetic*: it builds a [`PlatformInput`] and hands it to
//! [`Window::dispatch_event`], the same entry point the compositor's own events arrive through.
//! Bindings, hitboxes, focus and hover therefore resolve exactly as they do under a real hand,
//! the headless lane works with no compositor at all, and nothing races the developer's session.
//! Driving `wtype`/`ydotool` instead would test GPUI's Wayland input layer, which is not what
//! this harness is for (`docs/TESTING-HARNESS.md`, and the decision record it names).
//!
//! ## Targets
//!
//! Pointer commands address a *name* — `click worktrees.row[2]`, never `click 412 338`. The
//! painted rectangles live in `fleet-ui-kit`'s paint-time recorder, which this crate deliberately
//! does not depend on: the application installs a reader once at startup with
//! [`set_target_source`], and every resolution pulls through it, so a rect is never cached across
//! a frame. An unknown name fails loudly and lists near matches; nothing ever falls back to
//! `(0, 0)`.
//!
//! ## Frames between phases
//!
//! A click is three phases — move, down, up — and the layout under the pointer may change between
//! them. [`settle_frame`] paints one frame synchronously (`Window::draw`, the same call GPUI makes
//! to give a new window its first frame) so the next phase hit-tests the layout the previous one
//! produced, and so a target resolved for a later phase comes from the frame that is actually on
//! screen. Those intermediate frames run layout and paint against entity state as it stands right
//! after the event; the authoritative frame is still the one the caller awaits once this function
//! returns and the enclosing update flushes its effects.
//!
//! ## Threading
//!
//! The target reader and the pointer's own position and held button are thread-locals, because
//! every caller here runs on the foreground (window) thread and a harness drives one window.

use crate::legacy::{keystroke_for, with_simulated_key_char};
use crate::protocol::{
    ClickArgs, DragArgs, HoverArgs, KeyArgs, Location, MouseButton, MoveArgs, PressArgs,
    ReleaseArgs, ScrollArgs, TypeArgs,
};
use anyhow::Context as _;
use gpui::{
    App, ClipboardItem, Keystroke, MouseButton as GpuiMouseButton, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent,
    SharedString, TouchPhase, Window, point, px, size,
};
use std::{
    cell::{Cell, RefCell},
    time::{Duration, Instant},
};

/// Logical pixels one scroll unit moves, kept from the legacy `wheel` driver so a lazygit script
/// translated to `scroll` moves by exactly as much as it used to.
const WHEEL_UNIT: f32 = 18.0;

/// Interpolated moves a drag emits when a scenario asks for none.
const MIN_DRAG_STEPS: u16 = 1;

/// How many near matches an unknown target names.
const NEAR_MATCHES: usize = 5;

/// How long a hover dwell holds the foreground thread between frames.
const DWELL_TICK: Duration = Duration::from_millis(16);

/// The longest dwell a `hover` will honour, so a mistyped scenario cannot wedge the window.
const MAX_DWELL: Duration = Duration::from_secs(5);

/// Where a named element was painted, in logical window coordinates.
///
/// The field names are the ones `docs/TESTING-HARNESS.md` §3 pins for a `targets` entry.
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
    /// The frame this rect was painted in.
    pub frame: u64,
}

impl TargetRect {
    /// The point a named input lands on, centered in the part visible inside `viewport`.
    fn actionable_centre(self, viewport: gpui::Size<Pixels>) -> Option<Point<Pixels>> {
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height);
        let left = self.x.max(0.0);
        let top = self.y.max(0.0);
        let right = (self.x + self.w).min(width);
        let bottom = (self.y + self.h).min(height);
        (left < right && top < bottom).then(|| {
            point(
                px(left + (right - left) * 0.5),
                px(top + (bottom - top) * 0.5),
            )
        })
    }
}

/// Every target painted in a window's last frame, and that frame's number.
///
/// A name painted twice appears twice, in paint order; the later entry is the one on top.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetFrame {
    /// The frame the window has most recently painted.
    pub frame: u64,
    /// The painted targets, in paint order.
    pub targets: Vec<(SharedString, TargetRect)>,
}

impl TargetFrame {
    /// The topmost rect painted under `name` in this frame.
    fn get(&self, name: &str) -> Option<TargetRect> {
        self.targets
            .iter()
            .rev()
            .find(|(candidate, _)| candidate.as_ref() == name)
            .map(|(_, rect)| *rect)
    }
}

type TargetSource = Box<dyn Fn(&Window) -> TargetFrame>;

thread_local! {
    /// How this thread reads the painted target table. `None` in every normal launch.
    static TARGET_SOURCE: RefCell<Option<TargetSource>> = const { RefCell::new(None) };

    /// Where the synthetic pointer is and what it is holding.
    static POINTER: Cell<Pointer> = const { Cell::new(Pointer::new()) };
}

/// Teach this thread how to read the painted target table.
///
/// The application owns the recorder, so it installs the reader — once, from its run closure,
/// alongside the rest of harness setup. The closure is called on every resolution and must not
/// re-enter [`set_target_source`] or [`clear_target_source`].
pub fn set_target_source(source: impl Fn(&Window) -> TargetFrame + 'static) {
    TARGET_SOURCE.with_borrow_mut(|slot| *slot = Some(Box::new(source)));
}

/// Forget how to read targets, so a later resolution fails rather than reading a stale table.
pub fn clear_target_source() {
    TARGET_SOURCE.with_borrow_mut(|slot| *slot = None);
}

/// Reads the painted target table, or `None` when no application installed a reader.
fn target_frame(window: &Window) -> Option<TargetFrame> {
    TARGET_SOURCE.with_borrow(|slot| slot.as_ref().map(|source| source(window)))
}

/// The synthetic pointer's state, carried between commands the way a real one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pointer {
    /// Where the pointer is, once something has moved it.
    position: Option<Point<Pixels>>,
    /// The button a `press` left held, so a later move reports it as a drag.
    pressed: Option<GpuiMouseButton>,
}

impl Pointer {
    const fn new() -> Self {
        Self {
            position: None,
            pressed: None,
        }
    }
}

/// Dispatches the requested key sequence and reports handling per key.
pub fn dispatch_key(
    window: &mut Window,
    cx: &mut App,
    args: &KeyArgs,
) -> anyhow::Result<Vec<bool>> {
    args.keys
        .iter()
        .map(|token| {
            let key =
                Keystroke::parse(token).with_context(|| format!("bad keystroke {token:?}"))?;
            Ok(window.dispatch_keystroke(with_simulated_key_char(key), cx))
        })
        .collect()
}

/// Types composed characters through GPUI's keystroke dispatcher.
pub fn dispatch_type(window: &mut Window, cx: &mut App, args: &TypeArgs) -> anyhow::Result<()> {
    for character in args.text.chars() {
        window.dispatch_keystroke(keystroke_for(character), cx);
    }
    Ok(())
}

/// Dispatches a pixel scroll event at the requested point, the pointer, or the viewport centre.
///
/// `dx`/`dy` are wheel units; one unit is [`WHEEL_UNIT`] logical pixels, which is the convention
/// the legacy `wheel <rows>` driver used. That driver scrolled *down* for a positive row count, so
/// its `wheel N` is this command's `scroll 0 -N` and its `hwheel N` is `scroll -N 0`.
///
/// A location moves the pointer first, because a wheel a hand turns comes from wherever the
/// pointer already is; with no location the pointer stays where it is and does not move.
pub fn dispatch_scroll(window: &mut Window, cx: &mut App, args: &ScrollArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.dx.is_finite() && args.dy.is_finite(),
        "scroll deltas must be finite, got ({}, {})",
        args.dx,
        args.dy
    );
    let position = match &args.at {
        Some(location) => hover_to(window, cx, location)?,
        None => POINTER.get().position.unwrap_or_else(|| {
            let size = window.viewport_size();
            point(
                px(f32::from(size.width) * 0.5),
                px(f32::from(size.height) * 0.5),
            )
        }),
    };
    window.dispatch_event(
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(args.dx * WHEEL_UNIT), px(args.dy * WHEEL_UNIT))),
            modifiers: window.modifiers(),
            touch_phase: TouchPhase::Moved,
        }),
        cx,
    );
    Ok(())
}

/// Moves the pointer to a target or a raw point.
pub fn dispatch_move(window: &mut Window, cx: &mut App, args: &MoveArgs) -> anyhow::Result<()> {
    let position = resolve(window, cx, &args.to)?;
    move_pointer(window, cx, position);
    Ok(())
}

/// Moves the pointer, then clicks `count` times, each click carrying its own click count.
///
/// A real double click is two down/up pairs whose click counts are 1 then 2, which is what GPUI's
/// click detection reads, so that is what this emits.
pub fn dispatch_click(window: &mut Window, cx: &mut App, args: &ClickArgs) -> anyhow::Result<()> {
    anyhow::ensure!(args.count >= 1, "a click needs a count of at least 1");
    let button = button(args.button);
    let position = hover_to(window, cx, &args.at)?;
    for count in 1..=args.count {
        if count > 1 {
            // What the previous click did has to be on screen before the next one lands on it.
            settle_frame(window, cx);
        }
        press_button(window, cx, position, button, usize::from(count));
        settle_frame(window, cx);
        // A hand does not move between a button going down and coming up, so neither does this.
        release_button(window, cx, position, button, usize::from(count));
    }
    Ok(())
}

/// Moves the pointer and holds a button down, leaving it held for a later `move` or `release`.
pub fn dispatch_press(window: &mut Window, cx: &mut App, args: &PressArgs) -> anyhow::Result<()> {
    let button = button(args.button);
    let position = hover_to(window, cx, &args.at)?;
    press_button(window, cx, position, button, 1);
    Ok(())
}

/// Moves the pointer and releases a button, ending whatever a `press` started.
pub fn dispatch_release(
    window: &mut Window,
    cx: &mut App,
    args: &ReleaseArgs,
) -> anyhow::Result<()> {
    let button = button(args.button);
    // A release implies the button was down, so the move that precedes it reports the drag even
    // when the press happened before this process started driving the pointer.
    POINTER.set(Pointer {
        position: POINTER.get().position,
        pressed: Some(button),
    });
    let position = hover_to(window, cx, &args.at)?;
    release_button(window, cx, position, button, 1);
    Ok(())
}

/// Presses, moves in `steps` interpolated hops, then releases — a drag as a hand performs it.
///
/// The interpolation matters: a single jump from `from` to `to` never crosses a drag threshold or
/// paints a preview, so the surface under test would behave differently from production. The
/// destination is resolved *after* the press, because pressing may be what makes it appear.
pub fn dispatch_drag(window: &mut Window, cx: &mut App, args: &DragArgs) -> anyhow::Result<()> {
    let button = button(args.button);
    let steps = args.steps.max(MIN_DRAG_STEPS);
    let from = hover_to(window, cx, &args.from)?;
    press_button(window, cx, from, button, 1);
    settle_frame(window, cx);
    let to = resolve(window, cx, &args.to)?;
    for step in 1..=steps {
        let fraction = f32::from(step) / f32::from(steps);
        move_pointer(window, cx, interpolate(from, to, fraction));
        settle_frame(window, cx);
    }
    release_button(window, cx, to, button, 1);
    Ok(())
}

/// Moves the pointer and holds it there, painting frames for the whole dwell.
///
/// A caller that can await — `fleet-app`'s socket driver is the one that matters — passes
/// `dwell_ms: 0` and awaits the dwell itself, which frees the foreground thread and lets the app
/// timers a hover is usually waiting on actually run. For a synchronous caller the dwell is
/// honoured here instead, blocking the foreground thread in [`DWELL_TICK`] slices and painting
/// between them; it is bounded by [`MAX_DWELL`], and unreachable outside harness mode either way.
pub fn dispatch_hover(window: &mut Window, cx: &mut App, args: &HoverArgs) -> anyhow::Result<()> {
    hover_to(window, cx, &args.at)?;
    let dwell = Duration::from_millis(args.dwell_ms).min(MAX_DWELL);
    let deadline = Instant::now() + dwell;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        std::thread::sleep(remaining.min(DWELL_TICK));
        settle_frame(window, cx);
    }
    Ok(())
}

/// Replaces the application clipboard, failing loudly where the platform has none.
pub fn clipboard_set(_window: &mut Window, cx: &mut App, text: &str) -> anyhow::Result<()> {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
    // GPUI's headless platform accepts a write and drops it, which would turn a scenario that
    // pastes into a dialog into a silent no-op. Reading the selection straight back is enough to
    // tell a real clipboard from an absent one.
    anyhow::ensure!(
        cx.read_from_clipboard().is_some(),
        "no platform clipboard in this lane: this Fleet has no compositor surface"
    );
    Ok(())
}

/// Returns the application clipboard's text.
pub fn clipboard_get(_window: &mut Window, cx: &mut App) -> anyhow::Result<String> {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .context("the clipboard holds no text, or this lane has no platform clipboard")
}

/// Asks the platform for a new logical window size.
pub fn resize(window: &mut Window, _cx: &mut App, width: u32, height: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0,
        "a resize needs a positive logical size, got {width}x{height}"
    );
    window.resize(size(px(width as f32), px(height as f32)));
    Ok(())
}

/// Takes keyboard focus and the pointer away from the window.
///
/// GPUI exposes no way to tell the platform to deactivate a window — activation only ever arrives
/// *from* the compositor — so this models the half of it the application can observe: the window's
/// focus is cleared and the pointer leaves, which is what `focused`, the key contexts and every
/// hover state see when a real window loses activation.
pub fn blur(window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
    window.blur();
    let pointer = POINTER.get();
    if let Some(position) = pointer.position {
        window.dispatch_event(
            PlatformInput::MouseExited(MouseExitEvent {
                position,
                pressed_button: pointer.pressed,
                modifiers: window.modifiers(),
            }),
            cx,
        );
    }
    POINTER.set(Pointer::new());
    Ok(())
}

/// Brings the window back to the foreground.
pub fn focus(window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
    window.activate_window();
    cx.activate(true);
    Ok(())
}

/// Moves the pointer onto `location`, paints, and resolves it again before the caller acts.
///
/// Hovering is itself an interaction — a rail expands, a row grows a button, a list reflows — so
/// the rect that was current when the pointer set off may not be the one under it now. Resolving
/// the name a second time against the frame the move produced is what `docs/TESTING-HARNESS.md`
/// §3 asks for: "a target's `frame` must equal `window.frame` before input uses it". It is one
/// correction rather than a loop, because a surface that never settles is a bug in the surface
/// and a scenario must fail on it rather than wait.
fn hover_to(
    window: &mut Window,
    cx: &mut App,
    location: &Location,
) -> anyhow::Result<Point<Pixels>> {
    let position = resolve(window, cx, location)?;
    move_pointer(window, cx, position);
    settle_frame(window, cx);
    let settled = resolve(window, cx, location)?;
    if settled != position {
        move_pointer(window, cx, settled);
        settle_frame(window, cx);
    }
    Ok(settled)
}

/// Paints one frame synchronously, so the next phase sees the layout this one produced.
///
/// This is `Window::draw` — the call GPUI itself makes to give a new window its first frame. It
/// runs layout, prepaint and paint, which refreshes the hitboxes the next event tests against and
/// the painted target table the next resolution reads. The scene is presented by the platform's
/// own frame, which the draw schedules.
fn settle_frame(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
}

/// Resolves a location to a point in logical window coordinates.
///
/// A name that is missing, or whose rect was painted before the frame now on screen, is *not*
/// used: the window paints a frame and the name is looked up again. Only then does a name that
/// is still absent or still stale fail — naming near matches, never falling back to `(0, 0)`.
fn resolve(
    window: &mut Window,
    cx: &mut App,
    location: &Location,
) -> anyhow::Result<Point<Pixels>> {
    let name = match location {
        Location::Point { x, y } => {
            anyhow::ensure!(
                x.is_finite() && y.is_finite(),
                "pointer coordinates must be finite, got ({x}, {y})"
            );
            return Ok(point(px(*x), px(*y)));
        }
        Location::Target { target } => target.as_str(),
    };
    let installed = target_frame(window).with_context(|| {
        format!("target {name:?} cannot be resolved: this Fleet installed no harness target reader")
    })?;
    let viewport = window.viewport_size();
    let fresh = match installed.get(name) {
        Some(rect) if rect.frame == installed.frame => {
            return rect.actionable_centre(viewport).with_context(|| {
                format!("target {name:?} is painted entirely outside the window")
            });
        }
        _ => {
            settle_frame(window, cx);
            target_frame(window).with_context(|| {
                format!("target {name:?} cannot be resolved: the harness target reader went away")
            })?
        }
    };
    let rect = fresh
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("{}", unknown_target(name, &fresh)))?;
    anyhow::ensure!(
        rect.frame == fresh.frame,
        "target {name:?} was last painted in frame {} but the window is on frame {}; \
         it is no longer on screen",
        rect.frame,
        fresh.frame
    );
    rect.actionable_centre(window.viewport_size())
        .with_context(|| format!("target {name:?} is painted entirely outside the window"))
}

/// Explains an unresolvable name, listing the painted names closest to it.
fn unknown_target(name: &str, frame: &TargetFrame) -> String {
    if frame.targets.is_empty() {
        return format!(
            "unknown target {name:?}: frame {} painted no targets at all — is the surface on \
             screen, and is target recording on?",
            frame.frame
        );
    }
    format!(
        "unknown target {name:?} in frame {}; nearest of the {} painted: {}",
        frame.frame,
        frame.targets.len(),
        near_matches(name, frame).join(", ")
    )
}

/// The painted names closest to `name`, best first.
fn near_matches(name: &str, frame: &TargetFrame) -> Vec<String> {
    let mut names: Vec<&str> = frame
        .targets
        .iter()
        .map(|(candidate, _)| candidate.as_ref())
        .collect();
    names.sort_unstable();
    names.dedup();
    names.sort_by(|left, right| {
        common_prefix(name, right)
            .cmp(&common_prefix(name, left))
            .then_with(|| distance(name, left).cmp(&distance(name, right)))
            .then_with(|| left.cmp(right))
    });
    names
        .into_iter()
        .take(NEAR_MATCHES)
        .map(str::to_owned)
        .collect()
}

/// How many leading characters two names share.
fn common_prefix(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

/// Levenshtein distance, over the short names a target table holds.
fn distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len() + 1];
    for (row, from) in left.chars().enumerate() {
        current[0] = row + 1;
        for (column, to) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(from != *to);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// The protocol's button vocabulary as GPUI's.
fn button(button: MouseButton) -> GpuiMouseButton {
    match button {
        MouseButton::Left => GpuiMouseButton::Left,
        MouseButton::Right => GpuiMouseButton::Right,
        MouseButton::Middle => GpuiMouseButton::Middle,
    }
}

/// A point `fraction` of the way from `from` to `to`.
fn interpolate(from: Point<Pixels>, to: Point<Pixels>, fraction: f32) -> Point<Pixels> {
    let lerp = |from: Pixels, to: Pixels| {
        px(f32::from(from) + (f32::from(to) - f32::from(from)) * fraction)
    };
    point(lerp(from.x, to.x), lerp(from.y, to.y))
}

/// Dispatches a move, which is what updates hover, the cursor and any drag in progress.
fn move_pointer(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
    let pressed = POINTER.get().pressed;
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: pressed,
            modifiers: window.modifiers(),
        }),
        cx,
    );
    POINTER.set(Pointer {
        position: Some(position),
        pressed,
    });
}

/// Dispatches a button down and remembers that it is held.
fn press_button(
    window: &mut Window,
    cx: &mut App,
    position: Point<Pixels>,
    button: GpuiMouseButton,
    click_count: usize,
) {
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers: window.modifiers(),
            click_count,
            first_mouse: false,
        }),
        cx,
    );
    POINTER.set(Pointer {
        position: Some(position),
        pressed: Some(button),
    });
}

/// Dispatches a button up and forgets that it was held.
fn release_button(
    window: &mut Window,
    cx: &mut App,
    position: Point<Pixels>,
    button: GpuiMouseButton,
    click_count: usize,
) {
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers: window.modifiers(),
            click_count,
        }),
        cx,
    );
    POINTER.set(Pointer {
        position: Some(position),
        pressed: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MouseButton as ProtocolButton;
    use gpui::{
        AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, WindowOptions, div,
    };
    use std::rc::Rc;

    /// One dispatched event, reduced to what a scenario can observe.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Seen {
        Move(f32, f32),
        Down(f32, f32, usize),
        Up(f32, f32, usize),
        Scroll(f32, f32),
    }

    type Journal = Rc<RefCell<Vec<Seen>>>;

    /// Two absolutely-placed boxes under a full-size surface that records every event.
    ///
    /// `probe.a` is painted at `(100, 50, 40, 20)` and `probe.b` at `(300, 50, 40, 20)`, so their
    /// centres are `(120, 60)` and `(320, 60)`.
    struct Probe {
        seen: Journal,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let moves = Rc::clone(&self.seen);
            let scrolls = Rc::clone(&self.seen);
            div()
                .relative()
                .size_full()
                .on_mouse_move(move |event, _, _| {
                    moves.borrow_mut().push(Seen::Move(
                        f32::from(event.position.x),
                        f32::from(event.position.y),
                    ));
                })
                .on_scroll_wheel(move |event, _, _| {
                    if let ScrollDelta::Pixels(delta) = event.delta {
                        scrolls
                            .borrow_mut()
                            .push(Seen::Scroll(f32::from(delta.x), f32::from(delta.y)));
                    }
                })
                .child(hit_box(100.0, Rc::clone(&self.seen)))
                .child(hit_box(300.0, Rc::clone(&self.seen)))
        }
    }

    fn hit_box(left: f32, seen: Journal) -> impl IntoElement {
        // `on_any_mouse_up` is not on the element trait, so every button this harness can send
        // gets its own listener and the assertions stay button-agnostic.
        let mut element = div()
            .absolute()
            .left(px(left))
            .top(px(50.0))
            .w(px(40.0))
            .h(px(20.0))
            .on_any_mouse_down({
                let seen = Rc::clone(&seen);
                move |event, _, _| {
                    seen.borrow_mut().push(Seen::Down(
                        f32::from(event.position.x),
                        f32::from(event.position.y),
                        event.click_count,
                    ));
                }
            });
        for button in [
            GpuiMouseButton::Left,
            GpuiMouseButton::Right,
            GpuiMouseButton::Middle,
        ] {
            let seen = Rc::clone(&seen);
            element = element.on_mouse_up(button, move |event, _, _| {
                seen.borrow_mut().push(Seen::Up(
                    f32::from(event.position.x),
                    f32::from(event.position.y),
                    event.click_count,
                ));
            });
        }
        element
    }

    /// Restores the thread's harness state, so one test never leaks into the next.
    struct Reader;

    impl Drop for Reader {
        fn drop(&mut self) {
            clear_target_source();
            POINTER.set(Pointer::new());
        }
    }

    /// Installs a target reader that always answers `frame` with `targets`.
    fn reader(frame: u64, targets: &'static [(&'static str, TargetRect)]) -> Reader {
        POINTER.set(Pointer::new());
        set_target_source(move |_| TargetFrame {
            frame,
            targets: targets
                .iter()
                .map(|(name, rect)| (SharedString::from(*name), *rect))
                .collect(),
        });
        Reader
    }

    const fn rect(x: f32, frame: u64) -> TargetRect {
        TargetRect {
            x,
            y: 50.0,
            w: 40.0,
            h: 20.0,
            frame,
        }
    }

    /// Both boxes, painted in frame 4, which is the frame the reader reports as current.
    const FRESH: [(&str, TargetRect); 2] =
        [("probe.a", rect(100.0, 4)), ("probe.b", rect(300.0, 4))];

    fn probe(cx: &mut TestAppContext) -> (Journal, VisualTestContext) {
        let seen: Journal = Rc::new(RefCell::new(Vec::new()));
        let journal = Rc::clone(&seen);
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Probe { seen: journal })
            })
            .expect("open the probe window")
        });
        let cx = VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        seen.borrow_mut().clear();
        (seen, cx)
    }

    #[gpui::test]
    fn a_named_target_is_clicked_at_the_centre_of_its_painted_rect(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_click(
                window,
                cx,
                &ClickArgs {
                    at: Location::Target {
                        target: "probe.b".to_owned(),
                    },
                    button: ProtocolButton::Left,
                    count: 1,
                },
            )
        })
        .expect("click the named target");

        assert_eq!(
            *seen.borrow(),
            vec![
                Seen::Move(320.0, 60.0),
                Seen::Down(320.0, 60.0, 1),
                Seen::Up(320.0, 60.0, 1),
            ],
            "a click moves first, so hover and the hit test are current when the button goes down"
        );
    }

    #[gpui::test]
    fn a_partially_off_window_target_uses_the_visible_intersection(cx: &mut TestAppContext) {
        const PARTIAL: [(&str, TargetRect); 1] = [(
            "probe.partial",
            TargetRect {
                x: -112.0,
                y: 50.0,
                w: 200.0,
                h: 20.0,
                frame: 4,
            },
        )];
        let _reader = reader(4, &PARTIAL);
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_move(
                window,
                cx,
                &MoveArgs {
                    to: Location::Target {
                        target: "probe.partial".to_owned(),
                    },
                },
            )
        })
        .expect("move to the visible part of the target");

        assert_eq!(*seen.borrow(), vec![Seen::Move(44.0, 60.0)]);
    }

    #[gpui::test]
    fn a_fully_off_window_target_is_refused(cx: &mut TestAppContext) {
        const OUTSIDE: [(&str, TargetRect); 1] = [(
            "probe.outside",
            TargetRect {
                x: -240.0,
                y: 50.0,
                w: 100.0,
                h: 20.0,
                frame: 4,
            },
        )];
        let _reader = reader(4, &OUTSIDE);
        let (seen, mut cx) = probe(cx);

        let error = cx
            .update(|window, cx| {
                dispatch_move(
                    window,
                    cx,
                    &MoveArgs {
                        to: Location::Target {
                            target: "probe.outside".to_owned(),
                        },
                    },
                )
            })
            .expect_err("a target with no visible area must fail");

        assert!(
            format!("{error:#}").contains("entirely outside the window"),
            "{error:#}"
        );
        assert!(seen.borrow().is_empty());
    }

    #[gpui::test]
    fn a_double_click_carries_both_click_counts(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_click(
                window,
                cx,
                &ClickArgs {
                    at: Location::Target {
                        target: "probe.a".to_owned(),
                    },
                    button: ProtocolButton::Right,
                    count: 2,
                },
            )
        })
        .expect("double click");

        assert_eq!(
            *seen.borrow(),
            vec![
                Seen::Move(120.0, 60.0),
                Seen::Down(120.0, 60.0, 1),
                Seen::Up(120.0, 60.0, 1),
                Seen::Down(120.0, 60.0, 2),
                Seen::Up(120.0, 60.0, 2),
            ],
            "a real double click is two pairs whose click counts are 1 then 2"
        );
    }

    #[gpui::test]
    fn a_drag_presses_interpolates_and_releases(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_drag(
                window,
                cx,
                &DragArgs {
                    from: Location::Target {
                        target: "probe.a".to_owned(),
                    },
                    to: Location::Target {
                        target: "probe.b".to_owned(),
                    },
                    button: ProtocolButton::Left,
                    steps: 4,
                },
            )
        })
        .expect("drag between the two targets");

        assert_eq!(
            *seen.borrow(),
            vec![
                Seen::Move(120.0, 60.0),
                Seen::Down(120.0, 60.0, 1),
                Seen::Move(170.0, 60.0),
                Seen::Move(220.0, 60.0),
                Seen::Move(270.0, 60.0),
                Seen::Move(320.0, 60.0),
                Seen::Up(320.0, 60.0, 1),
            ],
            "a drag crosses the distance in hops, so thresholds and previews behave as they do \
             under a real hand"
        );
    }

    #[gpui::test]
    fn an_unknown_target_names_near_matches_and_dispatches_nothing(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        let error = cx
            .update(|window, cx| {
                dispatch_move(
                    window,
                    cx,
                    &MoveArgs {
                        to: Location::Target {
                            target: "probe.c".to_owned(),
                        },
                    },
                )
            })
            .expect_err("an unknown target must fail");
        let message = format!("{error:#}");

        assert!(
            message.contains("unknown target \"probe.c\"") && message.contains("probe.a"),
            "an unknown target names itself and the painted names nearest it: {message}"
        );
        assert!(
            seen.borrow().is_empty(),
            "nothing is dispatched for a name that could not be resolved: {:?}",
            seen.borrow()
        );
    }

    #[gpui::test]
    fn a_target_from_an_older_frame_is_refused_rather_than_clicked_blind(cx: &mut TestAppContext) {
        // The rect says frame 3; the reader says the window is on frame 4.
        const STALE: [(&str, TargetRect); 1] = [("probe.a", rect(100.0, 3))];
        let _reader = reader(4, &STALE);
        let (seen, mut cx) = probe(cx);

        let error = cx
            .update(|window, cx| {
                dispatch_click(
                    window,
                    cx,
                    &ClickArgs {
                        at: Location::Target {
                            target: "probe.a".to_owned(),
                        },
                        button: ProtocolButton::Left,
                        count: 1,
                    },
                )
            })
            .expect_err("a stale target must fail");
        let message = format!("{error:#}");

        assert!(
            message.contains("frame 3") && message.contains("frame 4"),
            "a stale target names both frames: {message}"
        );
        assert!(
            seen.borrow().is_empty(),
            "a stale rect is never clicked: {:?}",
            seen.borrow()
        );
    }

    #[gpui::test]
    fn a_target_without_a_reader_fails_loudly(cx: &mut TestAppContext) {
        clear_target_source();
        POINTER.set(Pointer::new());
        let (seen, mut cx) = probe(cx);

        let error = cx
            .update(|window, cx| {
                dispatch_move(
                    window,
                    cx,
                    &MoveArgs {
                        to: Location::Target {
                            target: "probe.a".to_owned(),
                        },
                    },
                )
            })
            .expect_err("no reader means no resolution");

        assert!(
            format!("{error:#}").contains("no harness target reader"),
            "the failure says why resolution is impossible: {error:#}"
        );
        assert!(seen.borrow().is_empty(), "and nothing was dispatched");
    }

    #[gpui::test]
    fn a_scroll_unit_is_eighteen_logical_pixels_at_the_named_target(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_scroll(
                window,
                cx,
                &ScrollArgs {
                    dx: 0.0,
                    dy: -5.0,
                    at: Some(Location::Target {
                        target: "probe.a".to_owned(),
                    }),
                },
            )
        })
        .expect("scroll at a named target");

        assert_eq!(
            *seen.borrow(),
            vec![Seen::Move(120.0, 60.0), Seen::Scroll(0.0, -90.0)],
            "`scroll 0 -5` is the legacy `wheel 5`: five rows of eighteen logical pixels"
        );
    }

    #[gpui::test]
    fn a_raw_point_needs_no_reader_and_must_be_finite(cx: &mut TestAppContext) {
        clear_target_source();
        POINTER.set(Pointer::new());
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_move(
                window,
                cx,
                &MoveArgs {
                    to: Location::Point { x: 120.0, y: 60.0 },
                },
            )
        })
        .expect("raw coordinates resolve without a target table");
        assert_eq!(*seen.borrow(), vec![Seen::Move(120.0, 60.0)]);

        let error = cx
            .update(|window, cx| {
                dispatch_move(
                    window,
                    cx,
                    &MoveArgs {
                        to: Location::Point {
                            x: f32::NAN,
                            y: 0.0,
                        },
                    },
                )
            })
            .expect_err("a non-finite coordinate is a scenario bug, not a click at (0, 0)");
        assert!(format!("{error:#}").contains("must be finite"), "{error:#}");
    }

    #[gpui::test]
    fn a_press_holds_the_button_until_a_release_and_a_move_between_them_drags(
        cx: &mut TestAppContext,
    ) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        cx.update(|window, cx| {
            dispatch_press(
                window,
                cx,
                &PressArgs {
                    at: Location::Target {
                        target: "probe.a".to_owned(),
                    },
                    button: ProtocolButton::Left,
                },
            )
        })
        .expect("press");
        assert_eq!(
            POINTER.get().pressed,
            Some(GpuiMouseButton::Left),
            "a press leaves the button held for the next command"
        );

        cx.update(|window, cx| {
            dispatch_release(
                window,
                cx,
                &ReleaseArgs {
                    at: Location::Target {
                        target: "probe.b".to_owned(),
                    },
                    button: ProtocolButton::Left,
                },
            )
        })
        .expect("release");

        assert_eq!(
            *seen.borrow(),
            vec![
                Seen::Move(120.0, 60.0),
                Seen::Down(120.0, 60.0, 1),
                Seen::Move(320.0, 60.0),
                Seen::Up(320.0, 60.0, 1),
            ],
            "press and release are two commands over one gesture"
        );
        assert_eq!(
            POINTER.get().pressed,
            None,
            "the release hands the button back"
        );
    }

    #[gpui::test]
    fn a_hover_dwell_is_bounded_and_leaves_the_pointer_where_it_asked(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (seen, mut cx) = probe(cx);

        let started = Instant::now();
        cx.update(|window, cx| {
            dispatch_hover(
                window,
                cx,
                &HoverArgs {
                    at: Location::Target {
                        target: "probe.b".to_owned(),
                    },
                    dwell_ms: 40,
                },
            )
        })
        .expect("hover with a dwell");

        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "the dwell is honoured rather than dropped"
        );
        assert_eq!(
            *seen.borrow(),
            vec![Seen::Move(320.0, 60.0)],
            "a dwell holds the pointer still; it does not jiggle it"
        );
        assert_eq!(POINTER.get().position, Some(point(px(320.0), px(60.0))));
    }

    #[gpui::test]
    fn the_clipboard_round_trips_and_blur_takes_the_pointer_away(cx: &mut TestAppContext) {
        let _reader = reader(4, &FRESH);
        let (_seen, mut cx) = probe(cx);

        cx.update(|window, cx| clipboard_set(window, cx, "feature/mouse input"))
            .expect("write the clipboard");
        assert_eq!(
            cx.update(clipboard_get).expect("read the clipboard"),
            "feature/mouse input",
            "text with spaces survives the round trip"
        );

        cx.update(|window, cx| {
            dispatch_move(
                window,
                cx,
                &MoveArgs {
                    to: Location::Point { x: 120.0, y: 60.0 },
                },
            )
        })
        .expect("move");
        cx.update(blur).expect("blur");
        assert_eq!(
            POINTER.get(),
            Pointer::new(),
            "a window that lost activation has no pointer in it"
        );

        cx.update(focus).expect("focus");
    }

    #[gpui::test]
    fn a_resize_needs_a_positive_logical_size(cx: &mut TestAppContext) {
        let (_seen, mut cx) = probe(cx);

        cx.update(|window, cx| resize(window, cx, 1280, 720))
            .expect("a real size is accepted");
        let error = cx
            .update(|window, cx| resize(window, cx, 0, 720))
            .expect_err("a zero edge is a scenario bug");
        assert!(format!("{error:#}").contains("0x720"), "{error:#}");
    }

    #[test]
    fn near_matches_rank_by_shared_prefix_then_edit_distance() {
        let frame = TargetFrame {
            frame: 9,
            targets: [
                "worktrees.row[0]",
                "worktrees.row[1]",
                "prs.row[0]",
                "repos.row[0]",
            ]
            .into_iter()
            .map(|name| (SharedString::from(name), rect(0.0, 9)))
            .collect(),
        };

        assert_eq!(
            near_matches("worktrees.row[2]", &frame),
            vec![
                "worktrees.row[0]".to_owned(),
                "worktrees.row[1]".to_owned(),
                "prs.row[0]".to_owned(),
                "repos.row[0]".to_owned(),
            ],
            "the two names sharing the longest prefix come first"
        );
    }

    #[test]
    fn an_empty_frame_says_so_rather_than_listing_nothing() {
        let message = unknown_target("worktrees.row[0]", &TargetFrame::default());
        assert!(
            message.contains("painted no targets at all"),
            "an empty table is a different problem from a misspelling: {message}"
        );
    }
}
