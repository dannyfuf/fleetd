---
name: rust-async-background-work
description: How fleetd runs asynchronous and background work in its two worlds — the GPUI app (smol-based foreground/background executors, `Task` ownership, debounce timers, the `Bridge` to the daemon) and the tokio daemon/client (`tokio::spawn`, `spawn_blocking`, `CancellationToken` loops, PTY threads). Load it before adding or changing any `cx.spawn`, `cx.background_executor().spawn`, `Task` field, `.detach()`, debounce constant, `bridge.request`/`bridge.send` call, `tokio::spawn`, `spawn_blocking`, `tokio::select!`, `std::thread::spawn`, or any timer/retry/poll loop — and when reviewing async code for cancellation, stale results, blocking the UI thread, or swallowed errors. Also load it when writing a `#[gpui::test]` that must drive async behaviour deterministically.
---

# Async and background work in fleetd

fleetd runs two async worlds. The **app** (`fleet-app`, `fleet-ui-kit`, `fleet-lazygit`) runs on
GPUI's smol-flavoured foreground/background executors; the **daemon and client** (`fleet-daemon`,
`fleet-client`, `fleet-cli`, `fleet-git`) run on tokio. They never touch each other's runtime:
they meet only at `Bridge` (`crates/fleet-app/src/bridge.rs`), a tokio multi-thread runtime on a
named OS thread talking to the UI through `async_channel`. This skill covers both sides and the
seam. GPUI patterns are verified against Zed **v1.18.1**, the GPUI tag fleetd depends on
(`Cargo.toml`, ADR `docs/decisions/0001-gpui-and-toolchain.md`).

## When to use

- Adding or changing a `cx.spawn`, `cx.background_executor().spawn`, `cx.background_spawn`, or a
  `Task<…>` field in `fleet-app` / `fleet-lazygit`.
- Wiring a `bridge.send(..)` / `bridge.request(..)` call from a view, or a new reply-handling path.
- Adding a debounce, a retry, a tail/poll loop, or any timer; touching daemon background work
  (`tokio::spawn`, `spawn_blocking`, a periodic loop in `services/maintenance.rs`,
  a `tokio::select!`, a dedicated `std::thread`).
- Reviewing async code for cancellation, stale results, blocked UI frames, or swallowed errors.
- Writing a `#[gpui::test]` / `#[tokio::test]` that must drive async behaviour deterministically.

## When not to

- Pure render/layout/styling questions — use `gpui-styling` or `gpui-components`.
- Entity graph, subscription and global lifetime questions — use `gpui-state-and-memory`.
- Wire format, framing, reconnect and protocol versioning — use `rust-ipc-protocol`.

## Rules

1. **Run it on the foreground executor unless it is CPU-bound or blocking.** GPUI polls `!Send`
   futures on the one app thread; that is where entity access lives. Only work with owned inputs
   and no entity access belongs on the background executor. fleetd already respects this —
   `docs/ARCHITECTURE.md:385-386` ("Render performs no filesystem access and starts no request")
   is honoured, and all four blocking-IO sites in `fleet-app` sit inside background tasks or the
   bridge thread (audit §7.3).

2. **Spawn with the narrowest context that does the job.** From `&mut App`,
   `cx.spawn(async move |cx| …)` (`gpui/src/app.rs:1959`); from `&mut Context<T>`,
   `cx.spawn(async move |this, cx| …)`, which hands you a `WeakEntity<T>` by construction
   (`gpui/src/app/context.rs:237`). Add `cx.spawn_in(window, …)` only when the continuation needs
   the `Window` (`context.rs:676`) — it pins the task to a window that can close. Use
   `cx.background_spawn(fut)` / `cx.background_executor().spawn(fut)` for `Send + 'static` work
   with no entity access (`gpui/src/app.rs:2880`). fleetd uses the `Context<T>` form in
   `shell/root/events.rs:113`, the `&mut App` form in `screens/board/lifecycle.rs:27`; `spawn_in`
   has 0 uses today.

3. **Never hold an `Entity`, `Window`, or lock guard across an `.await`.** Re-enter through
   `this.update(cx, …)` / `state.update(cx, …)` / `window.update(cx, …)` on every return from an
   await, and let the failure of that re-entry end the task. Zed's `.rules` also forbids nested
   entity updates and using the outer `cx` inside an update closure.

