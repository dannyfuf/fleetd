# Adversarial validation — batch "false-green" (run integrity)

Base `881162c5c283cefb4319bbfdb79629ef31960745`, branch `test-harness`. Every line number below
was re-checked against the working tree after the temporary test edit was reverted
(`git checkout -- crates/fleet-harness/src/report.rs`); the repo is unmodified by this work.

Two live repros were built:

1. **A fake `fleet`** (`$SCRATCH/fakefleet.py`) that speaks the real drive protocol on
   `FLEET_HARNESS_SOCK`, answers `quit` exactly the way `crates/fleet-app/src/drive.rs:314`
   does, and then leaves with status 134. Driven through the *real* `fleet-harness` binary and
   a *real* private `fleetd` via `FLEET_APP=… fleet-harness run … --lane headless`.
2. **Three temporary `#[tokio::test]`s** appended to `report.rs`'s inline `mod tests`, run with
   `cargo test -p fleet-harness --lib report::tests::adversarial_ -- --nocapture`, then reverted.
   All three passed; their output is quoted verbatim below.

---

### C5 (= F2-2) — verdict: SURVIVES

- **Confidence**: high — reproduced end to end.

- **Reasoning**, step by step:
  1. A scenario's last line is `quit` (all of them; `scenarios/daemon-down.scenario:23`,
     `scenarios/pointer-basics.scenario:40`, `scenarios/hub/help.scenario:30`,
     `scenarios/hub/settings.scenario:25`, `scenarios/agent-approval.scenario:29`).
  2. `dispatch`'s catch-all arm (`crates/fleet-harness/src/scenario.rs:856`) sends `quit` and
     awaits the response. On the app side, `drive.rs:314` returns the response *value*; the
     driver loop `drive.rs:206-216` then writes it to the reply channel, and only afterwards
     runs `cx.update(|_, cx| cx.quit())`, `break`s, `drop(channel)` and `drop(server)` — which
     **joins the listener thread** — before GPUI unwinds the window and the GPU context. So the
     app's remaining work after the bytes leave is: a channel hop, a `cx.quit()`, a thread join,
     a window teardown and a process exit.
  3. The runner, having the response in hand, journals the exchange
     (`scenario.rs:232-236`, a file append), pushes the `StepReport`, and only then polls
     `app.try_wait()` at `scenario.rs:561`. That is microseconds later. `try_wait` returns
     `None`, so neither the `Some(status) if … Quit` arm (`scenario.rs:564-572`) nor the general
     `Some(status) => bail!` arm (`scenario.rs:573-579`) runs.
  4. The loop ends (no more lines). `Stage::teardown` (`scenario.rs:326-331`) sees
     `app.try_wait()` is no longer `Ok(None)` — or sends a redundant `quit` and gets nothing —
     and then `stop(&mut app, "Fleet", QUIT_GRACE)` reaps the child at `scenario.rs:253`:
     `Ok(Some(_status)) => return Ok(())`. The status is discarded. The other success path,
     `scenario.rs:260-262`, discards it too: `waited.map(|_status| ())`.
  5. `outcome` is `Ok(())`, `problems` is empty → `run_one` prints `ok: N steps` and
     `write_report` writes **Passed**. Process exit code 0.

