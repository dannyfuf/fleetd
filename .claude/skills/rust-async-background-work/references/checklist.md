# Review checklist — async and background work

Standalone. Apply to any diff that adds or changes a `cx.spawn`, `cx.background_executor().spawn`,
`cx.background_spawn`, a `Task<…>` field, a `.detach()`, a `bridge.send`/`bridge.request` call, a
timer or debounce, a `tokio::spawn`, a `spawn_blocking`, a `tokio::select!`, a `std::thread::spawn`,
or a long-running loop. Each item is yes/no; "no" means fix before merge unless the exception is
stated in the diff. `[app]` = `fleet-app` / `fleet-ui-kit` / `fleet-lazygit`; `[daemon]` =
`fleet-daemon` / `fleet-client` / `fleet-cli` / `fleet-git` / `fleet-term`.

## Placement

1. **Does the work go on the executor its data demands?** Entity access must be foreground
   (`cx.spawn`); `Send + 'static` compute with owned inputs should be background
   (`cx.background_spawn` / `cx.background_executor().spawn`). *Why:* a background task cannot reach
   an entity at all, and foreground CPU work drops frames. *Fix:* split into a background future
   over owned data plus a foreground continuation that re-enters the entity.

2. `[app]` **Is `spawn_in(window, ..)` used only when the continuation actually needs the
   `Window`?** *Why:* `AsyncWindowContext` pins the task to a window that can close, adding failure
   surface for nothing. *Fix:* use `cx.spawn` and re-enter the window explicitly if needed.

3. **Is a new tokio runtime, `block_on`, or `Handle::current()` absent from app code?** *Why:*
   fleetd has exactly one app-side runtime, on the `fleet-daemon-bridge` thread
   (`crates/fleet-app/src/bridge.rs:321-324,479-493`); a second one deadlocks or double-schedules.
   *Fix:* route through `Bridge`.

4. **Is blocking work fenced at an ownership boundary?** No `std::fs::*`, `std::process::Command`,
   `.lock()`-then-`.await`, or `std::thread::sleep` reachable from the GPUI thread. *Why:* one slow
   syscall costs a frame. *Fix:* `cx.background_executor().spawn` `[app]`,
   `tokio::task::spawn_blocking` `[daemon]`, a named dedicated thread, or an `#[async_trait]`
   adapter in `crates/fleet-daemon/src/adapters/`.

5. `[daemon]` **Is a new subprocess spawned through the hardened runner?** *Why:*
   `crates/fleet-git/src/command.rs` sets `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`, `kill_on_drop(true)`,
   output limits and a supervising `join!` so a chatty child cannot deadlock (`:234-255,505-580`).
   *Fix:* use `fleet-git`'s runner or the `Shell` adapter; never `sh -c`, never raw
   `std::process::Command` in a request path.

6. **Is a new OS thread named?** *Why:* every production thread in fleetd is named
   (`fleet-daemon-bridge`, `fleet-pty-reader`, `fleet-remove`, `fleetd-reaper`, …), which is what
   makes a stuck process readable in a sample. *Fix:* `thread::Builder::new().name(..)`.

## Task ownership and cancellation

7. **Is every returned `Task` awaited, stored in a field, or detached?** *Why:* `Task` is
   `#[must_use]` and drops cancel; a silently dropped task is work that never happens. *Fix:* store
   it (`_tasks: Vec<Task<()>>`, `Option<Task<()>>`, or a keyed `HashMap`).

8. **If detached, is unbounded lifetime actually correct?** *Why:* a detached fleetd task holds a
   **strong** `Entity<AppState>`, so it cannot be stopped by closing the screen that started it.
   *Fix:* store the task on the owner if it should stop with the owner; keep `.detach()` only for
   work that must complete regardless.

9. **Does re-triggering the action cancel or supersede the previous run?** *Why:* two copies of the
   same debounce or refresh racing produces flicker and lost writes. *Fix:* assign the new `Task`
   over the field holding the old one (`crates/fleet-app/src/screens/hub/cache.rs:565`), or bump a
   generation.

