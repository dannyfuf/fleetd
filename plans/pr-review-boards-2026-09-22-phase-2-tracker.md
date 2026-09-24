# PR review boards, phase 2: Reviews boards in the daemon, the wire and the CLI — Tracker
> Plan: ./pr-review-boards-2026-09-22-phase-2-plan.md
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
- FEA-7 — P2-T01 — Add the review requests and the `board.reviews` capability to the wire
- FEA-8 — P2-T02 — Serve one Reviews board per context
- FEA-9 — P2-T03 — Upsert pull request cards in the daemon
- FEA-10 — P2-T04 — Let a card-worktree board automate, and recover its runs after a restart
- FEA-11 — P2-T05 — Run each review card in its own pull request worktree
- FEA-12 — P2-T06 — Diff each run against the worktree it ran in
- FEA-13 — P2-T07 — Drive Reviews boards from the CLI and advertise the capability
- FEA-14 — P2-T08 — Pin the review flow end to end with a smoke script and update the planning skill

## Notes / decisions log
(Append-only. Date-stamp entries. Decisions also go on the card as a comment.)

## Follow-ups
- FEA-29 — clone a PR's repository automatically
- FEA-30 — clean up review worktrees
