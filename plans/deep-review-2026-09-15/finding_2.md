### R2-1 | P2 | crates/fleet-harness/src/scenario.rs:107
- Problem: A directory run uses an explicit `--run-dir` without claiming or creating it, while its first scenario tries to atomically create a child beneath it. The default path instead creates a full single-run layout as the suite root, leaving stray `home/`, `shots/`, and `dumps/` directories.
- Impact: An absent explicit suite directory fails on the first scenario with `NotFound`; precreating it bypasses collision protection, while default suites permanently retain an unused hermetic home contrary to the documented layout.
- Fix: Add a dedicated suite-root creator that atomically claims the explicit or suffixed default root without populating single-run artifacts, and use it for both branches.
- Evidence: `scenario.rs:107-123` passes `<suite>/001-*` to `run_one`; `rundir.rs:254-263` uses `create_dir` and cannot create a missing parent; `rundir.rs:116-121` populates the unwanted single-run directories; `docs/TESTING-HARNESS.md:387-390` requires suite roots to follow the same atomic-claim rule.

### R2-2 | P1 | crates/fleet-harness/src/scenario.rs:377
- Problem: Teardown problems are collected after execution but are not folded into `outcome` before the failure journal and report are written. The success branch then deletes `home/` and prints `ok` before returning the teardown error.
- Impact: A run whose daemon, Fleet process, injected job, or display lane fails during teardown exits nonzero but has a passing report, no `failed` journal entry, and has already destroyed its failure-state home.
- Fix: Convert any teardown problem into the final failed outcome before journaling or reporting; preserve `home/`, write the failed verdict, and print success only when both execution and teardown succeeded.
- Evidence: Teardown failures originate at `scenario.rs:186-252`; only the pre-teardown `outcome` is journalled at `scenario.rs:392-402` and reported at `scenario.rs:406`; `home/` is removed at `scenario.rs:419` before `problems.first()` is checked at `scenario.rs:426-429`. The preservation contract is `docs/TESTING-HARNESS.md:406-408`.

### R2-3 | P2 | crates/fleet-harness/src/fault.rs:239
- Problem: The adopted replacement is still only a shell blocked on stdin when the PID seal is removed; Fleet and that shell can therefore race to start/acquire the singleton before the gate is released.
- Impact: If Fleet wins, readiness succeeds against an unowned daemon while the adopted child exits; an abrupt runner exit then kills only the dead shell and can leave the real daemon running while unlinking its socket.
- Fix: Suspend Fleet during the handoff, unseal and release the adopted replacement, verify that the PID file and daemon identity match the adopted child, then resume Fleet; restore the seal and resume Fleet on every error path.
- Evidence: `fault.rs:242` adopts the gated shell, `fault.rs:243` unseals, and only `fault.rs:245` releases it; `fault.rs:416-423` confirms Fleet independently auto-starts daemons. `env.rs:294-319` considers any daemon answering from the home ready and does not compare its PID with `process.pid()`.

### R2-4 | P2 | crates/fleet-harness/src/lane.rs:579
- Problem: Virtual-output geometry is always configured through the Lua-only `hyprctl eval hl.monitor(...)` path before the newly added config-provider detection runs.
- Impact: On a `hyprlang`/legacy-configured Hyprland session, the default virtual lane cannot pin its output and falls back to headless, so pixel scenarios fail or baseline-update runs record no images despite the documented support for both providers.
- Fix: Detect the provider before pinning geometry and use `hyprctl keyword monitor ...` for `Legacy` and `eval hl.monitor(...)` for `Lua`, validating each command’s textual response and resulting monitor geometry.
- Evidence: `lane.rs:362` calls `pin_geometry` during isolated-lane construction; `lane.rs:579-585` unconditionally uses Lua syntax; `lane.rs:981-984` recognizes `hyprlang` as legacy, but that detection is first called later at `lane.rs:628`. The dual-provider guarantee is `docs/TESTING-HARNESS.md:289-294`.

### R2-5 | P2 | crates/fleet-harness/src/scenario.rs:867
- Problem: With `--update-baselines`, every non-virtual lane skips screenshot capture entirely, even though only the headless lane lacks pixels.
- Impact: An attach-lane `shot` succeeds and journals `not-recorded` against a nonexistent PNG, leaving no screenshot artifact or report evidence although the command contract says the runner captures pixels.
- Fix: Skip capture only for headless updates; capture normally in attach mode, then let baseline checking return `NotRecorded` without writing a baseline.
- Evidence: `scenario.rs:867-875` substitutes `command_path` for `capture_command` on both Headless and Attach; `scenario.rs:919-920` records an artifact only when that file exists. Attach capture is implemented at `lane.rs:806-808`, and `docs/TESTING-HARNESS.md:35` defines `shot` as runner-captured pixels.

### R2-6 | P1 | crates/fleet-app/src/shell/root/events.rs:52
- Problem: Mutation settlement is not causally tied to the mutation: any `SnapshotChanged` clears every pending claim, and the grace timer clears one solely because 250 ms elapsed.
- Impact: `await idle` can return on stale state when an older coalesced snapshot lands after the mutation acknowledgement but before the mutation’s follow-up snapshot, or whenever snapshot assembly/event delivery exceeds 250 ms.
- Fix: Have the mutation worker obtain and emit a post-ack snapshot tagged with its local settle generation, release only that generation after the shell applies it, and make the daemon discard coalesced snapshots whose revision changed during assembly instead of publishing them as stale.
- Evidence: The claim begins after the reply at `bridge/requests.rs:91-103`; `shell/root/events.rs:52-62` clears all claims for any snapshot. The daemon records a revision, awaits snapshot assembly, publishes it, and only afterward detects revision drift at `fleet-daemon/src/server/broadcast.rs:115-128`; assembly itself awaits at `fleet-daemon/src/services/snapshots.rs:50-51` and `:117`.

### R2-7 | P2 | crates/fleet-app/src/screens/jobs/log_follow.rs:59
- Problem: The tail-loop reply receiver remains in scope across the 250 ms polling timer, so `Sender::closed()` retains the completed request’s in-flight claim throughout the sleep.
- Impact: While a job log is expanded and following, `idle.in_flight_requests` never has an observable zero edge and `await idle` times out even though no daemon request is running.
- Fix: Drop the reply receiver immediately after applying its answer and before awaiting `TAIL_INTERVAL`; add a paused-time test that observes zero in-flight requests between polls.
- Evidence: `bridge/requests.rs:165-172` deliberately holds the guard until the receiver closes; `log_follow.rs:59-99` keeps `reply` alive through the timer, after which the next loop synchronously claims another request. `screens/jobs.rs:35` sets that interval to 250 ms.

### R2-8 | P3 | crates/fleet-app/src/state/harness/projection.rs:443
- Problem: The Jobs projection filters its rows using `jobs_panel.filter` but always serializes `lists.jobs.filter` as an empty string.
- Impact: Scenarios cannot observe whether the Jobs panel is showing All, Running, or Failed, and an empty filtered result is indistinguishable from an actually empty job registry.
- Fix: Pass `self.jobs_panel.filter.label().unwrap_or_default().to_owned()` to `list` and assert the field in the Jobs filter regression test and scenario.
- Evidence: `projection.rs:443-449` hard-codes `String::new()` while `projection.rs:597-602` applies the real filter; `state/jobs_filter.rs:30-37` already defines the stable `running` and `failed` labels, and `docs/TESTING-HARNESS.md:204` includes `filter:string` in every list snapshot.