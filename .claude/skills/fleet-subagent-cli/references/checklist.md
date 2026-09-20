# Subagent usage checklist (fleet-subagent-cli)

Run the caller list before every `fleet subagent run`, the collection list before you wait or
end the turn, and the child list before `complete`. Each item names the rule in `SKILL.md`.

## Caller: before `run`

- [ ] This is worth a fresh context: it is separable, it does not need my conversation, and it
      is bigger than a few tool calls. (When to delegate / When not to)
- [ ] The brief is in a file and contains: goal, owned files and forbidden files, repo
      constraints and skills to load, the verification command, the report shape. (Rule 1)
- [ ] The brief does not repeat the footer's `complete` instructions. (Rule 1)
- [ ] `--expect` is something the child can run or check, not an adjective. (Rule 2)
- [ ] I chose the worktree on purpose: another worktree for isolation, or my own id passed
      explicitly with disjoint file ownership, or the default because I will end my turn. (Rule 3)
- [ ] Children sharing a Cargo tree each have their own `--env CARGO_TARGET_DIR=…`. (Rule 3)
- [ ] I have fewer than 4 live children, and I am not at depth 3. (When not to)
- [ ] If a board is in play, the card exists and its key is in the brief. (Rule 12)
- [ ] Model and effort are set on purpose or omitted for the defaults; `--mode` is
      `full-access` unless I accept a stranded gate. (Rule 10)

## Caller: collecting

- [ ] Every parallel child was started before the first `wait`. (Rule 4)
- [ ] I am collecting with `wait` or by ending the turn, not with a `status`/`sleep` loop. (Rule 5)
- [ ] A `wait` that exited 2 is followed by another `wait` or by ending the turn. (Rule 5)
- [ ] `--eager` is on only because I need the report inside this running turn. (Rule 6)
- [ ] Every child I no longer need is cancelled. (Rule 8)
- [ ] I ran the verification named in `--expect` myself before acting on `succeeded`. (Rule 9)
- [ ] `incomplete` and `failed` reports were read as work to review, not discarded. (Rule 9)
- [ ] I did not read a report from a file the child left behind; `wait` or `status` gave me the
      body. (Anti-patterns)

## Child: before `complete`

- [ ] I did not ask the user anything; assumptions are in the report. (Child 1)
- [ ] I touched only the files the brief gave me. (Child 2)
- [ ] The verification the brief or expectation names ran, and its result is in the report.
      (Child 3)
- [ ] The report is a file, short and structured: outcome, changes, verification, assumptions,
      leftovers. (Child 4)
- [ ] I am running `complete` exactly once, with no id or token on the command line. (Child 4)
- [ ] If blocked, the report says what is missing and I pass `--blocked`. (Child 5)
- [ ] After `complete` I stop: no new work, no background task left running. (Child 6)
- [ ] I set no `FLEET_*` variable and did not `cancel` or `wait` on my own delegation. (Child 8)

## Review (when checking someone else's orchestration)

- A brief under ten lines for a task that touches more than one file → finding.
- `run` / `wait` / `run` / `wait` with no overlap → finding.
- A `while` loop around `fleet subagent status` or `list` → finding.
- A card moved to `done` or a result relayed to the user without the caller's own verification
  → finding.
- Two children with overlapping owned files in one worktree → finding.
- A child that called `complete` before running the named verification → finding.
- A shared Cargo target directory across concurrent children → finding.
