# gpui-performance — reviewer checklist

Standalone. Apply to any fleetd PR that touches a render path, a list, a custom `Element`, a
background task, or a hot loop. Each item is yes/no; "no" means the fix on the right is required
or the PR must say why it is not. Zed citations are tag **v1.18.1**; fleetd paths are repo-relative.

Verification commands: `make lint` (fmt-check + `cargo clippy --workspace --all-targets
--all-features -- -D warnings`), `make test`, `make check`.

---

## A. Render purity

1. **Is every `render` / `render_prepared` / `RenderOnce::render` body free of `std::fs`,
   `Path::{exists,try_exists,is_file,is_dir,metadata,canonicalize,read_dir}`,
   `std::process::Command`, `thread::sleep` and network calls — including anything reachable
   from a fn taking `&App`, `&mut App`, `&Context<T>`, `&mut Context<T>` or `&mut Window`?**
   *Why:* the foreground thread is the only UI thread; the syscall duration is the freeze
   duration. *Fix:* move it into `cx.background_executor().spawn` / `cx.background_spawn` and
   apply the result from the foreground half.

2. **Is the body free of `cx.notify()`?**
   *Why:* every render would schedule another render — a loop or wasted frames.
   *Fix:* notify from the mutation site.

3. **Is the body free of a *mutating* `entity.update(…)`?** (An `.update()` that only reads a
   projection and returns a value is fine.)
   *Why:* mutating mid-render gives inconsistent UI and can re-enter render.
   *Fix:* mutate in `synchronize`, an observation, or a task.

4. **Is the body free of parsing, diffing, hashing, sorting, grouping, fuzzy matching, syntax
   highlighting and `to_lowercase()`?**
   *Why:* `docs/APP-CONTRACTS.md:101` — "Render prepares nothing." These are O(N) every frame
   whether or not anything changed. *Fix:* derive in `synchronize`/an observation/a background
   task and cache on the screen (see B1).

5. **Is the body free of `format!`, `to_string()` and `to_owned()` on values that could have
   been prepared upstream, and do string literals reach `SharedString` via `new_static`?**
   *Why:* a heap allocation per row per frame, and a fresh `String` misses gpui's two-frame
   `LineLayoutCache`. *Fix:* prepare `SharedString`s in the model;
   `SharedString::new_static("…")` for literals.

6. **Are `ElementId`s built from a tuple, not a per-frame `format!`?**
   *Why:* same allocation, one per row per frame. *Fix:* `ElementId::from(("row", index))` —
   gpui v1.18.1 has `From<(&'static str, usize | u32 | u64 | EntityId)>` and
   `From<(SharedString, usize)>` (`zed/crates/gpui/src/window.rs:6733-6763`).
   Known offender: `crates/fleet-app/src/dialogs/palette.rs:955-957`.

7. **Is text shaped through `window.text_system().shape_line(…)` with a stable `SharedString`,
   and not through a freshly constructed `WindowTextSystem`?**
   *Why:* a new `WindowTextSystem` has an empty `LineLayoutCache`, so every line re-shapes every
   frame. *Fix:* use the window's. Known offenders: `crates/fleet-lazygit/src/views/diff.rs:496`,
   `views/long_line.rs:166`, `diff_view.rs:246`, `root/diff_view.rs:38`.

8. **Does the returned root element still call `.track_focus(focus)`?**
   *Why:* `docs/APP-CONTRACTS.md:107` — a perf refactor that drops it silently breaks keys.
   *Fix:* restore it; the app-shell skill covers focus.

## B. Derived models

1. **Is every derived model memoised behind a key of revisions and cheap values, or provably too
   cheap to cache?**
   *Why:* an unmemoised projection runs at 120 fps regardless of what changed.
   *Fix:* copy `crates/fleet-app/src/screens/hub/projection.rs:3-38` — a `ProjectionKey` →
   `Rc<Model>` in a `RefCell` on the screen struct.

2. **Is the key made of revisions and cheap comparables, never the data, and does it *exclude*
   the current time?**
   *Why:* keying on data makes the compare as expensive as the derivation; keying on `now`
   invalidates every second. *Fix:* `state.snapshot_revision` (bumped in
   `state/snapshot.rs:97-100`) plus the screen's own revision/generation fields; pass `now` as an
   input and record it (`HubModel::prepared_at`).

3. **Does the model own its rows (`Rc<[Row]>`), so renderer and key handlers consume the same
   derivation?**
   *Why:* two derivations drift; `screens/hub/navigation.rs:113-115` shows the one-model shape.
   *Fix:* return owned rows, not borrows of the snapshot.

