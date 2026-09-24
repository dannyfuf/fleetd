# 0025 — Scheduled agent tasks run headless as daemon jobs

**Adopted** for `fleet_core::schedule`, the daemon's `Schedules` service and `ScheduleStore`, the
schedule protocol family, `fleet schedule`, and Board settings' Schedules section. A board may own
schedules: a prompt the daemon runs headless (`claude -p` or `codex exec`) on a cadence, whose
output is cards recorded through `fleet board card new --pr`. `docs/BOARD.md` §12 is the model.

## Context

A Reviews board (ADR 0024) is only as useful as the cards on it, and review requests arrive where
Fleet cannot see them: GitHub assigns reviewers, and a company MCP server posts requests in a chat
channel. The user already has the tools that read those sources — `gh`, MCP servers, skills —
installed for their agent. What is missing is something that asks the agent, on a cadence and with
nobody at the keyboard, to look and to record what it finds.

## Decision

- **A schedule is board-owned.** It names one `board_id`, its output is that board's cards, and it
  is edited in that board's settings and shown in that board's header. Deleting the board deletes
  its schedules and their directories. *Alternative rejected:* free-standing schedules, which would
  need their own surface and would outlive the board they write to.
- **It runs headless as a job, not as a native agent thread.** A thread needs a worktree and a tab,
  and nobody steers a fetch. A `JobKind::ScheduledTask` job gives cancel, a log, the Jobs panel and
  a recorded outcome for free. The run's cwd is a per-schedule directory under `FLEET_HOME`, its
  output is streamed to a per-run log file, and the last runs (20) are kept on the schedule with
  their outcome, `SUMMARY:` line and cost.
- **Every N minutes, or once — no cron in v1.** An interval between 5 and 1440 minutes covers the
  use; the 5-minute floor exists because every run costs money. Cron syntax is a second language to
  validate and explain before the first one has been used.
- **One catch-up run after downtime, never a burst.** A due time in the past becomes now, so a
  daemon that was down for an hour runs a 15-minute schedule once, not four times.
- **An overlapping fire is recorded, not run.** When a schedule is due while its previous run is
  still live, the daemon records a `Skipped` run and starts nothing. Two runs of one intake would
  race on the same sources and double the spend.
- **The default mode is full access.** Nobody is there to answer a permission prompt, so an agent in
  any narrower mode would stall until the timeout. The footer forbids reviewing, commenting,
  changing pull requests and editing files; the runner's timeout (20 minutes by default, at most
  120) bounds a run that misbehaves anyway. A user may pick a narrower mode per schedule.
- **The footer is the contract.** The daemon appends a fixed footer to every prompt that names the
  one command a run may use to record its findings — `fleet board --board <id> card new "<title>"
  --pr <url> --requested-at <time> --label <source>` — its three possible first lines, the
  `{last_run_at}` window for sources that keep old messages, and the `SUMMARY:` line the daemon
  reads back. The card-creation command is idempotent (ADR 0024), so the agent is told to run it
  for every request it finds and never has to remember what it already recorded.
- **Stored in one versioned document.** `<FLEET_HOME>/schedules.json`, version 1, validated on
  every load and save. A newer version is refused and left in place; a corrupt file is
  quarantined and read as empty, so a bad schedule never stops the daemon.

## Alternatives rejected

- **Running a schedule as a delegated native thread.** It would inherit reports and nudges nobody
  reads, need a worktree the intake does not use, and put a tab in front of the user every few
  minutes.
- **Having Fleet fetch GitHub itself.** It would cover one source of two; the agent with the user's
  own tools covers both, and the GitHub query survives as the starter prompt.
- **Several catch-up runs after downtime.** Each would read the same sources and find the same
  requests.

## Consequences

The capability `schedules` gates the five requests; `PROTOCOL_VERSION` stays 8, and the app shows
no Schedules section on a daemon without it. A schedule's job reaches every peer in the snapshot
and in `JobUpdated`, which no capability gates, so `JobKind::ScheduledTask` is spelt on the wire
through the existing extension point, `{"custom":"scheduled_task"}`: a peer built before schedules
decodes it as a custom job instead of dropping the connection. A run that was live when the daemon stopped is marked
`Failed` — `the daemon stopped during this run` — on the next start. Token spend is visible, not
capped: every Claude run records its cost, Codex runs record none because its exec output carries
none, and nothing runs while a schedule is disabled.
