# Fleet end-to-end testing harness

**Status: authoritative and frozen.** This document overrides the five phase plans wherever they
disagree. The envelope, command argument objects, scenario grammar, snapshot version 1, target
names, fixture names, transcript format, and path ownership are the stable surface on which the
parallel implementation stages work. Commands and optional data may be added, but shipped names
and meanings are not renamed or repurposed.

## 1. Transport

Fleet listens at the Unix socket named by `FLEET_HARNESS_SOCK`. Each frame is one compact JSON
object followed by `\n`; embedded newlines are JSON escapes. The runner sends one request and reads
its correlated response before sending the next command.

```json
{"id":7,"cmd":"key","args":{"keys":["ctrl-s","?"]}}
{"id":7,"ok":true,"data":{"handled":[true,true]},"error":null}
```

A request is always `{id: u64, cmd: string, args: object}`. A response is always
`{id: u64, ok: bool, data: object, error: string|null}`. Success has `error: null`; failure has an
empty or diagnostic `data` object and a useful error. Unknown commands receive a failure response
rather than closing the connection. Command values are additive-only: a new command is a new
variant, never a frame-shape change.

### Commands and exact argument objects

| `cmd` | `args` | Result / meaning |
| --- | --- | --- |
| `meta` | `{}` | run id, lane, logical bounds, scale, title, frame |
| `quit` | `{}` | request orderly application shutdown |
| `wait` | `{"millis":u64}` | compatibility wall-clock delay |
| `key` | `{"keys":[string,…]}` | GPUI keystrokes; returns handling per key |
| `type` | `{"text":string}` | composed text, including spaces |
| `shot` | `{"name":string}` | settle and return geometry; the runner captures pixels |
| `dump` | `{"name":string}` | complete `UiSnapshot` plus its summary |
| `await` | `{"predicate":string,"timeout_ms":u64}` | wait for a predicate; scenario default is 5000 ms |
| `assert` | `{"predicate":string}` | evaluate once and return the snapshot on failure |
| `move` | `{"to":location}` | move the pointer |
| `click` | `{"at":location,"button":button,"count":u8}` | move, then click |
| `press` | `{"at":location,"button":button}` | move, then button down |
| `release` | `{"at":location,"button":button}` | move, then button up |
| `drag` | `{"from":location,"to":location,"button":button,"steps":u16}` | interpolated drag; scenario default is 8 |
| `hover` | `{"at":location,"dwell_ms":u64}` | move and hold the pointer for the dwell, capped at 5000 ms |
| `scroll` | `{"dx":f32,"dy":f32,"at":location|null}` | wheel units at an optional location; one unit is 18 logical pixels |
| `clipboard_set` | `{"text":string}` | replace the application clipboard |
| `clipboard_get` | `{}` | return clipboard text |
| `resize` | `{"width":u32,"height":u32}` | logical window size |
| `blur` | `{}` | drop the window's focus and take the pointer off it |
| `focus` | `{}` | activate the window |
| `advance` | `{"millis":u64}` | advance the app's own stored dwells; answers `{"millis":u64,"changed":bool}` |

A `location` is exactly `{"target":"worktrees.row[0]"}` or `{"x":120.0,"y":64.0}`. A button
is `left`, `right`, or `middle`. Pointer commands dispatch a move before a button event and answer
only after the resulting frame has painted. A named location lands at the center of the target's
visible intersection with the window; a target painted wholly outside the window fails instead of
dispatching an off-window event.

Four commands carry a consequence worth stating once rather than rediscovering in a scenario:

- **`scroll` is GPUI-signed.** `dy` is negative to scroll content *down*, and one unit is 18
  logical pixels, which is the unit the lazygit driver's `wheel <rows>` used: its `wheel N` is
  `scroll 0 -N` and its `hwheel N` is `scroll -N 0`.
- **`blur` does not deactivate the window.** GPUI exposes no platform deactivation — activation
  only ever arrives *from* the compositor — so `blur` clears the window's focus and dispatches a
  pointer exit, which is the half an application can observe. `focus` is a real activation.
- **`advance` moves the app's own stored dwells only**: toast dwell, expiry and coalescing, the
  cold-start splash and the reconnect banner's countdown. It does not move
  `BackgroundExecutor` timers — the 150–400 ms debounce windows, the attach retry and the
  pull-request cache deadline are futures on the real monotonic clock that only a test
  dispatcher can rewind — and it does not reach `fleetd`'s clock at all. Wait those out with
  `await`.
- **`clipboard_set` and `clipboard_get` fail in every lane today, and that is deliberate
  loudness, not a missing feature.** `clipboard_set` writes and then reads its own write back,
  because GPUI's headless platform accepts a write and drops it and a paste scenario that passed
  against nothing would be worse than one that failed. The read-back also comes back empty on
  Wayland — verified on Hyprland 0.56.2 in the `virtual` lane, where `shot` captures fine and
  `clipboard set` still answers `no platform clipboard in this lane: this Fleet has no compositor
  surface` — so no scenario may use the clipboard until that is fixed. The command is part of the
  frozen grammar; it has no working lane.

### Environment

Harness mode is off unless the environment asks for it, and reading these costs one branch at
startup.

| Variable | Meaning |
| --- | --- |
| `FLEET_HARNESS=1` | Pin the window size and title and turn motion off. |
| `FLEET_HARNESS_SOCK` | Listen for commands at this path. Either variable alone enables harness mode. |
| `FLEET_HARNESS_RUN_ID` | Names the run, and through it the window title. Defaults to the socket's file stem, then the pid. |
| `FLEET_HARNESS_LANE` | The lane label `meta` reports. Only the runner can tell a dedicated virtual output from the developer's own session; behaviour always follows the app's own compositor detection, never this label. |
| `FLEET_HARNESS_SIZE=WxH` | Overrides the pinned logical window size. The default is `1440x900`. A malformed value falls back to the default rather than varying. |

`meta`, `resize` and `shot` answer with the same geometry object, and `shot` adds the `name` it
was given:

```json
{"run_id":"…","lane":"virtual","bounds":{"x":0.0,"y":0.0,"w":1440.0,"h":900.0},
 "scale_factor":1.0,"title":"Fleet [harness:…]","frame":6}
```

`frame` is the single frame counter: the target recorder stamps every rectangle with it, so
`targets[*].frame == window.frame` compares one counter with itself.

`dump` answers `{name, revision, summary, snapshot}`, where `snapshot` is the whole `UiSnapshot`
and `summary` is its one-line echo. `assert` and `await` answer
`{predicate, satisfied, clauses:[{clause,satisfied,actual}]}`, and add `idle` and `snapshot` when
the predicate did not hold. A failed `assert` and a timed-out `await` are `ok:false` responses
carrying that data, which is what "failure has an empty or diagnostic `data` object" allows.

## 2. Scenario files

Files are UTF-8 and line-oriented. Blank lines and lines whose first non-space character is `#`
are ignored. A scenario may open with one `fixture: <preset>` line, and if it does that line must
be the first non-comment directive and appear once — a preset seeds `FLEET_HOME` before `fleetd`
reads it, so it cannot be applied half way through a run. A scenario that names no preset gets
`empty`, which is a Fleet that has never been run. Corpus scenarios name their preset explicitly.
Arguments after `type` and `clipboard set` have leading and trailing whitespace trimmed; interior
spaces are preserved. The frozen line grammar is:

```text
fixture: empty|one-repo|busy|board|agents|agents-subagent|agents-subagent-other-worktree|board-workflow
meta
quit
wait <milliseconds>
key <gpui-keystroke> [<gpui-keystroke> ...]
type <text>
shot <name>
dump <name>
await <predicate> [timeout-ms]
assert <predicate>
move <target>|<x> <y>
click [left|right|middle] [count] <target>|<x> <y>
press [left|right|middle] <target>|<x> <y>
release [left|right|middle] <target>|<x> <y>
drag <from> <to> [steps]
hover <target>|<x> <y> [dwell-ms]
scroll <dx> <dy> [at <target>|<x> <y>]
clipboard set <text>
clipboard get
resize <width>x<height>
blur
focus
advance <milliseconds>
daemon kill|stop|cont|restart
socket remove
job success|failure|long|repeat <count>
```

