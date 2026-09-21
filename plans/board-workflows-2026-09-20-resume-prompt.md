# Resume prompt for the board-workflows session

Paste everything below the line into the new session.

---

Continue implementing plans/board-workflows-2026-09-20-roadmap.md on branch feat/workflows-v1.
A previous session paused mid phase 1. Read, in this order, before doing anything:

1. plans/board-workflows-2026-09-20-handoff.md — the resume procedure, what is already committed,
   the plan amendments already made (do not redo them), and the board friction noticed so far.
2. plans/board-workflows-2026-09-20-contracts.md, then the roadmap, then the phase-1 plan and tracker.
3. `fleet board --worktree=dannyfuf/fleetd#feat-workflows-v1 show` — the board is the source of
   truth for card state. FEA-1 and FEA-10 are In Progress, FEA-2..9 are Todo, FEA-11 is Done.

Facts you must not rediscover:
- HEAD is a9bf3ec (or later). Commit 4f54602 is a WIP snapshot of phase 1 (P1-T01 and P1-T02 done
  and verified, P1-T03 mid-edit in crates/fleet-daemon/src/stores/board.rs). Squash it before any PR.
- origin/main (PR #43, hosted worktree boards route to their owner, ADR 0021) is already merged.
  The feature ADR is 0022. The three run requests route by host_of_card, not Target::Local.
- FEA-10's fix is finished and reviewed: commit b869742 on branch worktree-agent-a08887fddd105783c
  (git worktree under .claude/worktrees/). Cherry-pick it right after phase 1 is committed, then
  remove that worktree and branch, then move FEA-10 to Done.
- Line numbers in the phase plans predate the PR #43 merge; re-grep symbols.

How to work:
- Codex and Grok are out of quota. Use only Opus subagents (Agent tool, model "opus"). You are the
  orchestrator: scope each brief tightly from the contracts and phase plan, review every diff
  yourself, run `make lint` and `make test` yourself, and only then move a card.
- One agent editing the main worktree at a time. Two agents in one tree broke each other's builds.
  For parallel work use `isolation: "worktree"` with its own CARGO_TARGET_DIR.
- Agents never run git or `fleet board`; you commit and you move cards. Commit form is
  `<area>: <imperative lowercase summary>`, no Co-Authored-By or any Claude attribution trailer.
- Order: finish phase 1 (resume one Opus agent with the brief described in the handoff, telling it
  T01/T02 are done and T03 is partial), land FEA-10, `make restart`, then phase 2, then phase 3,
  then the rest per the roadmap's Suggested order.
- Dogfood the board as you go: move cards only when the state really changes, comment decisions
  and hand-offs, and add a Backlog card for any board friction worth fixing (the handoff lists
  three candidates: noisy `card comment` output, no `--comment` on `card move`, Done meaning
  "merged on this branch"). Fix small friction items now if they are fleet-cli-only and do not
  collide with the running phase.
- Update the phase tracker files and my memory note board-workflows-progress.md whenever a phase
  or card changes state, so the next restart costs nothing.
