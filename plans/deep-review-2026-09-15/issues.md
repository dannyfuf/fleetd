# Deep review — the harness review fixes (commits `0f02991..HEAD`, 2026-09-15)

## Scope

The eight commits that closed the 33 issues of the previous review (`plans/issues.md`,
I1–I33), on branch `test-harness` at `20ffb2f`, excluding `plans/`: 49 files, +4222/−617.
The ten commits beneath them were covered by the previous deep review; its artifacts are
committed under `plans/` and referenced by the trackers, which is why this pass lives in
`plans/deep-review-2026-09-15/` rather than overwriting `plans/finding_*.md` and `plans/issues.md`.

## Process

- Two independent reviewers (Codex `gpt-5.6-sol`, high effort, read-only sandbox, isolated
  contexts) produced `finding_1.md` (4 findings, R1-1…R1-4) and `finding_2.md` (8 findings,
  R2-1…R2-8).
- Deduplicated into 9 candidates (`candidates.json`); three pairs shared a failure mode
  (R1-1/R2-6, R1-2/R2-2, R1-3/R2-1).
- 18 validators, one prover and one refuter per candidate, run sequentially with one filtered
  `cargo test` each; verdicts are the `out-spec-dv-*.txt` files in the session scratchpad.
- Result: 9 survive (6 narrowed), 0 drop. All 18 verdicts report high confidence. Every
  surviving issue below carries reproduction or exact-path evidence from both validators.

## Surviving issues, by severity

### D1 — Mutation settlement is not causally tied to the mutation (P1)

- **Sources:** R1-1, R2-6
- **Verdict:** 2/2 survives narrowed, high confidence
- **Evidence:**
  - `crates/fleet-app/src/bridge/requests.rs:91-103,113-122` — the settle claim begins after
    the mutation reply and arms a 250 ms grace timer.
  - `crates/fleet-app/src/shell/root/events.rs:52-63` — every `SnapshotChanged`/`Connected`
    calls `settled()`; `state/harness.rs:258-262` clears *all* pending claims without correlation.
  - `crates/fleet-daemon/src/server/broadcast.rs:89-128` — a snapshot assembled before a newer
    mutation is published anyway; revision drift is checked only after publishing.
    `services/snapshots.rs:50-51,117,148-156` captures state before its awaits.
  - `crates/fleet-proto/src/snapshot.rs:105-125` — no revision or correlation field on the wire.
  - `crates/fleet-app/src/drive.rs:687-694` — `await` returns the moment the cleared idle
    projection satisfies the predicate.
  - Tests at `bridge/tests.rs:725-792` and `events.rs:405-425` pin the current behaviour
    (clear-all, timer release); none injects a mutation during snapshot assembly or a delayed
    delivery. A 30-run headless stress of `idle-after-context-mutation` did not reproduce stale
    UI; the interleaving is present in the code path but was not observed.
  - `plans/harness-review-fixes-2026-09-14-contracts.md:49-50` chose this design deliberately.
- **Problem:** `await idle` can be satisfied by an older coalesced snapshot that lands after the
  mutation's acknowledgement but before the mutation's own snapshot, or by the grace expiry when
  assembly plus delivery exceeds 250 ms. §2's "no mutation still awaiting *its* snapshot" is not
  what the code checks.
- **Narrowed:** the expiry is not unconditional — an intervening `settled()` bumps the generation
  and turns the timer into a no-op; it is time-only when no snapshot arrives at all. Some
  `Bridge::send` bodies produce no snapshot by design, and for those the grace is the only
  release. No stale end-to-end result was observed.
