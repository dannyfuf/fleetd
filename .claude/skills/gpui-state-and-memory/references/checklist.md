# Review checklist — GPUI state & memory (fleetd)

Standalone. Each item is a yes/no question about the diff under review, with why it matters and
the fix. Zed citations are tag `v1.18.1`; fleetd paths are relative to the repo root.

Answer "n/a" freely — most diffs touch two or three sections.

---

## Ownership

**1. Is every new long-lived struct that is observed, emits events, owns async work, or needs
focus an `Entity<T>` rather than an `Rc<RefCell<T>>` field?**
Why: an `Rc<RefCell<T>>` is an entity that lost its notify-driven invalidation, `observe` /
`observe_release` hooks, and leak tracking.
Fix: `cx.new(|cx| T::new(..))`. If it stays an `Rc<RefCell>`, add the comment saying why, the way
Zed does at `crates/workspace/src/workspace.rs:1435-1441`.

**2. Do fields pointing *down* the ownership tree use `Entity<T>`, and parents / caches /
registries use `WeakEntity<T>` or `EntityId` keys?**
Why: mutually strong handles never drop (`crates/gpui/src/app/entity_map.rs:889-897`).
Fix: `.downgrade()` the upward field; index with `EntityId`, which is what `Entity` hashes by.

**3. Does any closure stored in the app registry (an observer, subscriber, release listener, or
detached task) capture a strong `Entity` that the emitter transitively owns?**
Why: the entity's own observer list then pins it alive for the process lifetime.
Fix: downgrade the captured handle, or hold the returned `Subscription` in a field whose drop is
guaranteed. fleetd's live example: `crates/fleet-app/src/screens/hub.rs:342-348` clones a `HubCtx`
holding `state: Entity<AppState>` (`:389`) into `cx.observe(state, …)`.

**4. Are `HashMap` keys `EntityId` rather than cloned `Entity<T>` handles when the map is an
index?**
Why: an `Entity` key is a strong reference; the map then keeps entries alive it is only observing.
Fix: key by `entity_id()`, store `WeakEntity` values —
`crates/fleet-app/src/dialogs/host.rs:61` is the fleetd precedent.

**5. Does the diff avoid promoting a screen to an `Entity` as a standalone refactor?**
Why: fleetd's one-`Entity<AppState>` model with free `render(props, .., cx) -> AnyElement`
functions is deliberate and documented in `docs/APP-CONTRACTS.md`; a partial migration leaves two
architectures.
Fix: promote only a *new* sub-view that trips two or more of the five granularity criteria (outside
handle, events, distinct async lifetime, cacheable subtree, own `FocusHandle`).

---

## Subscriptions

**6. Is the wiring done in the constructor and stored in `_subscriptions: Vec<Subscription>`, or is
the `.detach()` justified in a comment?**
Why: `Drop for Subscription` is the only deregistration (`crates/gpui/src/subscription.rs:188-193`);
a detached subscription cannot be cancelled early.
Fix: collect into one field, as `crates/fleet-app/src/shell/chrome.rs:230-233` does.

**7. Do subscriptions that must be replaced when their model swaps live in their own field?**
Why: reassigning one grouped field is the atomic way to re-wire (`_terminal_subscriptions`,
`crates/terminal_view/src/terminal_view.rs:158`).
Fix: a second `_x_subscriptions` field, reassigned wholesale.

**8. Is `observe` used for "redraw me" and `subscribe` for "something happened", with neither doing
the other's job?**
Why: `observe` firing business logic makes every unrelated `cx.notify()` run it
(`crates/terminal_view/src/terminal_view.rs:1119-1125`).
Fix: split the callback; emit a typed event for the logic half.

**9. Does anything registered against another entity's lifetime have a matching `observe_release` /
`on_release` cleanup?**
Why: a global registry entry outlives its subject otherwise.
Fix: copy `crates/fleet-app/src/dialogs/host.rs:90-111` — weak in the registry, strong retained by
the release listener.

---

## Tasks & async