10. **If a stale result could still land, is it rejected on arrival?** *Why:* bridge requests are
    **not** cancelled when the awaiting GPUI task drops — `bridge.request` returns an
    `async_channel::Receiver`, not a `Task` (`crates/fleet-app/src/bridge.rs:409-425`). *Fix:* mint
    a generation before the request and check it in the apply path, as
    `state/board.rs:191,223` and `screens/hub.rs:132-133,170-173` do.

11. **Is there exactly one mechanism per concern?** An `AtomicBool` cancel flag *and* a stored task
    *and* a generation for the same work is two too many. *Why:* each extra mechanism is another
    state to get wrong. *Fix:* drop-cancellation when it suffices; a generation only when the
    result must be ignored after landing.

## Across awaits

12. **Is any `Entity`, `Window`, `App`, or lock guard held across an `.await`?** *Why:* it either
    panics on a re-entrant update or keeps the object alive indefinitely. *Fix:* re-enter with a
    fresh `this.update(cx, …)` / `state.update(cx, …)` after every await.

13. **Is the entity-gone case handled explicitly?** `?` (ends the task), `.ok()` (expected, e.g.
    window already closed), or a log — but never `let _ =`. *Why:* silent exit hides a real
    lifetime bug. *Fix:* pick one and make it visible in the code.

14. **Is the inner `cx` used inside every update closure, never the captured outer one?** *Why:*
    multiple borrows; Zed states this rule explicitly. *Fix:* shadow with the closure's `cx`.

15. **Is a nested entity update avoided?** *Why:* updating an entity while it is already being
    updated panics. *Fix:* release the lease first, as
    `crates/fleet-app/src/shell/root/events.rs:145-153` does before entering the window.

## Errors

