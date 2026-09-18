# `scenarios/agents/` — the native-agent corpus

The surfaces that change most and break most: the floating agent popup, a native thread's
streaming turn, the decision drawer, the unread mark, and the `^s` table bound inside an agent
tab. Scenarios use `agents` or one of its additive subagent presets, which install
`fleet-harness agent` under the vendor's own name and hand it a scripted transcript — **no
scenario here needs the `claude` or `codex` binary, a network, or a token**
(`docs/NATIVE-AGENTS.md` §6.5).

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
| `prefix-inside-a-thread.scenario` | agent controls plus `^s 1`–`9`, `^s Tab`, `^s w`, and `^s s` are bound in an agent tab while an unknown second key is swallowed | `KEYMAP.md` §Shadowing, §Native agent thread |
| `scroll-wheel.scenario` | two fixture turns complete; wheel input moves up and back through the transcript without crashing | `NATIVE-AGENTS.md` §5 |
| `subagent-attach-from-picker.scenario` | `^s d` lists a hidden delegated child as `attach`; accepting it adds the child tab and focuses its composer | `NATIVE-AGENTS.md` §15; `KEYMAP.md` §Native agent thread |
| `subagent-reopen-closed-caller.scenario` | a locally closed caller remains in `AGENTS`; accepting it reopens the caller tab | `NATIVE-AGENTS.md` §15; `KEYMAP.md` §Native agent thread |
| `subagent-other-worktree-child.scenario` | the picker labels an other-worktree child and switches to its session before attaching it | `NATIVE-AGENTS.md` §15; `KEYMAP.md` §Native agent thread |
| `subagent-runs-end-to-end.scenario` | a scripted caller creates a real delegation; its child reports and delivery reaches `delivered` | `TESTING-HARNESS.md` §5; phase 4 P4-T01 |
| `subagent-attach-from-row.scenario` | `Enter` on a durable delegation row attaches and selects its child tab | `NATIVE-AGENTS.md` §15; phase 4 P4-T07 |
| `subagent-detach-and-reattach.scenario` | `^s x` detaches a child, and its caller's durable row re-attaches it | `NATIVE-AGENTS.md` §15; `KEYMAP.md` §Native agent thread |
| `subagent-blocked-child-paints-caller.scenario` | a blocked Claude child paints its selected Codex caller `needs_you` | `NATIVE-AGENTS.md` §3.3, §15; phase 4 P4-T07 |
| `subagent-up-to-caller.scenario` | `^s u` from a child selects its caller without detaching either thread | `KEYMAP.md` §Native agent thread; phase 4 P4-T07 |
| `subagent-result-card.scenario` | delivered delegation data projects a collapsed result card that `Enter` expands in place | `NATIVE-AGENTS.md` §15; phase 4 P4-T07 |

## UX surface audit

The native-agent surfaces added to the shared UX sections have executable coverage here:

| `UX-SPEC.md` surface | Scenarios |
| --- | --- |
| §3.6 Workspace: mixed strip, attached child ordering, child tab chrome, attach/detach and caller navigation | `subagent-attach-from-row`, `subagent-detach-and-reattach`, `subagent-up-to-caller` |
| §3.6.0 Native agent tab: streaming transcript, decisions, delegation row, blocked-child attention, result card, child caller metadata and composer boundary | `thread-streaming`, `edit-approval-allow`, `edit-approval-deny`, `subagent-runs-end-to-end`, `subagent-blocked-child-paints-caller`, `subagent-result-card`, `subagent-up-to-caller` |
| §3.9 Command palette: the `AGENTS` section and its attach, reopen, cross-worktree and focus outcomes | `subagent-attach-from-picker`, `subagent-reopen-closed-caller`, `subagent-other-worktree-child` |
| §9.6 component inventory: `TranscriptList`, `ToolRow`, `DelegationRow`, `DelegationResultCard`, `DecisionCard`, `MultilineInput`, targeted `MetadataSegment`, `Markdown` and `DiffView` | `scroll-wheel`, `codex-approval-shows-the-diff`, `prefix-inside-a-thread`, plus the delegation scenarios above |

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
- **A Codex approval waits for its prepared item join.** The gate can make attention observable
  one frame before the thread view prepares the named file-change item; the scenario waits for
  `decision.has_diff` before capturing or asserting the joined path.
- **Effort selection waits for the selected thread to be seen.** The model catalogue can advance
  attention while the composer remains selected, so the scenario awaits the resulting monotonic
  seen cursor instead of sleeping and sampling a transient `unread` state.
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