4. **Does a new screen-level cache live on the screen struct, not in `AppState`?**
   *Why:* presentation is not truth; `AppState` is the reducer's.

## C. Lists and large content

1. **Can this collection exceed the viewport, and if so does it render through `uniform_list`
   (via `fleet_ui_kit::{ListView, LogView}`) or `gpui::list` + `ListState`?**
   *Why:* otherwise cost scales with data, not with what the user can see.
   *Fix:* `ListView::new(id, item_count, render_row)` (`crates/fleet-ui-kit/src/components/list_view.rs:294`).

2. **Are `uniform_list` rows provably uniform-height — no wrap, no conditional extra line, no
   variable badge stack?**
   *Why:* it measures the first row and extrapolates; one taller row corrupts every offset below
   (`crates/fleet-lazygit/README.md:41`, `panels/main_panel.rs:761`).
   *Fix:* fix the row, or move to `gpui::list` + `ListState`.

3. **Do `ListState` mutations use `splice(changed_range, count)` rather than `reset`, and is
   `overdraw` a deliberate value?**
   *Why:* `reset` discards every measured height. *Fix:* splice the changed span. fleetd sites:
   `components/agent/transcript_list.rs:219`, `screens/jobs.rs:156`, `dialogs/confirm.rs:76`.

4. **Does scroll-to-item go through the handle's deferred request (`ListView::reveal`,
   `scroll_to_item`, `scroll_to_reveal_item`) rather than mutating a scroll offset from render?**
   *Why:* gpui applies deferred scroll during the next prepaint; mutating from render is a
   render-time mutation. *Fix:* use the handle.

5. **Do background updates adopt new data without moving the cursor?**
   *Why:* `docs/DESIGN-SYSTEM.md:536-538` — "Background events … must **never** re-sort,
   re-scroll or re-focus." *Fix:* `ListCursor::retain(new_len, find_previous)`
   (`components/list_view.rs:247`).

6. **Does a custom `impl Element` compute its visible range in `prepaint` and carry results
   forward in `PrepaintState`?**
   *Why:* the `uniform_list` shape (`zed/crates/gpui/src/elements/uniform_list.rs:473-490`).
   *Fix:* move the range math into prepaint; use `window.with_element_state` /
   `use_keyed_state` for state that must cross frames (never `use_state` inside a per-item loop).

## D. Async and background work

1. **Is CPU-heavy work on `cx.background_spawn` (or `cx.background_executor().spawn`), with the
   foreground half only applying the result?**
   *Why:* `.rules` — "All use of entities and UI rendering occurs on a single foreground thread."
   *Fix:* the sandwich in `crates/fleet-lazygit/src/diff_view.rs:415-425`. Note `fleet-app` has
   51 `cx.spawn`, 3 `cx.background_executor().spawn` and zero `cx.background_spawn` today (the
   two spellings are the same executor).

2. **Is work that a newer request supersedes stored in a `Task` field (or guarded by a generation
   counter / `AtomicBool`) so the stale run stops?**
   *Why:* dropping a `Task` cancels it — that is the idiom. *Fix:* a field, as in
   `screens/hub.rs:129-130`, `screens/workspace.rs:212`, `dialogs/host.rs:55`,
   `shell/root.rs:71-72`. fleetd's existing generation guards (`board_generation`,
   `inspect_generation`, `link_generation`) are acceptable — do not rewrite them unless you are
   already touching that code.

3. **Is `.detach()` used only for infallible fire-and-forget?**
   *Why:* a detached fallible task discards its error silently.
   *Fix:* `.detach_and_log_err(cx)` (`zed/crates/gpui/src/executor.rs:37`) or await it in a
   retained task. fleetd: 33 bare `.detach()`, zero `detach_and_log_err`.

4. **Does every long background loop yield (`smol::future::yield_now().await`) on an interval and
   carry an explicit line or time budget?**
   *Why:* the background executor is a shared pool.
   *Fix:* the shape in `crates/fleet-lazygit/src/views/syntax.rs:32-35, 261`
   (`MAX_LINES`, `BUDGET`, per-job and per-line cancel checks).

5. **Are bursty triggers debounced with `cx.background_executor().timer(..)` — never
   `smol::Timer`, never a busy poll — and is the debounce constant named and centralised?**
   *Why:* `smol::Timer` is non-deterministic under `run_until_parked()` in `#[gpui::test]`.
   *Fix:* the `DebouncedDelay` shape (`zed/crates/project/src/debounced_delay.rs:26-52`). Racing
   the timer against a cancel receiver with `futures_util::select_biased!` is the nicer form, but
   `crates/fleet-app/Cargo.toml` does not yet take `futures-util` (it is in
   `[workspace.dependencies]`, `Cargo.toml:52`). Dropping the stored `Task` cancels it too; adding
   the manifest line is its own commit, not a drive-by.

