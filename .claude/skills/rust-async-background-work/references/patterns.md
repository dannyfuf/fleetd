# Async and background-work patterns — full catalog

Zed citations are `zed/crates/<crate>/src/<file>.rs:<line>` at tag **v1.18.1**
(`/Users/danny/.swarm/repos/zed-industries/zed`, HEAD `bebe92f`). fleetd citations are relative to
the repo root and were verified against the worktree at `chore-skills`.

Read `../SKILL.md` first; this file is the long form of its `## Core patterns` and the reference
for the daemon-side patterns that did not fit there.

---

## Executor and API map

| Need | App (GPUI, smol) | Daemon / client (tokio) |
| --- | --- | --- |
| task that touches entities | `cx.spawn(async move \|this, cx\| …)` — `zed/crates/gpui/src/app/context.rs:237` | n/a |
| task from a bare `&mut App` | `cx.spawn(async move \|cx\| …)` — `zed/crates/gpui/src/app.rs:1959` | n/a |
| task that needs the `Window` | `cx.spawn_in(window, …)` — `zed/crates/gpui/src/app/context.rs:676` | n/a |
| `Send + 'static` CPU work | `cx.background_spawn(fut)` / `cx.background_executor().spawn(fut)` — `zed/crates/gpui/src/app.rs:2880` | `tokio::spawn` |
| blocking syscall / sync-only crate | background executor with owned inputs | `tokio::task::spawn_blocking` |
| delay / debounce | `cx.background_executor().timer(CONST).await` | `tokio::time::sleep` |
| test time control | `cx.executor().advance_clock(d)` — `zed/crates/gpui/src/executor.rs:201-204` | `tokio::time::pause` / `advance` |
| drain everything runnable (tests) | `cx.run_until_parked()` — `zed/crates/gpui/src/executor.rs:224-227` | n/a |

`AsyncApp::update` panics if the app is gone; `WeakEntity::{read_with, update, update_in}` all
return `anyhow::Result`, which is what makes `?` the natural exit condition inside a task.

---

## P1 bridge request

**Foreground task awaits a daemon reply, then re-enters state under a generation guard.**

This is fleetd's single most common async shape — `RequestBody::` is constructed directly
inside views across `fleet-app` (181 mentions in `fleet-app/src`), and every one of them ends here.

```rust
// crates/fleet-app/src/screens/board/lifecycle.rs:17-40
pub(super) fn ensure_current(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    ensure_backends(state, bridge, cx);
    let Some((context_id, generation)) = state.update(cx, |state, _| state.begin_board_load())
    else {
        return;
    };
    let reply = bridge.request(RequestBody::EnsureBoard { context_id: context_id.clone() });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let result = match reply.recv().await {
            Ok(Ok(ResponseBody::Board(view))) => Ok(view),
            Ok(Ok(_)) => Err("EnsureBoard returned an unexpected response".to_owned()),
            Ok(Err(error)) => Err(error.message),
            Err(error) => Err(format!("Board request channel closed: {error}")),
        };
        state.update(cx, |state, cx| {
            state.finish_board_load(&context_id, generation, result);
            cx.notify();
        });
    })
    .detach();
}
```

Points to copy:

- The generation is minted by `begin_board_load` **before** the request goes out and checked inside
  `finish_board_load`, so a superseded reply is dropped rather than applied.
- All four failure shapes of `Receiver<Result<ResponseBody, ProtoError>>` are handled
  (`Ok(Ok(right))`, `Ok(Ok(wrong variant))`, `Ok(Err(proto))`, `Err(closed)`). A `Bridge` reply that
  arrives after the daemon dropped the request appears as `Err(closed)`.
- Zed's equivalent is `cx.spawn(async move |this, cx| { let x = task.await?; this.update(cx, …)? })`
  — `zed/crates/project/src/project.rs:3169`.

**Same shape, releasing a one-shot flag on failure** (`lifecycle.rs:48-67`): `begin_backends_load`
(`state/board.rs:131`) sets an "asked already" flag; the error path must clear it or the connection never asks again. When
a request is guarded by a latch, every exit path has to release the latch.

**Bridge API** (`crates/fleet-app/src/bridge.rs`):

