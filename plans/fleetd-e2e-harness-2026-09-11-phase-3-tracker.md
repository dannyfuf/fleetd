# Fleet e2e harness — Phase 3: mouse, targets and the rest of human input — Tracker
> Plan: ./fleetd-e2e-harness-2026-09-11-phase-3-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] Phase 2 is done and its tracker is closed; `dump`, `await` and `assert` work.
- [x] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
- [x] I have loaded `gpui-components` and `gpui-performance` before touching `fleet-ui-kit`.
- [x] I am ready to start.

## Tasks
- [x] P3-T01 — The `harness_target` element wrapper — verified: `crates/fleet-ui-kit/src/harness.rs`
      records per frame behind a thread-local flag; `window.frame` advances in every run.
- [x] P3-T02 — Name the surfaces scenarios need — verified against a live `one-repo` dump:
      `hub.tab[0..2]`, `repos.rail`, `repos.row[0..1]`, `worktrees.row[0]` on the Hub;
      `agents.composer`, `agents.transcript`, `agents.tabs.tab[3]` in a native agent thread;
      `agents.decision`, `agents.approval.{allow_once,allow_always,deny,deny_and_stop}` at a gate.
- [x] P3-T03 — `targets` in the snapshot — verified: rects carry `frame`, and `frame` equals
      `window.frame` on the answering frame.
- [x] P3-T04 — Mouse commands — verified: `click`, `move`, `hover` all drive the real app, and
      an unknown target fails usefully rather than clicking `(0,0)`:
      `unknown target "worktrees.row[0]" in frame 9; nearest of the 8 painted: repos.row[0],
      repos.row[1], prs.tab[0], prs.tab[1], hub.tab[0]`.
- [x] P3-T05 — Unify scroll — verified: `scroll 0 -2 at worktrees.row[0]` is accepted and
      settles. (Signed-direction behaviour is *not* proven: the one-repo list is one row long,
      so nothing could move. See Follow-ups.)
- [~] P3-T06 — Clipboard, window and focus — three of four verified 2026-09-12 (docs stage),
      one is broken. A `one-repo` scenario ran `meta`, `resize 1200x800`,
      `await window.bounds.w == 1200`, `blur`, `focus`, `advance 5000`, `dump`, `quit` → `ok: 9
      steps`. `resize` answers the same geometry object `meta` does, with the new bounds already
      applied; `blur` and `focus` both answer `ok` with an empty data object; `advance 5000`
      answers `{"millis":5000,"changed":false}`. **`clipboard set` fails in the `virtual` lane**
      with `no platform clipboard in this lane: this Fleet has no compositor surface`, in the same
      run where `shot` captures fine — the write-then-read-back guard at
      `crates/fleet-drive/src/input.rs:321` never sees its own write on Wayland. So the clipboard
      has no working lane at all, not just no headless one.
- [x] P3-T07 — Documentation and a keymap cross-check — done 2026-09-12 (docs stage).
      `docs/TESTING-HARNESS.md` §1 documents every input command with its exact argument object
      and the consequences a scenario author would otherwise rediscover (scroll sign and the
      18 px unit, what `blur` can and cannot do, what `advance` does *not* move, and that the
      clipboard has no working lane); §3 "Target names" gives the `<surface>.<part>[<index>]`
      scheme, the by-surface table of what Fleet paints today, and the frame-staleness rule.
      `docs/DESIGN-SYSTEM.md` §6 carries the target wrapper's contract beside the kit's other
      component contracts, including why a recorded rectangle is a per-frame stamp rather than a
      cache. The plan's own acceptance scenario stays unrunnable; see Deviations.

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-11 — GPUI's `Window::debug_a11y_tree_json` looked like a free targeting oracle, but the
  tree is only captured while a11y is *active* (an assistive-technology client must attach), and the
  activation flag is set by the platform adapter with no programmatic override. Rejected as a
  harness dependency; recorded as a possible follow-up if Fleet ever adopts a11y roles broadly,
  since the two efforts would reinforce each other.
- 2026-09-11 — The existing wheel step uses a fixed 18 px "wheel unit" declared to be part of the
  driver protocol and independent of theme density. Keep that guarantee when unifying scroll.
- 2026-09-12 (integrate-2c) — **Acceptance evidence.**

  ```text
  fleet-harness run scenarios/pointer-basics.scenario --lane virtual
    ok: 23 steps; /tmp/fleet-harness/20260912-163328-pointer-basics   EXIT=0

  fleet-harness run <same file, shot lines stripped> --lane headless
    lines 1-30 pass, including every click / move / hover / scroll
    harness failure at line 31: await screen == Workspace 20000
      timed out: `screen == "Workspace"` failed; screen is "Hub"   EXIT=1
  ```

  Shots were opened. `017-board-tab.png`: the BOARD tab is the selected segment, the pane reads
  "No cards yet. / c new card", and the pointer is drawn as a hand at the click point — the
  click landed on the control, not on a coordinate guess. `034-workspace.png`: the Workspace
  with terminal tabs `1 nvim`, `2 cc`, `3 lg`, breadcrumb `Acme > feature`, mode TERMINAL.
