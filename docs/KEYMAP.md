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

## Modes and key contexts

| Mode | gpui key context | Entered by | Left by |
| --- | --- | --- | --- |
| Normal | `Hub` / `Hub > Repos` / `Hub > Worktrees` / `Hub > Prs` / `Hub > Board` | app start, `ctrl-s s` from a terminal, `Esc` from dialogs | opening a session |
| Terminal | `Workspace > Terminal` | opening a worktree session, `Enter` on a session tab | `ctrl-s` (prefix) |
| Native | `Workspace > Native` | selecting a tab whose configured command is a `fleet://` surface (the default third tab, `lg`) | `ctrl-s` (prefix), or selecting a PTY tab |
| Prefix | `Workspace > Prefix` (one-shot) | `ctrl-s` inside Terminal or Native | any key (consumed) or `Esc` |
| Scroll | `Workspace > Scroll` | `ctrl-s [` | `Esc`, `q`, `i` |
| Agent terminal | `Agent > Terminal` | `a`/`A` in Hub, `ctrl-s a`/`ctrl-s A` in Workspace | `ctrl-s` (agent prefix), `ctrl-q` (hide) |
| Agent prefix | `Agent > Prefix` (one-shot) | `ctrl-s` inside the popup | any key (consumed) or `Esc` |
| Agent scroll | `Agent > Scroll` | `ctrl-s [` inside the popup | `Esc`, `q`, `i` |
| Filter | `Filter` | `/` in a list | `Esc` (first keeps filter, second clears), `Enter` |
| Palette | `Palette` | `:` | `Esc`, `Enter` |
| Dialog | `Dialog > <name>` | action | `Esc`, `Enter` |
| Jobs | `Jobs` / `Jobs > Log` (overlay) | `J` anywhere in Normal, `ctrl-s J` in a terminal | `Esc`, `J`, `q` |
| Daemon | `Daemon > Down` / `Daemon > Banner` / `Daemon > Doctor` | fleetd will not start (§3.12 B), fleetd died while attached (§3.12 C), doctor runs | daemon comes back, `Esc` (banner/doctor), `ctrl-q` |
| FirstRun | `FirstRun` | no contexts and no repos exist | any key that creates or imports something |

The context stack is ordered: the Agent popup shadows `Hub` and `Workspace`; Help and the two
quit confirms may shadow the Agent popup. Palette and Settings are unavailable while the popup
owns focus. `Daemon > Down` and `FirstRun` are full-window and shadow everything except `ctrl-q`.

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
| `:` | command palette |
| `,` | settings |
| `?` | help |
| `J` | jobs panel |
| `!` | focus the sticky error slot: the last failed job, offering `R retry` [A18] |
| `i` | toggle the detail panel (never focusable; it mirrors the cursor row) |
| `H` | collapse / expand the repos rail (240 ↔ 44 px icon rail) [A22] |
| `a` / `A` | open the floating Claude / OpenCode agent popup [A21] |
| `r` | refresh (status, PRs, discovery) — runs as a job, never blocks |
| `U` | update Fleet (job) |
| `N` / `E` / `D` | new context / edit active context / delete active context (confirm) [A15] |
| `y` | copy path (worktree) or URL (PR) |
| `b` | open PR / repo in browser |
| `Esc` | clear filter if any, else close the topmost overlay, else no-op — **never quits** [A13] |
| `q` | close the topmost overlay; on the PR screen with no overlay open it returns to Worktrees; in the Hub lists with nothing open it is **unbound** [A14] |
| `ctrl-q` | quit app (daemon keeps running) |
| `ctrl-shift-q` | quit app and stop daemon (confirm; lists running jobs/sessions) |

`E` opens the Edit-context dialog, from which `ctrl-d` deletes the context. `D` keeps its
meaning and routes to the same expanded `Y` confirm.

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
| `W` | session switcher: the palette pre-filtered to `GO`/sessions [A4] |
| `c` | new terminal tab (shell in worktree path) |
| `x` | close current terminal (confirm if a keep-alive process is running) |
| `r` | restart the exited command in this terminal [A10] |
| `y` | copy the worktree path of the current session [A11] |
| `,` | rename current terminal |
| `[` | Scroll mode |
| `]` | paste clipboard (bracketed when the app requests it) |
| `a` / `A` | open the floating Claude / OpenCode agent popup |
| `z` | zoom: hide the session header and terminal tab strip; watch pane stays visible (toggle) |
| `v` | hide/show the cooperative/discovered subagent watch pane; no watches → `no subagent watches` |
| `V` | dismiss the selected exited watch; if running, hide pane and show `watch still running; pane hidden` |
| `N` | next watch in the visible session's start order, wrapping; show the pane if hidden |
| `P` | previous watch in the visible session's start order, wrapping; show the pane if hidden |
| `!` | focus the sticky error slot [A18] |
| `J` | jobs panel |
| `?` | help overlay listing this table |
| `Esc` | cancel prefix |

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
| `A` | hide when OpenCode is visible; otherwise switch to OpenCode |
| `[` | enter Agent Scroll mode |
| `]` | paste clipboard |
| `r` | restart the exited agent command [A10] |
| `?` | open Help above the popup |
| `Esc` | cancel Prefix |

