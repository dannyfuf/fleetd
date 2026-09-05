# Fleet keymap

Principles: nvim-inspired, modal, discoverable. Every binding here is authoritative for the
app; `fleet-app` registers exactly these via gpui key contexts. Lowercase = safe action,
uppercase = stronger variant. `ctrl-c` never quits the app (it belongs to terminals); quitting
is `ctrl-q` (with a confirm only if a job is running and the user asked to be warned).
`Esc` never quits. `q` never quits: it closes the topmost overlay, and where nothing is open it
is unbound. Over a terminal grid no bare key is ever an app affordance — every Workspace
affordance is written and bound with its `ctrl-s` prefix.

This file is the single source of truth for keys. `docs/UX-SPEC.md` describes screens and cites
this file; where the two disagree, this file wins.

## Modes and key contexts

| Mode | gpui key context | Entered by | Left by |
| --- | --- | --- | --- |
| Normal | `Hub` / `Hub > Repos` / `Hub > Worktrees` / `Hub > Prs` | app start, `ctrl-s s` from a terminal, `Esc` from dialogs | opening a session |
| Terminal | `Workspace > Terminal` | opening a worktree/agent session, `Enter` on a session tab | `ctrl-s` (prefix) |
| Native | `Workspace > Native` | selecting a tab whose configured command is a `fleet://` surface (the default third tab, `lg`) | `ctrl-s` (prefix), or selecting a PTY tab |
| Prefix | `Workspace > Prefix` (one-shot) | `ctrl-s` inside Terminal or Native | any key (consumed) or `Esc` |
| Scroll | `Workspace > Scroll` | `ctrl-s [` | `Esc`, `q`, `i` |
| Filter | `Filter` | `/` in a list | `Esc` (first keeps filter, second clears), `Enter` |
| Palette | `Palette` | `:` | `Esc`, `Enter` |
| Dialog | `Dialog > <name>` | action | `Esc`, `Enter` |
| Jobs | `Jobs` / `Jobs > Log` (overlay) | `J` anywhere in Normal, `ctrl-s J` in a terminal | `Esc`, `J`, `q` |
| Daemon | `Daemon > Down` / `Daemon > Banner` / `Daemon > Doctor` | fleetd will not start (§3.12 B), fleetd died while attached (§3.12 C), doctor runs | daemon comes back, `Esc` (banner/doctor), `ctrl-q` |
| FirstRun | `FirstRun` | no contexts and no repos exist | any key that creates or imports something |

The context stack is ordered: `Dialog` / `Palette` / `Jobs` / `Filter` shadow `Hub` and
`Workspace`; `Daemon > Down` and `FirstRun` are full-window and shadow everything except
`ctrl-q`.

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
| `a` / `A` | open the Claude / OpenCode agent session [A21] |
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
| `x` | prune eligible worktrees of selected repo (dry-run preview → confirm) |
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

All keys go to the PTY except `ctrl-s`, which enters Prefix for one key:

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
| `a` / `A` | open the Claude / OpenCode agent session |
| `z` | zoom: hide the session header and tab strip (toggle) |
| `!` | focus the sticky error slot [A18] |
| `J` | jobs panel |
| `?` | help overlay listing this table |
| `Esc` | cancel prefix |

**Caution [A5].** `ctrl-s s` (go to Hub, benign) and `ctrl-s S` (sleep this session, then Hub —
kills nothing but stops the terminals) are one shift apart. This is the riskiest adjacency in
the map; it is allowed only because it follows the uppercase-is-stronger rule and because sleep
is reversible.

**No bare keys over a terminal.** `!` is `ctrl-s !` here, never bare — the Workspace has exactly
one escape key, `ctrl-s`. Affordances drawn over the grid are written `^s r`, `^s l`, `^s ⏎`.

## Scroll mode (inside a terminal)

| Key | Action |
| --- | --- |
| `j` / `k`, `ctrl-d` / `ctrl-u`, `ctrl-f` / `ctrl-b`, `gg` / `G` | move viewport |
| `v` | start selection, `y` yank selection, `Esc` clear |
| `/` then text, `n` / `N` | search scrollback (may land after v1; keep the binding reserved) |
| `q`, `i`, `Esc` | back to Terminal mode (viewport snaps to bottom) |

## Workspace (Native mode)

The default third tab (`lg`) is not a terminal: it is Fleet's own git pane
(`crates/fleet-lazygit`) rendered inside the tab. `Workspace > Native` binds exactly what
`Workspace > Terminal` binds — `ctrl-s`, and nothing else — so every other key belongs to the
pane, whose own key table lives in `crates/fleet-lazygit/README.md`. The full context chain is
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