- 2026-09-12 (integrate-2c) — The Workspace shot is taken the instant `screen == Workspace`
  holds, so the terminal grid in it is still blank. A Workspace scenario should await terminal
  *content*, not the screen.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- **Fleet's Hub rows have no pointer affordance at all.** `views/worktrees_list.rs`,
  `views/repos_rail.rs`, `views/prs_screen.rs` and `screens/jobs/presentation.rs` register
  zero `on_click`/`on_mouse_down` handlers; `components/list_view.rs` and
  `components/fuzzy_list.rs` register none either. A single *and* a double click on
  `worktrees.row[0]` leave `screen=Hub`, `focused=worktrees.row[0]`, no toast, no log. The only
  clickable Hub surfaces are `SegmentedTabs` (`hub.tab[N]`, `prs.tab[N]`) and the board.
  Decide whether that is the product (keyboard-first by design) or a gap, and then either fix
  the plan's wording or give rows an `on_click`.
- **`terminal_tab_strip.rs` paints `tabs.tab[N]` but registers no click handler**, so
  "switch tabs by clicking" is not possible in the Workspace either.
- **Signed scroll is unproven, and it is blocked on a fixture.** No preset paints a list longer
  than the 900 px viewport, so nothing can move by `18·N` logical pixels. Either `busy` grows
  enough worktrees to overflow, or the proof moves to the terminal's scrollback.
- ~~**`--continue-on-failure` did not continue.**~~ **Not reproducible 2026-09-12 (docs stage).**
  `assert screen == Workspace` / `dump after-failure` / `quit` with the flag set on the integrated
  tree wrote both `dumps/failure-002.json` and `dumps/003-after-failure.json` and then exited 1,
  which is the documented behaviour. The suite path has the same shape — `scenario.rs:129` breaks
  only when the flag is unset. Whatever the original run hit is not in the tree now; re-open it
  with a repro if it comes back.

- 2026-09-12, fix round — **the headless lane can drive the keyboard now**, so the phase-3
  definition-of-done line "the pointer acceptance scenario passes in both lanes" is reachable:
  `scenarios/hub/pointer-tabs.scenario` is the pointer scenario without a `shot`, and it passes
  headless. Root cause of the old behaviour: `Shell::prepare_key_replay` parks a keystroke that
  arrives ahead of the painted dispatch tree and replays it from `Window::on_next_frame`, which
  GPUI's headless platform accepts and never calls — so `replayed_generation` never caught up,
  `is_stale()` stayed true for the life of the process, and every key was queued for ever while
  `dispatch_keystroke` still answered `true`. In a process with no frame loop (`drive::no_frame_loop`)
  the same drain is also scheduled through `Window::defer`, which runs at the end of the effect
  cycle the draw belongs to. `finish_render` is idempotent, so the two paths cannot double-replay.
- 2026-09-12, fix round — **five files are past the ~900-line rule** and stay that way for now:
  `fleet-harness/src/{report.rs, baseline.rs, scenario.rs}`, `fleet-drive/src/{input.rs,
  predicate.rs}`. Each is one subject, not an accumulation; splitting them is a deliberate
  refactor. Recorded in `docs/TESTING-HARNESS.md` §11 so no reviewer has to rediscover it.
- 2026-09-12, fix round — **a `begin_frame` called twice in one frame is still unguarded.** GPUI
  exposes no per-draw identity a `debug_assert!` could compare against (`simulate_next_frame` is
  `test-support`-only), so the invariant stays a comment on `AppFrame::render`. Re-open it if
  GPUI grows a public frame counter.

## Deviations

- 2026-09-12 (integrate-2c) — **`scenarios/pointer-basics.scenario` cannot be what the plan
  describes.** The plan asks for "open a worktree by clicking it, switch tabs by clicking,
  close with the keyboard". The first two are impossible against today's Fleet (see
  Follow-ups), so the shipped scenario drives every pointer path that *is* painted — three
  `click hub.tab[N]`, `move`, `hover`, `scroll at <target>` — and opens and closes the session
  with the keyboard, with a comment in the file saying why. This is a smaller scenario than
  the plan imagined, and it is the honest one.
- 2026-09-12 (integrate-2c) — **The scenario cannot pass in both lanes, and the reason is not
  the scenario.** In the headless lane a key is reported handled but never reaches the focused
  view: the identical Phase 2 help scenario that passes in `virtual` times out in `headless`
  with `overlay is null` after `key ?` returned `handled: [true]`; `key o` on a worktree row is
  a silent no-op there while it really opens the Workspace in `virtual`. Pointer commands are
  unaffected — all 15 pointer lines of `pointer-basics` pass headless. So the headless lane
  today runs mouse scenarios only, which means `make harness-headless` and any display-less CI
  lane cannot cover the keymap. This is the largest single blocker found in this stage.
- 2026-09-12 (integrate-2c) — P3-T06 is left open rather than ticked: no scenario in this
  stage exercised `blur`, `focus`, `resize` or the clipboard, and `clipboard_*` is documented
  as `virtual`-only anyway.

- 2026-09-11 — P3-T03's `targets` map and P3-T04–T06's pointer/host protocol shapes were satisfied
  by the contracts stage. `targets` shipped in the initial schema, so the planned version bump is
  intentionally omitted and `SNAPSHOT_VERSION` remains 1; behavior stays with the owning stages.
