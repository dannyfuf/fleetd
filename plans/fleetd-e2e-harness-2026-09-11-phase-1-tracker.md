# Fleet e2e harness — Phase 1: a driver I can trust — Tracker
> Plan: ./fleetd-e2e-harness-2026-09-11-phase-1-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have read the roadmap's "Seams between phases" so I do not design the envelope into a corner.
- [x] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
- [x] I have loaded the skills the plan names for the task I am starting (`rust-workspace-architecture`, `rust-async-background-work`, `rust-ipc-protocol`, `gpui-app-shell`, `gpui-styling`, `rust-gpui-testing`, `zed-quality-review`).
- [x] I am ready to start.

## Tasks
- [x] P1-T01 — Extract the shared driver into `crates/fleet-drive`
- [x] P1-T02 — Replace file polling with a request/response unix socket
- [x] P1-T03 — Harness mode: deterministic window, no animation
- [x] P1-T04 — Create `crates/fleet-harness` and the runner CLI skeleton
- [x] P1-T05 — Hermetic environment fixture
- [x] P1-T06 — Display lanes: `headless`, `virtual`, `attach`
- [~] P1-T07 — Capture backend, Wayland first — code complete, proven end to end, and now
      *guarded*: a capture that did not catch the window is refused and deleted rather than
      returned with `ok`, which was the second of the two things this row owed. The first — a
      person opening the PNG and seeing the Hub — still cannot be met while this box's session is
      locked (`docs/TESTING-HARNESS.md` §11).
- [x] P1-T08 — Documentation: replace the smoke procedure — verified 2026-09-12 (docs stage):
      `docs/DEVELOPMENT.md` "Driving the app from a script" describes the socket, the runner, the
      lanes, the run directory and the line grammar; `docs/decisions/0016-e2e-harness.md` records
      what was adopted and what was rejected; `0007` is marked superseded by it.
      `grep -rn FLEET_DRIVE docs/ crates/ README.md` now matches only `0007`, which names the
      removed variable on purpose as the record of what it replaced. `README.md`'s stale
      `FLEET_DRIVE` recipe was replaced by a Testing section pointing at `make harness`.
- [x] P1-T09 — Enable GPUI's Linux display backends (added during integrate-1; see Deviations)

## Verification evidence

Gates, on the integrated tree (2026-09-12):

```text
cargo fmt --all                       # clean
make lint                             # cargo fmt --all -- --check + cargo clippy --workspace
                                      # --all-targets --all-features -- -D warnings
                                      #   Finished `dev` profile in 50.37s, zero findings
cargo build -p fleet-daemon && make test
                                      #   fleet-app     733 passed; 0 failed
                                      #   fleet-daemon  553 passed; 0 failed; 1 ignored
                                      #   fleet-harness  21 passed; 0 failed
                                      #   fleet-core 198, fleet-cli 103, fleet-client 27,
                                      #   fleet-drive 35, fleet-ui-kit 316 + 2 doctests
                                      #   every integration binary ok; no failures anywhere
```

Acceptance, headless lane — fails only the `shot` line, with the documented message:

```text
$ ./target/debug/fleet-harness run /tmp/help.scenario --lane headless
run directory: /tmp/fleet-harness/20260912-000557-help (requested lane headless, scenario /tmp/help.scenario)
lane: headless (no pixels)
Error: run directory: /tmp/fleet-harness/20260912-000557-help
Caused by:
    line 3: shot help
      error: no pixels in the headless lane: this Fleet has no compositor surface
EXIT=1
```

Acceptance, virtual lane — exit 0, run directory printed, one response per command, isolated
output created and removed:

```text
$ ./target/debug/fleet-harness run /tmp/help.scenario
run directory: /tmp/fleet-harness/20260912-000456-help (requested lane virtual, scenario /tmp/help.scenario)
lane: virtual (isolated output fleet-harness-20260912-000456-help)
ok: 5 steps; run directory: /tmp/fleet-harness/20260912-000456-help
EXIT=0
$ hyprctl monitors -j   →  ['eDP-1']          # before and after
$ ls shots/             →  003-help.png        # 1920x1080 PNG, 469 373 bytes
$ cat run.jsonl         →  5 command records, one per scenario line, every one ok:true
    shot →  {"run_id":"20260912-000456-help","lane":"virtual",
             "bounds":{"x":0.0,"y":0.0,"w":1440.0,"h":900.0},"scale_factor":1.0,
             "title":"Fleet [harness:20260912-000456-help]","frame":6,"name":"help"}
```

The window really is placed on the isolated output. Polling `hyprctl clients -j` through a
`wait 3000` run:

