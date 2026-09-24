# PR review boards, phase 4: the Review tab board and the Schedules section — Tracker
> Plan: ./pr-review-boards-2026-09-22-phase-4-plan.md
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
- FEA-22 — P4-T01 — Point the board mirror at a context's Reviews board
- FEA-23 — P4-T02 — Show the Reviews board in the Pull requests screen's Review tab
- FEA-24 — P4-T03 — Show a card's pull request on its tile and in its detail
- FEA-25 — P4-T04 — Unlock column automation in Board settings for card-worktree boards
- FEA-26 — P4-T05 — Add the Schedules section to Board settings
- FEA-27 — P4-T06 — Show schedules in the board header
- FEA-28 — P4-T07 — Pin the Review board and the Schedules section in the harness

## Notes / decisions log
(Append-only. Date-stamp entries. Decisions also go on the card as a comment.)

## Follow-ups
- FEA-33 — open a run's log in a terminal tab