- **Evidence** — a real run of the real harness binary against a Fleet that aborts on quit:

  ```text
  $ FLEET_APP=$SCRATCH/fakefleet.py ./target/debug/fleet-harness run \
        $SCRATCH/abort-on-quit.scenario --lane headless
  run directory: /tmp/fleet-harness/20260914-072220-abort-on-quit (requested lane headless, …)
  lane: headless (no pixels)
  report: /tmp/fleet-harness/20260914-072220-abort-on-quit/report.md
  ok: 2 steps; run directory: /tmp/fleet-harness/20260914-072220-abort-on-quit
  EXIT=0
  ```

  `report.md`:

  ```text
  **Passed** — 2 lines in 1 ms.
  | **Lines** | 2 run, 0 failed |
  | 3 | `quit` | ok | 0 ms |  |
  ```

  `app.log`, from the same run directory:

  ```text
  fake fleet: listening on …/fleet-harness.sock
  fake fleet: answered quit, now aborting      # then os._exit(134)
  ```

  The fake exits with `os._exit(134)` — *no* GPU teardown, *no* thread join, *no* window drop —
  i.e. the fastest death physically available to it, orders of magnitude faster than the real
  Fleet's shutdown. It still lost the race every time. A control run with `os._exit(0)` and a
  run with a 2 s stall before the abort produce byte-identical stdout: the harness cannot
  distinguish an orderly shutdown from an abort.

  ```rust
  // crates/fleet-harness/src/scenario.rs:561
  match app.try_wait().context("poll the Fleet process")? {
      Some(status) if matches!(line.instruction, Instruction::App(Command::Quit(_))) => {
          anyhow::ensure!(status.success(), …);   // never reached
  // crates/fleet-harness/src/scenario.rs:251-262
  match child.try_wait() { Ok(Some(_status)) => return Ok(()), … }
  return waited.map(|_status| ())
  ```

- **Why existing tests do not catch it**: `scenario.rs` has no `#[cfg(test)]` module at all and
  `crates/fleet-harness/tests/` does not exist, so the runner's process lifecycle is covered by
  nothing but `make harness` itself — which is the thing reporting green.

- The branch's own history confirms the failure mode is not hypothetical: `log::set_max_level(Off)`
  was added in `shell/root/bootstrap.rs` because Fleet aborted with "fatal runtime error: failed
  to initiate panic" on quit. Reverting that fix today would be reported as a pass.

---

### C6 (= F1-8) — verdict: SURVIVES NARROWED

- **Confidence**: high (code path unambiguous; not run end to end because it needs a lane with
  real pixels and a recorded baseline, and §11 records that no baseline has ever been recorded).

- **Reasoning**: `dispatch`'s `Command::Shot` arm sets `context.artifact` and then
  `anyhow::ensure!(outcome.passed(), "{}", outcome.summary())` at
  `crates/fleet-harness/src/scenario.rs:842`, *before* the arm's `Ok(Some(response))` at
  `scenario.rs:844`. `passed()` is false only for `BaselineOutcome::Differed`
  (`baseline.rs:130`), i.e. a blown pixel budget. `dispatch` therefore returns `Err`, and
  `execute`'s match takes the `(Err(error), _)` arm at `scenario.rs:517-529`: it writes only
  `record_event("error", {line, source, error})`. The
  `(Ok(Some(response)), Instruction::App(command))` arm at `scenario.rs:230-236`, the only
  caller of `RunDirectory::record` (`rundir.rs:113`), is never reached for that line. The shot's
  request/response pair never enters `run.jsonl`.

- **What is actually lost** (this is the narrowing): the response the app sends for `shot`
  carries the settled window geometry plus the shot name (`crates/fleet-app/src/drive.rs:331-343`)
  — precisely the "geometry and frame the app reported for the shot it took". That is gone.
  Contradicts `docs/TESTING-HARNESS.md:461-462`: "The journal is the complete record … it is
  assembled only from what is already on disk, so it cannot describe something `run.jsonl` does
  not show."

- **The finding overstates one consequence, and it should be corrected**: the report does *not*
  lose the shot. `RunReport::shots_section` (`report.rs:553-576`) walks `self.steps` and uses
  `step.artifact`, which was set at `scenario.rs:841` *before* the `ensure!`; and the `baseline`
  journal event was already written at `scenario.rs:838-839`, so `baseline_notes`
  (`report.rs:1101`) still renders the verdict and inlines the diff. The `## Lines` "what
  happened" cell also survives, because `detail` returns `step.error` first (`report.rs:522-524`).
  Positional alignment also survives, because `"error"` is in `EXCHANGES` (`report.rs:27`).

- **Reduced claim that holds**: *a `shot` that fails its baseline is the one executed app command
  whose request/response never reaches `run.jsonl`, so the journal is not the complete record it
  is documented to be, and the app's reported geometry for the failing frame is unrecoverable.*
  Severity is lower than "the report loses its command entry" implies.

