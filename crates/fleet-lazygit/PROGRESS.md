# fleet-lazygit progress

A running log so an interrupted session can be resumed from disk. Newest entry last.

## 1. Scaffold and boot — done

`Cargo.toml` (added to the workspace `members`), `src/main.rs` (sequence-editor short-circuit →
arg parse → `run`), `src/lib.rs` (gpui boot: `gpui_platform::application().with_assets(KitAssets)`,
`Theme::init`, `keymap::init`, `on_window_closed → quit`, window options). The window uses an
**opaque** titlebar, unlike `fleet-app`: with a transparent one the traffic lights sit exactly on
top of the `[1] Status` pane title.

## 2. Bridge and snapshot — done

`src/bridge.rs`: one background thread with a multi-threaded Tokio runtime owning the
`Repository` and its `RepoWatcher`; `GitRequest`/`GitEvent` over two `async_channel`s;
`Mutation` is one enum covering every write. Every mutation is followed by a snapshot, success or
failure. Command-log broadcast lag re-seeds from `recent_commands()`.

## 3. Layout, panes and navigation — done

`src/state.rs` (`GitUiState` + pure reducers + unit tests), `src/panels/mod.rs` (five side panes,
banner, bottom bar), `src/panels/main_panel.rs` (main panel, split diff, command log),
`src/views/` (diff flattening + row renderers), `src/keymap.rs` + `src/actions.rs`.
Key contexts: `Lazygit > Panels > <Panel>` for the panels, `Lazygit > Dialog > <Kind>` for
overlays; an overlay replaces the whole chain, so a prompt swallows the keymap.

## 4. Files and staging — done

Two independently coloured status characters, stage/unstage/stage-all, discard confirmation,
commit prompt, amend, stash menu, and staging mode over `apply_patch_selection` with hunk mode,
line mode (`a`) and range select (`v`). Verified by hand against a throwaway repo: staging a hunk
and staging a single line both produce the right index.

## 5. Branches, remotes, tags — done

Local branches with recency and `↓n↑m`; checkout, new, delete (two-step force escalation),
rename, merge, rebase, upstream menu, tag. Remotes tab drills into a remote's branches and can
check out a tracking branch. Tags tab creates, deletes and checks out.

## 6. Commits and rebase — done

Commit rows with the red/yellow/green sha, `◎`/`○` glyph, author initials, age, decorations.
`enter` shows the patch; reword/squash/fixup/drop/edit/move go through `interactive_rebase` with
`std::env::current_exe()` as the sequence-editor helper — verified end to end (a reword that
conflicts during replay correctly leaves the app in `REBASING` with the stderr in the sticky
error slot, and `m → abort` cleans up). Reflog tab with checkout/copy/reset.

## 7. Stash — done

Apply, pop, drop (confirm), branch-from-stash, and the entry's diff in the main panel.

## 8. Dialogs and help — done

`src/overlays.rs`: confirm (`ConfirmDialog`), prompt (single- and multi-line with a real editable
`Buffer`, `⌘⏎` to submit a message), menu (`FuzzyList`, `/` filters), and the `?` help generated
from `keymap::table()`.

## 9. Watcher, refresh, command log — done

`RepoWatcher` events request one snapshot each (fleet-git already debounces); a 2 s / 5 s
periodic fallback covers watcher gaps; snapshots are epoch-checked against
`RepoSnapshot::generation`; cursors follow their item with `ListCursor::retain`. The command log
is a local non-wrapping row list — the kit's `LogView` uses `uniform_list`, whose uniform-height
assumption a wrapped `git` argv breaks.

## 10. Polish — done

Screen modes (`+`/`_`), conflict view (`o`/`t`/`b`), the operation banner and mode word, toasts,
the sticky error slot, and a `FLEET_LAZYGIT_DRIVE` scripted-input driver (`src/drive.rs`) ported
from `fleet-app` so the GUI can be exercised without Accessibility permission.

## 11. Verification pass — done

Every flow the earlier sections implemented but nobody had exercised was driven headlessly with
`FLEET_LAZYGIT_DRIVE` against throwaway repositories and asserted with `git` afterwards. The
scripts live in `drive/` (one per flow, each naming the git state it should leave) together with
`make-repo.sh`, `make-origin-ahead.sh` and `run-drive.sh`; 57 of them run green.

### Branches, remotes, tags

| Flow | Result |
| --- | --- |
| `f` fetch, `p` pull with origin ahead | verified — `branches-fetch-pull.txt` |
| `P` push with an upstream | verified — `branches-push.txt` |
| `d` delete a merged branch | **fixed** — the first confirm escalated instead of deleting, so no safe `git branch -d` path existed |
| `d` delete an unmerged branch (force) | **fixed** — the escalation now appears only after Git refuses; `branches-delete-force.txt` |
| `R` rename | verified — `branches-rename.txt` |
| `M` merge, fast-forward and real merge commit | verified — `branches-merge.txt` |
| `r` rebase onto the selected branch, clean tree | verified — `branches-rebase.txt` |
| `r` rebase onto the selected branch, dirty tree | **fixed** — `rebase_onto` lacked `--autostash`, so any uncommitted change aborted the rebase; `branches-rebase-dirty.txt` |
| `r` rebase that conflicts, then `m` → abort | verified — `branches-rebase-conflict-abort.txt` |
| `r` rebase that conflicts, then `t` and `m` → continue | verified — `branches-rebase-conflict-continue.txt` |
| `u` set / unset upstream | verified — `branches-upstream-{set,unset}.txt` |
| Remotes tab: `enter` drill-down, `space` checkout | verified — `remotes-checkout.txt` |
| Tags tab: `n` create, `d` delete, `space` checkout | verified — `tags-{create,delete,checkout}.txt` |
| `enter` on a branch | verified — it renders the branch's diff, not its commits (documented deviation) |

