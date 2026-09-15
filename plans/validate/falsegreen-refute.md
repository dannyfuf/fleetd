# Adversarial validation — batch "false-green" (run integrity)

Base `881162c5`, branch `test-harness`. Role: refute. Every candidate was read in its whole
function with its callers; `cargo test -p fleet-harness` was run (90 passed, 1 failed — the
failure is `baseline::adversarial_tmp::c17_hostile_ihdr_overflows`, a probe another validator
left in the tree, unrelated to this batch).

Two facts recur below and are established once here:

1. **No baseline exists anywhere.** `scenarios/baselines/virtual/` is empty (`ls -R` shows only
   `scenarios/baselines/README.md`), which `docs/TESTING-HARNESS.md` §11 records as a known gap:
   *"No baseline has ever been recorded."* `Baselines::check` (`baseline.rs:354`) therefore
   returns `Missing` — never `Differed` — for every shot in the corpus, and `BaselineOutcome::
   passed()` (`baseline.rs:130`) is `!matches!(self, Self::Differed { .. })`. Anything downstream
   of a *failed* baseline comparison is latent today, not live.
2. **The journal is written and read by one process, in one run.** `main.rs` has exactly two
   subcommands, `Run` and `Agent` — there is no `report` subcommand and no way to point a
   `fleet-harness` at someone else's run directory. `report::write_report` is called at the end
   of the same `run_one` that wrote `run.jsonl`, and `RunDirectory::append` (`rundir.rs:185`)
   opens/writes/flushes one line at a time, append-only, from a single sequential task. Nothing
   else in the crate writes the journal (`grep record_event|\.record(` finds nine call sites, all
   in `scenario.rs`).

---