- `Bridge::start(home)` — `:313-337`, spawns the OS thread named `fleet-daemon-bridge`;
  `run_thread` at `:473-494` builds `tokio::runtime::Builder::new_multi_thread().enable_all()` and
  `block_on`s the connection loop.
- `bridge.send(body)` — `:363-379`, fire-and-forget. A full queue sets a resync flag and reports a
  user-visible mutation failure; a closed queue reports the failure.
- `bridge.request(body) -> Receiver<Result<ResponseBody, ProtoError>>` — `:409-425`. The reply
  channel is `async_channel::bounded(1)`. A full or closed command queue is converted into an
  `offline(..)` error pushed onto that reply channel, so the awaiting task always completes.
- Capacities and their rationale: `:51-54` (`COMMAND_CAPACITY = 512`, `EVENT_CAPACITY = 1_024`).
- `bridge.events() -> Receiver<BridgeEvent>` — `:355-358`, cloneable; see P4.

**Known property:** dropping the GPUI task does **not** cancel the daemon request (P8).

---

## P2 background hop

**Foreground task hands owned data to the background executor, awaits it, re-enters the entity.**

```rust
// crates/fleet-app/src/shell/root/actions.rs:169-180 (trimmed)
let home = self.state.read(cx).home.clone();
let task = cx
    .background_executor()
    .spawn(async move { home.join("state.json").exists() });
cx.spawn(async move |shell, cx| {
    let exists = task.await;
    let _ = shell.update(cx, |shell, cx| { /* … */ });
})
```

Other fleetd instances: `shell/root/observations.rs:37-52` (`std::fs::metadata` mtime polling),
`screens/workspace/agent.rs:290-292` (`fs::read_dir` walk, capped at 2 000 entries),
`screens/workspace/lifecycle.rs:263`. `fleet-lazygit` uses the shorter `cx.background_spawn(..)`
form at `diff_view.rs:418,461`, `root/diff_view.rs:130,160`, `root/conflicts.rs:52`,
`drive/support/mod.rs:68,91,223` — 8 sites, and the only `background_spawn` in the workspace.

Rules:

- Only owned, `Send + 'static` data crosses into the background future. No `Entity`, no `Window`,
  no `App`. There is no way to reach an entity from a background task at all.
- Anything above ~1 ms of parsing, diffing or sorting belongs here. `fleet-app` currently does all
  its non-daemon compute on the foreground thread (0 `background_spawn`); the memoisation work in
  `screens/hub/projection.rs` and the board grouping are the candidates when they get slow.
- Zed's longer form with a background hop inside a foreground task:
  `zed/crates/editor/src/git/blame.rs:673`.

---

## P3 task fields and cancellation

**A `Task` in a field is fleetd's cancellation primitive.** `Task` is `#[must_use]` and dropping it
cancels the work, so assignment over the field cancels the previous run.

fleetd's retention fields, all verified:

| Field | Location | Shape |
| --- | --- | --- |
| `_tasks: Vec<Task<()>>` | `crates/fleet-app/src/shell/root.rs:72` | keep-alive only, `_`-prefixed |
| `_subscriptions: Vec<Subscription>` | `crates/fleet-app/src/shell/root.rs:71` | keep-alive only |
| `inspect_task`, `pr_refresh_task: Option<Task<()>>` | `crates/fleet-app/src/screens/hub.rs:129-130` | reassigned to cancel (assignment at `hub/cache.rs:565`) |
| `tail: Option<Task<()>>` | `crates/fleet-app/src/screens/jobs.rs:60` | "Keeps the follow loop alive; dropping it stops the poll." |
| `pr_tasks: HashMap<RepoId, gpui::Task<()>>` | `crates/fleet-app/src/screens/workspace.rs:212` | keyed, one in-flight per repo |
| `tasks: HashMap<&'static str, Task<()>>`, `completions: HashMap<u64, Task<()>>` | `crates/fleet-app/src/dialogs/host.rs:54-55` | keyed by dialog / completion id |

Conventions:

- `_`-prefix a field that exists **only** to keep a task alive; use a plain name when the code also
  reassigns or inspects it (Zed convention: `_task`, `_subscriptions`, `_maintain_remote_snapshot`).
- A `HashMap<K, Task<..>>` gives per-key cancel-by-replacement, which is what
  `screens/workspace.rs:212` and `dialogs/host.rs:54` use.