`daemon …`, `socket remove` and `job …` are runner-side operations, never app commands. `job`
submits a real daemon job of the named shape through the run's own client, because the job
registry is in memory and nothing seeded before the run survives into it; `long` is stopped at
teardown. `daemon restart` stops fleetd the way `fleet daemon restart` does — the PTY holders
stay up and the replacement adopts the same sessions — so a scenario can prove what a restart
keeps; `daemon kill` is the crash. A directory
run recursively executes scenario files in lexical order. Unless `--continue-on-failure` is set,
the first failed line stops the scenario and triggers `failure-NNN` shot/dump evidence.

The five pinned `idle` forms are `await idle`, `await idle <timeout-ms>`, `await idle exists`,
`await idle absent`, and `await idle exists <timeout-ms>`. A numeric second token is the timeout
only for the bare `idle` form; `exists` and `absent` remain predicate operators.

### Predicates

An atom is `<dotted.path> <op> <value>`, `<dotted.path> exists`, `<dotted.path> absent`, or the bare
word `idle`. Atoms may be joined only by `&&`; there are no parentheses or precedence rules.
Operators are `==`, `!=`, `~=`, `>`, and `<`. Values are JSON scalars; an unquoted residual token
is treated as a string. `~=` uses a regular expression. Paths traverse object keys and array
indices, for example `lists.worktrees.selected.label`, `toasts[0].text`, and `terminal.text`. A
key that is not a bare name is written as a quoted step, `["…"]`: the `board.cards` list is
`lists["board.cards"]` and a target is `targets["worktrees.row[0]"]`, because both of those names
are frozen in §3 and carry the `.` and `[]` a bare path would split on. Inside those quotes only
`\"` and `\\` are escapes, the same rule values follow.
Missing paths make comparisons false; `exists` and `absent` test that condition explicitly.
`absent` holds for a path that is missing *or* JSON `null`, and `exists` requires a non-null
value, because the snapshot reports an unavailable optional surface as `null`. `null` is itself a
legal unquoted value. Inside `"…"` only `\"` and `\\` are escapes, so a regular expression
reaches the matcher unaltered. Every clause is evaluated — a conjunction names all its offenders,
not just the first — and each one reports the value it actually saw.

`idle` is true exactly when `idle.idle` is true: no in-flight app requests, running jobs, pending
frame, live toast timer, or armed debounce; no mutation still awaiting its snapshot; and a daemon
link past its first connection. A mutation settles on the first snapshot whose daemon revision
covers the revision stamped on its reply. Against a daemon without the `snapshot.revision`
capability, the next snapshot or the grace backstop releases it. On an await timeout the response
contains the last snapshot and all eight idle fields: `idle`, `in_flight_requests`, `running_jobs`,
`pending_frame`, `live_toast_timers`, `armed_debounces`, `settling_mutations`, and `link_opening`.

## 3. `UiSnapshot`, version 1

JSON object field order follows the order below and is pinned by a serialization test. Version 1
already contains the final target, terminal, agent, and idle fields; adding them later must not
bump the version. Additive optional fields remain version 1. A version changes only if a shipped
field's meaning or type must change, and runners reject unknown versions by naming both values.
The snapshot is built from update-path state and memoised per revision. Render never builds it.

| Field | Shape and vocabulary |
| --- | --- |
| `version` | integer, currently `1` |
| `screen` | `Hub` or `Workspace` |
| `mode` | `Normal`, `Terminal`, `Native`, `Agent`, `Prefix`, `Scroll`, `Filter`, `Palette`, `Dialog`, `Jobs` |
| `hub_pane` | `Repos`, `List`, or null outside Hub |
| `hub_tab` | `Worktrees`, `Prs`, `Board`, or null outside Hub |
| `overlay` | `Filter`, `Palette`, `Jobs`, `Dialog`, or null |
| `key_contexts` | live outer-to-inner stack using the exact words in `KEYMAP.md`; `Fleet` may be implicit at the root |
| `focused` | stable focus-owner target name or null |
| `lists` | map name → `{rows:[row],selected:row|null,filter:string}` |
| row | `{id:string,label:string,badges:[string],marks:[string]}` |
| `dialog` | `{name,fields:[{name,value,focused}],buttons:[string],message:string|null}` or null; `message` carries the Confirm dialog's consequence sentence — the one line §3.8.3 has the user accept, including the two cancel cards' own — and is `null` over every other dialog, whose body is elements rather than a sentence |
| `toasts` | `[{level,text,count}]`; levels use the UX vocabulary `info`, `success`, `warning`, `error` |
| `sticky_error` | stable human-readable failed-job text or null |
| `jobs` | `[{id,status}]`; status is the daemon job vocabulary (`queued`, `running`, `succeeded`, `failed`, `cancelled`) |
| `agents` | `{popup:string|null,threads:[{id,provider,state,unread,pending_gate,decision,parent,attached,delegation_rows,result_cards,focused_row,expanded_result_cards}],delegations:[{id,status,caller,child,delivery,headline}],decision:{kind,item,diff}|null}`; `popup` is the floating popup's sub-mode `Terminal`, `Prefix`, or `Scroll`, or null when it is closed; providers are `claude`/`codex` and gate/state words match `NATIVE-AGENTS.md`; additive version-1 `threads[].decision` is null or `{kind:"permission"|"question"|"plan",title:string,paths:[string],has_diff:bool}` prepared from the rendered decision; additive `threads[].parent` is the caller thread id or null and `threads[].attached` says whether this window currently shows the thread in its strip; additive `threads[].delegation_rows` counts projected delegation items and `threads[].result_cards` counts delivered delegation-origin user messages; `threads[].focused_row` is null or `delegation`, `delegation_result`, or `other`, and `threads[].expanded_result_cards` reports the mounted view's expanded delivered cards, so scenarios can await transcript row state before acting; delegation `status` and `delivery` use their compact protocol words and `headline` is nullable; the legacy active-thread `agents.decision` remains available |
| `terminal` | `{rows:[string],text:string,cursor:{row,col,shape},viewport:{top,rows,history}}` or null; cursor shapes are `block`, `bar`, `underline`, `hidden` |
| `targets` | map target name → `{x,y,w,h,frame}` in logical window coordinates |
| `daemon` | `{link,attempt,dismissed,restarted}`; `link` is `starting`, `failed`, `connected`, `lost` or `reconnected`. An additive version-1 field: `key_contexts` appends `Daemon > Banner` only behind a chain that can carry it, so on a first-run Fleet or behind an open overlay a lost daemon is otherwise invisible to every predicate |
| `idle` | `{idle,in_flight_requests,running_jobs,pending_frame,live_toast_timers,armed_debounces,settling_mutations,link_opening}`; `idle` is *derived* from the seven pending-work inputs, never reported independently of them |
| `window` | `{bounds:{x,y,w,h},scale_factor,title,frame}` in logical coordinates |

An unavailable collection is empty, an unavailable optional surface is null, and unavailable
window metrics are zero/empty rather than guessed. `terminal.text` joins `rows` with `\n`;
`rows` drops the spacer cell that follows a wide grapheme and renders an unset cell as a space, so
column alignment survives the round trip. It trims trailing spaces from each row, then removes
trailing empty rows, so `terminal.rows.len()` need not equal `terminal.viewport.rows`. A scenario
must not assume the viewport's last row exists in `terminal.rows`. `jobs[].status` may also be
`cancelling`, which is a real daemon job state.

The list names are `repos`, `worktrees`, `prs`, `jobs`, `tabs`, `palette`, `board`,
`board.cards`, `board.summary`, `card.runs`, `card.properties` and `settings.columns`. The last
four are additive version-1 lists and each is absent unless its surface has something to say:
`board.summary` while the board states a run count, `card.runs` while the card detail is open on
a card that has run, `card.properties` while the card detail is open, `settings.columns` while
Board settings is open. While the agent picker is
open, `lists.palette` projects its native-thread rows:
`id` is the thread id; `label` is the rendered picker label; `badges` are provider,
`caller`/`child`, and worktree id; and `marks` are attention, `go`/`attach`, and
`attached`/`hidden`. An agent tab whose thread has a parent includes the additive `child` badge. The
last of those, and every key of `targets`, carries characters a dotted path would split on, so a
predicate reaches them with the quoted step §2 defines: `lists["board.cards"].rows[0].label`,
`targets["worktrees.row[0]"].frame`. The
`focused` vocabulary is `filter.input`, `palette.input`, `dialog`, `agents.popup`, `board.filter`,
`repos.row[N]`, `worktrees.row[N]`, `prs.row[N]`, `jobs.row[N]`, `board.column[C].card[R]`,
`tabs.tab[N]`, `agents.tabs.tab[N]` and `agents.composer`.