16. **Does fallible work surface its failure?** *Why:* a bare `.detach()` on a `Task<Result<_>>`
    swallows the error entirely — no log, no toast, no test signal. *Fix:* make the task
    `Task<anyhow::Result<()>>`, `?` the entity updates, end with `anyhow::Ok(())`, and
    `.detach_and_log_err(cx)` (`gpui::TaskExt`, `zed/crates/gpui/src/executor.rs:35-54`; available
    at fleetd's pinned tag, currently 0 uses against 33 `.detach()` in `fleet-app/src`).

17. **Is there any new `let _ =` or `let _ignored =` on a fallible call?** *Why:* it reads as
    deliberate when it usually is not. *Fix:* `?`, `.ok()` with a comment saying why the failure is
    expected, or a `tracing::warn!`.

18. **Does user-initiated work that can fail reach the user?** *Why:* a silently failed clone or
    attach looks like a hang. *Fix:* a toast via `AppState::toast_*`
    (`crates/fleet-app/src/terminal/surface.rs:660`) or `Bridge::report_mutation_failure`
    (`crates/fleet-app/src/bridge.rs:455`).

19. **Does every early-exit path release the latches it took?** *Why:* a "asked once per connection"
    flag set before the request must be cleared on failure or the request never happens again — see
    `crates/fleet-app/src/screens/board/lifecycle.rs:54-61`. *Fix:* clear on every error branch.

## Timers, loops and channels

20. **Are all delays executor timers with named `const … : Duration` intervals?**
    `cx.background_executor().timer(CONST).await` `[app]`, `tokio::time` `[daemon]`. *Why:*
    `std::thread::sleep` blocks a frame, `smol::Timer::after` breaks `run_until_parked()`
    determinism, and a literal interval cannot be advanced by a test. *Fix:* hoist a named const
    next to the existing ones (`AUTO_INSPECT_DEBOUNCE`, `DEBOUNCE`, `ATTACH_RETRY_DELAY`, `TICK`).

21. **Is the debounce timer the *first* statement of the task?** *Why:* work-then-wait does not
    debounce anything. *Fix:* `timer(CONST).await` first, work second, task stored in a field.

22. **Does every long-running loop have a defined exit?** Channel closed *and* entity gone. *Why:*
    a loop with only one exit condition leaks a task for the life of the app. *Fix:* `while let
    Ok(..) = rx.recv().await` plus `let Ok(..) = this.update(..) else { return }`.

23. **Does the drain loop batch or yield?** *Why:* an always-ready receiver starves rendering.
    *Fix:* copy `crates/fleet-app/src/shell/root/events.rs:111-160` — batch up to a named const,
    then `timer(1ms).await`.

24. `[daemon]` **Does a `tokio::select!` with a shutdown or control branch use `biased;` with that
    branch first?** *Why:* without it shutdown loses coin flips against a hot data branch; fleetd
    has 40 `tokio::select!` and zero `biased;`. *Fix:* add `biased;` (see
    `crates/fleet-client/src/terminal.rs:271-274`).

25. `[daemon]` **Does a new long-lived loop take a `CancellationToken` and is its `JoinHandle`
    kept?** *Why:* all seven existing periodic loops do
    (`crates/fleet-daemon/src/services/maintenance.rs:87-107`), which is how shutdown stays clean.
    *Fix:* thread `shutdown.clone()` through and push the handle into `PeriodicTasks`.

26. **Is a new channel's boundedness a deliberate, commented choice?** *Why:* unbounded hides
    backpressure until it is memory; bounded without a defined full-queue behaviour drops data
    silently. *Fix:* name the capacity as a const with a rationale comment (the standard is
    `crates/fleet-app/src/bridge.rs:51-54`) and define what a full send does — fleetd's is a resync
    flag plus a visible failure (`bridge.rs:369-377`).

27. **Does a new file watcher degrade rather than stall?** *Why:*
    `crates/fleet-git/src/watch.rs:41-71,100-124,158-167` does no async work in the notify callback,
    coalesces wakes through a `bounded(1)` channel with `try_send`, and collapses to a full refresh
    past `MAX_DIRTY_PATHS` or on a backend error. *Fix:* copy that shape; there should still be
    exactly one `notify` user in the workspace.

## Tests

28. **Does the new async behaviour have a test that drives it deterministically?** *Why:* async
    ordering bugs do not reproduce by hand. *Fix:* `#[gpui::test]` + `cx.run_until_parked()` +
    `cx.executor().advance_clock(2 * DEBOUNCE)` `[app]`; `#[tokio::test]` `[daemon]`.

29. **Are stored tasks awaited rather than slept on?**
    `std::mem::replace(&mut field, Task::ready(()))` then `.await`. *Why:* a sleep is a flake.
    *Fix:* see `zed/crates/git_ui/src/git_panel.rs:9361-9367`.

30. **Are the intervals the test advances the same named consts the code uses?** *Why:* a literal
    duplicated in the test drifts silently. *Fix:* import the const
    (`crates/fleet-app/src/terminal/surface/tests.rs:78` does this with `ATTACH_RETRY_DELAY`).

31. **Is the test free of wall-clock dependence?** No real sleeps, no assertions on
    `Instant::now()` deltas. *Why:* flaky under load and in CI. *Fix:* advance the test clock.

32. **Would `#[gpui::test(iterations = N)]` add value here?** *Why:* it re-runs with different
    seeds so `simulate_random_delay()` shuffles interleavings; fleetd has zero such tests and the
    bridge event loop plus the generation guards are exactly where ordering bugs hide. *Fix:*
    add it for tests whose assertion depends on task ordering.

## Documentation

33. **If the change alters where work runs, its cancellation contract, or the bridge shape, is the
    governing doc updated in the same commit?** `docs/ARCHITECTURE.md` owns processes, the daemon,
    the terminal pipeline and the client; `docs/APP-CONTRACTS.md` owns how `fleet-app`'s parts plug
    together ("Render prepares nothing", `:101-104`); `docs/decisions/` owns lifetime-contract
    changes. *Why:* `docs/README.md` makes each doc authoritative over its domain — code that
    contradicts a doc is a bug in one of the two. *Fix:* change both, commit as
    `<area>: <imperative lowercase summary>`.

34. **Does `make lint` pass?** fmt-check plus `clippy -D warnings`. `make test` builds `fleetd`
    first because app and integration tests launch the real daemon binary.