- Detach only when the work must genuinely outlive the owner. In fleetd a detached task holds a
  **strong** `Entity<AppState>`, so it cannot outlive the app but can outlive the screen that
  started it — which is exactly why the generation guards exist (P9).

Zed reference: `zed/crates/editor/src/linked_editing_ranges.rs:57` stores a `spawn_in` task in a
field; `zed/crates/git_ui/src/git_panel.rs:4731` reassigns `update_visible_entries_task`.

---

## P4 reader to channel to drain loop

**The shape for every long-running stream: PTY output, bridge events, log tails.**

1. Reader on a background task or a dedicated OS thread.
2. Reader pushes into a channel.
3. **One** foreground task drains it and applies to entities.
4. Loop exits when the channel closes or the entity update fails.
5. Batching + a yield so the producer cannot starve rendering.

```rust
// crates/fleet-app/src/shell/root/events.rs:111-160 (trimmed)
pub(super) fn spawn_event_loop(bridge: &Bridge, cx: &mut Context<Self>) -> Task<()> {
    let events = bridge.events();
    cx.spawn(async move |shell, cx| {
        while let Ok(first) = events.recv().await {
            let mut batch = Vec::with_capacity(EVENT_BATCH_LIMIT);
            batch.push(first);
            while batch.len() < EVENT_BATCH_LIMIT {
                let Ok(event) = events.try_recv() else { break };
                batch.push(event);
            }
            let updated = shell.update(cx, |shell, cx| { /* apply_batch, damage, recovery */ });
            let Ok(window) = updated else { return };
            // …enter the window only when a visible terminal changed…
            // A busy producer cannot keep the UI executor inside an always-ready receive loop.
            cx.background_executor().timer(Duration::from_millis(1)).await;
        }
    })
}
```

`EVENT_BATCH_LIMIT = 128` and `TICK = 250ms` are named consts at `events.rs:15-16`. The returned
`Task` is stored in `Shell::_tasks` (`shell/root.rs:72`).

Two refinements worth understanding before touching this loop:

- **Damage classification.** `apply_batch` returns a `Damage` that distinguishes state changes
  (which call `cx.notify()` and wake chrome observers) from terminal-only output (which only
  synchronizes surfaces). See `crates/fleet-app/src/presentation/damage.rs`. Adding a `cx.notify()`
  to the terminal-output path would wake every chrome observer on every keystroke of output.
- **The shell lease.** The loop releases the shell before entering the window
  (`events.rs:145-153`), because entering a window while the shell entity is being updated would be
  a nested update.

Zed's equivalent, including the batch-100-or-4ms drain and `yield_now()`:
`zed/crates/terminal/src/terminal.rs:1354`. Zed's fully-background variant (scanner on the
background executor, `mpsc::unbounded` back, a separate foreground `cx.spawn` applying it) is
`zed/crates/worktree/src/worktree.rs:1354,1400`.

**Other fleetd loops of this family:** the jobs log tail (`screens/jobs/log_follow.rs:88-101`,
`TAIL_INTERVAL` timer at the bottom of each iteration), the ticker (`events.rs:163-176`), the
filesystem observation loop (`shell/root/observations.rs:37-95`).

---

## P5 debounce and retry

**Timer first, work second, task stored in a field.**

```rust
// crates/fleet-app/src/screens/hub/cache.rs:537-565 (trimmed)
let state = self.state.downgrade();
let hub = self.hub.downgrade();
let task = cx.spawn(async move |cx| {
    cx.background_executor().timer(AUTO_INSPECT_DEBOUNCE).await;
    cx.update(|cx| {
        let (Some(state), Some(hub)) = (state.upgrade(), hub.upgrade()) else { return };
        if hub.read(cx).inspect_generation != generation {
            return;
        }
        // …do the work…
    });
});
self.hub.update(cx, |hub, _| hub.inspect_task = Some(task));
```

Note this site uses **both** mechanisms: `downgrade()` for liveness and `inspect_generation` for
staleness. That is the right combination when the debounce can also be superseded by a cursor move
that does not replace the field.

Named intervals in fleetd, all `const … : Duration`:

