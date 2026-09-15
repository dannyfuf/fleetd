# Fleet e2e harness — Phase 2: assertions and synchronisation — Tracker
> Plan: ./fleetd-e2e-harness-2026-09-11-phase-2-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] Phase 1 is done and its tracker is closed; `fleet-harness run` works on this box.
- [x] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
      (integrate-2b: full `make test` green, `make restart` → "Restarted fleetd".)
- [x] I have loaded `gpui-state-and-memory` and `gpui-performance` before touching `AppState`.
- [x] I am ready to start.

## Tasks
- [x] P2-T01 — Define `UiSnapshot` — verified: every version-1 field of
      `docs/TESTING-HARNESS.md` §3 is present in a live dump, in the pinned order.
- [x] P2-T02 — Build the snapshot in update paths and memoise it — verified: `dump` answers
      with a `revision` that moves only on real content change (`revision` 2 and 3 across a
      9-line run); no snapshot construction appears in any `render` path
      (`crates/fleet-app/src/drive.rs:570` `project()` runs in an update path).
- [x] P2-T03 — `dump` command — verified: `dumps/007-help.json` = `{name, revision, summary,
      snapshot}`, paired with `shots/006-help.png`.
- [x] P2-T04 — Predicate language and `await` — verified: `await overlay == Dialog &&
      dialog.name == Help` held in 9 ms with **no `wait` line anywhere in the scenario**; a
      timed-out await returns the last snapshot plus all six idle counters.
- [x] P2-T05 — `assert`, and failure that explains itself — verified below.
- [x] P2-T06 — Terminal grid as text — verified 2026-09-12 (docs stage). A `busy` scenario that
      opens the Workspace and waits 3 s before dumping returns a fully populated `terminal`:
      42 `rows` of the worktree's `nvim` netrw listing, `text` equal to those rows joined by
      `\n`, `cursor: {row:7,col:0,shape:"block"}` and `viewport: {top:0,rows:42,history:0}`.
      The field really is `null` off the Workspace. What the earlier run hit was timing, not a
      missing projection: `await screen == Workspace` is satisfied before the PTY has painted.
- [x] P2-T07 — The `idle` predicate — verified: `await idle` is the first line of every
      scenario here and every run settles without a wall-clock wait.
- [x] P2-T08 — Runner report — verified: `report.md` renders per-line results, an Assertions
      table, inline screenshots and a Dumps table.
- [x] P2-T09 — Documentation — done 2026-09-12 (docs stage). `docs/TESTING-HARNESS.md` §3
      documents every version-1 field, its vocabulary, the additive-only version policy and the
      "built in update paths and memoised per revision" rule; `docs/APP-CONTRACTS.md` carries the
      paragraph that keeps a future contributor from moving the projection into `render`. Every
      predicate path written into the docs was checked against a real dump captured for the
      purpose — `lists.worktrees.selected.label`, `lists.tabs.rows[0].badges`, `jobs[0].status`,
      `terminal.text`, `targets["hub.tab[0]"].w`, `window.bounds.w`, `agents.threads[0].
      pending_gate`, `daemon.link`, `idle.armed_debounces`. The Phase 2 *plan*'s acceptance
      snippet remains wrong (`await overlay == Help`); see Deviations.

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-11 — The binding constraint on this phase is `docs/APP-CONTRACTS.md`: render prepares
  nothing. The snapshot is a projection built in update paths and memoised per revision. If you find
  yourself wanting to build it in `render` because that is where the bounds are known, stop — that is
  Phase 3's problem and it has its own answer.
- 2026-09-11 — The snapshot's vocabulary should be the vocabulary of `docs/KEYMAP.md` and
  `docs/UX-SPEC.md`, so a scenario reads like the spec it is checking.
