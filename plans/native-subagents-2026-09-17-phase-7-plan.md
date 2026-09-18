# Native subagents, phase 7: ADR, doctor line and cross-document audit — Plan
> Tracker: ./native-subagents-2026-09-17-phase-7-tracker.md
> KEEP THE TRACKER UPDATED. The plan is reference; the tracker is truth. Update it before you commit.

## Summary

Close the initiative. Record the load-bearing decisions in an ADR, add the one diagnostic the
feature needs (`fleet doctor` says whether the `fleet` CLI is reachable from a harness child, since a
child that cannot find it can never report), move the deferred items into `TODO.md` in its
start-in/done-when style, and audit every authoritative document against the shipped code. The
per-phase doc edits already rode with their code; this phase reconciles them and runs the full
verification set one last time, including the live harness tests.

## Sizing call

**Phased, phase 7 of 7.** See ./native-subagents-2026-09-17-roadmap.md. Trivial in code, Standard
in reading: one ADR, one doctor check, one `TODO.md` section, and a documentation audit. Kept as a
phase so the audit is a deliberate, tracked act rather than something assumed done.

## Repository context

- Rust workspace; this phase touches `fleet-daemon` (doctor) and `docs/`.
- Lint: `make lint`. Test: `make test`. Harness: `make harness`. Live: `cargo test -p fleet-daemon
  --features real-agents delegation::live`.
- ADRs: `docs/decisions/0001` to `0016`; `0014-drop-opencode-add-codex.md` is the shape to copy
  (Adopted / Built / Amends header, then "Why"). `docs/README.md` lists every ADR in a table at
  line 30.
- Doctor: `crates/fleet-daemon/src/services/doctor.rs` — `host_checks` at line 303,
  `agent_binary_check` at 435 (the pattern), the checks list at 414.
- `TODO.md` at the repo root: one numbered section per owed item with **Impact**, **Start in**,
  **Done when**. `docs/NATIVE-AGENTS.md` §13 is authoritative for status; §14 lists risks and open
  questions.
- Skills: `rust-workspace-architecture` (ADR and log lines), `zed-quality-review` for the final
  gate.

## Assumptions

- The ADR number is 0017 (0016 is the e2e harness). If another ADR lands first, take the next
  number and update the roadmap's reference.
