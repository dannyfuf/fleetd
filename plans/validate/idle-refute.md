# Adversarial validation — batch `idle`

Refutation pass over C1 (F2-1 + F1-11), C2 (F1-10), C3 (F1-12), C4 (F2-6).
Base `881162c5`, branch `test-harness`.

---

### C1 — verdict: SURVIVES NARROWED

Two directions, settled separately. They are **not** contradictory readings: both describe the
same instant (the end of `dispatch`'s spawned task, `crates/fleet-app/src/bridge/requests.rs:122-134`),
which lies *after* the reply has been handed to the foreground and *before* the app has applied
anything. F2-1 objects to the second half, F1-11 to the first. Their fixes do conflict.

- **Confidence**: high (F2-1 mechanism), high (F1-11 negligibility)

#### F2-1 ("released too early") — the mechanism is real, and worse than stated; the impact is overstated

**What checks out.** Both release sites are where the reviewer says:

```rust
// crates/fleet-app/src/bridge/requests.rs:76-88  (Bridge::send lane)
while let Ok(Mutation { client, body, _in_flight }) = mutations.recv().await {
    if let Some(client) = client {
        if let Err(error) = client.request(*body).await { … }   // `_in_flight` drops at the end of this iteration
```
```rust
// crates/fleet-app/src/bridge/requests.rs:123-134  (Bridge::request lane)
tokio::spawn(async move {
    let _in_flight = in_flight;
    let result = client.request(body).await;
    …
    let _ignored = reply.send(result).await;
});                                                  // `_in_flight` drops here
```

And the gap is bigger than the finding claims. The daemon does not publish `SnapshotChanged`
inline with the mutation response — it **coalesces it behind a 50 ms sleep**:

```rust
// crates/fleet-daemon/src/server/broadcast.rs:13
const SNAPSHOT_COALESCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(50);
// crates/fleet-daemon/src/server/broadcast.rs:112-119
runtime.spawn(async move {
    tokio::time::sleep(SNAPSHOT_COALESCE_WINDOW).await;
    … events.publish(Event::SnapshotChanged(snapshot));
```

So for `Bridge::send` the hole between "claim released" and "app has the new snapshot" is a
deterministic ~50 ms, not a sub-millisecond race. I could not refute the mechanism.

**What does not check out — three separate overstatements.**

1. **The cited reachable path is wrong.** `scenarios/hub/pane-focus.scenario:13-15` is
   `key i` / `await idle` / `assert focused == worktrees.row[0] && hub_pane == List`. Every
   field in that assert is local chrome state that no daemon reply or `SnapshotChanged` can
   move. The example does not demonstrate the failure.

2. **No scenario in the corpus is exposed today.** I enumerated every
   `<mutating action>` immediately followed by `await idle` across `scenarios/` — 14 pairs
   (`pointer-basics:27`, `hub/pane-focus:13,39,44`, `hub/go-jumps:33`, `hub/palette:27`,
   `hub/detail-panel:17,30`, `hub/rail-collapse:21,34`, `hub/jobs-panel:21,27`,
   `hub/idle-accounting:20`, `hub/pointer-tabs:34`). In every one the following `assert` reads
   `mode`, `hub_pane`, `hub_tab`, `focused`, `overlay`, `screen`, `sticky_error` or a
   `lists.*.selected.label` that came from the snapshot already in hand. None asserts on state
   the pending daemon answer would change. "It degrades every scenario rather than one" is not
   supported.

3. **A waiter that parks is rescued by the very event the finding says is uncounted.** Nothing
   notifies `AppState` on the decrement, so once `wait_for` (`crates/fleet-app/src/drive.rs:651-700`)
   has parked with `in_flight == 1`, its only wake-up on a connected app with no toasts is
   `apply_batch`'s `cx.notify()` (`crates/fleet-app/src/shell/root/events.rs:110-112`) — which
   runs *after* the batch has been applied. The failure therefore needs the daemon to answer
   before the harness's **first** projection of the `await` line, i.e. before the runner's socket
   round-trip plus two `settle()`s. That is a coin flip, not a certainty.

4. **It is not a contract violation.** §2 freezes `idle` as "no in-flight app requests, running
   jobs, pending frame, live toast timer, or armed debounce", and
   `crates/fleet-app/src/bridge/tests.rs:691-696` pins the claim's lifetime as
   *"from admission to its answer"* in its name and doc. The code does exactly what the frozen
   contract says. The finding is "the frozen definition of `idle` is too weak for how the corpus
   uses it", which is a §11/§2 gap, not a bug — and §11 does not record it, so the gap is real.

#### F1-11 ("released too late") — true, and negligible

The ordering claim is correct (`requests.rs:133` then the block end at `:134`; same shape on the
offline early return at `:119-122`). But the window is the **three or four instructions** between
`async_channel::Sender::send` returning on the bridge runtime thread and the async block's drop
glue running `fetch_sub`. For a reader to observe the stale `1`, the foreground must, inside that
window: wake from park, poll the reply task, run the handler, `state.update`, `cx.notify()`, flush
effects, run the observer closure's `try_send` (`drive.rs:664-668`), get the driver task
re-scheduled, run `project()`, and `serde_json::to_value` the whole snapshot (`drive.rs:675-679`).
That is microseconds against nanoseconds, and it requires an OS preemption landing in the
nanosecond window. It is also self-healing for any mutating request, because the 50 ms
`SnapshotChanged` notify follows and re-projects.

More importantly: **F1-11's fix is wrong.** `drop(in_flight)` before `reply.send` would widen
F2-1's hole by exactly the reply-delivery latency, on the one lane (`Bridge::request`) where the
app *does* apply the answer promptly.

- **NARROWED to**: F2-1's mechanism holds — `idle` does not cover a daemon answer that has been
  produced but not yet applied, and the daemon's 50 ms snapshot coalesce makes that window
  deterministic for `Bridge::send`. But it is a §2/§11 contract-adequacy gap (a latent trap for
  scenarios not yet written), **not** a P1 that "degrades every scenario": no corpus `await idle`
  asserts on daemon-derived state, the cited reachable path is local-only, and a parked waiter is
  woken by the applied batch. F1-11 is technically true, practically unreachable, and its
  prescribed fix should not be taken.
- **Dropped from both**: F1-11's "the one `idle` race that needs no early-return path to reach"
  framing — it needs a nanosecond-scale preemption, which is a stronger precondition than any
  early-return path.

---

### C2 — verdict: SURVIVES NARROWED

- **Confidence**: high

**The headline is false as written.** "No `idle` counter's busy→idle transition notifies
`AppState`" is wrong for three of the five counters:

- `pending_frame` — `crates/fleet-app/src/shell/root/focus.rs:237-241`:
  `state.harness.set_pending_frame(still_queued); cx.notify();`. The finding concedes this,
  which already contradicts its own title.
- `live_toast_timers` — `AppState::tick` returns true exactly when `expire_toasts` removed one
  (`crates/fleet-app/src/state/connection.rs:118-120`), and `Shell::spawn_ticker`
  (`crates/fleet-app/src/shell/root/events.rs:170-186`) calls it every 250 ms and notifies on
  true. The edge is exercised end-to-end by `scenarios/hub/idle-accounting.scenario:36-41`
  (`advance 20000` / `await toasts[0] absent` / `assert idle.live_toast_timers == 0`) and by
  `an_await_timeout_names_the_idle_counter_that_is_still_busy` in `drive.rs`.
- `running_jobs` — projected from `state.snapshot` (`state/harness/projection.rs:266-268`), which
  only moves through `apply_batch`, which notifies whenever `damage.state`
  (`shell/root/events.rs:108-112`). Covered by `idle-accounting.scenario:27-31`.

**One of the two cited locations is dead code.** `HarnessState::finish_request`
(`crates/fleet-app/src/state/harness.rs:330`) has **no production caller** — `rg` over `crates/`
returns only `state/harness/tests.rs:142` and the definition. Its own doc says so:
*"this is the seam for a caller that owns a request the bridge cannot see, and the one the unit
tests drive"* (`harness.rs:321-324`). The bridge releases its claims through `InFlight::drop`,
never through `finish_request`. Citing `harness.rs:330` as a production early-return path is an
error.

**Both cited early-return paths are guarded.**

- `dialogs/clone_repo.rs:284` — `armed.disarm()` then two returns.
  - The *stale-seq* return requires a newer `schedule_search` to have already run, and that
    either armed a fresh guard (`clone_repo.rs:281`, so the count never reaches 0 — not a
    busy→idle edge) or took the empty-query branch, which calls `notify(state, cx)`
    (`clone_repo.rs:273`). Additionally the newer call's `retain_task(state, cx, "clone-search", …)`
    drops the older `Task`, releasing its guard inside that keystroke's own update path.
  - The `search_owners(...) → None` return (`clone_repo.rs:365`, `Err(_) => return None`) fires
    only when a reply `Sender` is dropped without sending. Every bridge path replies —
    `requests.rs:112-115` (offline), `bridge.rs:433-441` / `:462-470` (full/closed queue),
    `runtime.rs:233-240` (displaced while opening) — so the only producer of that error is the
    runtime loop returning, i.e. shutdown. And the disarm at `:284` happens *before* the requests
    are issued, with **no await point** between it and the first `bridge.request` inside
    `search_owners`, so no harness projection can interleave and observe the dip.
- `screens/hub/cache.rs:573` — `armed.disarm()` then `cx.update` with a dead-entity return and a
  generation return. `schedule_inspection` sets `hub.inspect_task = None` (`cache.rs:554`)
  *before* bumping the generation, so the older task and its guard are dropped at that moment;
  a task that survives to run its body past the timer is by construction the current generation,
  because both the drop and the body run on the foreground thread with no await between the
  timer resolving and the `cx.update`. The dead-entity arm is teardown.

**What is left.** The mechanism is real for exactly two counters:

- `in_flight_requests` — `InFlight::drop` (`crates/fleet-app/src/bridge.rs:356-361`) runs on the
  bridge runtime thread and cannot notify. A `Bridge::send` whose event produces no
  `AppState` damage (e.g. a `RequestFullFrame` for a terminal that is not the visible one, where
  `apply_batch`'s harness-mode notify at `events.rs:114-121` is gated by
  `affects_visible_terminal`) leaves a 1→0 edge with no wake-up.
- `armed_debounces` — the genuinely unnotified edge is a *different line* from the one cited:
  `schedule_inspection`'s `let Some(target) = target else { return; }` (`cache.rs:559-561`) drops
  the previous task's guard and arms nothing. Even that one runs synchronously inside
  `synchronize`, i.e. inside a `cx.observe(state)` callback registered at
  `screens/hub.rs:352`, so it sits inside an effect cycle that is already waking the harness's
  own observer, and `wait_for` re-projects after the flush.

The finding's claim that `spawn_ticker` "never" fires is also too strong: `AppState::tick` returns
true unconditionally while `DaemonLink::Starting | Lost` and for `Reconnected`
(`state/connection.rs:121-136`), and when a visible watch is running (`:118-120`) — so there *is*
a live 250 ms fallback across exactly the cold-start and fault windows.

- **NARROWED to**: "`in_flight_requests` and `armed_debounces` are the two `idle` inputs whose
  busy→idle edge raises no `AppState` notification, and `wait_for` has no fallback poll. The
  reachable unnotified edges are `InFlight::drop` on the runtime thread (`bridge.rs:359`) and
  `schedule_inspection`'s no-target return (`cache.rs:559`)." Everything else in F1-10 —
  the "no counter notifies" headline, `finish_request` as a live path, both quoted early
  returns, and "`spawn_ticker` never fires" — does not hold.

---

### C3 — verdict: SURVIVES

- **Confidence**: high on the mechanism, medium on the severity

I tried three separate refutations and all three failed; one of them backfired.

1. *"Requests admitted during cold start hold their claims, so `in_flight` is non-zero."*
   True — `runtime.rs:104-107` routes every command into `queue_while_opening` while
   `opening.is_some()`, and `runtime.rs:227-245` pushes the `InFlight` into `waiting` rather than
   dropping it, so a claim taken before the link opens survives the whole opening window. This
   would refute the finding **if any request were admitted**. None is.
2. *"`HubScreen::bind`'s `cx.defer(synchronize)` admits one."* — this is the reviewer's own stated
   reason the bug does not fire today, and it is **wrong**, in the direction that makes the
   finding stronger. `HubCtx::synchronize` (`screens/hub/cache.rs:495-531`) issues no bridge
   request while disconnected: `tick_pull_requests` returns at `cache.rs:419-426` because
   `state.daemon.is_connected()` is false (and the default tab is Worktrees, not Prs), and
   `inspection_target` returns `None` at `cache.rs:583-593` for the same reason, so
   `selected == inspect_target == None` and `schedule_inspection` is never called.
3. *"Something else in `Shell::new` requests at startup."* — no. The only `bridge.request`/`send`
   call sites under `shell/root/` are `daemon_lifecycle.rs:102` (Doctor), `actions.rs:178`
   (ImportFromSwarm), `agent.rs:148` (EnsureSession), `quit_actions.rs:145,192` and
   `events.rs:139` (RequestFullFrame) — all user- or event-driven.

So on a cold start every input is structurally zero (`in_flight` 0, `running_jobs` 0 because
`state.snapshot` is `None`, `pending_frame` false until the first
`drain_stale_keys_after_render`, no toasts, no debounces) and `IdleSnapshot::new`
(`state/harness.rs:184-193`) derives `idle: true` while `DaemonLink::Starting`.

What actually keeps the corpus green is the runner's own ordering: `fleetd` is started by the
harness before `fleet` launches, so `open()` succeeds in roughly a millisecond, while the runner
still has to poll for the app socket file and connect (`fleet-harness/src/client.rs:49-62`).
That is timing, not an invariant — exactly what the finding says, via a different route.

`scenarios/hub/idle-accounting.scenario:4-8` is the corroborating evidence: its own header records
that *"`await idle` returned in under a millisecond on a cold start and every later line raced the
work it was supposed to wait for"* — the same failure, for a different missing input.
`scenarios/daemon/first-run.scenario:14-17` (`await idle 20000` then
`assert daemon.link == connected`) is the line that pays for it, and §11 records no such gap.

- Not refuted. §2's wording is honest about what `idle` covers; the gap is that the corpus treats
  `await idle` as a cold-start barrier it was never defined to be.

---

### C4 — verdict: SURVIVES NARROWED

- **Confidence**: high on the mechanism, high on the narrowing

**Mechanism confirmed.** `project` copies the recorder (`drive.rs:595-598`:
`fleet_ui_kit::harness::frame(window)` / `painted(window)`), which `fleet-ui-kit/src/harness.rs:340`
writes only during `paint`. `Harness::paint` is called from exactly two places: the first-frame
bootstrap (`drive.rs:200-203`) and the headless arm of `settle` (`drive.rs:482-488`). `wait_for`'s
loop (`drive.rs:674-689`) calls `project` and `tokio::select!` and never `settle`/`paint`. So
headlessly `targets` and `window.frame` cannot move for the duration of an `await`, whereas on a
compositor a `cx.notify()` invalidates the window and a frame follows. The asymmetry is real.

**What narrows it.**

1. **It is already documented — in the code, next to the function.** `wait_for`'s doc comment
   (`drive.rs:640-648`) says verbatim: *"Two snapshot fields are not woken by this signal, and a
   scenario should reach for them with `dump` or `assert`, which project on demand: `window` and
   `targets` are copied in by `Harness::project` rather than by an update path…"*. Awaiting on
   `targets`/`window` is already told to authors as unsupported; the finding's real content is
   that the code doc's *"read as of the moment this call projects them"* is a misleading way to
   say it, and that §11 does not mirror the restriction. That is a doc-placement finding.
2. **The corpus obeys the restriction.** The only headless scenario that touches `targets` is
   `scenarios/hub/pointer-tabs.scenario:17`, and it uses `assert targets["hub.tab[0]"] exists &&
   targets["repos.rail"].w == 240` — an `assert`, which settles (and therefore paints) first
   (`drive.rs:372-379`). Every `await` in that file is over `hub_tab` or `idle`. The reviewer's
   own "No scenario hits it today" is correct.
3. **"Passes in `virtual`, fails headlessly" is an unlikely presentation.** The headless lane is
   selected mechanically by `crates/fleet-app/tests/harness_headless.rs:33-35` — a scenario is
   excluded only if it names `shot` or `clipboard` — and exactly three scenarios qualify today
   (`hub/idle-accounting`, `hub/pointer-tabs`, `daemon/link-recovers`). A new
   `await targets[…]`-style predicate would have to land in a scenario that also takes no
   screenshot before the divergence could be observed at all.

- **NARROWED to**: "`wait_for` never repaints, so headlessly `targets` and `window.frame` are
  frozen for the duration of an `await`; the restriction is stated only in `drive.rs`'s doc
  comment and is missing from §11, and that comment's phrasing (*'read as of the moment this call
  projects them'*) is true only where a frame loop exists." The part that does not hold is the
  severity framing — it is not an *undocumented* trap, no scenario can hit it today, and the
  lane-divergence presentation requires a screenshot-free scenario that awaits on a painted field.

---

## Verdict summary

| Candidate | Verdict |
| --- | --- |
| C1 (F2-1 + F1-11) | SURVIVES NARROWED |
| C2 (F1-10) | SURVIVES NARROWED |
| C3 (F1-12) | SURVIVES |
| C4 (F2-6) | SURVIVES NARROWED |

---### Note on test execution

`cargo test -p fleet-app harness` was run and **does not compile on `test-harness` HEAD (0f02991)**,
independently of anything in this review:

```
error[E0063]: missing field `reattached` in initializer of `state::connection::DaemonLink`
   --> crates/fleet-app/src/state/harness/tests.rs:440:20
```

`DaemonLink::Reconnected` gained a `reattached: usize` field
(`crates/fleet-app/src/state/connection.rs:43-48`, written at `:221`) and
`crates/fleet-app/src/state/harness/tests.rs:440` was not updated with it. Reproduced with
`cargo check -p fleet-app --lib --tests` on a tree whose only modifications are under
`crates/fleet-harness/`. This means `make test` cannot currently build `fleet-app`'s lib tests —
including `harness_headless.rs`'s corpus slice and every `idle` regression test cited above —
so the `idle` accounting on this branch is at present unverified by CI as well as by this review.
Out of scope to fix here, but it should be fixed before any of the findings above are actioned.

All findings in this document rest on static evidence — read paths, call-site enumeration
(`rg` over `crates/`), the frozen contract text, and the corpus itself. No claim depends on a
test run.