- 2026-09-12 (integrate-2c) — **Acceptance evidence, both halves.**

  Passing half, `--lane virtual`, no `wait` line anywhere:

  ```text
  ok: 9 steps; run directory: /tmp/fleet-harness/20260912-163040-help    EXIT=0
  shots/006-help.png   1920x1080, the Keymap dialog over the Hub — looked at, it is the app
  dumps/007-help.json  screen=Hub mode=Dialog focus=dialog idle=true, dialog.name=Help
  ```

  Failing half, the same scenario with `assert screen == Workspace`:

  ```text
  harness failure at line 3: assert screen == Workspace
    error: `screen == "Workspace"` failed; screen is "Hub"
    actual: screen == "Workspace" — actually Hub
    shot: …/shots/failure-003.png      dump: …/dumps/failure-003.json
  EXIT=1
  ```

  `failure-003.png` was opened: it is Fleet's first-run Hub — "Copies, sessions and PRs — all
  owned by fleetd, so they survive this window", the `N`/`n`/`?`/`,` hints, `fleetd running ·
  0.1.0 · <hermetic home>`, mode NORMAL. The evidence names the state at the failure.
- 2026-09-12 (integrate-2c) — **The developer's own compositor cannot be trusted for pixels
  while the session is locked, and the lane does not notice.** On this box Omarchy's
  `quickshell` holds an `ext-session-lock` surface; Hyprland paints it on *every* output,
  including a freshly created headless one. The first acceptance run therefore exited 0 with a
  screenshot of the lock screen — a green run over pixels that contain no Fleet at all. Proof:
  `hyprctl output create headless probe-out` followed by `grim -o probe-out` on an output with
  no window on it returns the same "Enter Password" wallpaper. Worse, and independent of the
  lock: `hyprctl layers -j` shows `omarchy-background` **and** `omarchy-bar` attach to each new
  headless output, so the lane's claim that "the output holds nothing but the harness window"
  is false on this developer's machine even unlocked — a whole-output capture bakes their
  wallpaper and a 26 px bar into every baseline. All pixel evidence in this stage was taken
  instead from a nested Hyprland (own instance signature, own `wayland-2`, Lua config so
  `hyprctl eval` works) driven by the unmodified runner. Two fixes are owed: the virtual lane
  must verify that what it photographed is the window (a probe capture of the empty output, or
  a crop to the window rect instead of `grim -o <output>`), and it must refuse rather than
  silently photograph a locked or occluded output.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- ~~**The runner ignores the app's exit status.**~~ **Closed 2026-09-12, fix round** — both
  halves. `shell::root::bootstrap::run` turns the `log`→`tracing` bridge off before returning, so
  the EGL debug callback that fires from a thread-local destructor after `main` has nothing to log
  into a destroyed buffer; a virtual-lane run now leaves `app.log` ending on its last `INFO` line.
  And `scenario::execute` checks the status a `quit` exited with instead of discarding it, so an
  app that answers `quit` and then aborts fails its run. The original note, for the record:
- **The runner ignores the app's exit status.** Every GPU-backed run of Fleet ends in
  `thread 'main' panicked … cannot access a Thread Local Storage value during or after
  destruction: AccessError` / `fatal runtime error: failed to initiate panic, error 5,
  aborting`, and the harness still reports `ok: 9 steps` and exits 0. Backtrace:
  `eglDestroyContext` → `wgpu_hal::gles::egl::egl_debug_proc` → `tracing_log::dispatch_record`
  → `tracing_subscriber::fmt::Layer::on_event` on a destroyed thread-local buffer. Two bugs:
  Fleet aborts on quit whenever a real GPU context exists (12 of 12 virtual-lane runs, 0 of 9
  headless runs), and the harness cannot see it. `shot` and `dump` should not be the only
  oracle — the app's exit code belongs in the report.
- ~~**`terminal.rows` has never been exercised.**~~ **Closed 2026-09-12** — see P2-T06. The
  lesson survives the closure and belongs in the corpus: a Workspace scenario must `await`
  terminal *content* (`terminal.text ~= <something the shell prints>`), never `screen ==
  Workspace`, or it screenshots a blank grid.
- **`busy` + Workspace raises a sticky error on this box.** Opening `acme/web#spike` from the
  `busy` preset leaves `sticky_error: command failed: git rev-parse: No such file or directory
  (os error 2)` for the whole run. *(2026-09-12, fix round: the same text is also what the
  injected `job failure` clone produces — a `git` run in a directory that is not there — so the
  two are the same shape of failure and `daemon/sticky-error.scenario` is not matching the wrong
  one; its regex is anchored on `^command failed: git` now and its header says so.)* The terminal, the tab strip and every list are correct around
  it, so it is one daemon git call with a bad cwd or a missing binary, not a broken fixture. Until
  it is understood, no scenario may assert `sticky_error absent` on `busy`.
