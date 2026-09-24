---
name: fleet-board-planning
description: How an agent plans and tracks work with `fleet board` — picking the right board (context or worktree), reading it before writing, creating cards that are actionable, moving them through statuses as the work really progresses, recording decisions as comments, starting a worktree from a card, syncing a remote-backed board, and reading the JSON envelopes from a script, recording pull requests on a Reviews board, and scheduling a recurring agent task that feeds a board. Load it when asked to plan, break down, track, or report on tasks, when running inside a Fleet terminal or native-agent thread and about to touch a board, when orchestrating subagents whose work should be visible, when a pull request should be reviewed through a board, or whenever a command starts with `fleet board` or `fleet schedule`.
---

# Planning work on a Fleet board

A Fleet board is a Linear-style kanban: ordered **status columns** holding **cards**. Every
context has one board; a worktree may additionally have its own. Boards live in the daemon, so
a card an agent creates is visible in the app, to other agents, and after the agent's session
ends. The board is the durable plan; the transcript is not.

Authoritative docs: `docs/BOARD.md` (model, engine, CLI contract §6) and `docs/BOARD-JIRA.md`
(what a Jira-backed board can and cannot do). Command source: `crates/fleet-cli/src/args.rs`
(`BoardArgs`, `BoardCommand`, `BoardCardCommand`, `ScheduleArgs`) and
`crates/fleet-cli/src/commands/{board,schedules}.rs`.
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
   - `--reviews` selects the **Reviews board** of `--context <id>`, or of the active context, and
     creates it on first use. It conflicts with `--board` and `--worktree`. See "Review boards".

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

## Building a chain

A **chain** is a plan the board runs by itself: columns carry actions, cards carry links, and the
daemon starts an agent on each card as its blockers clear. It is worth building when the plan has
real dependencies and every step can be described completely in a brief; it is not worth building
for three cards you are about to do yourself.

Give the board the pipeline once:

```sh
B="fleet board --worktree=acme/api#feat-x"
$B columns preset workflow          # adds Ready and In review; never rewrites a column you have
$B columns edit in-progress --on-enter prompt \
  --instructions "Implement this card in the current worktree. Do not commit." \
  --expect "make lint and make test pass" --on-success in-review
$B columns                          # read it back: which columns run something, and where they route
```

Then build the cards, in this order and no other:

```sh
# 1. Create every card in Todo, the one column the preset leaves human.
a=$($B card new "Throttle the board to one run" --provider codex | head -1 | awk '{print $1}')
b=$($B card new "Give the column an action" --provider codex --blocked-by "$a" | head -1 | awk '{print $1}')
c=$($B card new "Review the throttle"        --provider codex --blocked-by "$a" | head -1 | awk '{print $1}')
d=$($B card new "Ship the workflow" --provider codex --blocked-by "$b" --blocked-by "$c" | head -1 | awk '{print $1}')

# 2. Move the dependants into Ready, where `when-unblocked` will collect them. Their blockers
#    are still in Todo, so they stay put in Ready until the last blocker of each is done.
$B card move "$b" ready; $B card move "$c" ready; $B card move "$d" ready

# 3. Move the head into Ready last. Nothing blocks it, so it starts at once.
$B card move "$a" ready
```

The rules behind that order, each of which costs a debugging hour when broken:

1. **Write every link while the card is still in Todo.** A card that reaches an action column
   before its blockers are recorded starts at once, and a run you did not want is a run you have
   to cancel and a worktree you have to clean.
2. **Ready starts whatever nothing blocks.** A card moved into Ready with nothing blocking it
   starts at once, or waits in Ready for a free slot. That is why the head goes in last: move it
   earlier and its run starts before the dependants are in place. A card waiting in Ready for a
   slot is *queued*, not stuck; it raises no attention, and the next freed slot moves it on.
3. **One card per verifiable outcome still applies.** A chain does not make a bad card good; it
   makes a bad card run.
4. **Watch, do not poll hard.** `board show` at phase boundaries; `card runs <key>` for one
   card's history; `card attach <key>` plus `fleet agent tail` to read a run live. `card wait`
   waits for the newest run of *one* card and exits 2 when none has started, so it is a barrier
   for a run, never for a chain — for a chain, look at the board.
5. **Never move a card that has a live run.** The move is refused, and `--cancel-run` is the
   deliberate way to say "stop what it is doing and move it anyway".
6. **A board runs one card at a time** unless `board set --max-live-runs N` says otherwise; every
   run of one board edits the same checkout, so raising it is a decision about that checkout.
7. **A run does not move its own card.** If you are a child (`FLEET_DELEGATION` is set), report
   with `fleet subagent complete` and let the column's `on-success` route the card; `card move`
   on your own card is refused.

`make smoke-workflow` runs exactly this chain end to end against a private daemon with scripted
agents; `scripts/board-workflow-smoke.sh` is the worked example, and it is the first thing to run
when a chain of your own does not move.