### Commits and reflog

| Flow | Result |
| --- | --- |
| `s` squash, `f` fixup, `d` drop | verified — `commits-{squash,fixup,drop}.txt` |
| `e` edit, stopping the rebase, then `m` → continue | verified — `commits-edit-{stop,continue}.txt` |
| `ctrl-j` / `ctrl-k` move | verified — `commits-move.txt` |
| `g` reset soft / mixed / hard | verified — `commits-reset-{soft,mixed,hard}.txt` |
| `t` revert | verified — `commits-revert.txt` |
| `c` copy + `v` paste, one commit and two in order | verified — `commits-copy-paste-{one,two}.txt` |
| `v` paste that conflicts, then `m` → abort | verified — `commits-copy-paste-conflict.txt` |
| `T` tag, `n` new branch at a commit | verified — `commits-{tag,new-branch}.txt` |
| `space` detached checkout and back | verified — `commits-checkout-detach.txt` |
| Reflog tab: `space` checkout, `c`/`v` cherry-pick | verified — `reflog-{checkout,cherry-pick}.txt` |
| `enter` patch plus `j`/`ctrl-d`/`>`/`esc` inside it | verified — `commits-enter-diff.txt` |

### Files, staging, conflicts, stash

| Flow | Result |
| --- | --- |
| `A` amend | **fixed** — the UI amended with an empty `-m ""`, which Git aborts, so `A` silently did nothing |
| `a` stage all / unstage all | verified — `files-stage-all{,-unstage-all}.txt` |
| `d` discard a tracked and an untracked file | verified — `files-discard-{tracked,untracked}.txt` |
| `S` stash staged only, and including untracked | verified — `files-stash-{staged,untracked}.txt` |
| staging: stage a hunk, discard a hunk | verified — `staging-{stage,discard}-hunk.txt` |
| staging: unstage a hunk and a single line from the staged side | verified — `staging-unstage-{hunk,line}.txt` |
| staging: the selected side empties | **fixed** — it showed "no changes" over a file that still had staged work; the view now moves to the other half, as lazygit does |
| conflict view `t` theirs and `b` both | verified — `conflict-take-{theirs,both}.txt` |
| stash `space` apply, `g` pop, `d` drop, `n` branch, `enter` diff | verified — `stash-{apply,pop,drop,branch,enter-diff}.txt` |

### Cross-cutting

| Property | Result |
| --- | --- |
| Selection retained across a mutation's refresh | verified — `files-selection-retained.txt` proves the Files cursor still points at the same path after a stage/unstage round trip; an external `git commit` leaves the Commits cursor on the same OID |
| A failing git command shows its stderr and the app keeps working | **fixed** — the slot showed `GitError`'s `Display`, which leads with the argv, so the useful sentence never fitted; failures now report Git's own output and the slot picks the line carrying `fatal:` / `error:` / `CONFLICT` |
| The watcher notices an external file write and an external commit | verified — both appeared within ~1.1 s |
| The periodic refresh steals neither focus nor scroll | verified — two screenshots 7 s apart (three refresh rounds) over a 28 000-line patch are pixel-identical in the main panel |
| `q` quits, and asks first while an operation is in progress | verified — `quit-during-operation.txt` |
| A non-git directory and an empty repository | verified — "not a Git working tree: …" and `main (unborn)` with per-pane empty states; no crash |
| Large diffs and the real workspace repository, read-only | verified — a 28 000-line `git show` renders in 12 ms and keystroke handling never blocked; no repository state was touched |

### Still broken / not addressed

- `h` in staging mode used to land on the *last* line of the previous hunk; it now lands on its
  first line, matching `l`. Cosmetic, fixed in passing.
- `staging_range` and `patch_selection` key hunks by index only, ignoring `DiffRow::file`. Harmless
  today because staging only ever sees a one-file diff, but it would mis-select if a rename ever
  produced two `DiffFile`s.
- A failing **read** decrements the pending-mutation counter (`saturating_sub`), so a read error
  during a mutation can clear the in-progress guard early. Cannot over-count, so left alone.
- The `?` help overlay scrolls past its last row.
- Local branches are ordered by `-committerdate`; lazygit orders them by checkout recency and keeps
  the current branch first.

## Not implemented

Search/filter inside panels (`/` only filters menus), the custom patch
builder (`ctrl-p`), worktrees and submodules tabs, `:` shell commands, undo/redo (`z`/`Z`),
bisect, diff context size (`{`/`}`) and whitespace toggling (`ctrl-w`), external editor and
difftool, and the author-hashed graph colours. The graph column is one glyph per commit rather
than lazygit's pipe layout.

## 12. Polish pass — done

