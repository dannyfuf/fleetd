# Board workflows — session handoff (2026-09-20, evening)

Written when the orchestrating session had to restart. Read this, then the roadmap, contracts and
the phase-1 plan/tracker. The board `dannyfuf/fleetd#feat-workflows-v1` is the source of truth
for card state; this file is the narrative the board does not carry.

## Branch state

- Branch `feat/workflows-v1`, HEAD `4f54602 wip: phase 1 core model …`. Two commits before it:
  `7c9485e docs: fold the hosted-board routing into the workflow plans` (plan amendments, keep) and
  the merge of `origin/main` (PR #43, hosted worktree boards route to their owner; ADR 0021).
- **`4f54602` is a WIP snapshot, not a reviewed commit.** It holds phase 1 tasks P1-T01 and
  P1-T02 (verified green by the agent: `cargo test -p fleet-core`, `cargo check --workspace
  --all-targets`) plus P1-T03 mid-edit in `crates/fleet-daemon/src/stores/board.rs`.
  `cargo check -p fleet-core -p fleet-daemon` was clean right after the commit, so the tree
  compiles. Squash it into the real phase-1 commits before opening a PR
  (`git reset --soft 7c9485e` and recommit per area, or keep building on top and squash at the end).
- Baseline before phase 1: `cargo check --workspace --all-targets` clean on the merged tree.

## Plan amendments already made (do not redo)

1. ADR for this feature is `docs/decisions/0022-board-workflows.md` (0021 is the routing ADR).
   Its `docs/README.md` row goes after 0021. Its last decision reads "runs live on the daemon that
   owns the worktree, reached through the router (ADR 0021)", not "local daemon only".
2. Contracts §3.6 and phase-3 plan P3-T06: `CardRunStart/Cancel/Wait` classify like `MoveCard`,
   by `host_or_local(resolver.host_of_card(card_id))`, not `Target::Local`.
3. Line numbers in every phase plan predate the PR #43 merge; re-grep symbols.

## Phase 1 (FEA-1, In Progress)

Tracker: `plans/board-workflows-2026-09-20-phase-1-tracker.md` (the agent kept it current).
- [x] P1-T01, [x] P1-T02, [~] P1-T03, [ ] P1-T04, [ ] P1-T05, [ ] P1-T06.
- Agent findings worth keeping: the `ActivityKind` sweep reached no renderer (no exhaustive match
  outside `fleet-core`); it did reach every struct literal of `Card`/`Status`/`Comment`/
  `BoardSummary`/`BoardView` in daemon, router, mirror, app and CLI fixtures, and the five
  `summarize` call sites. The two CLI `summarize` callers pass `now = ""` with a comment (they only
  read dirty/conflict counts); phase 5 must pass a real RFC 3339 stamp when it prints
  working/attention counts.
- Resume by launching one Opus agent with the same brief as before (read contracts §0/§1/§3.6,
  phase-1 plan, tracker; skills `rust-workspace-architecture`, `rust-ipc-protocol`,
  `rust-gpui-testing`, `zed-quality-review`; no git ops; no `fleet board`; no `make restart`;
  verification block from the plan; report format from contracts §0) and tell it T01/T02 are done
  and committed in `4f54602`, T03 is partially written in `stores/board.rs`, continue from there.
- After the agent reports GREEN: review the diff yourself (goldens untouched, every new field has
  `default` + `skip_serializing_if`, refusal sentences verbatim), run `make lint` and `make test`,
  then `make restart`, then move FEA-1 to Done with a comment.

## FEA-10 (In Progress) — ready to land

- Fix is complete and reviewed: commit `b869742` on branch `worktree-agent-a08887fddd105783c`
  (git worktree at `.claude/worktrees/agent-a08887fddd105783c`, untracked directory, its own
  `target/`). `cargo test -p fleet-cli` and `make lint` green there.
- It resolves a bare `--worktree` from `FLEET_SESSION` against `snapshot.agent_threads` after
  `snapshot.sessions` misses (no second request; chosen over `AgentThreadList` because the snapshot
  is already fetched and degrades to the old refusal on an older daemon). Touches
  `crates/fleet-cli/src/commands/board.rs` + `board/tests.rs`, `docs/BOARD.md` §6 CLI paragraph,
  `.claude/skills/fleet-board-planning/{SKILL.md,references/checklist.md,references/commands.md}`.
- Land it with `git cherry-pick b869742` **after** phase 1 is committed (both touch fleet-cli's
  `board.rs` and `docs/BOARD.md`; expect at most a trivial conflict). Then delete the agent
  worktree (`git worktree remove .claude/worktrees/agent-a08887fddd105783c --force` and
  `git branch -D worktree-agent-a08887fddd105783c`), run `cargo test -p fleet-cli`, move FEA-10 to
  Done. Once landed, a native thread can drop `--worktree=<id>` for a bare `--worktree` (the
  `=` form still works); update the memory note `fleet-cli-on-path` accordingly.

## Remaining phases

Strictly 2 → 3 next, each as one Opus agent in the main worktree (parallel agents in one tree
broke each other's builds mid-edit, which is why FEA-10 ran in an isolated worktree with its own
`CARGO_TARGET_DIR`). Phase 2 needs `BOARD_AUTOMATION_CAPABILITY` from phase 1 T06. After 3:
4 and 5 can run in parallel (daemon vs cli) only with isolated worktrees; 6 → 7 and 8 likewise;
9 last. Cards FEA-2..FEA-9 are in Todo.

## Board-usage friction noticed so far (candidates for Backlog cards)

- Every `card comment` prints the whole card including every prior comment; for an agent adding
  a hand-off note this is the noisiest verb on the board. A `--quiet` or key-only print would help.
- Starting a card is always two commands (`card move … in-progress` then `card comment`); a
  `--comment` on `card move` would make the decision ride with the transition.
- FEA-11 stayed "Done" on the board while its PR was unmerged into this branch; the card's comment
  said "move to Done when merged", so the board and the branch disagreed until the merge here.
  Nothing to fix in the tool; a reminder that Done means "on the branch you are on".
