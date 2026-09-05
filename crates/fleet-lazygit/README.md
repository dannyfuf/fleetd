# fleet-lazygit

A native [lazygit](https://github.com/jesseduffield/lazygit) clone: `fleet-git` for the plumbing,
`fleet-ui-kit` for the design system, gpui for the window.

```sh
cargo build --release -p fleet-lazygit
./target/release/fleet-lazygit .          # or any path inside a work tree
```

With no argument it opens the current directory. `$RUST_LOG` (default `info`) goes to stderr.

## Layout

```
┌── 33 % ──────────────┬── 67 % ─────────────────────────────┐
│ [1] Status           │ [0] Diff / Patch / Log / Conflicts  │
│ [2] Files            │   (splits into unstaged | staged    │
│ [3] Local branches / │    when a file has both halves)     │
│     Remotes / Tags   ├─────────────────────────────────────┤
│ [4] Commits / Reflog │ Command log                         │
│ [5] Stash            │                                     │
├──────────────────────┴─────────────────────────────────────┤
│ key hints …                              MODE   version    │
└────────────────────────────────────────────────────────────┘
```

The focused pane gets the accent frame; its cursor row keeps the selection background when focus
moves away and loses the accent bar, which is lazygit's `HighlightInactive` behaviour. `+` and `_`
cycle the screen modes (normal → half → full, wrapping): half shows only the focused side pane
beside the main panel, full shows only the focused pane.

## The diff view

Every patch the app shows — a working-tree file, both halves of staging mode, a commit, a
commit's file, a sub-commit, a stash, a branch's log, a directory's combined diff — goes through
one renderer.

- **A file-header card per file**, two rows tall: status glyph and path on the first, the
  `+ n` / `− m` diffstat, the detected language and any mode change on the second. Two rows
  because `uniform_list` extrapolates from one measured row and a taller card would corrupt the
  scroll position — the same reason Zed spells its own file-header height in rows.
- **Syntax highlighting** from `syntect` with `two-face`'s grammar set (213 languages, TypeScript
  and TSX included). A hunk's `-` and `+` lines are two different versions of the file, so each
  hunk is parsed twice, once per side. The whole pass runs on the background executor and the
  rows repaint when it lands, so a 22 000-line diff never blocks a frame.
- **Word-level marks**: removed and added runs inside a hunk are paired by index, compared with
  `similar` on a code-aware tokenizer, gated on a 0.5 similarity ratio, and two marks separated by
  a single character are joined into one.
- **Tints derived from the theme**, not from new tokens: the row wash is the ANSI hue this diff
  colour already names, composited over the background at Zed's filled-hunk alpha (0.12 dark /
  0.16 light), with a lighter step for the gutters and a stronger one for changed words.
- **A selection composites over the tint** rather than replacing it, so a selected added line
  still reads as added, and every selected row carries the 2 px cursor bar.
- **Mouse wheel, always.** The wheel scrolls the diff whether or not the main panel holds the
  keyboard, in every content the panel can show, and a sideways delta — a two-finger swipe, or
  `shift` plus a wheel where the platform swaps the axis — pans instead. A 5 px scrollbar on the
  right edge shows the position. `j` / `k` / `ctrl-d` / `ctrl-u` / `<` / `>` still move the cursor, and neither the
  scroll offset nor the cursor is disturbed by the periodic refresh.
- **`|` toggles side-by-side.** Unified is the default, because lazygit is a keyboard-driven
  staging tool and one cursor over one row sequence is what the staging selection models. Split
  aligns the two columns with hatched filler rows and shows one line-number gutter per column.
  Staging mode always renders unified whatever the toggle says.
