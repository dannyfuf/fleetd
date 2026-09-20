---
name: fleet-board-planning
description: How an agent plans and tracks work with `fleet board` — picking the right board (context or worktree), reading it before writing, creating cards that are actionable, moving them through statuses as the work really progresses, recording decisions as comments, starting a worktree from a card, syncing a remote-backed board, and reading the JSON envelopes from a script. Load it when asked to plan, break down, track, or report on tasks, when running inside a Fleet terminal or native-agent thread and about to touch a board, when orchestrating subagents whose work should be visible, or whenever a command starts with `fleet board`.
---

# Planning work on a Fleet board

A Fleet board is a Linear-style kanban: ordered **status columns** holding **cards**. Every
context has one board; a worktree may additionally have its own. Boards live in the daemon, so
a card an agent creates is visible in the app, to other agents, and after the agent's session
ends. The board is the durable plan; the transcript is not.

Authoritative docs: `docs/BOARD.md` (model, engine, CLI contract §6) and `docs/BOARD-JIRA.md`
(what a Jira-backed board can and cannot do). Command source: `crates/fleet-cli/src/args.rs`
(`BoardArgs`, `BoardCommand`, `BoardCardCommand`) and `crates/fleet-cli/src/commands/board.rs`.
Full flag and output reference: `references/commands.md`. Pre-flight lists:
`references/checklist.md`.

## When to use

- The user asks you to plan, break down, sequence, track, or report on work.
- You are about to do a multi-step task and want the steps visible and resumable.
- You are orchestrating subagents (`fleet-subagent-cli`) and each delegation should be a card.
- You picked up a task named by a card key (`FLT-12`, `PROJ-123`) or a worktree started from one.

## When not to

- A task that fits in a few tool calls. A card costs more to write than the work it tracks.
- Recording the *conversation*. Cards hold plan and outcome; comments hold decisions, not chatter.
- Asking a human a question. Nobody reads a comment in time; ask through your own surface.

## The model in five lines

1. **Board** = `id`, `name`, `prefix` (`FLT`), ordered `statuses`, `labels`, `settings`, a
   `backend` (`local` by default, `jira` optional), and either a context or a worktree scope.
2. **Card** = `display_key` (`FLT-12`, or the remote key when mirrored), title, Markdown
   description, status, priority (`urgent|high|medium|low|none`), labels, assignee, estimate,
   due date, repo, linked worktree, comments, activity.
3. **Statuses** carry a category: `backlog`, `unstarted`, `started`, `completed`, `canceled`. A
   local board ships `Backlog`, `Todo`, `In Progress`, `Done`, `Canceled` with ids `backlog`,
   `todo`, `in-progress`, `done`, `canceled`. A Jira board's columns come from Jira; read them
   with `fleet board describe`.
4. **Scope**: the context board is the project's plan. A worktree board is the plan for one
   branch's work and is created on first use. Both are reachable from any terminal.
5. **Keys**: every `card` verb takes a display key, a card id, or (for cards with no remote
   link) the local key, case-insensitively. An ambiguous key is refused; use the id.

## Rules

1. **Select the board explicitly, and know what your `FLEET_SESSION` is.** Resolution order is
   `--board <id>`, else `--worktree[=<owner/name#slug>]`, else `--context <id>`, else the
   daemon's active context. `show`, `describe`, `set`, `sync`, and every `card` verb ensure
   (create on demand) a context or worktree board; only `--board` requires one to exist.
   - In a Fleet **worktree terminal**, `FLEET_SESSION` is a worktree session, so a bare
     `--worktree` selects this worktree's board.
   - In a **native agent thread** (Claude Code or Codex started by Fleet, including every
     subagent), `FLEET_SESSION` is the thread UUID, which is not a session; the CLI matches it
     against the daemon's agent threads instead, so a bare `--worktree` selects the thread's
     worktree board. It fails with `no worktree session` only when neither a session nor a thread
     owns that id — from a plain shell, say. To name another worktree, pass
     `--worktree=<owner/name#slug>`; find the id in the sixth column of `fleet agent list` for
     your thread, or match your `pwd` against the `path` fields of `fleet list --json`. The `=`
     is required when an id is passed.
   - `--board`, `--worktree`, and `--context` are mutually exclusive, and are global flags: they
     may sit before or after the subcommand.