- **Why existing tests do not catch it**: no test in the workspace drives `dispatch`; the
  baseline tests (`a_new_scenario_is_not_a_failure_and_update_baselines_records_it`,
  `baseline.rs:1518`) call `Baselines::check` directly and never touch the
  journal.

---

### C7 (= F1-2) — verdict: SURVIVES NARROWED

- **Confidence**: high.

- **Reasoning**:
  1. `Baselines::check` returns early for any non-`virtual` lane, *before* it consults `update`:
     ```rust
     // crates/fleet-harness/src/baseline.rs:343
     if lane != Lane::Virtual {
         return Ok(BaselineOutcome::Skipped { lane: lane.as_str().to_owned() });
     }
     let baseline = self.baseline_for(lane, shot)?;
     if update { … }                                  // baseline.rs:348
     ```
  2. `Skipped::passed()` is true (`baseline.rs:130`), so the shot does not fail; its journal
     event says `"passed": true` (`baseline.rs:181-191`) and its summary is
     `"baseline: not compared in the {lane} lane"` (`baseline.rs:137-139`). The run exits green
     having written nothing.
  3. The CLI accepts `--lane attach --update-baselines` together (`main.rs:24-42`), and
     `dispatch` passes `context.lane.lane()` — read live, per shot (`lane.rs:695-698` reads the mutex on every
     call) — into `check` (`scenario.rs:832-837`).
  4. The mid-run fallback is real: `HyprlandBackend::bind_window` calls `fall_back_to_attach`
     when `place_on_isolated_output` fails (`lane.rs:737` → `lane.rs:656-668`, which sets
     `inner.lane = Lane::Attach`), and `execute` calls `bind_window()` at `scenario.rs:457`
     after Fleet's socket answers. So `make harness HARNESS_ARGS=--update-baselines` (default
     lane `virtual`, `main.rs:25`) on a machine whose compositor refuses the isolated output
     records **nothing**, and `docs/TESTING-HARNESS.md:410` says flatly
     "`--update-baselines` explicitly replaces baselines", with no lane caveat.

- **Narrowings** (both matter):
  - The behaviour is **deliberate and already pinned by a test**:
    `nothing_is_compared_or_recorded_outside_the_virtual_lane` (`baseline.rs:1723`) calls
    `check(lane, &shot, true, …)` for `Headless` and `Attach` and asserts `Skipped` plus
    "the developer's own session never becomes a committed baseline". The doc comment on `check`
    (`baseline.rs:333-335`) says the same. So this is **not a logic bug in `check`** — it is a
    contract violation between `docs/TESTING-HARNESS.md:410` and the code, which under
    `CLAUDE.md`'s "docs/ is authoritative … when code and a doc disagree, one of them is a bug"
    is still a defect, but the fix belongs in the doc and in the *outcome name*, not in the lane
    rule.
  - The fallback is **not silent about the lane**: `fall_back_to_attach` prints two warnings to
    stderr (`lane.rs:669-671`), a `lane-fallback` journal event is written
    (`scenario.rs:349-358`) and the report carries the effective lane. What is silent is
    specifically *that no baseline was recorded*: `--update-baselines` produces the same
    `Skipped` outcome as a run without the flag, so there is no way to tell "skipped comparison"
    from "refused to record".
  - The fallback fires at window-bind time, i.e. after the run starts but before line 1 — so
    every shot in the run is affected uniformly, not a suffix of them.

- **Reduced claim that holds**: *`--update-baselines` is a deliberate no-op outside the `virtual`
  lane, `docs/TESTING-HARNESS.md:410` promises otherwise without qualification, and the
  `Skipped` outcome makes a refused recording indistinguishable from a skipped comparison — so
  a developer who asks to re-record after the documented `virtual`→`attach` fallback exits green
  with nothing written and no message saying so.*

---

### C8 (= F1-7) — verdict: SURVIVES

- **Confidence**: high — proven with a focused test (written, run, reverted).