- The doctor check resolves `fleet` using the same `PATH` the daemon hands to harness children
  (the adapters' `overrides` plus the inherited environment), reports the resolved path, and warns
  when absent with the text "subagents cannot report: `fleet` is not on the harness child's PATH".
- Nothing in this phase changes behaviour a scenario asserts, so no new scenario is added; the
  full corpus is run once as the final gate.

## Out of scope

- Any item in the deferred list: MCP wrapper, terminal-based callers beyond the accepted
  `--caller` flag, remote-host delegations, structured JSON results, the caller answering a child's
  gates, a Jobs-screen projection. They are recorded, not built.

## Affected areas

- `docs/decisions/0017-native-subagents.md` (new), `docs/README.md`.
- `crates/fleet-daemon/src/services/doctor.rs` and its tests.
- `TODO.md`, `docs/NATIVE-AGENTS.md` §13, §14, §15, `docs/UX-SPEC.md`, `docs/KEYMAP.md`,
  `docs/research/agents-contracts.md`, `docs/TESTING-HARNESS.md`, `docs/DEVELOPMENT.md`.

## Tasks

### P7-T01 — Write ADR 0017 and index it
- **Intent:** Record why a delegation is a thread plus a record, why the CLI is the spawn surface,
  why the outbox and not the bus, and the three user decisions.
- **Touches:** `docs/decisions/0017-native-subagents.md`, `docs/README.md`.
- **Steps:**
  - Header in the 0014 shape: Adopted for `fleet-core::agents`, the daemon's `services/agents/
    delegation/`, the proto agent family and `fleet-cli`; Built, naming what shipped in phases 1
    to 6; Amends 0010 (native agents) and 0013 (SQLite transcripts) by adding derived tables.
  - Sections: the entity (thread plus `Delegation`, `JobManager` rejected and why); the spawn
    surface (CLI over MCP, and that MCP can wrap it later); done (three conditions plus the
    background-task grace); the correctness path (store transaction plus outbox, the bus is
    lossy); trust (the token); delivery timing (idle by default, `--eager`); the three decisions
    the user made (same worktree by default, never auto-resume a user stop, always full access);
    limits; what is deferred.
  - Add the row to the ADR table in `docs/README.md`.
- **Verification:** The ADR reads without the design doc open; `docs/README.md` links resolve.
- **Done when:** A reader can answer "why not a Job?" and "why not MCP?" from the ADR alone.

### P7-T02 — `fleet doctor` reports whether a harness child can find `fleet`
- **Intent:** Turn the one silent failure mode ("the child never reports") into a diagnosed one.
- **Touches:** `crates/fleet-daemon/src/services/doctor.rs`, its tests, `docs/DEVELOPMENT.md` if
  it lists doctor checks.
- **Steps:**
  - A check beside the three agent-binary checks: resolve `fleet` on the `PATH` a harness child
    receives; pass with the resolved path, fail with the warning text in Assumptions and a hint to
    put the built `fleet` on `PATH` or symlink it.
  - Test with a `FakeProcess` or `FakeShell` environment that has and lacks the binary.
- **Verification:** `cargo test -p fleet-daemon doctor`, `make lint`, then `make doctor` on this
  machine and read the line.
- **Done when:** The line appears in `make doctor` output with the resolved path.

### P7-T03 — Record the deferred items in `TODO.md` and `NATIVE-AGENTS.md` §14
- **Intent:** Make every "later" from the design a named, costed item.
- **Touches:** `TODO.md`, `docs/NATIVE-AGENTS.md` §13, §14.
- **Steps:**
  - `TODO.md` gains one section per deferred item in the file's shape: MCP wrapper around the CLI;
    terminal-based caller UX beyond the flag; remote-host delegations (refused today with the
    reason); structured JSON results; the caller answering a child's gates through the CLI; a
    Jobs-screen read-only projection of delegations. Each with Impact, Start in, Done when.
  - §13 row 9 becomes **done** with the same "owed" phrasing the other rows use; §14 lists the
    deferred items in one line each.
- **Verification:** `git diff TODO.md docs/NATIVE-AGENTS.md`.
- **Done when:** Every item in the design's "deliberately later" list has a home.

### P7-T04 — Cross-document audit and the final verification set
- **Intent:** Make the docs agree with each other and with the code, then gate the initiative.
- **Touches:** `docs/NATIVE-AGENTS.md`, `docs/UX-SPEC.md`, `docs/KEYMAP.md`,
  `docs/research/agents-contracts.md`, `docs/TESTING-HARNESS.md`, `docs/DEVELOPMENT.md`.
- **Steps:**
  - Read §15 top to bottom against `services/agents/delegation/` and the tests; fix either side.
  - Check every key in `KEYMAP.md` against `keymap.rs` (the table test helps); every surface in
    `UX-SPEC.md` §3.6, §3.6.0, §3.9 against a scenario; every shape in `agents-contracts.md`
    against the goldens; the §3 and §5 additions in `TESTING-HARNESS.md` against the snapshot and
    the scripted provider.
  - `DEVELOPMENT.md` gains how to run the live delegation tests and the doctor line.
  - Run the full set below and the live tests; paste the outcomes in the tracker.
  - Load `zed-quality-review` and review the whole branch diff since phase 1 as the last step.
- **Verification:** the commands under Verification, plus the live tests.
- **Done when:** No sentence in any of the six documents contradicts the code, and every command
  is green.

## Verification

```sh
make lint
make test
make harness
make doctor
cargo test -p fleet-daemon --features real-agents delegation::live   # with both binaries on PATH
```

## Definition of done

- [ ] Every task above is `[x]` in the tracker.
- [ ] `make lint` is clean.
- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `make test` and `make harness` pass.
- [ ] Both live tests passed once and the tracker says when.
- [ ] ADR 0017 exists and is indexed; `TODO.md` and §13/§14 carry the deferred items.
- [ ] The tracker reflects reality; nothing is left as a follow-up without a home.

## Risks and rollback

- **The audit finds a real disagreement.** Fix the code or the doc in a commit of its own with the
  `docs:` or crate area prefix; do not fold it into the ADR commit.
- **Rollback** is not meaningful for this phase; every change is a document or a diagnostic.
