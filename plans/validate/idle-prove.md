# Adversarial validation — batch `idle`

Repo `/home/df/.fleet/worktrees/dannyfuf/fleetd/test-harness`, branch `test-harness`, base
`881162c5`. Every repro below was run against the tree as committed (`target/debug/{fleet,fleetd,fleet-harness}`,
built 2026-09-14 04:00–04:04) with scenario files kept outside the repo, in the session scratchpad
`…/scratchpad/repro/`. No repository file was modified.

---

### C1 — verdict: SURVIVES NARROWED

The two source findings read the same code in opposite directions. **F2-1 is real and reproducible;
F1-11 is not reachable, and its proposed fix would widen F2-1.** The surviving claim is F2-1's.

- **Confidence**: high (F2-1 proven by repro, 3/3, with a passing control); high (F1-11 refuted).

- **Reasoning — F2-1, the claim is released too early.**

  1. `Bridge::send` / `Bridge::request` take the claim on the UI thread, synchronously, before the
     command is queued (`crates/fleet-app/src/bridge.rs:461`, `crates/fleet-app/src/bridge.rs:509`).
  2. The claim rides the command to the runtime thread and is released by `Drop`
     (`crates/fleet-app/src/bridge.rs:344`) at the point the *daemon's response* resolves —
     `crates/fleet-app/src/bridge/requests.rs:84` for the fire-and-forget lane (`_in_flight` is bound
     at `requests.rs:80` and dropped at the end of that loop body, `requests.rs:90`), and
     `crates/fleet-app/src/bridge/requests.rs:125`/`133` for the reply lane.
  3. The daemon's response to a mutation is an `Ack` written immediately. The *state change* it
     caused is published on a **deliberate delay**:
     `crates/fleet-daemon/src/server/broadcast.rs:13`
     `const SNAPSHOT_COALESCE_WINDOW: Duration = Duration::from_millis(50);`
     and `broadcast.rs:113-118` spawns `sleep(SNAPSHOT_COALESCE_WINDOW)` → `services.snapshot()` →
     `events.publish(Event::SnapshotChanged(...))`. So the `SnapshotChanged` a mutation produces is at
     least 50 ms behind the `Ack` that released the claim.
  4. `IdleSnapshot::new` (`crates/fleet-app/src/state/harness.rs:184-203`) derives `idle` from five
     counters, none of which counts "a bridge event has arrived and is not yet applied". So for
     ≥50 ms after every daemon mutation, all five counters read zero and `idle` is **true** while the
     app still holds pre-mutation state.
  5. `Command::Await` (`crates/fleet-app/src/drive.rs:365-370`) settles once and then calls
     `wait_for`, whose **first** projection is unconditional (`drive.rs:675`). That first projection
     lands inside the 50 ms hole, so the `await` returns immediately and the next scenario line reads
     the old state.

  This is not an interleaving race. The 50 ms coalescer makes it deterministic.

- **Evidence — repro (3/3 failures, control 2/2 passes).**

  `…/scratchpad/repro/c1-context-create.scenario` — the same sequence
  `scenarios/hub/contexts.scenario:12-24` uses, but waiting with `await idle` instead of with a
  content predicate:

  ```
  fixture: busy
  await idle 30000
  assert lists.repos.rows[1].label == "acme/api"
  key N
  await overlay == Dialog 10000
  type Beta
  key enter                 # create the context — a daemon mutation
  await overlay absent 20000
  await idle 15000          # "no pending work"
  key 2                     # switch to the context that was just created
  await idle 10000
  assert lists.repos.rows[1] absent
  ```

  Run three times, `--lane headless`, three identical failures:

  ```
  line 15: assert lists.repos.rows[1] absent
    error: `lists.repos.rows[1] absent` failed; lists.repos.rows[1] is
           {"id":"acme/api","label":"acme/api","badges":[],"marks":[]}
  ```

  The run journal (`/tmp/fleet-harness/20260914-072630-c1-context-create/run.jsonl`) times it:

  ```
  07:26:31.184Z key    ["enter"]                      # the mutation
  07:26:31.186Z await  overlay absent    -> satisfied #  +2 ms
  07:26:31.188Z await  idle 15000        -> satisfied #  +4 ms, in_flight_requests: 0,
                                                      #  running_jobs: 0, pending_frame: false,
                                                      #  live_toast_timers: 0, armed_debounces: 0
  07:26:31.189Z key    ["2"]                          # acts on a context the app has not been told about
  07:26:31.191Z await  idle 10000        -> satisfied
  07:26:31.193Z assert lists.repos.rows[1] absent -> FAILED
  ```

  `await idle` reported total quiescence **4 ms** after the mutating keystroke — an order of
  magnitude inside the daemon's own 50 ms publish delay.

  Control (`…/scratchpad/repro/c1-control2.scenario`): byte-identical except that
  `await idle 15000` is replaced by a literal `hover worktrees.row[0] 1000` dwell. **Passes 2/2,
  10/10 lines.** The only variable is the wait primitive.

