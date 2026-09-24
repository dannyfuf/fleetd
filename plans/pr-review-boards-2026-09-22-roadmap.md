# PR review boards and scheduled agent tasks — Roadmap
> Phase plans:
>  - Phase 1: ./pr-review-boards-2026-09-22-phase-1-plan.md · ./pr-review-boards-2026-09-22-phase-1-tracker.md
>  - Phase 2: ./pr-review-boards-2026-09-22-phase-2-plan.md · ./pr-review-boards-2026-09-22-phase-2-tracker.md
>  - Phase 3: ./pr-review-boards-2026-09-22-phase-3-plan.md · ./pr-review-boards-2026-09-22-phase-3-tracker.md
>  - Phase 4: ./pr-review-boards-2026-09-22-phase-4-plan.md · ./pr-review-boards-2026-09-22-phase-4-tracker.md
>
> Contracts (shared names, fields, sentences — authoritative for the build): ./pr-review-boards-2026-09-22-contracts.md
>
> **The board is the source of truth, not these files.** Every task below is a card on the worktree
> board `dannyfuf/fleetd#feat-board-workflow-schedule-custom-task` (prefix `FEA`), and each card's
> description carries everything needed to implement it. These files are the design record the cards
> were generated from. When a card and this file disagree, the card wins; fix this file afterwards.

## Summary

The user reviews other people's pull requests. Review requests arrive from GitHub (assigned as
reviewer) and from sources GitHub does not know about (a company MCP that posts review requests in a
Google Chat channel). The user wants every request to become a card on a board, have an agent review
each PR automatically up to a "Reviewed" column, read the result, and then publish it with one move.

Three pieces make that possible:

1. **A Reviews board per context** — a new board kind whose columns are *Pending review → Reviewing →
   Reviewed → Review published* (+ *Dismissed*), whose cards carry a pull-request reference, and whose
   automation runs **each card in that card's own PR worktree** instead of one shared checkout.
2. **Scheduled tasks** — a daemon-owned, board-owned prompt that runs headless (`claude -p` or
   `codex exec`) on a cadence (every N minutes, or once at a time). The agent uses whatever tools,
   MCP servers and skills the user already has installed, and records what it finds with
   `fleet board card new --pr …`, which is idempotent per pull request.
3. **The app** — the Pull requests screen's **Review** tab becomes that board (the **Mine** tab is
   unchanged), and Board settings gains a **Schedules** section.

## Where the board lives — the decision

Three homes were compared against the code as it is today:

| Option | Verdict |
| --- | --- |
| Replace the whole PR screen with a board | Rejected. The screen's **Mine** tab ("what am I waiting on") has no board equivalent, and the screen is also the create-worktree-from-PR entry point. |
| A separate "Reviews" *context* holding an ordinary board | Rejected. A context groups repositories; a review context would own no repos, so PR worktrees (which need the repo) could not be created in it, and every review would still live one context-switch away from the work it belongs to. Context boards also refuse automation today. |
| **A Reviews board per context, shown in the PR screen's Review tab** | **Chosen.** The Review tab already answers "what is waiting on me", already owns the `p` / `g p` keys, the context-bar Review chip and open-or-create-worktree. Replacing its flat, stateless `gh` list with a board adds exactly what the list cannot hold: progress (reviewing, reviewed, published), the agent's report, and history. The Hub's Board tab keeps the context's task board, so the two never compete for one slot. |

The flat `gh pr list --search user-review-requested:@me` list is retired from the Review tab; the
same query becomes the *starter prompt* of the default schedule, so a user who adds nothing still
gets GitHub review requests on the board.

## Why this is phased

Four subsystems change — the pure board model (`fleet-core`), the daemon's board and worktree
services plus the wire and CLI, a new daemon scheduler, and the GPUI app — and each has to be
shippable on its own: the model must land before anything can persist it, the daemon must accept
per-card-worktree runs before a schedule can usefully create cards, and the app must not show a
Review board the daemon cannot serve. Each phase is one pull request.

## Phase list

### Phase 1 — The review model in `fleet-core`
- **Goal:** add the board kind, the pull-request reference on a card, the per-card-worktree run location, the reviews preset, the queue semantics for routing columns and the brief additions, all as pure, tested code.
- **Shippable state at end of phase:** nothing a user can see changes; every new field is optional, older documents read unchanged, a board that uses none of it is written byte-for-byte as before.
- **Plan:** ./pr-review-boards-2026-09-22-phase-1-plan.md
- **Tracker:** ./pr-review-boards-2026-09-22-phase-1-tracker.md

