# Adversarial validation — batch "env/fixture/parsing" (C25–C33)

Base `881162c5c283cefb4319bbfdb79629ef31960745`, branch `test-harness`.

> **Note on `cargo test -p fleet-harness`.** It does not compile in the working tree right now, but
> not because of anything on this branch: sibling validators have uncommitted edits in
> `crates/fleet-harness/src/{agent/tests.rs,baseline.rs,report.rs}` (`git status` shows three
> modified files) and two of them do not build (`E0599` at `agent/tests.rs:1023`, `E0716` at
> `report.rs:1622`). None of the verdicts below depend on running that suite; where I needed an
> executable check I extracted the function under test into a standalone program (C29) or used
> `cargo tree` (C33), both of which ran clean.

---

### C25 (F1-9 + F2-7) — verdict: SURVIVES
- **Confidence**: high
- **Refutation attempt**: three angles, all failed.
  1. *Does `FleetHome` give the app socket a fallback?* No. `fleet-core/src/paths.rs` has a
     `sun_path` fallback (`pty_socket_dir` → `socket_fallback_dir`, lines 176–194) but it is scoped
     to **PTY holder sockets only**. `FleetHome::socket_path()` (`paths.rs:131`) is a bare
     `root.join("fleetd.sock")`, and the app's harness socket never goes through `FleetHome` at all
     — `fleet_drive::server::listen` (`crates/fleet-drive/src/server.rs:83`) calls
     `UnixListener::bind(path)` directly on whatever `FLEET_HARNESS_SOCK` names. So *neither*
     socket has a fallback; the finding's "no `FleetHome`-style fallback" phrasing is the only
     inaccurate word in it, and it does not change the outcome.
  2. *Is there a second length guard anywhere?* `grep -rn "MAX_SOCKET_PATH\|107" crates/fleet-harness
     crates/fleet-app/src/drive.rs` returns only `rundir.rs:26/67/68/69/351`. There is exactly one
     guard and it measures exactly one path.
  3. *Do the inline tests catch it?*
     `rundir.rs:350 a_run_directory_too_deep_for_a_unix_socket_is_refused_up_front` uses
     `temp_dir().join("x".repeat(107))` — far past the window — so it passes either way and pins
     nothing about which socket is longest.
- **Evidence**:
  - `crates/fleet-harness/src/rundir.rs:28-29`
    ```rust
    /// The longest socket path any run creates inside its own directory.
    const LONGEST_SOCKET_NAME: &str = "home/fleetd.sock";
    ```
  - `crates/fleet-harness/src/env.rs:117` — `socket: root.join("fleet-harness.sock"),`
    (handed to the app as `FLEET_HARNESS_SOCK` at `env.rs:165`).
  - Byte counts, measured: `len("home/fleetd.sock") == 16`, `len("fleet-harness.sock") == 18`.
    With `MAX_SOCKET_PATH = 107` the guard admits `len(root) <= 90`; the app's socket needs
    `len(root) <= 88`. **The uncovered window is exactly `len(root) ∈ {89, 90}`** — two values.
  - Consequence: `crates/fleet-app/src/drive.rs:185-189` logs
    `"harness: could not open the command socket"` and returns; the runner then waits out
    `SOCKET_TIMEOUT` and reports `crates/fleet-harness/src/scenario.rs:662`
    `"Fleet did not open {} within {SOCKET_TIMEOUT:?}; see app.log"` — verbatim the failure the
    constant's own doc comment (`rundir.rs:21-25`) says it exists to prevent.
- **Severity judgement**: **P3.** The claim is exactly true and the doc comment on line 28 is
  false as written, but the blast radius is two specific `--run-dir` lengths that nobody will hit
  by accident. The fix is a one-line `max()` over the two names, and it is worth doing precisely
  because the guard is a *promise* that is currently unkept.

---