## Review boards

Every context can have one **Reviews board**: a board whose cards are pull requests to review,
each run in its own worktree checked out at the pull request's head. Reach it with `--reviews`:

```sh
R="fleet board --reviews"                       # the active context's; add --context <id> for another
$R card new "Fix the login redirect" --pr acme/api#42          # or the pull request's URL
$R card new "Fix the login redirect" --pr https://github.com/acme/api/pull/42 \
  --requested-at 2026-09-22T10:00:00Z --label github
```

`card new --pr` is idempotent. It prints `Created <KEY>`, `Existing <KEY>` or `Reopened <KEY>`
first, then the card. A pull request already on the board is never duplicated. A card in Review
published, or archived, is reopened (`Review re-requested`) only when `--requested-at` is later
than its last completion; otherwise it is `Existing`.
`--requested-at` without `--pr` is refused: `--requested-at needs --pr`.

The five columns, and what each does on its own:

| Column | Id | What happens |
| --- | --- | --- |
| Pending review | `pending` | Where a new card lands. With nothing blocking it, it moves on to Reviewing at once, or waits here for a free slot (two reviews run at a time). |
| Reviewing | `reviewing` | Creates the card's worktree from the pull request on first run, then reviews it there. The report is the review: a verdict, a summary, numbered findings with `file:line`. Nothing is posted. On success the card moves to Reviewed. |
| Reviewed | `reviewed` | Waits for you. Read the report on the card, and add a comment for every finding to drop or change; the publish run applies those notes. |
| Review published | `published` | **Moving a card here posts the review to GitHub** with `gh`, as one review with inline comments. Move it only when the review should be public. |
| Dismissed | `dismissed` | Drop a pull request you will not review. Nothing runs. |

A run on a Reviews board happens in the card's own worktree, never the board's, so reviews of
different pull requests never share a checkout. The repository must be a Fleet repository in
the context; clone it first, or the run is refused with `{owner/name} is not a Fleet repository;
clone it into this context first`.

`make smoke-reviews` runs this flow end to end against a private daemon with scripted agents;
`scripts/reviews-smoke.sh` is the worked example.

## Scheduling

A **schedule** is a prompt that belongs to one board. The daemon runs it with a coding agent every
N minutes, or once at a set time, headless (`claude -p` or `codex exec`) in a scratch directory of
its own and with your normal configuration: every MCP server, skill and CLI tool you have
installed is available to it.

Use a schedule for **recurring intake from a source Fleet cannot see**: review requests arriving
in a chat channel, an inbox, or GitHub. Do not use one for work Fleet already drives. A chain
runs cards by itself, and a card's column runs its agent.

```sh
S="fleet schedule --reviews"                    # same selectors as fleet board, plus --json
$S new --name "GitHub reviews" --starter github-reviews --every 15
$S new --name "Chat reviews" --prompt-file ~/prompts/chat-reviews.md --every 30 --timeout 10
$S list                                          # id, name, cadence, next, last outcome, last summary
$S run sch-1a2b3c4d --wait                       # fire now, whatever the cadence
$S runs sch-1a2b3c4d                             # every recorded run, with its log path
$S edit sch-1a2b3c4d --every 60                  # or --disable / --enable
$S rm sch-1a2b3c4d
```

What to know before writing one:

1. **The footer is the contract.** The daemon appends a fixed footer to your prompt. It tells the
   agent to record every pull request with
   `fleet board --board <id> card new "<title>" --pr <url> --requested-at <time> --label <source>`,
   to change nothing else, and to end with one line:
   `SUMMARY: <n> created, <n> existing, <n> reopened, <anything the user must know>`. Your prompt
   says only *where to look*. It does not repeat the footer. `--starter github-reviews` is a
   complete example of such a prompt.
2. **Placeholders** in your prompt are filled at fire time: `{board}`, `{now}`, and
   `{last_run_at}` (`never` before the first run). Use `{last_run_at}` to skip old messages.
3. **Every N minutes means at least 5** (`--every` takes 5 to 1440). A run that would overlap a
   live one is recorded `skipped`. After downtime the schedule fires once, never once per missed
   interval.
4. **Runs are jobs.** Each run is a daemon job of kind `scheduled_task`, visible and cancellable
   in the jobs panel. It has a log file under
   `<FLEET_HOME>/schedules/<id>/logs/`, a cost when the provider reports one, and a one-line
   summary taken from the `SUMMARY:` line. The last 20 runs are kept. Read `schedule runs` before
   you guess why a run failed.
5. **Headless means full access by default.** No one can answer a permission prompt, so `--mode`
   defaults to `full-access`. A narrower mode is your decision about what the prompt may do.
   `--timeout` (1 to 120 minutes, default 20) bounds each run.

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
- `docs/BOARD.md` §12 — the schedule model, how a run executes, and the CLI contract.
- `rust-ipc-protocol` — if you are changing the board wire family rather than using it.