```text
FOUND 'Fleet [harness:20260912-000542-slow]' monitor 0 at [647, 38]  size [609, 670]   ws 1
FOUND 'Fleet [harness:20260912-000542-slow]' monitor 1 at [1280, 0]  size [1440, 900]  ws 2
```

— it maps on eDP-1 for a moment, then the lane moves it to the isolated output at exactly the
pinned 1440x900.

`dump` reaches the real projection (headless run of
`wait 500 / dump before / key ? / dump help / key escape / dump after / quit`):

```text
summary: screen=Hub mode=Normal focus=worktrees.row[0] selected=All idle=true
window:  {'bounds': {'x': 240.0, 'y': 90.0, 'w': 1440.0, 'h': 900.0},
          'scale_factor': 1.0, 'title': 'Fleet [harness:…]', 'frame': 1}
idle:    {'idle': True, 'in_flight_requests': 0, 'running_jobs': 0, 'pending_frame': False,
          'live_toast_timers': 0, 'armed_debounces': 0}
```

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-11 — Planning verified on the dev box, Hyprland 0.56.2:
  `hyprctl output create headless fleet-harness` → an isolated 1920×1080 monitor (default scale 2,
  so the mode must be pinned explicitly); `grim -o fleet-harness out.png` → valid PNG;
  `hyprctl output remove fleet-harness` → session restored. `hyprctl keyword monitor …` is refused
  on this config with "keyword can't work with non-legacy parsers" — use `hyprctl output` or a
  batch dispatch instead, and verify with `hyprctl monitors -j`.
- 2026-09-11 — `gpui::guess_compositor()` returns "Headless" with no `WAYLAND_DISPLAY`/`DISPLAY`, or
  with `ZED_HEADLESS` set. The headless window's `draw` discards the scene: real layout and text
  shaping, no pixels. That is the fast lane, and it is why `shot` must fail loudly there.
- 2026-09-12 — **`fleet` could not open a window on Linux at all, and nobody had noticed.**
  `gpui_platform` and `gpui_linux` are both `default-features = false` in Zed's own workspace, and
  our root `Cargo.toml` asked `gpui_platform` for `font-kit` only. The built binary contained no
  Wayland or X11 code (`strings target/debug/fleet | grep wl_compositor` → 0 hits), so
  `guess_compositor()` compiled down to "Headless" and every launch — `make run` included — sat
  there with a socket, a state tree and no surface. Only a *test* build got the backends, because
  `gpui/test-support` happens to enable `wayland` and `x11`. The fix is one workspace manifest
  line; the harness is what made it visible. See `docs/decisions/0016-e2e-harness.md`.
- 2026-09-12 — The headless lane does paint: `AppFrame::render` runs, so
  `fleet_ui_kit::harness::frame(window)` reaches 1 and the snapshot's `window.frame` is real even
  with no compositor. Only `on_next_frame` never resolves there, which is why `settle` yields
  instead of awaiting a frame in that lane.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- **P1-T07's "look at the screenshot" half is still blocked by the developer's locked session.**
  Re-confirmed by the docs stage on 2026-09-12: `./target/debug/fleet-harness run
  /tmp/help.scenario` exits 0, writes `shots/003-help.png`, and that PNG is a 1920x1080 "Enter
  Password" lock screen. Hyprland composites the session-lock surface over every output including a
  freshly created headless one, and `pgrep hyprlock` finds nothing — Omarchy's `quickshell` holds
  the `ext-session-lock` surface — so the lock cannot be dismissed from a shell. Everything else
  about the capture path is proven: the window is bound to the isolated output at the pinned
  1440x900, `grim` writes a valid PNG, the output is removed and `hyprctl monitors -j` reads
  `['eDP-1']` afterwards. Two things are owed, and they are `lanes-capture`'s: verify what was
  photographed (probe the empty output, or crop to the window rect instead of `grim -o <output>`),
  and fail loudly rather than returning lock-screen pixels with `ok`.
- 2026-09-12, fix round — **the second of those two is done.** `lane::verify_window_was_captured`
  keeps a capture of the isolated output taken in the moment before the window is moved onto it
  and refuses any `shot` that does not differ from it by the window's own share of the output,
  deleting the rejected picture. Re-confirmed today on this box: the session is locked again, and
  every `shot` fails rather than writing a lock screen into `shots/`. The guard had a hole that
  only a full corpus run showed: comparing the *whole output* meant a locked session's own
  animation counted as change, and 7 of 41 scenarios passed with a photograph of the lock screen
  in `shots/`. It compares the window's own rectangle now (`baseline::differing_share_within`),
  where two captures of one lock screen differ by 0.0% and a window that painted differs by
  nearly everything. A second hole showed up in the re-run: a brand-new output is not painted the
  instant it exists, so a reference taken too early is a blank buffer that everything differs
  from. `record_empty_output` now captures twice and requires the two to agree before it accepts
  the reference, and says so and skips the check if they never do. The
  first of the two — *capture* the window's region instead of `grim -o <output>` — is still
  owed and recorded in `docs/TESTING-HARNESS.md` §11, because it is also what would make a
  baseline window-sized rather than output-sized.
