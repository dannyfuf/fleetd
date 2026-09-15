# Fleet e2e harness — Phase 4: agents, fixtures and fault injection — Tracker
> Plan: ./fleetd-e2e-harness-2026-09-11-phase-4-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [~] Phase 3 is done and its tracker is closed; pointer scenarios pass — pointer scenarios
      pass in the `virtual` lane; the headless lane cannot run the keyboard half (phase-3
      tracker, Deviations).
- [x] I have read `docs/NATIVE-AGENTS.md` and the two wire references under `docs/research/`.
- [x] I have run `make lint` and `make test` once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [ ] P4-T01 — Promote the mock peer to `test-support` — not exercised end to end by this
      stage; the scripted agent path below goes through `fleet-harness agent`, not the peer.
- [x] P4-T02 — The scripted agent binary and its transcript format — verified: with **no
      `codex` or `claude` on PATH, no network and no token**, a native Codex thread reached a
      real permission gate from `transcripts/edit-approval.json`, and the status line in the
      screenshot reads `scripted-sonnet`, which is the transcript's own `model` answered
      through Codex's `model/list`. It is the real protocol, spoken by the harness.
- [x] P4-T03 — The fixture builder — verified: `agents` seeds a context, `acme/api`, a
      worktree `agent` and both providers; `one-repo` seeds a real local git repo whose
      `state.json` names a real worktree path. Both render correctly (screenshots opened).
- [x] P4-T04 — Fault injection — verified end to end below.
- [~] P4-T05 — Job, toast and PR injection — half verified 2026-09-12 (docs stage). A `job
      failure` line on the `busy` preset submits a real daemon job that reaches
      `jobs[0].status == "failed"` and appears in `lists.jobs` as `Clone fixture/unreachable` with
      a `failed` badge and a `retryable` mark, so the job half works through the run's own client.
      **The toast half does not**: `toasts` stayed `[]` for the 20 s that followed. §4 of the
      frozen doc says `busy`'s toasts come from exactly this line, so either the app does not
      toast job failures or the notification never reaches the mirror. PR injection was not
      exercised at all.
- [~] P4-T06 — App-side clock control — the command is verified, its effect is not.
      `advance 5000` answers `{"millis":5000,"changed":false}` in a live `virtual`-lane run
      (2026-09-12, docs stage), which is the documented shape. `changed: false` is correct there —
      nothing had a live dwell — but it means no run has yet shown `advance` expiring a real toast,
      and it cannot until P4-T05's toast half works. The two are the same ticket.
- [x] P4-T07 — Freeze the scenario grammar and document it — `docs/TESTING-HARNESS.md` §2 is
      frozen and every line this stage wrote parsed against it unmodified. Completed 2026-09-12
      (docs stage): §1 and §2 together carry every command, every predicate operator, the fixture
      presets (§4), the target naming scheme (§3), the transcript format (§5) and the run-directory
      layout (§6), and the document is marked FROZEN in its own first line.
      `docs/NATIVE-AGENTS.md` §6.5 describes how an agent surface is exercised from a transcript —
      the two frames the scripted player emits, and the scenario shape that drives a gate from
      `click agents.composer` to `click agents.approval.allow_once` — so the two documents describe
      one mechanism rather than two.

## Notes / decisions log
(Append-only. Date-stamp entries. Capture anything that surprised you or that future-you will want.)

- 2026-09-11 — The repo's existing policy on agent tests is worth keeping verbatim, from
  `crates/fleet-daemon/tests/agents_real_binaries.rs`: the default test run spawns neither `claude`
  nor `codex`, because "a test suite that needs two vendor CLIs installed is a test suite that
  silently skips on the machine that most needs it". The harness inherits that rule — scripted by
  default, real binaries opt-in and token-free.
- 2026-09-11 — `TODO.md` lists three known native-agent gaps (Codex approval shows paths not a diff;
  `^s e` draws nothing on Codex; the amber dot returns after restart). They make good first
  scenarios: each has a definite current state and a definite fixed state.

## Follow-ups
(Things discovered mid-flight that are out of scope for this plan. Each gets a one-line description.)

- **The approval scenario is green while the app shows a persistent error.** Throughout the
  run `sticky_error` reads `filesystem operation failed for <home>/agents: native-agent storage
  failed: apply native-agent event 4: event targets the wrong turn: 907d7ccc-…`, and
  `fleetd.log` repeats `dropped invalid native-agent provider event … unknown item:
  59a40caf-…` plus `dropping a Codex settlement for a turn that is not the running one`. Either
  `edit-approval.json` omits framing Codex really sends (the item that the `file_change` and
  its approval both refer to, and a turn boundary before the approval), or fleetd's Codex turn
  bookkeeping is wrong. Until it is settled, no scenario should assert `sticky_error absent`.
- **`agents.approval` shows the gate id, not the diff.** The card in `022-approval.png` reads
  `codex wants to edit` over the bare UUID `a334787d-…` with the summary underneath. This is
  `TODO.md`'s known "Codex approval shows paths not a diff", now with a concrete cause: the
  `itemId` the approval joins on was never introduced, so there is nothing to show.