- **Fix:** carry a revision from the daemon: stamp each mutation reply with the snapshot
  revision it will produce (or the daemon's revision at the time of the reply) and add the
  revision to `SnapshotChanged`; release a settle claim only when the shell applies a snapshot
  whose revision is ≥ the claim's. Have the daemon discard a coalesced snapshot whose revision
  moved during assembly instead of publishing it. Keep the grace as a loud failure (journal it)
  rather than a silent release. This touches `fleet-proto` and needs the `rust-ipc-protocol`
  discipline (additive field, golden update).

### D2 — Teardown failures are invisible to the per-run report and delete the evidence (P2)

- **Sources:** R1-2, R2-2
- **Verdict:** 2/2 survives (refuter narrowed), high confidence
- **Evidence:**
  - `crates/fleet-harness/src/scenario.rs:350-377` — `outcome` is fixed before teardown problems
    are collected; `:392-406` journals `failed` and renders the report from `outcome` alone;
    `:414-420` removes `home/` on the success branch; `:426-429` only then turns the first
    teardown problem into an error.
  - `crates/fleet-harness/src/report.rs:268-309,409-433` — the verdict derives from failed steps
    or a `failed` journal event, so an unjournalled teardown problem renders `**Passed**`.
  - Reproduced by both validators with a Fleet wrapper exiting non-zero after an orderly run of a
    scenario with no `quit`: exit code 2, stdout `ok: 1 steps` then `Error: the scenario passed
    but teardown failed`, `report.md` `**Passed**`, no `failed` entry in `run.jsonl`, `home/`
    deleted.
  - `docs/TESTING-HARNESS.md:400,406-408` permits reclaiming `home/` only after a clean pass.
  - `scenario.rs:1603-1652` tests only the explicit-`quit` path; nothing tests teardown-only
    failure or artifact retention.
- **Problem:** a run that passes its lines and then fails in teardown (daemon, Fleet exit, injected
  job, display lane) exits non-zero but leaves a passing report, no failed journal entry, and a
  deleted home.
- **Narrowed:** a non-zero Fleet exit after an explicit `quit` is already handled (I2 fix), the
  suite report does go red (`scenario.rs:123-132`), and `--keep` preserves `home/`. All 42 corpus
  scenarios end in `quit`, so the corpus does not hit this; a one-off scenario without `quit`, or
  a lane/daemon teardown failure, does.
- **Fix:** fold teardown problems into `outcome` before journaling and rendering, keep `home/`
  when any problem exists, and print `ok` only when execution and teardown both succeeded. Add a
  test with a teardown-only failure asserting the `failed` event, the red report, and the kept home.

### D3 — A directory run does not claim its suite root (P2)

- **Sources:** R1-3, R2-1
- **Verdict:** 2/2 survives, high confidence, not narrowed
- **Evidence:**
  - `crates/fleet-harness/src/scenario.rs:107-123` — an explicit `--run-dir` is copied as the
    suite root and `<suite>/001-<stem>` is handed to `run_one`; the default suite root is created
    through `RunDirectory::create`, which (`rundir.rs:116-121`) also creates `shots/`, `dumps/`,
    `home/` at the suite level.
  - `rundir.rs:79-87,254-263` — children are claimed with non-recursive `create_dir`.
  - Reproduced: `fleet-harness run <dir> --lane headless --run-dir <absent>` exits 1 with
    `claim run directory …/001-bad: No such file or directory`; with a pre-created root holding a
    marker `report.md`, the marker is overwritten (`report.rs:131-134` truncates without an
    ownership check); a default run leaves `home/`, `shots/`, `dumps/` beside `001-*/`.
  - `docs/TESTING-HARNESS.md:387-390,410-418` requires the suite root to follow the atomic-claim
    rule and documents only `report.md` plus child run directories.
  - No test covers the suite-root lifecycle (`scenario.rs:1314-1337` tests collection only;
    suite-report tests pre-create their roots).
- **Problem:** the documented explicit-directory run fails for a new destination, a pre-created
  root bypasses the collision protection that I14 added for single runs, and default suites leave
  an undocumented single-run layout at the root.
- **Fix:** add a suite-root creator that atomically claims the explicit or suffixed default root
  (same `create_dir` + `-2`, `-3` policy) without populating single-run artifacts, and use it on
  both branches; test absent, pre-existing and default roots.

### D4 — `daemon restart` can still hand ownership to a daemon Fleet started (P2)

- **Sources:** R2-3
- **Verdict:** 2/2 survives, high confidence, not narrowed
- **Evidence:**
  - `crates/fleet-harness/src/fault.rs:239-255` — the adopted replacement is a shell gated on
    stdin when the seal is removed; readiness accepts any daemon answering from the home
    (`env.rs:291-319`, no PID comparison with `Daemon::pid()`).
  - `crates/fleet-app/src/bridge/runtime.rs:168-198`, `bridge/connection.rs:115-120`,
    `crates/fleet-client/src/spawn.rs:68-129` — Fleet's reconnect path calls `ensure_daemon`,
    which spawns a detached fleetd when no live singleton is observed.
  - `crates/fleet-daemon/src/server/listener.rs:88-100,146-169` — the singleton lock goes to the
    first process; the loser exits.
  - Reproduced with a scratch test (`c5_competing_ensure_daemon_can_win`): `adopted_pid=524411
    recorded_pid=524413 adopted_status=exit status: 1 readiness=true`.
  - `env.rs:388-423` — unwind-time `Drop` kills only the adopted child and unlinks the socket, so
    Fleet's winner survives without its socket. `fault.rs:688-721` tests the sequential path only.
- **Problem:** the I15 fix holds the seal across the spawn, but the gate is released after the
  seal is lifted, so Fleet's auto-spawn can win the singleton; readiness then succeeds against a
  daemon the runner does not own, and an abrupt runner exit orphans it.
- **Fix:** keep Fleet from racing during the hand-off (suspend it, or hold the seal until the
  adopted child has acquired the lock), then verify the PID file names the adopted child before
  reporting readiness; re-seal and resume on every error path; add the competing-spawn test.

### D5 — Virtual-lane geometry pinning is Lua-only (P2, narrowed)

- **Sources:** R2-4
- **Verdict:** 2/2 survives narrowed, high confidence
- **Evidence:**
  - `crates/fleet-harness/src/lane.rs:344-363` calls `pin_geometry` during isolated-lane
    construction; `:575-585` always uses `hyprctl eval 'hl.monitor(...)'`; `:950-967` checks only
    the exit status; provider detection (`:977-985`) runs later and only for window dispatch.
  - Live probe on a `hyprlang`-configured 0.56.2 instance: the eval returns
    `eval is only supported with the lua config manager` with exit 0, ignored.
  - Aquamarine 0.14 creates headless outputs at 1920×1080@60 scale 1 by default, which is the
    pinned mode, so the refused eval is masked on default configurations.
  - `docs/TESTING-HARNESS.md:289-294` promises isolation under both providers.
- **Problem:** on a legacy-provider session whose monitor rules give a new output a different
  mode or scale, the lane cannot pin geometry, times out, and falls back to headless; pixel
  scenarios then fail and `--update-baselines` records nothing.
- **Narrowed:** a legacy provider alone does not fall back; the failure needs a non-default
  matching or default monitor rule. Not reproduced on this box.
- **Fix:** detect the provider before pinning; use `hyprctl keyword monitor …` for legacy and
  `eval hl.monitor(...)` for Lua; treat a textual refusal as an error; test both branches.

### D6 — `--update-baselines` in the attach lane skips the capture (P2, narrowed)

- **Sources:** R2-5
- **Verdict:** 2/2 survives (refuter narrowed), high confidence
- **Evidence:**
  - `crates/fleet-harness/src/scenario.rs:867-875` substitutes `command_path` for
    `capture_command` whenever updating outside the virtual lane, attach included;
    `capture.rs:161-189` is the only path that writes and verifies the PNG; `lane.rs:806-808`
    shows attach has a working capture path.
  - `baseline.rs:354-360` returns a passing `NotRecorded` before reading the shot;
    `scenario.rs:916-925` sets an artifact only if the file exists.
  - `docs/TESTING-HARNESS.md:35` defines `shot` as runner-captured pixels; `:433-435` forbids
    only writing a baseline outside the virtual lane.
  - Tests miss the combination: `scenario.rs:1739-1764` hard-codes a headless outcome,
    `baseline.rs:1908-1931` pre-writes the PNG.
- **Problem:** an attach-lane update run (explicit, or a virtual run that fell back to attach)
  reports `not recorded` and succeeds with no screenshot written.
- **Narrowed:** the report's Lines row and the journal do carry the `not-recorded` verdict, so
  "no report evidence" overstated it; the missing PNG and screenshot section are real.
- **Fix:** skip capture only for headless updates; capture normally in attach and let the
  baseline check return `NotRecorded` without writing a baseline; test attach + update.

### D7 — A followed Jobs log keeps `in_flight_requests` above zero (P2, narrowed)

- **Sources:** R2-7
- **Verdict:** 2/2 survives narrowed, high confidence
- **Evidence:**
  - `crates/fleet-app/src/screens/jobs/log_follow.rs:59-100` — each poll's `reply` receiver
    stays in scope across the 250 ms `TAIL_INTERVAL` timer; `bridge/requests.rs:165-172` holds
    the `InFlight` claim until `reply.closed()`; `bridge.rs:544-560` claims the next request
    synchronously before returning.
  - Reproduced with a scratch headless scenario: `await idle 1500` after expanding a log times
    out with `in_flight_requests=1` and every other constituent clear
    (`/tmp/fleet-harness/20260915-192639-repro/run.jsonl:9`).
  - `scenarios/hub/jobs-panel.scenario:30-37` works around it by awaiting idle only after the
    collapse (the close-out agent removed the `await idle` that had been there).
  - `log_follow.rs:29-41` — `still_following` ignores `PanelState::following`, so pausing the
    follow does not stop the polling either.
- **Problem:** the I3 reply-lane hold turns a busy-polling loop into a permanently non-idle app:
  with a log expanded, no harness-observable idle edge exists and every `await idle` times out.
- **Narrowed:** the atomic may transiently reach zero between iterations, but no foreground
  observer can see it before the next synchronous claim; the observable claim is the timeout.
- **Fix:** drop the reply receiver as soon as its answer is applied (before the timer), so the
  loop is idle between polls; honour `following` in `still_following`; add a paused-clock test
  that observes zero in-flight requests between polls.

### D8 — A post-response `shot` failure loses the app exchange from the journal (P3, narrowed)

- **Sources:** R1-4
- **Verdict:** 2/2 survives narrowed, high confidence
- **Evidence:**
  - `crates/fleet-harness/src/scenario.rs:861` receives the successful `shot` response;
    `:870-882` propagates capture, decode, size, baseline I/O and diff-write errors with `?`;
    `:594-624` journals an `Err` as a runner `error` without the exchange.
  - Reproduced headless: the journal holds only `fixture`, an `error`, and `failed`; no
    `command` entry and no geometry.
  - `docs/TESTING-HARNESS.md:484-490` requires a complete journal with every exchange.
  - `scenario.rs:1655-1735` covers `Differed` only.
- **Problem:** exactly when a capture or baseline infrastructure failure needs diagnosing, the
  settled geometry and the correlated app exchange are missing from `run.jsonl`.
- **Narrowed:** ordinary pixel differences are preserved by `finish_shot` (the I24 fix); the loss
  is limited to runner-side `Err` paths after a successful response, and the run still fails
  visibly.
- **Fix:** return a structured shot outcome carrying the original response plus the runner-side
  failure, record the exchange, then mark the step failed; test a capture error after a successful
  response.

### D9 — `lists.jobs.filter` is always empty (P3, narrowed)

- **Sources:** R2-8
- **Verdict:** 2/2 survives narrowed, high confidence
- **Evidence:**
  - `crates/fleet-app/src/state/harness/projection.rs:443-449` passes `String::new()` while
    `:596-617` filters rows with `jobs_panel.filter`; `state/jobs_filter.rs:30-37` already has
    stable `running`/`failed` labels; `docs/TESTING-HARNESS.md:204` includes `filter:string` in
    every list snapshot.
  - `projection.rs:926-970` and `scenarios/hub/jobs-panel.scenario:41-44` never assert the field.
- **Problem:** a scenario cannot read which Jobs filter is active.
- **Narrowed:** the claimed ambiguity between an empty filtered list and an empty registry is
  false — top-level `jobs[]` exposes the registry (`projection.rs:265-274`), and the filter can be
  inferred from known jobs and the deterministic `f` cycle.
- **Fix:** project `filter.label().unwrap_or_default()` and assert it in the filter test and in
  `jobs-panel.scenario` after `f`.

## Dropped or narrowed

Nothing was dropped. Narrowings, all applied above:

- **C1 / D1:** "expiry is unconditional" struck — an intervening `settled()` invalidates the
  timer; the defect is the missing causal link, reachable via an older coalesced snapshot or a
  grace with no snapshot at all. Stale end-to-end state was not observed in 30 runs.
- **C2 / D2:** explicit-`quit` failures, suite-level red, and `--keep` are already handled; the
  defect is teardown-only failure after a passing run.
- **C4 / D8:** `Differed` is preserved; only post-response `Err` paths lose the exchange.
- **C6 / D5:** a legacy provider alone does not fall back; a non-default monitor rule is needed.
- **C7 / D6:** "no report evidence" struck — the verdict line survives; the PNG does not.
- **C8 / D7:** "never reaches zero" struck — no observable zero edge is the reachable claim.
- **C9 / D9:** "indistinguishable from an empty registry" struck — `jobs[]` disambiguates.

## Notes on coverage

The reviewers did not report, and the validators did not check, anything about the four
edited scenarios beyond D9, the `fleet-drive` input intersection change (commit `c472173`), or
the PNG ceiling arithmetic (I20/I21) beyond what the previous review already validated. D1 is the
only finding that questions a design decision recorded in the contract rather than an
implementation slip; fixing it properly needs a wire change.