Board workflows add row vocabulary rather than fields. Two of the changes are **not** additive,
and a scenario written before them can read differently: `board.cards` and a `board` row's badge
now project the column as the pane draws it — ordered by position, the board filter applied and
archived cards left out — where they used to carry every card of the status in document order;
and `dialog.message`, documented as always `null`, now carries an open Confirm's consequence
line. Everything else below is additive within version 1.
A `board.cards` row carries, in this order, its card's run mark — one of `working`, `pending`,
`stalled`, `needs you`, `done` — then `blocked:<n>` for the cards still blocking it, then the
assignee it already carried; a card whose tile draws neither mark carries only the assignee, so
`marks[0]` is the run state whenever there is one. `board.cards` is the focused column exactly as
the pane draws it — ordered by the card's position, archived cards left out, and narrowed by the
board filter — so `rows[R]` and `focused == board.column[C].card[R]` always name the same card;
a `board` row's badge counts that same population. A `board` row carries `action` when its
column starts a run on arrival, which is the `on_enter` automation alone. `board.summary` holds
the board header's two counts as one row in their compact form — `1/1 working · 1 needs you`,
each half omitted while its count is zero, where the header itself reads `1 of 1 run working` —
and the list is absent when both are. `card.runs` is one row per run of the open
card, oldest first: `label` is the run row the detail draws, `badges` is the provider, and
`marks` is that run's mark word. `card.properties` is one row per row of the open card's
property column, top to bottom — the index `card_detail.property[N]` paints — with `label` the
field's name (empty on a second link row), the one badge its value as drawn (`–` when unset), and
`locked` in `marks` on a row its backend owns. `settings.columns` is one row per column of the board the
dialog is editing, `label` its name, marked `action` on the same rule as `board` and `disabled`
on a board that may not carry automation at all — a context board, or a board whose columns
answer to its backend. The elapsed time inside a `card.runs` label is as of the last projection:
nothing keys on the wall clock, so a scenario awaits a row or a mark, never a duration.

`lists.jobs.rows` is the row set accepted by the Jobs panel's current filter, in the order the
panel draws it — live and failed jobs first, then the "Finished" group (succeeded and cancelled),
each in the daemon's order — and `lists.jobs.selected` is the row at the panel cursor within that
filtered set. Thus `jobs.row[N]` and `lists.jobs.rows[N]` address the same job.
`lists.jobs.filter` is `""` for All, then `running`, `failed` or `done`.

The snapshot is memoised behind a key naming every input its builder reads, and its revision moves
only when the projected content actually differs — a repainted frame that changes nothing does not
wake a waiting `await`. A key that misses an input is a stale snapshot, which is an `await` that
hangs until its timeout; adding a field to the builder means adding its input to the key.

### Target names

Names have the stable form `<surface>.<part>[<index>]`, with lower-case surface and part words.
Indices describe current visual order; rows also carry stable domain ids in `lists`. A target's
`frame` must equal `window.frame` before input uses it. An unknown target fails and names the
nearest available names; a stale one fails and names both frame numbers. Neither dispatches
anything, and neither falls back to `(0,0)`.

For virtualized list rows, `N` is the model position handed to the row builder. Only rows inside
the viewport are painted, so `targets["<list>.row[N]"]` is absent while that model row is scrolled
out; scroll it into view before targeting it.

The names Fleet paints today, by surface:

