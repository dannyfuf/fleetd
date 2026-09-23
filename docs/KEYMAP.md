# Fleet keymap

Principles: nvim-inspired, modal, discoverable. Every binding here is authoritative for the
app; `fleet-app` registers exactly these via gpui key contexts. Lowercase = safe action,
uppercase = stronger variant. `ctrl-c` never quits the app (it belongs to terminals); quitting
is `ctrl-q` (with a confirm only if a job is running and the user asked to be warned).
`Esc` never quits. `q` never quits: it closes the topmost overlay, and where nothing is open it
is unbound. Over a terminal grid no bare key is ever an app affordance. Workspace commands use
the `ctrl-s` prefix except for the standard macOS clipboard shortcuts. The floating Agent popup
also binds direct `ctrl-q` to hide itself; its other commands use `ctrl-s`.

This file is the single source of truth for keys. `docs/UX-SPEC.md` describes screens and cites
this file; where the two disagree, this file wins.

Every action here also has a visible control, and every control shows its key as a chip resolved
from this table at runtime, never typed (ADR 0023). Controls are not focusable, so no binding
below gains or loses a meaning because a button exists: the key stays the keyboard path.

## Action catalogue — where labels live

This file says which key runs which action; `crates/fleet-app/src/action_catalogue.rs` says what
a person calls that action. Every action bound in `keymap::table()` has one hand-written entry
there (`action_catalogue/entries.rs`):

- **label** — a sentence-case verb phrase a newcomer understands: "Go to tab 1–9", "Allow the
  agent's request once", "Delete the worktree safely". Never generated from the action's type
  name; `humanize` survives only as a debugging aid, and a test fails if anything else calls it.