- **`{` / `}` change the context width** (lazygit's own keys). The width lives on the
  `Repository`, so the patch a staging selection builds is read back with the same `-U` it was
  displayed with. The `} more context` affordance on each `@@` separator fires the same action.
- **No wrap mode.** Wrapping needs variable row heights, which `uniform_list` cannot express;
  `gpui::list` can, but only by estimating total content height, which costs an exact scrollbar
  and an exact `scroll_to_item`. Horizontal panning is the trade this view makes instead.

## Architecture

| File | Purpose |
| --- | --- |
| `src/main.rs` | `maybe_run_from_env()` first (Git re-invokes this binary as `GIT_SEQUENCE_EDITOR`), then the path argument, then `run`. |
| `src/lib.rs` | gpui boot: assets, theme, keymap, window, the optional drive script. |
| `src/bridge.rs` | One background thread with a Tokio runtime owning the `Repository` and its `RepoWatcher`; `GitRequest` in, `GitEvent` out over `async_channel`. |
| `src/state.rs` | `GitUiState`: the `Arc<RepoSnapshot>`, every cursor, the overlay stack, the command log, plus the pure reducers and their unit tests. |
| `src/root.rs` | The one gpui view: focus routing, the key-context chain, every action handler. |
| `src/keymap.rs` | The declarative key table; also feeds the `?` help and the bottom hint bar. |
| `src/actions.rs` | One `gpui::actions!` namespace per key context. |
| `src/panels/` | The five side panes, the banner, the bottom bar, the main panel and the command log. |
| `src/overlays.rs` | Confirmations, prompts, menus and the generated help. |
| `src/views/` | Pure presentation, one function per row type. |
| `src/views/diff_model.rs` | The cached flattened diff: one uniform-height row per rendered line, the split layout that indexes those rows, per-file metadata (counts, language, mode) and the slot the background syntax pass fills. Keyed by `ModelKey` (`Arc<Diff>` pointer + shape + view mode), so it is rebuilt only when the patch really changes. |
| `src/views/diff.rs` | The renderer: file-header cards, `@@` separators, tinted rows, gutters, the position scrollbar and the `uniform_list` that virtualises them. |
| `src/views/syntax.rs` | `syntect` + `two-face` TextMate highlighting, per-hunk and per-side, yielding colour *buckets* rather than colours so the work can run off the foreground thread. |
| `src/views/intraline.rs` | Change-block pairing inside a hunk and `similar` word-level marks, with the ratio gate, the size caps and Pierre's join-single-separator rule. |
| `src/views/file_tree.rs` | lazygit's Files tree model (sort, path compression, collapse set, flat/tree flag). |
| `src/drive.rs` | Developer-only scripted input (`FLEET_LAZYGIT_DRIVE`). |

Rules the crate follows: no `git` call ever touches the foreground thread; every mutation is
followed by a snapshot, successful or not; a snapshot whose `generation` is not newer than the one
on screen is dropped; a refresh never moves the cursor, the scroll or the focus; **nothing fetches
on a timer** — the network runs only on `f`, `p` and `P`.

## Key map

Bindings are declared once in `src/keymap.rs`; the `?` overlay and the bottom hint bar are
generated from that table, so this section is documentation, not a second source of truth.

### Everywhere (context `Lazygit > Panels`)

| Keys | Action |
| --- | --- |
| `j` `k` `↓` `↑` | move down / up |
| `<` `home` / `>` `G` `end` | first / last item |
| `g g` | first item (only where `g` is free: Status, Files, Branches, Remotes, Tags, Main) |
| `ctrl-d` `.` / `ctrl-u` `,` | half page down / up (half of the pane's own measured height) |
| `H` / `L` | pan the diff payload left / right (clamped to the longest line) |
| `{` / `}` | fewer / more context lines around every hunk (re-runs `git diff -U<n>`) |
| `\|` | switch the diff between unified and side by side |
| `1`…`5` | focus Status, Files, Branches, Commits, Stash |
| `0` | focus the main panel |
| `tab` / `shift-tab` | next / previous side pane |
| `]` `l` / `[` `h` | next / previous tab of the focused pane |
| `q` | quit (asks first when an operation is in progress) |
| `R` | refresh — never fetches |
| `?` | keybinding help for the current context |
| `+` / `_` | next / previous screen mode |
| `@` | show or hide the command log |
| `esc` | leave the main panel, drop a remote drill-down, clear the error |
| `p` / `P` / `f` | pull / push / fetch |
| `m` | merge / rebase options (continue, skip, abort) |

### Files (`Lazygit > Panels > Files`)

The pane is lazygit's **file tree** by default (`gui.showFileTree`): directories first then files,
both alphabetical, two spaces of indent per level, `▼` / `▶` on a directory, and a single-child
directory chain compressed into one row (`src/app/deep`) instead of three. A directory row is
coloured by the aggregate of everything under it — green fully staged, yellow mixed — and every
key below that acts on "the selected file" acts on **all** of a directory's files instead. The
pane's counter stays the number of *files*, never the number of rows.

| Keys | Action |
| --- | --- |
| `space` | stage or unstage the selected file, or every file under the selected directory |
| `a` | stage everything, or unstage everything when nothing is unstaged |
| `d` | discard the file's changes, or the whole directory's (confirm) |
| `c` | commit the index (multi-line prompt, `⌘⏎` submits) |
| `A` | amend the last commit with the index (confirm) |
| `S` | stash options: all / staged / including untracked |
| `` ` `` `~` | toggle the tree and flat layouts |
| `-` / `=` | collapse / expand every directory |
| `enter` | on a file: staging mode, or the conflict view; on a directory: collapse or expand it |

A collapsed directory is remembered by path, so a refresh never re-opens it, and collapsing the
directory the cursor was inside leaves the cursor on that directory. With a directory selected the
main panel shows the combined patch of its files (`Repository::diff_paths`).

### Staging mode (`Lazygit > Panels > Staging`)

| Keys | Action |
| --- | --- |
| `space` | stage the selection (unstage it on the staged side) |
| `d` | discard the selection (confirm), or unstage it on the staged side |
| `a` | switch between hunk and line selection |
| `v` | start or clear a range selection |
| `tab` | switch between the unstaged and staged halves |
| `h` `←` / `l` `→` | previous / next hunk |
| `esc` | back to the Files pane |

### Conflict view (`Lazygit > Panels > Conflict`)

`o` keep ours · `t` keep theirs · `b` keep both · `h` / `l` previous / next section · `esc` back.

### Local branches (`Lazygit > Panels > Branches`)

| Keys | Action |
| --- | --- |
| `space` | check out |
| `n` | new branch (prompt) |
| `d` | delete — one confirm runs `git branch -d`; only if Git refuses does a second dialog offer `-D` |
| `R` | rename (prompt) |
| `M` | merge the selected branch into the current one (confirm) |
| `r` | rebase the current branch onto the selected one (confirm) |
| `u` | upstream options: set (prompt) / unset |
| `T` | tag the branch (prompt) |
| `enter` | open the branch's commits in the main panel (lazygit's sub-commits view) |

Local branches are ordered as lazygit orders them: the checked-out branch first (recency `*`), then
every branch the HEAD reflog records a checkout **away from**, most recent first, then the rest by
`-committerdate`. The recency column is that checkout time, not the tip's committer date.

### Remotes (`Lazygit > Panels > Remotes`)

`enter` list a remote's branches, or open a branch's commits (sub-commits) · `space` check out a
local tracking branch · `esc` leave the remote.

### Tags (`Lazygit > Panels > Tags`)

`n` new tag at HEAD (prompt) · `d` delete (confirm) · `space` check out (detached).

### Commits (`Lazygit > Panels > Commits`)

| Keys | Action |
| --- | --- |
| `enter` | open the commit's file list in the main panel (lazygit's commit-files view) |
| `space` | check out the commit (detached, confirm) |
| `r` | reword (prompt) |
| `s` / `f` | squash / fixup into the commit below (confirm) |
| `d` | drop (confirm) |
| `e` | start an interactive rebase that stops here |
| `ctrl-j` / `ctrl-k` | move the commit down / up |
| `g` | reset options: soft / mixed / hard (confirm) |
| `c` / `v` | copy (mark) / paste (cherry-pick the copied commits, confirm) |
| `t` | revert (confirm) |
| `T` | tag the commit (prompt) |
| `A` | amend with the staged changes (confirm) |
| `n` | new branch at the commit (prompt) |

### Reflog (`Lazygit > Panels > Reflog`)

`enter` open the commit's file list · `space` check out · `c` / `v` copy / paste · `g` reset
options.

### Sub-commits (`Lazygit > Panels > Main > SubCommits`)

A branch's own log, opened with `enter` from Branches or Remotes.

| Keys | Action |
| --- | --- |
| `j` `k` `<` `>` `ctrl-d` `ctrl-u` | move over the commits |
| `enter` / `space` | show the selected commit's patch under the list |
| `c` / `v` | copy (mark) / paste (cherry-pick onto the current branch) |
| `esc` | back to the pane it was opened from |

### Commit files (`Lazygit > Panels > Main > CommitFiles`)

A commit's changed files, opened with `enter` from Commits, the Reflog or a sub-commit.

| Keys | Action |
| --- | --- |
| `j` `k` `<` `>` `ctrl-d` `ctrl-u` | move over the header row and the files |
| `enter` / `space` | re-read the selected row's patch |
| `esc` | back to the pane it was opened from |

Row 1 is the commit itself and shows the **whole** patch; every later row shows that one file's
patch, so `<` is how the whole-commit view stays one keystroke away.

### Stash (`Lazygit > Panels > Stash`)

`space` apply · `g` pop · `d` drop (confirm) · `n` branch from the entry (prompt) · `enter` diff.

### Overlays

| Context | Keys |
| --- | --- |
| `Dialog > Confirm` | `enter` / `y` confirm · `esc` / `n` cancel |
| `Dialog > Prompt` | printable characters type · `enter` confirms (inserts a newline in a multi-line prompt) · `⌘⏎` / `ctrl-enter` always submits · `esc` cancels · `backspace`, `ctrl-w`, `ctrl-u`, `←`/`→`, `ctrl-a`/`ctrl-e`, `⌘v`/`ctrl-v` edit |
| `Dialog > Menu` | `enter` run · the letter printed beside a row runs that row · `j`/`k` move · `/` filter · `esc` close |
| `Dialog > MenuFilter` | printable characters filter · `ctrl-n`/`ctrl-p` or `↓`/`↑` move · `enter` run · `esc` clears the filter |
| `Dialog > Help` | `j`/`k` scroll (clamped at the last page) · `esc` / `q` / `?` close |

**A context that hosts a text field binds no single-character key** — that rule is enforced by a
test in `src/keymap.rs`, because gpui dispatches key bindings before any key listener, so a bare
letter bound there would run a command instead of typing.

## Deviations from lazygit

- `q` quits (lazygit's default) rather than closing an overlay; `esc` closes overlays and never
  quits.
- `<` / `>` / `home` / `end` are the primary "go to top/bottom" keys; `g g` exists only in the
  panes where lazygit leaves `g` unbound.
- The Remotes tab is its own key context rather than a sub-context of Branches, so `d` cannot mean
  "delete branch" while a remote is selected.
- Green shas mean "reachable from a main branch" (`main`, `master`, `develop`, `trunk`, or their
  `origin/` counterparts) **and** pushed; lazygit asks git the same question with `rev-list`.
- The graph column is one glyph per commit (`◎` merge, `○` otherwise), not lazygit's pipe layout.
- The sub-commits and commit-files views live in the **main panel**, split above the patch they
  select. lazygit borrows the branches and commits windows for them and keeps the patch in the main
  panel; the split keeps the list and its patch in one place instead.
- In the commit-files view `enter` re-reads the selected row's patch (the patch already follows the
  cursor) rather than lazygit's "enter the file to build a custom patch", which this crate does not
  implement. Its extra header row is what keeps the whole-commit patch reachable.
- The sub-commits view binds `enter`/`space` to "show this commit's patch". lazygit uses `enter`
  for "view files" and `space` for "checkout".
- The file tree draws **no root `/` row**, although lazygit's `showRootItemInFileTree` defaults to
  `true`: every row of the Files pane stays something the keys can act on.
- The file tree sorts directories before files. lazygit's `fileTreeSortOrder` defaults to `mixed`,
  which interleaves them alphabetically; `foldersFirst` is the order this pane always uses.
- A directory row prints only its arrow and its name, as lazygit does — the aggregated status is
  carried by the name colour. `FileRow::status` computes the two characters anyway, for tests.

## Development

```sh
cargo build -p fleet-lazygit
cargo test -p fleet-lazygit
cargo fmt -p fleet-lazygit --check
cargo clippy -p fleet-lazygit --all-targets -- -D warnings
```

`fleet-lazygit` does not depend on `fleet-term`, so building it alone does not need Zig.

### Driving the GUI from a script

Like `fleet-app`, the binary ships a developer-only scripted-input driver, enabled only when
`FLEET_LAZYGIT_DRIVE` names a file:

```sh
: > /tmp/lazygit-drive.txt
FLEET_LAZYGIT_DRIVE=/tmp/lazygit-drive.txt ./target/release/fleet-lazygit /path/to/repo &
printf 'wait 500\nkey 2\nkey enter\nkey space\nshot /tmp/staged.png\nquit\n' \
  >> /tmp/lazygit-drive.txt
```

`key`, `type`, `wait`, `shot` and `quit` behave exactly as `docs/DEVELOPMENT.md` documents for
`FLEET_DRIVE`; every line is echoed to `$FLEET_LAZYGIT_DRIVE.log` with a `run` and a `done` entry,
so a runner can wait on `done <line>` instead of sleeping. One step is local to this crate:

```text
wheel 5               scroll the pointer wheel five rows down (negative scrolls up)
wheel 5 0.75 0.5      the same, aimed at 75 % across and 50 % down the window
hwheel 4              scroll four cells to the right (a sideways trackpad swipe)
```

Both dispatch a real `ScrollWheelEvent` through the window's hit test, so they exercise the
element's own listener rather than a test-only shortcut.

### The `drive/` regression scripts

`drive/` holds one drive script per user flow, each with a comment stating the git state it should
leave behind, plus the three shell helpers that make them runnable:

| File | Purpose |
| --- | --- |
| `drive/make-repo.sh <dir> [dirty\|clean\|conflict\|merging\|reflog\|tree\|ts]` | Builds the throwaway repository every script assumes: a bare `origin`, `main` (pushed) and `feature`, four commits, a tag, a stash, and a dirty/staged/untracked working tree. The modes add a clean tree, a conflicting `main` commit, a stopped merge, a lost commit in the reflog, nested dirty directories for the file tree, or committed-then-edited TypeScript sources plus a long file for the diff view. |
| `drive/make-origin-ahead.sh <dir>` | Pushes one commit straight to the bare `origin`, so `f` has something to fetch and `p` something to pull. |
| `drive/run-drive.sh <repo> <script> [seconds]` | Launches the release binary on `<repo>` with the script wired to `FLEET_LAZYGIT_DRIVE` and waits for `done quit`. `$SCRATCH` chooses where the drive file, its log and the app's stderr land. |
| `drive/run-suite.sh [name-filter …]` | Runs the whole suite. One manifest row per script names the `make-repo.sh` mode it needs, the shell that prepares the tree and the shell that asserts the result; a row also fails on a missing `done quit`, an `unhandled key` or a panic. |

```sh
cargo build --release -p fleet-lazygit
crates/fleet-lazygit/drive/run-suite.sh            # the whole suite
crates/fleet-lazygit/drive/run-suite.sh staging-   # one family
```

```sh
cargo build --release -p fleet-lazygit
crates/fleet-lazygit/drive/make-repo.sh /tmp/verify clean
crates/fleet-lazygit/drive/run-drive.sh /tmp/verify/work \
  crates/fleet-lazygit/drive/commits-squash.txt
git -C /tmp/verify/work log --oneline        # the assertion the script's comment names
```

The scripts navigate by absolute motions (`<`, `>`, counted `j`) and never by a remembered cursor,
so they stay valid as long as `make-repo.sh` keeps building the same repository. Row numbers in
the comments are 1-based and follow the order the pane renders: **local branches by checkout
recency with the current branch first**, remote branches by `-committerdate`, tags by
`-creatordate`, commits and the reflog newest first, files alphabetically.