| Constant | Value | Location |
| --- | --- | --- |
| `AUTO_INSPECT_DEBOUNCE` | 400 ms | `crates/fleet-app/src/screens/hub.rs:75` |
| `DEBOUNCE` (owner search) | 150 ms | `crates/fleet-app/src/dialogs/clone_repo.rs:19` |
| `ATTACH_RETRY_DELAY` | 250 ms | `crates/fleet-app/src/terminal/surface.rs:34` |
| `TICK` | 250 ms | `crates/fleet-app/src/shell/root/events.rs:15` |
| `HEALTH_INTERVAL` / `HEALTH_TIMEOUT` / `IDENTITY_INTERVAL` | 2 s / 3 s / 60 s | `crates/fleet-app/src/bridge.rs:45-49` |
| `DEBOUNCE` / `MAX_DEBOUNCE` (notify) | 150 ms / 1 s | `crates/fleet-git/src/watch.rs:16-17` |

Every one of these is the value a test advances (`advance_clock(2 * CONST)`), which is the reason
they must be named consts and not literals.

Zed reference: `zed/crates/editor/src/git/blame.rs:772`, `zed/crates/git_ui/src/git_panel.rs:4731`.
Zed's coalescing variant — a persistent task that drains a `watch` channel and only acts once the
stream goes quiet — is `zed/crates/git_ui/src/text_diff_view.rs:234`.

---

## P6 timeouts

fleetd races a reply against a timer with a hand-rolled `poll_fn`:

```rust
// crates/fleet-app/src/terminal/surface.rs:629-644, duplicated at
// crates/fleet-app/src/dialogs/create_worktree.rs:512-527
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
```

Call sites: `surface.rs:582` (`ATTACH_TIMEOUT`), `create_worktree.rs:467,488` (`BASE_REF_TIMEOUT`).
It is correct and deterministic under the GPUI test scheduler because the timeout future is always
`cx.background_executor().timer(..)`. The problem is only the duplication: hoist one helper the
next time either file is touched.

Zed's idiom is `futures::select_biased!` / `select!` over `executor.timer(..)`, plus
`gpui_util::defer` for cancel-on-drop side effects
(`zed/crates/project/src/debounced_delay.rs:38-43`; `lsp.rs:1516-1542` uses plain `select!`);
the fleetd spelling is `futures_util::select_biased!`.
Adopting it in `fleet-app` means adding `futures-util`
(currently only in `fleet-cli`, `fleet-client`, `fleet-daemon`) — a dependency decision.

---

## P7 tests

**Deterministic async tests: `advance_clock` makes timers ready, `run_until_parked` runs them.**

```rust
// crates/fleet-app/src/views/watch_pane/tests.rs:126-136 (trimmed)
let _controller = cx.new(|cx| WatchController::new(&state, harness.requests(), cx));
cx.run_until_parked();
assert_eq!(harness.len(), 1);
assert!(matches!(harness.respond(Err(offline("list unavailable"))), RequestBody::ListWatches { .. }));
cx.run_until_parked();
assert_eq!(harness.len(), 0, "failure must not retry without a delay");
cx.executor().advance_clock(RECOVERY_RETRY_DELAY);
```

```rust
// crates/fleet-app/src/terminal/surface/tests.rs:78
cx.executor().advance_clock(ATTACH_RETRY_DELAY);
```

fleetd has 105 `#[gpui::test]`, 106 `run_until_parked` and 22 `advance_clock` uses.

Missing idioms worth adding:

- **Awaiting a task stored in a field.** Take it out and await it instead of sleeping:

  ```rust
  let handle = cx.update_entity(&hub, |hub, _| {
      std::mem::replace(&mut hub.inspect_task, Some(Task::ready(()))).unwrap()
  });
  cx.executor().advance_clock(2 * AUTO_INSPECT_DEBOUNCE);
  handle.await;
  ```

  Zed: `zed/crates/git_ui/src/git_panel.rs:9361-9367`.
- **Randomised interleavings.** `#[gpui::test(iterations = N)]` re-runs with different seeds so
  `simulate_random_delay()` (`zed/crates/gpui/src/executor.rs:196`) shuffles task ordering. fleetd
  has zero such tests; the bridge event loop and the board generation guards are where ordering
  bugs would live.

