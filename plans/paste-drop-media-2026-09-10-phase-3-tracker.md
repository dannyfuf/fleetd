# Phase 3 — The gesture in dialog text inputs — Tracker

> Plan: ./paste-drop-media-2026-09-10-phase-3-plan.md
> READ ME FIRST. Update this file as you work. The plan is reference; this tracker is the source of truth for state. If reality diverges from the plan, update both.

## Working agreement
- Check the kickoff box below before starting.
- Move tasks through: [ ] todo → [~] in progress → [x] done. One task in progress at a time.
- After each task: tick its box, paste the verification command output (or a one-line "verified: <how>"), and commit.
- If you discover work the plan missed, add a new task with the next ID. Never silently expand an existing task.

Repo-specific: load the skill `CLAUDE.md` names for the area; commit as `ui-kit: …` / `app: …` / `docs: …`; `fleet-ui-kit` depends only on `gpui`.

## Kickoff
- [ ] Phase 1 and phase 2 have shipped (`media::stage`, `MediaAnchor::Local`, `TextInput::accepts_media` + `TextInputMedia`).
- [ ] I have read the plan end to end.
- [ ] Green baseline: `make lint`, `make test`, `make check`, `make harness`.

## Tasks
- [ ] P3-T01 — Stop pasting concatenated paths
- [ ] P3-T02 — Inventory the dialog fields, and wire the ones where a path belongs
- [ ] P3-T03 — Move the docs with the code

## Dialog fields (P3-T02)

Fill this in completely, including the deliberate noes.

| Dialog / screen | Field | Wired? | Quoted? | Reason |
| --- | --- | --- | --- | --- |
| | | | | |

## Notes / decisions log
(Append-only. Date-stamp entries.)

- 2026-09-24 — Phase created from the dialog half of the original phase 2. `TextField` no longer exists (ADR 0020); every field is a `TextInput`, so there is no longer a paste/drop asymmetry between two input kinds to document.

## Follow-ups