The seven items §11 left open, in the order they were asked for. Every changed or new flow has a
`drive/` script, and `drive/run-suite.sh` now runs the whole suite reproducibly: one manifest row
per script names the `make-repo.sh` mode it needs, the shell that prepares the tree and the shell
that asserts the result, and a row also fails on a missing `done quit`, an `unhandled key` or a
panic. Before this pass the scripts existed but the repository each one wanted lived only in its
comment.

### 1. Menu shortcut letters — done

The letter printed beside a menu row now runs that row, as lazygit's menus do
(`c continue`, `a abort`, the stash letters, the reset letters). `Menu::item_for_key` is the
lookup; `Lazygit::typed` consults it **before** the buffer path, so `j`/`k`/`enter`/`esc`/`/` keep
their bindings and a printable key still types into the filter whenever the filter prompt is open.
Verified by `menu-letter-stash.txt`, `menu-letter-reset.txt` and `menu-filter-then-letter.txt` —
the last one proves the `u` and `a` inside the word `untracked` filter instead of firing rows.

### 2. Sub-commits and commit files — done

Two new `MainContent` variants, two new key contexts (`… > Main > SubCommits`,
`… > Main > CommitFiles`) and two new reads.

- `enter` on a local or remote branch opens the branch's **own log** in the main panel: `j`/`k`
  over the commits, `enter`/`space` shows the selected commit's patch in the lower three fifths,
  `c`/`v` copy and paste (cherry-pick), `esc` back. `selected_oid` follows the sub-commits cursor
  while that view owns the keyboard, so `c`/`v` act on it and not on the Commits pane.
- `enter` on a commit in Commits or the Reflog opens the commit's **file list**: a header
  row carrying the commit, then one row per changed file. The selected row's patch fills the lower
  half and follows the cursor. **The whole-commit patch stays reachable** on the header row, which
  is where the cursor starts and where `<` returns.
- A drill-down survives a refresh: `refresh_main` returns early for both variants, because they are
  views of history that the side panel's selection cannot re-derive.
- Backend: `Repository::commits_for_ref(reference, upstream, limit)` (`git log <ref>`, with
  `Commit::pushed` decided by `<upstream>..<ref>`), factored out of `read_commits` so both share
  `log_commits` and `mark_pushed`. The per-file patch reuses `diff_commit(oid, [path])`.
- Scripts: `branches-enter.txt` (rewritten), `branches-subcommits-cherry-pick.txt`,
  `remotes-enter-subcommits.txt`, `commits-enter-files.txt` (replaces `commits-enter-diff.txt`),
  `commits-files-whole-patch.txt`.

### 3. Local branch ordering — done

`RepoSnapshot` now orders `local_branches` the way lazygit's branch loader does: the checked-out
branch first, then every branch the HEAD reflog records a checkout **away from** (`checkout: moving
from <name> …`, most recent first), then the rest in `-committerdate` order. It reuses the reflog
the same snapshot already read, so it costs no extra `git` call. `Branch::checked_out_at` carries
the matched timestamp and `state::recency` prefers it over the tip's committer date, so the recency
column is checkout recency. `*` still marks the current branch.

This reorders the Branches pane, so every drive script that navigated it by row number was
re-numbered and its comment rewritten (`branches-delete{,-force}`, `branches-merge`,
`branches-rename`, `branches-rebase{,-dirty,-conflict-*}`, `branches-upstream-{set,unset}`,
`commits-checkout-detach`, `error-slot`).

### 4. Read versus mutation bookkeeping — done

`GitEvent::Failed` is now a **mutation** failure only; reads report `GitEvent::ReadFailed`. The
counter `q` asks about (`pending`) is touched by `begin_mutation` / `finish_mutation` alone, reads
are counted in `pending_reads`, and a read failure arms no escalation dialog. Unit test:
`a_failing_read_never_clears_the_mutation_guard`.

### 5. Help clamp and real page sizes — done

`help_last_top` clamps the `?` overlay at `rows − HELP_ROWS`, so `j` stops on the last page instead
of scrolling into a blank dialog (`help-scroll-clamp.txt`). `Lazygit::measure` runs once per frame
and repeats the frame's own arithmetic from `window.viewport_size()` to get each pane's row
capacity, then calls `ListCursor::set_page_from_visible` on every cursor; the staging cursor reads
`page_rows()` too. The hardcoded `10` is gone.

### 6. Selection keyed by (file, hunk) — done

`state::hunk_range` and `state::selection_hunks` replace the hunk-index matching in
`staging_range` / `patch_selection`, and `staging_hunk`'s `h`/`l` search is restricted to the
cursor's file as well. A range that spans two `DiffFile`s now contributes only the rows of the file
it starts in, which is what `PatchSelection`'s single `path` and per-file `hunk_index` mean. Unit
tests: `a_hunk_range_stops_at_the_file_boundary`,
`a_selection_covers_one_file_even_when_the_range_spans_two`.

### 7. Visual — done

- **Whole rows.** Every side list is wrapped in a box of exactly `rows × row_h`, so a pane whose
  body is not a multiple of the row height keeps the remainder as background instead of painting a
  sliced row.
- **Ellipsis.** Commit subjects, branch names, remote-branch names and file paths take a `ch`
  budget derived from the measured side-column width and go through the kit's `truncate` helper
  (`Truncate::Tail` for subjects, `Middle` for names and paths). The commit row's subject also
  gained the `min_w_0` its decoration/subject flex row was missing, which is why a long subject
  used to be sliced mid-word with no ellipsis at all.
