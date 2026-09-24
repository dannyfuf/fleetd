# PR review boards, phase 1: the review model in fleet-core — Tracker
> Plan: ./pr-review-boards-2026-09-22-phase-1-plan.md
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
- FEA-1 — P1-T01 — Add the review fields to the board model and stamp version 3
- FEA-2 — P1-T02 — Parse and normalise pull request references
- FEA-3 — P1-T03 — Ship the reviews preset and the Reviews board constructor
- FEA-4 — P1-T04 — Put the pull request and the user's notes into the run brief
- FEA-5 — P1-T05 — Make routing columns a queue
- FEA-6 — P1-T06 — Create pull request cards idempotently and reopen them on a new request

## Notes / decisions log
(Append-only. Date-stamp entries. Decisions also go on the card as a comment.)

## Follow-ups