4. **Every `Task` is awaited, stored in a field, or deliberately detached.** `Task` is `#[must_use]`
   and dropping it cancels the work; storing it is what makes work cancellable. fleetd does this
   correctly in `shell/root.rs:72` (`_tasks: Vec<Task<()>>`), `screens/hub.rs:128-130`,
   `screens/jobs.rs:60`, `screens/workspace.rs:212`, `dialogs/host.rs:54-55`. Prefix a field `_`
   when it exists purely to keep the task alive.

5. **Cancel by replacement, not by flag.** Assigning a new `Task` over the field holding the old
   one drops and cancels it mid-flight — `screens/hub/cache.rs:565` does this for the inspect
   debounce. Add a generation counter only when a landed result must also be *ignored*.

6. **Debounce with a named `const … : Duration` and a timer as the first statement.**
   `cx.background_executor().timer(CONST).await` then the work, inside a task stored in a field;
   the next trigger replaces the field and the old timer dies. fleetd already does this:
   `AUTO_INSPECT_DEBOUNCE` (`screens/hub.rs:75`, used `hub/cache.rs:544`), `DEBOUNCE`
   (`dialogs/clone_repo.rs:19,280`), `ATTACH_RETRY_DELAY` (`terminal/surface.rs:34,620`), `TICK`
   (`shell/root/events.rs:15`). Never `smol::Timer::after`, `std::thread::sleep`, or
   `tokio::time::sleep` on the GPUI side.

7. **Surface errors instead of dropping them.** `gpui::TaskExt::detach_and_log_err(cx)` exists in
   the pinned GPUI (`gpui/src/executor.rs:35-54`, re-exported through `gpui::prelude`) and is the
   default for a `Task<Result<_>>`. fleetd has **33 bare `.detach()` sites in `fleet-app/src` and
   zero `detach_and_log_err`** (audit §7.5), plus 21 `let _ =` and 15 `let _ignored` on fallible
   calls. Convert the site you touch: `Task<anyhow::Result<()>>`, `?` on the entity updates,
   `anyhow::Ok(())` last, `.detach_and_log_err(cx)`. Never leave a `let _ =` on a fallible call.

8. **Long-running IO is: reader → channel → one foreground drain loop.** The consumer must live on
   the foreground executor (only it can touch entities), must batch or yield so a chatty producer
   cannot starve the frame, and must exit when the channel closes *or* the entity is gone. fleetd's
   canonical implementation is `shell/root/events.rs:111-160` (batch ≤ `EVENT_BATCH_LIMIT = 128`,
   then a 1 ms timer) — copy that shape.

9. **Pick the channel deliberately and justify the bound in a comment.** `async_channel` for the
   cross-runtime bridge and multi-consumer cases, `tokio::sync::{mpsc, broadcast, oneshot, watch}`
   inside the daemon, `oneshot` for a single reply. fleetd's bounds are already documented with
   rationale (`bridge.rs:51-54`: `COMMAND_CAPACITY = 512`, `EVENT_CAPACITY = 1_024`;
   `fleet-term/src/host.rs:32-35`) — keep that standard, and define what a full queue does (fleetd
   converts it to a resync flag plus a user-visible failure, `bridge.rs:369-377`).

10. **Fence blocking work at an ownership boundary.** No `std::fs::*`, `std::process::Command`, or
    `.lock()`-then-`.await` on the GPUI thread. Blocking work goes behind
    `cx.background_executor().spawn` (app), `tokio::task::spawn_blocking` (daemon, 21 sites, e.g.
    `stores/state.rs:47`, `services/snapshots.rs:176`), a named dedicated thread
    (`fleet-term/src/pty.rs:295,452,460`), or a trait adapter (`Files`, `Shell`, `Git` in
    `fleet-daemon/src/adapters/`).

11. **The daemon is reached only through `Bridge`; never start a runtime in a view.** There is
    exactly one app-side tokio runtime, on the thread named `fleet-daemon-bridge`
    (`bridge.rs:321-324,479-493`). `bridge.send(body)` is fire-and-forget; `bridge.request(body)`
    returns an `async_channel::Receiver<Result<ResponseBody, ProtoError>>` a `cx.spawn` awaits
    (`bridge.rs:363,409`). Note the gap vs Zed's `gpui_tokio`: **dropping the GPUI task does not
    cancel the in-flight daemon request** — see `## fleetd-specific guidance`.

