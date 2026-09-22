---
name: fleet-subagent-cli
description: How a native Claude Code or Codex thread delegates work with `fleet subagent` — deciding what to delegate, writing a brief and an expectation the child can act on, choosing provider, model, effort, mode, and worktree, running several children without blocking, collecting results with wait or by ending the turn, cancelling, and the child-side `complete` contract. Load it when asked to delegate, parallelise, spawn a helper or subagent, when `FLEET_DELEGATION` is set in your environment (you are the child), or whenever a command starts with `fleet subagent` or `fleet agent`.
---

# Delegating with `fleet subagent`

A subagent is an ordinary native-agent thread (Claude Code or Codex) plus a durable
**delegation** record that ties it to the thread that spawned it. The daemon owns the child's
lifecycle, waits for it, and delivers its report back into the caller's transcript exactly once.
The CLI is the whole surface: `run` spawns, the child's `complete` reports, and `wait`, `status`,
`list`, `cancel` let a caller or a person look in.

Authoritative docs: `docs/NATIVE-AGENTS.md` §15 (lifecycle, delivery, limits, exact copy) and
`docs/decisions/0017-native-subagents.md` (why). Command source: `crates/fleet-cli/src/args.rs`
(`SubagentCommand`, `AgentTailArgs`) and `crates/fleet-cli/src/commands/subagents.rs`. Full flag
and output reference: `references/commands.md`. Pre-flight lists: `references/checklist.md`.

## Which role am I in?

- `FLEET_DELEGATION` is set → you are a **child**. Read "As the child" and do that.
- `FLEET_DELEGATION` **and** `FLEET_CARD` are set → you are a **card's run**: a board column
  started you, and rule 9 of "As the child" is the part that is different for you.
- `FLEET_SESSION` is a UUID and `FLEET_DELEGATION` is not set → you are a **native thread** and
  can be a **caller**.
- `FLEET_SESSION` looks like `owner/name#slug` or you are in a plain shell → you are a
  terminal-origin agent. `fleet subagent run` needs a native caller thread that is mid-turn, so
  delegation is not available to you; `status`, `list`, `wait`, and `cancel` still work for
  inspecting other people's delegations.

## When to delegate

- The work is separable: it can be described completely in a brief, done without your
  conversation history, and checked against a stated expectation.
- It is large enough to be worth a fresh context: a sweep across many files, a test suite to
  write, a second implementation or an independent review, a long build-and-fix loop.
- You want parallelism: several disjoint pieces that can run at once.

## When not to

- It takes you a handful of tool calls. A child costs its whole context plus your brief.
- It needs your judgement mid-way. A child cannot ask you; it reports blocked and stops.
- It needs the conversation. The child sees only the brief; if the brief would have to quote the
  transcript, do the work yourself.
- The depth or concurrency limits are already spent: depth 3, 4 live children per caller, 8 live
  delegations per daemon. A refused `run` says which rule.

## Rules for the caller

1. **The brief is the entire contract; write it to a file.** The child starts with your brief,
   one blank line, and Fleet's fixed footer (delegation id, expectation, the `complete`
   command). Nothing else. Put in the brief: the goal, the exact scope and the files it owns,
   what it must not touch, constraints the repo has (skills to load, `make lint`, `make test`),
   how to verify, and the shape of the report you want back. Use `--brief-file`; a heredoc on
   stdin works but is harder to reread. Never repeat the footer's instructions in the brief.

2. **`--expect` is the child's definition of done. Make it checkable.** It is printed in the
   footer as `The caller expects: …`. `tests in crates/fleet-cli pass and clippy is clean` is
   checkable; `do a good job` is not.

3. **Say which worktree, on purpose.** With no `--worktree` the child edits *your* worktree and
   `run` warns you. That is right for a child that continues your task after you end your turn,
   and wrong for a child running while you keep editing. Pass `--worktree <id>` for another
   worktree when isolation matters, or pass your own worktree id explicitly when you are running
   several children with disjoint file ownership in one tree (the warning is then silenced
   because you chose it). Children sharing a Cargo tree each get their own
   `--env CARGO_TARGET_DIR=…`, otherwise they serialise on one build lock.

4. **Start every child before waiting on any.** `run` returns as soon as the record exists.
   Launch all the parallel work, capture each id, and only then collect. A `wait` right after a
   single `run` is fine when there is nothing else to do; a `wait` between two `run`s
   throws the parallelism away.