6. **Does a producer faster than a frame coalesce — first event immediate, then a bounded window
   with a hard count cap, idempotent events deduped to a flag?**
   *Why:* otherwise it schedules a render per event.
   *Fix:* `crates/fleet-app/src/shell/root/events.rs:111-152` (cap 128) is the local shape;
   `zed/crates/terminal/src/terminal.rs:1354-1409` adds the 4 ms window.

7. **Does a fix for a notify storm coalesce upstream rather than throttle `cx.notify()`?**
   *Why:* gpui already collapses repeated notifies into one dirty flag per frame; throttling only
   adds latency. *Fix:* batch at the producer; classify damage
   (`crates/fleet-app/src/presentation/damage.rs`) so terminal-only output does not wake
   `AppState`/chrome observers.

## E. Types and allocation

1. **Are hot-path strings `SharedString`/`Arc<str>`, and do constructors take
   `impl Into<SharedString>`?**
   *Why:* the frame clones them; `SharedString` is `&'static str` or `Arc<str>`.
   *Fix:* thread `SharedString` from the model to the element.

2. **Are shared immutable slices `Arc<[T]>`, with `Arc::make_mut` for copy-on-write updates?**
   *Why:* pointer identity is what lets a downstream cache short-circuit (`Arc::ptr_eq`).
   *Fix:* the model in `crates/fleet-app/src/terminal/presentation.rs:186-230`.

3. **Are per-frame vectors with a small known size `SmallVec<[T; N]>`?** (fleetd has none yet;
   this is an "if you are adding one" item, not a rewrite request.)
   *Why:* avoids a heap allocation per frame — `UniformListFrameState { items: SmallVec<[AnyElement; 32]> }`.
   *Fix:* add `smallvec` where the size is genuinely bounded and small.

4. **Do maps with non-adversarial keys use an Fx hasher rather than SipHash?** (Also not yet
   present in fleetd.) *Why:* SipHash is DoS resistance nobody is paying for here.
   *Fix:* a `fleet-core::collections` alias to `rustc_hash`, as
   `zed/crates/collections/src/collections.rs:1-2` does.

5. **Does `make lint` pass clean, and if the PR touches a render path, does it also pass
   `-W clippy::redundant_clone`?**
   *Why:* `Cargo.toml:30-32` names exactly that as the exit criterion for turning the lint on.
   *Fix:* remove the clones; do not add new ones to a render path.

## F. Docs and evidence

1. **If the change touches a render contract, a list contract, or a screen's prepare/render
   split, does the same commit update the governing doc?**
   *Why:* `docs/` is authoritative (`docs/README.md`), and code that contradicts a doc is a bug
   in one of the two. *Fix:* `docs/APP-CONTRACTS.md` (render/prepare split, dialogs),
   `docs/ARCHITECTURE.md` (render discipline, event pipeline), `docs/DESIGN-SYSTEM.md`
   (component contracts, cursor stability), `docs/DEVELOPMENT.md` (commands, budgets).

2. **Is a claim of "this is faster now" backed by a bench, a `tracing` span timing, or a frame
   measurement, against the 8 ms/frame bar?**
   *Why:* `zed/CONTRIBUTING.md:134`. *Fix:* add the measurement, or drop the claim from the PR
   description. Note `crates/fleet-term/benches/viewport.rs` is a `#[test] #[ignore]` manual
   timer, not a `criterion` bench — convert it before quoting its numbers.

3. **Is the commit message `<area>: <imperative lowercase summary>` with the crate short name
   (`app:`, `ui-kit:`, `lazygit:`, `core:`, `daemon:`, `build:`, `docs:`)?**

---

## Quick greps

```sh
# render bodies that allocate or derive
rg -n -A 30 'fn render(_prepared)?\(' crates/fleet-app/src crates/fleet-ui-kit/src \
  | rg 'to_lowercase|to_owned\(\)|format!\(|sort_by|parse_|highlight'

# forbidden foreground IO reachable from a sync context
rg -n 'std::fs::|Path::(exists|metadata|read_dir|canonicalize)|thread::sleep|Command::new' \
  crates/fleet-app/src crates/fleet-ui-kit/src

# task hygiene
rg -n '\.detach\(\)' crates/fleet-app/src | wc -l      # want this falling
rg -n 'detach_and_log_err' crates/                      # want this rising

# shaping cache bypass
rg -n 'WindowTextSystem::new' crates/

# per-frame ElementId allocation
rg -n 'ElementId::from\(format!|\.id\(.*format!' crates/

# banned timer
rg -n 'smol::Timer' crates/
```