- **The gate id leaks into the composer.** After the approval arrives, the composer input
  contains `a334787d-1136-5e1e-99b1-fcd860752345` — visible in the screenshot. Nothing typed it.
- **A new agent thread does not focus its composer.** `focused` is `agents.tabs.tab[3]`, and a
  `type` sent straight after `ctrl-s A` is accepted (`ok`) and dropped. Every agent scenario
  must `click agents.composer` first; that is a workaround for an app-side focus bug.
- **The hermetic PATH has no `lsof`**, so the daemon logs `failed to refresh terminal process
  observations: command failed: lsof: No such file or directory` on every maintenance cycle.
- **`ctrl-q` aborts the process.** Sending `key ctrl-q` tears `fleet` down and then kills it with
  `SIGABRT` and `cannot access a Thread Local Storage value during or after destruction`, on a
  healthy Hub and behind the daemon banner alike. The harness `quit` command exits cleanly, so no
  scenario needs `ctrl-q` — but the defect is real, it is app-owned, and it is the same teardown
  ordering as the "runner ignores the app's exit status" item in the phase-2 tracker.
- **`fleet-harness` takes ~34 s to tear down after SIGINT.** Measured: SIGINT at T+12 s, the
  `failed: interrupted; tearing the run down` journal entry at T+47 s. Throughout that window
  the runner, `fleet`, `fleetd` and the isolated output are all alive, and a second SIGINT does
  not shorten it. See Deviations for why that matters.

- Remote machines: `docs/REMOTE-MACHINES.md` and `crates/fleet-daemon/tests/remote_*.rs` suggest a
  loopback-link GUI scenario is achievable; not in this phase.

- 2026-09-12, fix round — **`daemon restart` left a fleetd the runner did not own.**
  `fault::start_replacement` dropped the new child, and `Daemon::drop` returned early because
  `exited` was still true from the orderly stop, so an interrupted restart scenario leaked a
  daemon holding the run's socket and pid lock. `Daemon::adopt` installs the replacement and
  clears `exited`; the spawn also carries `kill_on_drop`.
- 2026-09-12, fix round — **`mockpeer` is back behind `#[cfg(test)] pub(crate)`.** Both module
  comments claimed `fleet-harness` builds the same peer; `crates/fleet-harness` has no
  `fleet-daemon` dependency and no crate outside `fleet-daemon` names `MockPeer`, so the `pub`
  widening, `test-support = ["dep:tempfile"]` and the optional `tempfile` were all dead. The GUI
  harness scripts *providers* from the outside with its own `agent` subcommand.
- 2026-09-12, fix round — **an answered Codex gate still does not reliably settle its turn.**
  Reproduced today on `agents/edit-approval-allow.scenario` in the headless lane: `fleetd.log`
  reads *"dropped invalid native-agent provider event … event targets the wrong turn"* followed by
  a cascade of *"unknown item"*, and Fleet's sticky-error slot carries it for the rest of the run
  while the scenario passes. `agents/unread-mark.scenario` fails intermittently at its
  `pending_gate == permission` line for the same reason. This is a native-agent adapter defect the
  harness found — the duplicate `TurnStarted` that `codex::map::turns` is meant to suppress — not
  a harness defect, and it belongs to whoever owns `docs/NATIVE-AGENTS.md`. Recorded in
  `docs/TESTING-HARNESS.md` §11 and `scenarios/agents/README.md`.

## Deviations

- 2026-09-11 — **The contracts stage froze the scenario grammar, the fixture names and the
  transcript shape before this phase ran** (`docs/TESTING-HARNESS.md` §2, §4, §5), which is what
  P4-T07 had been going to do at the end of it. The freeze is what let the fixture, fault and
  scripted-agent stages work in parallel; P4-T07 became "confirm the frozen text is true and
  document the agent side of it" rather than "decide it".
- 2026-09-12 — **The transcript format grew two additive pieces during implementation**, both of
  which §5 now carries: `{"type":"end_turn", …}`, because a flat step list otherwise cannot say
  where one turn stops and both protocols have exactly one turn terminal; and optional top-level
  `session_id` / `thread_id` / `model` / `context_window` plus an optional `output` on a
  `tool_call`, so two scripted threads in one scenario can be told apart. "Optional data may be
  added" allows both; no shipped field changed meaning.
- 2026-09-12 — **An `approval` step must immediately follow the `file_change` it gates.** Neither
  provider's approval frame carries a diff — Claude reads it from the `Edit` input, Codex joins by
  `itemId` — so the pairing has to be positional. This is a constraint the plan did not anticipate.
- 2026-09-12 — **P4-T01 was not needed for P4-T02.** The scripted agent goes through the
  `fleet-harness agent` subcommand on the run's `PATH`, not through the daemon's mock peer, so
  promoting the mock peer to `test-support` is still open and is no longer on the critical path.
- 2026-09-12 — **`agents.approval.deny_and_stop` was added to the frozen target list.** §3 named
  four approval controls; the decision drawer paints five (`y`/`a`/`n`/`e`/`esc`), and the fifth
  needed a name rather than a silent gap. Additive, so the freeze holds.
