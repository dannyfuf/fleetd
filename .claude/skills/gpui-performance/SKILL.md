---
name: gpui-performance
description: Render discipline, memoised projections, list virtualization, background offload with cancellation, allocation hygiene, frame budget and build profiles for fleetd's GPUI app. Load before writing or reviewing any `render` / `render_prepared` / `RenderOnce::render` body, before adding a list or grid that can exceed the viewport, before adding a custom `impl Element`, when a screen recomputes a derived model or filters on every keystroke, when work on the foreground thread parses/diffs/hashes/sorts/highlights, when a producer (PTY, agent stream, watcher) fires faster than a frame, or when the UI feels janky and you need a budget and a measurement. Also load on the phrases "slow render", "per-frame allocation", "memoise", "virtualize the list", "debounce", "background_spawn", "redundant_clone", "frame budget".
---

# GPUI performance in fleetd

How to keep fleetd's foreground thread under budget: render as a pure projection, derive once
and cache, render only what is visible, push everything else to the background executor, and
keep hot-path types cheap to clone. Patterns verified against Zed v1.18.1, the GPUI tag fleetd
depends on (`Cargo.toml:53`, `gpui` with `default-features = false`).

The governing fleetd docs are `docs/APP-CONTRACTS.md:101-103` ("**Render prepares nothing.**")
and `docs/ARCHITECTURE.md:384-386` ("a screen prepares in `synchronize` … `render_prepared`
only composes what is already prepared"). Code and doc change in the same commit.

## When to use

- Writing or reviewing a `render`, `render_prepared` or `RenderOnce::render` body in
  `fleet-app`, `fleet-ui-kit` or `fleet-lazygit`, or adding a custom `impl Element`.
- Adding a list, table, board column, transcript, log or grid whose row count is bounded by
  data rather than by the viewport.
- A screen derives a model — filtering, sorting, grouping, fuzzy matching, formatting — from
  `AppState`.
- Moving work off the foreground thread: parsing, diffing, hashing, syntax highlighting,
  serialising, filesystem walks, subprocesses.
- A producer emits faster than 120 fps: PTY output, agent `ContentDelta`s, file watchers.
- Investigating jank, a notify storm, or a `redundant_clone` warning.

## When not to

- Correctness and clarity first: Zed's `.rules` says "Speed and efficiency are secondary
  priorities unless otherwise specified." Do not obfuscate a reducer to save an allocation.
- Trivially cheap derivations (a bool, a count, one `format!` on data in hand) need no cache —
  invalidating the cache is the expensive part.
- Daemon-side (`fleet-daemon`, `fleet-client`, `fleet-core`) throughput and tokio work: that is
  `rust-async-background-work`. This skill is about the GPUI frame.

## Rules

1. **Render is a pure projection.** No IO, no `cx.notify()`, no mutating `entity.update(…)`, no
   parsing, highlighting or sorting inside a render body. Zed enforces this with dylint lints
   (`tooling/lints/src/{notify_in_render,entity_update_in_render,blocking_io_on_foreground}.rs`);
   fleetd enforces it with prose (`docs/APP-CONTRACTS.md:101`). An `.update()` that only *reads*
   a projection and returns a value is fine — Zed's lint deliberately skips it.

2. **Prepare in `synchronize`, an observation or a background task; render composes.** This is
   fleetd's own contract, not an import. `WorkspaceScreen` does it right: `Model::build` runs in
   `synchronize` (`crates/fleet-app/src/screens/workspace/lifecycle.rs:307`) and
   `render_prepared` only reads `self.model` (`screens/workspace.rs:124-134`).

3. **Memoise a derived model behind a revision key, not a timestamp.** The pattern is
   `crates/fleet-app/src/screens/hub/projection.rs:4-38`: a `ProjectionKey` of cheap comparable
   values (`snapshot_revision`, `presentation_revision`, `link_generation`, scope, pane, tab,
   screen, query) → `Rc<HubModel>`, with `snapshot_revision` bumped on every applied mutation
   (`state/snapshot.rs:97-100`). **Only the Hub does this today**; the board is next.

4. **Never build elements for rows you cannot see.** `uniform_list` for fixed-height rows,
   `gpui::list` + `ListState` (with a deliberate `overdraw`) for variable heights; both compute
   the visible range first. fleetd uses both — `fleet-ui-kit/src/components/list_view.rs:385`,
   `log_view.rs:170`, `agent/transcript_list.rs:219`, `fleet-app/src/screens/jobs.rs:156` — and
   `fleet-app` calls `uniform_list` zero times directly, so go through the kit.

5. **`uniform_list` rows must be provably uniform, and `ListState` mutations use `splice`.**
   `uniform_list` measures the first row and extrapolates, so one wrapped row corrupts every
   offset below it (documented at `fleet-lazygit/src/panels/main_panel.rs`); `splice(range,
   count)` invalidates only the changed span while `reset` discards every measured height.

6. **A custom `Element` does its range math in `prepaint` and carries results in
   `PrepaintState`.** Same shape as `uniform_list::prepaint`
   (`zed/crates/gpui/src/elements/uniform_list.rs:473-490`). State that must survive the frame
   goes in `window.with_element_state`/`use_keyed_state` (`zed/crates/gpui/src/window.rs:3794`);
   fleetd uses it once (`fleet-ui-kit/src/components/terminal_tab_strip.rs:279`).

7. **Keep hot text as `SharedString` end to end.** GPUI's `LineLayoutCache` carries shaped lines
   across two frames (`zed/crates/gpui/src/text_system/line_layout.rs:454-457, 594-610`), so
   re-shaping the *same* `SharedString` is nearly free and a fresh `String` each frame defeats
   it. Shape through `window.text_system()`, never a freshly constructed `WindowTextSystem`.

8. **CPU work goes to the background executor; the foreground half only applies the result.**
   `cx.background_spawn(f)` and `cx.background_executor().spawn(f)` are the same executor —
   `fleet-app` uses the longer spelling (3 sites), `fleet-lazygit` the shorter
   (`diff_view.rs:415-425`). Nothing that parses, diffs, hashes or serialises is offloaded yet.

9. **Store a `Task` in a field when a newer request should supersede it.** Reassigning the field
   drops the old task, so cancellation is automatic — fleetd does this in `screens/hub.rs:129-130`,
   `screens/workspace.rs:212`, `dialogs/host.rs:55`, `shell/root.rs:71-72`. `.detach()` is for
   infallible fire-and-forget only; fallible work uses `.detach_and_log_err(cx)` (present in the
   pinned gpui, `zed/crates/gpui/src/executor.rs:37`). fleetd has 33 bare `.detach()` and zero of it.

10. **Every long background loop yields and has an explicit budget.** `yield_now().await` on an
    interval plus a line or time cap, as in `fleet-lazygit/src/views/syntax.rs:240-263`
    (`MAX_LINES = 40_000`, `BUDGET = 1.5s`, per-job and per-line `AtomicBool`).

11. **Debounce with `cx.background_executor().timer(..)`, cancelled by dropping the task.** Never
    `smol::Timer` (non-deterministic under `run_until_parked`), never a busy poll. fleetd has 14
    timer sites and generation counters (`screens/hub.rs:128`) but **zero** `select_biased!`: the
    macro is `futures_util::select_biased!`, and `fleet-app`'s manifest does not yet take that dep.

12. **Coalesce a high-frequency producer at the source, not by throttling `notify`.** First event
    immediate for latency, then a bounded window with a hard count cap, idempotent events deduped
    to a flag. fleetd's shell batches bridge events (`shell/root/events.rs:16,111-152`,
    `EVENT_BATCH_LIMIT = 128`) and splits terminal-only from state damage — that is what makes 240
    `cx.notify()` calls affordable when 354 entity references are the one `Entity<AppState>`.