Agent Scroll mode uses the same `j`/`k`, half/page, `gg`/`G`, `v`/`y`, search-reservation, and
exit keys as Workspace Scroll mode.

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
(`crates/fleet-lazygit`) rendered inside the tab. `Workspace > Native` binds `ctrl-s` and
nothing else — not even the `cmd-c` / `cmd-v` clipboard keys or the viewport shortcuts
`Workspace > Terminal` reserves, because the pane owns its own selection and its own scrolling —
so every other key belongs to the pane, whose own key table lives in
`crates/fleet-lazygit/README.md`. The full context chain is
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
| `f` | cycle the filter all → running → failed; inside an expanded log, toggle follow [A19] |

Inside an **expanded log**: `f` toggles follow, `j` / `k` scroll, `G` re-enables follow, `Esc`
collapses the sheet back to 440 px (a second `Esc` closes the panel).

## Dialogs and text inputs

Text inputs accept printable keys, `Backspace`, `ctrl-w` (delete word), `ctrl-u` (clear),
`ctrl-a` / `ctrl-e` (home / end), `←` / `→`. Lists under a text input use `ctrl-n` / `ctrl-p`
or `↓` / `↑` (never `j` / `k`, because the text field owns them). `Tab` / `S-Tab` move between
fields. `Enter` confirms; `Esc` cancels.

| Dialog | Keys beyond the shared frame |
| --- | --- |
| Create worktree | `←` / `→` cycle the host · `Enter` create & open · `⌥Enter` create **without** opening [A8] |
| Clone repo | type to search · `ctrl-n` / `ctrl-p` or `↓` / `↑` · `Enter` clone · `Esc` cancels only the search request, never a started clone |
| Confirm (delete / prune / kill / close terminal) | `y` / `Enter` confirm · `Y` **required instead of `y`** when any decisive safety fact is unknown or the inspection errored, and for repo / context delete [A12] · `n` / `Esc` / `q` cancel · `I` re-check (delete) · `s` toggle the KEEP list (prune). Nothing else is bound. |
| New / Edit context | `ctrl-d` delete this context (routes to the expanded `Y` confirm) |
| Assign repo to context (`m`) | this dialog has **no** text field, so `j` / `k` move the selection as well as `↓` / `↑` and `ctrl-n` / `ctrl-p` |
| Settings (`,`) | `Space` toggles · `h` / `l` or `←` / `→` cycle a choice · `j` / `k` move (surrendered while a text input has focus) · `Enter` saves · `Esc` discards · `E` open `config.json` in a new terminal tab · `D` run doctor |
| Help (`?`) | `Esc` / `?` close |
| Quit (`ctrl-q`) | `y` quit · `n` / `Esc` cancel · `J` open the jobs panel · `W` never warn again (writes `jobs.warnBeforeQuit=false`) and quit [A23] |
| Quit and stop daemon (`ctrl-shift-q`) | `Y` stop and quit · `n` / `Esc` cancel |

**`E` is context-scoped.** In `Hub` it edits the active context [A15]; in `Dialog > Settings` it
opens `config.json` in a new terminal tab. The two never coexist in one key context.

## Filter mode (`/`)

| Key | Action |
| --- | --- |
| printable, `Backspace`, `ctrl-w`, `ctrl-u` | edit the query |
| `ctrl-n` / `↓`, `ctrl-p` / `↑` | move the **list** cursor while still typing |
| `Enter` | open the highlighted row directly from inside the input [A24] |
| `Esc` | first press leaves the input keeping the filter, second press clears it — **never quits** [A13] |

## Palette mode (`:`)

