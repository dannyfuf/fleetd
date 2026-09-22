# Resume prompt for the board-workflows session

Paste everything below the line into the new session. It describes the tree as of 2026-09-21,
after all nine phases were built.

---

Finish and land plans/board-workflows-2026-09-20-roadmap.md on branch feat/workflows-v1, in the
worktree /home/df/.fleet/worktrees/dannyfuf/fleetd/feat-workflows-v1. The building is done; what
is left is one bug, the commits, and a manual smoke. Read, in this order, before doing anything:

1. plans/board-workflows-2026-09-20-handoff.md — the state of the tree, what is verified, what is
   not, and the fourteen-commit plan by area.
2. plans/board-workflows-2026-09-20-phase-9-tracker.md, its Follow-ups section in particular: it
   carries the measurements for the one red thing and the manual smoke that is owed.
3. plans/board-workflows-2026-09-20-contracts.md §0 and the phase tracker for whatever you touch.

Facts you must not rediscover:
- HEAD is ab5f195. Phase 1 and the old FEA-10 are committed; **phases 2 through 9 are entirely
  uncommitted in the working tree** — ~159 files, ~16.7k lines, nothing staged. That is expected:
  the build agents were forbidden every state-changing git command.
- `make lint` is clean. Every crate's own suite is green alone. The documents were audited against
  the shipped code on 2026-09-21 and agree with it.
- `make test` is **red** in exactly one place: fleet-app's `harness_headless`, on
  scenarios/board/workflow-chain.scenario and one sibling, deterministically. The board tile never
  paints a live run's mark — a card-called child that goes `blocked` does not repaint its tile,
  and the face can lag a run by the whole of that run. The reducer is right
  (`state::board::tests::a_child_that_goes_blocked_turns_its_card_amber_without_a_board_reload`);
  the wiring into it from the delegation mirror is not. Fix that first: it is the only thing
  between this tree and a PR, and it is a real user-visible bug, not a test artifact.
- No phase ever ran a real `claude` or `codex`. Every run was a scripted `fleet-harness agent`
  replaying a transcript. The manual smoke owed before announcing the feature is written out step
  by step in the phase-9 tracker's Follow-ups. Do it on a real worktree board against your own
  daemon, both providers, and record what you saw.
- Line numbers in every phase plan predate the PR #43 merge; re-grep every symbol.

How to work:
- Delegate the execution — the bug fix, and any follow-up sweep — with a tightly scoped brief; you
  own the architecture, the review and the commits. Never hand over a task you have not scoped.
- Commit form `<area>: <imperative lowercase summary>`, one logical change each, the doc update
  riding with its code. No Co-Authored-By and no Claude attribution of any kind.
- Before the PR: `make lint`, `make test`, and `make harness` — this round changed the keymap, two
  screens, a dialog and the fixture set, so the GUI corpus is not optional. Run
  `make harness-one SCENARIO=scenarios/board/workflow-chain.scenario` first.
- `make restart` after landing daemon code, so the running fleetd matches the build.
- Keep the trackers current as you go: tick a box only with a "verified: <how>" line under it, and
  put anything non-obvious in that tracker's Notes/decisions log rather than in a chat message.
