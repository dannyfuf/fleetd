---
name: gpui-state-and-memory
description: Teaches GPUI ownership and lifetime discipline in fleetd — when a struct becomes an Entity<T>, Entity vs WeakEntity direction, Subscription and Task retention fields, cx.spawn/background_spawn, notify discipline, the model-emits/view-subscribes pairing, Global newtypes, defer, and leak assertions. Load it before adding or changing an Entity, cx.observe/cx.subscribe wiring, a Task or Subscription field, a cx.spawn/detach call, a cx.notify, an impl Global, or an Rc<RefCell<…>> field anywhere in fleet-app, fleet-ui-kit or fleet-lazygit. Also load it when reviewing a leak, a "cannot update X while it is already being updated" panic, a notify storm, or a detached task that outlives its screen.
---

# GPUI state and memory (fleetd)

How ownership, lifetimes and invalidation work in GPUI, and how to apply them inside fleetd's
one-`Entity<AppState>` architecture without rewriting it. Patterns are verified against Zed
v1.18.1, the GPUI tag fleetd depends on (`Cargo.toml`, ADR `docs/decisions/0001-gpui-and-toolchain.md`);
citations are `crates/<crate>/src/<file>.rs:<line>` in that checkout.

The governing fleetd docs for this area are `docs/APP-CONTRACTS.md` ("Render prepares nothing",
`:101`; "Read and write `AppState` through the entity", `:112`; "Never block the foreground thread
on the daemon", `:291`) and `docs/ARCHITECTURE.md:384-386`. Code and doc change in the same commit.

## When to use

- Adding a struct and deciding between a plain field, a `RenderOnce` component, and an `Entity<T>`.
- Adding or removing a `cx.observe` / `cx.subscribe` / `cx.observe_release` and deciding where the
  returned `Subscription` lives.
- Writing a `cx.spawn` / `cx.background_executor().spawn` and deciding field vs `.detach()` vs
  `.detach_and_log_err(cx)`.
- Touching `cx.notify()`, a generation counter, or anything that repaints the shell.
- Adding an `impl Global`, or an `Rc<RefCell<T>>` / `Arc<Mutex<T>>` field in `fleet-app`.
- Debugging a double-lease panic, a leaked entity, a stale async continuation, or a notify storm.

## When not to

- Pure presentation work (tokens, layout, elevation) — that is `gpui-styling`.
- Building a stateless kit component — that is `gpui-components`; the kit's rule is components own
  no state (`crates/fleet-ui-kit/src/lib.rs:13-25`).
- Daemon-side concurrency: `fleet-daemon` is tokio, not GPUI. Use `rust-async-background-work`.

## Rules

**Keep `Entity<AppState>` as the single model; promote a struct to its own entity only when it
earns it.** fleetd deliberately threads one `Entity<AppState>` (`crates/fleet-app/src/shell/root.rs:48`,
`crates/fleet-app/src/state.rs:78`) through free `render(props, .., cx) -> AnyElement` functions.
Do not propose Zed's many-entities model wholesale. Promote when at least one holds: something
outside needs a handle surviving the parent's render; it must emit events or be observed; it owns
async work with a different lifetime; it needs its own `FocusHandle`. Existing precedents:
`Chrome`, `ActiveDialog`, `DialogHost`, `WatchController`, `AgentThreadView`, `DiagnosticView`.

**Own downward with `Entity<T>`, point back upward with `WeakEntity<T>`.** Children are strong
fields; parents, caches and registries are weak or `EntityId` keys. Zed:
`left_dock: Entity<Dock>` beside `maximized_pane: Option<WeakEntity<Pane>>` and
`panes_by_item: HashMap<EntityId, WeakEntity<Pane>>` (`crates/workspace/src/workspace.rs:1381-1385`).
fleetd already does this in `crates/fleet-app/src/views/watch_pane/controller.rs:104` and
`crates/fleet-app/src/dialogs/host.rs:61`.

**Never let a callback stored in an entity's own registry capture a strong handle to that entity.**
That is the documented leak shape (`crates/gpui/src/app/entity_map.rs:894-897`).
`Context::observe` downgrades `self` for you (`crates/gpui/src/app/context.rs:72-81`), but a value
you *clone into* the closure is not downgraded. fleetd hazard:
`crates/fleet-app/src/screens/hub.rs:342-348` clones a `HubCtx` — which holds
`state: Entity<AppState>` (`:389`) — into `cx.observe(state, …)`.

**Store every `Subscription` in a `_subscriptions: Vec<Subscription>` field built in the
constructor; `.detach()` is a decision, not a default.** `Drop for Subscription` unregisters
(`crates/gpui/src/subscription.rs:188-193`). fleetd already does this in
`crates/fleet-app/src/shell/root.rs:71`, `crates/fleet-app/src/shell/chrome.rs:230-233`,
`crates/fleet-app/src/views/watch_pane/controller.rs:106`. Detach only when the callback must
outlive your ability to cancel it and captures nothing strong.

**Store `Task<T>` in a field when the work must die with the struct; use
`.detach_and_log_err(cx)` on `Task<Result<_>>`; never bare `.detach()` on a fallible task.**
`TaskExt::detach_and_log_err` exists in the pinned GPUI (`crates/gpui/src/executor.rs:37-55`; 428
uses in Zed) and is used **zero** times in fleetd against 33 `.detach()`. The clearest offenders
are the five in `crates/fleet-app/src/screens/board/lifecycle.rs` (`:39, :66, :175, :231, :282`),
whose async block ends in `state.update(cx, …)` and so returns a discarded `Result`.

**Push filesystem, subprocess, parse and CPU work into `cx.background_executor().spawn(...)`, and
only touch entities on the foreground.** `docs/APP-CONTRACTS.md:291` and `docs/ARCHITECTURE.md:386`
make this a contract. fleetd honours it — the three offload sites are
`crates/fleet-app/src/shell/root/observations.rs:37-39`, `.../shell/root/actions.rs:170-172`,
`.../screens/workspace/agent.rs:290-292`. Keep it that way when adding work to any of the other
50 `cx.spawn` bodies; entities are not `Send`, so capture plain data, await, then hop back.

**Handle `WeakEntity::update` results with `.ok()` or `.log_err()`, never `let _ =`.** Zed's
`.rules` bans `let _ =` on fallible operations; the ratio in Zed is ~10:1 in favour of `.ok()`.
fleetd has 14 `let _ = …update(` sites (`crates/fleet-app/src/views/watch_pane/controller.rs:124,
159, 164, 167, 182`; `.../shell/root/quit_actions.rs:150, 197`; `.../shell/root/events.rs:148-149`;
`.../terminal/surface.rs:794`). Use `.ok()` when "entity gone" is expected, `.log_err()` otherwise.

**Prefer `cx.spawn(async move |this, cx| …)`'s weak handle over a new generation counter.**
`Context::spawn` hands the closure a `WeakEntity<T>` precisely so a dropped owner cancels the
continuation (`crates/gpui/src/app/context.rs:237-245`). fleetd's `board_generation`,
`inspect_generation` and `inspection_requests` (`crates/fleet-app/src/screens/hub.rs:127-131`) are
a hand-rolled substitute. Leave existing guards alone; do not add new ones — reach for the weak
handle in new code.

**Call `cx.notify()` only after observable state actually changed, and never from `render`.**
Notifications coalesce per effect cycle (`crates/gpui/src/app.rs:1650-1656`) but each one dirties
every view subtree that read the entity (`crates/gpui/src/view.rs:386-392`). With 354 of fleet-app's
entity references being the one `Entity<AppState>`, every `state.update(cx, |_, cx| cx.notify())`
repaints the whole tree — 240 `cx.notify()` sites do this today. Guard it as
`crates/fleet-app/src/shell/root/observations.rs:55-59` does. For continuous motion use
`window.request_animation_frame()` (`crates/gpui/src/window.rs:2376-2379`) or `with_animation`,
which respects `App::reduce_motion` (`window.rs:2370-2375`).

**Model emits, view subscribes.** A model declares `impl EventEmitter<Event> for M {}` and calls
`cx.emit`; the observer wires `cx.observe(&m, |_, _, cx| cx.notify())` for "redraw me" and
`cx.subscribe(&m, …)` for "something happened", both in its constructor
(`crates/terminal_view/src/terminal_view.rs:1119-1125`). fleetd does this correctly in
`crates/fleet-ui-kit/src/components/text_field/input.rs:202` and
`crates/fleet-app/src/screens/agent_thread/mod.rs`. Do not reach into `AppState` from a promoted
sub-entity — give it a small `Event` enum instead.

**Globals are private newtypes with typed accessors, not the public type.** `Global`'s own doc
says so (`crates/gpui/src/global.rs:12-21`); Zed's shape is
`struct GlobalReplStore(Entity<ReplStore>)` + `ReplStore::global(cx)`
(`crates/repl/src/repl_store.rs:23-51`), or `GlobalTheme { theme: Arc<Theme> }`
(`crates/theme/src/theme.rs:321-347`). fleetd's `impl Global for Theme` on the public by-value
type (`crates/fleet-ui-kit/src/theme/theme.rs:70`) is the exception; `ActiveTheme` (`:230-241`)
already gives the accessor, so wrap the value the next time `theme.rs` is touched, not before.

**Defer out of the update stack, with a comment saying why.** A nested `read`/`update` of an entity
already leased panics (`crates/gpui/src/app/entity_map.rs:206-210`). Reach for `cx.defer`
(`crates/gpui/src/app.rs:1994`), `cx.defer_in(window, …)` (`crates/gpui/src/app/context.rs:305`) or
`window.defer(cx, …)` (`crates/gpui/src/window.rs:2252`) — all run inside the same `flush_effects`
loop, before the next frame. Zed comments every one (`crates/inspector_ui/src/inspector.rs:19`).
fleetd's five deferrals (e.g. `crates/fleet-app/src/views/watch_pane/controller.rs:122-125`)
should carry the same comment.

**Assert entity release in tests for any new entity graph.** `crates/fleet-app/Cargo.toml:34`
already dev-depends on `gpui` with `test-support`, which enables `gpui/leak-detection`
(`crates/gpui/Cargo.toml:21-33`), so `weak.assert_released()`
(`crates/gpui/src/app/entity_map.rs:647-660`) and `App::assert_no_new_leaks`
(`crates/gpui/src/app.rs:952, 968`) work today. fleetd uses them zero times.

**A code change that contradicts `docs/` is a bug in one of the two.** `docs/README.md` assigns
each document a domain; `docs/APP-CONTRACTS.md` owns how fleet-app's parts plug together. Update
both in the same commit (`<area>: <imperative lowercase summary>`, e.g. `app: …`).

## Core patterns

Full catalog with Zed snippets: `references/patterns.md`.

### Promote a sub-view to an entity (`references/patterns.md#p1-strong-down-weak-up`)

```rust
pub(crate) struct JobsPanelView {
    state: WeakEntity<AppState>,      // upward / ambient: weak
    focus: FocusHandle,               // own focus, own key context
    log_tail: Option<Entity<LogModel>>, // downward: strong
    _subscriptions: Vec<Subscription>,
    refresh: Option<Task<()>>,        // dropped with the view => cancelled
}

impl JobsPanelView {
    fn new(state: &Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(state, |_, _, cx| cx.notify())];
        Self {
            state: state.downgrade(),
            focus: cx.focus_handle(),
            log_tail: None,
            _subscriptions: subscriptions,
            refresh: None,
        }
    }
}
```

### Async continuation via the weak handle (`references/patterns.md#p4-spawn-weak`)

```rust
// Replaces the generation-guard shape for new code.
self.refresh = Some(cx.spawn(async move |this, cx| {
    let Ok(Ok(ResponseBody::Jobs(jobs))) = reply.recv().await else { return };
    this.update(cx, |this, cx| {
        this.apply_jobs(jobs);
        cx.notify();
    })
    .ok(); // the view was dropped; nothing to do
}));
```

### Background offload, then foreground update (`references/patterns.md#p5-background`)

```rust
// crates/fleet-app/src/shell/root/observations.rs:37-51 is the live example.
let probe_home = home.clone();
let exists = cx
    .background_executor()
    .spawn(async move { probe_home.join("state.json").exists() })
    .await;
shell.update(cx, |shell, cx| shell.set_state_file_present(exists, cx)).ok();
```

### Fallible fire-and-forget (`references/patterns.md#p3-task-ownership`)

```rust
// Today: crates/fleet-app/src/screens/board/lifecycle.rs:27-39 ends in `.detach()`,
// discarding the Result the async block returns. Prefer:
cx.spawn(async move |cx| {
    let view = board_reply(reply).await?;
    state.update(cx, |state, cx| {
        state.finish_board_load(&context_id, generation, Ok(view));
        cx.notify();
    })
})
.detach_and_log_err(cx);
```

### Guarded notify (`references/patterns.md#p6-notify`)

```rust
// Never `cx.notify()` unconditionally after a background result.
shell.update(cx, |shell, cx| {
    if shell.local_files != files {
        shell.local_files = files;
        cx.notify();
    }
})
.ok();
```

### Registry keyed by `EntityId` with release cleanup (`references/patterns.md#p9-release`)

```rust
// crates/fleet-app/src/dialogs/host.rs:90-111 — weak in the registry, strong retained by the
// release listener, so the draft dies exactly when its AppState does.
cx.default_global::<DialogRegistry>().hosts.insert(id, host.downgrade());
let retained = host.clone();
cx.observe_release(state, move |_, cx| {
    cx.default_global::<DialogRegistry>().hosts.remove(&id);
    drop(retained);
})
.detach();
```

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| `.detach()` on a `Task<Result<_>>` | The error vanishes; a failed daemon round-trip becomes a screen that silently never updates. 33 sites in fleet-app, 0 `detach_and_log_err`. | `.detach_and_log_err(cx)`, or store the `Task` in a field. |
| `let _ = entity.update(cx, …)` | Hides both "entity dropped" and real errors; banned by Zed's `.rules`. | `.ok()` when the drop is expected, `.log_err()` otherwise. |
| A strong `Entity<T>` cloned into a closure registered against `T` itself | `T`'s own observer list pins `T` alive forever (`entity_map.rs:894-897`). | Downgrade the captured handle, or hold the `Subscription` in a field whose drop is guaranteed. |
| New `Rc<RefCell<T>>` for state anyone else observes | Loses notify-driven invalidation, `observe`, `observe_release` and leak tracking. | `cx.new(|_| T::default())`; keep `Rc<RefCell>` only for element-closure sharing, with a comment saying why (Zed does: `workspace.rs:1435-1441`). |
| A fresh generation counter to invalidate stale async work | Hand-rolls what `cx.spawn`'s `WeakEntity` already gives you, and every reader must remember the check. | `cx.spawn(async move \|this, cx\| …)` + `.ok()`; break loops on `Err`. |
| `cx.notify()` inside `render` | Requests an infinite frame loop. | `window.request_animation_frame()` or `with_animation`; honour `App::reduce_motion`. |
| Unconditional `state.update(cx, \|_, cx\| cx.notify())` after a poll | Repaints the entire tree because `AppState` is the only model. | Compare first; notify only on change. |
| Reading an entity that is already leased | `cannot update X while it is already being updated` panic (`entity_map.rs:206-210`). | Hoist the read out, keep the shared scalar in an `Rc<Cell<_>>`, or `cx.defer` with a comment. |
| `impl Global` on a public, cloneable domain type | Anyone can `set_global` a half-built value; clones are not cheap. | Private newtype + typed accessor (`global.rs:12-21`). |
| Blocking IO inside `cx.spawn` | Freezes the frame; breaks `docs/APP-CONTRACTS.md:291`. | `cx.background_executor().spawn(...)`, then `update`. |

## fleetd-specific guidance

**Keep doing** (already at or above Zed's bar — do not "fix" these): `_subscriptions: Vec<Subscription>`
and `_tasks: Vec<Task<()>>` retention on `Shell` (`crates/fleet-app/src/shell/root.rs:71-72`); keyed
task maps (`crates/fleet-app/src/dialogs/host.rs:54-55`,
`crates/fleet-app/src/views/watch_pane/controller.rs:107-108`); the `EntityId`-keyed registry with
`observe_release` cleanup (`crates/fleet-app/src/dialogs/host.rs:90-111`); the background-executor
discipline; zero production `unwrap()`; the pure `AppState` (`crates/fleet-app/src/state.rs:78` —
no `Entity`, `Rc` or `Mutex` fields).

**The reference implementation to copy** is `WatchController`
(`crates/fleet-app/src/views/watch_pane/controller.rs:103-135`): `WeakEntity<AppState>`,
`_subscriptions`, two keyed `Task` maps, `observe_release` deregistering from the global, and a
`cx.defer` for the initial sync. Its one flaw is `let _ = this.update(…)` at `:124`.

**Concrete gaps, in the order worth fixing** — each is local, none needs a rewrite:

1. `crates/fleet-app/src/screens/board/lifecycle.rs:39, 66, 175, 231, 282` — `.detach()` on tasks
   that end in `state.update(cx, …)` and therefore return a discarded `Result`. Switch to
   `.detach_and_log_err(cx)` when you next touch that file.
2. `crates/fleet-app/src/screens/hub.rs:342-349` — the `cx.observe(state, …)` closure owns a
   `HubCtx` holding a strong `Entity<AppState>` (`:389`). The `Subscription` is retained in
   `HubScreen::observation` (`:321`), so this only leaks if the screen outlives the state; downgrade
   `HubCtx::state` to `WeakEntity<AppState>` when editing `HubCtx`. Same review applies to
   `crates/fleet-app/src/dialogs/context.rs` and `crates/fleet-app/src/screens/jobs/presentation.rs`.
3. The 14 `let _ = …update(` sites listed under the rules — mechanical, do them in the file you are
   already editing.
4. `crates/fleet-app/src/views/board_screen.rs:88` recomputes `grouped_cards` per frame; only the
   Hub memoises (`crates/fleet-app/src/screens/hub/projection.rs:3-38`, keyed on
   `AppState::snapshot_revision`). The Workspace is *not* a second offender — `Model::build` runs in
   `synchronize` (`screens/workspace/lifecycle.rs:307`). Copy the `ProjectionCache` shape rather than
   promoting the screen to an entity first. Detail belongs to `gpui-performance`.
5. `crates/fleet-app/src/dialogs/palette.rs` (2 046 lines) and
   `crates/fleet-app/src/dialogs/create_worktree.rs` (1 750 lines) mix draft, render and dispatch
   while sibling dialogs are split into `draft.rs`/`view.rs`/`persistence.rs`. If you split one,
   the draft half is the natural first `Entity<T>` — but a split is not required to fix a bug.
6. `spawn_in` / `subscribe_in` are used zero times; `Shell` stashes
   `window: Option<AnyWindowHandle>` (`crates/fleet-app/src/shell/root.rs:47`) instead. When a new
   continuation touches focus or dispatch, use `cx.spawn_in(window, …)`
   (`crates/gpui/src/app/context.rs:676`) + `this.update_in(cx, …)`.
7. Zero leak assertions. Add `weak.assert_released()` to the next test that closes a dialog or drops
   a promoted screen; run once with `LEAK_BACKTRACE=1`.

**Migration stance.** Every item above is incremental and file-local. Do not promote screens to
entities, introduce a `Dialog` trait, or replace generation guards as a standalone refactor —
do it when the file is already open for another reason, and say so in the commit body.

**Verification.** `make lint` (fmt-check + clippy `-D warnings`), `make test` (builds `fleetd`
first, because app tests launch the real daemon), `make check`.

## Review checklist

Full list, with the "why" and the fix for each: `references/checklist.md`.

- Is every new long-lived, observed, event-emitting or focus-owning struct an `Entity<T>` rather
  than an `Rc<RefCell<T>>` field?
- Do downward fields use `Entity<T>` and upward/cache fields use `WeakEntity<T>` or `EntityId`?
- Does any closure stored in an entity's registry capture a strong handle to that same entity?
- Is every `Subscription` in a `_subscriptions` field, or is the `.detach()` justified?
- Is every `Task` stored, awaited, or `detach_and_log_err`'d — and is bare `.detach()` off a
  `Task<Result<_>>`?
- Is all filesystem / subprocess / CPU work inside `cx.background_executor().spawn(...)`?
- Are all `WeakEntity::update` results `.ok()`/`.log_err()` — no `let _ =`, no `unwrap()`?
- Does every `cx.notify()` follow a real state change, and is `render` free of it?
- Does every new `defer` carry a comment saying which lease it is escaping?
- Does a new entity graph get a `weak.assert_released()` teardown test, and did the matching
  `docs/` section change in the same commit?

## Related skills

- `gpui-components` — `RenderOnce` vs `Render`, builders, ui-kit conventions; the other half of the
  "plain field or entity?" decision.
- `gpui-performance` — memoisation, `Entity::cached`, render-time allocation; owns gaps 4 above.
- `rust-async-background-work` — executors, channels, the `Bridge`'s tokio thread, daemon-side async.
- `gpui-app-shell` — actions, keymaps, focus handles, modals, windows.
- `rust-gpui-testing` — `#[gpui::test]`, `TestAppContext`, `run_until_parked`, leak assertions.
- `zed-quality-review` — aggregates this skill's `references/checklist.md`.