Semantics to remember: `advance_clock` is documented as *"In tests, move time forward. This does
not run any tasks, but does make `timer`s ready."* (`zed/crates/gpui/src/executor.rs:200-204`);
`run_until_parked` advances the clock to the next timer when nothing is runnable
(`:213-227`). Never `std::thread::sleep` and never assert on `Instant::now()` deltas.

---

## P8 the tokio bridge

**fleetd's bridge vs Zed's `gpui_tokio` — the same problem, two answers.**

Zed keeps a 2-worker tokio runtime as a GPUI global and hands back a GPUI `Task` that aborts the
tokio join handle when dropped:

```rust
// zed/crates/gpui_tokio/src/gpui_tokio.rs:55-73
pub fn spawn<C, Fut, R>(cx: &C, f: Fut) -> Task<Result<R, JoinError>> {
    cx.read_global(|tokio: &GlobalTokio, cx| {
        let join_handle = tokio.handle.spawn(f);
        let abort_handle = join_handle.abort_handle();
        let cancel = defer(move || { abort_handle.abort(); });
        cx.background_spawn(async move {
            let result = join_handle.await;
            drop(cancel);
            result
        })
    })
}
```

The runtime is built at `gpui_tokio.rs:13-18` with `worker_threads(2)` and the comment *"Since we
now have two executors, let's try to keep our footprint small."*

fleetd instead owns a whole tokio runtime on a dedicated OS thread and communicates only by
channel:

```rust
// crates/fleet-app/src/bridge.rs:319-337 (trimmed)
thread::Builder::new()
    .name("fleet-daemon-bridge".to_owned())
    .spawn(move || run_thread(home, command_rx, thread_events, thread_resync))

// crates/fleet-app/src/bridge.rs:473-494 (trimmed)
let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
runtime.block_on(runtime::run(&home, &commands, &events, &resync_pending));
```

Consequences, in order of how often they bite:

1. **No drop-cancellation.** `bridge.request` returns an `async_channel::Receiver`, not a `Task`.
   Dropping the GPUI task that awaits it abandons the receiver; the daemon request keeps running.
   For fleetd this is largely *by design* — `docs/ARCHITECTURE.md:12`: "Nothing the user started is
   ever tied to a UI surface." Guard against stale replies with a generation, never with a
   cancellation assumption.
2. **No `worker_threads` tuning.** Neither the app bridge nor `#[tokio::main]` in
   `crates/fleet-daemon/src/main.rs:39` sets a worker count. `fleet-lazygit/src/bridge.rs:478` uses
   `new_current_thread().max_blocking_threads(2)`; `fleet-cli` uses `new_current_thread`.
3. **The seam is `async_channel` in both directions**, with the bounds at `bridge.rs:51-54`. That
   is the only place the two runtimes touch. Never call `block_on` from a view, never start another
   runtime.

A `Tokio::spawn`-shaped wrapper for fleetd would change the lifetime contract of every daemon
request. It is an ADR-level decision (`docs/decisions/`), not a refactor.

---

## P9 generation guards vs weak handles

fleetd uses generation counters where Zed uses `WeakEntity`, because a detached fleetd task holds a
**strong** `Entity<AppState>`.

| Guard | Location |
| --- | --- |
| `board_generation` | `crates/fleet-app/src/state.rs:112`; `begin_board_load` / `finish_board_load` at `state/board.rs:191,223` |
| `inspect_generation` | `crates/fleet-app/src/screens/hub.rs:128`, checked at `hub/cache.rs:550` |
| `inspection_requests` / `inspection_sequence` | `crates/fleet-app/src/screens/hub.rs:132-133`, checked in `apply_inspection` (`:170-173`) |
| `clone.seq` / `clone.search_seq` | `crates/fleet-app/src/dialogs/clone_repo.rs:279-289` |
| `AtomicBool` cancel + `self.key` | `crates/fleet-lazygit/src/diff_view.rs:400,440` |

Counts: `WeakEntity` appears 11 times in the workspace, `.downgrade()` 28 times, against Zed's
hundreds. This is a coherent choice, not an accident — do not rewrite it.

**When to use which:**

- The work should *stop* when the owner goes away → store the `Task` in a field on the owner. Drop
  cancels it; no guard needed.
- The work must *not be applied* when a newer request superseded it → generation counter. This is
  most bridge replies, because the request keeps running in the daemon regardless (P8).
