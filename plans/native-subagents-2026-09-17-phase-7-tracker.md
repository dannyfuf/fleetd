# Native subagents, phase 7: ADR, doctor line and cross-document audit — Tracker
> Plan: ./native-subagents-2026-09-17-phase-7-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.
- Definition of done is not met until every box is ticked and this tracker matches reality.

## Kickoff
- [x] I have read the plan end to end.
- [x] I have run the project-wide verification commands once on a clean tree to confirm a green baseline.
- [x] I am ready to start.

## Tasks
- [x] P7-T01 — Write ADR 0017 and index it
  - verified: ADR 0017 exists, follows the adopted/built/amends shape, and is indexed.
- [x] P7-T02 — `fleet doctor` reports whether a harness child can find `fleet`
  - verified: `make doctor` passed after restart and resolved this worktree's `target/debug/fleet`.
- [x] P7-T03 — Record the deferred items in `TODO.md` and `NATIVE-AGENTS.md` §14
  - verified: every deferred design item has the required impact/start/done fields and §14 entry.
- [x] P7-T04 — Cross-document audit and the final verification set
  - verified: the cross-document audit, doctor, all 55 headless scenarios, lint, check and workspace tests passed.

## Notes / decisions log
- 2026-09-18 — `make doctor` initially found protocol 7 and a missing child PATH; restarting this build with `target/debug` on PATH produced a fully green report.
- 2026-09-18 — The live-test module is exposed at the documented `delegation::live` filter; it was not executed on this machine.
- 2026-09-18 — The virtual lane opened successfully, but another layer painted over the isolated output; the complete corpus was rerun headlessly.
- 2026-09-18 — Final verification passed: `make lint`, `cargo check --workspace --all-targets`, `make test`, and the 55-scenario headless fallback.
- 2026-09-18 — Corrected the harness run root and prune target to honor `TMPDIR`; default names now shorten long scenario stems enough to preserve the Unix-socket limit under that root.
- 2026-09-18 — Teardown now disables the fixture `fleet` shim before stopping the owned daemon, preventing a late scripted `subagent complete` from auto-starting an unowned replacement.

## Follow-ups
- 2026-09-18 — Re-run `make harness` when the virtual output is no longer painted over by another layer.
- 2026-09-18 — Live delegation tests were NOT run on this machine because no vendor CLIs are installed.