13. **Prefer cheap-to-clone types in anything a frame touches.** `SharedString` (and
    `SharedString::new_static` for literals), `Arc<[T]>` with `Arc::make_mut` for copy-on-write,
    `SmallVec<[T; N]>` for small per-frame vectors, an Fx-hashed map for non-adversarial keys.
    fleetd has `Arc<[GridRow]>` (`terminal/presentation.rs:188`) but no `SmallVec`/`FxHashMap`;
    adding either crate to `[workspace.dependencies]` is its own commit, not a drive-by.

14. **Make hygiene mechanical.** `make lint` runs `cargo clippy --workspace --all-targets
    --all-features -- -D warnings` (`Makefile:61-62`). `Cargo.toml:24-32` denies the hazards and
    names the one missing lint plus its exit criterion: "`redundant_clone` belongs here too, but
    the app and lazygit render paths still trip it." Do not add new clones to a render path.

15. **Budget the frame and prove the claim.** Zed's bar is "Frames must take no more than 8ms
    (120fps)" (`zed/CONTRIBUTING.md:134`); fleetd states no equivalent. Adopt it as the review
    bar, and back "this is faster now" with a bench or a `tracing` span timing. fleetd's only
    bench is a `#[test] #[ignore]` manual timer (`crates/fleet-term/benches/viewport.rs:8-10`)
    and root `Cargo.toml` has no `[profile.release]` at all.

