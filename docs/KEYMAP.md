# Fleet keymap

Principles: nvim-inspired, modal, discoverable. Every binding here is authoritative for the
app; `fleet-app` registers exactly these via gpui key contexts. Lowercase = safe action,
uppercase = stronger variant. `ctrl-c` never quits the app (it belongs to terminals); quitting
is `ctrl-q` (with a confirm only if a job is running and the user asked to be warned).

## Modes and key contexts

| Mode | gpui key context | Entered by | Left by |
| --- | --- | --- | --- |
| Normal | `Hub` / `Hub > Repos` / `Hub > Worktrees` / `Hub > Prs` | app start, `ctrl-s s` from a terminal, `Esc` from dialogs | opening a session |
| Terminal | `Workspace > Terminal` | opening a worktree/agent session, `Enter` on a session tab | `ctrl-s` (prefix) |
| Prefix | `Workspace > Prefix` (one-shot) | `ctrl-s` inside Terminal | any key (consumed) or `Esc` |
| Scroll | `Workspace > Scroll` | `ctrl-s [` | `Esc`, `q`, `i` |
| Filter | `Filter` | `/` in a list | `Esc` (first keeps filter, second clears), `Enter` |
| Palette | `Palette` | `:` | `Esc`, `Enter` |
| Dialog | `Dialog > <name>` | action | `Esc`, `Enter` |
| Jobs | `Jobs` (overlay) | `J` anywhere in Normal, `ctrl-s J` in a terminal | `Esc`, `J` |

## Global (Normal mode, all Hub screens)

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | move cursor |
| `gg` / `G` | first / last row |
| `ctrl-d` / `ctrl-u` | half page down / up |
| `h` / `l`, `←` / `→`, `S-Tab` / `Tab` | focus previous / next pane (repos ⇄ worktrees ⇄ detail) |
| `1`–`9`, `gt` / `gT` | switch to nth / next / previous context |
| `p` | toggle Worktrees ⇄ Pull requests screen |
| `/` | filter current list |
| `:` | command palette |
| `,` | settings |
| `?` | help |
| `J` | jobs panel |
| `r` | refresh (status, PRs, discovery) — runs as a job, never blocks |
| `U` | update Fleet (job) |
| `N` / `D` | new context / delete active context (confirm) |
| `y` | copy path (worktree) or URL (PR) |
| `b` | open PR / repo in browser |
| `Esc` | clear filter if any, else close overlay, else no-op |
| `ctrl-q` | quit app (daemon keeps running) |
| `ctrl-shift-q` | quit app and stop daemon (confirm; lists running jobs/sessions) |

## Hub › Repos pane

| Key | Action |
| --- | --- |
| `Enter`, `o`, `l` | select repo → focus worktrees |
| `n` | clone repo (dialog with fuzzy GitHub search) |
| `d` | delete repo (confirm, cascades) |
| `m` | move repo to another context (assign dialog) |
| `i` | toggle detail panel |

## Hub › Worktrees pane

| Key | Action |
| --- | --- |
| `Enter`, `o` | open session (sleeps previous session) → Workspace |
| `O` | open session keeping previous awake |
| `n` | create worktree (dialog) |
| `d` | delete worktree (confirm shows dirty/unique-commit/session facts from inspect) |
| `x` | prune eligible worktrees of selected repo (dry-run preview → confirm) |
| `s` | sleep session |
| `K` | kill session (confirm) |
| `I` | inspect (refresh safety facts as a job) |
| `i` | toggle detail panel |

## Hub › Pull requests screen

| Key | Action |
| --- | --- |
| `Tab` / `S-Tab`, `l` / `h` | Mine ⇄ Review tabs |
| `Enter`, `o` / `O` | open or create the PR worktree (sleeping / keeping previous) |
| `b` / `y` | open in browser / copy URL |
| `r` | force refresh both tabs |
| `p`, `q` | back to worktrees |

## Workspace (Terminal mode)

All keys go to the PTY except `ctrl-s`, which enters Prefix for one key:

| After `ctrl-s` | Action |
| --- | --- |
| `ctrl-s` | send a literal `ctrl-s` to the terminal |
| `s` | go to Hub (session keeps running) |
| `1`–`9` | switch to nth terminal tab |
| `h` / `l`, `p` / `n` | previous / next terminal tab |
| `c` | new terminal tab (shell in worktree path) |
| `x` | close current terminal (confirm if a keep-alive process is running) |
| `,` | rename current terminal |
| `[` | Scroll mode |
| `]` | paste clipboard (bracketed when the app requests it) |
| `a` / `A` | open the Claude / OpenCode agent session |
| `z` | zoom: hide the session header and tab strip (toggle) |
| `J` | jobs panel |
| `?` | help overlay listing this table |
| `Esc` | cancel prefix |

## Scroll mode (inside a terminal)

| Key | Action |
| --- | --- |
| `j` / `k`, `ctrl-d` / `ctrl-u`, `ctrl-f` / `ctrl-b`, `gg` / `G` | move viewport |
| `v` | start selection, `y` yank selection, `Esc` clear |
| `/` then text, `n` / `N` | search scrollback (may land after v1; keep the binding reserved) |
| `q`, `i`, `Esc` | back to Terminal mode (viewport snaps to bottom) |

## Dialogs and text inputs

Text inputs accept printable keys, `Backspace`, `ctrl-w` (delete word), `ctrl-u` (clear),
`ctrl-a` / `ctrl-e` (home / end), `←` / `→`. Lists under a text input use `ctrl-n` / `ctrl-p`
or `↓` / `↑` (never `j` / `k`, because the text field owns them). `Tab` / `S-Tab` move between
fields. `Enter` confirms; `Esc` cancels. Confirm dialogs: `y` / `Enter` confirm, `n` / `Esc` cancel.
Settings: `Space` toggles, `h` / `l` cycle a choice, `Enter` saves.