**10. Is every `Task` stored in a field, awaited, or detached — and does every `Task<Result<_>>`
use `.detach_and_log_err(cx)` rather than bare `.detach()`?**
Why: a dropped `Task` is silently cancelled, and a detached fallible task swallows the error, so a
failed daemon round-trip becomes a screen that just never updates.
Fix: `TaskExt::detach_and_log_err` (`crates/gpui/src/executor.rs:37-55`). fleetd has 33 `.detach()`
and zero `detach_and_log_err`; `crates/fleet-app/src/screens/board/lifecycle.rs:39, 66, 175, 231,
282` are the clearest.

**11. Is replacing the task field the cancellation mechanism, with no unbounded `Vec<Task>`
growth?**
Why: assigning over a `Task` field drops and cancels the old one
(`crates/gpui/examples/view_example/example_editor.rs:87-90`); pushing instead accumulates.
Fix: `Option<Task<_>>` or a keyed `HashMap<K, Task<_>>` —
`crates/fleet-app/src/views/watch_pane/controller.rs:107-108` is the fleetd shape.

**12. Is all filesystem, subprocess, parse and CPU work inside
`cx.background_executor().spawn(...)` / `cx.background_spawn(...)`, with the foreground task only
awaiting and then updating?**
Why: `docs/APP-CONTRACTS.md:291` and `docs/ARCHITECTURE.md:386` make this a contract, and fleetd
currently honours it at all four blocking call sites.
Fix: capture plain data (entities are not `Send`), await, then `this.update(cx, …)`. Model:
`crates/fleet-app/src/shell/root/observations.rs:37-51`.

**13. Are all `WeakEntity::update` / `update_in` / `read_with` results handled with `.ok()` or
`.log_err()` — no `let _ =`, no `unwrap()`?**
Why: `let _ =` hides both "entity dropped" (expected) and real errors; Zed's `.rules` bans it, and
fleetd is otherwise at zero production `unwrap()`.
Fix: `.ok()` for the expected drop, `.log_err()` otherwise. Existing offenders: 14 sites, listed in
`patterns.md#p4-spawn-weak`.

**14. Do loops driven by `this.update(...)` break on `Err` instead of spinning?**
Why: after the owner drops, every iteration does nothing but keep the timer alive.
Fix: `if result.is_err() { break; }`
(`crates/gpui/examples/view_example/example_editor.rs:92-105`).

**15. Are timers `cx.background_executor().timer(..)` rather than `smol::Timer::after`?**
Why: non-GPUI timers make `#[gpui::test]` non-deterministic; Zed's `clippy.toml` bans them.
Fix: the executor timer, awaited inside a retained task.

**16. Does new async work use `cx.spawn`'s `WeakEntity` instead of introducing a new generation
counter?**
Why: `Context::spawn` already gives the cancel-on-drop semantics
(`crates/gpui/src/app/context.rs:237-245`); a counter re-implements it and every reader must
remember the check.
Fix: `cx.spawn(async move |this, cx| …)` + `.ok()`. Existing guards
(`crates/fleet-app/src/screens/hub.rs:127-131`, `board_generation`) stay until their file is
touched for another reason.

**17. If the continuation touches focus, dispatch or drawing, does it use `cx.spawn_in(window, …)`
+ `update_in` instead of a stored window handle?**
Why: `spawn_in` gives an `AsyncWindowContext` whose fallibility is checked
(`crates/gpui/src/app/context.rs:676`); `Shell::window` (`crates/fleet-app/src/shell/root.rs:47`)
has to be re-entered by hand.
Fix: `cx.spawn_in(window, async move |this, cx| { this.update_in(cx, |this, window, cx| …).ok(); })`.

---

## Notify & render

**18. Does every `cx.notify()` follow a real state change, and is `render` free of it?**
Why: with one `Entity<AppState>` every notify repaints the whole tree
(`crates/gpui/src/view.rs:386-392`); a notify in `render` is an infinite frame loop.
Fix: compare before notifying, as `crates/fleet-app/src/shell/root/observations.rs:55-59` does.

**19. Does continuous animation go through `with_animation` / `window.request_animation_frame()`
and honour `App::reduce_motion`?**
Why: documented at `crates/gpui/src/window.rs:2370-2379`; a notify loop is the alternative and it
never idles.
Fix: `with_animation`, which checks `reduce_motion` for you.