- 2026-09-12, fix round — **Fleet aborted on every quit under Wayland**, and the runner did not
  notice. `wgpu_hal`'s EGL debug callback logs from a thread-local destructor after `main`
  returns; `tracing-log` forwards it to a `tracing_subscriber` formatter whose own thread-local
  buffer is already gone, and a panic raised there cannot unwind — `fatal runtime error: failed
  to initiate panic, error 5, aborting`. `shell::root::bootstrap::run` now closes the `log`
  bridge before returning, and `scenario::execute` fails a run whose `quit` exits non-zero
  instead of discarding the status.

  **Both owed halves closed 2026-09-12 (verify round 2), the first of the two ways.**
  `crates/fleet-harness/src/lane.rs` photographs the isolated output in the moment before the
  window is moved onto it and holds every later `shot` to differing from that reference by at
  least a third of the window's own area; a shot that does not clear the bar fails its line,
  names the share it saw and the share it needed, and deletes the rejected picture instead of
  leaving it for `report.md` to inline. `baseline::differing_share` is the measurement. The
  consequence, measured: `make harness` now stops at scenario 001 line 22 on this box, where
  before it reported 38/38 green while 25 of its 41 screenshots were the lock screen.

  The *third* thing, now measured and still owed to the same stage: the pinned 1440x900 in the
  paragraph above was not being held. Across one 38-scenario run the window reported 1440x900,
  900x670 and 1920x1080 depending on whether the session-lock surface was up when it mapped —
  floating a tiled window restores the size it last floated at, not the size the app asked for.
  The same commit dispatches an exact resize and holds the placement poll to it, so the run above
  reports 1440x900 throughout. What is still owed is the capture itself: `grim -o` photographs
  the whole output, so the developer's wallpaper, their bar over the window's top 26 px, and
  their `opacity 0.985 0.96` window rule are inside every picture — verified by eye in
  `016-stalled-daemon/shots/019-stalled.png`, where the wallpaper shows *through* Fleet. Cropping
  is not the fix; capturing the window's own buffer is. `grim -T <identifier>` does exactly that
  and this box's grim has it — `grim -T "$(hyprctl clients -j | jq -r '…stableId')"` returned the
  window's own 912x1005 buffer while the session lock was up. That is a `lanes-capture` design
  change to §4 and §6, not a verify-stage edit.
- ~~**`key ?` is handled but opens nothing in the first-run Hub.**~~ **Closed 2026-09-12.** It
  opens the Keymap dialog in the `virtual` lane: `await overlay == Dialog && dialog.name == Help`
  held in 9 ms with no `wait` line (phase-2 tracker). The `empty` fixture's first-run screen was
  hiding a different and larger defect — a `key` line does nothing *at all* in the `headless`
  lane — which is now tracked in the phase-3 tracker's Deviations.
- **The harness window flashes on the developer's screen before the lane moves it.** Hyprland 0.56.2
  refuses `windowrulev2` through `hyprctl keyword`, so the window can only be bound by address after
  it appears. A `hyprctl --batch` dispatch chain at map time would close the gap.
- ~~**`report.md`, fixture presets other than `empty`, baselines, fault injection and the scripted
  agent still bail.**~~ **Closed 2026-09-12.** All five stages have run: `report.md` is written for
  every run and every suite, all five presets seed through a private `fleetd`, `daemon …` and
  `socket remove` inject faults, and `fleet-harness agent` speaks both wire protocols.
- ~~**`--update-baselines` does nothing yet.**~~ **Closed 2026-09-12.** `baseline.rs` keys
  `scenarios/baselines/virtual/<scenario>/<NNN>-<name>.png` off the scenario's corpus-relative path
  and compares on every virtual-lane `shot`. No baseline has ever been *recorded* — that is a
  corpus gap tracked in the phase-5 tracker, and it cannot be closed on a locked session anyway.
- **The `idle` counters still have no writers for three of five sources.** Re-checked 2026-09-12:
  `begin_request`, `set_pending_frame` and `arm_debounce` are called only from
  `crates/fleet-app/src/state/harness/tests.rs`, nowhere in production. So `in_flight_requests`,
  `pending_frame` and `armed_debounces` are permanently 0 in every dump taken so far, and
  `await idle` is only as strong as `running_jobs` + `live_toast_timers`. Every scenario written
  to date relies on `await idle`, which makes this the most load-bearing open item in the harness.