12. **In the daemon, every long-lived loop is cancellation-aware and puts control first.**
    All seven periodic loops take a `CancellationToken`
    (`fleet-daemon/src/services/maintenance.rs:87-107`). In a `tokio::select!` whose branches
    include a shutdown or control channel, add `biased;` so the control branch wins — fleetd has
    40 `tokio::select!` and **zero** `biased;`, so today shutdown races output on every one.

13. **Tests are deterministic — no sleeping, no wall clock.** `#[gpui::test]` +
    `cx.run_until_parked()` (106 uses) + `cx.executor().advance_clock(2 * DEBOUNCE)` (22 uses, e.g.
    `crates/fleet-app/src/views/watch_pane/tests.rs:135`,
    `crates/fleet-app/src/terminal/surface/tests.rs:78`). To await a task stored in a field,
    `std::mem::replace(&mut field, Task::ready(()))` then `.await` it. `advance_clock` makes timers
    ready without running tasks (`gpui/src/executor.rs:201-204`).

14. **Docs move with the code.** `docs/README.md` assigns each document a domain of authority.
    Async placement, the bridge and the terminal pipeline are governed by `docs/ARCHITECTURE.md`;
    the shell/screen/view async contract by `docs/APP-CONTRACTS.md` ("Render prepares nothing",
    `:101-104`). A change contradicting a doc is a bug in one of the two — fix both in the same
    commit, styled `app: <imperative lowercase summary>`.

## Core patterns

### Bridge request → foreground apply, guarded by a generation

```rust
// shape of crates/fleet-app/src/screens/board/lifecycle.rs:17-40
let Some((context_id, generation)) = state.update(cx, |s, _| s.begin_board_load()) else {
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
```

The generation is taken *before* the request goes out and re-checked when the reply lands, so a
superseded reply is discarded (`references/patterns.md#p1-bridge-request`).

### Background hop, then re-enter the entity

```rust
// shape of crates/fleet-app/src/shell/root/actions.rs:169-180
let home = self.state.read(cx).home.clone();
let probe = cx
    .background_executor()
    .spawn(async move { home.join("state.json").exists() });
cx.spawn(async move |shell, cx| {
    let exists = probe.await;
    shell.update(cx, |shell, cx| shell.apply_import_probe(exists, cx))?;
    anyhow::Ok(())
})
.detach_and_log_err(cx);
```

Owned inputs go into the background future; nothing entity-shaped crosses the boundary.
`cx.background_spawn(fut)` is the shorter equivalent, used in `fleet-lazygit`
(`crates/fleet-lazygit/src/diff_view.rs:418`, 8 sites).

### Debounce in a stored task, cancelled by replacement

```rust
// shape of crates/fleet-app/src/screens/hub/cache.rs:537-565
let hub = self.hub.downgrade();
let generation = self.hub.read(cx).inspect_generation;
let task = cx.spawn(async move |cx| {
    cx.background_executor().timer(AUTO_INSPECT_DEBOUNCE).await;
    cx.update(|cx| {
        let Some(hub) = hub.upgrade() else { return };
        if hub.read(cx).inspect_generation != generation {
            return;
        }
        // …do the work…
    });
});
self.hub.update(cx, |hub, _| hub.inspect_task = Some(task));
```

The assignment on the last line drops any in-flight debounce. `AUTO_INSPECT_DEBOUNCE` is a named
`const` at `crates/fleet-app/src/screens/hub.rs:75` — the value the test advances.

### Reader → channel → batched foreground drain loop

```rust
// shape of crates/fleet-app/src/shell/root/events.rs:111-160
cx.spawn(async move |shell, cx| {
    while let Ok(first) = events.recv().await {
        let mut batch = Vec::with_capacity(EVENT_BATCH_LIMIT);
        batch.push(first);
        while batch.len() < EVENT_BATCH_LIMIT {
            let Ok(event) = events.try_recv() else { break };
            batch.push(event);
        }
        let Ok(()) = shell.update(cx, |shell, cx| apply_batch(shell, batch, cx)) else {
            return;
        };
        // A busy producer cannot keep the UI executor inside an always-ready receive loop.
        cx.background_executor().timer(Duration::from_millis(1)).await;
    }
})
```

Same shape as Zed's terminal event loop (`zed/crates/terminal/src/terminal.rs:1354`). Store the
returned `Task` in `_tasks` so dropping the shell stops the drain.

### Daemon: cancellation-aware loop with a biased select