- A new sub-view / dialog / list that is its own `Entity` → `cx.spawn(async move |this, cx| …)`
  with the `WeakEntity` GPUI hands you, `?` on each `this.update(cx, …)`, `anyhow::Ok(())` last.
  Add a generation only if staleness is still possible.

---

## P10 blocking work and threads

**App side.** Four blocking-IO call sites in `fleet-app/src`, each fenced (audit §7.3):

| Site | Fence |
| --- | --- |
| `shell/root/observations.rs:94` — `fs::metadata` | inside `cx.background_executor().spawn` at `:37-39` |
| `screens/workspace/agent.rs:710` — `fs::read_dir` walk (capped at 2 000 entries) | inside `cx.background_executor().spawn` at `:290-292` |
| `bridge/connection.rs:232` — `fs::File::open` (log tail) | runs on the bridge's tokio thread |
| `notify_sound.rs:40` — `Command::new("/usr/bin/afplay")` | dedicated `SyncSender`-fed worker thread, `LazyLock`-initialised |

**Daemon side.** 21 `spawn_blocking` sites, all file or process IO: `stores/config.rs:48,75,89`,
`stores/state.rs:47,60,79,96`, `jobs/manager/logs.rs:10`, `services/watch_discovery.rs:221,393`,
`services/snapshots.rs:176,214`, `services/doctor.rs:358,395`, `services/worktrees.rs:539`,
`services/pool.rs:653,850`, `services/agents/manager.rs:340`,
`services/sessions/{host_bridge.rs:218,lifecycle.rs:92}`.

**Dedicated OS threads**, all named, all at platform boundaries:

| Thread name | Location |
| --- | --- |
| `fleet-daemon-bridge` | `crates/fleet-app/src/bridge.rs:321` |
| `fleet-terminal-{id}` | `crates/fleet-term/src/host.rs:204` |
| `fleet-pty-writer` / `fleet-pty-reader` / `fleet-pty-wait` | `crates/fleet-term/src/pty.rs:295,452,460` |
| `fleet-terminal-events-{id}` | `crates/fleet-daemon/src/services/sessions/host_bridge.rs:284` |
| `fleet-remove` (blocking rmtree) | `crates/fleet-daemon/src/adapters/files.rs:372` |
| `fleetd-reaper` | `crates/fleet-client/src/spawn.rs:108` |

`fleet-term` uses **zero tokio** — it is a pure blocking-thread crate, with queue bounds
`COMMAND_QUEUE_BYTES = 4 MiB`, `HOST_EVENT_MAX_BYTES = 16 MiB`,
`HOST_EVENT_QUEUE_BYTES = 2 * HOST_EVENT_MAX_BYTES` (`crates/fleet-term/src/host.rs:32-35`). That
is five OS threads per live terminal; thread count scales linearly with terminals.

**Trait-fenced blocking** is the daemon's rule: `#[async_trait]` adapters (`Files`, `Shell`, `Git`,
`Github`, `Process`, `Clock`, `BoardBackend`, `MachineProvider`, `AgentProvider`) in
`crates/fleet-daemon/src/adapters/`, mirroring Zed's `Fs` trait with `RealFs`/`FakeFs`
(`zed/crates/fs/src/fs.rs:98,417,1398`). Adapter tests assert exact argv; service tests assert
domain results (`docs/ARCHITECTURE.md:392`).

**Subprocesses.** `crates/fleet-git/src/command.rs` is the hardened runner — `tokio::process`,
shell-free, `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`, `kill_on_drop(true)`, `GIT_OPTIONAL_LOCKS=0` on
background reads (`:234-255`), `DEFAULT_TIMEOUT = 60s`, `DEFAULT_OUTPUT_LIMIT = 64 MiB` (`:27-29`),
a `supervise_process` `tokio::join!` over stdin-write + both pipes + `child.wait()` so a chatty
child cannot deadlock (`:505-580`), and a `CancellationGuard` so the command log always records a
terminal outcome (`:470-503`). Zed's counterpart is `GitBinary` + `build_command`
(`zed/crates/git/src/repository.rs:3779,3852,3882`). Note the workspace has **two** git process layers:
`fleet-git` and the daemon's `adapters/git.rs`, which goes through the `Shell` adapter and shares
none of the hardening.