- **Reasoning — F1-11, the claim is released too late: DROPS.**

  F1-11 says the foreground can wake on `reply.send(result)` (`requests.rs:133`), apply, notify and
  re-project before the tokio task drops `_in_flight` a few instructions later. Three reasons it does
  not stand:

  1. The window is the remainder of one async block after an `async_channel` send returns — an `Arc`
     clone drop and a `fetch_sub`. To win it, a *parked foreground thread on another core* must be
     unparked by the OS, scheduled, run a `state.update`, `cx.notify()` and a full
     `Harness::project` — micro- to milliseconds — inside that. It requires the tokio thread to be
     preempted at exactly that instruction.
  2. The repro above measures the opposite sign directly: 4 ms after the mutation the claim was
     already gone (`in_flight_requests: 0`) while the app state was demonstrably not updated. The
     observed ordering is *release, then apply*, which is F2-1.
  3. F1-11's fix — `drop(in_flight)` before `reply.send(...)` — moves the release *earlier*, which
     makes the proven 50 ms hole strictly wider. The two findings are not both true; keeping F1-11
     would be a regression.

- **The narrowed claim that holds**: the `InFlight` claim is released when the daemon's *response*
  resolves on the runtime thread, not when the application has applied the resulting state, so `idle`
  reports quiescence during the daemon's `SNAPSHOT_COALESCE_WINDOW` (≥50 ms) and for the whole
  foreground hop after it. F2-1's fix direction (hold the claim until the app has consumed the
  answer, or add a sixth `pending_events` counter fed by the shell's drain loop) is the right one.
  F1-11 is dropped.

- **Would a test catch it today?** No. No `#[gpui::test]` exercises `drive::wait_for` (it is a private
  method on a `pub(crate)` `Harness`), and the scenario corpus *works around* the defect rather than
  exposing it: of the 68 `await idle` lines, every one that directly follows a key is an app-local
  key (`i`, `tab`, `H`, `escape`, `q` — `rail_collapsed` and the detail toggle are plain `AppState`
  fields, `crates/fleet-app/src/shell/root/routing.rs:76,93`). Every real daemon mutation is waited on
  with a content predicate instead: `scenarios/board/move-card.scenario:25`,
  `scenarios/hub/contexts.scenario:24`, `scenarios/workspace/session-open.scenario:13`. One scenario
  even states the reason in prose — `scenarios/agents/prefix-inside-a-thread.scenario:16-17`:
  *"awaiting `idle` alone would hold before the turn had even started and leave every line below
  racing a stream."* That is this finding, discovered empirically and worked around in a comment.

---

### C2 — verdict: SURVIVES NARROWED

- **Confidence**: medium-high on the mechanism (proven by reading, unconditional); the two *cited*
  early-return paths do **not** produce the hang, and I could not make them.

- **Reasoning — what is certainly true.** Every claim F1-10 makes about the wiring checks out:

  - `ArmedDebounce::disarm` / `Drop` — `crates/fleet-app/src/state/harness.rs:253-265` — a bare
    `fetch_sub`, no `cx.notify()`.
  - `HarnessState::finish_request` — `harness.rs:330-336` — a bare `fetch_update`, no notify.
  - `InFlight::Drop` — `crates/fleet-app/src/bridge.rs:344-349` — a bare `fetch_sub`, and it runs on
    the bridge's tokio thread, where there is no `cx` at all. This one *cannot* notify without a new
    seam.
  - `drive::Harness::wait_for` — `crates/fleet-app/src/drive.rs:665-688` — is woken solely by
    `cx.observe(state, …)` and `tokio::select!`s on that receiver and the expiry timer. There is no
    poll, by design, and the doc comment says so.
  - The only fallback, `Shell::spawn_ticker` (`crates/fleet-app/src/shell/root/events.rs:177-190`),
    notifies only when `AppState::tick` returns true, and `tick`
    (`crates/fleet-app/src/state/connection.rs:116-139`) returns `false` on a `Connected` app with no
    toasts and no running visible watch. On the regime every scenario spends its life in, there is no
    250 ms safety net.

  So: **no counter's busy→idle edge raises the signal `await idle` listens for.** A decrement is
  visible only when something unrelated calls `cx.notify()` afterwards.