| Key | Action |
| --- | --- |
| printable, `Backspace`, `ctrl-w`, `ctrl-u` | edit the query |
| `ctrl-n` / `↓`, `ctrl-p` / `↑` | move between `GO` / `DO` / `CONTEXT` rows |
| `Enter` | run the highlighted row (destructive commands still route through their confirm) |
| `Esc` | close (`q` remains a printable query character) |

## Daemon-down surfaces (§3.12)

| Surface | Key | Action |
| --- | --- | --- |
| **B. fleetd will not start** (full window) | `r` | retry starting fleetd |
| | `L` | open `~/.fleet/logs/fleetd.log` |
| | `D` | run doctor |
| | `ctrl-q` | quit |
| **C. fleetd died while attached** (28 px banner) | `r` | reconnect now |
| | `l` | open the log |
| | `Esc` | dismiss the banner (the daemon dot stays red) |

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
and "cancel job" in the Jobs panel. `f` cycles the Jobs filter in the list and toggles follow
inside an expanded log. `y` copies a path, a URL or a log path depending on the pane. All are
mode- or pane-disjoint; the Help dialog groups by mode precisely so they can be read side by
side.

## Board and card detail (BOARD §8)

Board shortcuts override the inherited Hub shortcuts. `j` moves down (next),
and `k` moves up (previous), following the global nvim convention.

`/` does **not** open the Hub's filter overlay: the board owns `BoardState.filter`,
and while its input has the keyboard the screen publishes the `Filter` key context
instead of `Hub > Board`. Every row below is therefore shadowed while you are typing
a filter, and the `Filter` rows above apply instead — including their two-stage `Esc`.

The board dialog rows below are **in addition to** everything the generic Dialog
context binds: enter, tab, ctrl-n / ctrl-p, backspace, ctrl-w, ctrl-u, ctrl-a,
ctrl-e, left and right. A bare letter bound in the board settings dialog types
itself when a text row owns the keyboard, exactly as in §3.8.6.

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
| `escape` | `Dialog > CardDetail` | `card_detail::Close` — Close |
| `i` | `Dialog > CardDetail` | `card_detail::EditTitle` — Edit title |
| `d` | `Dialog > CardDetail` | `card_detail::EditDescription` — Edit description |
| `c` | `Dialog > CardDetail` | `card_detail::AddComment` — Add comment |
| `j` | `Dialog > CardDetail` | `card_detail::NextProperty` — Next property |
| `k` | `Dialog > CardDetail` | `card_detail::PrevProperty` — Previous property |
| `enter` | `Dialog > CardDetail` | `card_detail::EditProperty` — Edit selected property |
| `w` | `Dialog > CardDetail` | `card_detail::CreateWorktree` — Create worktree |
| `x` | `Dialog > CardDetail` | `card_detail::OpenRemote` — Open the remote issue |
| `K` | `Dialog > CardDetail` | `card_detail::KeepLocal` — Resolve conflict: keep local |
| `R` | `Dialog > CardDetail` | `card_detail::TakeRemote` — Resolve conflict: take remote |
| `ctrl-s` | `Dialog > CardDetail` | `card_detail::Save` — Save text edit |
| `:` | `Dialog > CardDetail` | `OpenPalette` — Command palette over the open card detail |
| `ctrl-enter` | `Dialog > CardCreate` | `board::CreateAndOpen` — Create and open |
| `space` | `Dialog > CardPicker` | `settings::Toggle` — Toggle the highlighted label |
| `j` | `Dialog > BoardSettings` | `settings::MoveDown` — Next row |
| `k` | `Dialog > BoardSettings` | `settings::MoveUp` — Previous row |
| `h` | `Dialog > BoardSettings` | `settings::CyclePrev` — Previous choice |
| `l` | `Dialog > BoardSettings` | `settings::CycleNext` — Next choice |
| `space` | `Dialog > BoardSettings` | `settings::Toggle` — Toggle the row |

Board filtering also keeps horizontal navigation:

| Key | Context | Action |
| --- | --- | --- |
| `left` | `Filter > BoardFilter` | `board::PrevColumn` — Previous column |
| `ctrl-b` | `Filter > BoardFilter` | `board::PrevColumn` — Previous column |
| `right` | `Filter > BoardFilter` | `board::NextColumn` — Next column |
| `ctrl-f` | `Filter > BoardFilter` | `board::NextColumn` — Next column |

In card text editors, Tab indents by two spaces. In CardCreate, Tab from the title enters
its description; Shift-Tab returns to the title, Enter in the description inserts a newline,
and Ctrl-Enter creates and opens the card. Escape in the property picker returns to the
card detail when opened there. Space toggles both labels and custom multi-select options.