**`clippy.toml`** deliberately bans only `serde_json::from_reader`, with the comment: *"Fleet uses
blocking process APIs on dedicated Tokio/PTY threads. Process and timer restrictions belong at
those ownership boundaries, not across the workspace."* Zed bans
`std::process::Command::{spawn,output,status,stdin,stdout,stderr}` and `smol::Timer::after`
workspace-wide (`zed/clippy.toml:8-19`). If fleetd adopts those, scope them to `fleet-app`,
`fleet-ui-kit` and `fleet-lazygit` only.

---

## P11 daemon loops and select

Seven daemon periodic loops, all `CancellationToken`-aware, spawned together and joined on shutdown:

```rust
// crates/fleet-daemon/src/services/maintenance.rs:87-107 (trimmed)
let handles = vec![
    tokio::spawn(self.watches.clone().run(shutdown.clone())),
    tokio::spawn(self.watch_discovery.clone().run(shutdown.clone())),
    tokio::spawn(run_status_refresh(Arc::clone(self), events.clone(), shutdown.clone())),
    tokio::spawn(run_agent_activity_refresh(Arc::clone(self), shutdown.clone())),
    tokio::spawn(run_host_refresh(Arc::clone(self), events.clone(), shutdown.clone())),
    tokio::spawn(run_pool_refresh(Arc::clone(self), events, shutdown.clone())),
    tokio::spawn(run_pr_cache_expiry(Arc::clone(self), shutdown)),
];
Ok(PeriodicTasks { handles })
```

**The gap:** 40 `tokio::select!` in the workspace, **zero** `biased;`. Where one branch is a
shutdown or control signal and the other is a data firehose, the control branch must win a tie:

```rust
let event = tokio::select! {
    biased;
    () = shutdown.cancelled() => return,
    _ = updates.closed() => return,
    event = events.recv() => event,
};
```

Compare `crates/fleet-client/src/terminal.rs:271-274`, which races `updates.closed()` against
`events.recv()` without `biased;` — a terminal that is closing can still process a frame first.
Zed's rationale for the biased form: *"Process any path refresh requests from the worktree.
Prioritize these before handling changes reported by the filesystem."*
(`zed/crates/worktree/src/worktree.rs:4561-4566`; 70 `select_biased!` vs 55 `select!`).

Zed's heartbeat/reconnect loop is worth lifting wholesale if fleetd's bridge health checks grow:
a fused keepalive timer reset at the bottom of each iteration, the activity channel as the
biased-first branch, a nested `select_biased!` racing the ping against fresh activity, a
`missed_heartbeats` counter, then `ControlFlow::Break` to trigger reconnect
(`zed/crates/remote/src/remote_client.rs:781-838`).

---

## P12 file watching

`crates/fleet-git/src/watch.rs` (405 lines) is the workspace's only `notify` user and the best
storm defence in the repo. Copy its shape for any new watcher:

- The notify callback runs on notify's own thread and does **no async work**: it filters, inserts
  into a `Mutex<PendingChanges>`, and pokes an `async_channel::bounded(1)` wake channel with
  `try_send`, so a pending wake already covers every buffered path and the channel cannot back up
  (`:41-71`).
- `DEBOUNCE = 150 ms`, `MAX_DEBOUNCE = 1 s`, `MAX_DIRTY_PATHS = 1024` (`:16-18`).
- Past `MAX_DIRTY_PATHS` the path set collapses to the roots and flips `full_refresh` (`:158-167`);
  a backend error does the same and is retained for `take_error()` (`:100-124`). **A watcher
  failure degrades to a full refresh, never to a silent stall.**
- It watches up to three roots — worktree root plus `git_dir`/`common_dir` only when not already
  inside it (`:75-88`) — covering linked worktrees and the shared object dir without
  double-watching.

---

## Error surfacing ladder

| Call | When | Count in fleetd |
| --- | --- | --- |
| `?` inside a `Task<Result<_>>` | the failure ends the task | 0 task fields typed `Result` today |
| `.detach_and_log_err(cx)` | fallible fire-and-forget; logs with the caller's file/line (`zed/crates/gpui/src/executor.rs:48-54`) | **0** |
| `.detach()` | infallible fire-and-forget | 33 in `fleet-app/src` |
| `.ok()` | failure is genuinely expected ("window already closed") | — |
| `let _ =` / `let _ignored =` | never, on a fallible call | 21 + 14 in `fleet-app/src` |