- **Inactive selection.** A pane that lost focus paints its cursor row on `row_hover` rather than
  `row_selected`, so only the focused pane's row reads as *the* selection while the other panes
  still show where their cursor is.

### Screenshots

Window-only captures (`screencapture -l<window>`, so a system dialog on top of the screen cannot
photobomb them) under
`/private/tmp/claude-501/-Users-danny--swarm-worktrees-dannyfuf-fleetd-lazygit/a9a6b104-1a54-433f-b4fa-2e71c0f146fa/scratchpad/shots2/`:
`01-branches-order`, `02-subcommits`, `03-subcommits-patch`, `04-commit-files-whole`,
`05-commit-files-one`, `06-stash-menu-letters`, `07-help-clamped`, `08-unfocused-selection`. The
`shot` step of a drive script still uses `screencapture -x`, which photographs the whole screen.

### Not addressed

- `g` (reset options) is not bound inside the sub-commits view: `g g` already means "first item" in
  the main panel, and a bare `g` in a deeper context makes that sequence ambiguous.
- `enter` inside the **sub-commits** view shows the selected commit's patch rather than drilling one
  level further into its file list. The brief asked for both; the sub-commits sentence
  ("`enter`/`space` on a sub-commit showing its diff") is the one that won, because a third
  drill-down level would need an explicit main-panel stack for `esc` to unwind.
- The main panel's diff rows are still not snapped to whole rows — the requirement was the side
  panes, and a diff is a continuous text surface where a clipped last line is the normal reading
  experience.

## 13. File tree — done

The Files pane is lazygit's file tree, on by default (`gui.showFileTree`), with `` ` `` / `~`
switching to the flat layout.

### The model — `src/views/file_tree.rs`

`FileTree` owns three things: the `tree_mode` flag, the set of collapsed directory **paths**, and
the flattened `Vec<FileRow>` the pane renders. `rebuild(&snapshot.files)` is the only way rows
come into existence, and `GitUiState::apply_snapshot` calls it before it retains the Files cursor,
so the row list and the cursor can never disagree.

The build is `pkg/gui/filetree`'s three steps in order:

1. **Insert** every `FileStatus` path into a `BTreeMap` node tree — the map *is* the alphabetical
   sort, and directories are held separately from files, so they come out first (see the deviation
   below).
2. **Compress** — while a directory's only child is another directory, the child replaces it and
   the display name grows a segment, so `src/app/deep` is one row. lazygit advances tree depth by
   `1 + CompressionLevel` but *visual* depth by 1, and `FileRow::depth` is the visual one.
3. **Flatten** depth first, computing each directory's aggregate on the way back up:
   `staged`, `unstaged`, `conflict`, plus `children` — every file path under the row, which is
   what `space`, `d` and the main panel act on. A collapsed directory is walked into a scratch
   buffer that is thrown away, so it still knows its children without emitting their rows.

Flat mode is lazygit's `BuildFlatTreeFromFiles`: the tree's leaves, then a **stable** re-sort that
puts conflicted files first, tracked next and untracked last.

`index_of(path)` is what keeps the selection: the row with that exact path, or else the deepest
**visible ancestor** of it, which is where lazygit leaves the cursor when a collapse swallows the
row it was on. `GitUiState::resync_file_rows` runs it after every rebuild and re-derives
`sel_file` from the row the cursor landed on, so `ListCursor::retain` keeps working unchanged.

### Rendering — `views/rows.rs::file_tree_row`

One row shape for both kinds, `presentation/files.go:143`-`:179` exactly: a leading column of
`2 × depth` characters of indent (plain spaces, no line art) plus a 2 ch glyph cell, then the
flexible name column. The glyph cell holds either the two independently coloured status characters
of a file or a directory's `▼` / `▶`; the name colour is lazygit's three cases — green fully
staged, yellow mixed, default otherwise — computed from the same two booleans for a file and for a
directory. The ellipsis budget loses `2 × depth` characters, so a deep row truncates rather than
overflowing. The pane header still counts **files**: a directory is a fold of the list, not an
entry in it.

### Keys

`space`, `d` and `enter` branch on `GitUiState::file_row()` rather than on `selected_file()`,
which now returns `None` on a directory row:

