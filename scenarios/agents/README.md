# `scenarios/agents/` — the native-agent corpus

The surfaces that change most and break most: the floating agent popup, a native thread's
streaming turn, the decision drawer, the unread mark, and the `^s` table bound inside an agent
tab. Every scenario runs on the `agents` preset, which installs `fleet-harness agent` under the
vendor's own name and hands it a scripted transcript — **no scenario here needs the `claude` or
`codex` binary, a network, or a token** (`docs/NATIVE-AGENTS.md` §6.5).

```sh
target/debug/fleet-harness run scenarios/agents --lane virtual        # the corpus
scenarios/agents/expected-to-fail/run.sh                              # the TODO watch list
```

Keyboard input reaches the focused view in both lanes. Scenarios with `shot` lines remain in the
`virtual` lane; `scroll-wheel.scenario` deliberately has no shot and also runs headless.

## What is here

| Scenario | Checks | Document |
| --- | --- | --- |
| `popup-open.scenario` | `a` / `A` open the floating popup, `^s q` hides it, the Hub keeps its selection | `KEYMAP.md` §Hub [A21], §Agent popup |
| `thread-streaming.scenario` | `^s a` starts a thread; idle → working → idle, tab mark and finished toast | `KEYMAP.md` §Native agent thread; `NATIVE-AGENTS.md` §3.3 |
| `edit-approval-allow.scenario` | the file-change gate arrives, owns the keyboard, and `[y]` is clicked on its own target | `NATIVE-AGENTS.md` §6.1, §6.2 |
| `edit-approval-deny.scenario` | the same gate answered `n`; the turn continues, and `Enter` is never an answer | `NATIVE-AGENTS.md` §6.2 |
| `codex-approval-shows-the-diff.scenario` | a Codex file-change approval joins its named item and renders that item's diff | `NATIVE-AGENTS.md` §6.2; `TODO.md` §1 |
| `codex-effort-menu.scenario` | `^s e` consumes Codex's discovered per-model effort vocabulary without sending the draft | `NATIVE-AGENTS.md` §7 |
| `unread-mark.scenario` | a turn that reaches its gate while you are on another tab marks the thread; looking clears the mark and not the gate | `NATIVE-AGENTS.md` §3.3, §6.1 |
| `unread-mark-survives-a-reconnect.scenario` | a thread read before a daemon restart remains read after reconnect through its persisted installation cursor | `NATIVE-AGENTS.md` §3.3, §10 |
| `prefix-inside-a-thread.scenario` | agent controls plus `^s 1`–`9`, `^s Tab`, and `^s w` are bound in an agent tab while `^s s` is not | `KEYMAP.md` §Shadowing, §Native agent thread |
| `scroll-wheel.scenario` | two fixture turns complete; wheel input moves up and back through the transcript without crashing | `NATIVE-AGENTS.md` §5 |

`expected-to-fail/` currently has no entries. Files placed there carry an extension a directory
run does not collect; `run.sh` runs them and inverts the verdict until they are promoted.

`blocked/` holds scenarios that are written but cannot run yet, each naming the one missing
piece. They are not failures, they are parked assertions.

## Behaviours pinned by this round

These assertions close regressions found while building the corpus:

- **A new thread focuses its composer.** Every new-thread path asserts
  `focused == agents.composer` before typing; no defensive composer click remains.
- **The Hub's attaching Codex popup can be hidden.** `popup-open.scenario` exercises `^s q`
  before a terminal model exists and verifies that focus returns to the Hub.
- **Answered Codex turns settle against the caller's turn id.** Both approval scenarios wait
  for idle and assert that no sticky error remains after the answer.
- **Queued printable keys retain text semantics.** `scroll-wheel.scenario` sends a second turn
  immediately after the first settles, then exercises wheel input in both directions.
- **A target is read with a quoted path step.** Target names carry `.` and `[]`, so a bare
  dotted path cannot name one; `targets["agents.approval.edit"] absent` is how a clause says
  that Codex is offered no `[e] edit` (`TESTING-HARNESS.md` §2). A dump is still the friendlier
  evidence when a reviewer wants to see the whole table.

## Why the remaining TODO items have no scenario

§1 is closed by `codex-approval-shows-the-diff.scenario`; the former §2 effort-picker gap is
closed by `codex-effort-menu.scenario`; and the former §3 reconnect failure is closed by
`unread-mark-survives-a-reconnect.scenario`. §4 (`[u] revert this edit`), §5 (attachments), §6 (a cold window read),
§7 (the raw NDJSON log) and §8 (per-instance homes) are absent capabilities rather than wrong
answers: nothing is drawn, so there is no snapshot state to assert and nothing for a scenario to
watch flip. Each becomes testable the same day it becomes visible.