Zed's canonical task body (`zed/crates/git_ui/src/git_panel.rs:2341`):

```rust
cx.spawn(async move |_, cx| {
    if let Err(e) = receiver.await? {
        if let Some(workspace) = workspace.upgrade() {
            cx.update(|cx| show_error_toast(workspace, "add to .gitignore", e, cx));
        }
    }
    anyhow::Ok(())
})
.detach_and_log_err(cx);
```

`anyhow::Ok(())` as the last line pins the error type so `?` works on the entity updates above it.

fleetd's app-side task fields are all `Task<()>` and exit through
`let Ok(..) = .. else { return }` (`shell/root/events.rs:142-144`,
`screens/board/lifecycle.rs:55-60`). That is acceptable for one-step tasks; for multi-step ones the
`Result` + `?` + `anyhow::Ok(())` form removes the nested `else { return }` ladder.

`detach_and_log_err` emits through the `log` crate. `fleet-app` installs
`tracing_subscriber::fmt().with_env_filter(..).try_init()`
(`crates/fleet-app/src/shell/root/bootstrap.rs:28-35`) and `tracing-log` is in `Cargo.lock`, so the
bridge should be in place — confirm the record actually appears the first time you rely on it.
fleetd already surfaces user-visible failures through `AppState::toast_*`
(`terminal/surface.rs:660`) and `Bridge::report_mutation_failure` (`bridge.rs:369-377`), which
are the fleetd equivalents of Zed's `detach_and_notify_err`.

---

## Channel selection

| Need | fleetd's choice | Evidence |
| --- | --- | --- |
| app ↔ bridge commands and events | `async_channel::bounded` | `crates/fleet-app/src/bridge.rs:315-316,51-54` |
| single daemon reply | `async_channel::bounded(1)` | `crates/fleet-app/src/bridge.rs:410` |
| notify wake signal (coalescing) | `async_channel::bounded(1)` + `try_send` | `crates/fleet-git/src/watch.rs:41-71` |
| daemon event fan-out | `tokio::sync::broadcast` | `crates/fleet-client/src/terminal.rs:267` |
| daemon backpressured stream | `tokio::sync::mpsc` | `crates/fleet-client/src/terminal.rs:268` |
| one-shot completion across a thread | `tokio::sync::oneshot` | `crates/fleet-client/src/spawn.rs:108` |
| byte-budgeted PTY queues | custom, with byte bounds | `crates/fleet-term/src/host.rs:32-35` |

Rules: bounded is the default at the bridge because backpressure there is real; every bound is a
named `const` with a comment saying what happens when it fills. `bridge.send` converts a full queue
into a resync flag plus a visible failure rather than blocking or silently dropping
(`bridge.rs:369-377`) — that is the standard to match for any new bounded channel.

Zed's table for comparison: `oneshot` for one-shot replies (212 uses), `mpsc::unbounded` for
single-consumer event streams (128), `async_channel` for multi-consumer / `send_blocking` (167),
its in-tree `watch` crate for "latest value wins" (53), `postage::barrier` for completion barriers.

---

## Doc authority

Async changes usually touch one of these; the doc and the code move in the same commit
(`docs/README.md` assigns the domains):

- `docs/ARCHITECTURE.md` — processes, the daemon, the terminal pipeline, the client, the bridge.
  Guiding rule 1 at `:12` ("Nothing the user started is ever tied to a UI surface"), render
  discipline at `:384-386`, daemon test layering at `:392`.
- `docs/APP-CONTRACTS.md` — how `fleet-app`'s parts plug together. "Render prepares nothing"
  at `:101-104`: filesystem access, request initiation, expensive projection and focus
  reconciliation belong in `synchronize`, an observation, or a background task — never in `render`.
- `docs/decisions/` — anything that changes a lifetime or cancellation contract (the bridge shape,
  adopting a `gpui_tokio`-style wrapper, pooling terminal threads) is ADR material.

Verification: `make lint` (fmt-check + `clippy -D warnings`), `make test` (builds `fleetd` first
because app/integration tests launch the real daemon binary), `make check`. Toolchain is pinned:
Rust 1.97.1, edition 2024, gpui at Zed tag `v1.18.1`.