5. **Collect in one of two ways, not by polling.**
   - **End your turn.** The report arrives as a new user message when you are idle, at most
     once. This is the default and costs nothing.
   - **`fleet subagent wait <id>`** blocks up to 540 s (`--timeout` raises it, but your own
     shell-tool ceiling still applies). Exit 0 prints the report; exit 2 prints one
     `still running` line and nothing else. Issued from the caller, a successful wait *consumes*
     the delivery, so the same report is not injected again when your turn ends.
   - Never loop `status` or `list` to watch progress. A `wait` that returned 2 is answered by
     another `wait` or by ending the turn, not by a sleep loop.

6. **Use `--eager` only when you need the result inside a running turn.** It steers the child's
   report into your active turn instead of waiting for you to go idle. For an orchestration loop
   that is `wait`ing anyway, it changes nothing.

7. **Peek without blocking when you must.** `fleet subagent status <id>` prints the record, the
   brief whole, the child's token spend, and, once terminal, the report byte-for-byte as `wait`
   prints it. `fleet agent tail <child> --no-follow --last 20` shows the child's last events and
   exits. Both are for a stuck or surprising child, not a heartbeat.

8. **Cancel what you will not use.** `fleet subagent cancel <id>` interrupts the child and every
   live descendant and delivers a `cancelled` report. A forgotten child keeps spending tokens
   and holds one of your four live slots.

9. **Verify the report; never relay it.** `succeeded` means the child ran `complete` and its
   turn settled with no open gate. It does not mean the work is right. Read the diff, run the
   verification you named in `--expect`, then act. `incomplete` means the child never reported
   and the daemon captured its last assistant text after two nudges; `failed` covers a
   `--blocked` report, provider errors, and exhausted turns; treat all three as work to review,
   not to trust.

10. **Choose model and effort deliberately, or omit both.** Omitted, the child uses the
    provider's configured defaults. `--effort` is free text passed straight to the provider (the
    legal ladder is the provider's); it may be given without `--model`. `--mode` defaults to
    `full-access` because nobody is there to answer a gate; narrow it only when you accept the
    child may strand on a permission prompt.

11. **Keep JSON where a script reads it.** `run --json`, `wait --json`, and `list --json` cut the
    brief to a 200-character preview and set `briefElided: true`; `status --json` carries it
    whole. The report body is `delegation.result.text`. Parse ids with `jq -r .delegation.id`.

12. **One card per delegation when a board is in play.** Create it before `run`, name the key in
    the brief, and move it on delivery after you verified (`fleet-board-planning`).

## An orchestration, end to end

```sh
# Brief: complete, scoped, with verification and the report shape.
cat > /tmp/brief-cli-tests.md <<'EOF'
Goal: add regression tests for `fleet subagent wait` exit codes in crates/fleet-cli.
Own: crates/fleet-cli/src/commands/tests.rs only. Do not touch args.rs or the daemon.
Context: a wait that reaches its timeout exits 2 and prints one `still running` line;
a terminal record exits 0 and prints the delivered message. See docs/NATIVE-AGENTS.md §15.3.
Load .claude/skills/rust-gpui-testing before writing a test.
Verify: cargo test -p fleet-cli, then make lint.
Report: what you added, the test names, the exact verification commands and their result,
anything you assumed.
EOF

# Start every child first.
A=$(fleet subagent run --provider codex --brief-file /tmp/brief-cli-tests.md \
      --expect "cargo test -p fleet-cli passes and make lint is clean" \
      --worktree acme/fleetd#feat-x --env CARGO_TARGET_DIR=/tmp/target-a \
      --title "cli wait tests" --json | jq -r .delegation.id)
B=$(fleet subagent run --provider claude --brief-file /tmp/brief-adr.md \
      --expect "docs/decisions/0017 amended; no code changes" \
      --worktree acme/fleetd#feat-x --json | jq -r .delegation.id)

# Then collect. Each wait consumes its own delivery.
fleet subagent wait "$A"; echo "exit $?"
fleet subagent wait "$B"; echo "exit $?"
# exit 2 → still running: wait again, or end the turn and receive it as a message.

# Verify before trusting.
git diff --stat && make lint && cargo test -p fleet-cli
```

## As the child

You were started with `FLEET_DELEGATION`, `FLEET_DELEGATION_TOKEN`, and `FLEET_SESSION` set
and a `fleet` on your `PATH`. No human is watching. The footer at the end of your first message
is the contract; this is how to honour it without wasting the caller's tokens or your own.

