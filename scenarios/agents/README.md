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

Every scenario here is keyboard-driven, which used to keep all six out of the headless subset;
the keyboard reaches the focused view in that lane now, so the only thing keeping them in the
`virtual` lane is their `shot` lines.

## What is here

| Scenario | Checks | Document |
| --- | --- | --- |
| `popup-open.scenario` | `a` / `A` open the floating popup, `^s q` hides it, the Hub keeps its selection | `KEYMAP.md` §Hub [A21], §Agent popup |
| `thread-streaming.scenario` | `^s a` starts a thread; idle → working → idle, tab mark and finished toast | `KEYMAP.md` §Native agent thread; `NATIVE-AGENTS.md` §3.3 |
| `edit-approval-allow.scenario` | the file-change gate arrives, owns the keyboard, and `[y]` is clicked on its own target | `NATIVE-AGENTS.md` §6.1, §6.2 |
| `edit-approval-deny.scenario` | the same gate answered `n`; the turn continues, and `Enter` is never an answer | `NATIVE-AGENTS.md` §6.2 |
| `unread-mark.scenario` | a turn that reaches its gate while you are on another tab marks the thread; looking clears the mark and not the gate | `NATIVE-AGENTS.md` §3.3, §6.1 |
| `prefix-inside-a-thread.scenario` | `^s [`, `^s m`, `^s F`, `^s x` are bound in an agent tab and `^s s` is not | `KEYMAP.md` §Shadowing, §Native agent thread |

`expected-to-fail/` holds one scenario per open `TODO.md` entry that documents wrong behaviour
today. They carry an extension a directory run does not collect, so `fleet-harness run
scenarios/` stays green; `run.sh` runs them and inverts the verdict. When one starts passing,
rename it to `.scenario`, move it up one directory and close its `TODO.md` entry in the same
commit.

`blocked/` holds scenarios that are written but cannot run yet, each naming the one missing
piece. They are not failures, they are parked assertions.

## Things the corpus deliberately works around

Each of these is a real defect or limit found while writing these scenarios. None is in
`TODO.md`, so none gets an expected-to-fail scenario; they are recorded here so the workarounds
do not read as superstition.

- **A new thread focuses its tab, not its composer.** `focused` is `agents.tabs.tab[N]` right
  after `^s a`, and a `type` sent then is accepted and dropped, so every scenario clicks
  `agents.composer` before typing.
- **The Hub's Codex popup cannot be hidden.** After `A`, neither `^s q` nor `^s A` dismisses it
  and the Hub underneath stops answering keys — `o` does nothing. The Claude popup hides and
  reopens correctly, in the Hub and over a worktree, so `popup-open.scenario` ends on `quit`.
- **An answered Codex turn does not reliably settle.** After `n` it never settles; after `[y]`
  it settled on three unloaded runs and not on a suite run sharing the box. Both leave
  `fleetd.log` reading `dropping a Codex settlement for a turn that is not the running one` and
  the sticky error reading `native-agent storage failed: … event targets the wrong turn`. Both
  approval scenarios therefore assert that the gate closes and stop there, and no scenario here
  asserts `sticky_error absent`.
- **A target is read with a quoted path step.** Target names carry `.` and `[]`, so a bare
  dotted path cannot name one; `targets["agents.approval.edit"] absent` is how a clause says
  that Codex is offered no `[e] edit` (`TESTING-HARNESS.md` §2). A dump is still the friendlier
  evidence when a reviewer wants to see the whole table.

## Why `TODO.md` §4–§8 have no scenario

§1, §2 and §3 document behaviour that is *wrong* where a user meets it, and each has a scenario
here or in `blocked/`. §4 (`[u] revert this edit`), §5 (attachments), §6 (a cold window read),
§7 (the raw NDJSON log) and §8 (per-instance homes) are absent capabilities rather than wrong
answers: nothing is drawn, so there is no snapshot state to assert and nothing for a scenario to
watch flip. Each becomes testable the same day it becomes visible.