- **Reasoning**:
  1. `RunDirectory::record` writes `command` entries as
     `{"at", "kind":"command", "request", "response"}` — **no `data` object**
     (`crates/fleet-harness/src/rundir.rs:113-121`). Confirmed against a real run's `run.jsonl`
     from the C5 repro:
     ```json
     {"at":"…","kind":"command","request":{"id":2,"cmd":"quit","args":{}},"response":{…}}
     ```
  2. `JournalEntry::line()` is `self.data.as_ref()?.get("line")?…` (`report.rs:170-173`), so it
     is `None` for every `command` entry — the dominant kind.
  3. `align`'s guard is `entry.line().is_none_or(|line| line == step.line)` (`report.rs:239`),
     which `None` satisfies. `trustworthy = paired.is_some()` (`report.rs:240`) therefore never
     goes false while entries remain, and the pairing shifts by one from the first dropped line
     to the end of the run — exactly what the doc comment at `report.rs:145-148` promises will
     not happen ("abandoned the moment the two disagree … a misaligned one is a lie with a table
     around it").

- **Evidence** — temporary test `adversarial_c8_one_dropped_command_line_misattributes_every_later_row`:
  three `assert` lines (2, 3, 4), with **line 3's `command` entry absent** from the journal. It
  passed, i.e. the mis-attribution is exactly as predicted:

  ```text
  ## Lines
  | line | command | result | took | what happened |
  | 2 | `assert screen == Hub`      | ok | 12 ms | held |
  | 3 | `assert overlay == Dialog`  | ok | 12 ms | held |      <- line 4's verdict
  | 4 | `assert mode == Normal`     | ok | 12 ms |      |

  ## Assertions
  | line | predicate         | outcome | took |
  | 2    | `screen == Hub`   | held    | 12 ms |
  | 3    | `mode == Normal`  | held    | 12 ms |              <- line 4's predicate, under line 3
  ```

  Line 3's actual predicate was `overlay == Dialog`. It is printed as `mode == Normal`, with a
  verdict it never produced, and line 4 disappears from the Assertions table entirely. The report
  header still reads **"Passed — 3 lines in 36 ms"**.

  Stronger than the finding claims: when the journal line is *lost* rather than *malformed*, even
  the `report.rs:202` footnote ("N journal lines were malformed") does not appear — the report
  carries **no trace at all** that it is misaligned.

- **Why existing tests do not catch it**: the one test that covers drift,
  `a_journal_that_drifts_from_the_steps_stops_enriching_rather_than_inventing`
  (`report.rs:1555`), builds its journal from a `"runner"` entry — the one kind that *does*
  carry `data.line` (`rundir.rs:124-131`) — so it exercises the only branch where the guard is
  live, and never the `command` kind that makes up almost every line of a real scenario.

---

### C9 (= F1-14) — verdict: SURVIVES NARROWED

- **Confidence**: medium-high on the defect, low on the specific trigger.

- **Reasoning**: `pngs()` swallows fallible calls twice, and both are the `let _ =`-on-a-fallible-call
  pattern that `CLAUDE.md`'s non-negotiables forbid:
  ```rust
  // crates/fleet-harness/src/report.rs:1228
  let Ok(mut entries) = tokio::fs::read_dir(directory).await else { return Vec::new(); };
  // crates/fleet-harness/src/report.rs:1232
  while let Ok(Some(entry)) = entries.next_entry().await {
  ```
  `pngs()` is the sole source of `SuiteScenario::shots` (`report.rs:757`), which feeds both the
  failure block (`report.rs:856-870`, the `failure-NNN.png` inlining) and the suite screenshots
  section (`report.rs:908-935`). A truncated or empty result therefore silently drops a failed
  scenario's evidence from the suite report while everything else still renders.

- **Evidence** — temporary test
  `adversarial_c9_a_shots_directory_that_cannot_be_read_lists_nothing_and_says_nothing`, which
  passed: a `shots/` directory containing `failure-002.png`, chmod `0o000`, →
  `pngs()` returns `[]` with no error, no log line, no marker on the report.

- **Honest limit — this is the narrowing**: I could **not** construct a case where
  `next_entry()` itself returns `Err` on a local filesystem. `EACCES` is raised by `read_dir`
  (line 1228), not by iteration; deleting the directory mid-walk makes `getdents` return EOF,
  not an error. The `next_entry()` `Err` at line 1232 is reachable in principle (a failing
  block device, a stale NFS handle, an interrupted `spawn_blocking`) but I have no repro for it.

- **Reduced claim that holds**: *`pngs()` cannot distinguish an IO failure from an empty
  directory at either of its two fallible calls, and a failure at the reachable one
  (`read_dir`, `report.rs:1228` — e.g. an unreadable `shots/`) makes the suite report omit a
  failed scenario's `failure-NNN.png` from both the failure block and the screenshots section
  with nothing said about it. The `next_entry()` variant at `report.rs:1232` is the same defect
  class and the same silent-omission consequence, but I could not trigger it.*

- **Why existing tests do not catch it**: no test calls `pngs()`; the suite-report test
  (`report.rs:1480-1552`) writes readable `shots/` directories and only asserts on what *is*
  rendered.

---

### C10 (= F1-13) — verdict: SURVIVES

- **Confidence**: high — proven with a focused test (written, run, reverted).

- **Reasoning**:
  1. `baseline::diff_path` writes the diff **into the shot's own directory**:
     `shot.with_file_name(format!("{stem}-diff.png"))` (`baseline.rs:533-536`), i.e.
     `shots/NNN-name-diff.png`. `docs/TESTING-HARNESS.md:406` freezes that name.
  2. `pngs()` returns every PNG under `shots/` (`report.rs:1226-1240`), including the diff.
  3. The suite `shots_section` skips only the `failure-` prefix
     (`is_failure_evidence`, `report.rs:1203-1206`) at `report.rs:912`, `918` and `925` (`SuiteReport::shots_section`, `report.rs:908`), then
     inlines each remaining file bare.
  4. `baselines.get(file_name(shot))` (`report.rs:930`) is keyed by the shot the comparison was
     made against (`baseline_notes`, `report.rs:1101-1109`), so the lookup for
     `NNN-name-diff.png` misses and the diff is rendered with **no caption and no verdict**,
     while `render_baseline` (`report.rs:1092-1098`) has already inlined the very same file
     under the real shot.
  5. Sort order makes it worse than the finding says: `pngs()` sorts by path, and `-` (0x2D)
     precedes `.` (0x2E), so `003-help-diff.png` sorts **before** `003-help.png`. The magenta
     diff is the *first* image a reader meets, presented as a screenshot of Fleet.

- **Evidence** — temporary test
  `adversarial_c10_the_diff_image_is_inlined_twice_in_the_suite_report` (asserts the diff's
  markdown appears exactly twice) passed. The generated suite `report.md`, verbatim:

  ```markdown
  ## Screenshots

  **scenarios/hub/help.scenario**

  ![001-help/shots/003-help-diff.png](001-help/shots/003-help-diff.png)

  ![001-help/shots/003-help.png](001-help/shots/003-help.png)

  **baseline: 812 of 1024000 pixels differ**

  ![001-help/shots/003-help-diff.png](001-help/shots/003-help-diff.png)
  ```

- **Why existing tests do not catch it**: the only suite-report test that involves a diff
  (`a_shot_that_blew_its_baseline_budget_shows_the_verdict_and_the_diff`, `report.rs:1434`) writes the diff as `002-hub.diff.png` — a **dot**, not the hyphen
  `baseline::diff_path` actually produces — and it asserts on that spelling at `report.rs:1452`; it also exercises the single-run report, which is
  safe because `RunReport::shots_section` (`report.rs:553`) walks `self.steps` rather than the
  directory. The one test naming a diff therefore uses a filename the runner never writes.

---

## Repo state

`git status --porcelain` after this work:

```text
 M crates/fleet-harness/src/agent/tests.rs      <- another validator's concurrent edit, untouched
 M crates/fleet-harness/src/baseline.rs         <- another validator's concurrent edit, untouched
?? plans/finding_1.md
?? plans/finding_2.md
?? plans/validate/
```

`crates/fleet-harness/src/report.rs` — the only file this validation modified — was reverted with
`git checkout --` and is identical to `HEAD`.