2. **Read before you write.** Run `fleet board show` (human output) once at the start of a
   planning pass. It prints one line per card: key, priority, title, labels, assignee. That is
   enough to avoid creating a duplicate, to see what is already in progress, and to pick the
   next card. Read a single card with `fleet board card show <key>` when you need its
   description, comments, or linked worktree.

3. **Prefer the human output; reserve `--json` for a script.** The JSON envelope of `show`
   carries every card whole, including descriptions, comments, and up to 200 activity entries
   per card. On a board of forty cards that is thousands of tokens you will not read. Use
   `--json` when you will parse it (`jq '.cards[] | select(.statusId=="todo") | .title'`), and
   then filter in the pipe rather than in your context.

4. **A card is one outcome a reader could verify.** Title in the imperative, specific enough to
   stand alone (`Add --json to fleet board card delete`, not `CLI work`). The description says
   the goal, the scope boundary, and *done when* in one short Markdown block. One card per
   deliverable, not per file, not per tool call. Three to eight cards is a plan; thirty is a
   transcript.

5. **Create cards in the column that is true.** `card new` without `--status` lands in the
   first `unstarted` column (`Todo` locally). Use `--status backlog` for things you are noting
   but not committing to. Never create a card directly in a `started` column unless you are
   starting it in the same breath.

6. **Move a card when the work state actually changes, and only then.** `in-progress` when you
   start it, `done` when you have verified it (tests run, diff reviewed), `canceled` with a
   comment saying why when you drop it. Do not move a card to `done` because a subagent said
   it was done; verify first. Keep one card in progress per agent; parallel children each own
   their own card.

7. **Comment decisions, blockers, and hand-offs — nothing else.** A comment is the durable trace
   another agent or the user reads later: `Chose X over Y because …`, `Blocked on …; needs …`,
   `Delegated to codex, delegation <id>`. Keep it to a few lines. A comment is not a log of every
   command you ran.

8. **Batch edits into one command.** `card edit` takes every field flag at once and refuses an
   empty patch. Three separate `card edit` calls are three daemon writes and three printed
   cards in your context. `--clear-*` flags belong to `edit` only.

9. **Do not re-read after every write.** Every mutating `card` verb prints the resulting card.
   Do not follow it with `card show` or `board show` to confirm; the print *is* the
   confirmation. Read the board again when you have finished a phase and need to choose the
   next card.

10. **Labels must exist before a card can carry them.** `--label` resolves against the board's
    label set by id or name (case-insensitive) and errors on an unknown one. Add labels once with
    `fleet board set --add-label <name>`; do not invent a label per card.

11. **Start branch work from the card, not beside it.** `fleet board card worktree <key>` creates
    (or adopts) the worktree named by the board's branch template (`{key}-{slug}` by default),
    links it on the card, and, with the default `start_on_worktree`, moves an unstarted card to
    the first `started` column. The repo resolves from `--repo`, else the card's repo, else the
    board's default repo, else an error; set a default repo once with
    `fleet board set --default-repo owner/name`. It prints `Created <id>` or `Existing <id>`.

12. **`sync` is for remote-backed boards only.** On a local board it starts a job that does
    nothing. On a Jira board, `fleet board sync --wait` pulls and pushes and prints counts; a
    `Last error:` line after a *successful* sync is a warning about skipped remote cards, not a
    failure. `--full` ignores the incremental cursor; use it once when the board looks stale, not
    every time. Sync is not needed after each local edit: cards mark themselves `dirty` and the
    next sync pushes them. New local cards reach the remote only when the board has
    `push_new_cards` on.

