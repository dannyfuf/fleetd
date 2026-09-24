# PR review boards, phase 3: scheduled agent tasks — Tracker
> Plan: ./pr-review-boards-2026-09-22-phase-3-plan.md
> READ ME FIRST. For this initiative the **board is the tracker**: state lives on the worktree board `dannyfuf/fleetd#feat-board-workflow-schedule-custom-task` (prefix FEA), one card per task, each card carrying its full spec. This file only maps task ids to card keys and holds the decisions log. Read state with `fleet board --worktree show`; never tick boxes here instead of moving cards.

## Working agreement
- Before starting a phase, run `make lint` and `make test` once on a clean tree to confirm a green baseline.
- Move a card to In Progress when you start it, and to Done only after its Verification commands pass; comment the one-line result on the card.
- Never move a card to In review: that column starts an automated `deep-review` run in this worktree.
- Work the cards in `blocked_by` order; one card in progress at a time.
- Work the plan missed becomes a new card (next FEA number), never a silent expansion of an existing one.
- The phase is done when every card below is in Done and the plan's Definition of done holds.

## Kickoff
- [ ] I have read the roadmap, the contracts and this phase's cards.
- [ ] Green baseline confirmed.

## Tasks
- FEA-15 — P3-T01 — Add the schedule model, its validation and its fire-time rule to fleet-core
- FEA-16 — P3-T02 — Persist schedules in a versioned daemon store
- FEA-17 — P3-T03 — Build the headless runner for Claude and Codex
- FEA-18 — P3-T04 — Fire due schedules from a daemon loop and record every run
- FEA-19 — P3-T05 — Put schedules on the wire and in the client
- FEA-20 — P3-T06 — Add the `fleet schedule` command group
- FEA-21 — P3-T07 — Teach agents to use schedules and pin the flow with a scripted smoke

## Notes / decisions log
(Append-only. Date-stamp entries. Decisions also go on the card as a comment.)

## Follow-ups
- FEA-31 — cron cadences and active hours
- FEA-32 — board-less schedules