### Phase 2 — Reviews boards in the daemon, the wire and the CLI
- **Goal:** serve a context's Reviews board, run each review card in its own PR worktree (created on demand), make `card new --pr` idempotent, and drive all of it from `fleet board --reviews`.
- **Shippable state at end of phase:** a user can create a Reviews board and cards from the CLI and watch them go Pending review → Reviewing → Reviewed with no GUI.
- **Plan:** ./pr-review-boards-2026-09-22-phase-2-plan.md
- **Tracker:** ./pr-review-boards-2026-09-22-phase-2-tracker.md

### Phase 3 — Scheduled agent tasks
- **Goal:** a board-owned schedule document, a daemon loop that fires due schedules, a headless runner for Claude and Codex, the wire family and `fleet schedule`.
- **Shippable state at end of phase:** `fleet schedule new --reviews …` makes review cards appear on their own every N minutes.
- **Plan:** ./pr-review-boards-2026-09-22-phase-3-plan.md
- **Tracker:** ./pr-review-boards-2026-09-22-phase-3-tracker.md

### Phase 4 — The app: the Review tab board and the Schedules section
- **Goal:** the PR screen's Review tab shows the context's Reviews board, cards show their PR, runs are attachable, Board settings edits schedules, and the harness pins it.
- **Shippable state at end of phase:** the full flow works from the GUI; the flat review list is gone.
- **Plan:** ./pr-review-boards-2026-09-22-phase-4-plan.md
- **Tracker:** ./pr-review-boards-2026-09-22-phase-4-tracker.md

## Seams between phases

- **Phase 1 → 2: the document version.** Phase 1 raises `BOARD_DOCUMENT_VERSION` to 3 and stamps 3
  *only* when a board uses a phase-1 field (a non-default `kind`, a non-default `run_location`, or a
  card with `pull_request`). Phase 2 is the first thing that can write such a board. A daemon older
  than phase 1 refuses a version-3 document by name instead of silently dropping the new fields.
- **Phase 1 → 2: the queue rule.** Phase 1 changes what a routing column (`advance_when_unblocked`)
  does with a card that *enters* it already unblocked: it advances at once, or waits in place with a
  queue marker when the target column is out of run slots. This also changes the shipped workflow
  preset's *Ready* column. Phase 1 updates every core test that pinned the old "waits forever"
  behaviour; phase 2 updates `scripts/board-workflow-smoke.sh`, the `fleet-board-planning` skill and
  any daemon test that relied on it.
- **Phase 2 → 3: the capability strings.** Phase 2 advertises `board.reviews`; phase 3 advertises
  `schedules`. Neither bumps `PROTOCOL_VERSION` (8). The app (phase 4) gates the Review tab board on
  `board.reviews` and the Schedules section on `schedules`, and falls back to "Restart the daemon to
  use review boards" when a daemon lacks them.
- **Phase 2 → 3: the card-creation contract.** A scheduled agent creates cards only through
  `fleet board --board <id> card new "<title>" --pr <ref> [--requested-at <rfc3339>] [--label <l>]`,
  whose first output line is `Created <KEY>`, `Existing <KEY>` or `Reopened <KEY>`. Phase 3's footer
  text teaches exactly that line; phase 2 must not change it.
- **Phase 3 → 4: schedules are board-owned.** A schedule names one `board_id`; deleting the board
  deletes its schedules. The app never shows a schedule outside its board's settings and header.

## Cross-phase risks

- **Token spend.** A schedule every 5 minutes plus up to `max_live_runs` review runs is real money.
  Minimum cadence is 5 minutes, the Reviews board defaults to `max_live_runs = 2`, every run records
  its cost, and nothing runs while a schedule is disabled.
- **Full-access agents.** Scheduled and review runs default to full access (`bypassPermissions` /
  `danger-full-access`) because nobody is there to answer a prompt. The review preset's instructions
  forbid posting, committing and pushing; only the *Review published* column's action posts.
- **Chat sources re-listing old requests.** A Google Chat message stays in the channel forever. The
  footer gives the agent `{last_run_at}`, and a card is only *reopened* when `--requested-at` is
  later than the card's last completion, so an old message never resurrects a published review.
- **Repo not cloned.** A PR in a repository Fleet has never cloned cannot get a worktree. Phase 2
  refuses the run with a sentence the card shows; automatic cloning is a follow-up.

## Suggested order

Strictly 1 → 2 → 3 → 4. Inside a phase, follow the cards' `blocked_by` links; tasks with no link
between them may be done in either order. Run `make lint` and `make test` at the end of every card
and `make harness` for every phase-4 card.