- **short label** — the same, where space is tight (the `^s` command menu, a compact button).
- **description** — one line saying what happens, including safety ("Asks first", "keeps running
  in fleetd").
- **place** — where the entry is filed: `Everywhere`, `Hub`, `Worktrees`, `Pull requests`,
  `Board`, `Card`, `Terminal`, `Agent thread`, `Agent window`, `Scrolling`, `Jobs`, `Dialogs`,
  `Editing text` or `fleetd`. Every key context above maps to one place.
- **group** — the heading Help and the `^s` menu list it under (Tabs, Session, Terminal, Agents,
  Panels, Navigation, Worktree, Card, …).
- **destructive** and **palette** — whether it always goes through a confirm, and whether the
  command palette offers it.
- **rank** — for the few entries that answer "what is this surface for", where they rank in
  Help's *Here in …* list, lower first. Help shows the six lowest ranks whose keys reach the
  surface it was opened over, so one number serves every surface an entry works on.

A numbered range (`SelectTab1`–`9`, `SelectContext1`–`9`, `Choose1`–`5`) is one entry whose keys
are shown as `1`–`9`. Help, the palette, the `^s` menu, buttons and tooltips read their words from
the catalogue — `action_catalogue::info(action)` for one action, `for_place(place)` for what works
somewhere — so a binding added below without an entry fails the tests rather than shipping
unlabelled. Change a row here and its entry there in the same commit.

## Modes and key contexts

| Mode | gpui key context | Entered by | Left by |
| --- | --- | --- | --- |
| Normal | `Hub` / `Hub > Repos` / `Hub > Worktrees` / `Hub > Prs` / `Hub > Board` | app start, `ctrl-s s` from a terminal, `Esc` from dialogs | opening a session |
| Terminal | `Workspace > Terminal` | opening a worktree session, `Enter` on a session tab | `ctrl-s` (prefix) |
| Native | `Workspace > Native`, plus `Workspace > Native > Board` on the board tab | selecting a tab whose configured command is a `fleet://` surface (the default third tab, `lg`, or the `board` tab of `ctrl-s b`) | `ctrl-s` (prefix), or selecting a PTY tab |
| Prefix | `Workspace > Prefix` (one-shot) | `ctrl-s` inside Terminal or Native | any key (consumed) or `Esc` |
| Scroll | `Workspace > Scroll` | `ctrl-s [` | `Esc`, `q`, `i` |
| Agent thread | `Agent > AgentIdle` / `Agent > AgentWorking` / `Agent > AgentDecision > *` / `Agent > AgentNativeScroll` (`> AgentRow` under it) | `ctrl-s a`/`ctrl-s A` in Workspace, selecting a native agent tab | selecting another tab, `ctrl-s x` |
| Agent terminal | `Agent > Terminal` | `a`/`A` in Hub, `ctrl-s F` in Workspace | `ctrl-s` (agent prefix), `ctrl-q` (hide) |
| Agent prefix | `Agent > Prefix` (one-shot) | `ctrl-s` inside the popup | any key (consumed) or `Esc` |
| Agent scroll | `Agent > Scroll` | `ctrl-s [` inside the popup | `Esc`, `q`, `i` |
| Filter | `Filter` | `/` in a list | `Esc` (first keeps filter, second clears), `Enter` |
| Palette | `Palette` | `:`, ⌘K / `ctrl-k`, `ctrl-s k` in the Workspace, the title bar's command field | `Esc`, `Enter`, a click on a row or outside the card |
| Dialog | `Dialog > <name>` while browsing; `Dialog > <name>Editing` where the focused field owns typing | action | `Esc`, `Enter` |
| Text input | `FleetTextInput` (`mode = single_line` \| `multiline`) | focusing a live `TextInput` | its container moves focus or closes |
| Menu | `FleetMenu`, under the context that opened it | clicking a menu trigger, a dropdown, or right-clicking a row | `Esc`, `Enter`, clicking an item or anywhere outside |
| Jobs | `Jobs` / `Jobs > Log` (overlay) | `J` anywhere in Normal, `ctrl-s J` in a terminal | `Esc`, `J`, `q` |
| Daemon | `Daemon > Down` / `Daemon > Banner` / `Daemon > Doctor` | fleetd will not start (§3.12 B), fleetd died while attached (§3.12 C), doctor runs | daemon comes back, `Esc` (banner/doctor), `ctrl-q` |
| FirstRun | `FirstRun` | no contexts and no repos exist | any key that creates or imports something |

The board **pane** is the one Native tab that names itself. A `fleet://board` tab appends a word
*under* `Workspace > Native`, so the whole Hub board table is bound over it (see *Board and card
detail*) while `ctrl-s` keeps working, because the word its parent binds the prefix on is still
on the chain. `lg` adds no such word: the embedded pane takes every key that is not `ctrl-s` and
publishes its own contexts inside itself.

The agent **thread** is not a shadowing surface: it is the Workspace's selected tab, so it
*replaces* the `Workspace > …` context rather than covering it. Nothing is therefore inherited
from the Workspace prefix table below: every row that works inside an agent tab is repeated
explicitly under *Native agent thread*, which is the whole session-level table except the four
PTY-only rows (`^s r`, `^s ,`, `^s ]`, `^s ^s`). A second key with no row there is **swallowed**
— it never reaches the composer — and toasts `^s <key> is not bound here`. The context stack is ordered: the Agent popup shadows
`Hub` and `Workspace`; Help and the two quit confirms may shadow the Agent popup. Palette and Settings are unavailable while the popup
owns focus. `Daemon > Down` and `FirstRun` are full-window and shadow everything except `ctrl-q`.
`Daemon > Doctor` is full-window in the same sense — it replaces the whole context chain, so a key
that has no Doctor binding is dead while the report is up — but only while no overlay is open: with
a dialog, the palette or the Jobs panel up, that overlay keeps its own chain and the report waits
behind it (`shell/root/focus.rs`, `focus_owner`).

## Global (Normal mode, all Hub screens)

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | move cursor |
| `gg` / `G` | first / last row |
| `ctrl-d` / `ctrl-u` | half page down / up |
| `h` / `l`, `←` / `→`, `S-Tab` / `Tab` | focus previous / next pane (**repos ⇄ list only** — the detail panel is never in the cycle) [A1] |
| `gr` / `gw` / `gp` / `gj` / `ga` | go to Repos pane / Worktrees list / PR screen / Jobs panel / the `All` pseudo-repo [A7] |
| `1`–`9`, `gt` / `gT` | switch to nth / next / previous context |
| `p` | toggle Worktrees ⇄ Pull requests screen |
| `/` | filter current list |
| `:`, ⌘K / `ctrl-k` | command palette |
| `,` | settings |
| `?` | help |
| `J` | jobs panel |
| `!` | focus the sticky error slot: the last failed job, offering `R retry` [A18] |
| `X` | dismiss the sticky error slot (its ✕); the failed jobs stay in the Jobs panel |
| `i` | toggle the detail panel (never focusable; it mirrors the cursor row) |
| `H` | collapse / expand the Hub sidebar (232 ↔ 44 px icon column; also its foot button) [A22] |
| `a` / `A` | open the floating Claude / Codex agent popup [A21] |
| `r` | refresh (status, PRs, discovery) — runs as a job, never blocks |
| `U` | update Fleet (job) |
| `N` / `E` / `D` | new context / edit active context / delete active context (confirm) [A15] |
| `y` | copy path (worktree) or URL (PR) |
| `b` | open PR / repo in browser |
| `Esc` | clear filter if any, else close the topmost overlay, else no-op — **never quits** [A13] |
| `q` | close the topmost overlay; on the PR screen with no overlay open it returns to Worktrees; in the Hub lists with nothing open it is **unbound** [A14] |
| `ctrl-q` | quit app (daemon keeps running) |
| `ctrl-shift-q` | quit app and stop daemon (confirm; lists running jobs/sessions) |

`E` opens the Edit-context dialog, from which `ctrl-shift-d` deletes the context. `D` keeps its
meaning and routes to the same expanded `Y` confirm. The dialog's key is the **shift** variant
because its two fields are live editors: plain `ctrl-d` is `FleetTextInput`'s delete-forward and
would never reach the dialog.

## Hub › Repos pane

| Key | Action |
| --- | --- |
| `Enter`, `o`, `l` | select repo → focus worktrees |
| `n` | clone repo (dialog with fuzzy GitHub search) |
| `d` | delete repo (confirm, cascades) |
| `x` | dismiss a **failed clone** row — only on a clone-failed row, unbound on every other row |
| `e` | edit the selected repo's `prepare` / `postCreate` hooks [A16] |
| `m` | move repo to another context (assign dialog) |
| `i` | toggle detail panel |
| `Enter` on a clone-failed row | open the Jobs panel focused on that clone job |

**Arbitration.** UX-SPEC §3.2 wanted `d` to both *delete a repo* and *dismiss a failed clone*
depending on row state — same key, same pane, same mode, different blast radius. Dismissal is
`x`; `d` is always "delete repo, cascades".

## Hub › Worktrees pane

| Key | Action |
| --- | --- |
| `Enter`, `o` | open session (sleeps previous session) → Workspace |
| `O` | open session keeping previous awake |
| `n` | create worktree (dialog) |
| `d` | delete worktree (confirm shows dirty/unique-commit/session facts from inspect) |
| `u` | undo the last delete while its `trash/<epochms>-<slug>` entry still exists [A6] |
| `x` | prune eligible worktrees of selected repo (dry-run preview → confirm exact reviewed DELETE rows) |
| `s` | sleep session |
| `K` | kill session (confirm) |
| `I` | inspect (refresh safety facts as a job) |
| `i` | toggle detail panel |
| `y` / `Y` | copy the worktree path / copy the **branch name** [A17] |

## Hub › Pull requests screen

| Key | Action |
| --- | --- |
| `Tab` / `S-Tab`, `l` / `h` | Mine ⇄ Review tabs |
| `Enter`, `o` / `O` | open or create the PR worktree (sleeping / keeping previous) |
| `c` | create the PR worktree **without** opening it [A9] |
| `I` | inspect the local worktree matching the selected PR [A20] |
| `b` / `y` | open in browser / copy URL |
| `i` | toggle detail panel |
| `r` | force refresh both tabs |
| `p`, `q` | back to worktrees |

## Workspace (Terminal mode)

All keys go to the PTY except these direct terminal affordances:

| Key | Action |
| --- | --- |
| `cmd-c` | copy the current selection; with no selection, forward the key to the PTY |
| `cmd-v` | paste clipboard through the daemon's bracketed-paste-aware path |
| `ctrl-s` | enter Prefix for one key |

Plain left-drag selects cells in reading order, double-click selects a word, and triple-click
selects a line. Non-empty text is copied on mouse-up, including inside alternate-screen programs
and while terminal mouse reporting is active. A click without a drag clears the selection. Typing
or pasting clears mouse selection; ordinary terminal repaints preserve it in absolute scrollback
coordinates. Switching between primary and alternate screen, changing the active terminal, or
resizing the grid columns clears it because those events change the coordinate space.
Trailing padding is stripped from hard rows and wide cells are copied once. Soft-wrapped rows retain
their cells and join directly to the following visual row; hard row boundaries copy as newlines.
While a selection exists, Fleet caches only its visible rows, up to 5,000 rows, and skips repeated
renders of the same frame and viewport. This lets selections that move partly or fully outside the
mirror still copy. If a required row is no longer cached, Fleet clears the selection and reports
“selection scrolled away” without changing the clipboard. Fleet clears selection coordinates when
the daemon's `historyEpoch` advances after history shrinks, its tracked oldest row is discarded,
a column change reflows it, or output arrives at the nominal history bound; viewport scrolling
alone preserves them.
`ctrl-c` remains the PTY interrupt and `ctrl-v` remains available to shells and applications
(quoted insert / block selection); neither is a Fleet clipboard binding.

`ctrl-s` then accepts one of:

| After `ctrl-s` | Action |
| --- | --- |
| `ctrl-s` | send a literal `ctrl-s` to the terminal |
| `s` | go to Hub (session keeps running) |
| `S` | sleep this session **and** return to the Hub [A5] |
| `1`–`9` | switch to nth terminal tab |
| `h` / `l`, `p` / `n` | previous / next terminal tab |
| `Tab` | last terminal tab (MRU within this session) [A2] |
| `w` | last session (MRU alternate, vim `ctrl-^`) [A3] |
| `W` | session switcher: the palette pre-filtered to running sessions, most recent first [A4] |
| `u` | select the caller of the current child thread, attaching it first if needed; on a card run, the worktree's board tab with that card selected |
| `d` | agent picker: the palette seeded with `!`, its agent threads |
| `k` | command palette (the Workspace's palette key: `ctrl-k` belongs to the shell) |
| `c` | new terminal tab (shell in worktree path) |
| `b` | this worktree's board tab: created the first time, selected every time; an agent session is told `boards belong to worktrees` |
| `x` | close current terminal (confirm if a keep-alive process is running) |
| `r` | restart the exited command in this terminal [A10] |
| `y` | copy the worktree path of the current session [A11] |
| `,` | rename current terminal |
| `[` | Scroll mode |
| `]` | paste clipboard (bracketed when the app requests it) |
| `a` / `A` | new native Claude / Codex agent thread in this worktree |
| `F` | the terminal fallback: the floating agent PTY popup (§10) |
| `z` | zoom: hide the terminal tab strip; watch pane stays visible (toggle) |
| `v` | hide/show the cooperative/discovered subagent watch pane; no watches → `no subagent watches` |
| `V` | dismiss the selected exited watch; if running, hide pane and show `watch still running; pane hidden` |
| `N` | next watch in the visible session's start order, wrapping; show the pane if hidden |
| `P` | previous watch in the visible session's start order, wrapping; show the pane if hidden |
| `!` | focus the sticky error slot [A18] |
| `J` | jobs panel |
| `?` | help overlay listing this table |
| `Esc` | cancel prefix |

**The ⌃S command menu.** Holding the prefix for `motion.prefix_hint_delay` (400 ms) without a
second key shows *Fleet commands*: every row of this table the live chain reaches, grouped by
the catalogue's headings (Tabs, Session, Terminal, Agents, Panels) and labelled with each entry's
short label. A row shows only its second key, because the prefix is already held; `ctrl-s` itself
and `Esc` are the header (the note that pressing the prefix again sends it, and a Close button).
Clicking a row leaves Prefix and runs the row's action, exactly as the key would; clicking Close
is `Esc`. The menu takes no focus and changes nothing about the keys: a second key typed before
the delay skips it, every row above keeps working, and an unbound key still leaves Prefix and
does nothing. The agent popup (`Agent > Prefix`) and a native agent tab (`ctrl-s <key>` chords)
show the same menu with their own rows; a row whose key already works without the prefix
(`cmd-c`, `ctrl-q`) is left off.

Watch navigation uses uppercase `N`/`P` because lowercase `n`/`p` already move between
terminal tabs; uppercase acts on the watch pane, mirroring `v`/`V`. With no watches,
both keys toast `no subagent watches`. With one watch, selection is unchanged and no
toast appears; a hidden pane is still shown. Discovered watches use the same keys and
carry a quiet `◦ ` label marker. Keyboard focus stays in the terminal.

**Caution [A5].** `ctrl-s s` (go to Hub, benign) and `ctrl-s S` (sleep this session, then Hub —
kills nothing but stops the terminals) are one shift apart. This is the riskiest adjacency in
the map; it is allowed only because it follows the uppercase-is-stronger rule and because sleep
is reversible.

**No bare Workspace commands over a terminal.** `!` is `ctrl-s !` here, never bare. Affordances
drawn over the grid are written `^s r`, `^s l`, `^s ⏎`; `cmd-c` / `cmd-v` are clipboard actions,
not modal Workspace commands.

## Agent popup

The popup uses `Agent > Terminal`, `Agent > Prefix`, and `Agent > Scroll`. In Terminal mode every
unlisted key goes to the agent PTY. Mouse selection, wheel routing, and Scroll mode match a
Workspace terminal; direct viewport keystrokes remain PTY input unless Scroll mode is entered.
While the daemon is still ensuring the popup session, the same tracked card shows `attaching…`:
`ctrl-s`, `ctrl-q`, `ctrl-s q`, and the `ctrl-s a` / `ctrl-s A` switch-or-hide actions remain live.
The header's Claude/Codex switch, Restart and Hide buttons dispatch these same actions and show
their keys as chips, and a press on the scrim outside the card is `ctrl-q` (UX-SPEC §3.6.1).

| Key | Action |
| --- | --- |
| `ctrl-s` | enter the popup's one-shot Prefix |
| `cmd-c` | copy selection; with no selection, forward the key to the PTY |
| `cmd-v` | paste through the bracketed-paste-aware path |
| `ctrl-q` | hide the popup; do **not** quit Fleet or stop the session |
| `ctrl-shift-q` | unchanged global quit-and-stop-daemon flow |

After `ctrl-s`:

| Key | Action |
| --- | --- |
| `ctrl-s` | send a literal `ctrl-s` to the agent PTY |
| `q` | hide the popup |
| `a` | hide when Claude is visible; otherwise switch to Claude |
| `A` | hide when Codex is visible; otherwise switch to Codex |
| `[` | enter Agent Scroll mode |
| `]` | paste clipboard |
| `r` | restart the exited agent command [A10] |
| `?` | open Help above the popup |
| `Esc` | cancel Prefix |

Agent Scroll mode uses the same `j`/`k`, half/page, `gg`/`G`, `v`/`y`, search-reservation, and
exit keys as Workspace Scroll mode.

## Native agent thread

A native agent tab is drawn by Fleet, so keys reach its composer rather than a PTY (the harness
snapshot's `mode` is `Agent`). The context is chosen by what the thread is doing: an open decision
card shadows everything else, and the focused transcript row is last.

Creating a thread or selecting an existing agent tab with `ctrl-s 1`–`9` focuses its composer
after the tab's first mounted frame. That request is one-shot: selecting a thread already in
scroll mode or owned by a decision card preserves that mode's keyboard owner instead.

| Context | Key | Action |
| --- | --- | --- |
| `Agent > AgentIdle` | `Enter` | send the composer |
| `Agent > AgentIdle` | `cmd-Enter` | on a thread that has not started: start it in the background |
| `Agent > AgentIdle` | `Shift-Enter` | newline *(not bound — the composer owns it)* |
| `Agent > AgentIdle` | `Shift-Tab` | toggle build ⇄ plan |
| `Agent > AgentIdle` | `/` · `@` · `$` | *(not bound — the composer inserts the character and reports a trigger)* |
| `Agent > AgentIdle` | `Up` / `ctrl-p`, `Down` / `ctrl-n` | move an open completion picker; with no picker, the composer's own caret motion (and prompt history at the visual buffer edge) |
| `Agent > AgentWorking` | `Esc` | interrupt the **active** turn |
| `Agent > AgentWorking` | `Enter` | send — a **steer**, dispatched immediately, never a queue |
| every mode with a composer | `ctrl-s m` · `ctrl-s e` · `ctrl-s t` | model picker · reasoning / traits menu · access mode |
| every mode with a composer | `ctrl-s [` · `ctrl-s x` · `ctrl-s a`/`A` · `ctrl-s F` | toggle scroll mode · close a caller tab or detach a child · new thread · terminal fallback |
| every agent-thread sub-mode | `ctrl-s 1`–`9` · `ctrl-s Tab` · `ctrl-s w` | select a Workspace tab · return to the tab MRU · return to the session MRU |
| every agent-thread sub-mode | `ctrl-s s` · `ctrl-s S` | go to Hub (thread keeps running) · sleep this session and return to the Hub |
| every agent-thread sub-mode | `ctrl-s h`/`p` · `ctrl-s l`/`n` | previous / next tab, across the mixed terminal-and-thread strip |
| every agent-thread sub-mode | `ctrl-s W` · `ctrl-s u` · `ctrl-s d` | session switcher · select the caller (attaching it first) · agent picker seeded with `!` |
| every agent-thread sub-mode | `ctrl-s c` · `ctrl-s b` · `ctrl-s y` · `ctrl-s z` | new terminal tab · this worktree's board tab · copy the worktree path · zoom |
| every agent-thread sub-mode | `ctrl-s v` · `ctrl-s V` · `ctrl-s N` · `ctrl-s P` | the subagent watch pane: show/hide · dismiss · next · previous |
| every agent-thread sub-mode | `ctrl-s !` · `ctrl-s J` · `ctrl-s ?` · `ctrl-s k` · `ctrl-s Esc` | sticky error · jobs panel · help · command palette (⌘K too on macOS) · cancel the prefix |
| both | `Esc` | close a picker, else abandon a gate draft, else leave scroll mode, else interrupt — and nothing at all on an idle thread |
| `Agent > AgentDecision > AgentPermission` | `y` · `a` · `n` · `e` · `Esc` | allow once · allow for this session · deny · edit the command · deny and stop |
| `Agent > AgentDecision > AgentQuestion` | `1`-`5` · `Space` · `Enter` · `p` | choose · toggle (multi-select) · answer / next · previous question |
| `Agent > AgentDecision > AgentPlan` | `y` · `n` · `Enter` | implement · refine · send whatever the composer holds |
| `Agent > AgentNativeScroll` | `j`/`k` · `ctrl-d`/`ctrl-u` · `ctrl-f`/`ctrl-b` · `gg`/`G` · `q`/`i`/`Esc` · `ctrl-s [` | move the focused row · half page · page · oldest/newest · leave scroll mode |
| `Agent > AgentNativeScroll > AgentRow` | `Enter` · `u` · `o` · `y` · `d` · `x` | expand/collapse (or attach a delegation's child) · revert the edit or turn · open in the editor · copy the payload (or a delegation id) · open the diff · cancel a delegation |

Every one of these contexts is derived from **daemon state**, not from the view, which is what
makes a decision own the keyboard in the same frame its gate appears rather than one frame later.
Precedence: `AgentNativeScroll` (a frozen tail beats everything, including an open gate) >
`AgentDecision > AgentPermission | AgentQuestion | AgentPlan` > `AgentWorking` > `AgentIdle`.

`ctrl-s [` freezes the transcript's tail, enters `Agent > AgentNativeScroll` and focuses the row
nearest the bottom of the viewport; the focused row is how the mode shows, and the snapshot's
`mode` reads `Scroll` for as long as it is on. `G` jumps to the newest row without leaving the mode — the tail stays frozen until `q`, `i` or
`Esc` leaves it, which is what re-arms the follow.

**Row focus lives inside scroll mode.** `Agent > AgentNativeScroll > AgentRow` is on the chain
exactly while a row carries the focus ring, which is what finally makes `Enter`/`u`/`o`/`y`/`d`/`x`
fire and retires the caveat that those keys were bound, handled and never entered. `j`/`k` move
the focus and scroll to it. Clicking a tool row, a `thought …` line or a `worked …` fold still
expands and collapses it, so the mouse reaches every `[⏎] show` hint too.

On a delegation row, `Enter` attaches and selects the child and `x` cancels the delegation. On a
result card, `Enter` expands or collapses the body without attaching the child. `y` copies the
delegation id from either row.

**The Workspace session rows are repeated here, and the four PTY-only ones are not.** `^s r`
(restart the exited command), `^s ,` (rename the terminal), `^s ]` (paste into it) and `^s ^s`
(send a literal `ctrl-s`) each address a PTY, and a Fleet-drawn tab has none; binding them would
be a key that silently does nothing. Everything else in the Workspace table above is listed in
this one, handled by the same listener on the same ancestor — `^s s` on the `Fleet` root, the
rest on the Workspace root — so a thread tab loses no session command by being drawn by Fleet.

**An unbound second key is swallowed.** `^s` inside an agent tab is taken by the shell's keystroke
interceptor rather than by gpui's two-key matcher, because gpui replays the keystrokes of a
sequence that matched nothing as *input*: left to it, `^s s` typed an `s` into the composer. The
interceptor consumes `^s`, runs the row the second key names, and where there is no row consumes
that key too and toasts `^s <key> is not bound here`. `^s Esc` cancels the prefix without a toast.

Note that gpui matches `>` as a **subsequence**, not as a parent test, so every `^s` row is
registered against each sub-mode by name — `AgentIdle`/`AgentWorking` are not on the
`AgentDecision > *` chain, and none of the six is on another's — and an embedded pane may not
reuse any context word this table uses.

**`Enter` is not bound on a permission.** A queued Return keystroke must never approve a shell
command. `e` is offered only by a harness whose gate carries an edit answer (Claude, never Codex).
`e` on a permission, `n` on a plan and a question's free-text answer all *open* the composer rather
than answering at once: while that draft is being typed the composer keeps the bare letters and the
gate's keys stand down, so the text can start with a `y`, an `n` or contain a space. `Enter` sends
it, `Esc` abandons it and leaves the gate open.

`Esc` is a keymap binding only in `Agent > AgentWorking`, `Agent > AgentNativeScroll` and
`Agent > AgentDecision > AgentPermission`. In `Agent > AgentIdle` the composer owns the key and
reports it, which runs the same cascade — binding it there as well would take `Esc` away from the
buffer that has to cancel an IME preedit and a selection first. On an idle thread with nothing
open, `Esc` does nothing, and it never quits.

`1`-`5` are bound unconditionally but only the digits a question actually offers are honoured: a
`3` on a two-option question is ignored rather than stored as an answer no label matches. The
fifth digit exists because a question that accepts free text appends a "Something else…" row
after its options, so a four-option question of that kind numbers its last row `5`.

`/`, `@` and `$` are not bound in `Agent > AgentIdle`: the composer inserts the character and
reports it (`MultilineInputEvent::Trigger`), which is what opens the picker, so all three stay
typable inside a prompt and the picker can filter on what follows them. `/` is offered at **line
start only**, because a harness expands a slash command only when it opens the whole message.

`/` lists Fleet's own built-ins before the harness's own commands: `model`, `plan`, `default`,
`compact`, and — on a **Codex** thread only — `login` and `logout`. A built-in is applied locally
and its trigger text is deleted, because a built-in sends nothing to the model; a harness command
is inserted and the harness expands it. `login` and `logout` are absent on a Claude thread rather
than refusing there: Claude publishes no account surface, and a listed command that answers
"unsupported" is the affordance `DESIGN-SYSTEM.md` §7 forbids (`NATIVE-AGENTS.md` §7.1).

An open completion picker is a list under a text field, so DESIGN-SYSTEM §4's `ctrl-n`/`ctrl-p`
and `↓`/`↑` move it, in that sense: `↑` moves the highlight up. All four keys are handed
straight back to the composer when no picker is open, so a `Shift-Enter` draft still moves its
caret and `↑` at the visual top of an untouched buffer still recalls the previous prompt.

`u` reverts **files** and never the conversation: it restores the worktree from a Fleet-owned
checkpoint taken before the turn was submitted (`NATIVE-AGENTS.md` §5). The key is drawn only on a
turn footer whose turn has a checkpoint, because a `[u]` that answers "unsupported" is worse than
no `[u]` — a daemon that keeps none answers `Unsupported` and the hint is simply absent. A **tool
row** never draws it: `TurnCheckpoint` names the turn a file-scope checkpoint was taken in rather
than the edit it covered, so one row has nothing to key on, and pressing `u` there says so rather
than reverting a turn the row did not offer to revert (`NATIVE-AGENTS.md` §13 phase 8).

**Rationale [A25].** The agent surface is a detachable view of the fixed daemon session, not a
navigation destination. Repeating the visible agent key therefore hides it, switching agent keys
reattaches the other live session, and neither path kills a PTY or loses scrollback. `ctrl-q` is
scoped to `Agent` so swarm's hide muscle memory cannot quit Fleet.

## Wheel and viewport shortcuts

| Input | Primary screen | Alternate screen |
| --- | --- | --- |
| Wheel / two-finger trackpad scroll | Fleet scrollback, including Claude Code with mouse tracking | Application wheel reports when tracking is enabled (e.g. OpenCode); otherwise arrow keys with DECSET 1007, or no action |
| Shift + wheel | Application wheel reports if tracking is enabled; otherwise Fleet scrollback | Same routing as ordinary wheel |
| Shift+PageUp / Shift+PageDown | Page up / down, outside copy mode | Forward the original modified key to the child |
| Cmd+Home / Cmd+End | Oldest history / live bottom, outside copy mode | Forward the original modified key to the child |

Trackpad movement accumulates fractional rows using measured cell height. Line-based wheels
move three rows per step by default (`terminal.scrollLinesPerStep` in config.json). Momentum
continues naturally. Typing or pasting returns to live bottom before input reaches the child;
scrolling and copy-mode navigation do not. Output arriving while scrolled up preserves the
history anchor.

## Scroll mode (inside a terminal)

| Key | Action |
| --- | --- |
| `j` / `k`, `ctrl-d` / `ctrl-u`, `ctrl-f` / `ctrl-b`, `gg` / `G` | move viewport |
| `v` | start selection, `y` yank selection, `Esc` clear |
| `/` then text, `n` / `N` | search scrollback (may land after v1; keep the binding reserved) |
| `q`, `i`, `Esc` | back to Terminal mode (viewport snaps to bottom) |

## Workspace (Native mode)

The default third tab (`lg`) is not a terminal: it is Fleet's own git pane
(`crates/fleet-lazygit`) rendered inside the tab, and `ctrl-s b`'s `board` tab is the other
reserved command: it draws this worktree's board with the Hub board's own keys under
`Fleet > Workspace > Native > Board` (see "Board and card detail" below).
`Workspace > Native` binds `ctrl-s` and
nothing else — not even the `cmd-c` / `cmd-v` clipboard keys or the viewport shortcuts
`Workspace > Terminal` reserves, because the pane owns its own selection and its own scrolling —
so every other key belongs to the pane. Where that table lives is the one difference between the
two reserved commands: the git pane's is
`crates/fleet-lazygit/README.md`, while the board tab's is this file, because Fleet draws it
itself. The git pane's full context chain is
`Fleet > Workspace > Native > Lazygit > …`, and the pane's own context words are prefixed `Lg`
(`LgDialog`, `LgConfirm`, `LgHelp`) so they cannot satisfy Fleet's `Dialog`, `Dialog > Confirm`
or `Dialog > Help` predicates.

Two prefix keys behave differently over a native tab, because there is no process behind it:

| Key | Over a native tab |
| --- | --- |
| `ctrl-s [` | no scrollback; a toast says so |
| `ctrl-s r` | nothing to restart |

`q` inside the pane leaves the tab (it selects the previous one) instead of quitting Fleet.

Scroll mode is suppressed while an alt-screen app is running: `ctrl-s [` then shows the toast
`no scrollback in alt-screen`.

## Jobs panel (`J`)

| Key | Action |
| --- | --- |
| `J`, `q` | close, restoring the exact prior focus (pane, row, terminal and mode) |
| `Esc` | collapse an expanded log; otherwise close and restore the prior focus |
| `j` / `k`, `gg` / `G` | move the job cursor |
| `Enter` | expand / collapse the log for the selected job (sheet 440 ↔ 640 px) |
| `c` | cancel the selected job (only when it is cancellable) [A19] |
| `X` | cancel every cancellable job (confirm) [A19] |
| `R` | retry a failed job with identical parameters [A19] |
| `y` | copy the log path of the selected job [A19] |
| `D` | dismiss finished and failed jobs [A19] |
| `f` | cycle the filter all → running → failed → done; inside an expanded log, toggle follow [A19] |

Inside an **expanded log**: `f` toggles follow, `j` / `k` scroll, `G` re-enables follow, `Esc`
collapses the sheet back to 440 px (a second `Esc` closes the panel).

Every key has a pointer twin that dispatches the same action (ADR 0023): a press on a row moves
the cursor, a double-click is `Enter`, a right click opens the row's menu (Show log, Retry, Cancel,
Copy log path — each only when it can work). A failed row carries Retry `R`, Show log `⏎` and Copy
log path `y`; a cancellable running row shows Cancel `c` while it is hovered or selected. The
header's segments pick the filter `f` cycles, Clear finished is `D`, the ⋯ holds Cancel all `X`,
and the ✕ is `Esc`. In an expanded log: Back `Esc`, Following `f`, Jump to end `G`.

## Dialogs and text inputs

Every live `TextInput` entity publishes `FleetTextInput`; this is the one application table that
defines its editing. Printable text and IME input are delivered by the component itself. `Tab`,
`S-Tab`, `ctrl-n`, `ctrl-p`, and `Esc` remain container keys. A multi-line input publishes
`enter = newline | owner`: plain `Enter` inserts only under
`FleetTextInput && mode == multiline && enter == newline`, while `Shift-Enter` inserts in every
multi-line input. The agent composer publishes `owner`, so its plain `Enter` reaches Send/Steer.

### Motion

| Key | Action |
| --- | --- |
| `left` | move left one grapheme |
| `right` | move right one grapheme |
| `alt-left` | move to the previous word boundary |
| `alt-right` | move to the next word boundary |
| `home` | move to the visual row start (logical line without a layout) |
| `end` | move to the visual row end (logical line without a layout) |
| `cmd-left` | move to the logical line start |
| `cmd-right` | move to the logical line end |
| `up` | move one visual row up (logical line without a layout) |
| `down` | move one visual row down (logical line without a layout) |
| `cmd-up` | move to the document start |
| `cmd-down` | move to the document end |
| `ctrl-a` | move to the logical line start |
| `ctrl-e` | move to the logical line end |
| `ctrl-shift-a` | extend to the logical line start |
| `ctrl-shift-e` | extend to the logical line end |
| `ctrl-b` | move left one grapheme |
| `ctrl-f` | move right one grapheme |

### Selection

| Key | Action |
| --- | --- |
| `shift-left` | extend left one grapheme |
| `shift-right` | extend right one grapheme |
| `alt-shift-left` | extend to the previous word boundary |
| `alt-shift-right` | extend to the next word boundary |
| `shift-home` | extend to the visual row start (logical line without a layout) |
| `shift-end` | extend to the visual row end (logical line without a layout) |
| `cmd-shift-left` | extend to the logical line start |
| `cmd-shift-right` | extend to the logical line end |
| `shift-up` | extend one visual row up (logical line without a layout) |
| `shift-down` | extend one visual row down (logical line without a layout) |
| `cmd-shift-up` | extend to the document start |
| `cmd-shift-down` | extend to the document end |
| `cmd-a` | select all |

### Deletion

| Key | Action |
| --- | --- |
| `backspace` | delete the previous grapheme or selection |
| `delete` | delete the next grapheme or selection |
| `alt-backspace` | delete the previous word run |
| `alt-delete` | delete the next word run |
| `cmd-backspace` | delete to the logical line start |
| `cmd-delete` | delete to the logical line end |
| `ctrl-w` | delete the previous word run |
| `ctrl-u` | delete to the logical line start |
| `ctrl-k` | delete to the logical line end |
| `ctrl-h` | delete the previous grapheme or selection |
| `ctrl-d` | delete the next grapheme or selection |

### Clipboard

| Key | Action |
| --- | --- |
| `cmd-c` | copy the selection |
| `cmd-x` | cut the selection |
| `cmd-v` | paste clipboard text |

`ctrl-v` is deliberately unbound: it remains available to shells and terminal applications.

### History

| Key | Action |
| --- | --- |
| `cmd-z` | undo |
| `cmd-shift-z` | redo |

### Newline

| Key | Action |
| --- | --- |
| `enter` | insert under `FleetTextInput && mode == multiline && enter == newline` only |
| `shift-enter` | insert under `FleetTextInput && mode == multiline` |

**Key ownership.** Every dialog field is a live `TextInput`, so the `Dialog` container binds no
editing key of its own: `Backspace`, `ctrl-w`, `ctrl-u`, `ctrl-a` / `ctrl-e` and `←` / `→` all
belong to the `FleetTextInput` table above wherever an editor owns the keyboard. A surface with
bare-letter or caret-collision commands publishes its existing word while browsing and a distinct
`*Editing` word while a field owns typing. The browsing words are `CardDetail`, `BoardSettings`,
`Settings`, and `Create`; their editing partners are `CardDetailEditing`, `BoardSettingsEditing`,
`SettingsEditing`, and `CreateEditing`; Settings' header search publishes `SettingsSearch`, a third
word of the same kind. Editing words contain only container commands, so the
deeper `FleetTextInput` rows own editing and no dialog action can steal an accepted character.
Lists under an input use `ctrl-n` / `ctrl-p` or `down` / `up`; `Tab` / `S-Tab` move fields, `Enter`
confirms where the single-line container says so, and `Esc` cancels. `CardPicker` is the documented
exception: its query is a filter that never contains a space, so it always publishes
`Dialog > CardPicker` and `space` toggles the highlighted card.

| Dialog | Keys beyond the shared frame |
| --- | --- |
| Create worktree | under browsing `Dialog > Create`, `←` / `→` cycle the host; the branch editor publishes `Dialog > CreateEditing`, where the same arrows move its caret · `Tab` / `S-Tab` move between branch, base and host · `Enter` create & open (create only, while the dialog's "Open after creating" box is unchecked) · `⌥Enter` create **without** opening [A8] |
| Clone repo | type to search · `ctrl-n` / `ctrl-p` or `↓` / `↑` · `Enter` clone · `Esc` cancels only the search request, never a started clone |
| Confirm (delete / prune / kill / close terminal) | `y` / `Enter` confirm · `Y` **required instead of `y`** when any decisive safety fact is unknown or the inspection errored, and for repo / context delete [A12] · `n` / `Esc` / `q` cancel · `I` re-check (delete) · `s` toggle the KEEP list (prune). Nothing else is bound. |
| New / Edit context | `Tab` / `S-Tab` move between name and owners · `ctrl-shift-d` delete this context (routes to the expanded `Y` confirm); plain `ctrl-d` belongs to the focused editor |
| Repository hooks (`e`) | one editor per command row, `Tab` / `S-Tab` between them; a filled trailing row grows the next blank one · `Enter` saves |
| Assign repo to context (`m`) | this dialog has **no** text field, so `j` / `k` move the selection as well as `↓` / `↑` and `ctrl-n` / `ctrl-p` |
| Settings (`,`) | browsing is `Dialog > Settings`: `Tab` / `S-Tab` move between sections · `Space` toggles · `h` / `l` or `←` / `→` cycle a choice · `j` / `k` move · `E` opens `config.json` · `D` runs doctor · `/` searches every section · `Enter` on a text or number row opens it for editing, and saves on every other row. That row's editor publishes `Dialog > SettingsEditing`, where every printable key types and `Enter` saves; `ctrl-n` / `ctrl-p` or `↓` / `↑` move to the next row and close it, and `Esc` discards in either word. The search field publishes `Dialog > SettingsSearch`, where every printable key types, `ctrl-n` / `ctrl-p` or `↓` / `↑` move over the matching settings, `Enter` opens the selected one's section with the cursor on it, and `Esc` clears the search and gives the keys back to the rows (a second `Esc` discards). Every key has a pointer twin: the rail, a row, a switch, a segment or dropdown option, a text or number box (`Enter`), Open config.json (`E`), Run doctor (`D`), Cancel (`Esc`) and Save (`Enter`). |
| Help (`?`) | `Esc` / `?` close · typing goes to its search field · `↓` / `↑` or `ctrl-n` / `ctrl-p` move in its list · `Enter` runs the row (or opens a guide a search found) on the surface Help was opened over · `ctrl-tab` / `ctrl-shift-tab` switch Guides ⇄ All shortcuts |
| Quit (`ctrl-q`) | `y` quit · `n` / `Esc` cancel · `J` open the jobs panel · `W` never warn again (writes `jobs.warnBeforeQuit=false`) and quit [A23] |
| Quit and stop daemon (`ctrl-shift-q`) | `Y` stop and quit · `n` / `Esc` cancel |

**`E` is context-scoped.** In `Hub` it edits the active context [A15]; in `Dialog > Settings` it
opens `config.json` in a new terminal tab. The two never coexist in one key context.

## Open menus (`FleetMenu`)

A kit `Menu` — a row's ⋯ or right-click menu, the `+` new-tab menu, a dropdown's options — takes
the focus while it is open and publishes `FleetMenu` beneath the context of the element that
opened it. It is navigated like a list under a text field (ADR 0023), so `j` / `k` are not menu
keys. Each item shows its own key as a chip; that key still works from the surface once the menu
is closed. On close the focus goes back to where it was.

| Key | Action |
| --- | --- |
| `↓` / `ctrl-n` | highlight the next item (stops at the last) |
| `↑` / `ctrl-p` | highlight the previous item (stops at the first) |
| `Enter` | close the menu and run the highlighted item, dispatched to the element that opened it |
| `Esc` | close the menu without running anything |

## Filter mode (`/`)

| Key | Action |
| --- | --- |
| printable, `Backspace`, `ctrl-w`, `ctrl-u`, motion, selection, undo | edit the query through the full `FleetTextInput` table above |
| `ctrl-n` / `↓`, `ctrl-p` / `↑` | move the **list** cursor while still typing |
| `Enter` | open the highlighted row directly from inside the input [A24] |
| `Esc` | first press leaves the input keeping the filter, second press clears it — **never quits** [A13] |

## Palette mode (`:`)

⌘K on macOS and `ctrl-k` on other platforms open the palette alongside `:`, in every context
where `:` does (`Hub` and `Dialog > CardDetail`), and so does clicking the title bar's command
field. `:` stays bound. Where `ctrl-k` already has an owner it keeps it: a terminal grid, where
it belongs to the shell, and a focused `FleetTextInput`, where it deletes to the line end
(ADR 0020). ⌘K also opens the palette from the Workspace — its terminal and native tabs and the
agent thread — on macOS; on every platform the Workspace has `ctrl-s k`, the only Workspace key
on Linux. The title bar's chip shows whichever key the focused context binds.

The query's first character narrows the list: `>` commands, `@` worktrees and sessions, `#`
cards, `!` agent threads (UX-SPEC §3.9).

| Key | Action |
| --- | --- |
| printable, `Backspace`, `ctrl-w`, `ctrl-u`, motion, selection, undo | edit the query through the full `FleetTextInput` table above |
| `ctrl-n` / `↓`, `ctrl-p` / `↑` | move through the ranked rows, across sections; the list scrolls to keep the row in view |
| `Enter` | run the highlighted row (destructive commands still route through their confirm); a click on a row does the same |
| `Esc` | close (`q` remains a printable query character) |

## Daemon-down surfaces (§3.12)

| Surface | Key | Action |
| --- | --- | --- |
| **B. fleetd will not start** (full window) | `r` | retry starting fleetd |
| | `L` | open `~/.fleet/logs/fleetd.log` |
| | `D` | run doctor |
| | `ctrl-q` | quit |
| **C. fleetd died while attached** (40 px banner) | `r` | reconnect now |
| | `l` | open the log |
| | `Esc` | dismiss the banner (the daemon dot stays red) |
| **Doctor report** (`Daemon > Doctor`, full window) | `r` | dismiss the report, then retry or reconnect if the daemon is down |
| | `L` | open the log |
| | `D` | run doctor again |
| | `Esc` | dismiss the report and return to the surface it was raised from |

The doctor report replaces the whole context chain, so it repeats the recovery keys of the
surface it was raised from — B, C, or Settings §3.8.6 About, where the daemon is healthy and
`r` only closes the report.

Case C's banner is a container, and `r` and `l` are bare letters, so it obeys the key-ownership
rule above: `Daemon > Banner` leaves the chain entirely — `Esc` with it — while a live input owns
the keyboard, and comes straight back when the keyboard returns to a surface that is not typing.

Every key in this table is also a button showing that key (ADR 0023): case B's `Retry`, `Open
log`, `Run doctor` and `Quit`; case C's `Reconnect now`, `Open log` and ✕; the report's `Run
again`, `Open log` and `Close`. Each dispatches the same action its key does.

Case A (cold start) binds nothing: fleetd is auto-spawned. While disconnected, read-only keys
(`j` / `k`, `y`, `b`, `/`, `i`, `:`) keep working; mutating keys flash the banner. Keys typed
into a veiled terminal grid are dropped, not buffered.

## First run (`FirstRun` context)

| Key | Action |
| --- | --- |
| `i` | `fleet import --from-swarm` as a job — shown only when `~/.swarm/state.json` exists |
| `N` | create your first context |
| `n` | clone a repository |
| `?` / `,` | help / settings |
| `ctrl-q` | quit |

The page's step cards and footer buttons dispatch these same actions and show these keys: step 1
is `N`, step 2 `n`, the import card `i`, and the footer's `Keyboard shortcuts` and `Settings` are
`?` and `,`.

**Arbitration.** `i` means *import* only in the `FirstRun` context; in `Hub` it stays *toggle
detail panel*. The first-run card is a full-window surface with its own key context, so the two
never overlap.

## Explicitly rejected

Recorded so they are not re-proposed: `ctrl-s o` for the MRU session (collides with tmux's
"other pane" and with `o` = open in the Hub and on the PR screen — use `ctrl-s w`); `0` for the
`All` pseudo-repo (breaks the digit vocabulary, where `1`–`9` mean *context* everywhere — use
`ga`); `Space` hold-to-peek the detail panel (hold semantics exist nowhere else, and it
duplicates `i`); unbinding `D` (the risk is handled by `Y` + the fact list + `u`, not by hiding
a documented key).

## Conflicts noticed and deliberately kept

`Tab` is "next pane" in the Hub and "next PR tab" on the PR screen. `i` is "toggle detail" in
the Hub and "leave Scroll mode" in the Workspace. `r` is "refresh" in Normal and "restart" after
`ctrl-s`. `c` is "create without opening" on the PR screen, "new terminal tab" after `ctrl-s`,
and "cancel job" in the Jobs panel. `b` is "open in browser" in the Hub and on the PR screen and
"this worktree's board tab" after `ctrl-s`.
`f` cycles the Jobs filter in the list and toggles follow
inside an expanded log. `y` copies a path, a URL or a log path depending on the pane. All are
mode- or pane-disjoint; Help's All shortcuts table files every key under the place it works, so
they are read side by side with their place.

## Board and card detail (BOARD §8)

Board shortcuts override the inherited Hub shortcuts. `j` moves down (next),
and `k` moves up (previous), following the global nvim convention.

`/` does **not** open the Hub's filter: the board owns `BoardState.filter`,
and while its input has the keyboard the screen publishes the `Filter` key context
instead of the board word it would otherwise publish. Every row below is therefore shadowed
while you are typing a filter, and the `Filter` rows above apply instead — including their
two-stage `Esc`.

The board has two surfaces and one key table. The Hub's board tab publishes
`Fleet > Hub > Board`; the worktree board drawn in a Workspace's `fleet://board` tab
publishes `Fleet > Workspace > Native > Board`, and every row is repeated verbatim for
it below against the same action. `ctrl-s` is not among them: the board word nests
**under** `Workspace > Native`, which keeps the prefix, so `ctrl-s 1` leaves the tab
and `ctrl-s b` returns to it. `Filter > BoardFilter` serves both surfaces, and so do the
four board dialogs below — they are opened by the same keys from either one and publish the
same `Dialog > …` words. One row answers differently by surface: `o`
(`board::OpenWorktree`) on a card linked to the worktree you are already standing in
answers "Already in this worktree" instead of re-opening the session, which can only happen
in the pane, because the Hub is never standing in a worktree.

The board dialog rows below are **in addition to** the generic `Dialog` container rows. Browsing
uses the existing dialog word; a text owner replaces it with the matching `*Editing` word, and a
migrated field appends `FleetTextInput` beneath that. Thus `CardDetail` /
`CardDetailEditing` and `BoardSettings` / `BoardSettingsEditing` never expose their bare browsing
keys while text is being edited. The card picker keeps `Dialog > CardPicker`: its query is a filter
that cannot contain a space, and `space` toggles the highlighted card.

| Key | Context | Action |
| --- | --- | --- |
| `g b` | `Hub` | `board::GoBoard` — Go to board |
| `h` | `Hub > Board` | `board::PrevColumn` — Previous column |
| `left` | `Hub > Board` | `board::PrevColumn` — Previous column |
| `l` | `Hub > Board` | `board::NextColumn` — Next column |
| `right` | `Hub > Board` | `board::NextColumn` — Next column |
| `j` | `Hub > Board` | `board::NextCard` — Next card |
| `down` | `Hub > Board` | `board::NextCard` — Next card |
| `k` | `Hub > Board` | `board::PrevCard` — Previous card |
| `up` | `Hub > Board` | `board::PrevCard` — Previous card |
| `enter` | `Hub > Board` | `board::OpenCard` — Open card |
| `c` | `Hub > Board` | `board::NewCard` — New card |
| `s` | `Hub > Board` | `board::PickStatus` — Status picker |
| `p` | `Hub > Board` | `board::PickPriority` — Priority picker |
| `a` | `Hub > Board` | `board::PickAssignee` — Assignee picker |
| `t` | `Hub > Board` | `board::PickLabels` — Labels picker |
| `e` | `Hub > Board` | `board::PickEstimate` — Estimate picker |
| `[` | `Hub > Board` | `board::MovePrevColumn` — Move card to previous column |
| `]` | `Hub > Board` | `board::MoveNextColumn` — Move card to next column |
| `w` | `Hub > Board` | `board::CreateWorktree` — Create worktree from card |
| `o` | `Hub > Board` | `board::OpenWorktree` — Open linked worktree |
| `S` | `Hub > Board` | `board::Sync` — Sync |
| `F` | `Hub > Board` | `board::FullSync` — Full sync, ignoring the incremental cursor |
| `x` | `Hub > Board` | `board::OpenRemote` — Open the focused card's remote issue |
| `d` | `Hub > Board` | `board::DeleteCard` — Delete card (a mirrored card answers "Mirrored card — delete it in the backend") |
| `,` | `Hub > Board` | `board::Settings` — Settings |
| `r` | `Hub > Board` | `board::Reload` — Reload |
| `/` | `Hub > Board` | `board::Filter` — Filter cards |
| `b` | `Hub > Board` | `board::PickBlockedBy` — Pick the cards this one is blocked by (shadows the Hub's `b` while the board owns the keys) |
| `m` | `Hub > Board` | `board::PickAgent` — Pick the agent that runs this card |
| `C` | `Hub > Board` | `board::Columns` — Board settings, on its Columns section |
| `h` | `Workspace > Native > Board` | `board::PrevColumn` — Previous column |
| `left` | `Workspace > Native > Board` | `board::PrevColumn` — Previous column |
| `l` | `Workspace > Native > Board` | `board::NextColumn` — Next column |
| `right` | `Workspace > Native > Board` | `board::NextColumn` — Next column |
| `j` | `Workspace > Native > Board` | `board::NextCard` — Next card |
| `down` | `Workspace > Native > Board` | `board::NextCard` — Next card |
| `k` | `Workspace > Native > Board` | `board::PrevCard` — Previous card |
| `up` | `Workspace > Native > Board` | `board::PrevCard` — Previous card |
| `enter` | `Workspace > Native > Board` | `board::OpenCard` — Open card |
| `c` | `Workspace > Native > Board` | `board::NewCard` — New card |
| `s` | `Workspace > Native > Board` | `board::PickStatus` — Status picker |
| `p` | `Workspace > Native > Board` | `board::PickPriority` — Priority picker |
| `a` | `Workspace > Native > Board` | `board::PickAssignee` — Assignee picker |
| `t` | `Workspace > Native > Board` | `board::PickLabels` — Labels picker |
| `e` | `Workspace > Native > Board` | `board::PickEstimate` — Estimate picker |
| `[` | `Workspace > Native > Board` | `board::MovePrevColumn` — Move card to previous column |
| `]` | `Workspace > Native > Board` | `board::MoveNextColumn` — Move card to next column |
| `w` | `Workspace > Native > Board` | `board::CreateWorktree` — Create worktree from card |
| `o` | `Workspace > Native > Board` | `board::OpenWorktree` — Open linked worktree |
| `S` | `Workspace > Native > Board` | `board::Sync` — Sync |
| `F` | `Workspace > Native > Board` | `board::FullSync` — Full sync, ignoring the incremental cursor |
| `x` | `Workspace > Native > Board` | `board::OpenRemote` — Open the focused card's remote issue |
| `d` | `Workspace > Native > Board` | `board::DeleteCard` — Delete card (a mirrored card answers "Mirrored card — delete it in the backend") |
| `,` | `Workspace > Native > Board` | `board::Settings` — Settings |
| `r` | `Workspace > Native > Board` | `board::Reload` — Reload |
| `/` | `Workspace > Native > Board` | `board::Filter` — Filter cards |
| `A` | `Workspace > Native > Board` | `board::AttachRun` — Attach the focused card's run as an agent tab |
| `X` | `Workspace > Native > Board` | `board::CancelRun` — Cancel the focused card's live run |
| `>` | `Workspace > Native > Board` | `board::RunNow` — Run the column's action on the focused card now |
| `b` | `Workspace > Native > Board` | `board::PickBlockedBy` — Pick the cards this one is blocked by |
| `m` | `Workspace > Native > Board` | `board::PickAgent` — Pick the agent that runs this card |
| `C` | `Workspace > Native > Board` | `board::Columns` — Board settings, on its Columns section |
| `escape` | `Dialog > CardDetail` | `card_detail::Close` — Close |
| `i` | `Dialog > CardDetail` | `card_detail::EditTitle` — Edit title |
| `d` | `Dialog > CardDetail` | `card_detail::EditDescription` — Edit description |
| `c` | `Dialog > CardDetail` | `card_detail::AddComment` — Add comment |
| `j` | `Dialog > CardDetail` | `card_detail::NextProperty` — Next property |
| `k` | `Dialog > CardDetail` | `card_detail::PrevProperty` — Previous property |
| `enter` | `Dialog > CardDetail` | `card_detail::EditProperty` — Expand every folded run report, else edit the selected property |
| `w` | `Dialog > CardDetail` | `card_detail::CreateWorktree` — Create worktree |
| `x` | `Dialog > CardDetail` | `card_detail::OpenRemote` — Open the remote issue |
| `K` | `Dialog > CardDetail` | `card_detail::KeepLocal` — Resolve conflict: keep local |
| `R` | `Dialog > CardDetail` | `card_detail::TakeRemote` — Resolve conflict: take remote |
| `ctrl-s` | `Dialog > CardDetail` | `card_detail::Save` — Save text edit |
| `A` | `Dialog > CardDetail` | `board::AttachRun` — Attach this card's run as an agent tab |
| `X` | `Dialog > CardDetail` | `board::CancelRun` — Cancel this card's live run |
| `>` | `Dialog > CardDetail` | `board::RunNow` — Run the column's action on this card now |
| `b` | `Dialog > CardDetail` | `board::PickBlockedBy` — Pick the cards this one is blocked by |
| `m` | `Dialog > CardDetail` | `board::PickAgent` — Pick the agent that runs this card |
| `s` | `Dialog > CardDetail` | `board::PickStatus` — Status picker for the card on show (the Status row) |
| `p` | `Dialog > CardDetail` | `board::PickPriority` — Priority picker for the card on show |
| `a` | `Dialog > CardDetail` | `board::PickAssignee` — Assignee picker for the card on show |
| `t` | `Dialog > CardDetail` | `board::PickLabels` — Labels picker for the card on show |
| `e` | `Dialog > CardDetail` | `board::PickEstimate` — Estimate picker for the card on show |
| `o` | `Dialog > CardDetail` | `board::OpenWorktree` — Open the card's linked worktree session |
| `:` | `Dialog > CardDetail` | `OpenPalette` — Command palette over the open card detail (⌘K / `ctrl-k` too) |
| `escape` | `Dialog > CardDetailEditing` | `card_detail::Close` — Cancel text edit |
| `enter` | `Dialog > CardDetailEditing` | `card_detail::EditProperty` — Submit title or insert a legacy multiline newline |
| `ctrl-s` | `Dialog > CardDetailEditing` | `card_detail::Save` — Save text edit |
| `ctrl-enter` | `Dialog > CardDetailEditing` | `card_detail::Save` — Save text edit (post the comment) |
| `ctrl-enter` | `Dialog > CardCreate` | `board::CreateAndOpen` — Create and open |
| `space` | `Dialog > CardPicker` | `settings::Toggle` — Toggle the highlighted label |
| `j` | `Dialog > BoardSettings` | `settings::MoveDown` — Next row |
| `k` | `Dialog > BoardSettings` | `settings::MoveUp` — Previous row |
| `h` | `Dialog > BoardSettings` | `settings::CyclePrev` — Previous choice |
| `l` | `Dialog > BoardSettings` | `settings::CycleNext` — Next choice |
| `left` | `Dialog > BoardSettings` | `settings::CyclePrev` — Previous choice |
| `right` | `Dialog > BoardSettings` | `settings::CycleNext` — Next choice |
| `space` | `Dialog > BoardSettings` | `settings::Toggle` — Toggle the row |
| `n` | `Dialog > BoardSettings` | `board_settings::NewColumn` — Add a column to the draft |
| `d` | `Dialog > BoardSettings` | `board_settings::DeleteColumn` — Delete the focused column |
| `J` | `Dialog > BoardSettings` | `board_settings::MoveColumnDown` — Move the column one place later |
| `K` | `Dialog > BoardSettings` | `board_settings::MoveColumnUp` — Move the column one place earlier |
| `P` | `Dialog > BoardSettings` | `board_settings::ApplyPreset` — Add the workflow preset's missing columns |
| `ctrl-s` | `Dialog > BoardSettings` | `board_settings::Save` — Save the board's settings |
| `ctrl-s` | `Dialog > BoardSettingsEditing` | `board_settings::Save` — Save the board's settings |
| `enter` | `Dialog > BoardSettingsEditing` | `dialog::Confirm` — Open a column, commit an edit, or save |

Three of the board's keys act on a card's **run** rather than its fields — `A` attach, `X` cancel,
`>` run now — and they are bound on the worktree board and the card detail only: the Hub's context
board has no worktree to run in, and a key that could only refuse is worse than no key. `b` and `m`
are bound on all three surfaces, and on `Hub > Board` `b` **shadows** the Hub's own `b` (open in
browser) for as long as the board owns the keys, which is what a board screen inside the Hub means
(`APP-CONTRACTS.md` §3). `C` opens Board settings on its Columns section on both boards.

Board filtering keeps horizontal navigation on keys the input does not own. `left` / `right` and
`ctrl-b` / `ctrl-f` now move the caret through `FleetTextInput`; column navigation therefore uses
`Tab` / `Shift-Tab`:

| Key | Context | Action |
| --- | --- | --- |
| `shift-tab` | `Filter > BoardFilter` | `board::PrevColumn` — Previous column |
| `tab` | `Filter > BoardFilter` | `board::NextColumn` — Next column |

In card-detail multi-line editors, Tab inserts a hard tab. In CardCreate, Tab and Shift-Tab move
between title and description; Enter in the description inserts a newline,
and Ctrl-Enter creates and opens the card. Escape in the property picker returns to the
card detail when opened there. Space toggles both labels and custom multi-select options.