| Surface | Names |
| --- | --- |
| Sidebar | `repos.rail` (the whole sidebar), `repos.row[N]`, `repos.row[N].menu` (a repository's `⋯`, painted while the row is hovered), `repos.clone` (the `+` beside `Repositories`), `repos.collapse` (the foot button, the pointer's `H`), `repos.resize` (the draggable edge), `agents.sidebar.row[N]` (the Agents section's rows, absent while there are none). `repos.rail`'s `w` is the collapse and drag oracle — 232 expanded by default, 44 collapsed, the dragged width (200–320) after `drag repos.resize <x> <y>`. |
| Hub lists | `worktrees.row[N]`, `prs.row[N]`, `jobs.row[N]`, `hub.tab[N]`, `prs.tab[N]` |
| Worktrees page | `worktrees.new`, `worktrees.clone`, `worktrees.filter`, `worktrees.row[N].open`, `worktrees.row[N].menu`, `worktrees.row[N].log` |
| Pull requests | `prs.filter`, `prs.refresh`, `prs.retry`, `prs.more`, `prs.row[N].open`, `prs.row[N].menu` |
| Detail panel | `detail.open`, `detail.sleep`, `detail.menu`, `detail.copy_path`, `detail.inspect` |
| Title bar | `titlebar.context`, `titlebar.command`, `titlebar.needs_you`, `titlebar.jobs`, `titlebar.update`, `titlebar.daemon`, `titlebar.help`, `titlebar.settings`, `titlebar.back`, `workspace.back`, `workspace.switcher`, `workspace.pr` |
| Status bar | `statusbar.shortcuts`, `statusbar.commands` |
| Board | `board.column[C]`, `board.column[C].card[R]`, `board.filter`; the pointer controls `board.new` (New card), `board.sync` (the sync button; absent on a local board), `board.settings` (Board settings), `board.column[C].add` (a column's `+`) and `board.column[C].card[R].menu` (a card's `⋯`, painted while the card is hovered or selected). `board.filter` is the header's filter field, painted always. The card menu's entries are `menu.item[N]`. |
| Filter and palette | `filter.input`, `filter.clear` (the query's clear ✕, only while it holds text), `palette.input`, `palette.row[N]` (one flat numbering down the ranked list, across its sections; `palette.row[0]` is the top match) |
| Dialogs | `dialog.field[N]`, `dialog.row[N]`, `dialog.close`, `dialog.button[N]`, `dialog.checkbox`, `dialog.segment[N]` |
| Help | `help.search` (also `dialog.field[0]`), `help.tab[N]` (0 Guides, 1 All shortcuts), `help.here[N]` (the *Here in …* rows), `help.guide[N]` (by the guide's position in the full list, searched or not), `help.step[N].action[M]` (the shown guide's step `N`, button `M`, both from 0), `help.shortcut[N]` (the table's rows, or the actions a Guides-tab search lists), `help.place[N]` (0 All places, then the catalogue places in order), `help.run`, `help.related` |
| Settings | `settings.search`, `settings.section[N]` (the rail, `0` General, `1` Agents, `2` Sleep, `3` Jobs & warnings, `4` Pool, `5` GitHub, `6` Status, `7` Hosts, `8` About), `settings.row[N]` (the shown section's rows, `0` first), `settings.switch`, `settings.option[N]`, `settings.dropdown`, `settings.copy[N]`, `settings.hit[N]`, `settings.config`, `settings.doctor`; its footer is `dialog.button[0]` Cancel and `dialog.button[1]` Save |
| Sheets | `sheet.close` |
| Card detail | `card_detail.close` (the sheet's ✕, also `sheet.close`), `card_detail.title` (a click edits the title, as `i`), `card_detail.menu` (the header's ⋯), `card_detail.property[N]` (the property column's rows, `0` Status, numbered as `card.properties`), `card_detail.comment` (the composer's *Add a comment…*, as `c`), and on the run card `card_detail.run.attach`, `card_detail.run.rerun` and `card_detail.run.cancel`, each painted only while its action can work on the card |
| Jobs panel | `jobs.row[N].retry`, `jobs.row[N].cancel`, `jobs.row[N].log`, `jobs.filter[N]`, `jobs.clear`, `jobs.more`, `jobs.log.back`, `jobs.log.follow`, `jobs.log.end` |
| Tabs | `tabs.tab[N]` for a process tab, `agents.tabs.tab[N]` for a conversation, sharing one numbering; `tabs.tab[N].close` / `agents.tabs.tab[N].close` (the tab's `✕`), `tabs.new` (the `+`), `tabs.fallback`, `tabs.watch`, `tabs.zoom` |
| Toasts and errors | `toasts.toast[N]` (0 is the oldest, matching the `toasts` array), `toasts.toast[N].action`, `toasts.toast[N].close`, `sticky_error.retry`, `sticky_error.close` |
| Daemon banner | `banner.button[N]` (`0` Reconnect now, `1` Open log), `banner.close` |
| First run | `first_run.step[N]` (`0` Create a context, `1` Clone a repository, `2` Start a worktree and an agent), `first_run.import`, `first_run.help`, `first_run.settings` |
| Menus | `menu.item[N]`: the items of the one open kit `Menu` (a ⋯, `+`, right-click or dropdown menu), numbered over the visible items in order, separators and headers skipped |
| ⌃S command menu | `prefix_menu`, `prefix_menu.item[N]`, `prefix_menu.close` |
| Native agents | `agents.popup`, `agents.popup.agent[N]` (the header's provider switch: 0 Claude, 1 Codex), `agents.popup.restart`, `agents.popup.hide`, `agents.transcript`, `agents.composer`, `agents.decision`, `agents.approval.allow_once`, `agents.approval.allow_always`, `agents.approval.deny`, `agents.approval.deny_and_stop`, `agents.approval.edit`, `agents.send`, `agents.tool[N]`, `agents.row[N]` |

The Worktrees page's header paints `worktrees.filter` (the idle filter field; while the filter is
being edited the same box is `filter.input`), `worktrees.clone` and `worktrees.new` (the primary
*New worktree*). A row's hover actions, `worktrees.row[N].open` (*Open*) and
`worktrees.row[N].menu` (the `⋯` trigger), are drawn only while that row is hovered or selected,
so a scenario clicks the row first; `worktrees.row[N].log` is the *View log* button of a row whose
hooks failed. Right-clicking `worktrees.row[N]` opens the same menu at the pointer. The detail
panel's buttons are `detail.open`, `detail.sleep`, `detail.menu` (its `⋯`), `detail.copy_path` and
`detail.inspect` (only while the worktree's safety is not known); the panel is open by default at
the harness's 1440 px window.

`hub.tab[N]` is the title bar's section nav — `0` Worktrees, `1` Pull requests, `2` Board — painted
only on the Hub. The `titlebar.*` names are its other controls. `titlebar.context` is the context
switcher; clicking it opens a kit menu whose `menu.item[N]` are the contexts in order, then *New*,
*Edit* and *Delete context*. `titlebar.command` opens the palette, `titlebar.help` Help and
`titlebar.settings` Settings. Four are conditional: `titlebar.needs_you` exists only while an
agent thread needs you (one waiting opens that thread, two or more the agents picker),
`titlebar.jobs` only while a job runs or has failed, `titlebar.update` only on the Hub with an
update available, and `titlebar.daemon` only while the daemon is unhealthy. In the Workspace the
switcher and the section nav give way to the breadcrumb: `titlebar.back` (`⌃S s`), also recorded as
`workspace.back`; `workspace.switcher`, the worktree switcher, whose `menu.item[N]` are the
sessions in `⌃S W` order, then *Last session* and *All sessions…*; and `workspace.pr`, the pull
request button, painted only while the branch has one. The status bar paints
`statusbar.shortcuts` (Help) everywhere and `statusbar.commands` (`⌃S`, entering the prefix) over
a terminal or a Fleet-drawn pane, not over a native agent thread.

The tab strip paints `tabs.tab[N].close` (or `agents.tabs.tab[N].close`) on every tab — hidden, but
laid out, until the tab is hovered or active, so a scenario clicks it on the active tab or after a
`move` onto the tab. `tabs.new` is the `+`; clicking it opens a kit menu whose `menu.item[N]` are
*Terminal*, *Claude thread*, *Codex thread*, *Board*, then *Lazygit* on a worktree session and
*Terminal fallback* on an agent tab. A right-click on a tab selects it and opens its menu:
*Rename* and, on an exited PTY, *Restart command* for a terminal, then *Close* and, with more than
one tab, *Close others*. `tabs.zoom` is always painted; `tabs.watch` only while the session has a
subagent watch; `tabs.fallback` only on an agent tab.

`prefix_menu` is the ⌃S command menu's panel, painted only once a held prefix has waited out
`motion.prefix_hint_delay` — a scenario awaits it rather than assuming it. `prefix_menu.item[N]`
counts its rows column by column, top to bottom, in the order the menu draws them; the order
follows the action catalogue, so a scenario that clicks a row names what that row is in a
comment and checks what it did. `prefix_menu.close` is the header's Close button.

`dialog.field[N]` counts the dialog's **tab cycle**: create-worktree `0` branch / `1` base /
`2` host (absent on a single-host daemon); new and edit context `0` name / `1` owners;
rename-terminal `0`; clone-repo `0` search; help `0` search; edit-hooks `0..` prepare commands then post-create;
new-card `0` title / `1` description; card-property `0` query. `dialog.row[N]` is a dialog's
result list — the assign-repo contexts, the clone-repo matches, the create-worktree base refs, the
card-property values (painted while on screen: the list scrolls past eight rows).

Two names in the table are real but conditional, and a scenario that assumes them unconditionally
will fail on an unknown target rather than on the thing it meant to check. `dialog.row[N]` exists
only in the dialogs that have a result list; `scenarios/hub/create-worktree-clicks.scenario` opens
the create-worktree base list, whose rows are there once the refs have arrived. `agents.approval.edit` is painted only where the provider accepts an amended
invocation (the `DecisionDock` contract in `docs/DESIGN-SYSTEM.md`); the other four approval
controls are always there.

`agents.composer` is the editor's own row, not the stack of editor, host badge and metadata strip
it sits on top of: a target is aimed at, and a `click` resolves to the centre of its rectangle, so
the rectangle has to be somewhere clicking does what the name says. Over the whole stack that
centre falls in the metadata strip, where a mouse down reaches no editor and every keystroke after
it is dropped in silence.

`agents.send` is the composer's Send button, which reads Steer with a draft while the agent works
and Stop without one — one control, one name. `agents.tool[N]` is a tool call's 30 px line, `N` the
row's index in the transcript (so it counts every row above it, not only tool rows), and it is
painted only while that row is on screen and is a row of its own: a settled turn folds its calls
into a group, which a scenario opens first by clicking it: `agents.row[N]` is any transcript row,
the whole of it, at the same index — collapsed, a group, fold or delegation row is its one line. The approval names now sit on the dock's buttons, and
the question and plan controls carry no names.

`menu.item[N]` exists only while a menu is open, so a scenario opens one first (by clicking its
trigger or right-clicking its row) and awaits the target before clicking it. Only one menu is open
at a time — opening another closes the first — so the name needs no surface prefix. An item whose
action the surface cannot run is left out of the menu, not greyed, so `N` counts only what is
shown.

`prs.tab[N]` is the PR screen's tab (`0` Mine, `1` Waiting for my review); a click on it switches
tabs as `Tab` does. `prs.filter` is the page header's filter field (`/`; while the filter owns
the keys the field is `filter.input`). `prs.refresh` is the page header's Refresh (`r`); `prs.retry` is the Retry on
the error callout and exists only while a fetch error is shown; `prs.more` is the
`+n more — select a repo to narrow` button, only in `All` scope past the 100-row cap.
`prs.row[N].open` (`Open ⏎`) and `prs.row[N].menu` (the `⋯` trigger) are the row's hover actions:
painted on every visible row but visible only while it is hovered or selected, so a scenario
clicks the row before them. A press on either first puts the cursor on that row; the menu's
items are `menu.item[N]`, and a right click on `prs.row[N]` opens the same menu.

`toasts.toast[N].action` is the toast's `View` button, painted only on a toast that points
somewhere (a background success, a native agent that needs you); a click on it — or on the
toast's line — goes there as the key would and retires the toast. `toasts.toast[N].close` is its
✕, on every toast. `sticky_error.retry` is the whole error in the status bar: a click on it runs
`!` (the Jobs panel, on the failure); `sticky_error.close` is the ✕ beside it, which runs `X` and
clears the slot. `banner.button[N]` and `banner.close` are the §3.12 C banner's `Reconnect now`,
`Open log` and ✕, each dispatching the banner's `r`, `l` and `Esc`; they are painted only while
the link is lost, never on the `reconnected` or restart banner. `first_run.step[N]` are the
first-run page's step cards: `0` and `1` run `N` and `n`, and `2` is drawn dimmed and does
nothing until a repository exists. `first_run.import` is the import card, painted only when
`~/.swarm/state.json` exists (so never in a hermetic run); `first_run.help` and
`first_run.settings` are the footer's `?` and `,`.

`dialog.close` is the close ✕ in every dialog's header; clicking it runs the action `Esc` runs in
that dialog, and so does a click on the scrim outside the card. `sheet.close` is the same ✕ on a
dismissable sheet (the Jobs panel, the card detail). The card detail paints its ✕ under
`card_detail.close` as well; a click on a `card_detail.property[N]` row selects it and runs what
`⏎` runs there, so it opens the same picker, worktree or issue — a locked or read-only row takes
no click.

The Jobs panel's buttons are named by the row they act on, and each one first puts the cursor on
that row: `jobs.row[N].retry` (a retryable failure), `jobs.row[N].log` (Show log, on a failure)
and `jobs.row[N].cancel` (a cancellable live job; it shows only while its row is hovered or
selected, so a scenario selects the row before clicking it). A button whose action cannot work on that job is not
painted, so its name is absent rather than disabled. `jobs.filter[N]` is the filter's segment —
`0` All, `1` Running, `2` Failed, `3` Done. `jobs.clear` is Clear finished (absent with nothing to
clear), `jobs.more` the ⋯ holding Cancel all (absent with nothing cancellable). While a log is
open the header is the log's toolbar: `jobs.log.back`, `jobs.log.follow`, `jobs.log.end`.

Settings names its controls by the row they sit in. `settings.row[N]` is the whole row: a click
puts the cursor on it and, on a text or number row, opens its editor, as `⏎` would. The control
of the row **under the cursor** carries its own name, so a scenario clicks a row first and its
control second: `settings.switch` is its switch, `settings.option[N]` a segment of its choice
(`N` from `0`, left to right), and `settings.dropdown` its dropdown field when the choice has too
many or too long options to sit side by side — the open list's options are then `menu.item[N]`.
Only one of the three is painted, the one the row draws. `settings.copy[N]` is the copy button
beside read-only row `N` (About's fleetd and `FLEET_HOME`). While a search is typed the pane lists
its matches instead, `settings.hit[N]`, and a click on one opens its section with the cursor on
it. `settings.config` and `settings.doctor` are the footer's Open config.json and Run doctor.

`dialog.button[N]` is a button in a dialog's footer, `0`
leftmost, painted by `Dialog::actions`: a dialog still on the legacy footer, whose `Dialog::primary`
renders a label rather than a control, paints none, so a scenario may target `dialog.button[N]`
only in a dialog that has been moved onto the button footer. Create worktree paints `0` Cancel and
`1` Create (`Open` on a duplicate id). Every Confirm paints `0` Cancel and `1` its action — the
primary `y` button, or the red `Y` one where `Y` is required; a click on it dispatches the same
`Accept` / `AcceptStrong` its key does, so the escalation rule holds under the pointer. A Confirm
is an alert with no ✕, so it paints no `dialog.close`; the prune confirm's `Show kept` toggle and
every confirm's `Re-check` are footer and body buttons with no name of their own.
`dialog.checkbox` is create-worktree's "Open after creating" box, and `dialog.segment[N]` its host
choices while they sit side by side (`0` is `local`), absent on a local-only daemon or when the
hosts draw as a dropdown, whose options are `menu.item[N]`.

These dialogs paint `dialog.button[N]`, `0` leftmost; a click dispatches exactly the action its
chip names, so it is the key under the pointer. `0` is always `Cancel`, which runs what `Esc` runs.

| Dialog | `dialog.button[N]` |
| --- | --- |
| Clone repo | `0` Cancel · `1` Clone |
| New / Edit context | `0` Cancel · `1` Create (Save when editing) |
| Assign repo | `0` Cancel · `1` Move |
| Quit | `0` Cancel · `1` Quit |
| Quit and stop the daemon | `0` Cancel · `1` Stop and quit |
| Rename terminal | `0` Cancel · `1` Rename |
| Repository hooks | `0` Cancel · `1` Save |
| New card | `0` Cancel · `1` Create & open · `2` Create |
| Card property | `0` Cancel · `1` Apply |
| Board settings | `0` Cancel · `1` Save |

The buttons on the left of a footer — Edit context's *Delete context*, Quit's *Show jobs* and
*Never warn again*, the Board settings Columns list's *New column*, *Delete* and *Apply preset* —
and the rows' own controls (a hook row's ✕, a column's ↑ / ↓) carry no names of their own.

## 4. Lanes and fixtures

- `headless` sets `ZED_HEADLESS=1`, removes compositor variables, runs all layout/state logic, and
  has no pixels. `shot` fails clearly.
- `virtual` creates a dedicated Hyprland headless output, pins geometry/scale, moves the harness
  window there by its unique title, floats it and holds it at the pinned logical size — a tiling
  session sizes a new window from its own layout rules, so the size the app asked for is a size
  the lane has to insist on — and captures it with `grim`. Window placement selects the classic
  or Lua dispatcher syntax from `hyprctl status`, so both config providers preserve isolation. It
  is the default interactive lane.
- `attach` uses the developer's current compositor and is opt-in for watching a run.

Every lane owns unconditional teardown. Automatic selection may fall back from unavailable
`virtual` to `headless` and reports that choice; an explicit lane does not silently change.

Two lane limits are load-bearing for anyone writing a scenario today, and neither is a property of
the scenario that hits them:

- **The keyboard reaches the focused view in the `headless` lane.** It did not until the app's
  stale-key queue stopped waiting for a frame callback GPUI's headless platform never delivers
  (`shell::root::focus::prepare_key_replay`); before that fix a `key` answered `handled: true`
  while the snapshot never moved, and the lane could not cover `docs/KEYMAP.md` at all. It can
  now, so a keyboard scenario belongs in the headless subset unless it takes a screenshot.
- **The `virtual` lane captures the whole output, not the window.** `grim -o <output>` photographs
  whatever the compositor paints there, and on a session that is locked — or one whose shell
  attaches background and bar layers to every new output — that is not Fleet. The structured
  assertions still pass, because they never look at pixels; only the picture is wrong.

  So the lane asks the compositor first. Hyprland reports `LOCK` in every monitor's
  `solitaryBlockedBy` while a session-lock surface owns the outputs, whoever drew it — a shell
  that paints its own lock screen runs no separate lock process and leaves logind's `LockedHint`
  unset, so neither is a usable signal. A run that starts locked warns once, and every `shot` taken
  while the session is locked fails before anything is photographed, naming the lock. The pixel
  check below is the second line, for whatever paints over an output without being a lock.

  Then the lane checks what it photographed. It keeps one capture of the isolated output taken in
  the moment before the window is moved onto it, and every `shot` must differ from that reference
  by at least a third of the window's own area — Fleet is opaque chrome over most of the output,
  and a capture of a lock surface differs from it by nothing. A shot that does not clear the bar
  fails the line, naming the share it saw and the share it needed, and the rejected picture is
  deleted rather than left in `shots/` for a report to inline as evidence. A run whose reference
  could not be taken says so and skips the check rather than failing.

  What the check cannot do is make the picture right: a session that paints over every output has
  no working `virtual` lane until it stops, and the shot is still the whole output, so the
  session's wallpaper, bar and window-opacity rules are inside every baseline recorded from it.

Fixture presets are `empty` (first run), `one-repo` (one clean repository and worktree), `busy`
(several repositories, worktrees and pull requests, one worktree degraded), `board` (two boards
with cards, plus fake `acli`), and `agents` (native-agent configuration plus scripted transcripts). Each gets a
private `FLEET_HOME`, child-only `HOME`, real local Git repositories, and fake `gh`/`acli`; it
never reads the developer's Fleet home. Every child also starts with the variables an *outer*
fleetd exports — `FLEET_DELEGATION`, `FLEET_DELEGATION_TOKEN`, `FLEET_SESSION`, `FLEET_TERMINAL`,
`FLEET_TERMINAL_ID` and `FLEET_STATUS_PATH` — cleared, so a run launched from a Fleet terminal or
from inside a delegation sees the same environment as a run launched from a bare shell.

A preset is applied by driving a private `fleetd` through `fleet-client`'s own typed operations
before the run's daemon starts, so nothing is written to a Fleet file by hand. Four consequences
follow:

- **Jobs and toasts are not part of any preset.** The daemon's job registry is in memory, so a job
  seeded before the run is gone by the time Fleet connects. `busy`'s jobs, toasts and sticky error
  come from the `job` scenario line, which submits real jobs through the live client.
- **`hotPoolSize` and `hotRefreshIntervalMs` are seeded to 0**, so a background pool cannot put
  unrequested jobs, directories or timing into a run.
- **The `board` preset uses Fleet's local board backend.** The fake `acli` exists to keep a
  Jira-backed board off the network, not to serve the fixture's cards.
- **The `board` preset seeds two boards, and they are tellable apart in a dump.** The context
  board holds five cards across three columns and numbers them `FLT-…`; `acme/api#feature`'s own
  board — asked for with `EnsureWorktreeBoard`, the request `ctrl-s b` sends, so its id, name and
  prefix are the daemon's derivation — holds three disjoint cards in two columns and numbers them
  `FEA-…` after the slug. `lists["board.cards"]` therefore says which of the two a surface is
  drawing, which is what `scenarios/workspace/board-tab.scenario` reads.

A third additive preset, `board-workflow`, seeds what `agents-subagent` seeds — `acme/api#agent`,
`acme/api#other` and the two scripted providers — plus `acme/api#agent`'s own board, asked for the
same way and numbered `AGE-…` after the slug, carrying `fleet-core`'s workflow columns (Backlog,
Todo, Ready, In Progress, In review, Done, Canceled) and `maxLiveRuns` of 1. Four cards start in
Todo, which the preset deliberately leaves human: card 1 blocks card 3, and cards 1 and 2 block
card 4. **No card is seeded into a column with an action**, because a run the seeding daemon
started is gone by the time the scenario's own daemon reads the home — a scenario moves a card in
and watches the run it started itself. The run's child is the scripted provider in its delegation
role (§5), so Codex completes its card and Claude asks a question and parks it.

## 5. Scripted-agent transcripts

The `fleet-harness agent --provider <claude|codex> --transcript <file>` subcommand reads one JSON
document and speaks that provider's native wire protocol. The frozen document shape is:

```json
{"version":1,"steps":[
  {"type":"text","text":"I will inspect it.","pace_ms":10},
  {"type":"tool_call","id":"call-1","name":"read_file","arguments":{"path":"README.md"}},
  {"type":"shell","command":"printf '%s\\n' \"$FLEET_SESSION\"","name":"Bash"},
  {"type":"file_change","path":"src/lib.rs","diff":"@@ ..."},
  {"type":"permission","id":"gate-1","command":"cargo test"},
  {"type":"approval","id":"gate-2","summary":"apply the edit"},
  {"type":"models","models":["model"],"reasoning_efforts":["low","high"]},
  {"type":"error","message":"provider failed"},
  {"type":"exit","code":0}
]}
```

Steps execute in order. `pace_ms` advances between text deltas; zero is immediate. Provider-specific
framing follows the two authoritative research documents indexed by `docs/README.md`.

Three additive pieces the "optional data may be added" rule allows, all of which a document may
omit entirely:

- **`{"type":"end_turn","status":"completed"|"failed"|"interrupted"}`** marks where one turn stops.
  Both protocols have exactly one turn terminal — Claude's `result`, Codex's `turn/completed` — and
  a flat step list has no other way to say so. A transcript that never uses it is one turn, and a
  trailing `exit` with no `end_turn` before it kills the process *inside* the turn, which Fleet
  sees as a dead harness rather than a settled one.
- **Optional top-level `session_id`, `thread_id`, `model` and `context_window`**, plus an optional
  `output` on a `tool_call`, so two scripted threads in one scenario can be told apart and a
  transcript that names none stays byte-identical between runs.
- **`{"type":"shell","command":"<sh -c string>","name":"Bash"}`** runs the command with the
  player's inherited environment, waits for it, and presents an ordinary tool call whose output
  is stdout followed by stderr. `name` is optional and defaults to `Bash`.

**There is no account step, and the scripted provider's account is fixed.** Fleet reads
`account/read` in its Codex handshake (`NATIVE-AGENTS.md` §4.2); the scripted agent answers a
constant signed-in ChatGPT account so a scripted thread looks like a real one. Signing in and out
is deliberately *not* scriptable: a step for it would only be observable through fields the
version-1 snapshot does not carry — the composer's `/` rows and the metadata row are both absent
from it — so a scenario could drive the flow and assert nothing about it. Both surfaces are
covered by `#[gpui::test]`s in `screens/agent_thread/tests/` instead.

An `approval` step must immediately follow the `file_change` it gates: neither provider's approval
frame carries a diff — Claude reads it from the `Edit` input, Codex joins by `itemId` — so the
pairing has to be unambiguous. `models` is configuration rather than a stream event: it is hoisted
at load to answer Claude's `list_models` and Codex's `model/list`, and playing it emits nothing.

The `agents` fixture embeds all three starters: Claude serves the two turns from
`two-turns.json` followed by the failed turn from `error-mid-stream.json`, and Codex serves
`edit-approval.json`.

Two additive presets select delegation callers without changing `agents`: `agents-subagent`
serves `subagent-caller.json` to Claude, and `agents-subagent-other-worktree` serves
`subagent-caller-other-worktree.json`. Both also serve `subagent-caller-blocked.json` to Codex,
so `^s A` reaches the Claude-child blocked path. Both seed `acme/api#agent` and
`acme/api#other`. Each caller writes a brief to a temporary file and invokes the hermetic
`fleet subagent run`; the other-worktree variant passes `--worktree acme/api#other`.

**An unanswered gate is held for twenty seconds** (`agent::GATE_BUDGET`), and then the scripted
child gives up and exits, which settles its turn as *cancelled*. That is the ceiling on every
state a parked child can hold — a `needs you` mark, a run slot a board is counting, a caller
painting the gate — so a scenario that means to read one has to act inside it. The budget is
four times the default await rather than equal to it because a loaded machine can take seconds
to show the run at all; a gate a scenario answers is answered in milliseconds, so the number
only ever bounds the case where nobody answers.

The launcher chooses by role. With `FLEET_DELEGATION` unset it plays the preset transcript —
§4 clears an inherited one, so only this run's own daemon can set it. With
the variable set, Codex plays `subagent-child.json`, which writes a temporary report and runs
`fleet subagent complete`; Claude plays `subagent-child-blocked.json`, whose available permission
gate stands in for the blocked child's clarification question. The fixture installs `fleet`
beside the vendor shims with the run's private `FLEET_HOME` and resolved `FLEET_APP` baked in.

## 6. Run directory and baselines

The runner prints its directory first. The default is
`$TMPDIR/fleet-harness/<UTC-timestamp>-<scenario-stem>[-2|-3|…]/` (`/tmp` when `TMPDIR` is
unset): the first claimant gets the
unsuffixed name, and a concurrent or same-second collision atomically claims the next suffix. A
long scenario stem is shortened only enough to leave room for the run's Unix sockets. Suite roots
follow the same rule.

```text
run.jsonl          timestamped request/response journal, one JSON object per exchange
scenario.txt       exact scenario source
report.md          the run's digest (§8)
app.log            Fleet stdout/stderr
fleetd.log         private daemon stdout/stderr
shots/             NNN-name.png, failure-NNN.png and NNN-name-diff.png
dumps/             NNN-name.json and failure-NNN.json
home/              private FLEET_HOME — reclaimed after a clean run unless `--keep` is given
child-home/        the child-only HOME, with its XDG_{CONFIG,DATA,STATE,CACHE}_HOME below it
bin/               the fake `gh`/`acli`/agent executables, first on the run's PATH
fixture/           the preset's bare git origins, fake-tool data and seeding daemon log
```

`home/` is the only thing a run reclaims, and only on a pass without `--keep`; everything else
survives, because the run directory is the evidence. `child-home/`, `bin/` and `fixture/` exist so
that "hermetic" is inspectable after the fact: they are what the run's processes actually saw.

A directory run adds one level. The suite directory holds its own `report.md` and one
subdirectory per scenario, numbered in execution order and named after the scenario's file stem:

```text
$TMPDIR/fleet-harness/<UTC-timestamp>-<directory-name>/
  report.md            the suite digest, linking into each run below
  001-hub-help/        a complete run directory, exactly as above
  002-pointer-basics/
```

Virtual-lane shots compare against
`scenarios/baselines/virtual/<scenario>/<NNN>-<name>.png` when one exists, where `<scenario>` is
the scenario's corpus-relative path without its extension — `scenarios/hub/help.scenario` keys
`scenarios/baselines/virtual/hub/help/`, so `hub/help.scenario` and `agents/help.scenario` cannot
collide.

Comparison uses two budgets at once: a per-channel difference a pixel may carry before it counts
as changed, and a share of the image that may count as changed before the shot fails. The defaults
are **8 per channel** and **0.2% of pixels** — antialiasing of the same glyph by two GPU drivers
lands well inside both, a changed colour token does not. A failed comparison writes
`shots/<NNN>-<name>-diff.png` beside the run shot. A capture whose *size* differs from its
baseline is an error telling the reader to re-record, never a pixel count.

`--update-baselines` explicitly replaces baselines only in the `virtual` lane. In every other
lane an update request reports `not-recorded` with the lane name and writes no baseline.
Structured assertions remain the oracle; pixels are regression evidence.

## 7. The corpus

`scenarios/` is the checked-in corpus. A path given to `fleet-harness run` may be one file or a
directory; a directory is walked recursively and its scenarios execute in lexical order, so the
directory names decide the order and the numbering of the suite's run directories.

```text
scenarios/
  <surface>/<what-it-checks>.scenario    one scenario, named after the behaviour it pins
  baselines/virtual/<scenario>/          the golden images for that scenario (§6)
  baselines/README.md                    the baseline policy, at the files it governs
```

A file counts as a scenario when its extension is `.scenario` or `.txt`, which is what lets
`baselines/` and a `README.md` live inside the corpus directory without being executed. Prefer
`.scenario`: the extension is what tells a reader which files run.

Surface directories group scenarios by the part of Fleet they exercise — `hub/`, `workspace/`,
`agents/`, `daemon/`, `board/` — and a scenario's corpus-relative path without its extension is
also its baseline key, so `hub/help.scenario` and `agents/help.scenario` never collide.
The agent corpus includes a headless two-turn wheel regression and a structured Codex
file-approval assertion; the latter reads `agents.threads[0].decision.paths` and `has_diff` to
prove the named item supplied the rendered diff rather than merely observing that a gate opened.

Three make targets run it, and none of them holds a list of scenario names — the corpus grows
without a `Makefile` change:

```sh
make harness                                            # the whole corpus, virtual lane, with pixels
make harness-headless                                   # the pixel-free scenarios, no compositor
make harness-one SCENARIO=scenarios/daemon-down.scenario # one scenario (LANE=… to change lane)
make harness-prune                                      # delete run directories older than SWEEP_DAYS
```

`HARNESS_DIR` points any of them at a different corpus directory and `HARNESS_ARGS` forwards
runner flags, so `make harness HARNESS_ARGS=--update-baselines` re-records.

Selection for the display-less lanes is textual and mechanical: a scenario is excluded when it
names a directive the headless lane cannot answer — `shot`, which has no surface to photograph,
and `clipboard`, which cannot read its own write back (§1). `make harness-headless` runs what is
left, one runner invocation per file, and says so plainly when nothing is left. A slice of the
same set rides `make test` through `crates/fleet-app/tests/harness_headless.rs`, which applies
that same rule and no other, so the two selectors cannot drift apart; it fails rather than
passing silently when the rule selects nothing, and holds itself to a 60-second budget.

A scenario meant to run in both lanes therefore drives the app with the pointer, asserts on the
snapshot, and takes no screenshot.

## 8. The report

Every run writes `report.md` beside its journal, and a directory run writes one more for the
suite. The journal is the complete record; the report is its digest, and it is assembled only
from what is already on disk, so it cannot describe something `run.jsonl` does not show.
Every `command` journal entry carries its scenario line in `data.line`, and report alignment
requires an exact match. If a journal line is lost or lacks the expected number, the remaining
alignment is marked untrustworthy and those rows are left unenriched rather than being paired
with later exchanges.

A scenario report leads with the verdict and the run's identity — scenario, lane, fixture, line
counts, start time, duration, run directory — then:

| Section | What it holds |
| --- | --- |
| **Failure** | Present only on a red run, first: the failing line verbatim, the error, the clause that did not hold with the value it actually saw, the busy `idle` counters, the state at the failure, and links to `failure-NNN.png` / `failure-NNN.json`. |
| **Lines** | One row per executed line: line number, the source line, ok/failed, duration, and a one-phrase "what happened" — which keys were handled, which predicate held, which baseline compared. |
| **Screenshots** | Every `shot`, inlined, with its baseline verdict. A differing shot shows its diff once, labelled **Diff**, beside the screenshot; diff files are not also listed as screenshots. |
| **Assertions** | Every `await` and `assert`, with the predicate and what satisfied it. |
| **Dumps** | Every `dump`, linked, with its one-line summary. |

A suite report names the lane, how many scenarios were planned and how many ran, lists the
failures first with a link into each failing scenario's own report and its failure evidence, then
every scenario with its verdict and duration, then the screenshots grouped by scenario.
If the suite cannot list a failing scenario's `shots/` directory, its failure block says that the
screenshots could not be listed and includes the filesystem error.

After Fleet answers `quit`, the runner waits up to five seconds for the process to exit. A non-zero
or signal-terminated exit, or an exit that misses that deadline, fails the line and therefore the
run.
On a display lane the app first blurs and paints one cleanup frame, then closes the driven window,
so GPUI releases the focused platform input handler and its entity handles before leak detection
runs. Headless closes directly because it installs no platform handler and has no frame source;
in neither lane is `quit` a direct process-level escape hatch.

Stdout is a contract of its own: a failure must be diagnosable without opening a file. The runner
prints the run directory first, the lane second, and on a failure the line, the error, the value
it saw and the paths to the evidence. The report is where a reader goes next, not instead.

## 9. Adding a scenario

1. **Pick the surface directory and the behaviour.** One scenario pins one behaviour, and its
   file name says which: `hub/filter-narrows-worktrees.scenario`, not `hub/test2.scenario`.
2. **Name the fixture on the first line.** `fixture: busy` or whichever preset already has the
   state you need (§4). Do not build state with scenario lines that a preset could seed.
3. **Synchronise with `await`, never with `wait`.** `await idle` after a step that starts work,
   `await <predicate>` for a specific state. A `wait` line is a bug waiting for a slower machine;
   it exists for the case where nothing observable is available yet.
4. **Assert on the snapshot.** The structured dump is the oracle. Every predicate path must be one
   a real dump contains — run the scenario once with a `dump` line and read `dumps/NNN-*.json`
   rather than guessing a path.
5. **Take at most one screenshot, at the moment that matters.** Pixels are evidence, not the
   oracle, and a scenario whose only check is a screenshot is not allowed. A corpus where every
   scenario shoots three frames is a corpus nobody reviews.
6. **End with `quit`.** It is the orderly shutdown; teardown is unconditional either way, but a
   scenario that ends on `quit` says where it meant to stop.
7. **Run it in both lanes.** `make harness-one SCENARIO=<file>` for pixels, then
   `make harness-one SCENARIO=<file> LANE=headless` for the lane a display-less machine runs. If
   it cannot pass headless, say why in a `#` comment at the top of the file — and remember that a
   scenario with a `shot` line is excluded from the headless selection by construction (§7).
8. **Record a baseline only when you have looked at the image.** `--update-baselines` writes what
   the run captured; a baseline nobody opened is a committed screenshot of whatever was on the
   output.

A scenario that cannot be written without a new command, a new target name or a new fixture is not
a scenario problem: this document is frozen, and the change belongs to the stage that owns the
surface (§10).

## 10. Ownership table

Ownership is exclusive during this initiative. A stage may edit only its rows; cross-row needs are
integration notes to the owner.

| Stage | Owned paths |
| --- | --- |
| `app-socket` | `crates/fleet-drive/{Cargo.toml,src/lib.rs,src/protocol.rs,src/server.rs,src/legacy.rs,src/legacy/}`, `crates/fleet-lazygit/{Cargo.toml,src/drive.rs,src/drive/}`, `crates/fleet-app/{Cargo.toml,src/drive.rs,src/drive/socket.rs}`, and app startup socket wiring |
| `snapshot` | `crates/fleet-app/src/state/harness.rs`, its `state.rs` mod line, and snapshot registration/population seams |
| `runner-core` | root `Cargo.toml`/generated lockfile wiring and `crates/fleet-harness/{Cargo.toml,src/main.rs,src/lib.rs,src/scenario.rs,src/client.rs,src/rundir.rs}` |
| `hermetic-env` | `crates/fleet-harness/src/env.rs`, harness process fixtures and fake executable setup |
| `lanes-capture` | `crates/fleet-harness/src/lane.rs`, `capture.rs` |
| `predicate` | `crates/fleet-drive/src/predicate.rs` |
| `ui-kit-target` | harness target recorder/wrapper under `crates/fleet-ui-kit/src/`, and the target names painted inside kit components that take typed items rather than elements |
| `mockpeer` | `crates/fleet-daemon/src/agents/harness/mockpeer.rs`, its module registration, and `crates/fleet-daemon/Cargo.toml` feature wiring |
| `app-commands` | `crates/fleet-app/src/drive/commands.rs` and its owned notification clock seams |
| `mouse-input` | `crates/fleet-drive/src/input.rs` and `crates/fleet-app/src/drive/mouse.rs` |
| `target-naming` | target-only annotations in `crates/fleet-app/src/{views,screens,dialogs,shell}/` surfaces |
| `report` | `crates/fleet-harness/src/report.rs` |
| `fixtures` | `crates/fleet-harness/src/fixture/` |
| `scripted-agent` | `crates/fleet-harness/src/agent/` and `crates/fleet-harness/transcripts/` starter assets |
| `fault` | `crates/fleet-harness/src/fault.rs` |
| `baseline` | `crates/fleet-harness/src/baseline.rs`, `scenarios/baselines/` |
| `corpus-*` | corresponding `scenarios/{hub,workspace,agents,daemon,board}/` surface directory |
| `makefile` | `Makefile` and the owned headless wrapper under `crates/fleet-app/tests/` |
| `docs` | `docs/TESTING-HARNESS.md`, its `docs/README.md` row, assigned harness doc edits, and phase tracker deviation notes |

The contracts stage alone owns initial crate manifests, root workspace wiring, the protocol types,
legacy-driver extraction, initial module registration, and the initial skeletons subsequently
handed to the stage named above.

## 11. Known gaps

What this harness does *not* do today. Each entry is a verified limit, not a suspicion; the
frozen surface above describes them as they should be, and this section says where reality is
still short of it, so no other document has to claim a capability that does not work.

- **The `virtual` lane needs an unlocked session, and one that does not paint over new
  outputs.** `grim -o` photographs the output, so a session lock surface or a shell that attaches
  background and bar layers to every created output is what lands in the file (§4). On the
  Hyprland session this was last exercised on (Omarchy, whose quickshell draws the lock screen
  itself after an idle timeout), both are true while the screen is locked: every `shot` is
  refused — now up front by the session-lock check, *"the compositor session is locked …"*, and
  before that check existed by the empty-output guard with *"only 0.0% differs … has to change at
  least 33.3%"* — and the structured half of the same scenario passes. The lock check exists
  because the pixel guard alone is not enough: when the reference is taken before the lock
  surface reaches the freshly created output, two blank captures agree, and a photograph of the
  lock screen then clears the bar and was filed as a passing `shot`. The guard is correct and must not be relaxed — it is what keeps a corpus of
  lock-screen photographs out of `shots/`. It compares the window's own rectangle rather than the
  whole output, because a locked session animates: an output-wide comparison answers "did
  anything move?" instead of "is Fleet here?", and let 7 of 41 lock screens through as evidence
  before it was narrowed. The reference itself is taken twice and required to agree, because a
  created output is not painted the instant it exists and a blank reference turns the guard off
  the same way. The workaround is an unlocked session or a throwaway nested compositor;
  the remaining fix is to *capture* the window's region as well as compare it, which would also
  make a baseline window-sized instead of output-sized.
- **No baseline has ever been recorded.** `scenarios/baselines/virtual/` holds a `README.md` and
  nothing else, because §9.8 forbids recording a baseline nobody has looked at and nobody can
  look at one until the point above is resolved. Every `shot` therefore reports
  `baseline: none yet …` in its report, which is a verdict rather than silence (§8).
- **`clipboard set` and `clipboard get` have no working lane.** Stated in §1 and unchanged: they
  are part of the frozen grammar and no scenario may use them.
- **`advance` does not move `BackgroundExecutor` timers.** Stated in §1 and unchanged.
- **A suite writes about five megabytes per scenario into `$TMPDIR/fleet-harness` (`/tmp` when
  `TMPDIR` is unset) and never prunes it.** The run directory is the evidence (§6), so nothing
  deletes it on its own; `make
  harness-prune` is the broom, and it is opt-in.
- **Six source files are past the ~900-line rule** `rust-workspace-architecture` sets:
  `fleet-harness/src/{report.rs, baseline.rs, scenario.rs, agent/codex.rs}`,
  and `fleet-drive/src/{input.rs, predicate.rs}`.
  Each is one coherent subject rather than an accumulation, so splitting them is a deliberate
  refactor, not a drive-by.
- **`dialog.message` is carried for the Confirm dialog alone, and `dialog.fields` is empty for the
  dialogs whose tab cycle is not all text.** The dialog host entity, not `AppState`, owns both.
  `fields` is carried for the dialogs whose whole cycle is live editors — new-card, new/edit
  context, rename-terminal, clone-repo, edit-hooks and card-property, whose one editor is the
  query it filters and types values into — where `fields[N]` is exactly the field
  `targets["dialog.field[N]"]` paints; the command about to answer a `dump`, an `assert` or an
  `await` poll reads them across in its update path the same way it brings the target table
  across. A dialog with a non-editor in its cycle (create-worktree's base list and host cycler)
  reports `[]` rather than a partial numbering that would not line up with its targets. `message` is the one sentence an open Confirm asks — the consequence line §3.8.3
  has the user accept, read from the same draft the card is drawn from, including the board `X`
  and transcript `x` cards that draw their own — and stays `null` over every other dialog, whose
  body is elements rather than a sentence and would have to be invented to be named one. Board
  settings is the one exception in the other direction: it reports a leading non-editor field
  named `section`, whose value is the open rail section's own title (`General`, `Backend`,
  `Columns`), because the section is draft state no projection can otherwise reach, and after it
  the one row-scoped editor a row owns while it is open — named after that row in lower case
  (`name`, `prefix`, `effort`, `on enter`, or a backend row's own name), which is the only place a
  value *typed* into this dialog can be read back, since every other row is a cycler whose value
  `settings.columns` or `AppState` already carries. A row the cursor is merely on in the Columns
  pane owns no editor until `⏎` opens one, and a locked automation row never does, so the second
  field is absent in both cases. Board settings paints no `dialog.field[N]` target at all, so
  neither field can put the field↔target numbering out of step. Settings follows the same rule:
  a leading `section` field whose value is the shown section's title (`General`, `Agents`,
  `Sleep`, …), then `row`, the row under the cursor as `<label> = <value>` with the value the
  draft holds (`Sleep on switch = off`, `Default agent = Codex`; a switch reads `on` or `off`), so
  a click on a control can be checked without a picture, then `search`, the header's search field, whose value is the typed query and which
  is `focused` while the field owns the keyboard (`Dialog > SettingsSearch`). It too paints no
  `dialog.field[N]` target; the search field is `settings.search`. Typing into a field does not
  itself notify `AppState`, so a scenario reads a field with `assert` or `dump`, which project on
  demand, and not with `await`.
- **A headless `await` does not repaint, so `window.frame` freezes for the duration of the wait.**
  The await loop reprojects update-path state but does not draw another headless frame. Use
  `assert` or `dump`, which paint before projecting, when current frame geometry matters.