### C26 (F2-8) — verdict: SURVIVES
- **Confidence**: high
- **Refutation attempt**: both suggested refutations tried; **both failed**, and one of them made
  the finding *worse* rather than better.
  1. *Is `run_id()` (which includes the pid) used for the directory name after all?* **No.**
     `rundir.rs:55-63` builds the default root from `stamp(utc(now))` + `sanitize(stem)` only;
     `run_id()` (`rundir.rs:100-103`) is a **separate, later** function that appends
     `std::process::id()` to the *already-chosen* directory name, and its own doc comment
     (`rundir.rs:94-99`) says plainly why: "the directory name alone is not unique". The pid is
     applied to the compositor output name and window title, never to the path.
  2. *Does the daemon pid lock already prevent the collision?* **It prevents the two-daemon race
     and nothing else — and its failure mode damages the first run.**
     `fleet-daemon/src/server/listener.rs:95` takes an exclusive `flock` on `<home>/fleetd.pid`
     and `listener.rs:96-98` additionally refuses when the socket answers, so run 2's `fleetd`
     exits with `already_running`. But run 2 has *already* written into run 1's directory by then
     (`write_scenario` overwrites `scenario.txt`; `HarnessEnv::lay_out` rewrites the shared
     `bin/gh`), and when run 2's `Daemon::start` times out, `Daemon`'s `Drop`
     (`crates/fleet-harness/src/env.rs:414`) runs:
     ```rust
     if let Err(error) = remove_if_present(&self.socket) {
     ```
     `self.socket` is `<shared root>/home/fleetd.sock` — **run 1's live socket**. Run 2's cleanup
     unlinks the socket run 1's Fleet is still using. The pid lock converts "two daemons racing"
     into "run 2 fails *and* silently breaks run 1".
- **Evidence**:
  - `crates/fleet-harness/src/rundir.rs:58-62` (name), `rundir.rs:77` (`create_dir_all`, for which
    an existing directory is `Ok(())`), `rundir.rs:240` (`stamp` → `YYYYmmdd-HHMMSS`, one-second
    resolution).
  - Suite runs collide the same way: `scenario.rs:107` timestamps the *suite* root and
    `scenario.rs:118` hangs `NNN-<stem>` under it, so two suites started in the same second share
    every scenario directory.
  - Journal interleaving is real: `rundir.rs:188-191` opens `run.jsonl` with `.append(true)`, and
    `report::align` (`report.rs:228-243`) pairs steps to entries **positionally**, with
    `trustworthy` latching false at the first mismatch — so one interleaved entry silently blanks
    the rest of the report.
  - `remove_home()` (`rundir.rs:163`) is called from `scenario.rs:391` on a clean run without
    `--keep`, and deletes `<shared root>/home` whether or not the other run is still using it.
- **Severity judgement**: **P3.** Needs two runs started inside the same wall-clock second, which
  is an operator doing something slightly unusual (two `make harness-one` in parallel, or a
  script). But the damage is to *the evidence*, which is the entire point of the run directory,
  and the failure is silent. Cheap fix (pid or millisecond in the name, or `create_dir` +
  `AlreadyExists` retry).

---