- **A failed daemon job raises no toast.** A `job failure` line submits a real job, the job
  reaches `status: "failed"` in `jobs[]` and appears in `lists.jobs` with a `failed` badge, and
  `toasts` stays `[]` for 20 s afterwards. Either the app does not toast job failures or the
  notification never reaches the mirror; §4 of the frozen doc says `busy`'s toasts are supposed to
  come from exactly this line, so one of the two is wrong.
- **A dump of the `empty` fixture's first-run Hub has `targets: {}`.** Correct — the FirstRun
  surface paints no named target — but it makes `empty` a poor fixture for any pointer work.

- 2026-09-12, fix round — **three of the five `idle` counters were written by nothing.**
  `begin_request`, `finish_request`, `set_pending_frame`, `arm_debounce` and `disarm_debounce`
  had no caller outside `state/harness/tests.rs`, so `IdleSnapshot` always reported
  `in_flight_requests: 0, pending_frame: false, armed_debounces: 0` and `await idle` returned
  while a daemon request was outstanding or a 150–400 ms debounce was armed — under all 32 `await
  idle` lines in the corpus. All three are fed now: `Bridge` claims an `InFlight` guard on
  admission and releases it on the runtime thread wherever the request ends, and
  `AppState::harness` reads that same `Arc<AtomicU32>`; the clone-repo and Hub auto-inspect
  debounces hold an `ArmedDebounce` guard, so a window a newer keystroke replaces counts down too;
  and the stale-key gate sets `pending_frame` while a keystroke is parked waiting for a frame.
  `scenarios/hub/idle-accounting.scenario` reads all five by name.
- 2026-09-12, fix round — **`record_target` and `clear_targets` are gone.** They had no caller but
  their own test; the live path is `set_targets`, which the `dump` command uses to copy one
  frame's painted table in.
- 2026-09-12, fix round — **`acknowledge_dismissal` edited `snapshot.jobs` without bumping
  `snapshot_revision`,** so the memoised projection kept serving a dismissed job to every later
  `dump` and `await`. `AppState::bump_snapshot_revision` is `pub(crate)` now and the jobs panel
  calls it; `screens::jobs::tests::dismiss_updates_only_acknowledged_jobs` fails without it.
- 2026-09-12, fix round — **`lists["board.cards"]` and `targets[…]` were unreachable.** The
  predicate path grammar split on `.` with no quoting, so two frozen §3 names could be published
  and never read. `fleet_drive::predicate` takes a quoted step `["…"]` now — additive, no shipped
  name renamed — and `docs/TESTING-HARNESS.md` §2 documents it.

## Deviations

- 2026-09-12 (integrate-2c) — **The plan's acceptance scenario is unrunnable as written.** It
  says `await overlay == Help`, but `docs/TESTING-HARNESS.md` §3 — authoritative over the
  phase plans — fixes the `overlay` vocabulary at `Filter`, `Palette`, `Jobs`, `Dialog` or
  null, and Fleet's Help is a dialog. The acceptance was run as
  `await overlay == Dialog && dialog.name == Help`, which is the same assertion in the frozen
  vocabulary. The phase plan text is the thing that is wrong.
- ~~2026-09-12 (integrate-2c) — The run directory holds `child-home/` and `fixture/` beside the
  layout §6 lists; §6 names neither.~~ **Resolved 2026-09-12 (docs stage):** §6 now lists
  `child-home/`, `bin/` and `fixture/`, says what each is for, records that `home/` is the only
  thing a clean run reclaims, and adds the suite directory's `NNN-<stem>/` layout.

- 2026-09-11 — P2-T01's complete snapshot shape and P2-T03–T05's `dump`, `await`, and `assert`
  protocol shapes were satisfied by the contracts stage at `SNAPSHOT_VERSION = 1`. This tracker
  still leaves their behavior and state population open for the owning implementation stages.