```rust
// shape for crates/fleet-daemon/src/services/*.rs periodic loops
loop {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => return,
        () = tokio::time::sleep(REFRESH_INTERVAL) => {}
    }
    if let Err(error) = refresh(&services).await {
        tracing::warn!(%error, "status refresh failed");
    }
}
```

`biased;` makes shutdown win a tie against a ready timer or a chatty channel. Spawn these with
`tokio::spawn`, keeping the `JoinHandle`s so shutdown can join them (`maintenance.rs:87-107`).

### Deterministic async test

```rust
#[gpui::test]
async fn inspection_debounces(cx: &mut TestAppContext) {
    cx.run_until_parked();                                  // drain arrange-time work
    cx.executor().advance_clock(2 * AUTO_INSPECT_DEBOUNCE); // make the timer ready
    cx.run_until_parked();                                  // then assert on the entity
}
```

`advance_clock` makes timers ready without running tasks; `run_until_parked` drains what is
runnable. To await a stored task, `mem::replace` it out of its field first — see
`references/patterns.md#p7-tests`.

## Anti-patterns

| Anti-pattern | Why it hurts | Do this instead |
| --- | --- | --- |
| Bare `.detach()` on fallible work | The error vanishes; no log, no toast, no test signal | `Task<anyhow::Result<()>>` + `.detach_and_log_err(cx)` |
| `let _ = fallible()` / `let _ignored = …` | Same, and it reads as intentional when it usually is not | `?`, `.ok()` for genuinely expected failures, or log it |
| Holding `Entity`/`Window`/a guard across `.await` | Panics on re-entrant update, or keeps the entity alive forever | Re-enter with a fresh `update(cx, …)` after every await |
| `std::thread::sleep` / `smol::Timer::after` / `tokio::time::sleep` in app code | Blocks the frame or breaks `run_until_parked()` determinism | `cx.background_executor().timer(CONST).await` |
| `std::fs::*` or `Command` on the GPUI thread | One slow syscall drops frames | `cx.background_executor().spawn` with owned inputs |
| An `AtomicBool` cancel flag where a dropped `Task` would do | Two mechanisms to reason about; the flag outlives the task | Store the `Task` and reassign to cancel |
| A drain loop with no yield/batch | A chatty producer starves rendering | Batch to a `const` limit, then `timer(1ms).await` |
| `tokio::select!` with shutdown but no `biased;` | Shutdown loses coin flips against a hot branch | `biased;` with the control branch first |
| Starting a tokio runtime or `block_on` in a view | Deadlocks the UI thread; duplicates the bridge | Go through `Bridge` |
| Unbounded channel with no rationale | Memory grows silently under backpressure | Bounded with a documented capacity and a defined full-queue behaviour |
| Real sleeps / `Instant::now()` deltas in assertions | Flaky tests | `advance_clock` + `run_until_parked` |

## fleetd-specific guidance