### C27 (F2-9) — verdict: SURVIVES NARROWED
- **Confidence**: medium-high
- **Refutation attempt**: I went after the window width, which is what the finding itself flags as
  the soft spot, and the two halves come out very differently.
  - **`kill()` — refuted in practice.** `crates/fleet-harness/src/fault.rs:199-219`: on the common
    path (`process.pid() == Some(pid)`, i.e. the runner's own child)
    `process.child_mut().kill().await` both signals and reaps, and then `seal(process)?` runs at
    line 214 **with no `.await` between them**. The window between "the daemon is dead" and "the
    home is sealed" is the few microseconds of a synchronous `symlink_metadata` + `create_dir`.
    For the app to lose the race it would have to already be *inside* `ensure_daemon`, past
    `Client::connect` and past `live_process`, at that exact instant — but the app has no reason to
    be there: it only reaches that path after its own 2 s health tick
    (`crates/fleet-app/src/bridge.rs:46 HEALTH_INTERVAL`) has failed, which by construction has not
    happened yet. The `else` branch (`signal`/`await_exit`, `fault.rs:486-498`) widens it to at
    most one `EXIT_INTERVAL` of 5 ms, still with no app attempt in flight.
  - **`restart()` — not refuted.** `fault.rs:228-236`: `resume` → `shutdown()` → `unseal()` →
    `start_replacement()`. Every shipped use of `daemon restart` is preceded by `daemon kill`
    (`scenarios/daemon-down.scenario:11,17`; `scenarios/daemon/reconnect-banner.scenario:36,46`;
    `scenarios/daemon/link-recovers.scenario:16,22`), so at that moment the app is *actively*
    retrying `ensure_daemon` on the 1/2/4/8/8 s backoff
    (`crates/fleet-app/src/state/connection.rs:84-87`) and failing on the seal. `unseal()` lifts the
    seal, and only then does `start_replacement` create a log directory, spawn a process and wait
    for it to boot — a window of tens to hundreds of milliseconds during which an app retry can
    win. This is a real 1–5 %-per-line race, not a theoretical one.
  - **But the consequence is milder than the finding claims.** If the app wins, `start_replacement`'s
    own `fleetd` exits with `already_running`, `process.adopt(child)` adopts that corpse, and
    `wait_until_ready()` then **succeeds** — because the app's daemon *is* serving the home and
    answers `daemon_ping`. So the scenario does not fail. At teardown, `Daemon::shutdown`
    (`env.rs:314-320`) asks for an orderly stop **through the socket**, which reaches the
    app-spawned daemon and stops it cleanly. The daemon therefore does *not* normally leak.
    What is genuinely broken is the invariant `Daemon::adopt`'s own doc comment
    (`env.rs:246-254`) states — "the process cannot outlive the runner whatever happens next" —
    since `kill_on_drop` and `Drop`'s kill now point at a reaped child. The leak materialises only
    when the runner is itself killed (`SIGKILL`, OOM, stage timeout).
- **Evidence**: `fault.rs:199` (`async fn kill`), `fault.rs:214` (`seal(process)?`), `fault.rs:235`
  (`unseal(process)?`), `fault.rs:379-386` (the `seal` doc comment naming `ensure_daemon` as the
  exact hazard), `fleet-client/src/spawn.rs:74-85` (`ensure_daemon`: connect, then `live_process`,
  then spawn — and `live_process` reads the *dead* recorded pid, so it does not block the spawn).
- **If NARROWED**: the reduced claim that holds is —
  > The seal/kill ordering is inverted, and in `restart()` the `unseal()`-before-`start_replacement()`
  > ordering leaves a tens-to-hundreds-of-milliseconds window in which the app's own reconnect
  > backoff can start a `fleetd` the runner's handle does not own. Teardown still stops it through
  > the socket, so it leaks only if the runner dies; what is reliably broken is `Daemon::adopt`'s
  > documented "cannot outlive the runner" guarantee. The `kill()` half of the finding is
  > effectively unreachable — microseconds, with no app attempt in flight.
- **Severity judgement**: **P3.** No shipped scenario fails because of it. It stays worth fixing
  because the correct ordering (seal before signalling; spawn before unsealing) is free, strictly
  safer, and the `seal` doc comment already argues for it.

---

### C28 (F1-21 + F2-10) — verdict: SURVIVES, as robustness, **not** as a security finding
- **Confidence**: high
- **Refutation attempt**: two angles, both failed.
  1. *Is the path sanitised before it reaches the template?* No. `fixture.rs:133-139` builds the
     workspace as `<run root>/fixture`, `tools.rs:29` appends `tools`, and `tools.rs:77` / `:101`
     substitute `data.to_string_lossy()` with no validation. `rundir.rs:206-225 sanitize()` — which
     *would* map `'` to `-` — is applied **only** to the generated default name (`rundir.rs:61`),
     never to a `--run-dir` the operator passes (`rundir.rs:56-57` takes it verbatim).
  2. *Does a test cover it?* `fixture/tests.rs:205
     the_fake_gh_answers_the_daemon_s_own_queries_from_fixture_data` exercises the real script, but
     from a `tempdir()` path that never contains a quote. It cannot catch this.
- **Evidence**:
  - `crates/fleet-harness/src/fixture/tools.rs:77`
    ```rust
    &GH.replace("@DATA@", &data.to_string_lossy())
    ```
    expanding into `crates/fleet-harness/src/fixture/tools.rs:197` — `data='@DATA@'` — and the same
    shape at `tools.rs:101` into `tools.rs:253`.
  - The guard that exists two modules away, `crates/fleet-harness/src/agent/launcher.rs:120-129`:
    ```rust
    fn shell_word(path: &Path) -> anyhow::Result<String> {
        let rendered = path.to_string_lossy();
        if rendered.contains('\'') {
            anyhow::bail!(
                "{} cannot be launched from a scripted agent: the path contains a single quote",
    ```
- **Severity judgement**: **P3, robustness/consistency.** Honest call on the security framing:
  **this is not a security finding.** The only input is `--run-dir`, an argument the operator types
  into their own `fleet-harness` invocation; anyone who can supply it can already run arbitrary
  commands without going through a fake `gh`. There is no privilege boundary crossed. The real bug
  is mundane and non-adversarial: a perfectly ordinary home directory (`/Users/dan/Danny's runs/…`)
  produces a `/bin/sh` script with an unterminated literal, so every `gh` call dies with a shell
  parse error and the operator is pointed at `gh` rather than at the fixture. That, plus the
  inconsistency with `shell_word` in the same crate, is the whole of it.

---

### C29 (F2-11) — verdict: SURVIVES (and is *stronger* than the reviewer argued)
- **Confidence**: high
- **Refutation attempt**: I went looking for the grammar to say `await idle exists` is illegal.
  **The opposite is true, and there is a test pinning it.**
  - `docs/TESTING-HARNESS.md:157-158` (§2): *"An atom is `<dotted.path> <op> <value>`,
    `<dotted.path> exists`, `<dotted.path> absent`, or the bare word `idle`."* `idle` is a snapshot
    object (`docs/TESTING-HARNESS.md:205` lists the `idle` field), so `idle exists` is a
    well-formed `<dotted.path> exists` atom.
  - More decisively, the **app-side evaluator implements it deliberately**.
    `crates/fleet-drive/src/predicate.rs:670` only reduces `idle` to `Clause::Idle` when it is at
    the end of an atom (`head.raw == "idle" && is_end`), and otherwise falls through to path
    parsing — and `crates/fleet-drive/src/predicate.rs:996-1011` is a test *named*
    `idle_is_a_path_when_it_is_followed_by_an_operator` asserting exactly
    `parsed("idle exists").clauses == vec![Clause::Exists { path: "idle" }]`.
    So one half of the system has a test guaranteeing the form the other half cannot parse.
- **Evidence** — I extracted `predicate_and_timeout` (`scenario.rs:1107-1127`) verbatim into a
  standalone program and ran it:
  ```
                              idle -> Ok(("idle", 5000))
                          idle 750 -> Ok(("idle", 750))
                       idle exists -> Err("invalid digit found in string")
                       idle absent -> Err("invalid digit found in string")
                  idle exists 3000 -> Ok(("idle exists", 3000))
                    overlay absent -> Ok(("overlay absent", 5000))
                overlay absent 900 -> Ok(("overlay absent", 900))
             idle.running_jobs > 0 -> Ok(("idle.running_jobs > 0", 5000))
      screen == Hub && idle exists -> Err("invalid digit found in string")
    ```
  Two details the finding did not state and that sharpen it: the conjunction form
  `... && idle exists` fails too, and **supplying an explicit timeout works around it**
  (`idle exists 3000` parses fine), which is the signature of the arm-ordering bug — `["idle", _]`
  at `scenario.rs:1121` matches the 2-element slice before `[_, "exists" | "absent", _]` at
  `scenario.rs:1122` ever gets a look at the 3-element one.
- **Severity judgement**: **P3.** The affected predicates are `idle exists` / `idle absent`, both
  of which are tautologies against a snapshot that always carries an `idle` object, so nobody is
  blocked from expressing anything useful. What earns the finding its place is the layer
  disagreement — `fleet-drive` has a test guaranteeing a form `fleet-harness` rejects — and the
  fact that the failure message is a bare `ParseIntError` ("invalid digit found in string") that
  names neither the line's real problem nor the grammar.

---

### C30 (F1-16 + F2-12) — verdict: SURVIVES
- **Confidence**: high
- **Refutation attempt**: I checked whether `error-mid-stream.json` reaches a preset by any route
  other than `include_str!`. It does not. `grep -rn "include_str" crates/fleet-harness/src/`
  returns exactly two hits, `plan.rs:455` and `plan.rs:463`, and `grep -rn "error-mid-stream"
  --include=*.rs crates/` returns only `agent/tests.rs:185,399,712,803`. `fn agents()`
  (`plan.rs:421-444`) builds exactly two `Agent` entries, Claude → `conversation()` and
  Codex → `approval()`. There is no third provider and §2's frozen `fixture:` vocabulary
  (`docs/TESTING-HARNESS.md:141`, `empty|one-repo|busy|board|agents`) has no sixth preset to hang
  one on, so the gap cannot be closed from the corpus side.
- **Evidence**:
  - `docs/TESTING-HARNESS.md:359-361`, quoted exactly:
    > The starter transcripts ship in `crates/fleet-harness/transcripts/`
    > (`two-turns.json`, `edit-approval.json`, `error-mid-stream.json`) and are what the `agents`
    > fixture embeds.
  - `ls crates/fleet-harness/transcripts/` → `edit-approval.json  error-mid-stream.json  two-turns.json`.
  - `crates/fleet-harness/src/fixture/plan.rs:455` and `:463` — the only two `include_str!` sites.
  - `scenarios/agents/blocked/error-mid-stream.blocked` exists and its header diagnoses this
    finding almost word for word: *"docs/TESTING-HARNESS.md §5 lists `error-mid-stream.json` among
    the starter transcripts … but the `agents` preset embeds exactly two documents … A scenario
    cannot reach a mid-stream error today."* The corpus already hit this once and wrote it down.
  - Secondary inaccuracy in the same area, worth folding into the same fix:
    `plan.rs:470-471`'s doc comment names
    `agent::tests::all_three_starter_transcripts_load_and_validate` as the build-time check for
    *embedded* transcripts, which is only true for two of the three.
- **Severity judgement**: **P3** by blast radius — no user-facing defect, one blocked scenario —
  but it is not optional polish under this repo's own rule (`CLAUDE.md`: "`docs/` is authoritative,
  not descriptive … When code and a doc disagree, one of them is a bug; fix both in the same
  commit"). Either append a failing third turn to `two-turns.json` or amend §5 to say two of the
  three are embedded; the `.blocked` file's header already spells out the first option in full.

---

### C31 (F1-20) — verdict: SURVIVES
- **Confidence**: high
- **Refutation attempt**: two angles, both failed.
  1. *Is "every call site pins `0`" actually true?* Yes. `grep -n "success("
     crates/fleet-harness/src/fixture/jobs.rs` returns the definition at `:107` and exactly one
     call, `jobs.rs:92` — `Injected::Success => one(success(client, fixture, 0).await?)`. The
     `ordinal: u8` parameter has been dead since it was written.
  2. *Does the daemon adopt an existing worktree instead of conflicting?* No.
     `crates/fleet-daemon/src/services/worktrees/creation.rs:378-416
     assert_create_conflicts` returns `DaemonError::Conflict` on all three of a matching id, a
     matching session/path, and an existing destination directory. (The board service does have an
     adopt path — `services/boards/worktree.rs:164` — but `jobs.rs` calls `client.create_worktree`
     directly and never reaches it.)
  - Worth noting for contrast: the sibling injection *does* get this right. `repeated()`
    (`jobs.rs:181-215`) creates **and deletes** `injected-repeat` on every iteration, which is
    exactly why repeating it is safe; `success()` creates and never deletes.
- **Evidence**: `jobs.rs:92`, `jobs.rs:107-109` (`let slug = format!("{SLUG_PREFIX}-{ordinal}")`,
  `SLUG_PREFIX = "injected"` at `jobs.rs:32`), `creation.rs:392-395`. The grammar admits the repeat:
  `docs/TESTING-HARNESS.md:145` is `job success|failure|long|repeat <count>` with no
  once-per-scenario constraint, and `scenario.rs` parses each line independently.
- **Severity judgement**: **P3, latent.** No shipped scenario writes `job success` twice, so nothing
  is broken today. It is a trap rather than a bug: a scenario that reads perfectly correct fails
  with `"inject a successful job as injected-0: … already exists"`, which points at the daemon
  rather than at the fixture. The dead `ordinal` parameter is the tell that the author saw this
  coming and did not finish the thought.

---

### C32 (F1-24) — verdict: SURVIVES NARROWED
- **Confidence**: high on the fact, medium on it mattering
- **Refutation attempt**: I tried to make §2 mean only *interior* spaces. It does not say so, but
  it does not say the opposite either — the sentence is simply not precise enough to settle it,
  which is itself the finding.
  - `docs/TESTING-HARNESS.md:117`, quoted exactly: *"Arguments after `type` and `clipboard set`
    preserve spaces."*
  - `docs/TESTING-HARNESS.md:34`, the command table row: `` `type` | `{"text":string}` | composed
    text, including spaces ``.
  - Neither line distinguishes leading, interior or trailing. What the code actually does:
    interior spaces survive (`required` at `scenario.rs:983-989` does not trim, and
    `Command::Type` takes `rest` verbatim at `scenario.rs:902`); leading spaces after the command
    word are eaten by `split_command`'s `tail.trim_start()` (`scenario.rs:981`); trailing spaces
    are eaten one level up by `let trimmed = raw.trim();` (`scenario.rs:762` — the finding cites
    768, off by six). `clipboard set` goes through the same `split_command` + `required` pair
    (`scenario.rs:1084-1089`), so it behaves identically.
- **If NARROWED**: the claim that holds is —
  > §2's "preserve spaces" is imprecise: interior spaces are preserved, but `parse` trims the whole
  > line before the argument is taken, so leading and trailing whitespace are not. `type foo␣␣␣`
  > and `clipboard set foo␣␣␣` cannot be expressed.
- **Severity judgement**: **P3 polish, and the honest fix is the document, not the code.** A
  trailing space in a committed text file is not reliably preserved by anything — editors strip it
  on save, `git diff` flags it as whitespace error, and reviewers delete it — so a scenario that
  depended on one would be fragile regardless of what the parser did. Say plainly in §2 that
  leading and trailing whitespace on a line is trimmed and that "preserve spaces" means interior
  spaces; that costs one clause and closes the gap.

---

### C33 (F1-25) — verdict: SURVIVES NARROWED
- **Confidence**: high — this one has a real refutation the reviewer missed
- **Refutation attempt**: the reviewer's *fact* is right and the reviewer's *conclusion* is wrong.
  `regex` does reach `fleet-lazygit` through `fleet-drive` — but it would reach it anyway, because
  **`gpui` depends on `regex` and `fleet-lazygit` depends on `gpui` directly**
  (`crates/fleet-lazygit/Cargo.toml:24 gpui.workspace = true`). Making `fleet-drive`'s `regex`
  optional and gating it behind `socket` would save **zero** build time for `fleet-lazygit`.
- **Evidence** — `cargo tree -p fleet-lazygit -e normal -i regex`:
  ```
  regex v1.13.1
  ├── fleet-drive v0.1.0 (…/crates/fleet-drive)
  │   └── fleet-lazygit v0.1.0 (…/crates/fleet-lazygit)
  └── gpui v0.2.2 (https://github.com/zed-industries/zed?tag=v1.18.1#bebe92f4)
      ├── fleet-drive v0.1.0 (…/crates/fleet-drive) (*)
      ├── fleet-lazygit v0.1.0 (…/crates/fleet-lazygit)
      …
  ```
  Two independent normal-dependency edges into `fleet-lazygit`; only one of them runs through
  `fleet-drive`. And `crates/fleet-drive/Cargo.toml:24` is indeed `regex.workspace = true` with no
  `optional = true`, while the `#[cfg(feature = "socket")]` gating of `predicate` and `server`
  (`lib.rs:9-18`) is correct.
- **If NARROWED**: the claim that holds is —
  > `crates/fleet-drive/src/lib.rs:5-7` is inaccurate: `regex` does not "stay out of" a
  > `legacy`-only consumer. But the accurate half of the sentence is the only half worth keeping —
  > the predicate evaluator and the server loop genuinely are excluded, and `regex` is not
  > excludable at all, because `gpui` pulls it into `fleet-lazygit` on its own. **The fix is to
  > drop `regex` from the sentence, not to make the dependency optional**; the `optional = true` +
  > `socket = ["legacy", "dep:regex"]` change the reviewer proposes buys nothing measurable.
- **Severity judgement**: **P3, documentation only.** No build-time win is available, so the
  entire content of the finding is one misleading word in a module doc comment.