## Core patterns

### Memoised projection (the pattern to copy)

```rust
#[derive(Default)]
pub(super) struct ProjectionCache { key: Option<ProjectionKey>, model: Rc<BoardModel> }

/// Every input the model is derived from, as revisions and cheap values.
#[derive(PartialEq, Eq)]
struct ProjectionKey {
    source: u64,          // AppState::snapshot_revision
    board: Option<BoardId>,
    query: String,
    secondary: Option<GroupBy>,
}

pub(super) fn prepare(state: &AppState, cache: &RefCell<ProjectionCache>) -> Rc<BoardModel> {
    let key = ProjectionKey { /* … */ };
    let mut cache = cache.borrow_mut();
    if cache.key.as_ref() != Some(&key) {
        cache.model = Rc::new(model(state));
        cache.key = Some(key);
    }
    cache.model.clone()
}
```

Full version with the Hub's real key: `references/patterns.md#p2-memoised-projection`.

### Virtualized rows through the kit

```rust
// Fixed-height rows: the kit wraps uniform_list and owns the UniformListScrollHandle.
// The closure is called only for the visible range; `rows` is the prepared model.
let list = ListView::new("board-column", rows.len(), move |index, is_cursor, _w, cx| {
    let Some(row) = rows.as_ref().get(index) else {
        return div().into_any_element();
    };
    card_row(row, is_cursor, cx)
})
.cursor(cursor)
.track_scroll(scroll);

// Variable-height rows: ListState, invalidating only the changed span.
self.list.splice(changed..changed + removed, inserted);
```

See `references/patterns.md#p3-uniform-list` and `#p4-list-state`.

### Background offload with cancellation

```rust
// Reassigning the field drops the previous task, so the stale run stops.
self.highlight_task = cx.spawn(async move |this, cx| {
    let prepared = cx
        .background_spawn(async move { highlight(&source, &cancelled) })
        .await;
    this.update(cx, |this, cx| {
        this.prepared = prepared;
        cx.notify();
    })
    .ok();
});
```

`cx.background_spawn` requires `Send` and entity handles are not `Send` — that is why the
foreground half applies the result. See `references/patterns.md#p7-background-cancel`.

### Debounce: timer raced against cancellation

```rust
self.task = Some(cx.spawn(async move |this, cx| {
    let mut timer = cx.background_executor().timer(INSPECT_DEBOUNCE).fuse();
    futures_util::select_biased! { // biased: cancellation first, deterministically
        _ = receiver => return,
        _ = timer => {}
    }
    let _ = this.update(cx, |this, cx| this.inspect(cx));
}));
```

Needs `futures-util.workspace = true` in `crates/fleet-app/Cargo.toml`; name the constants centrally.

### Coalescing a high-frequency producer

```rust
while let Ok(first) = events.recv().await {
    let mut batch = Vec::with_capacity(EVENT_BATCH_LIMIT);
    batch.push(first);                       // first event applied uncoalesced
    let mut timer = cx.background_executor().timer(COALESCE_WINDOW).fuse();
    while batch.len() < EVENT_BATCH_LIMIT {  // hard cap: a firehose cannot starve the frame
        futures_util::select_biased! {
            _ = timer => break,
            event = events.recv().fuse() => match event {
                Ok(event) => batch.push(event),
                Err(_) => return,
            },
        }
    }
    apply_batch(&state, batch, cx);
    smol::future::yield_now().await;         // never hold the executor
}
```

