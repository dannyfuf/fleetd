# GPUI performance patterns — long form

Zed snippets are trimmed from `/Users/danny/.swarm/repos/zed-industries/zed` at tag **v1.18.1**
(`bebe92f`), the GPUI revision fleetd pins in `Cargo.toml:53`. fleetd paths are relative to the
repo root. Where gpui v1.18.1 differs from newer Zed, the difference is called out.

Contents:
[p1](#p1-render-purity) ·
[p2](#p2-memoised-projection) ·
[p3](#p3-uniform-list) ·
[p4](#p4-list-state) ·
[p5](#p5-visible-range) ·
[p6](#p6-frame-caches) ·
[p7](#p7-background-cancel) ·
[p8](#p8-debounce) ·
[p9](#p9-coalesce) ·
[p10](#p10-cheap-types) ·
[p11](#p11-lints) ·
[p12](#p12-notify) ·
[p13](#p13-measurement) ·
[p14](#p14-build-profiles)

---

## p1 — Render purity {#p1-render-purity}

GPUI is immediate-mode over a retained frame: `Render::render` rebuilds the whole element tree
every frame, and the tree plus every callback it registered is dropped before the next frame
(`zed/crates/gpui/src/element.rs:9-14`). "Make render cheap" therefore never means "render less
often" — it means the per-frame work must be O(visible) and the expensive derivation must live
somewhere else.

Zed encodes four render rules as `dylint` lints in `tooling/lints`, wired in via
`[workspace.metadata.dylint]` in the root `Cargo.toml`. They are the densest statement of the
discipline anywhere in the codebase.

```rust
// zed/tooling/lints/src/notify_in_render.rs:14-21
/// `notify()` tells the framework that the entity's state has changed and
/// it should be re-rendered. Calling it during render means every render
/// pass schedules another render pass — either an infinite loop or wasted
/// work.
pub NOTIFY_IN_RENDER, Warn,
"calling `cx.notify()` during render schedules a redundant re-render"
```

```rust
// zed/tooling/lints/src/entity_update_in_render.rs:16-22
/// The `render` method should be a pure function of state. Calling
/// `.update()` mutates an entity during the render pass, which can trigger
/// re-renders mid-render and lead to inconsistent UI state or infinite
/// render loops.
pub ENTITY_UPDATE_IN_RENDER, Warn, "mutating an entity via `.update()` during render"
```

```rust
// zed/tooling/lints/src/blocking_io_on_foreground.rs:21-27
/// In GPUI, code that receives a synchronous context type runs on the
/// foreground (UI) thread. A blocking IO call on this thread freezes the
/// application until the syscall returns.
pub BLOCKING_IO_ON_FOREGROUND, Warn, "blocking IO call on the GPUI foreground thread"
```

The blocking-IO lint's trigger surface doubles as a review checklist. It fires on `std::fs::*`,
`std::fs::File::{open,create}`, `std::thread::sleep`, `std::net::TcpStream::connect`, and —
easy to miss — `Path::{exists,try_exists,is_file,is_dir,metadata,canonicalize,read_dir}`
(`zed/tooling/lints/src/blocking_io_on_foreground.rs:31-70`) whenever the enclosing fn takes
`&App`, `&mut App`, `&Context<T>`, `&mut Context<T>` or `&mut Window`.

**Exception worth knowing.** `entity_update_in_render` deliberately fires only when the closure
returns `()` or `Result<()>`. An `.update()` used purely to *read* a projection and return a
value is fine, and Zed does it constantly.

**fleetd.** The same rule is prose, not tooling: `docs/APP-CONTRACTS.md:101-103` ("**Render
prepares nothing.** A render function composes already-prepared data. Filesystem access, request
initiation, expensive projection and focus or lifecycle reconciliation belong in `synchronize`,
in an observation, or in a background task — never in `render`.") and
`docs/ARCHITECTURE.md:384-386`. It is honoured for IO — the four blocking-IO call sites in
`fleet-app/src` are all inside background tasks — but not for CPU (see p2 and the SKILL's gaps).
Adopting Zed's `tooling/lints` crate, or copying the three lints above, is the mechanical fix.

---

## p2 — Memoised projection {#p2-memoised-projection}

Zed's `ProjectPanel` never walks the worktree in render: it maintains `visible_entries`
(`zed/crates/project_panel/src/project_panel.rs:107`) from ~30 mutation and event sites, backed
by a `_visible_entries_task: Task<()>` (`:171`), and render only reads the length and hands a
closure to `uniform_list`. The editor does the same with `EditorSnapshot`, produced before layout
and consulted for the visible rows only.

fleetd's own version is better documented and is the pattern to copy verbatim:

```rust
// crates/fleet-app/src/screens/hub/projection.rs:3-38 (trimmed)
#[derive(Default)]
pub(super) struct ProjectionCache {
    key: Option<ProjectionKey>,
    model: Rc<HubModel>,
}

/// Every input the model is derived from, as revisions and cheap values.
#[derive(PartialEq, Eq)]
struct ProjectionKey {
    source: u64,        // AppState::snapshot_revision
    cache: u64,         // HubState::presentation_revision
    connection: u64,    // AppState::link_generation
    scope: RepoScope,
    pane: HubPane,
    tab: PrTab,
    screen: Screen,
    query: String,
}

/// The Hub model for this frame, rebuilt only when one of its inputs changed.
pub(super) fn prepare(state: &AppState, hub: &HubState, now: i64) -> Rc<HubModel> {
    let mut cache = hub.projection.borrow_mut();
    let key = ProjectionKey { /* … */ };
    if cache.key.as_ref() != Some(&key) {
        cache.model = Rc::new(model(state, hub, now));
        cache.key = Some(key);
    }
    cache.model.clone()
}
```

Design notes worth preserving when you copy it:

- The key is *revisions and cheap values*, never the data itself. `snapshot_revision` is bumped
  in one place, `AppState::bump_snapshot_revision` (`crates/fleet-app/src/state/snapshot.rs:97-100`),
  called from every applied mutation — so the cache cannot go stale silently.
- `now: i64` is an **input, not part of the key**. Time-derived display ("2m ago") must not
  invalidate the model every second; `HubModel::prepared_at` records when it was built.
- The model owns its rows (`Rc<[RailRow]>`, `Rc<[WorktreeRow]>`, `Rc<[PrRow]>`), so the same
  `Rc<HubModel>` is handed to the renderer *and* to the key handlers
  (`screens/hub/navigation.rs:113-115`) — one derivation, two consumers, no divergence.
- The cache lives in a `RefCell` on the screen struct (`screens/hub.rs:139`), not in `AppState`,
  because it is presentation, not truth.

`fleet-ui-kit` has a second, differently-shaped memoiser that is also correct — the terminal
grid batch cache, keyed on pointer identity rather than a revision:

```rust
// crates/fleet-ui-kit/src/components/terminal_grid/batching.rs:243-260 (trimmed)
pub(super) fn content(&self, rows: &Arc<[GridRow]>, theme: &Theme,
                      visible: std::ops::Range<usize>) -> Rc<GridContent> {
    let mut cached = self.0.borrow_mut();
    if let Some(previous) = cached.as_ref()
        && Arc::ptr_eq(&previous.rows, rows)
        && previous.theme == *theme
        && previous.visible == visible
    {
        return previous.content.clone();
    }
    // … rebuild only the visible rows …
}
```

Its supplier is a copy-on-write publisher: `TerminalPresentation::update` re-converts only rows
whose FNV digest changed and uses `Arc::make_mut` (`crates/fleet-app/src/terminal/presentation.rs:186-230`),
with a test asserting `Arc::ptr_eq` holds across 120 cursor-only frames of a 200×60 grid
(`presentation.rs:262-281`). Pointer identity + `Arc::make_mut` is the right key when the data is
a large immutable slice; a revision counter is right when the data is a projection of state.

**When NOT to memoise:** a bool, a count, or a `format!` over values already in hand. The
invalidation is the expensive and bug-prone part.

---

## p3 — `uniform_list` for fixed-height rows {#p3-uniform-list}

```rust
// zed/crates/gpui/src/elements/uniform_list.rs:1-5
//! A scrollable list of elements with uniform height, optimized for large lists.
//! Rather than use the full taffy layout system, uniform_list simply measures
//! the first element and then lays out all remaining elements in a line based on that
//! measurement. This is much faster than the full layout system, but only works for
//! elements with uniform height.
```

The virtualization core, in `prepaint`:

```rust
// zed/crates/gpui/src/elements/uniform_list.rs:473-490 (trimmed)
let first_visible_element_ix =
    (-(scroll_offset.y + padding.top) / item_height).floor() as usize;
let last_visible_element_ix =
    ((-scroll_offset.y + padded_bounds.size.height) / item_height).ceil() as usize;
let visible_range = first_visible_element_ix
    ..cmp::min(last_visible_element_ix, self.item_count);
let items = (self.render_items)(visible_range.clone(), window, cx);
```

`item_height` comes from measuring exactly one row (`measure_item` renders `ix..ix+1`), the
frame's elements land in a `SmallVec<[AnyElement; 32]>` (`uniform_list.rs:70-73`), and scroll
requests are *deferred* into the scroll state and applied during the next prepaint — never as a
mutation from render.

**fleetd.** `fleet-app` calls `uniform_list` zero times directly; it goes through the kit:

```rust
// crates/fleet-ui-kit/src/components/list_view.rs:294-298
pub fn new(
    id: impl Into<ElementId>,
    item_count: usize,
    render_row: impl Fn(usize, bool, &mut Window, &mut App) -> AnyElement + 'static,
) -> Self
```

with `.cursor(index)`, `.row_height(px)`, `.track_scroll(&UniformListScrollHandle)`,
`.empty(el)`, `.loading(bool)`, `.skeleton_rows(n)` builders (`list_view.rs:313-350`) and a
static `ListView::reveal(handle, cursor, moving_down)` (`:358`). A real call site:
`crates/fleet-app/src/views/prs_screen.rs:287-295`. `LogView` is the same shape for log lines
(`components/log_view.rs:170`). `fleet-lazygit` calls `uniform_list` directly twice
(`views/diff.rs:439`, `overlays.rs:150`).

The uniform-height invariant is documented in fleetd better than in Zed —
`crates/fleet-lazygit/README.md` and `panels/main_panel.rs` both state that a wrapped row breaks
the assumption, and `list_view.rs:318-319` records that "`uniform_list` measures the first
rendered row itself."

`ListCursor` (`list_view.rs:138-266`) is the companion: `retain(new_len, find_previous)` is how
background data adopts without moving the selection, which is `docs/DESIGN-SYSTEM.md:536-538`'s
rule — "Background events … must **never** re-sort, re-scroll or re-focus."

**When NOT to:** any row that can wrap, expand, or otherwise vary in height. One taller row
corrupts every subsequent offset.

---

## p4 — `gpui::list` + `ListState` for variable heights {#p4-list-state}

```rust
// zed/crates/gpui/src/elements/list.rs:1-8
//! A list element that can be used to render a large number of differently sized elements
//! efficiently. Clients of this API need to ensure that elements outside of the scrolled
//! area do not change their height for this element to function correctly. If your elements
//! do change height, notify the list element via [`ListState::splice`] or [`ListState::reset`].
//! In order to minimize re-renders, this element's state is stored intrusively
//! on your own views, so that your code can coordinate directly with the list element's cached state.
```

Heights live in a `SumTree<ListItem>` whose items are `Unmeasured { size_hint }` or
`Measured { size }`, so index↔offset lookups are O(log n). The measurement loop renders the
visible window *plus* `overdraw` and skips anything already `Measured`
(`zed/crates/gpui/src/elements/list.rs:1062-1074`).

API surface at v1.18.1:

| Call | Site | Note |
|---|---|---|
| `ListState::new(item_count, ListAlignment::{Top,Bottom}, overdraw)` | `list.rs:314` | `Bottom` is the chat/log/scrollback mode |
| `splice(old_range, count)` | `list.rs:503` | inserts `Unmeasured` — invalidates only that span |
| `splice_focusable(..)` | `list.rs:511` | same, preserving focus handles |
| `scroll_to_reveal_item(ix)` | `list.rs:677` | deferred, applied in prepaint |
| `measure_all()` | `list.rs:336` | opt-in exact scrollbar, slow first frame |

**fleetd.** Three call sites, all correct in shape:
`crates/fleet-ui-kit/src/components/agent/transcript_list.rs:219`
(`ListState::new(0, ListAlignment::Bottom, AGENT_LIST_OVERDRAW)` — the agent transcript, whose
rows genuinely vary), `crates/fleet-app/src/screens/jobs.rs:156` and
`crates/fleet-app/src/dialogs/confirm.rs:76` (both `ListAlignment::Top, px(0.0)`).
`transcript_list.rs` is one of the four documented stateful kit components
(`docs/DESIGN-SYSTEM.md:1058-1061`) precisely because it caches measured row heights.

**Review point:** every mutation path must reach `splice(changed_range, count)`. A `reset` throws
away every measured height and forces a full remeasure on the next frame.

**When NOT to:** if rows *are* uniform, `uniform_list` is strictly cheaper — no SumTree, no
per-item measurement — and `list` cannot give an exact scrollbar without `measure_all()`.

---

## p5 — Compute the visible range first (the general principle) {#p5-visible-range}

The editor proves this generalises past list elements. In `prepaint`:

```rust
// zed/crates/editor/src/element.rs:8181-8196 (trimmed)
let max_row = snapshot.max_point().row();
let start_row = cmp::min(
    DisplayRow((scroll_position.y + clipped_top_in_lines).floor() as u32), max_row);
let end_row = cmp::min(
    (scroll_position.y + clipped_top_in_lines + visible_height_in_lines).ceil() as u32,
    max_row.next_row().0);
let row_infos = snapshot                    // note we only get the visual range
    .row_infos(start_row)
    .take((start_row..end_row).len())
    .collect::<Vec<RowInfo>>();
```

Everything downstream — `layout_lines`, cursors, gutter, indent guides, blocks — is parameterised
by `start_row..end_row`, so a 100k-line file costs the same per frame as a 100-line file.

**fleetd.** The terminal grid is the local instance and it is done right: it is deliberately not
a `uniform_list` (ADR 0002 forbids it) but a `canvas()` that batches runs over a visible row
range and shapes each batch once
(`crates/fleet-ui-kit/src/components/terminal_grid/{batching,painter}.rs`). Only two custom
`impl Element` exist workspace-wide, both text inputs
(`components/text_field/element.rs:33`, `components/multiline_input/element.rs:39`), and both use
`RequestLayoutState = ()`. If either ever recomputes a layout it could carry forward, that is
what `PrepaintState` and `with_element_state` are for (p6).

---

## p6 — Frame-crossing caches: element state and shaped text {#p6-frame-caches}

```rust
// zed/crates/gpui/src/window.rs:3790-3800 (doc trimmed)
/// Updates or initializes state for an element with the given id that lives across multiple
/// frames. … This method should only be called as part of element drawing.
pub fn with_element_state<S, R>(
    &mut self,
    global_id: &GlobalElementId,
    f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
) -> R
```

State is keyed by `GlobalElementId` + `TypeId`. `use_keyed_state` / `use_state` wrap it to hand
back an `Entity<S>` and auto-`observe` it into the current view; `use_state` derives its key from
`#[track_caller]` and is therefore **wrong inside a list-item loop** — use `use_keyed_state`
there (stated in gpui's own doc comment).

**fleetd** uses it exactly once, correctly:
`crates/fleet-ui-kit/src/components/terminal_tab_strip.rs:279` (`window.use_keyed_state(…)`).

**Text shaping** is cached by GPUI itself across two frames. `LineLayoutCache` keeps a
`previous_frame` and a `current_frame` map and migrates hits forward
(`zed/crates/gpui/src/text_system/line_layout.rs:454-457, 594-610`):

```rust
// zed/crates/gpui/src/text_system/line_layout.rs:594-608 (trimmed)
let current_frame = self.current_frame.upgradable_read();
if let Some(layout) = current_frame.wrapped_lines.get(key) { return layout.clone(); }
let previous_frame_entry = self.previous_frame.lock().wrapped_lines.remove_entry(key);
if let Some((key, layout)) = previous_frame_entry {
    // migrate last frame's layout into this frame — no reshape
}
```

So `window.text_system().shape_line(text, size, &runs, force_width)` with the *same*
`SharedString` and runs is nearly free each frame; building a fresh `String` defeats it, and so
does constructing your own `WindowTextSystem`.

**fleetd.** Right: `crates/fleet-ui-kit/src/components/terminal_grid/painter.rs:41,106` shape per
batch through `window.text_system()` with `force_width = cell_width`;
`components/text_field/element.rs:183` likewise. Wrong: `crates/fleet-lazygit/src/views/diff.rs:496`,
`views/long_line.rs:166`, `diff_view.rs:246` and `root/diff_view.rs:38` each construct
`WindowTextSystem::new(cx.text_system().clone())`, which has its own empty `LineLayoutCache` and
so re-shapes every line, every time.

**When NOT to:** element state is dropped when its id stops appearing, so do not put anything in
it that must survive scrolling out of view.

---

## p7 — Background work, cancellation, yielding {#p7-background-cancel}

Zed's `.rules`, verbatim:

> To do work on other threads, `cx.background_spawn(async move { ... })` is used. Often this
> background task is awaited on by a foreground task which uses the results to update state.

> Both `cx.spawn` and `cx.background_spawn` return a `Task<R>` … If this task is dropped, then
> its work is cancelled. To prevent this one of the following must be done: … **Storing the task
> in a field, if the work should be halted when the struct is dropped.**

Storing-in-a-field is the cancellation idiom. `Editor` alone holds `document_highlights_task`,
`pull_diagnostics_task`, `refresh_colors_task`, `refresh_code_lens_task`,
`show_git_blame_inline_delay_task` and more (`zed/crates/editor/src/editor.rs:1008-1150`);
reassigning the field drops the old task, so stale work stops without a flag.

Long CPU loops on the background executor still yield, because it is a shared pool:

```rust
// zed/crates/git/src/blame.rs:155-159
lines_read += 1;
if lines_read % BLAME_PARSE_YIELD_INTERVAL == 0 {
    smol::future::yield_now().await;
}
```

**fleetd.** The foreground/background sandwich is done correctly in lazygit:

```rust
// crates/fleet-lazygit/src/diff_view.rs:415-425 (trimmed)
self._task = cx.spawn(async move |this, cx| {
    let build_cancel = cancellation.clone();
    let prepared = cx
        .background_spawn(async move {
            prepare(&unified, path.as_deref(), &style, text_system, &build_cancel)
        })
        .await;
    // … this.update(cx, …) applies it …
});
```

and the budget+cancel idiom is in `crates/fleet-lazygit/src/views/syntax.rs`:

```rust
// crates/fleet-lazygit/src/views/syntax.rs:32-35, 261 (trimmed)
pub(crate) const MAX_LINES: usize = 40_000;
pub(crate) const BUDGET: Duration = Duration::from_millis(1_500);
// inside run_cancellable(jobs, cancelled):
if seen > MAX_LINES || started.elapsed() > BUDGET { break; }
```

Counts across fleetd (`rg`, `src/` of the three GPUI crates): 51 `cx.spawn` in `fleet-app` and
**0** `cx.background_spawn` but 3 `cx.background_executor().spawn` (same executor, longer
spelling); 33 `.detach()` and **0** `.detach_and_log_err`; 27 `yield_now`;
16 `Task`-typed fields. `detach_and_log_err` exists in the pinned gpui
(`zed/crates/gpui/src/executor.rs:37`), so the zero is a gap, not a limitation.

Retention *is* correct where it matters: `Shell::_tasks: Vec<Task<()>>` and
`_subscriptions: Vec<Subscription>` (`crates/fleet-app/src/shell/root.rs:71-72`),
`HubState::{inspect_task, pr_refresh_task}` (`screens/hub.rs:129-130`),
`WorkspaceScreen::pr_tasks: HashMap<RepoId, Task<()>>` (`screens/workspace.rs:212`),
`DialogHost::tasks: HashMap<&'static str, Task<()>>` (`dialogs/host.rs:55`).

Where fleetd detaches, correctness is recovered by **generation guards** rather than weak
handles: `board_generation` (`state.rs:112`), `inspect_generation` (`screens/hub.rs:128`),
`link_generation`, `inspection_requests`, `snapshot_revision`. That works and is not a bug —
leave existing guards alone until you touch that code, and prefer a `Task` field plus
`WeakEntity` for new code (see `gpui-state-and-memory`).

**When NOT to:** `cx.background_spawn` requires `Send`, and entity handles are not `Send`. That
is the structural reason for the sandwich: background half computes, foreground half applies.

---

## p8 — Debounce with the executor timer, never `smol::Timer` {#p8-debounce}

Zed's `clippy.toml` denies it outright:

```toml
{ path = "smol::Timer::after", reason = "smol::Timer introduces non-determinism in tests",
  replacement = "gpui::BackgroundExecutor::timer" },
```

The reusable debouncer, which is also the `select_biased!` idiom:

```rust
// zed/crates/project/src/debounced_delay.rs:26-52 (trimmed)
pub fn fire_new<F>(&mut self, delay: Duration, cx: &mut Context<E>, func: F) {
    if let Some(channel) = self.cancel_channel.take() { _ = channel.send(()); }
    let (sender, mut receiver) = oneshot::channel::<()>();
    self.cancel_channel = Some(sender);
    let previous_task = self.task.take();
    self.task = Some(cx.spawn(async move |entity, cx| {
        let mut timer = cx.background_executor().timer(delay).fuse();
        if let Some(previous_task) = previous_task { previous_task.await; }
        futures::select_biased! {
            _ = receiver => return,
            _ = timer => {}
        }
        if let Ok(task) = entity.update(cx, |project, cx| (func)(project, cx)) { task.await; }
    }));
}
```

`select_biased!` rather than `select!` so cancellation and timeout are polled in a deterministic
order — essential under `#[gpui::test]` + `run_until_parked()`. The same shape gives timeouts
(`zed/crates/editor/src/editor.rs:8277`). Zed names and centralises the constants
(`editor.rs:299-305`: `CODE_ACTIONS_DEBOUNCE_TIMEOUT` 250 ms,
`SELECTION_HIGHLIGHT_DEBOUNCE_TIMEOUT` 100 ms, `LSP_REQUEST_DEBOUNCE_TIMEOUT` 50 ms).

**fleetd.** 14 `background_executor().timer` sites (good), **0** `select_biased!`, and no
`smol::Timer`. Today the cancel half is a generation counter: `HubState::inspect_generation` is
"bumped on every cursor move; an older debounce wakes up and does nothing"
(`crates/fleet-app/src/screens/hub.rs:127-128`). Both approaches are sound; `select_biased!`
cancels the timer instead of letting it fire and no-op, which is cheaper and reads better. The
fleetd spelling is `futures_util::select_biased!`: `futures-util` is already in
`[workspace.dependencies]` (`Cargo.toml:52`) and used by `fleet-client`, `fleet-daemon` and
`fleet-cli`, but `crates/fleet-app/Cargo.toml` does not take it. Adding that line is a deliberate
change with its own commit, not something to slip into an unrelated diff.

---

## p9 — Coalescing a high-frequency producer {#p9-coalesce}

Zed's PTY pump is the reference implementation:

```rust
// zed/crates/terminal/src/terminal.rs:1354-1409 (heavily trimmed)
while let Some(event) = self.events_rx.next().await {
    terminal.update(cx, |terminal, cx| {
        //Process the first event immediately for lowered latency
        terminal.process_pty_event(event, cx);
    })?;
    'outer: loop {
        let mut events = Vec::new();
        let mut timer = cx.background_executor().timer(Duration::from_millis(4)).fuse();
        let mut wakeup = false;
        loop {
            futures::select_biased! {
                _ = timer => break,
                event = self.events_rx.next() => {
                    /* dedupe Wakeup into a bool; push the rest */
                    if events.len() > 100 { break; }
                }
            }
        }
        if events.is_empty() && !wakeup { yield_now().await; break 'outer; }
        terminal.update(cx, |this, cx| { /* apply wakeup once, then all events */ })?;
        yield_now().await;
    }
}
```

Five ideas in ~50 lines: (1) the first event is uncoalesced so keystroke echo stays instant;
(2) a fixed ~4 ms coalescing window; (3) a hard cap of 100 so a firehose cannot starve the frame;
(4) idempotent events (`Wakeup`) deduped to a single flag instead of queued; (5) `yield_now()`
between batches. The frame-side counterpart is `terminal.sync(window, cx)` once during prepaint,
which drains the queued internal events and republishes `last_content` — so the grid is
snapshotted exactly once per frame no matter how many PTY events arrived.

**fleetd** implements (1), (3) and (5) and adds a damage classifier, but has no timer window:

```rust
// crates/fleet-app/src/shell/root/events.rs:16, 111-152 (trimmed)
const EVENT_BATCH_LIMIT: usize = 128;

while let Ok(first) = events.recv().await {
    let mut batch = Vec::with_capacity(EVENT_BATCH_LIMIT);
    batch.push(first);
    while batch.len() < EVENT_BATCH_LIMIT {
        let Ok(event) = events.try_recv() else { break };
        batch.push(event);
    }
    let updated = shell.update(cx, |shell, cx| {
        let damage = apply_batch(&shell.state, batch, cx);
        let terminal_update = !damage.state && damage.affects_visible_terminal(shell.state.read(cx));
        terminal_update.then_some(shell.window).flatten()
    });
    if let Some(window) = updated.ok().flatten() {
        // Release the shell lease before entering its window: plain terminal output
        // updates prepared surfaces without waking AppState/chrome observers.
        let _ = window.update(cx, |_, window, cx| { /* synchronize_surfaces + notify */ });
    }
}
```

The `EventDamage` split (`crates/fleet-app/src/presentation/damage.rs`) is fleetd's addition and
is *more* than Zed does: terminal-only damage updates prepared surfaces without waking
`AppState`/chrome observers. This is what keeps 240 `cx.notify()` sites affordable when 354 of
`fleet-app`'s entity references are the single `Entity<AppState>` — there is no way to notify a
subtree in this architecture, so coalescing upstream is the whole defence.

Agent `ContentDelta`s are coalesced "per item on a 16 ms tick before broadcast"
(`docs/ARCHITECTURE.md:218`) on the daemon side. If the app ever grows an app-side PTY pump that
is hotter than the bridge batch, Zed's 4 ms + cap-100 window is the shape to port.

---

## p10 — Cheap-to-clone types {#p10-cheap-types}

Zed's `.rules`: "`SharedString` is used to avoid copying strings, and is either an `&'static str`
or `Arc<str>`."

```rust
// zed/crates/gpui_shared_string/gpui_shared_string.rs:15, 26-28
pub struct SharedString(SmolStr);
/// Creates a static [`SharedString`] from a `&'static str`.
pub const fn new_static(str: &'static str) -> Self { Self(SmolStr::new_static(str)) }
```

Two custom lints police it. `SHARED_STRING_FROM_STR_LITERAL` flags `SharedString::from("…")` and
`"…".into()` with the cost model spelled out: `SmolStr::from` either copies into inline storage
(≤ 23 bytes) or allocates a fresh `Arc<str>`, while `new_static` stores the `'static` pointer.
`OWNED_STRING_INTO_SHARED` (`zed/tooling/lints/src/owned_string_into_shared.rs:24-33`) flags
`String::from("foo").into()` → `SharedString`/`Arc<str>`/`Rc<str>`/`Cow<str>`: "Two heap
allocations and two copies of the literal happen where one is enough."

| Type | Zed count | When | fleetd |
|---|---:|---|---|
| `SharedString` | 3394 | any string held by a view or passed to an element | 530 in ui-kit, 243 in app — idiom known |
| `SharedString::new_static` | 120 | every literal that becomes a `SharedString` | 59 — under-used |
| `impl Into<SharedString>` | 233 | constructor params, so callers pass `&'static str` free | 160 — good |
| `Arc<str>` | 651 | shared owned strings outside the UI layer | — |
| `Arc<[T]>` | 119 | immutable shared slices; `Arc::make_mut` for CoW | `Arc<[GridRow]>`, `terminal/presentation.rs:188` |
| `SmallVec<[T; N]>` | 252 | per-frame collections with a known small size | **0** |
| `collections::HashMap` (`FxHashMap`) | 459 `use collections::` | non-cryptographic hashing everywhere | **0**; `std::collections::HashMap` used directly |
| `SumTree<T>` | — | ordered collection needing O(log n) index↔offset | — (only inside gpui's `ListState`) |
| `postage::watch` | 46 | latest-value-wins broadcast; avoids a notify storm | — (fleetd uses `async_channel` + damage) |

`zed/crates/collections/src/collections.rs:1-2` is the whole trick:
`pub type HashMap<K, V> = FxHashMap<K, V>;`. `zed/crates/gpui/src/elements/uniform_list.rs:70-73`
is the canonical `SmallVec` use: `items: SmallVec<[AnyElement; 32]>`.

**When NOT to:** `SmallVec`'s inline capacity is stack space — do not inline a large `N` in a type
that itself lives in a `Vec`. `FxHashMap` is not DoS-resistant; keep SipHash for
attacker-controlled keys.

---

## p11 — Hygiene by lint, not by review {#p11-lints}

Zed root `Cargo.toml:1076-1084`:

```toml
[workspace.lints.clippy]
dbg_macro = "deny"
todo = "deny"
declare_interior_mutable_const = "deny"
redundant_clone = "deny"
disallowed_methods = "deny"
```

Note what Zed *disables* and why: `style` is allowed because `./script/clippy` takes minutes and
slowing shipping is judged worse; `type_complexity`, `too_many_arguments`, `large_enum_variant`
and `nonminimal_bool` are allowed with a stated reason each. Deny the **hazards**, allow the
**taste**. `clippy.toml` additionally disallows `std::process::Command::{spawn,output,status,…}`
("can block the current thread for an unknown duration" → `smol::process::Command`) and
`serde_json::from_reader` ("Parsing from a buffer is much slower than first reading the buffer
into a Vec/String … Use `serde_json::from_slice`"). `ASYNC_BLOCK_WITHOUT_AWAIT`
(`zed/tooling/lints/src/lib.rs`) catches "an async block without an `.await` wraps synchronous
code in a `Future` state machine for no benefit."

**fleetd** already has the same philosophy, in its own words:

```toml
# Cargo.toml:21-32
# Hazards only. Clippy's default `style`, `complexity`, `perf` and `correctness`
# groups stay at warn and are enforced by `-D warnings` in `make clippy`, so this
# list never grants an allowance — it only raises what a warning would let slip.
[workspace.lints.clippy]
dbg_macro = "deny"
declare_interior_mutable_const = "deny"
disallowed_methods = "deny"
todo = "deny"
unimplemented = "deny"
# `redundant_clone` belongs here too, but the app and lazygit render paths still
# trip it. Add it once `cargo clippy --workspace --all-targets --all-features --
# -W clippy::redundant_clone` is quiet.
```

Entry point: `make lint` → `fmt-check` + `cargo clippy --workspace --all-targets --all-features
-- -D warnings` (`Makefile:58-64`), and `make ci` = `lint test test-scripts`. Zed's
`script/clippy` additionally runs `cargo shear` for unused deps; adding that is cheap.

Allocation counts to attack, in order (`rg -c`, production + tests):
`crates/fleet-app/src/dialogs/palette.rs` 42 `.clone()` + 23 `.to_owned()`;
`views/worktrees_list.rs` 17 + 32; `views/prs_screen.rs` 11 + 18;
`crates/fleet-lazygit/src/panels/main_panel.rs` 15 + 11. Whole crates (`src/`, tests included):
fleet-app 1 240 `.clone()`, fleet-lazygit 308, fleet-ui-kit 165.

The single clearest per-frame allocation in a render path:

```rust
// crates/fleet-app/src/dialogs/palette.rs:955-957
row = row.leading(StatusGlyph::new(status).id(gpui::SharedString::from(
    format!("palette-glyph-{}", entry.label),
)));
```

An `ElementId` built from a `format!` allocates once per row per frame. gpui v1.18.1 has
`impl From<(&'static str, usize)> for ElementId` (`zed/crates/gpui/src/window.rs:6739`), plus
`(&'static str, u32)`, `(&'static str, u64)`, `(&'static str, EntityId)` and
`(SharedString, usize)` — use one of those.

---

## p12 — Notify hygiene {#p12-notify}

`cx.notify()` is cheap and Zed calls it 2137 times. `App::notify` only wakes windows currently
*displaying* the entity, and repeated notifies inside one frame collapse into a single dirty
flag (`zed/crates/gpui/src/window.rs:165-195`). The real rules are:

- Never in `render` (p1).
- Notify on an actual change when the notify has a side effect beyond a repaint. Zed's comment in
  `zed/crates/markdown/src/mermaid.rs:559-565` is the model: "Only notify when the zoom actually
  changed. A no-op zoom (e.g. clamped at the min/max) must not pause tail-following…"
- Coalesce *upstream* of notify (p9), never by throttling notify itself.
- `window.refresh()` marks the whole window dirty — reach for it only when no single entity owns
  the change (theme, settings, font).
- `window.request_animation_frame()` schedules a notify on the next frame; a continuous animation
  frame request is a permanent 120 fps tax. Decorative motion should use `AnimationExt::with_animation`,
  which respects `App::reduce_motion`.

**fleetd** has 240 `cx.notify()` in `fleet-app`, 123 in `fleet-lazygit`, 27 in `fleet-ui-kit`.
Because 354 of `fleet-app`'s 403 `Entity<…>` references are the one `Entity<AppState>`, every
`state.update(cx, |…| cx.notify())` repaints the whole tree. That is architectural, not a bug —
the defence is the event batch + damage split (p9), and the fix for a specific storm is upstream
coalescing, never a throttle.

---

## p13 — Measurement {#p13-measurement}

- **Budget.** `zed/CONTRIBUTING.md:129-134`: "All user interactions must have instant feedback…
  Does it handle large files, big projects, or heavy workloads without degrading? **Frames must
  take no more than 8ms (120fps)**". fleetd's `docs/DEVELOPMENT.md` has no performance section —
  adding one is a one-line doc change with real review value.
- **Benches.** Zed's `crates/benchmarks` uses `criterion` with `harness = false` and
  `gpui = { features = ["bench-support"] }`; benches are written with
  `#[gpui::bench(inputs = …, group = …, sample_size = …)]` against a real `BenchAppContext` and a
  real window (`benches/editor_render.rs`, `benches/display_map.rs`, `benches/markdown_renderer.rs`).
  fleetd's only bench is `crates/fleet-term/benches/viewport.rs:8-10`, a `#[test] #[ignore]`
  manual `Instant` timer with no `[[bench]]` / `harness = false` entry in its `Cargo.toml`.
  Convert it before quoting numbers, and add a render-path bench (transcript list with N rows,
  terminal grid 200×60).
- **Tracing.** Zed's `crates/ztracing` wraps `tracing` so `#[instrument]` and `*_span!` compile to
  nothing unless a cfg is on, and puts `#[instrument(skip_all)]` on `SumTree` and `Rope` ops.
  fleetd has `tracing` as a workspace dep and uses it in hot paths already
  (`crates/fleet-app/src/terminal/presentation.rs`, `shell/root/events.rs:139-144` logs
  `events`/`recovery`/`state_notify` per batch) but has no zero-cost gate; `tracing`'s own
  `max_level_*` features are the cheap option.
- **Frame profiler.** gpui has an optional `profiler` feature that instruments `Window::draw` and
  records how many invalidations were coalesced into each frame (`FrameDirtyAccumulator`,
  `zed/crates/gpui/src/window.rs:131-136`). fleetd pins gpui with `default-features = false`
  (`Cargo.toml:53`), so enabling it is a feature flip on a profiling build.

**Open question carried from the source report:** at v1.18.1 there is no `crates/perf` (the
`perf` package lives at `tooling/perf/`) and no `--perf` CLI flag in Zed; only `.cargo/config.toml`'s `perf-test`/`perf-compare` aliases and the
`profiler` feature. Do not cite `zed --perf` or `util::measure` — neither exists at this tag.

---

## p14 — Build profiles {#p14-build-profiles}

Zed's root `Cargo.toml`:

```toml
[profile.dev]                     # split-debuginfo = "unpacked", incremental,
                                  # codegen-units = 16, debug = "limited"
[profile.dev.build-override]      # mirrors it, else ~400 crates compile twice
[profile.dev.package]             # opt-level = 3 for syn/quote/proc-macro2/serde_json/
                                  # tree-sitter/taffy/wasmtime…; codegen-units = 1 for
                                  # single-file crates
[profile.release]                 # zed/Cargo.toml:1059-1063
debug = "limited"
lto = "thin"
codegen-units = 1
[profile.release.package]
zed = { codegen-units = 16 }      # the leaf binary trades a little perf for link time
[profile.release-fast]            # zed/Cargo.toml:1067-1071 — for perf runs
inherits = "release"
lto = false
codegen-units = 16
```

`.cargo/config.toml` adds `-C symbol-mangling-version=v0` ("provides more detailed backtraces
around closures") and a faster linker on some targets.

**fleetd** has only:

```toml
# Cargo.toml:81-88
[profile.dev]
opt-level = 1

# Dependencies are optimized, so their debuginfo is rarely useful and it dominates
# target/ size. Workspace crates keep full debuginfo.
[profile.dev.package."*"]
opt-level = 3
debug = false
```

no `[profile.release]`, no `release-fast`, and no `.cargo/` directory. It does already trim gpui's
features (`Cargo.toml:53`, `default-features = false` — drops wayland/x11/font-kit/windows-manifest,
with `gpui_platform` supplying the platform at `:54`) and keeps `target/` bounded with
`cargo sweep` in `make prune` (`Makefile:71-78`). Adding `[profile.release]` with `lto = "thin"`,
`codegen-units = 1`, `debug = "limited"` plus a `release-fast` profile is the missing half, and
`make run-release` (`Makefile:27-28`) already has a caller for it.