**Keep doing** (at or above Zed's bar — do not "fix" these): background executor for every
filesystem walk, metadata read and subprocess in the app; `_subscriptions`/`_tasks` retention
fields (`shell/root.rs:71-72`); documented channel bounds (`bridge.rs:51-54`);
`spawn_blocking`-fenced daemon IO; a `CancellationToken` on every periodic loop; batching and
yielding in the bridge event loop; 105 `#[gpui::test]` with `run_until_parked`/`advance_clock`.

**The three concrete gaps to close, incrementally, as you touch the code:**

1. **Error surfacing.** 33 `.detach()` in `fleet-app/src`, zero `detach_and_log_err` (audit §7.5).
   `gpui::TaskExt` is already available at the pinned tag — no new trait needed. Convert the site
   you are editing, not the whole tree. `detach_and_log_err` emits through the `log` crate;
   `fleet-app` installs `tracing_subscriber::fmt().try_init()`
   (`crates/fleet-app/src/shell/root/bootstrap.rs:28-35`), whose default `tracing-log` feature
   bridges `log` into tracing — confirm the record really appears the first time you rely on it.

2. **Generation guards vs `WeakEntity`.** fleetd recovers correctness with hand-rolled generation
   counters (`state.rs` `board_generation`, `screens/hub.rs:128` `inspect_generation`,
   `inspection_requests`, `link_generation`) because detached tasks capture a **strong**
   `Entity<AppState>` (`screens/board/lifecycle.rs:26-39`). That is deliberate and it works —
   `WeakEntity` appears only 11 times in the workspace. **Do not rewrite it.** For *new* stateful
   sub-views, dialogs or lists that become their own `Entity`, prefer
   `cx.spawn(async move |this, cx| …)` with the `WeakEntity` GPUI hands you, plus `?` on the
   updates. Keep a generation counter only where a stale reply must be *ignored* after landing
   (which is most bridge replies, so most existing guards stay).

3. **Bridge requests are not drop-cancelled.** `bridge.request` returns an
   `async_channel::Receiver`, not a `Task` (`bridge.rs:409-425`), so dropping the awaiting GPUI
   task leaves the daemon request in flight — unlike Zed's `gpui_tokio::Tokio::spawn`, which
   aborts its tokio join handle when the returned `Task` drops
   (`zed/crates/gpui_tokio/src/gpui_tokio.rs:55-73`). For fleetd this is usually *correct*:
   `docs/ARCHITECTURE.md:12` says nothing the user started is tied to a UI surface. Treat it as a
   known property, guard stale replies with a generation, and do not assume cancellation.
   A `Tokio::spawn`-shaped wrapper is a design change worth an ADR, not a PR.

**Timeouts.** fleetd hand-rolls a `poll_fn` racer, `before_timeout`, duplicated verbatim in
`terminal/surface.rs:629-644` and `dialogs/create_worktree.rs:512-527` (called at `surface.rs:582`,
`create_worktree.rs:467,488`). Keep the call sites; hoist **one** shared helper when you next touch
either file. Zed's idiom is `futures::select_biased!` over `executor.timer(..)`
(`zed/crates/project/src/debounced_delay.rs:38-43`); fleetd spells it `futures_util::select_biased!`, but
`crates/fleet-app/Cargo.toml` does not take `futures-util` yet — its own commit, not a drive-by.

**Other placement facts:** `fleet-term` uses zero tokio — it is a pure blocking-thread crate, five
OS threads per live terminal (`fleet-term/src/pty.rs:295,452,460`, `fleet-term/src/host.rs:204`,
plus a daemon forwarder). `fleet-git` is the only `notify` user (`crates/fleet-git/src/watch.rs`,
`DEBOUNCE = 150ms`, `MAX_DEBOUNCE = 1s`, `MAX_DIRTY_PATHS = 1024`); its callback does no async work
and degrades to a full refresh instead of stalling — copy that when adding a watcher. `clippy.toml`
deliberately bans only `serde_json::from_reader`, noting that process/timer restrictions "belong at
those ownership boundaries"; if you add the Zed-style ban, scope it per crate, never
workspace-wide (the daemon and `fleet-term` legitimately block).

**Verify with** `make lint` (fmt-check + clippy `-D warnings`) and `make test` (builds `fleetd`
first, because app/integration tests launch the real daemon binary).

## Review checklist

Full list in `references/checklist.md`. The quick pass:

1. Right executor — entity access on the foreground, owned CPU/blocking work on the background?
2. Is every `Task` awaited, stored in a field, or detached with a stated reason?
3. Does re-triggering the action cancel or supersede the previous run?
4. Is any `Entity`, `Window` or lock guard held across an `.await`?
5. Does fallible work end in `?` + `anyhow::Ok(())` + `.detach_and_log_err(cx)`, not `.detach()` or `let _ =`?
6. Are delays `background_executor().timer(NAMED_CONST)` (app), or `tokio::time` behind a `biased;` shutdown branch (daemon)?
7. Does every long-running loop have an exit condition, and does it batch or yield?
8. Is any `std::fs` / `Command` / `block_on` reachable from the GPUI thread?
9. Is a new channel's boundedness a deliberate, commented choice with defined full-queue behaviour?
10. Is the new behaviour covered by a test driven by `run_until_parked` + `advance_clock`, no real sleeps?

## Related skills

- `gpui-state-and-memory` — entities, `WeakEntity`, subscriptions, `Task` fields as a lifetime tool, globals.
- `rust-ipc-protocol` — what travels over the bridge: framing, request/response, reconnect, versioning.
- `rust-gpui-testing` — `#[gpui::test]`, `TestAppContext`, fakes, deterministic async, IPC tests.
- `gpui-performance` — render discipline and the frame budget this skill's yielding rules protect.
- `rust-workspace-architecture` — crate layering, lints, error handling, where a new helper belongs.
- `zed-quality-review` — aggregate review pass that loads `references/checklist.md`.