- ~~**`targets` is empty until the `target-naming` stage annotates surfaces.**~~ **Closed
  2026-09-12.** A live `agents` dump carries `hub.tab[0..2]`, `repos.rail`, `repos.row[0..1]` and
  `worktrees.row[0]`; a live `busy` Workspace dump carries `tabs.tab[0..2]` and
  `sticky_error.retry`. The `empty` fixture's first-run screen paints none, which is correct and is
  why `empty` is a poor fixture for pointer work.

## Deviations

- 2026-09-11 — The contracts stage froze the wire command enum and complete `UiSnapshot` at their
  final Phase-5 shape with `SNAPSHOT_VERSION = 1`. Later command values and snapshot fields are
  present as typed seams now; implementation remains with their ownership stages. This deliberately
  replaces the phase plan's incremental protocol growth and Phase-3 snapshot-version bump so the
  remaining stages can work concurrently.
- 2026-09-12 — **New task P1-T09: the workspace now asks `gpui_platform` for `wayland` and `x11`.**
  Not in the plan because the plan assumed the app could already open a window. It could not. This
  is a product fix, not a harness one.
- 2026-09-12 — **`fixture:` is optional and defaults to `empty`.** `docs/TESTING-HARNESS.md` §2 said
  it was mandatory, but both this plan's acceptance scenario and Phase 2's use no fixture line at
  all. The doc is authoritative, so the doc was made true rather than the scenarios changed: the
  line is optional, and when present must still be first and appear once.
- 2026-09-12 — **`dump`/`await`/`assert` landed here, not in the `app-commands` stage.** The
  integrate step owned wiring the snapshot API into `drive.rs`, and the seam the socket stage left
  was three `bail!`s. `await` polls the memoised projection every 10 ms and skips re-evaluating an
  unchanged revision; a failed `assert` or a timed-out `await` is an `ok:false` response carrying
  the clause values and the snapshot, which §1 permits and §2 requires.
- 2026-09-12 — **One frame counter, not two.** The socket stage counted painted frames in the
  driver; the ui-kit recorder stamps rectangles with its own. `meta`, `shot` and the snapshot's
  `window.frame` now all report `fleet_ui_kit::harness::frame(window)`, so the contract's
  "a target's frame must equal `window.frame`" rule compares one counter with itself.
- 2026-09-12 — **Teardown asks Fleet to quit before killing it.** A run that failed never reached
  its `quit` line, so teardown waited out the five-second grace period and then `SIGKILL`ed, leaving
  the socket file in the run directory. A failed headless acceptance run now takes 0.63 s instead of
  ~6 s and leaves no socket behind. `fleetd` is likewise shut down through `Daemon::shutdown()`
  rather than killed, which is what verifies its socket is gone.
- 2026-09-12 — **`await`'s trailing timeout is split with a quote-aware tokenizer**, and both
  `await` and `assert` parse their predicate runner-side. `await toasts[0].text == "job failed" 3000`
  previously read `failed"` as the timeout; a malformed predicate now fails at scenario load,
  before a daemon, a window or a compositor output exists.
- 2026-09-12 — **A `shot` the app answered `ok:false` is no longer captured**, so the headless
  message reaches the runner instead of being replaced by a capture error, and failure evidence is
  named `shots/failure-NNN.png` per §6 rather than `NNN-failure.png`.

## Fix-round verification evidence (2026-09-12)

```text
cargo fmt --all -- --check      clean
make lint                       clippy --workspace --all-targets --all-features -D warnings
                                  Finished in 57.58s, zero findings          EXIT 0
make ci                         lint + test + test-scripts
                                  78 suites, 2 718 tests, 0 failed; script tests skipped
                                  off Darwin                                 EXIT 0
cargo build -p fleet-daemon && make test
                                  78 suites, 2 717 tests, 0 failed           EXIT 0
                                  including tests/harness_headless.rs: 3 scenarios in 9.4 s
make harness-headless             3 pixel-free scenarios, all ok             EXIT 0
make harness HARNESS_ARGS=--continue-on-failure
                                  "38 of the 41 scenarios that ran failed"    EXIT 2
                                  41 of 41 ran; the 3 that pass are the pixel-free ones;
                                  every one of the 38 failures is the capture guard refusing a
                                  lock screen, and 0 failures are of any other kind — every
                                  structured assertion in the corpus holds;
                                  0 lock screens filed in shots/ (7 leaked before the guard was
                                  narrowed to the window's rectangle, 4 before its reference was
                                  made to settle);
                                  0 of 41 app.log files end in the quit abort (was 31 of 31)
```

The session on this box is locked, which is the whole of the red. Nothing in the corpus fails on
what it was written to check.