13. **Resolve conflicts deliberately.** A card with `conflict` set names the differing fields in
    `card show`. Decide with `fleet board card resolve <key> keep-local|take-remote`; do not edit
    around a conflict.

14. **Archive, do not delete, finished or superseded cards.** `card delete` is for mistakes you
    just made. `card edit <key> --archive` keeps the history; `canceled` keeps the reason.

15. **Do not create boards you will not use.** A worktree board is worth creating when the branch
    has more than one step to track. `fleet board create` is the explicit form; a plain `show`
    with `--worktree` already creates one if missing, so never "check whether it exists" with a
    `show` you did not want.

## A planning pass, end to end

```sh
# 1. Where am I, and what is already planned?
fleet agent list                                   # my thread → worktree id (6th column)
fleet board --worktree=acme/api#feat-x show        # columns and one line per card

# 2. Plan: one card per verifiable outcome, in Todo
fleet board --worktree=acme/api#feat-x card new "Add DelegationWait.caller to the wire" \
  --desc "Goal: waits from the caller consume delivery. Done when: golden updated, tests pass." \
  --priority high
fleet board --worktree=acme/api#feat-x card new "Consume delivery in the worker" --priority high
fleet board --worktree=acme/api#feat-x card new "Document the amendment in ADR 0017" --priority medium

# 3. Start the first card
fleet board --worktree=acme/api#feat-x card move FLT-1 in-progress

# 4. Record the one decision worth keeping
fleet board --worktree=acme/api#feat-x card comment FLT-1 "Advisory identity, not authorisation: an old client keeps today's behaviour."

# 5. Finish it only after verification
make lint && make test
fleet board --worktree=acme/api#feat-x card move FLT-1 done
```

Set the selector once in a shell variable when a pass has many commands:
`B="fleet board --worktree=acme/api#feat-x"` then `$B card move FLT-2 in-progress`.

## Reading the output

`fleet board show` prints a header, then each column with its count, then one row per card:

```text
Feature X (FLT) · worktree acme/api#feat-x
backend: Local

Todo (2)
FLT-2  high  Consume delivery in the worker
FLT-3  medium  Document the amendment in ADR 0017

In Progress (1)
FLT-1  high  Add DelegationWait.caller to the wire  [proto]  @me
```

A trailing `No column (n)` section lists cards whose status no longer exists; fix them with
`card move`. `fleet board list` prints one row per board across all scopes with counts of cards,
open, dirty, and conflicts, plus a trailing error cell for a board whose last sync failed.

Exit codes: `0` on success, `1` on any error (validation, not found, ambiguous key, daemon
refusal). Errors go to stderr as one line, or as a JSON error envelope when `--json` was passed.

## Anti-patterns

- **A card per tool call, or a card per file.** The board is for outcomes; the diff is for files.
- **`board show --json` in a loop.** Nothing on the board changes unless someone changes it. Read
  it at phase boundaries.
- **Moving to `done` on a subagent's word.** The child reported; you verify; then you move.
- **Deleting instead of canceling.** Deletion erases the reason it was planned.
- **A comment thread as a chat.** Comments are for the next reader, weeks later.
- **Creating labels ad hoc.** `--label` errors; and a board with twenty one-off labels filters
  nothing.
- **A bare `--worktree` from a plain shell.** Nothing owns `FLEET_SESSION` there; pass the id
  with `=`.
- **`sync` on a local board, or after every edit on a Jira board.** Dirty cards accumulate and one
  sync pushes them all.

## With subagents

When `fleet-subagent-cli` is in play: one card per delegation, created before `fleet subagent
run` and named in the brief so the child knows which outcome it owns. The orchestrator owns the
status transitions; a child may add a comment with what it found but does not move cards. On
delivery, verify, then move the card. If the child reports `--blocked`, comment the blocker on
the card and leave it in progress or move it back to `todo`.

## Related skills

- `fleet-subagent-cli` — delegating the work a card describes.
- `rust-ipc-protocol` — if you are changing the board wire family rather than using it.
