# Test-harness review fixes — Phase 4: a run directory and report that keep their evidence — Plan
> Tracker: ./harness-review-fixes-2026-09-14-phase-4-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

A harness run's output is its product: a run directory holding a JSONL journal, screenshots, dumps
and logs, plus a generated report a human reads. A two-reviewer adversarial review found thirteen
defects in that machinery. None of them blocks another, and most are latent today, but together they
describe a run directory that can be shared by two runs (with the second's teardown unlinking the
first's live socket), a trustworthiness guard that never fires because it reads a field nothing
writes, a report that inlines a magenta diff image ahead of the real screenshot, a socket-length
guard that measures the wrong socket, and a hand-rolled PNG decoder that aborts the process on a
crafted IHDR instead of returning the named error its own contract promises.

This phase closes all thirteen. It is the long tail: thirteen independent fixes, each roughly an
hour, most of them latent, none of them blocking.

## Sizing call

**Phased**, and this is phase 4 of 4 — see
[the roadmap](./harness-review-fixes-2026-09-14-roadmap.md). One P2 and twelve P3. It is the largest
phase by task count and the smallest by risk, and it is one phase rather than three because no task
in it depends on another: batching independent hour-long fixes is right, and splitting them into
per-issue phases would produce trackers nobody opens. It is also the phase most amenable to being
picked up in several sittings — the tracker's one-task-in-progress rule is what keeps that honest.

## Repository context

- **Project type:** Rust, one Cargo workspace, `resolver = "3"`, edition 2024, toolchain pinned by
  `rust-toolchain.toml`. Twelve crates under `crates/`.
- **Lint command:** `make lint` = `cargo fmt --all -- --check` plus
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **Test command:** `make test` = `cargo build -p fleet-daemon -p fleet-app -p fleet-harness`, then
  `cargo test --workspace` with `FLEET_DAEMON`, `FLEET_APP` and `FLEET_HARNESS_BIN` pointed at
  `target/debug/`.
- **Build profile matters here.** `Makefile:10` sets `RELEASE ?= 0`, there is no
  `[profile.release]` section and no `.cargo/config.toml`, so `make harness` builds `dev` with
  `overflow-checks` **on**. That is why `I20` is an abort rather than a wrap.
- **The machinery:** `crates/fleet-harness/src/rundir.rs` (run directory, journal `record`),
  `report.rs` (the generated report, `pngs()`, `align`), `baseline.rs` (the hand-rolled PNG codec
  and the screenshot comparison), `env.rs` (socket paths), `fault.rs` (`daemon restart`),
  `fixture/tools.rs` (the fake `gh`/`acli`), `fixture/jobs.rs` (`job success`).
- **Run directories** live under `/tmp/fleet-harness` and are never pruned automatically;
  `make harness-prune` (default `SWEEP_DAYS=7`) is the opt-in broom. A suite writes about five
  megabytes per scenario.
- **The contract:** `docs/TESTING-HARNESS.md` §6 covers the run directory and baselines, §8 the
  report ("the journal is the complete record"), §11 the known gaps.
- **`scenarios/baselines/virtual/` is empty** — no baseline has ever been recorded (§11) — which is
  precisely why `I24`, `I25` and `I26` are latent: each needs an existing baseline to fire.
- **The `virtual` lane needs a live, unlocked Hyprland session and a hand-exported environment.**
  `export XDG_RUNTIME_DIR=/run/user/1000`, probe each directory under `/run/user/1000/hypr/` for the
  signature whose socket actually answers `hyprctl monitors -j`, export it as
  `HYPRLAND_INSTANCE_SIGNATURE`, and `export WAYLAND_DISPLAY=wayland-1`.
- **Skills to load before editing** (`CLAUDE.md`'s table): `rust-workspace-architecture` for the
  error-handling policy (`I27`'s swallowed error is a `let _ =` wearing a `while let`),
  `rust-async-background-work` for `I15`'s process race, `rust-gpui-testing` for every test added
  here, `zed-quality-review` before calling the phase done.
- **Prerequisite:** `P1-T01` must be committed before this phase starts, or `make test` cannot pass.

## Assumptions

- The narrowings in `plans/issues.md` are authoritative, and several of them make the *fix* smaller
  than the headline implies: `I15`'s `kill()` half is unreachable, `I20`'s release-build half is
  refuted, `I21`'s magnitude is 1032:1 rather than "hundreds of gigabytes", `I24` loses only the
  geometry exchange, `I25` reduces to one unqualified doc sentence plus an indistinguishable
  outcome, and `I27`'s reachable swallow is at `report.rs:1228` rather than `:1232`.
- `I16` is a robustness fix, **not** a security fix. The only input is the operator's own
  `--run-dir` and there is no privilege boundary; the real bug is an apostrophe in a home directory
  breaking every `gh` invocation.
- `I20`/`I21` are P3 because nothing decodes PR-supplied pixels automatically: `scenarios/baselines/`
  holds only `README.md` and `.gitkeep`, and there is no `.github/workflows` at all. What justifies
  fixing them is the contract — `read_image`'s own doc at `baseline.rs:594` promises a named
  `anyhow` error, and every other malformed shape in the decoder returns one.
- `scenarios/baselines/virtual/` stays empty through this phase. If anyone records a baseline
  mid-flight, `P4-T08` and `P4-T10` stop being latent and must land before the next corpus run is
  believed.

## Out of scope

- Recording screenshot baselines, and the §11 compositor work that would make recording one
  possible.
- Adding CI, and adding a fuzz target for the PNG decoder (noted as a follow-up under `P4-T07`).
- `catch_unwind` around the scenario runner. `P4-T07` removes the one known panic; the general
  problem — a panic anywhere skips `stage.teardown()` and `report::write_report`
  (`scenario.rs:350`, `:378`) and loses the run directory — stays open.
- Splitting `fleet-harness/src/{report,baseline,scenario}.rs`, all three of which are past the
  ~900-line rule (§11 records the split as a deliberate future refactor).
- Adding a `report` subcommand to `main.rs`. Its absence is load-bearing for `I9`'s narrowing.
- Phases 1–3.

## Affected areas

Single repository (`fleetd` workspace).

- `crates/fleet-harness/src/rundir.rs` — `LONGEST_SOCKET_NAME` (`:29`), the default root name
  (`:55`), `record` (`:113`).
- `crates/fleet-harness/src/report.rs` — `align`'s guard (`:239`), the footnote (`:202`), `pngs()`
  (`:1227`, `:1228`, `:1232`), the failure and screenshot sections (`:912`, `:918`, `:925`), the
  drift test (`:1555`) and the diff-name test (`:1434`).
- `crates/fleet-harness/src/baseline.rs` — the lane gate (`:343`), `compare`'s `read_image`
  (`:385`), `update_baseline` (`:508`), the diff filename (`:535`), `read_image`'s contract
  (`:594`), the IHDR multiply (`:733`), `with_capacity` (`:739`), the first scanline slice (`:745`),
  `zlib_decompress` (`:1053`), the back-reference copy (`:1125`), the lane test (`:1723`).
- `crates/fleet-harness/src/env.rs` — the app socket at `<root>/fleet-harness.sock` (`:117`).
- `crates/fleet-harness/src/fault.rs` — `restart()` (`:199`).
- `crates/fleet-harness/src/fixture/tools.rs` — the `data='@DATA@'` templates (`:77`, `:101`).
- `crates/fleet-harness/src/fixture/jobs.rs` — `inject`'s hardcoded ordinal (`:92`).
- `crates/fleet-harness/src/scenario.rs` — the line number to thread into `record` (`:488`), the
  `shot` `ensure!` (`:842`).
- `crates/fleet-drive/src/lib.rs` (`:5-8`) and `crates/fleet-drive/Cargo.toml` (`:24`).
- `docs/TESTING-HARNESS.md` — §6 (`:410`), §8.

## Tasks

### P4-T01 — Give the journal's command exchange a line number and tighten `align` (I9)

- **Intent:** make the report's trustworthiness guard actually guard, so a lost journal line cannot
  silently mis-attribute every later row.
- **Touches:** `crates/fleet-harness/src/rundir.rs`, `crates/fleet-harness/src/report.rs`,
  `crates/fleet-harness/src/scenario.rs`.
- **Steps:**
  - Confirm the defect: `report.rs:239`'s guard is
    `entry.line().is_none_or(|line| line == step.line)`, and `JournalEntry::line()` reads
    `data["line"]` — but `RunDirectory::record` (`rundir.rs:113`) writes `command` entries with
    `at`/`kind`/`request`/`response` and **no `data` object**. `command` is the kind covering almost
    every scenario line, so `line()` is always `None` and `trustworthy` never goes false.
  - Understand the demonstrated consequence: drop line 3's `command` entry and the report prints
    line 4's predicate and verdict under line 3, drops line 4 from `## Assertions`, and still says
    "Passed — 3 lines". A *lost* line leaves no trace at all, not even the `report.rs:202` footnote.
  - Note the narrowing, so the fix is not oversold: the impact chain has **no known trigger today**.
    `main.rs` has no `report` subcommand, so the journal is always parsed by the process that wrote
    it, and `append` is append-only and single-task, so a kill can only lose the tail — which sets
    `trustworthy = false` at the last row without misaligning anything earlier. The dead guard is
    real; the route to a dropped *middle* line is not established.
  - Thread the line number from `scenario.rs:488` into `rundir.rs:113` so `command` entries carry
    `"data": {"line": line}`.
  - Tighten the filter to `entry.line() == Some(step.line)` so a missing line number is a mismatch,
    not a pass.
  - Note that the existing drift test (`report.rs:1555`) uses a `runner` entry — the one kind that
    already carries `data.line` — which is why it passes today. Add a test using a `command` entry.
- **Verification:** the new test plus the existing drift test via `cargo test -p fleet-harness`;
  `make test`; then `make harness` and open a generated report to confirm rows still align and the
  footnote still appears where it should. Paste a report excerpt into the tracker.
- **Done when:** a `command` entry carries its line number, a missing one marks the run untrustworthy,
  and both are pinned by tests.

### P4-T02 — Derive the socket-length guard from `HarnessEnv` (I13)

- **Intent:** make the guard measure the longest socket a run actually creates.
- **Touches:** `crates/fleet-harness/src/rundir.rs`, `crates/fleet-harness/src/env.rs`.
- **Steps:**
  - Confirm the arithmetic: `rundir.rs:29`'s `LONGEST_SOCKET_NAME = "home/fleetd.sock"` is 16 bytes
    and is documented as the longest socket path a run creates, but `env.rs:117` puts the app's
    socket at `<root>/fleet-harness.sock`, which is 18. The uncovered window is exactly
    `len(root) ∈ {89, 90}`.
  - Understand the failure it lets through: neither socket has a fallback (only PTY sockets do), so
    in that window the guard passes, `fleetd` starts, and Fleet's own `bind` fails with
    `ENAMETOOLONG`, surfacing 60 s later as "Fleet did not open … see app.log" — precisely the buried
    failure the guard exists to prevent.
  - Derive the constant from `HarnessEnv`'s two socket names rather than restating one of them, so a
    rename cannot desynchronise them.
  - Add a test that asserts the derived constant equals the longer of the two names, and a test at
    `len(root) == 89` that the guard refuses.
- **Verification:** `cargo test -p fleet-harness <test names>`; `make test`. The failure needs a
  90-byte run root, which `make harness` will not produce, so the tests are the verification.
- **Done when:** the guard is derived from the two socket names and refuses at `len(root) ∈ {89, 90}`.

### P4-T03 — Make run-directory names collision-proof (I14)

- **Intent:** stop two runs started in the same second from sharing a directory and killing each
  other's daemon.
- **Touches:** `crates/fleet-harness/src/rundir.rs`.
- **Steps:**
  - Confirm the mechanism: `rundir.rs:55` names the default root `<UTC seconds>-<stem>` and
    `create_dir_all` treats an existing directory as success. `run_id()` is deliberately *not* used
    for the directory name — its own doc comment says why, so read that before changing anything.
  - Understand why the flock does not save it: the lock converts the collision into "run 2 fails
    **and** its `Daemon::drop` unlinks run 1's live socket", killing a healthy run. The second run
    failing would be acceptable; taking the first one down with it is not.
  - Fix by making the name unique: include the pid or millisecond precision, or `create_dir` (not
    `create_dir_all`) and retry with a suffix on `AlreadyExists`. Prefer the `create_dir` retry — it
    is the only option that cannot collide rather than merely colliding less often.
  - Whatever you choose, make the failure path safe too: a run that does not own a directory must not
    unlink anything in it. Check `Daemon::drop` and the teardown path for other unlink sites with the
    same assumption.
  - Add a test that two runs starting in the same second get distinct directories, and a test that a
    run which failed to claim a directory unlinks nothing.
- **Verification:** `cargo test -p fleet-harness <test names>`; `make test`; then start two
  `make harness-one` runs back to back (a shell loop of two, no sleep) and confirm both complete and
  both run directories exist. Paste the two directory names into the tracker.
- **Done when:** two simultaneous runs get distinct directories and neither can unlink the other's
  socket.

### P4-T04 — Hold the daemon seal across `restart()`'s replacement spawn (I15)

- **Intent:** close the window in which `daemon restart` leaves a daemon the runner does not own.
- **Touches:** `crates/fleet-harness/src/fault.rs`.
- **Steps:**
  - Read the narrowing first, because it changes both the scope and the claim. The `kill()` half is
    effectively **unreachable** — there is no `.await` between reap and `seal()`, the window is
    microseconds, and the app is not yet attempting a restart. Do not touch it.
  - The `restart()` half (`fault.rs:199`, between `unseal()` and `start_replacement()`) is a real
    ~1–5% race per line, but its consequence is weaker than the finding claims: `wait_until_ready`
    succeeds against the app-spawned daemon and teardown still stops it via the socket. What actually
    breaks is `Daemon::adopt`'s documented "cannot outlive the runner" guarantee.
  - Fix it: spawn the replacement before unsealing, or hold the seal across the spawn. Pick whichever
    keeps `Daemon::adopt`'s guarantee literally true and say which in the code.
  - Add a test if the race can be made deterministic; if it cannot, run the scenario that exercises
    it in a loop and record the pass rate before and after, which is the honest evidence for a
    1–5% race.
- **Verification:** `cargo test -p fleet-harness <test name>` if a deterministic test exists;
  otherwise `make harness-one SCENARIO=<the daemon-restart scenario> LANE=virtual` run at least
  50 times, with the before/after pass rates pasted into the tracker. `make test`.
- **Done when:** `Daemon::adopt`'s "cannot outlive the runner" guarantee holds across `restart()`,
  with either a test or a before/after pass rate as evidence.

### P4-T05 — Quote the substituted path in the fake `gh`/`acli` (I16)

- **Intent:** stop an apostrophe in a home directory breaking every `gh` invocation in a run.
- **Touches:** `crates/fleet-harness/src/fixture/tools.rs`.
- **Steps:**
  - Confirm the sites: `fixture/tools.rs:77` and `:101` substitute `data='@DATA@'` with no quoting
    guard, pasting an unquoted path into a single-quoted shell literal.
  - Note the framing, and put it in the commit message: this is **robustness, not security**. The
    only input is the operator's own `--run-dir`, there is no privilege boundary, and
    `plans/issues.md` says so explicitly. The real bug is an ordinary apostrophe.
  - Note the precedent: `agent/launcher.rs`'s `shell_word` already refuses exactly this case for the
    agent shim, so the codebase has the right helper and two call sites that do not use it.
  - Route both templates through `shell_word`, or escape as `'\''`. Prefer `shell_word` — a second
    escaping implementation is the thing that drifts.
  - Add a test with an apostrophe in the run-directory path, asserting the generated script runs.
- **Verification:** `cargo test -p fleet-harness <test name>`; `make test`; then run one scenario
  with `--run-dir` pointing at a path containing an apostrophe and confirm the fake `gh` works.
  Paste the path used into the tracker.
- **Done when:** a run directory containing an apostrophe produces working `gh` and `acli` shims,
  pinned by a test.

### P4-T06 — Give injected jobs a per-run ordinal (I19)

- **Intent:** make `job success` usable twice in one scenario, as the frozen grammar admits.
- **Touches:** `crates/fleet-harness/src/fixture/jobs.rs`.
- **Steps:**
  - Confirm the defect: `fixture/jobs.rs:92` has one call site pinned to `0`, producing the slug
    `injected-0` every time. `assert_create_conflicts` confirms the conflict, and `repeated()` avoids
    it only by deleting each time.
  - Note that §2's frozen grammar admits `job success` twice, so a scenario that reads correct cannot
    be written today. No shipped scenario does this, which makes it latent — the test is the
    verification.
  - Thread a per-run counter into `inject`, or derive the ordinal from the existing `injected-*`
    worktrees. Prefer the counter; deriving from the filesystem makes the slug depend on cleanup
    state.
  - Add a test that two `job success` directives in one scenario both succeed with distinct slugs.
  - Consider adding a corpus scenario that uses `job success` twice, since a latent fix with no
    scenario stays latent. If you add one, it belongs under `scenarios/hub/` beside
    `jobs-panel.scenario`.
- **Verification:** `cargo test -p fleet-harness <test name>`; `make test`; and
  `make harness-one SCENARIO=<the new scenario> LANE=virtual` if you added one.
- **Done when:** two `job success` directives in one scenario produce two distinct jobs.

### P4-T07 — Bound the PNG decoder (I20 + I21)

- **Intent:** make `read_image` return the named error its contract promises instead of aborting on
  a crafted IHDR or inflating unboundedly.
- **Touches:** `crates/fleet-harness/src/baseline.rs`.
- **Steps:**
  - Confirm both reproductions. `I20`: a hand-built **69-byte** PNG (`width = height = 0xFFFFFFFF`,
    colour 6, depth 16, correct CRCs) panics at `baseline.rs:733:20` with
    `attempt to multiply with overflow` — `make harness` builds `dev` with `overflow-checks` on
    (`Makefile:10` `RELEASE ?= 0`, no `[profile.release]`, no `.cargo/config.toml`). `I21`: a 162 KB
    PNG whose IHDR says `1×1` inflates to 25.8 MB before `expand` rejects it, proving the size check
    runs only after `zlib_decompress` returns a complete `Vec` (`baseline.rs:1053`, back-reference
    copy `:1125`).
  - Read both narrowings. `I20`'s release-build "silently accepts garbage" half is **refuted** — a
    wrap needs `bytes_per_row ≥ 4 GiB`, so the first scanline slice at `:745` panics on `row == 0`
    and a release build can never silently accept garbage. The `update_baseline` (`:508`)
    reachability claim is also refuted: it decodes the fresh capture, never the stored baseline. The
    sole attacker-controlled decode is `compare`'s `read_image(baseline)` at `:385`, virtual lane
    only. `I21`'s magnitude is **refuted** — DEFLATE's ceiling is 1032:1, so 1 MB → ~1 GB, not
    "hundreds of gigabytes"; the decoder does accept the 1-bit distance code that ceiling needs, so
    1032:1 is genuinely reachable.
  - Apply the joint fix `plans/issues.md` prescribes: compute `expected` once with `checked_mul` in
    `u64`, thread it into `inflate` as a ceiling checked at each push site, and guard the
    `with_capacity` product at `:739`.
  - Return a named `anyhow` error at each new rejection, matching `read_image`'s contract at
    `baseline.rs:594` and the style of the decoder's other malformed-input errors.
  - Add tests: the 69-byte hostile IHDR, and a stream that would exceed the declared image size.
    Existing malformed-input coverage is only "not a PNG" and "truncated" — there is no hostile-IHDR
    or adversarial-stream test today.
  - Record in the tracker's Follow-ups that there is still no fuzz target for this decoder.
- **Verification:** `cargo test -p fleet-harness <test names>` — both must return errors rather than
  abort, so run them in a `dev` build where `overflow-checks` is on, which is the default;
  `make test`; then `make harness` to confirm normal captures still decode.
- **Done when:** a hostile IHDR and an over-long stream each return a named error, pinned by tests,
  and real screenshots still decode.

### P4-T08 — Record a failed `shot`'s geometry exchange (I24)

- **Intent:** keep §8's "the journal is the complete record" true when a baseline comparison fails.
- **Touches:** `crates/fleet-harness/src/scenario.rs`.
- **Steps:**
  - Confirm the defect: `scenario.rs:842`'s `ensure!(outcome.passed(), …)` runs before the exchange
    is recorded, so the app's settled geometry response (`drive.rs:331-343`) never reaches
    `run.jsonl`.
  - Read the narrowing, which strikes most of the finding: `record_event("baseline", …)` and
    `context.artifact = Some(path)` both run *before* the `ensure!`, so the image, pixel counts,
    verdict and diff all still render; `shots_section` walks `self.steps`, not `command` entries; and
    alignment survives because `"error"` is in `EXCHANGES`. **Only the geometry response is lost.**
  - Record the exchange before the `ensure!`, and let the baseline failure ride as the step error.
  - Note it is latent: the `ensure!` fires only on `Differed`, which needs an existing baseline, and
    none exist. The test is the verification.
  - Add a test that a `Differed` outcome still journals the geometry exchange.
- **Verification:** `cargo test -p fleet-harness <test name>`; `make test`. A corpus run cannot reach
  this path while `scenarios/baselines/virtual/` is empty.
- **Done when:** a failed `shot` journals its geometry exchange, pinned by a test.

### P4-T09 — Distinguish "not recorded" from "skipped" and qualify §6 (I25)

- **Intent:** stop `--update-baselines` silently recording nothing, and make §6 say where baselines
  apply.
- **Touches:** `crates/fleet-harness/src/baseline.rs`, `docs/TESTING-HARNESS.md` (§6).
- **Steps:**
  - Read the narrowing first — it reduces this from a bug to a two-part honesty fix. The behaviour
    is **deliberate and test-pinned** by `nothing_is_compared_or_recorded_outside_the_virtual_lane`
    (`baseline.rs:1723`, which passes `update=true`), the restriction is stated in `check`'s own doc
    comment, and §6 scopes baselines to the virtual lane two paragraphs above the sentence the
    reviewer quoted. The fallback is not silent either — each shot journals `status: "skipped"`.
  - What is left is real: the unqualified promise at `docs/TESTING-HARNESS.md:410`, and a `Skipped`
    outcome that cannot distinguish "skipped comparison" from "refused to record".
  - Qualify §6's sentence at `:410` so it says `--update-baselines` records only in the `virtual`
    lane.
  - Add a distinct `NotRecorded { lane }` outcome beside `Skipped`, returned by the lane gate at
    `baseline.rs:343`, and surface it in the journal and report so the two cases read differently.
  - Note the fallback that makes this matter: the documented mid-run `virtual`→`attach` fallback
    (`lane.rs:737` → `scenario.rs:457`) can put a run outside the virtual lane without the developer
    choosing it, so `--update-baselines` can do nothing for a reason the operator did not pick.
  - Update `nothing_is_compared_or_recorded_outside_the_virtual_lane` to assert the new outcome
    rather than `Skipped`.
- **Verification:** `cargo test -p fleet-harness nothing_is_compared_or_recorded_outside_the_virtual_lane`
  and any new test; `make test`; then
  `make harness-one SCENARIO=scenarios/hub/help.scenario LANE=headless HARNESS_ARGS=--update-baselines`
  and confirm the report says "not recorded (lane: headless)" rather than "skipped".
- **Done when:** §6 is qualified and a run outside the virtual lane reports `NotRecorded` with its
  lane named.

### P4-T10 — Keep diff images out of the report's screenshot sections (I26)

- **Intent:** stop the suite report inlining the magenta diff as if it were a screenshot, ahead of
  the real one.
- **Touches:** `crates/fleet-harness/src/report.rs`.
- **Steps:**
  - Confirm the defect: `pngs()` (`report.rs:1227`) excludes only the `failure-` prefix, but diffs
    are written as `<stem>-diff.png` into the same `shots/` directory (`baseline.rs:535`). The
    generated suite report inlines `003-help-diff.png` twice, and because `-` sorts before `.` the
    bare, unlabelled diff appears **first**, ahead of the real screenshot.
  - Note the trap in the existing coverage: the one test naming a diff (`report.rs:1434`) spells it
    `002-hub.diff.png` — a filename the runner never writes. Fix the test's filename as part of this
    task, or it will keep passing against a fix that does not work.
  - Add an `is_diff` predicate beside `is_failure_evidence` and skip it at `report.rs:912`, `:918`
    and `:925`.
  - Decide whether the diff should appear at all — labelled, beside its screenshot — rather than
    merely disappear. If it should, say so and do it; a diff nobody can see is a regression in a
    different direction.
  - Latent until a baseline exists, so the tests are the verification.
- **Verification:** `cargo test -p fleet-harness <test names>`, including the corrected `:1434`
  test; `make test`. Construct a `shots/` directory with a `<stem>-diff.png` in the test rather than
  waiting for a baseline.
- **Done when:** a `<stem>-diff.png` no longer appears in the screenshots section, and the test that
  covers it uses the filename the runner actually writes.

### P4-T11 — Surface a directory-listing error in `pngs()` (I27)

- **Intent:** stop a `shots/` directory that cannot be read from silently dropping failure evidence.
- **Touches:** `crates/fleet-harness/src/report.rs`.
- **Steps:**
  - Read the narrowing, which relocates the defect. The finding names
    `report.rs:1232`'s `while let Ok(Some(entry)) = entries.next_entry().await` — a `let _ =` on a
    fallible call wearing a `while let` — but a validator **could not** make `next_entry()` return
    `Err` on a local filesystem. The sibling swallow at `report.rs:1228` *is* reachable and was
    proven: `shots/` at mode 000 makes `pngs()` return `[]` silently, dropping `failure-NNN.png` from
    both the suite failure block and the screenshots section.
  - Fix both, since they are the same anti-pattern and `rust-workspace-architecture` forbids it
    outright, but write the test against `:1228` — the one that reproduces.
  - Match the error and record it on the scenario so `failures_section` can say the screenshots could
    not be listed, rather than printing nothing.
  - Note the blast radius is narrower than it looks: only the *suite* report is affected — a failing
    scenario's own report finds the image by path existence.
  - Add a test with `shots/` at mode 000, asserting the report says the listing failed.
- **Verification:** `cargo test -p fleet-harness <test name>` — the mode-000 test must be written so
  it restores permissions on failure, or it will poison the target directory; `make test`.
- **Done when:** an unreadable `shots/` produces a report that says so, pinned by a test.

### P4-T12 — Drop the `regex` claim from `fleet-drive`'s feature doc (I33)

- **Intent:** remove a doc sentence promising a build-time saving that does not exist.
- **Touches:** `crates/fleet-drive/src/lib.rs`.
- **Steps:**
  - Confirm the drift: `lib.rs:5-8` says `fleet-lazygit` takes `legacy` alone so `regex` stays out,
    but `Cargo.toml:24` has `regex.workspace = true` un-gated.
  - Read the narrowing before "fixing" the manifest: the reviewer's proposed
    `optional = true` change **buys nothing**. `cargo tree` shows `gpui` pulls `regex` into
    `fleet-lazygit` on an independent normal edge, so gating it saves zero build time.
  - Drop `regex` from the doc sentence. **Doc only** — do not touch `Cargo.toml`.
  - Re-run `cargo tree -p fleet-lazygit -i regex` yourself and paste the edge into the tracker, so
    the next person who reads the sentence does not re-derive it.
- **Verification:** `cargo tree -p fleet-lazygit -i regex` shows the `gpui` edge; `make lint`.
  Verified: by inspection plus the `cargo tree` output; paste both into the tracker.
- **Done when:** `lib.rs`'s feature doc no longer claims a saving `regex` does not provide.

## Verification

Run from the workspace root, in this order:

```sh
make lint     # cargo fmt --all -- --check + clippy --workspace --all-targets --all-features -D warnings
make test     # builds fleet-daemon, fleet-app, fleet-harness, then cargo test --workspace
make harness  # the whole scenarios/ corpus in the virtual lane
```

Plus, specific to this phase:

```sh
make harness-one SCENARIO=scenarios/hub/help.scenario LANE=headless HARNESS_ARGS=--update-baselines
make harness-prune   # after the phase; a suite writes ~5 MB per scenario into /tmp/fleet-harness
```

`make harness` needs a live, **unlocked** Hyprland session with `WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR` and a *probed* `HYPRLAND_INSTANCE_SIGNATURE` exported by hand — see Repository
context. Several tasks here need a *generated report* read by eye rather than asserted on
(`P4-T01`, `P4-T09`, `P4-T10`); open one and paste the relevant excerpt into the tracker.

## Definition of done

- [ ] Every task `P4-T01`…`P4-T12` is checked off in the tracker, each with pasted verification
      output or a one-line "verified: <how>".
- [ ] `make lint` is clean.
- [ ] `make test` passes.
- [ ] `make harness` runs the full corpus green, or the tracker records exactly why it could not run
      and what was verified instead.
- [ ] `docs/TESTING-HARNESS.md` §6 (`:410`) is qualified, and §8's "the journal is the complete
      record" is true of a failed `shot`.
- [ ] Every latent fix — which is most of this phase — carries a test written with it. A latent fix
      with no test is indistinguishable from no fix.
- [ ] No `let _ =` on a fallible call, and no `while let Ok(_) =` standing in for one, remains in
      the files this phase touched (`CLAUDE.md`'s non-negotiables).
- [ ] `zed-quality-review` has been run over the phase's diff.
- [ ] The tracker reflects reality, including the `P4-T03` uniqueness strategy, the `P4-T04`
      before/after pass rates, and the `P4-T10` decision about whether diffs appear labelled.
- [ ] Follow-ups discovered mid-flight are captured in the tracker's Follow-ups section — at minimum
      the missing PNG fuzz target and the missing `catch_unwind`.

## Risks and rollback

- **`P4-T03` changes where evidence lands.** Anything that reads a run directory by its
  `<UTC seconds>-<stem>` name — a script, a habit, a doc example — breaks when the name gains a pid
  or a suffix. Grep for the pattern before committing, and check `make harness-prune`, which globs
  that directory.
- **`P4-T07` tightens a decoder every screenshot goes through.** A ceiling computed slightly too
  low rejects legitimate captures, and the corpus is currently the only thing that would notice.
  Run `make harness` and confirm every `shot` still decodes; a real 1920×1080 RGBA capture is the
  regression test that matters more than either hostile input.
- **`P4-T11`'s test manipulates filesystem permissions.** A mode-000 directory left behind poisons
  the target tree and produces confusing failures in unrelated tests. Restore permissions in a
  guard that runs on unwind, not at the end of the happy path.
- **`P4-T04` is a 1–5% race.** A single green run is not evidence. If you cannot make it
  deterministic, the before/after pass rates over at least 50 runs are the deliverable, and a task
  ticked with one green run is a task nobody verified.
- **Most of this phase is invisible to the corpus.** Seven of the thirteen issues are latent, and
  three of those need a baseline that does not exist. A green `make harness` after this phase says
  little about whether the work is correct; the unit tests are the verification.
- **No CI catches any of this.** There is no `.github/workflows` in the repo, so the Verification
  block above is the only gate.