- **Reasoning — why the cited paths do not hang, and what does.**

  `crates/fleet-app/src/screens/hub/cache.rs:570-590` and
  `crates/fleet-app/src/dialogs/clone_repo.rs:281-292` both disarm and then early-return with no
  notify, exactly as F1-10 says. But the early returns are unreachable as the cause of a hang:

  - The stale-generation / stale-seq branches are defensive. A superseded debounce's `Task` is
    *dropped*, and gpui's `Task` cancels on drop — `crates/scheduler/src/executor.rs:377-380`:
    *"If you drop a task it will be cancelled immediately."* The stale task never reaches its own
    check; its guard is released from the update path that then notifies.
  - The daemon-lost branches (`inspect`'s `daemon.is_lost()` guard, `cache.rs:276`;
    `inspection_target`'s `is_connected()` guard, `cache.rs:537`) are rescued by the ticker: in
    `Lost` and `Starting`, `tick` returns `true` unconditionally (`connection.rs:135`), so the app
    notifies every 250 ms.
  - Measured: `…/scratchpad/repro/c2-debounce.scenario` (`key j` / `await idle`, three times) — each
    `await idle` took **~421 ms**, i.e. it waited out the full 400 ms `AUTO_INSPECT_DEBOUNCE`
    (`crates/fleet-app/src/screens/hub.rs:77`) plus the inspect round trip, then returned with all
    counters zero. The debounce accounting works end to end on the live path, because `inspect`
    notifies at `cache.rs:284`.
  - Measured: `…/scratchpad/repro/c2-disarm.scenario` (`key j`, `key h`, `await idle 3000`) — the
    disarm-to-zero was silent, but `wait_for`'s **first projection is unconditional** (`drive.rs:675`)
    and reads the atomics live, so the await returned in 2 ms with `armed_debounces: 0`. A silent
    decrement that happens *before* the await starts costs nothing.

  What remains, and is genuinely unprotected, is the `InFlight` release: it happens on a thread that
  has no way to notify. After it, the only rescues are (a) the coalesced `SnapshotChanged` ~50 ms
  later — which is C1's defect, not a guarantee — or (b) the reply continuation's own notify. Reply
  continuations that early-return without notifying exist:
  `HubCtx::apply_slices` returns at `crates/fleet-app/src/screens/hub/cache.rs:338` (`current_key != key`,
  and `PrCacheKey::from_state` keys on scope, context and `link_generation`, `cache.rs:13-26`) and at
  `cache.rs:346` (`!applied`), having consumed two claims (`fetch_pull_requests` issues two requests,
  `cache.rs:322-333`). On a `Connected`, toast-free app whose scope changed while two `gh` fetches
  were outstanding, both claims fall to zero silently and nothing is scheduled to notify.

- **The exact reduced claim that holds**: no `idle` counter's busy→idle edge notifies `AppState`, and
  `wait_for` has no fallback poll, so a decrement that lands *strictly inside* a running `await`
  window is invisible until an unrelated `cx.notify()`. The dangerous decrement is `InFlight::Drop`
  (`bridge.rs:344`), which executes on the bridge's tokio thread and structurally cannot notify; the
  two debounce sites F1-10 cites are protected by gpui's cancel-on-drop `Task` and by the ticker's
  unconditional `true` in `Starting`/`Lost`, and do not produce the hang. The fix F1-10 proposes —
  make the decrement itself the signal, or give `wait_for` a cheap safety poll — is still the right
  one, and the safety poll is the only one that covers the cross-thread release.

- **Would a test catch it today?** No. `crates/fleet-app/src/state/harness/tests.rs:142-157` drives
  the counters directly on a bare `AppState` and asserts the *projection* flips — it never involves
  `cx.observe`, so it cannot see a missing notification. Nothing tests `wait_for`.

---

### C3 — verdict: SURVIVES NARROWED

- **Confidence**: high on the mechanism; the cold-start manifestation does **not** reproduce today,
  and F1-12's own explanation of why is wrong.

- **Reasoning.**

  1. `IdleSnapshot::new` (`crates/fleet-app/src/state/harness.rs:184-203`) takes five inputs.
     `DaemonLink` is not one of them, and `crates/fleet-app/src/state/harness.rs:162-175` documents
     the five as the whole definition. `docs/TESTING-HARNESS.md` §2 says the same.
  2. On a Fleet whose bridge is still opening: `in_flight_requests` is 0 (the bridge's own
     `ensure_daemon` never goes through `Bridge::send`/`request`, so it takes no claim);
     `running_jobs` is 0 because there is no snapshot; `pending_frame` is false (it is written only by
     the key-queue path, `crates/fleet-app/src/shell/root/focus.rs:205` and `:239`, and both writers
     `cx.notify()`); `live_toast_timers` and `armed_debounces` are 0. **`idle` is therefore true, by
     construction, on a Fleet that has not attached.**
  3. F1-12 says "it happens not to today only because the Hub's deferred `synchronize` admits a
     request before the socket is connectable". That is not the reason: `HubCtx::synchronize`
     (`crates/fleet-app/src/screens/hub/cache.rs:495-533`) issues **no** request while the link is
     `Starting` — `tick_pull_requests` bails on `!state.daemon.is_connected()` (`cache.rs:419-426`)
     and `inspection_target` returns `None` for the same reason (`cache.rs:537-539`). Nothing arms
     anything at cold start. The finding's conclusion is right for a reason stronger than the one it
     gives.
  4. The real reason it does not fire is the runner's margin. `Daemon::start`
     (`crates/fleet-harness/src/env.rs:194-205`) does not return until `fleetd` answers
     `daemon_ping`, so the daemon is warm before Fleet is even spawned; `Bridge::start` runs inside
     `cx.open_window` (`crates/fleet-app/src/shell/root.rs:78`, reached from
     `crates/fleet-app/src/shell/root/bootstrap.rs:74-79`) while the harness socket is not created
     until the window task runs `server::listen` (`crates/fleet-app/src/drive.rs:186`); and the runner
     then probes that socket every 50 ms (`crates/fleet-harness/src/scenario.rs:35,665`). Measured
     across four of my runs, the gap from fixture-applied to the first command being answered was
     412–571 ms, against a warm-socket `Hello`/`Subscribe`/`GetSnapshot` that costs well under a
     millisecond.
  5. What the gap still costs is a *silent green*, and the corpus is exposed to it. Every
     `assert … absent` that follows a first `await idle` passes vacuously on an unattached Fleet —
     `scenarios/daemon/first-run.scenario:15`
     (`assert lists.worktrees.rows[0] absent && lists.prs.rows[0] absent && lists.jobs.rows[0] absent`)
     is exactly that shape, and `:16` (`assert daemon.link == connected`) is the only line in the
     whole corpus that would turn it red. Worse, the scenario written to *guard* `idle` does not:
     `scenarios/hub/idle-accounting.scenario:14-16` asserts all five counters are zero and says
     nothing about the link — it would pass on a Fleet that has not attached. That file's own header
     (`:4-7`) records the last time this failed for real: *"with `in_flight_requests`, `pending_frame`
     and `armed_debounces` structurally zero, `await idle` returned in under a millisecond on a cold
     start and every later line raced the work it was supposed to wait for."* The link is the same
     class of input, still missing.

- **The exact reduced claim that holds**: `idle` is structurally true on a Fleet that has not
  attached to its daemon, so `await idle` carries no guarantee that the link is up; the corpus's
  reliance on it is protected only by a ~400 ms runner margin against a pre-warmed daemon, and
  nothing in the code or the corpus pins that ordering. The failure is a silent green, not a red.
  Both fixes F1-12 offers remain valid; folding the link into the derivation is the one that closes
  it rather than documenting it.

- **Would a test catch it today?** No, and the scenario that exists to catch exactly this class does
  not assert on the link.

---

### C4 — verdict: SURVIVES NARROWED

- **Confidence**: high on the freeze (proven by reading, unconditional); low on any scenario reaching
  it today — F2-6 already says none does, and I could not construct one from the frozen grammar.

- **Reasoning.**

  1. `Harness::project` (`crates/fleet-app/src/drive.rs:589-626`) *copies*
     `fleet_ui_kit::harness::frame(window)` and `fleet_ui_kit::harness::painted(window)` into
     `AppState`. It never paints.
  2. Both of those read a thread-local table that is advanced only by
     `fleet_ui_kit::harness::begin_frame` (`crates/fleet-ui-kit/src/harness.rs:122-129`), which
     `AppFrame` calls from a real paint. `frame()` is `harness.rs:133-137`.
  3. `wait_for`'s loop (`drive.rs:674-688`) is `project` → `select!{ expiry, changed_rx }`. No paint.
  4. The only headless painter is `Harness::paint` (`drive.rs:514-516`, `window.draw(cx)`), called
     from `Harness::settle` (`drive.rs:481-486`) and once at `drive.rs:200-203` before the command
     loop. `Command::Await` settles *before* `wait_for` (`drive.rs:367`) and never again.
  5. Therefore, in the headless lane, `targets` and `window.frame` are frozen for the entire duration
     of an `await` at whatever the pre-loop settle painted. In a compositor lane a `cx.notify()`
     invalidates the window and a frame follows, so the same predicate refreshes. A predicate over
     `targets[…]` or `window.frame` that first becomes true *after* that settle can never be
     satisfied headlessly and times out, while passing in `virtual`.
  6. `window.bounds`, `window.scale_factor` and `window.title` are **not** frozen — `project` reads
     `window.bounds()` and `window.scale_factor()` live at `drive.rs:612-619`. F2-6's "`targets` and
     `window` are frozen" over-reaches; the frozen fields are `targets` and `window.frame`.
  7. The trap is armed rather than sprung. `docs/TESTING-HARNESS.md` §2 (line 217) explicitly blesses
     `targets["worktrees.row[0]"].frame` as a predicate path, line 102 blesses
     `targets[*].frame == window.frame`, and §9.7 tells authors to run every scenario in both lanes.
     §11 (Known gaps) does not mention this. `wait_for`'s own doc comment
     (`drive.rs:640-642`) asserts the opposite of the truth for this lane: *"`window` and `targets`
     are copied in by `Harness::project` rather than by an update path, so they are read as of the
     moment this call projects them."*
  8. Today's headless subset is three files — `scenarios/daemon/link-recovers.scenario`,
     `scenarios/hub/pointer-tabs.scenario`, `scenarios/hub/idle-accounting.scenario` (the shot/clipboard
     rule at `Makefile:31` and `crates/fleet-app/tests/harness_headless.rs:35`). The only `targets`
     use among them is `pointer-tabs.scenario:13`, which is an `assert` — and `Command::Assert`
     settles first (`drive.rs:372`), so it paints and is safe. No `await` in the corpus reads
     `targets` or `window.frame`.

- **Repro attempts (all negative, and why).** Three headless scenarios were built to make a `targets`
  predicate flip inside an await: a job row arriving from the daemon
  (`…/scratchpad/repro/c4-targets.scenario`, `c4-b.scenario`) and a rail row disappearing on a context
  switch (`c4-c.scenario`). All three passed in 2–52 ms, because the predicate was already satisfied
  at `wait_for`'s first projection: `Job*` events reach Fleet in under 5 ms, and the context switch
  itself is applied locally, so each command's own `settle()` had already painted the result before
  the loop began. Reaching the gap needs a change that lands strictly *after* the await's settle —
  which the daemon's 50 ms snapshot coalescer does supply in principle (see C1), but I could not
  attach it to a painted target using only the frozen grammar. Static evidence stands; no evidence was
  manufactured.

- **The exact reduced claim that holds**: in the headless lane `wait_for` never paints, so `targets`
  and `window.frame` (not the rest of `window`) are frozen at the await's own settle for the whole
  await; a predicate over them that first becomes true during the await cannot be satisfied
  headlessly. It is a latent lane-divergence trap that no current scenario reaches. F2-6's fix —
  `self.paint(cx)` at the top of each `wait_for` iteration when `self.headless`, or a §11 entry —
  is correct; the doc comment at `drive.rs:640-642` is wrong today either way and should be fixed
  with it.

---

## Repro index

All files under the session scratchpad, none in the repo:

| file | purpose | result |
| --- | --- | --- |
| `repro/c1-context-create.scenario` | `await idle` after a real daemon mutation | **fails 3/3** |
| `repro/c1-control2.scenario` | same, with a 1 s dwell instead | passes 2/2 |
| `repro/c1-board-move.scenario` | `await idle` after `]` (board move is applied optimistically) | passes — not a valid probe |
| `repro/c1-idle-early.scenario` | `await idle` after `o` (screen switch is app-local) | passes — not a valid probe |
| `repro/c2-debounce.scenario` | does `await idle` wait out the 400 ms auto-inspect debounce | yes, ~421 ms each |
| `repro/c2-disarm.scenario` | silent disarm before the await starts | harmless (first projection reads it) |
| `repro/c4-targets/-b/-c.scenario` | `targets` predicate flipping inside a headless await | negative, see C4 |