fleetd's live version is `shell/root/events.rs:111-152` (count cap, no timer window);
`references/patterns.md#p9-coalesce` has Zed's PTY loop, which adds the 4 ms window.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
|---|---|---|
| `cx.notify()` or a mutating `entity.update(…)` inside a render body | Every render schedules another render: a loop, wasted frames, inconsistent UI | Notify and mutate from the mutation site — `synchronize` or an observation |
| `std::fs`, `Path::{exists,metadata,read_dir}`, `Command`, `thread::sleep` reachable from a fn taking `&App` / `&Context<T>` / `&mut Window` | Freezes the app for the syscall | Background task; fleetd's 4 blocking sites are all already guarded |
| Filtering/sorting/grouping/lowercasing in render | O(N) per frame regardless of what changed | Memoised projection keyed on `snapshot_revision` |
| `parse_markdown` / syntax highlight inside `RenderOnce::render` | Re-parses the same text 120×/s once the row is on screen | Parse once at ingest, cache ranges next to the text |
| Iterating all N rows to build elements | Cost scales with data, not viewport | `uniform_list` / `gpui::list` |
| `ListState::reset` after any change | Discards every measured height | `splice(changed_range, count)` |
| `WindowTextSystem::new(cx.text_system().clone())` | Bypasses the window's two-frame `LineLayoutCache`; every line re-shaped | `window.text_system().shape_line(…)` |
| `format!("row-{}", label)` as an `ElementId` | One heap allocation per row per frame | `ElementId::from(("row", index))` |
| `SharedString::from("literal")` / `String::from("x").into()` | Copy or a second heap allocation for a `&'static str` | `SharedString::new_static("literal")` |
| `.detach()` on fallible work | Errors vanish silently; the task outlives its screen | `.detach_and_log_err(cx)` or a `Task` field |
| `smol::Timer::after` | Non-deterministic under `run_until_parked()` in `#[gpui::test]` | `cx.background_executor().timer(..)` |
| An unbounded background loop with no yield | Starves the shared background pool | `yield_now().await` + an explicit line/time budget |
| Throttling `cx.notify()` to fix a storm | Notifies are already coalesced per frame; the cost is upstream | Coalesce the producer (batch + cap) |

## fleetd-specific guidance