### C5 (F2-2) — verdict: SURVIVES
- **Confidence**: high
- **Reasoning**: I tried three ways to refute this and all three failed.

  *Does the app survive the poll?* `drive.rs:205-216` is explicit: `answer` is awaited, the
  response is pushed into `incoming.reply`, and only *then* does `cx.update(|_, cx| cx.quit())`
  run. `apply`'s own comment at `drive.rs:311` says so: "The response is written before
  `cx.quit()` runs". So at the instant `Client::send` returns, Fleet has not yet begun to quit.
  The runner's next actions are `run_dir.record(...)` (one open/write/flush on tmpfs) and
  `try_wait()`. Fleet in that window must unwind GPUI, drop the window, and release the GPU
  context. `try_wait` loses the race.

  *Is the specific regression even late enough to matter?* This is the decisive point, and it is
  worse than the finding says. `fleet-app/src/shell/root/bootstrap.rs:103-111` documents the
  exact abort this assertion exists to catch: the EGL context is torn down "in a thread-local
  destructor **after `main` returns**", and the process "aborts with *fatal runtime error: failed
  to initiate panic* instead of exiting 0 — **on every quit, in every Wayland session**". An
  abort raised after `main` returns is the last thing the process ever does; there is no timing
  in which a `try_wait()` issued microseconds after the `quit` *response* observes it.

  *Does anything else catch a bad status?* No. `teardown` (`scenario.rs:225-231`) calls
  `stop(&mut app, "Fleet", QUIT_GRACE)`; `stop` (`scenario.rs:251-268`) discards the status on
  both success paths — `Ok(Some(_status)) => return Ok(())` and `waited.map(|_status| ())`.
  Nothing reads `app.log` (`grep app.log` across the crate finds only path construction and
  report prose). `Child` is `kill_on_drop(true)` (`scenario.rs:637`), which reaps without
  inspecting. `run_one` never sees an exit status. No test covers it (`scenario.rs:1206`'s `mod
  tests` is parser/corpus only). §11 "Known gaps" does not record it.

  *Reach*: all 41 corpus scenarios end in `quit` (verified by tailing each `.scenario`'s last
  non-comment line: `41 quit`).

  Minor correction only: the finding's "the `break` never fires" is true but immaterial — `quit`
  is the last line, so the loop ends regardless. The impact claim is unaffected.
- **Evidence**:
  - `crates/fleet-harness/src/scenario.rs:561` — `match app.try_wait().context("poll the Fleet process")?`
  - `crates/fleet-app/src/drive.rs:207-213` — response sent, then `cx.quit()`
  - `crates/fleet-app/src/shell/root/bootstrap.rs:103-111` — "after `main` returns … aborts … on every quit"
  - `crates/fleet-harness/src/scenario.rs:252` / `:262` — status discarded on both paths

---

### C6 (F1-8) — verdict: SURVIVES NARROWED
- **Confidence**: high
- **Reasoning**: The mechanism is real — `ensure!(outcome.passed(), …)` at `scenario.rs:844`
  precedes the `Ok(Some(response))` return, so `execute`'s `(Err(error), _)` arm writes an
  `"error"` event and the `shot` request/response never reaches `run.jsonl`. But three of the
  four things the finding hangs on it are wrong, and the fourth is unreachable today.

  1. **The pixel evidence is not lost.** `record_event("baseline", event)` runs *before* the
     `ensure!` (`scenario.rs:839`), and `journal_event` for `Differed` (`baseline.rs:220-230`)
     carries `shot`, `baseline`, `passed:false`, `differing_pixels`, `total_pixels`, the `diff`
     path, `status` and `summary`. The shot image, the diff image and the verdict are all on disk
     and all in the journal.
  2. **`report::shots_section` does not lose anything.** The finding says it "loses its `command`
     entry for that line". It does not read `command` entries at all: `report.rs:553-576` walks
     `self.steps` filtered by `is_png(step.artifact)` and looks the verdict up in
     `baseline_notes(self.journal)`, i.e. the `baseline` events. `context.artifact = Some(path)`
     is also set *before* the `ensure!` (`scenario.rs:842`), so the failed shot is still inlined
     with its summary and its diff. §8's "Screenshots: Every `shot`, inlined, with its baseline
     verdict" is met.
  3. **The Lines row is not degraded either.** `detail` (`report.rs:520-523`) returns
     `step.error.clone()` first when a step failed, so "what happened" reads *"baseline: differs
     by N of M pixels, x% — see …"* — strictly more useful than the geometry the `command` entry
     would have carried.
  4. **Alignment is unaffected**: an `error` exchange entry *is* written for the line, so the
     one-exchange-per-step invariant `align` relies on still holds.
  5. **Unreachable today**: `ensure!` only fires on `Differed`, which requires an existing
     baseline (fact 1 above). No committed baseline exists; §11 records this.

  §8's sentence is also weaker than quoted: *"The journal is the complete record; the report is
  its digest, and it is assembled only from what is already on disk, so it cannot describe
  something `run.jsonl` does not show."* The clause is a constraint on the *report* not exceeding
  the journal — and the report here describes only things the journal and disk do show.
- **The reduced claim that holds**: the `shot` command exchange — the window geometry, frame and
  `name` the app answered (`drive.rs:331-345`) — is dropped from `run.jsonl` for a shot that
  fails its baseline, so §6's "`run.jsonl` … one JSON object per exchange" is not literally true
  for that line. Cheap to fix (`record` before the `ensure!`), P3 at most, latent until a
  baseline is committed.
- **What does not hold**: "the one exchange a reviewer most wants after a pixel regression" —
  the diff, the counts, the verdict and the image are all recorded; "`report::shots_section`
  loses its `command` entry" — that function never uses one; the P2 severity.

---

### C7 (F1-2) — verdict: SURVIVES NARROWED
- **Confidence**: medium
- **Reasoning**: The code is exactly as quoted (`baseline.rs:343-347`, lane gate before the
  `update` branch), and the fallback path does reach it with the *effective* lane: `check` is
  called with `context.lane.lane()` (`scenario.rs:837`), which is `LaneBackend::lane()` — the
  same accessor `run_one` uses at `scenario.rs:344` to record the mid-run `virtual`→`attach`
  fallback. So `--update-baselines` after a fallback really does record nothing. I could not
  refute that half.

  What I can refute is "silently" and the doc-drift claim.

  *Not silent.* Every skipped shot still journals a `baseline` event (`journal_event`'s
  `Skipped | Missing` arm, `baseline.rs:180-192`) with `"status": "skipped"`, and `report.md`
  prints `baseline: not compared in the attach lane` under each inlined shot. A developer who ran
  `--update-baselines` and reads the report sees "not compared" on every shot instead of
  "rewrote"/"unchanged", and `scenarios/baselines/attach/` stays empty. The run is not green-
  looking-like-a-record; it is a run whose report says nothing was compared.

  *Not undocumented, and deliberate.* `check`'s own doc-comment (`baseline.rs:330-333`) states the
  rule and its reason: *"Comparison and recording both run only in the `virtual` lane: `headless`
  has no pixels, and recording the developer's own decorated session from `attach` would commit a
  baseline no other machine can reproduce."* §6 scopes baselines to the lane two paragraphs
  before the sentence the finding quotes — *"**Virtual-lane** shots compare against
  `scenarios/baselines/virtual/<scenario>/…`"* — and the directory layout is
  `baselines/<lane>/…` (`baseline.rs:341`). Reading "`--update-baselines` explicitly replaces
  baselines" as promising an attach-lane recording requires ignoring the sentence that set up
  which baselines are under discussion. The "§6 says it with no lane caveat" framing is the
  weakest part of the finding.
- **The reduced claim that holds**: after the documented mid-run `virtual`→`attach` fallback, a
  developer who explicitly asked to record baselines gets a clean exit and no written baseline,
  and the only signal is a per-shot report line reading "not compared in the attach lane" rather
  than "not recorded". An explicit error (or a `NotRecorded { lane }` outcome) when `update &&
  lane != Virtual` is a real improvement, and one sentence in §6 would remove the ambiguity. P3.
- **What does not hold**: "silently records nothing" (the outcome is journalled and reported per
  shot); "§6 states flatly … with no lane caveat" (§6's baseline paragraph is virtual-lane
  scoped, and the code comments the restriction and its reason); the P2 severity.

---

### C8 (F1-7) — verdict: SURVIVES NARROWED
- **Confidence**: high
- **Reasoning**: The mechanical half is confirmed and I could not refute it. `RunDirectory::
  record` (`rundir.rs:113-121`) writes `{at, kind:"command", request, response}` with no `data`
  object, `JournalEntry::line()` (`report.rs:170-173`) is `self.data.as_ref()?.get("line")?`, so
  `line()` is `None` for every `command` entry, and `align`'s filter
  `.filter(|entry| entry.line().is_none_or(|line| line == step.line))` is a no-op for them. The
  corpus is dominated by app commands — only ~11 non-`fixture` runner lines (`daemon`/`job`)
  exist across all 41 scenarios — so in most scenarios the guard is inert end to end. The finding
  is also right that the covering test
  (`report.rs:1554 a_journal_that_drifts_from_the_steps_stops_enriching_rather_than_inventing`)
  uses a `runner` entry, the one kind that *does* carry a line.

  The impact chain is where it breaks. Mis-attribution needs a journal line dropped **in the
  middle**, and no mechanism produces one:

  - *"A newer binary can no longer deserialize a `Request`/`Response`"* — impossible. There is no
    `report` subcommand (`main.rs`); the journal is always parsed by the process that wrote it,
    seconds later, with the same types.
  - *"A partially flushed write when the runner is killed"* — `append` is append-only and
    per-line, from one sequential task, so a torn or lost write can only be the **last** line.
    A missing tail entry makes `exchanges.next()` return `None` for the final step, which sets
    `trustworthy = false` there; every earlier row was already paired correctly. No
    mis-attribution. (A SIGKILL also means no report is written at all; SIGINT is caught at
    `scenario.rs:806` and tears down through the normal path.)
  - *A failed `append` mid-run* — `record`/`record_event` are `?`-propagated at
    `scenario.rs:504-527`, so the step is not pushed either; counts stay consistent, and any
    surplus entry lands at the tail.
  - No other writer touches `run.jsonl`; the six non-`EXCHANGES` kinds (`fixture`,
    `scenario-error`, `failed`, `lane-fallback`, `baseline`) are all filtered out by
    `EXCHANGES: ["command","runner","error"]` (`report.rs:27`), and the `fixture` branch
    `continue`s without pushing a step, so the one-exchange-per-step invariant holds exactly.

  The docstring is also more careful than the finding allows: *"It is verified against the line
  number **wherever the entry carries one**"* (`report.rs:226`). It advertises a partial guard,
  not a total one — so "a lie with a table around it" is not what the code claims.
- **The reduced claim that holds**: `align`'s line-number cross-check is dead code for the
  `command` kind, i.e. for nearly every line of nearly every scenario; only the run-out-of-entries
  case and a `runner`/`error` mismatch can set `trustworthy = false`. Adding `"data": {"line": …}`
  to `record` and tightening the filter to `entry.line() == Some(step.line)` is a cheap way to
  make the guard real. P3 — a latent defence-in-depth gap.
- **What does not hold**: "one dropped journal line then mis-attributes every later report row" —
  there is no in-process mechanism that drops a *middle* line, and a dropped *tail* line degrades
  the last row to unenriched rather than misaligning anything; the two mechanisms the finding
  names (a kill mid-write, a cross-version deserialization failure) are respectively tail-only and
  impossible. The P2 severity does not survive.

---

### C9 (F1-14) — verdict: SURVIVES NARROWED
- **Confidence**: high
- **Reasoning**: The code is exactly as quoted (`report.rs:1232`), and it is a genuine violation
  of the repo's own non-negotiable ("Never `let _ =` a fallible call; use `?`, log it, or match
  it") wearing a `while let`. The line above it (`let Ok(mut entries) = read_dir … else { return
  Vec::new() }`) swallows the directory error the same way. I could not refute the rule violation.

  The impact is smaller than stated on two counts. First, `pngs()` has exactly one caller —
  `SuiteScenario::read` at `report.rs:757` — so only the *suite* report is affected. The
  single-run report's `shots_section` walks `self.steps` (`report.rs:553`) and its Failure block
  resolves `failure-NNN.png` by direct path existence (`report.rs:291`
  `exists(run_dir.shots.join(format!("failure-{:03}.png", step.line)))`), so the failing
  scenario's own report — which the suite links on the row above (`report.rs:857`) — still shows
  the evidence. Second, reachability: `read_dir`/`next_entry` on a local directory the harness
  itself created and populated moments earlier fails only on EIO or an fd exhaustion, and the
  suite report is written after every scenario has finished writing into it.
- **The reduced claim that holds**: `pngs()` conflates an IO error with end-of-directory, so the
  **suite-level** screenshot list and failure-evidence inline can be short by one or more images
  with nothing said. It should `match` and at minimum `eprintln!`. P3, as filed.
- **What does not hold**: the implication that the failure evidence is lost to the reader — the
  file is on disk, and the scenario's own report (linked from the suite row) finds it by path,
  not by directory listing.

---

### C10 (F1-13) — verdict: SURVIVES NARROWED
- **Confidence**: high
- **Reasoning**: Every mechanical link checks out, and one detail is worse than filed.
  `diff_path` (`baseline.rs:332-335`) is `shot.with_file_name(format!("{stem}-diff.png"))`, and
  the shot lives at `run_dir.shots/NNN-name.png` (`capture.rs:172`), so the diff really does land
  in `shots/`. §6's run-directory listing confirms it is meant to: `shots/ NNN-name.png,
  failure-NNN.png and NNN-name-diff.png`. `pngs()` filters only on the `.png` extension;
  `shots_section` (`report.rs:920-933`) skips only `is_failure_evidence` (a `failure-` prefix
  test, `report.rs:1204`); and `baseline_notes` is keyed by `file_name` of the *shot*
  (`report.rs:1101-1108`), so `baselines.get("NNN-name-diff.png")` misses and the diff is inlined
  with no summary under it — while `render_baseline` (`report.rs:1092`) has already inlined the
  same file under the real shot. Worse than the finding says: `found.sort()` puts
  `NNN-name-diff.png` **before** `NNN-name.png` (`-` = 0x2D sorts before `.` = 0x2E), so the
  unlabelled magenta image is rendered *first*, ahead of the screenshot it explains. The finding's
  note that the single-run `shots_section` is safe is correct — it walks `self.steps`.

  The only thing I can refute is reachability. A `-diff.png` is written only by `compare`
  (`baseline.rs:357`), reached only when `baseline.exists()`. `scenarios/baselines/virtual/` is
  empty and §11 records "No baseline has ever been recorded", so no diff image has ever been
  written and no suite report has ever rendered one. It also requires a suite run with a failing
  scenario whose baseline exists — a state a developer reaches one `--update-baselines` flag after
  the virtual lane starts working, but not one anybody can reach from `main` today.
- **The reduced claim that holds**: as soon as the first baseline is committed, a suite report of
  a run with a pixel regression inlines `NNN-name-diff.png` twice — once labelled under its shot,
  once bare and *above* it — presenting a magenta diff as a screenshot of Fleet. The fix is an
  `is_diff` predicate beside `is_failure_evidence`, applied at `report.rs:912`, `:918` and `:925`.
  P3, latent.
- **What does not hold**: nothing in the mechanism. Only "a scenario that blew its baseline budget
  renders…" as a present-tense description of what runs do today; per §11 no run can yet reach it.