1. **Read the brief and the expectation, then work.** State assumptions in the report; do not ask
   questions. A question opens a gate nobody answers and the delegation stalls as `blocked`.
2. **Stay inside the scope the brief gives you.** Files it owns, nothing else. Another child may
   own the rest of the tree.
3. **Verify before reporting.** Run exactly what the brief or expectation names. The caller will
   run it again; a report that says "should pass" is a failed delegation in slow motion.
4. **Write the report to a file, then run `complete` once.** Short and structured: outcome in one
   line, what changed and where, the verification commands and their results, assumptions,
   anything left undone. Then:
   ```sh
   fleet subagent complete --result-file /tmp/report.md
   ```
   The id, thread, and token come from the environment; pass none of them. A second `complete`
   with a different body is refused; an identical one is an idempotent no-op. Above 256 KiB the
   report is truncated at ingest and marked elided, so keep it well under.
5. **Blocked means blocked.** If you genuinely cannot finish, say exactly what is missing and run
   `fleet subagent complete --blocked --result-file /tmp/blocked.md`. Do not spin.
6. **Stop after reporting.** Do not start new work or leave background tasks running: the
   delegation only succeeds once your turn settles with no live background work, and a leaked
   task holds it in `settling` for 30 s. If you forget to report, the daemon nudges you twice and
   then ends the delegation `incomplete` with your last message as the result; do not rely on it.
7. **You may delegate too**, up to depth 3, with the same rules. Your own children's reports
   reach you the same way.
8. **Never set or forward `FLEET_*` variables**, never `cancel` your own delegation, and do not
   `wait` on yourself.
9. **If `FLEET_CARD` is set, you are a card's run — do not move your own card.** A column started
   you, and that column's `on-success` route is what moves the card when your report lands. Moving
   it yourself races the route you are about to be given, so `fleet board card move` on the key in
   `FLEET_CARD` is refused: *a run cannot move its own card; its report moves the card when it
   finishes*. The refusal compares the key you typed, so it is advisory — the point is the rule,
   not the check. Report with `complete` and let the board route you.

   `FLEET_CARD` and `FLEET_BOARD` are read-only context, not a licence: read the card with `fleet
   board --board "$FLEET_BOARD" card show "$FLEET_CARD"` when the brief is not enough, and leave a
   `card comment` when you found something the next run needs. Everything else about the card —
   its column, its links, its runs — belongs to whoever is orchestrating.

## Reading the output

`run` prints `delegation <id> started, child thread <child>` and, when you omitted
`--worktree`, the sharing warning on a second line.

`list` and the first line of `status` print nine tab-separated fields: id, status, provider,
child thread, duration, total tokens, cost, delivery, caller. A `-` in the spend fields means the
daemon has no usage yet, not zero. The caller reads `thread <id>`, or `card <KEY>` for a run a
board column started. Status words: `starting`, `running`, `blocked`, `settling`,
`succeeded`, `incomplete`, `failed`, `cancelled`. Delivery words include `pending`,
`delivered`, `consumed`, `recorded` (a card run's board write committed), `undeliverable`.

A finished `wait`, and the message injected into your transcript, both read:

```text
[fleet subagent <id> finished: succeeded]
provider: codex, thread: <child>, duration: 14m 02s, files changed: 6

<the child's report>
```

A timed-out `wait` prints exactly one line and exits 2:

```text
[fleet subagent <id> still running after 9m 47s, status: running, thread: <child>]
```

Grep for `finished:` to tell them apart; the still-running line never contains it.

## Anti-patterns

- **A one-line brief.** The child has no history; "fix the failing tests" is a coin flip.
- **`run` then `wait` then `run` then `wait`.** Serial delegation with idle children slots.
- **A `status` loop with `sleep`.** The daemon already waits; `wait` or end the turn.
- **Trusting `succeeded`.** It is a lifecycle word, not a review.
- **Several children in one worktree with overlapping files, or a shared Cargo target dir.**
- **A child that asks the user a question.** It stalls until someone cancels it.
- **A child that reports early "to be safe".** The second, different report is refused.
- **Reading the child's report out of a file it left behind.** `wait` and `status` return the
  whole body; the file may be gone or stale.
- **Forgetting a child.** Four live children per caller; a stuck one blocks the fifth.

## Related skills

- `fleet-board-planning` — the card each delegation should live on.
- `zed-quality-review` — review the diff a child returns before you accept it.
- `rust-async-background-work` — if you are changing the delegation service rather than using it.