**Keep doing** (at or above Zed's bar — do not "fix" these): `TerminalGridCache` keyed on
`Arc::ptr_eq(rows)` + theme + visible range (`fleet-ui-kit/src/components/terminal_grid/batching.rs:243-274`);
copy-on-write row snapshots with `Arc::make_mut`, tested with `Arc::ptr_eq` across 120
cursor-only frames (`fleet-app/src/terminal/presentation.rs:186-190, 262-281`);
`shape_line(…, force_width)` per batch through `window.text_system()`
(`terminal_grid/painter.rs:41,106`); the bridge event batch and terminal-vs-state damage split
(`shell/root/events.rs`, `presentation/damage.rs`); zero blocking IO on the foreground thread;
`_subscriptions`/`_tasks` retention (`shell/root.rs:71-72`); `gpui = { default-features = false }`.

**Gap 1 — the board recomputes and re-lowercases on every frame.**
`views/board_screen.rs:88` calls `grouped_cards(view, props.filter)` from inside the render
function, which reaches `visible_cards` → `matches_needle` (`views/board_screen/model.rs:9-34`).
The needle is lowercased once, but `text.to_lowercase()` allocates **per card per field** —
title, remote key, local key, assignee, every label id and name. `BoardScreen::render`
(`screens/board.rs:102-161`) runs on every frame of the Hub body, so a 200-card board pays
hundreds of allocations at 120 fps. Fix in two commits:
1. `board: match the needle without allocating` — in `matches_needle`, replace
   `text.to_lowercase().contains(needle)` with an ASCII-case-insensitive substring scan (or a
   lowercased haystack cached beside the card when the snapshot is applied in
   `state/board.rs`). Local, no signature change.
2. `board: memoise the column model` — add `projection: RefCell<ProjectionCache>` to
   `BoardScreen` (`screens/board.rs:58-68`) and a `screens/board/projection.rs` modelled
   line-for-line on `screens/hub/projection.rs:4-38`, keyed on `(state.snapshot_revision, board
   id, filter, filter_editing, group_secondary)` → `Rc<BoardModel>` with **owned** rows
   (`Rc<[ColumnRows]>`, not borrows of `BoardView`). Prepare it in `screens/board/lifecycle.rs`'s
   synchronize path and have `board_screen::render` take `&BoardModel` in `BoardProps`.
   `screens/board/navigation.rs:8,31,58,135` re-runs `visible_cards` per key press — point those
   at the same model.

**Gap 2 — the palette rebuilds every candidate on every `AppState` notify.**
`dialogs/host.rs:287-288` calls `palette::refresh(&state, cx)` unconditionally from
`cx.observe(state, …)` whenever the palette overlay is open. The keystroke path is guarded —
`refresh_query` (`palette.rs:1093-1100`) compares `prepared_query` before rebuilding — but the
observation path is not, so terminal output or a PR poll that bumps `AppState` rebuilds all rows.
`candidates` (`palette.rs:665`) allocates a `SnapshotIndex` and an owned `Entry` per row, in a
file with 42 `.clone()` and 23 `.to_owned()`; its own doc comment at `palette.rs:662-664` names
the cost. Fix: widen the `prepared_query` guard into a projection key `(snapshot_revision, query,
behind, detail_card)` stored next to `host.palette.rows`, and route both `host.rs:132` and
`host.rs:287` through it. Do not restructure the 2 046-line file for this — that split belongs
to `gpui-app-shell`.

**Correction to the audit:** the Workspace does **not** build a fresh `Model` per frame.
`Model::build` runs in `synchronize` (`screens/workspace/lifecycle.rs:307`) and
`render_prepared` reads `self.model` (`screens/workspace.rs:132`). The board is the only screen
with no cache at all.

**Gap 3 — per-frame markdown parsing and highlighting in the kit.**
`fleet-ui-kit/src/components/markdown_text.rs:385` calls `parse_markdown(self.source.as_ref())`
inside `RenderOnce::render` (`:377-378`), and `components/markdown/render.rs:178` calls
`code::highlight(lang, text)` per frame per visible code block, then allocates
`SharedString::from(text.to_owned())` at `:181`. Virtualization bounds this to visible rows,
which hides the cost until a long transcript row scrolls. Fix at ingest: parse once into a
prepared document, cache the highlight ranges beside it, and let render only map ranges →
`HighlightStyle` and pass the existing `SharedString` through untouched.

**Gap 4 — `WindowTextSystem::new` bypasses the shaping cache.**
`fleet-lazygit/src/views/diff.rs:496`, `views/long_line.rs:166`, `diff_view.rs:246` and
`root/diff_view.rs:38` build a fresh `WindowTextSystem` instead of using `window.text_system()`,
so no shaped line is reused across frames. Route through the window where a `&mut Window` is in
scope; where the work runs off-window, shape once into a prepared structure, never per frame.

**Gap 5 — allocation hygiene is not mechanical yet.** `Cargo.toml:30-32` names the exit criterion
for `redundant_clone`. `fleet-app` has 1 240 `.clone()`, `fleet-lazygit` 308, `fleet-ui-kit` 165;
`dialogs/palette.rs:955-957` builds an `ElementId` with `format!("palette-glyph-{}", entry.label)`
per row per frame. There is no `smallvec` and no Fx-hashed map in the workspace. Order the work:
fix render-path clones → flip `redundant_clone = "deny"` → then `SmallVec` and a `collections`
alias.

**Gap 6 — no budget, no bench, no release profile.** `docs/DEVELOPMENT.md` has sections for
commands, `target/`, lints, Zig, scripting and logs but no performance section; add the 8 ms
frame budget there in the same commit as the first perf fix. `crates/fleet-term/benches/viewport.rs`
is a `#[test] #[ignore]` manual timer with no `[[bench]]`/`harness = false` in its `Cargo.toml` —
convert it to `criterion` before quoting numbers from it. Root `Cargo.toml` has only
`[profile.dev]` (`:81-88`); add `[profile.release]` with `lto = "thin"`, `codegen-units = 1`,
`debug = "limited"` plus a `release-fast` profile, mirroring `zed/Cargo.toml:1059-1071`.

## Review checklist

1. Does any render body parse, sort, filter, lowercase, hash, highlight or touch the filesystem?
2. Is every derived model memoised behind a revision key, or provably too cheap to cache?
3. Can this list exceed the viewport, and if so does it go through `uniform_list` / `gpui::list`?
4. Are `uniform_list` rows provably uniform-height, and do `ListState` mutations use `splice`?
5. Does a custom `Element` compute its visible range in `prepaint` and carry state forward?
6. Is CPU work on `cx.background_spawn` with the foreground half only applying the result?
7. Is a task a newer request supersedes stored in a field, `.detach()` only on infallible work?
8. Does every long background loop yield and carry an explicit line or time budget?
9. Are hot-path strings `SharedString` (literals via `new_static`) and `ElementId`s built from
   `(&'static str, index)`?
10. Is text shaped through `window.text_system()`, and does `make lint` pass clean?

Full reviewer list: `references/checklist.md`.

## Related skills

- `gpui-state-and-memory` — entities, `WeakEntity`, subscriptions, `Task` ownership, globals.
- `gpui-components` — `RenderOnce` vs `Render`, builders, kit list components, the gallery gate.
- `gpui-styling` — reading the theme in render, tokens, animation and `reduce_motion`.
- `rust-async-background-work` — executors, channels, blocking IO, the daemon's tokio side.
- `rust-gpui-testing` — deterministic timers; `gpui-app-shell` — dialogs and the palette split.
- `zed-quality-review` — aggregates this skill's `references/checklist.md`.