| Key | On a file | On a directory |
| --- | --- | --- |
| `space` | stage / unstage it | `Stage`/`Unstage` **every** file under it — staged when anything below is unstaged, unstaged otherwise, which is lazygit's `node.GetHasUnstagedChanges()` test |
| `d` | discard it (confirm) | discard every file under it, the confirm naming the file count |
| `enter` | staging mode, or the conflict view | collapse or expand it (lazygit's "Stage lines / Collapse directory") |
| `` ` `` `~` | toggle tree / flat | same |
| `-` / `=` | collapse-all / expand-all | same |
| `a` | unchanged — still the whole worktree | unchanged |

### The main panel on a directory row

`refresh_main` puts a directory into `MainContent::FileDiff` with the **directory** as its path
and asks for `GitRequest::PathsDiff { key, paths }`, which the worker answers with the ordinary
`GitEvent::FileDiff` keyed by that path. Reusing the file variant is deliberate: to the main panel
a directory is a path with two sides, so nothing in `panels/main_panel.rs` or `views/diff.rs`
needed to change, and both halves (unstaged | staged) still split when a directory has both.

Backend: `Repository::diff_paths(&[PathBuf], DiffSide)` — one `git diff [--cached] -- <paths>`
for the tracked paths, then one `--no-index` patch per untracked path, concatenated (`API.md`
updated, `diff_paths_combines_tracked_and_untracked_children` covers it).

### Verification

`make-repo.sh` gained a **`tree`** mode: nested `src/app/deep/{one,two}.txt` and `pkg/three.txt`
committed, then dirtied into one modified, one staged and one untracked nested file, so the pane
renders

```
▼ pkg              ▼ src/app/deep      M  alpha.txt
  M  three.txt       ?? new.txt        M  gamma.txt
                      M one.txt        ?? untracked.txt
```

Seven new drive scripts, all green, each asserting git state that a *wrong* tree would get wrong:

| Script | What its assertion proves |
| --- | --- |
| `files-tree-render` | the tree renders (screenshot only) |
| `files-tree-stage-directory` | `space` on `src/app/deep` stages both of its files, the untracked one included |
| `files-tree-collapse` | `enter` collapsed `pkg`, so the next `j` reached `src/app/deep` — without the collapse `space` would have *unstaged* `pkg/three.txt` |
| `files-tree-collapse-expand` | `-` then `=` restored the full tree, so `j` landed back on a file row |
| `files-tree-discard-directory` | `d` + confirm restored `one.txt`, deleted the untracked `new.txt`, and left `alpha.txt` dirty |
| `files-tree-toggle-flat` | `~` really flattened: row 2 was `src/app/deep/one.txt`, not `pkg/three.txt` |
| `files-tree-selection-retained` | two `space`es on the same directory row — the refresh between them did not move the cursor |

Unit tests: nine in `views::file_tree::tests` (sorting, compression, a chain that stops
compressing where it branches, aggregation, collapse persistence across a rebuild, collapse-all /
expand-all, the hidden-row fallback, flat ordering) plus two in `state::tests`
(`a_directory_row_survives_a_refresh_and_asks_for_its_children_diff`,
`collapsing_a_directory_moves_the_selection_onto_it`).

### Deviations, and why

- **No root `/` row**, although lazygit's `showRootItemInFileTree` defaults to `true`. Every row
  of the pane stays something the keys can act on, and the row numbering the existing `drive/`
  scripts rely on does not shift.
- **Directories sort before files.** lazygit's `fileTreeSortOrder` defaults to `mixed`, which
  interleaves them alphabetically; this pane is always `foldersFirst`, which is what the build
  brief asked for.
- **A directory row prints no status characters**, only its arrow and its name — that is lazygit
  (`presentation/files.go:143`-`:151`), where the aggregate shows up as the name colour.
  `FileRow::status` computes the two characters anyway (`M`/`?` from the aggregate) so a test can
  assert on them.
- Both `` ` `` and `~` are bound to the toggle, because they are the two halves of one physical
  key and a platform may report either.
- `h` / `l` still cycle pane tabs rather than collapsing; the reference's Files table binds
  collapse to `enter`, `-` and `=` only.

### Screenshots

Window-only captures (`screencapture -l<window>`, the window id read out of
`CGWindowListCopyWindowInfo`, so a suite window or a system dialog cannot photobomb them) under
`/private/tmp/claude-501/-Users-danny--swarm-worktrees-dannyfuf-fleetd-lazygit/a9a6b104-1a54-433f-b4fa-2e71c0f146fa/scratchpad/shots3/`:

| File | What it shows |
| --- | --- |
| `01-tree.png` | the default tree: `▼ pkg` green (fully staged), the compressed `▼ src/app/deep`, the two status characters indented under each |
| `02-collapsed.png` | after `-`: both directories `▶`, the three root-level files below |
| `03-flat.png` | after `~`: full paths, tracked first, the two untracked ones last |
| `04-directory-diff.png` | `src/app/deep` selected — the main panel is the *combined* unstaged patch of `one.txt` and the untracked `new.txt`, and the command log shows the three `git diff` calls `diff_paths` made |
| `05-directory-mixed.png` | one file of the directory staged: the directory name turns yellow and the main panel splits unstaged \| staged |

`files-tree-render.txt`'s own `shot` step still uses `screencapture -x` (whole screen), like every
other drive script.

## 14. Diff view v2 — done

The complaint was two-fold: the diff would not scroll with the wheel, and it did not look like a
modern web diff viewer. `docs/research/diff-view-brief.md` is the research this pass implements.

### What the old renderer was

250 lines. It flattened `Diff` into `Vec<DiffRow>` **on every render and several times per
keystroke**, then painted each row as four flex `Text` runs whose only distinction was a
foreground colour taken from the terminal palette. No background tint, no syntax highlighting, no
word diff, no clamp on the horizontal offset (`payload.chars().skip(h_scroll)` silently produced
blank rows past the longest line), and diff context nailed to git's default 3.

### The pieces

| Module | What it does |
| --- | --- |
| `views/syntax.rs` | `syntect` 5.3 (`default-fancy`, so pure Rust — no `onig_sys`) + `two-face` 0.5 (`syntect-fancy`) for 213 grammars. Language detection is extension-then-filename, so `Dockerfile` resolves too. A hunk is parsed **twice**, once over its old side (context + removed) and once over its new side (context + added), because a hunk's `-` and `+` lines are two different versions of the file and one parser state cannot serve both. Output is a [`Bucket`] per span — nine of them — not an `Hsla`, so the pass needs no `Theme` and can run on the background executor while light/dark resolves at paint time. Lines over 1 000 bytes are skipped (Pierre's `tokenizeMaxLineLength`); the pass stops at 40 000 lines or 1.5 s and logs a warning. |
| `views/intraline.rs` | Change blocks (a maximal removed run followed by a maximal added run — Pierre's `ChangeContent`), paired by index, gated on `similar`'s ratio ≥ 0.5, capped at 5 lines a block, 512 bytes and 400 tokens a line, with a 10 ms `timeout` on the diff itself. Tokenization is ours, not `similar`'s: `from_words` splits on whitespace only and marks the whole of `foo.bar(x)` as changed, while unicode word segmentation folds `foo.bar` into one word for the same reason it folds `e.g.`. The tokenizer is Zed's `CharClassifier` shape — a boundary at every character-class transition, each punctuation character its own token — fed to `TextDiff::diff_slices`, which is also what lets a `timeout` be set at all (`iter_inline_changes`'s 500 ms deadline is hardcoded). |
| `views/diff_model.rs` | The cached model. `ModelKey` is the `Arc<Diff>` pointer plus the patch's shape plus the view mode; `state::keep_or_replace` keeps the existing `Arc` when a refresh re-reads an identical diff, which is what makes the cache survive the two-second tick. Unified rows are the single source of truth; the split layout is a thinner `Vec<SplitRow>` that *indexes* them, so syntax runs, word spans and staging coordinates are computed once. |
| `views/diff.rs` | The renderer and the `uniform_list` that virtualises it. |

`DiffRow` keeps `(file, hunk, line)` and `is_change()` exactly as before, so
`state::hunk_range`, `state::selection_hunks` and `fleet_git::PatchSelection` are untouched — the
staging contract did not change, and its five drive scripts pass unmodified.

### Decisions worth recording

- **Colours are derived, not tokenised.** `docs/DESIGN-SYSTEM.md` reserves the semantic tokens for
  Fleet-level state and `views/mod.rs` points diff colours at the terminal palette. The row wash
  is therefore `bg.blend(hue.opacity(a))` with Zed's filled-hunk ladder (0.12 dark / 0.16 light),
  a lighter step for the gutters (Pierre's 91 % vs 88 %) and a stronger one for changed words.
  Zero new `ColorTokens` fields; a future theme gets sane diff colours for free.
- **The file header is two rows, not a taller row.** `uniform_list` measures one item and
  extrapolates, so one taller row corrupts the scroll position and the thumb. Zed spells its own
  `FILE_HEADER_HEIGHT` in rows for the same reason. Path on the first row, `+ n` / `− m` (U+2009
  thin space, U+2012 figure dash, so the columns line up), language and mode on the second.
- **Selection composites over the tint.** The old code replaced the row background outright, which
  turned a selected added line into a plain grey row. It is now the accent hue at α 0.26 over the
  diff wash, plus the 2 px cursor bar on every row of the selection.
- **Unified is the default**, unlike both Zed and Pierre, which default to split: lazygit is a
  keyboard staging tool in a narrow pane and `Staging { cursor, anchor, line_mode }` models one
  cursor over one row sequence. `|` toggles split (free in lazygit, and it reads as two columns);
  staging mode always renders unified whatever the toggle says, because the staging cursor is a
  unified row index.
- **The syntax pass is entirely off the foreground thread.** syntect at `regex-fancy` is ~60 µs a
  line here, so a 22 000-line diff is over a second. Rows render plain and repaint when the pass
  lands. There is no visible-window-only path: with the model cached and the pass backgrounded,
  the foreground never does highlight work at all, which is a stronger guarantee than a budget.
- **No wrap mode.** It needs variable row heights; `uniform_list` cannot express them and
  `gpui::list` only estimates total content height, which costs an exact scrollbar and an exact
  `scroll_to_item`. Stated as a deviation from the brief rather than half-built.
- **`similar` needs no extra features.** The brief specified `["inline", "unicode"]`; with our own
  tokenizer and `diff_slices` neither is used, so neither is enabled.
- **`{` / `}` set the context on the `Repository`**, not per call site: a patch built from a
  displayed hunk must be applied against a diff read with the same `-U`, and one shared
  `AtomicU32` is the only way that cannot drift. Every existing `diff_*` signature and test is
  unchanged.

### Two bugs found on the way

- **The wheel already worked; the cursor did not survive a refresh.** `uniform_list` presets
  `overflow.y = Scroll` and owns a persistent offset through `track_scroll`, so the wheel was
  always live. What actually looked broken was `snap_to_change`: the periodic re-read of the file
  diff dragged the staging selection back to the first hunk every two seconds. `Staging::snapped`
  makes snapping one-shot per open / side flip / mode toggle.
- **`gpui`'s `UniformListDecoration` is laid out as a *root*.** Absolutely positioned children of
  the returned element never paint, because a percentage or an inset has no containing block to
  resolve against. The scrollbar is therefore a `canvas` painting two quads at coordinates
  computed from the `bounds` and `scroll_offset` the hook hands over, with the offset undone so
  the bar stays pinned to the viewport.

### One follow-up fix

The sign shared a 2 ch cell with a leading space and the code followed immediately, so an
unindented line put `+`/`-` flush against its first character while an indented one left a gap:
the code column drifted by a character between row kinds. The sign now has its own `SIGN_CH`
(1 ch) cell and the code a fixed `SIGN_GAP_CH` (1 ch) of padding after it, in both modes, so the
code starts at the same x on every row — `gutter_width` is derived from the same two constants,
and a unit test pins the relationship. Found with it: `max_h_scroll` measured against the whole
pane in split mode, so `L` stopped short of the end of a long line; it now halves the column.

### Measured, on this machine, release build

| Case | Model build (foreground) | Syntax pass (background) |
| --- | --- | --- |
| `Cargo.lock`, 569 rows / 742 lines of TOML | 68–73 µs | 1–2 ms |
| the biggest commit (`a65c1d4`, 92 files), 7 731 rows / 10 451 lines | 1.5 ms | 533 ms |
| the whole `crates/` directory diff, 23 146 rows / 22 777 lines | 2.1–2.6 ms | 1 399 ms, budget-capped |

That is ~50–60 µs a line for the syntax pass, which is the `regex-fancy` figure the brief
measured, and it is entirely off the foreground thread.

The foreground number is the only one that can drop a frame, and the worst case is a seventh of a
16 ms budget. A build that overruns 16 ms logs a warning (`diff: flattening the model overran one
frame`), as does a background pass over 250 ms.

### Tests and drive

27 new unit tests (net +25 after retiring the two the old renderer had) across `views::syntax` (language detection, bucket assignment, the length cap,
sorted-and-disjoint runs), `views::intraline` (block pairing, the addition-then-removal split, the
line cap, word spans, the join-single-separator rule, the ratio gate, the tokenizer) and
`views::diff_model` (row layout, staging identity, file metadata, word marks, split alignment,
syntax jobs and their one-shot claim, binary notes, tab expansion, cache-key discrimination) plus
`views::diff` (tint ordering and opacity, the horizontal clamp, gutter width).

Five new drive scripts on a new `make-repo.sh ts` tree (committed-then-edited `demo.ts` and
`demo.tsx`, plus a 124-line `long.ts` ending in three very wide lines):
`diff-wheel-scroll`, `diff-split-toggle`, `diff-context-keys`, `diff-syntax-typescript`,
`diff-word-highlight`. `drive.rs` gained `wheel <rows> [x] [y]` and `hwheel <cells> [x] [y]` steps that dispatch a real
`ScrollWheelEvent` through the window's hit test. Suite: **77 passed, 0 failed**.

### Screenshots

Window-only captures under
`/private/tmp/claude-501/-Users-danny--swarm-worktrees-dannyfuf-fleetd-lazygit/a9a6b104-1a54-433f-b4fa-2e71c0f146fa/scratchpad/shots4/`:

| File | What it shows |
| --- | --- |
| `01-unified-typescript.png` | `demo.ts` unified: the header card, TypeScript colours, word marks on `string`→`SkuCode`, `0.2`→`0.21`, `TAX_RATE)`→`TAX_RATE + FEE)` |
| `02-split-typescript.png` | the same patch after `\|`, one gutter per column |
| `03-staging-line-mode.png` | line mode, one change row selected, the tint still readable under the selection |
| `04-staging-range.png` | `v` + `j`: the removed and added rows as one block, cursor bar on both |
| `05-cargo-lock-top.png` / `06-cargo-lock-midscroll.png` | the 742-line `Cargo.lock` diff, TOML coloured, scrollbar thumb on the right |
| `07-commit-patch.png` | the biggest commit's patch: one header card per file, function context on every `@@` |
| `08-horizontal-scroll.png` | `L` panning past a 1 143-character line, gutters pinned |
| `09-context-widened.png` | `}` up to `-U6`: demo.ts's three hunks merged into one |
| `10-context-narrowed.png` | `{` back down to `-U2` — visible in the command log — with the hunks split apart again |
| `11-split-panned.png` | split mode after `L`: both columns pan together, the gutters and sign cells stay pinned, the divider does not move |

## 15. Fleet integration — done

The native lazygit view is now the third tab of every Fleet worktree session, in the same
window. `fleet-app` depends on `fleet-lazygit` and renders `Lazygit` as a pane.

### Checkpoints

1. **Linkable next to `fleet-app`.** gpui registers actions process-wide as `namespace::Name`
   through `inventory` and `App::load_actions` panics on a duplicate, so `confirm` → `lg_confirm`
   and `help` → `lg_help` (actions.rs, keymap.rs, root.rs, README). gpui's `>` is a
   *subsequence* test over the rendered chain, so the overlay context words `Dialog` / `Confirm`
   / `Help` → `LgDialog` / `LgConfirm` / `LgHelp` (state.rs `context_chain`, keymap.rs,
   overlays.rs, README) — otherwise fleet-app's bare `Dialog`, `Dialog > Confirm` and
   `Dialog > Help` bindings would fire inside the pane. Two invariant tests in
   `fleet-app/src/keymap.rs` keep both true: `no_action_name_is_registered_twice` walks the
   whole gpui inventory, `the_embedded_pane_shares_no_context_word_with_the_app` intersects the
   two key tables.
2. **Embeddable.** `Lazygit::embedded(path, cx)` next to `Lazygit::new`. Its `render` builds the
   same bands *without* `AppFrame` (`Lazygit::pane`), keeps the one-row key-hint/mode bar, drops
   the unconditional per-frame `window.focus`, and takes focus only through
   `set_active(bool, window, cx)` or a click. `cx.quit()` became `cx.emit(LazygitEvent::Quit)`;
   `lib.rs` subscribes and quits for the standalone binary. Row budgets come from a measured
   pane size (`canvas`) instead of `Window::viewport_size`, which is far too tall inside a tab.
   `root::tests::the_embedded_frame_draws_no_app_frame` pins the frame split.
3. **Config and daemon.** `fleet://` is a reserved `windows[].command` scheme; `fleet://lazygit`
   is the only member. It is the third default window; `validate_config` rejects any other
   `fleet://` command; `normalize_imported_windows` upgrades a swarm `lazygit` (bare or with
   arguments) on import only, so a hand-written `lazygit` in Fleet's own `config.json` is the
   opt-out. `Terminal.kind: Pty | Native` (serde default `Pty`, so the wire model is
   backward-compatible and `PROTOCOL_VERSION` stays 1). `Sessions::new_native_terminal` registers
   the tab with no PTY and no `TerminalHost`, so numbering, `active_terminal` and
   `SelectTerminal` are unchanged while attach/key/resize/scroll/paste answer `NotFound` and
   restart answers `Conflict`. Sleep treats it as idle by construction; `refresh_statuses` never
   lets it report a session attached. A remote worktree degrades the command back to `lazygit`.
4. **The mount.** `fleet-app` registers `fleet_lazygit::keymap::init` once. `TerminalTab` gained
   a `kind` with a `git-branch` glyph. `WorkspaceScreen` holds `panes: HashMap<WorktreeId, Pane>`
   created lazily on first activation, observed and subscribed, evicted when the daemon stops
   listing the worktree. `TerminalMode::Native` gives the chain
   `Fleet > Workspace > Native > Lazygit > …`, and `Workspace > Native` binds exactly one key,
   `ctrl-s`, mirroring `Workspace > Terminal`. The shell yields focus to the pane while it owns
   the keyboard and takes it back for any overlay. The tab strip's `on_select` is finally wired,
   so a click reaches the pane too.
5. **Tests and docs.** New tests: config validation + import normalization + the remote fallback
   (fleet-core), `a_native_window_is_a_tab_without_a_process` and
   `a_native_tab_is_idle_and_never_keeps_a_session_awake` and the import assertion
   (fleet-daemon), `the_workspace_mode_follows_the_kind_of_the_active_tab`,
   `a_native_tab_is_marked_but_keeps_its_number` and the two keymap invariants (fleet-app), and
   the embedded-frame test here. Docs: `ARCHITECTURE.md` (native tabs), `SWARM-INVENTORY.md`
   (the `windows` default and the divergence), `KEYMAP.md` (Native mode), `APP-CONTRACTS.md`
   (the chain and the two collision rules), `UX-SPEC.md` §2.1 chrome contract and §3.6, this
   crate's README ("Embedding"), and the settings dialog's read-only windows list.

### Gates

| Gate | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` (the Makefile's `clippy` target) | clean |
| `cargo test --workspace` | 646 passed, 0 failed |
| `cargo build --release` | ok (`fleet`, `fleetd`, `fleet-lazygit`) |
| `drive/run-suite.sh` | **77 passed, 0 failed** |

The drive suite is timing-sensitive: run under a concurrently running Fleet app it flaked on
`branches-upstream-set`, `branches-upstream-unset` and `branches-rebase-conflict-continue` with
`unhandled key`, because a script's opening `wait 1500` is not enough for the first snapshot on a
loaded machine. All three pass in isolation and the full suite is green when it runs alone.

### Live verification

A throwaway `FLEET_HOME` under the scratchpad, a throwaway repository and worktree, and the
release binaries (`FLEET_DAEMON` pointed at `target/release/fleetd` — the ambient `FLEET_DAEMON`
of a Fleet terminal otherwise wins and spawns a *stale* daemon from another checkout). The
window-only captures live in
`…/scratchpad/shots5/`; `winidpid.swift` matches the window by pid, because matching by owner
name catches every other `fleet*` window on the machine.

* `fleet import --from-swarm` on a fake swarm home upgraded `{"name":"lg","command":"lazygit"}`
  to `fleet://lazygit` in Fleet's own `config.json`, live.
* `02` / `12` / `24`: `ctrl-s 3` renders the pane with the worktree's real data (three changed
  files, two branches, two commits, an empty stash, a syntax-highlighted diff with word marks),
  inside the tab band, with Fleet's context bar, header, tab strip and status bar intact.
* The tab strip reads `1 nvim │ 2 cc ⚡● │ 3 ⑂ lg │ +` — the `git-branch` glyph marks the native
  tab and the index is still `3`.
* `03` `j`/`k` · `04` `space` (unstaged `util.rs`, the diff re-read) · `05` `?` (the pane's help
  overlay, clipped to the pane) · `06`/`07` `3`/`4` panels · `08`–`10` `+` half/full/normal ·
  `11` `ctrl-s 1` back to the live nvim PTY · `22` `ctrl-s [` toasts `no scrollback in this tab`
  · `23` `q` selects the previous tab · `13`/`14` a Fleet dialog takes the keyboard and gives it
  back · `31`/`32` `ctrl-s s` returns to a *focused* Hub.