**20. Does the change keep the shell's event coalescing intact (batch per wake, terminal-only
damage separated from state damage)?**
Why: `crates/fleet-app/src/shell/root/events.rs:111-152` plus
`crates/fleet-app/src/presentation/damage.rs` are what make 240 notify sites affordable.
Fix: route new bridge events through `apply_batch` and classify their damage; do not notify
per-event.

**21. Does any new call update an entity that is already on the update stack — and if it must,
does it defer *with a comment saying why*?**
Why: `cannot update <T> while it is already being updated`
(`crates/gpui/src/app/entity_map.rs:206-210`).
Fix, in order: hoist the read out of the update; keep the shared scalar in an `Rc<Cell<_>>` outside
both entities; then `cx.defer` / `cx.defer_in(window, …)` / `window.defer(cx, …)` with the comment
(`crates/inspector_ui/src/inspector.rs:19`).

---

## Events

**22. Does a promoted sub-entity signal upward with `impl EventEmitter<Event>` + `cx.emit` rather
than mutating `AppState` directly?**
Why: reaching into `AppState` from a child couples it to the whole model and forces a global
repaint.
Fix: a small `Event` enum plus `cx.subscribe` in the parent's constructor. Precedents:
`crates/fleet-ui-kit/src/components/text_field/input.rs:202`,
`crates/fleet-app/src/screens/agent_thread/mod.rs`.

---

## Globals

**23. Is a new global a private newtype with typed accessors, rather than `impl Global` on the
public type?**
Why: `Global`'s own doc says so (`crates/gpui/src/global.rs:12-21`); otherwise any caller can
`set_global` a half-built value.
Fix: `struct GlobalX(Entity<X>)` / `struct GlobalX(Arc<X>)` + `X::global(cx)`
(`crates/repl/src/repl_store.rs:23-51`, `crates/theme/src/theme.rs:321-347`). Known fleetd
exception: `crates/fleet-ui-kit/src/theme/theme.rs:70` — wrap when that file is next edited, not as
a standalone change.

**24. Do mutations go through `set_global` / `update_global` (so `observe_global` fires), and do
readers use `try_global` where absence is legal?**
Why: read-only `global` does not notify; a direct field write through an `Entity` inside the global
does not either.
Fix: mutate through the accessor; observe with `cx.observe_global::<G>()`
(`crates/fleet-app/src/shell/chrome.rs:232`).

---

## Windows & focus

**25. Does a view that takes keyboard input own a `FocusHandle`, implement `Focusable`, and does
its root element call `.track_focus(focus)`?**
Why: `docs/APP-CONTRACTS.md:108` — GPUI dispatches actions from the focused node upwards, so an
`on_action` listener below the focus node never fires.
Fix: `cx.focus_handle()` in the constructor; `.track_focus(&self.focus)` on the returned root.

---

## Tests & docs

**26. Does a new entity graph get a `weak.assert_released()` teardown test?**
Why: `crates/fleet-app/Cargo.toml:34` already enables `gpui/test-support`, which turns on
`leak-detection` (`crates/gpui/Cargo.toml:21-23, 33`), and the workspace currently has zero such
assertions.
Fix: `let weak = entity.downgrade(); drop(entity); cx.run_until_parked(); weak.assert_released();`
Run once with `LEAK_BACKTRACE=1` when it fails.

**27. Is the test `#[gpui::test]` with `run_until_parked()` / `advance_clock()` and no real
sleeps?**
Why: real sleeps make the suite flaky and hide "Parking forbidden" (a task that should have been
awaited or detached with logging).
Fix: drive the executor explicitly.

**28. Did the matching `docs/` section change in the same commit?**
Why: `docs/README.md` assigns each document a domain and the docs are authoritative, not
descriptive; `docs/APP-CONTRACTS.md` governs how fleet-app's parts plug together.
Fix: update the doc alongside the code, commit as `<area>: <imperative lowercase summary>`
(`app:`, `ui-kit:`, …).

**29. Does `make lint` (fmt-check + clippy `-D warnings`) and `make test` pass?**
Why: `make test` builds `fleetd` first because app tests launch the real daemon binary.
Fix: run both before requesting review; `make check` for the fast pass.
