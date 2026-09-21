# 0022 — The worktree board is a control plane, not a mirror

**Adopted** for `fleet_core::board`, the daemon's `Boards` and `DelegationService`, the board
protocol family, `fleet board`, and the app's board surface. A column may carry one *on enter*
action; moving a card into it starts a native subagent run on the daemon that owns the worktree.
Delivered in nine phases; this record is written with phase 1, which ships the model alone.

## Context

A worktree board today records what a person or an agent has already decided to do. The work
itself happens somewhere else — in a terminal, in an agent thread, in a subagent a thread
delegated — and the board learns about it only when somebody moves a card. An agent orchestrating
several pieces of work has to hold the plan in its own context, poll for completion, and move the
cards itself.

Everything the missing half needs already exists. Native subagents (ADR 0017) give a durable
delegated thread with a transactional result: a brief, an expectation, a `fleet subagent complete`
report, nudges, once-only delivery, a depth and concurrency limit, and a cancel path. Checkpoints
give the diff a run produced. ADR 0021 settled where a worktree board lives: on the daemon that
owns the worktree. What is missing is the sentence that connects a card's column to a delegation.

## Decision

- **A failed review stops.** A run whose outcome is not `Succeeded` moves nothing and marks the
  card as wanting a human. Automation never retries and never routes a failure anywhere: a loop
  that re-ran a failing card would burn tokens on the same failure until somebody noticed.
- **Review scope comes from the checkpoint diff.** The files a run changed are read from the
  thread's first checkpoint against a fresh snapshot, at delivery, and appended to that run's
  report comment. *Alternative rejected:* asking the reviewing agent to work out what changed —
  it cannot know which of the worktree's edits were this run's.
- **The throttle is board-level, and the preset sets one.** Every run of one board edits one
  checkout, so concurrency is a property of the board, not of a column. `max_live_runs` is
  `1..=8`, `None` meaning one. *Alternative rejected:* a per-column limit, which would let two
  columns each start a card and have them edit the same files.
- **Runs never commit.** The preset's instructions say so outright. A run that committed would
  make its work impossible to review as one diff and impossible to undo without touching history.
- **The routing column is Ready, not Queued.** A card is put in Ready once it is meant to run;
  `advance_when_unblocked` releases it when its blockers are done. Todo stays human.
  *Alternative rejected:* a "Queued" column, which names a mechanism rather than a state of the
  work and invites a person to think of it as the automation's inbox.
- **Three rows, not a three-stage picker.** A card's agent preference is `Provider`, `Model` and
  `Effort` as three ordinary property rows, each sending the whole `CardPatch.agent`, exactly as
  labels already behave. *Alternative rejected:* one composite picker walking the three in
  sequence, which cannot express "change only the model".
- **Model and effort are free text against a suggested list.** The pickers offer `column default`
  and the same list the agent composer offers, plus whatever the user typed. A provider ships a
  new model between Fleet releases and the board must not be the thing that cannot name it.
- **No spend cap in v1.** Cost and tokens are recorded per run and shown, and nothing is refused
  on them. A cap that stopped a run mid-flight would leave a half-finished worktree, which is
  worse than an expensive run somebody can see and cancel.
- **Skill actions run on Claude only.** Codex has no verb that takes a skill, so a column asking
  for both is refused at validation with the sentence that says what to do instead: put the
  invocation in the column's instructions. A *card* preferring Codex under a skill column is
  ignored rather than refused — a move must not fail because of a preference.
- **Live run state is never on the card.** The card records a run's identity and its terminal
  facts; what a run is doing right now belongs to the delegation and is joined onto
  `BoardView.live_runs` on read. *Alternative rejected:* mirroring progress onto the card, which
  would make every headline a board write and every board document a transcript.
- **Reports are capped at 8 KiB and three per card.** A board document is read whole on every
  request; an uncapped report history turns a card into a log file. The full report stays in the
  run's thread and the excerpt says so.
- **The document version bumps lazily.** `BOARD_DOCUMENT_VERSION` is 2, but a document is stamped
  from its contents: a board that never opts in keeps writing 1 and a daemon built before this
  feature keeps reading it. The store reads `1..=2`. *Alternative rejected:* stamping 2
  unconditionally, which would make one upgraded daemon enough to lock every board out of every
  older one.
- **The engine returns a plan applied outside the gate.** `after_card_entered` decides under the
  board's non-reentrant write gate and returns the runs to start; the caller starts them after
  dropping it, with an in-flight reservation so the throttle counts a run that has been decided
  but not yet created. *Alternative rejected:* starting delegations while holding the gate, which
  would deadlock the moment a start wrote back to the board.
- **Columns are edited in Board settings.** One dialog, a Columns pane, one `UpdateBoard` on save.
  *Alternative rejected:* a separate columns dialog, which would give the same object two editors.
- **One column glyph, ⚡.** A column with an action is marked once, in its header. A per-card
  badge repeating it would say the same thing as many times as the column has cards.
- **`A`, `X` and `>` bind on the workspace board and the card detail only.** Attach, cancel and
  run-now are meaningless over the Hub's context board, which has no worktree to run in.
- **No `satisfies_blockers` in v1.** A link is satisfied by the blocker reaching a `Completed`
  column and by nothing else. A second, per-link predicate is a second thing to explain before
  the first one has been used.
- **Runs live on the daemon that owns the worktree, reached through the router (ADR 0021).**
  A laptop that only mirrors a worktree forwards every board and card request for it, so it never
  runs automation for a checkout it does not hold.

## Alternatives rejected

- **A separate automation document or service.** The automation of a column is part of that
  column, and the runs of a card are part of that card: a second document would need its own
  identity, its own sync story and its own consistency with the board it describes.
- **A new delegation-like mechanism for card runs.** A card run *is* a delegation, with a caller
  that happens to be a card rather than a thread. Reusing `Delegation` inherits the child
  lifecycle, the report contract, the nudges, the once-only recovery and the cancel path verbatim;
  a parallel mechanism would reimplement all five and drift from them.
- **Bumping `PROTOCOL_VERSION`.** The three run requests are additive and gated on
  `board.automation`, the same shape ADR 0014 and ADR 0019 used. A bump would force every local
  and remote daemon to upgrade in lockstep for a feature most boards will never enable.
- **Letting a run move its own card.** A run's report moves the card when it finishes. A run that
  moved itself could route around a failed review, and a card's column would stop being a
  statement the board makes about the work.

## Consequences

A board that opts in cannot be read by a daemon built before this feature: the document writes
version 2 and the older store refuses it by name rather than quarantining it. Recovery is removing
the automation, the links and the runs, after which the next save stamps 1 again. Boards that
never opt in are unaffected, byte for byte.

Automation is refused on a context board, on a board with a remote backend, and on a worktree
another host owns; the refusal sentences are fixed in `docs/BOARD.md` §11 so every surface says
the same words. A daemon with no native-agent database refuses automation outright rather than
accepting a move it cannot act on.

Phase 1 ships the model and nothing else. `board.automation` is defined and not advertised, so a
daemon built from phase 1 is indistinguishable, to every client, from the one before it.
