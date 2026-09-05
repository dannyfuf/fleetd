# A modern diff view for `fleet-lazygit` — implementer's brief

**Date of research:** 2026-09-05
**Audience:** whoever rewrites `crates/fleet-lazygit/src/views/diff.rs` and its host in
`crates/fleet-lazygit/src/panels/main_panel.rs`.

Read `docs/research/gpui.md` first for the framework basics and `docs/research/lazygit-native-brief.md`
for how this crate is wired. This document is only about the diff view: what modern diff viewers do,
how Zed does it at the exact gpui tag we are pinned to, and what we should build.

Ground rules that shaped every recommendation:

- gpui is pinned to the Zed tag **v1.18.1** (`Cargo.toml:26`-`Cargo.toml:27`). Every gpui signature
  in this document was read from the checkout at
  `/Users/danny/.cargo/git/checkouts/zed-a70e2ad075855582/bebe92f/` (verified
  `crates/zed/Cargo.toml:5` → `version = "1.18.1"`, `crates/gpui/Cargo.toml:3` → `0.2.2`).
  **Anything you read online about gpui may be newer.** Where this document contradicts your memory
  of gpui, this document read the source.
- Naming on this tag is `Window` + `Context<'a, T>` + `App`. **`ViewContext` does not exist**
  (`rg ViewContext crates/gpui/src` → 0 hits).
- `crates/fleet-ui-kit` depends on `gpui` and nothing else (`crates/fleet-ui-kit/src/lib.rs:11`).
  Any new dependency lands in `fleet-lazygit` or `fleet-git`, never in the kit.
- Rust edition 2024, `rust-version = "1.97.1"`.

Contents:

- [0. Where we are today](#0-where-we-are-today)
- [1. How Zed renders diffs](#1-how-zed-renders-diffs)
  - [1a. Hunks in the editor](#1a-hunks-in-the-editor)
  - [1b. Painting text with syntax highlighting in gpui](#1b-painting-text-with-syntax-highlighting-in-gpui)
  - [1c. The syntax highlighting pipeline](#1c-the-syntax-highlighting-pipeline)
  - [1d. Word-level diff and deleted/added alignment](#1d-word-level-diff-and-deletedadded-alignment)
  - [1e. gpui v1.18.1 primitives, with signatures](#1e-gpui-v1181-primitives-with-signatures)
- [2. Diffs by Pierre (diffs.com)](#2-diffs-by-pierre-diffscom)
- [3. Recommendation for this codebase](#3-recommendation-for-this-codebase)
  - [3a. Syntax highlighting: pick syntect + two-face](#3a-syntax-highlighting-pick-syntect--two-face)
  - [3b. Intra-line diff: pick similar](#3b-intra-line-diff-pick-similar)
  - [3c. Pairing removed and added lines inside a hunk](#3c-pairing-removed-and-added-lines-inside-a-hunk)
  - [3d. Target design](#3d-target-design)
  - [3e. Theme token additions](#3e-theme-token-additions)
  - [3f. gpui gotchas for this work](#3f-gpui-gotchas-for-this-work)

---

## 0. Where we are today

### 0.1 The current renderer

`crates/fleet-lazygit/src/views/diff.rs` is 250 lines. It flattens a `fleet_git::Diff` into a
`Vec<DiffRow>` (one row per rendered line) and renders each row as a flex `div` of four sibling
`Text` runs. Virtualisation is delegated to `ListView` → `gpui::uniform_list`.

Public surface:

| item | line | note |
|---|---|---|
| `pub const ROW_H: f32 = 18.0;` | `crates/fleet-lazygit/src/views/diff.rs:17` | |
| `pub enum RowKind { FileHeader, HunkHeader, Added, Removed, Context, Other }` | `:21` | |
| `pub struct DiffRow` | `:38` | `{ kind, text: String, old_no, new_no, file, hunk, line }` |
| `DiffRow::is_change()` | `:58` | `Added \| Removed` |
| `pub fn flatten(diff: &Diff) -> Vec<DiffRow>` | `:65` | |
| `pub fn marker(kind: RowKind) -> &'static str` | `:135` | |
| `pub fn row(row, selected, focused, h_scroll, cx) -> AnyElement` | `:150` | the whole renderer |
| `pub fn gutter_width() -> Pixels { ch(9.0) }` | `:207` | |

The core of `row()` (`crates/fleet-lazygit/src/views/diff.rs:150`-`:203`) picks a **foreground
colour only**:

```rust
let (color, weight) = match row.kind {
    RowKind::Added => (Ansi::Green.color(theme), gpui::FontWeight::NORMAL),
    RowKind::Removed => (Ansi::Red.color(theme), gpui::FontWeight::NORMAL),
    RowKind::HunkHeader => (Ansi::Cyan.color(theme), gpui::FontWeight::NORMAL),
    RowKind::FileHeader => (theme.colors.text, gpui::FontWeight::MEDIUM),
    RowKind::Context | RowKind::Other => (theme.colors.text_secondary, gpui::FontWeight::NORMAL),
};
```

and then emits `gutter(old_no)`, `gutter(new_no)`, the marker, and the payload as four children.
Horizontal scrolling is `payload.chars().skip(h_scroll)` (`:168`).

### 0.2 The eight constraints that shape the rewrite

1. **There is no background tint on changed lines today** — only foreground colour, and it comes
   from `theme.terminal.ansi[..]` by deliberate design. `crates/fleet-lazygit/src/views/mod.rs:12`
   states the rule: *"lazygit names ANSI colours in its presentation code (`style.FgGreen`,
   `style.FgRed`, …). The faithful mapping is the theme's terminal palette, not the semantic
   tokens, which stay reserved for Fleet-level state."* §3e proposes how to add diff backgrounds
   without breaking that rule.
2. **`flatten()` runs on every render and several times per keystroke.** `main_panel.rs:359`
   rebuilds the whole `Vec<DiffRow>` (a `String` per line) inside `diff_list`, and
   `Lazygit::main_rows()` (`crates/fleet-lazygit/src/root.rs:437`) does the same for
   `staging_range`, `patch_selection`, `main_len`, `snap_to_change`, `staging_hunk` and
   `retarget_staging_side`. **Caching the flattened model keyed by `Arc<Diff>` pointer identity is
   a prerequisite** for anything heavier (word diff, syntax highlighting).
3. **`uniform_list` forces a uniform row height.** Anything variable — a taller file-header card, a
   collapsed-hunk row, a wrapped long line — breaks `ListView`. `main_panel.rs:572` already
   documents hitting this trap for the command log.
4. **There is no intra-line diff data and no crate in the workspace to compute it.** `similar`,
   `imara-diff`, `syntect`, `tree-sitter`, `two-face`, `diffy`, `dissimilar` have **zero**
   occurrences in `Cargo.lock`.
5. **Diff context is fixed at git's default 3 lines.** `crates/fleet-git/src/read.rs:16`:
   `const DIFF_ARGS: [&str; 4] = ["--no-color", "--no-ext-diff", "--patch", "--find-renames"];`
   — no `-U<n>` anywhere in the workspace. Adding a context control means threading a parameter
   through the five `read.rs` functions that share that const.
6. **Horizontal scroll has no upper clamp** (`views/diff.rs:168`, `root.rs:899`) — scrolling past
   the longest line silently yields blank rows.
7. **`ROW_H` is duplicated as a literal `px(18.0)`** at `main_panel.rs:382` and `:414`.
8. **Binary files short-circuit before hunks** (`views/diff.rs:83`) and `crates/fleet-git/src/patch.rs:38`
   rejects them for line staging. The rewrite should surface that as a distinct row kind, not a
   generic `Other`.

### 0.3 The data model we render

`crates/fleet-git/src/model.rs:407`-`:481`:

```rust
pub struct LineRange { pub start: u32, pub count: u32 }              // :407
pub enum LineKind { Context, Added, Removed, NoNewline, Other }      // :~415

pub struct DiffLine {                                                // :~425
    pub kind: LineKind,
    pub content: Vec<u8>,        // raw bytes, no prefix, no trailing LF
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}
pub struct Hunk {
    pub old: LineRange, pub new: LineRange,
    pub header: Vec<u8>,         // the function-context text after the `@@` ranges
    pub lines: Vec<DiffLine>,
}
pub struct DiffFile {
    pub old_path: Option<PathBuf>,   // without `a/`
    pub new_path: Option<PathBuf>,   // without `b/`
    pub kind: DiffKind,
    pub binary: bool,
    pub mode: Option<ModeChange>,
    pub headers: Vec<Vec<u8>>,       // raw file-header lines, for patch reconstruction
    pub hunks: Vec<Hunk>,
}
pub struct Diff { pub files: Vec<DiffFile> }
```

with `DiffKind { Added, Deleted, Modified, Renamed, Copied, TypeChanged, Unmerged, Unknown }`
(`:379`), `DiffSide { Unstaged, Staged }` (`:370`), `ModeChange { old, new }` (`:400`).

What we have, and what we must compute ourselves:

| available | **not** available |
|---|---|
| `old_no` / `new_no` per line | **add/remove counts** — count `LineKind` yourself |
| `binary: bool` per file | **intra-line / word diff** — no `--word-diff` in the parser |
| renames and copies (`old_path` + `new_path`) | **similarity index %** — raw in `headers` only |
| file mode (`ModeChange`) | **blob OIDs** — raw in `headers` only |
| `hunk.header` (function context) | **submodule diffs, file size, language metadata** |
| `headers` (raw, reconstructable) | anything ANSI (`--no-color` is forced) |

Selection maths already lives in pure testable functions:
`state::hunk_range(rows, cursor)` (`crates/fleet-lazygit/src/state.rs:1601`) and
`state::selection_hunks(rows, start, end)` (`:1619`), both keyed by `(file, hunk)` because hunk
indices restart per `DiffFile`. `Staging { path, side, cursor, anchor, line_mode }` is at
`state.rs:246`. **The rewrite must keep those signatures working** — they are the staging contract.

---

## 1. How Zed renders diffs

All paths in this section are relative to
`/Users/danny/.cargo/git/checkouts/zed-a70e2ad075855582/bebe92f/`, abbreviated **`$Z`**.

Four headline findings, because they invalidate the obvious assumptions:

1. **`crates/editor/src/hunk_diff.rs` does not exist.** The replacement is
   `$Z/crates/editor/src/git.rs` (3283 lines).
2. **Deleted lines are not block decorations.** They are *real text spliced into the multibuffer's
   coordinate space* via `DiffTransform::DeletedHunk`. No `BlockContext`, no `BlockPlacement::Above`,
   no "deleted hunk block". `$Z/crates/editor/src/display_map/block_map.rs` has no diff-specific
   block at all — `enum Block` (`:376`) is `Custom | FoldedBuffer | ExcerptBoundary | BufferHeader | Spacer`.
3. **Zed does word-level intra-line diff, on by default,** with dedicated theme colours. This is
   newer than most write-ups about Zed.
4. **The default diff view is side-by-side (split), not unified.** `DiffViewStyle::Split` is
   `#[default]` (`$Z/crates/settings_content/src/editor.rs:980`). Row alignment between the two
   panes is done with generated `Block::Spacer` filler rows.

### 1a. Hunks in the editor

#### Deleted lines are text, not decorations

`$Z/crates/multi_buffer/src/multi_buffer.rs:714`:

```rust
#[derive(Debug, Clone)]
enum DiffTransform {
    BufferContent {
        summary: MBTextSummary,
        inserted_hunk_info: Option<DiffTransformHunkInfo>,
    },
    DeletedHunk {
        summary: TextSummary,
        buffer_id: BufferId,
        hunk_info: DiffTransformHunkInfo,
        base_text_byte_range: Range<usize>,
        has_trailing_newline: bool,
    },
}
```

stored as `diff_transforms: SumTree<DiffTransform>` (`:699`). The builder is
**`$Z/crates/multi_buffer/src/multi_buffer.rs:2810` — `fn recompute_diff_transforms_for_edit(...)`**;
it flushes context text (`:2932`), pushes the **entire deleted run as one contiguous transform**
(`:2963`-`:2996`), and then lets the buffer's own added rows follow tagged as inserted (`:2996`).

Because a `DeletedHunk` region reads from the diff's *base text* buffer, deleted rows are read-only
for free: every edit path filters on `region.is_main_buffer` (`multi_buffer.rs:1582`
`convert_edits_to_buffer_edits`, guards at `:1594`, `:1612`, `:1634`, `:1650`, `:1663`, `:1683`).

**This is the single biggest architectural decision in Zed's design, and we should not copy it.**
It exists so a diff is *editable in place* inside a real editor. We render a read-only patch; a flat
row model is the right shape for us (§3d).

#### Status types — `$Z/crates/buffer_diff/src/buffer_diff.rs`

```rust
// :86
pub struct DiffHunkStatus { pub kind: DiffHunkStatusKind, pub secondary: DiffHunkSecondaryStatus }
// :92
pub enum DiffHunkStatusKind { Added, Modified, Deleted }
// :102 — "Diff of Working Copy vs Index — aka 'is this hunk staged or not'"
pub enum DiffHunkSecondaryStatus {
    HasSecondaryHunk,             // unstaged
    OverlapsWithSecondaryHunk,    // partially staged
    NoSecondaryHunk,              // staged
    SecondaryHunkAdditionPending, // we are unstaging
    SecondaryHunkRemovalPending,  // we are staging
}
// :116 — the hunk retains sub-line information
pub struct DiffHunk {
    pub range: Range<Point>,
    pub buffer_range: Range<Anchor>,
    pub diff_base_byte_range: Range<usize>,
    pub secondary_status: DiffHunkSecondaryStatus,
    pub buffer_word_diffs: Vec<Range<Anchor>>,  // absolute buffer anchors
    pub base_word_diffs: Vec<Range<usize>>,     // offsets rel. to diff_base_byte_range.start
}
```

The five-state `DiffHunkSecondaryStatus` including two *pending* states is worth stealing: it is how
Zed keeps a stage/unstage button honest while the git command is in flight.

#### Row background tints — computed per row, painted as coalesced quads

Zed does **not** iterate hunks to paint row backgrounds. It iterates `RowInfo`
(`$Z/crates/multi_buffer/src/multi_buffer.rs:815`, field `diff_status: Option<DiffHunkStatus>`) and
builds a map of row → `LineHighlight`. `$Z/crates/editor/src/element.rs:8261`:

```rust
struct DiffHunkHighlightColors { filled_background: Hsla, hollow_background: Hsla, hollow_border: Hsla }

let added_diff_hunk_colors = DiffHunkHighlightColors {           // :8268
    filled_background: colors.editor_diff_hunk_added_background,
    hollow_background: colors.editor_diff_hunk_added_hollow_background,
    hollow_border:     colors.editor_diff_hunk_added_hollow_border,
};
// … deleted at :8273 …
for (ix, row_info) in row_infos.iter().enumerate() {             // :8280
    let Some(diff_status) = row_info.diff_status else { continue };
    let diff_hunk_colors = match diff_status.kind { /* Added | Deleted */ };
    let hollow_highlight = LineHighlight {                       // :8295
        background: diff_hunk_colors.hollow_background.into(),
        border: Some(diff_hunk_colors.hollow_border),
        include_gutter: true, type_id: None,
    };
    let filled_highlight = LineHighlight {                       // :8302
        background: solid_background(diff_hunk_colors.filled_background),
        border: None, include_gutter: true, type_id: None,
    };
    let background = if self.diff_hunk_hollow(diff_status, cx) { hollow_highlight } else { filled_highlight };
    highlighted_rows.entry(/* display row */).or_insert(background);   // :8318
}
```

The paint is **`$Z/crates/editor/src/element.rs:4910` — `fn paint_background(...)`**, closure at
`:4986`, one `window.paint_quad(fill(Bounds { origin, size }, highlight.background))` per *run*.
The run-coalescing loop at `:5019`-`:5057` merges consecutive rows with an identical `LineHighlight`
into a single quad and applies `Edges { top: px(1.), bottom: px(1.) }` only at run boundaries. **That
is what makes a hollow hunk read as one outlined box rather than a stack of per-row boxes** — copy
this, it is cheap and it is the difference between "looks designed" and "looks striped".

Filled vs hollow encodes staged-ness (`$Z/crates/editor/src/element.rs:6608`):

```rust
fn diff_hunk_hollow(&self, status: DiffHunkStatus, cx: &mut App) -> bool {
    let unstaged = !self.editor.read(cx).diff_hunk_delegate().render_hunk_as_staged(&status, cx);
    let unstaged_hollow = matches!(
        ProjectSettings::get_global(cx).git.hunk_style, GitHunkStyleSetting::UnstagedHollow);
    unstaged == unstaged_hollow
}
```
Default is `"hunk_style": "staged_hollow"` (`$Z/assets/settings/default.json:1735`).

#### The gutter marker

`$Z/crates/editor/src/element.rs:5329`:

```rust
fn gutter_strip_width(line_height: Pixels, cx: &App) -> Pixels {
    match EditorSettings::get_global(cx).gutter.git_gutter_width {
        GitGutterWidth::Custom(width) => px(*width),
        GitGutterWidth::Default => (0.275 * line_height).floor(),
    }
}
```

`fn diff_hunk_bounds(...)` at `:5337` gives three geometries:

| case | width | height | corner radius |
|---|---|---|---|
| folded (`:5349`) | `floor(0.275 × line_height)` | 1 line | `px(0.)` |
| pure deletion, empty row range (`:5365`) | `floor(0.35 × line_height)` (`:5374`) | 1 line, **y-offset `−line_height/2`** so it straddles the boundary (`:5371`) | `1.0 × line_height` → a **pill** |
| normal (`:5378`) | `gutter_strip_width` | `start_row..end_row` | `px(0.)` |

All flush at x = 0 of the gutter. Painting is `fn paint_gutter_diff_hunks` (`:5222`), inside
`window.paint_layer(layout.gutter_hitbox.bounds, …)` (`:5235`). The quad (`:5293`-`:5322`) uses a
flatten-then-blend trick and carries **the only hard-coded diff opacity in the whole renderer**:

```rust
// Flatten the background color with the editor color to prevent
// elements below transparent hunks from showing through
let flattened_background_color = cx.theme().colors().editor_background.blend(background_color);

if !self.diff_hunk_hollow(status, cx) {
    window.paint_quad(quad(hunk_bounds, corner_radii, flattened_background_color,
                           Edges::default(), transparent_black(), BorderStyle::default()));
} else {
    let flattened_unstaged_background_color =
        cx.theme().colors().editor_background.blend(background_color.opacity(0.3));   // <- 0.3
    window.paint_quad(quad(hunk_bounds, corner_radii, flattened_unstaged_background_color,
                           Edges::all(px(1.0)), flattened_background_color, BorderStyle::Solid));
}
```

Line numbers are tinted by diff status too — `$Z/crates/editor/src/element.rs:107`
`enum LineNumberStyle { Breakpoint, DiffAdded, DiffDeleted, Active, Inactive }`, `fn color` at `:130`
maps `DiffAdded → version_control_added`, `DiffDeleted → version_control_deleted`.

#### The hunk control strip

The extension seam is a trait — `$Z/crates/editor/src/git.rs:26`:

```rust
pub trait DiffHunkDelegate {
    fn toggle(&self, hunks: Vec<ResolvedDiffHunks>, editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>);
    fn stage_or_unstage(&self, stage: bool, hunks: Vec<ResolvedDiffHunks>, ...);
    fn restore(&self, hunks: Vec<ResolvedDiffHunks>, ...) { /* default at :44 */ }
    fn render_hunk_controls(                                              // :71
        &self, row: u32, status: &DiffHunkStatus, hunk_range: Range<Anchor>,
        is_created_file: bool, line_height: Pixels,
        editor: &Entity<Editor>, window: &mut Window, cx: &mut App,
    ) -> AnyElement;
    fn render_hunk_as_staged(&self, status: &DiffHunkStatus, _cx: &App) -> bool {
        !status.has_secondary_hunk()                                      // :83
    }
}
```

The default strip is `$Z/crates/editor/src/git.rs:3056` — `pub fn render_diff_hunk_controls(...)`.
Its container (`:3076`-`:3089`) hangs off the *bottom* of the hunk's first row like a tab:

```rust
h_flex()
    .h(line_height).mr_1().gap_1().px_0p5().pb_1()
    .border_x_1().border_b_1()
    .border_color(cx.theme().colors().border_variant)
    .rounded_b_lg()
    .bg(cx.theme().colors().editor_background)
    .block_mouse_except_scroll()
    .shadow_md()
```

Children: **Stage**/**Unstage** (`:3090`-`:3144`, `.alpha(if status.is_pending() { 0.66 } else { 1.0 })`),
**Restore** (`:3145`-`:3170`, `.disabled(is_created_file)`), and next/prev-hunk `IconButton`s
(`:3172`-`:3237`, only when `!all_diff_hunks_expanded()`).

Layout is `$Z/crates/editor/src/element.rs:4651` — `fn layout_diff_hunk_controls(...)`, and the
three behaviours worth stealing are:

- **Visibility** (`:4674`, `:4741`): `active_rows = [hovered_diff_hunk_row, newest_cursor_row]`. The
  strip appears for the hovered *or* cursor hunk, one at a time. Never all of them.
- **Sticky clamping** (`:4718`-`:4726`): `y = if hunk_start_y >= sticky_top { hunk_start_y } else
  { sticky_top.min(hunk_end_y - line_height) }` — the strip slides down to stay on screen as its
  hunk scrolls off the top, but never past the hunk's own last row.
- **Position** (`:4750`): right-aligned in the text area,
  `x = text_hitbox.bounds.right() - right_margin - px(10.) - size.width`.

#### Expand-context controls are a *separate* mechanism

The gutter chevrons that widen an excerpt are not hunk controls.
`$Z/crates/multi_buffer/src/multi_buffer.rs:809` `pub struct ExpandInfo { direction, start_anchor }`,
`:1167` `pub enum ExpandExcerptDirection { Up, Down, UpAndDown }`, computed per row at `:7817`-`:7841`
and surfaced on `RowInfo.expand_info`.

`$Z/crates/editor/src/element.rs:2657` — `fn layout_expand_toggles(...)`: icons
`IconName::ExpandUp / ExpandDown / ExpandVertical` (`:2699`-`:2703`), sized
`IconSize::Custom(rems(editor_font_size / window.rem_size()))` where
`editor_font_size = text.font_size × 1.2` (`:2673`), positioned at
`x = git_gutter_width + px(1.)` (`:2735`) — immediately right of the diff strip. Tooltip
`"Expand Excerpt"`, action `ExpandExcerpts`. Context width comes from
`$Z/crates/editor/src/editor.rs:12655` `pub fn multibuffer_context_lines(cx) -> u32` (setting
`excerpt_context_lines`, default 2, capped `.min(32)`).

Relevant actions (`$Z/crates/editor/src/actions.rs`): `ExpandAllDiffHunks` `:512`,
`CollapseAllDiffHunks` `:514`, `ToggleAllDiffHunks` `:517`, `GoToHunk` `:586`,
`GoToPreviousHunk` `:588`, `ToggleSelectedDiffHunks` `:907`.

#### The project-diff multibuffer

`$Z/crates/git_ui/src/project_diff.rs` is a thin shell — its `Render` is literally
`div().size_full().child(self.diff.clone())` (`:578`-`:582`). The engine is
`$Z/crates/git_ui/src/diff_multibuffer.rs` (1038 lines), entry `DiffMultibuffer::new` at `:62`.

**File ordering is literally `Ord` on `PathKey`** — there is no separate sort pass.
`$Z/crates/multi_buffer/src/path_key.rs:19`:

```rust
#[derive(PartialEq, Eq, Ord, PartialOrd, Clone, Hash, Debug)]
pub struct PathKey { pub sort_prefix: Option<u64>, pub path: Arc<RelPath> }
```

with `$Z/crates/git_ui/src/diff_multibuffer.rs:951`:

```rust
const CONFLICT_SORT_PREFIX: u64 = 1;
const TRACKED_SORT_PREFIX:  u64 = 2;
const NEW_SORT_PREFIX:      u64 = 3;
```

The doc comment at `:955`-`:967` states the invariant that makes it work: **the key must be a pure
function of `(path, status)`, never of the current file set**, because it doubles as excerpt
identity. Worth copying if we ever group files in the diff view.

#### The per-file header

There is no custom block in `git_ui`. `MultiBuffer::new` sets `show_headers: true`
(`$Z/crates/multi_buffer/src/multi_buffer.rs:1212`), the block map inserts a
`Block::BufferHeader`, and the editor renders it.

Sizes are measured in **rows**, not pixels — `$Z/crates/editor/src/editor.rs:290`:

```rust
pub const FILE_HEADER_HEIGHT: u32 = 2;                  // ROWS
pub const BUFFER_HEADER_PADDING: Rems = rems(0.25);     // 4px @ 16px rem
pub const MULTI_BUFFER_EXCERPT_HEADER_HEIGHT: u32 = 1;  // ROWS
```

`$Z/crates/editor/src/element/header.rs:617` — `pub(crate) fn render_buffer_header(...)`. Outer box
(`:698`-`:710`) is `.p(BUFFER_HEADER_PADDING).w_full().h(FILE_HEADER_HEIGHT as f32 * window.line_height())`;
inner bar (`:711`-`:731`) is `h_flex().group("buffer-header-group").size_full().pl_1().pr_2()
.rounded_sm().gap_1p5().border_1()` on `editor_subheader_background`, `.when(is_sticky && opaque_window, shadow_md)`.

Children in order: fold chevron (`:732`-`:794`), an **addon slot** (`:796`-`:804`), dirty/conflict
`Indicator::dot()` (`:805`-`:813`), filename button + file icon + `Label` struck through when
deleted (`:838`-`:875`), parent directory `Label` with `.truncate_start()` (`:877`-`:892`),
`FileLock` icon when not editable (`:893`-`:895`), the diff stat (`:908`-`:919`), and an
`"Open File"` button (`:920`-`:940`).

`file_status` and `diff_stat` are computed **only when `all_diff_hunks_expanded()`** (`:646`-`:655`).
Label colour comes from `fn file_status_label_color` (`:1093`): conflicted → `Color::Conflict`,
modified → `Color::Modified`, deleted → `Color::Disabled`, created → `Color::Created`.

**`$Z/crates/ui/src/components/diff_stat.rs` is directly copyable** — `DiffStat::new(id, added, removed)`,
rendering (`:36`-`:59`):

```rust
h_flex().gap_1()
  .child(Label::new(format!("+\u{2009}{added}")).color(Color::Success).size(self.label_size))
  .child(Label::new(format!("\u{2012}\u{2009}{removed}")).color(Color::Error).size(self.label_size))
```

Note **U+2009 THIN SPACE** between the sign and the number, and **U+2012 FIGURE DASH** rather than a
hyphen — a figure dash is the same width as a digit, so `+ 12 / − 3` columns line up. That is a
one-character detail with a disproportionate effect on how considered the header looks.

The sticky file header is `$Z/crates/editor/src/element/header.rs:140` —
`layout_sticky_buffer_header(...)`, with a gradient scrim at `:171`-`:181`:
`bg(linear_gradient(0., linear_color_stop(editor_bg.opacity(0.), 0.), linear_color_stop(editor_bg, 0.6)))`.

#### Stage/unstage in the file header is a tri-state Checkbox, not a Button

`$Z/crates/git_ui/src/git_panel.rs:7287` implements the addon hook (declared
`$Z/crates/editor/src/editor.rs:764`-`:772`), and at `:7313`:

```rust
Checkbox::new("stage-file", is_staging_or_staged.into())
    .disabled(!self.has_write_access(cx))
    .fill()
    .elevation(ElevationIndex::Surface)
```

Click → `toggle_staged_for_entry(&entry, StageIntent::Toggle, window, cx)` (`:7326`), wrapped in
`h_flex().id("start-slot").text_lg()` with `on_mouse_down(Left, stop_propagation)` (`:7329`).

#### Theme colours

Declared in `$Z/crates/theme/src/styles/colors.rs`:

| field | line |
|---|---|
| `editor_diff_hunk_added_background` | `245` |
| `editor_diff_hunk_added_hollow_background` | `247` |
| `editor_diff_hunk_added_hollow_border` | `249` |
| `editor_diff_hunk_deleted_background` | `251` |
| `editor_diff_hunk_deleted_hollow_background` | `253` |
| `editor_diff_hunk_deleted_hollow_border` | `255` |
| `version_control_added` / `_deleted` / `_modified` / `_renamed` | `323` / `325` / `327` / `329` |
| `version_control_conflict` / `_ignored` | `331` / `333` |
| **`version_control_word_added`** / **`version_control_word_deleted`** | `335` / `337` |
| `version_control_conflict_marker_ours` / `_theirs` | `339` / `341` |

Base constants — `$Z/crates/theme/src/default_colors.rs:11`-`:40`:

```rust
const ADDED_COLOR:        Hsla = Hsla { h: 134./360., s: 0.55, l: 0.40, a: 1.00 };  // #2E9E48FF
const WORD_ADDED_COLOR:   Hsla = Hsla { h: 134./360., s: 0.55, l: 0.40, a: 0.35 };  // #2E9E4859
const MODIFIED_COLOR:     Hsla = Hsla { h:  48./360., s: 0.76, l: 0.47, a: 1.00 };  // #D3AF1DFF
const REMOVED_COLOR:      Hsla = Hsla { h: 350./360., s: 0.88, l: 0.25, a: 1.00 };  // #78081AFF
const WORD_DELETED_COLOR: Hsla = Hsla { h: 350./360., s: 0.88, l: 0.25, a: 0.80 };  // #78081ACC
```

**The opacity ladder is the number to steal.** `$Z/crates/theme_settings/src/schema.rs:16`-`:21`:

```rust
const LIGHT_DIFF_HUNK_FILLED_OPACITY: f32 = 0.16;
const LIGHT_DIFF_HUNK_HOLLOW_BACKGROUND_OPACITY: f32 = 0.08;
const LIGHT_DIFF_HUNK_HOLLOW_BORDER_OPACITY: f32 = 0.48;
const DARK_DIFF_HUNK_FILLED_OPACITY: f32 = 0.12;
const DARK_DIFF_HUNK_HOLLOW_BACKGROUND_OPACITY: f32 = 0.06;
const DARK_DIFF_HUNK_HOLLOW_BORDER_OPACITY: f32 = 0.36;
```

Applied at `ThemeColors::light()` `$Z/crates/theme/src/default_colors.rs:132`-`:137` and
`::dark()` `:285`-`:290`. **Row backgrounds apply no runtime multiplier** — the alpha is baked into
the theme field; only the hollow *gutter strip* gets a further `.opacity(0.3)` at element.rs:~5314.

The One Dark / One Light JSON (`$Z/assets/themes/one/one.json`) **omits `editor.diff_hunk.*`
entirely**, so the derived values are what you actually see:

| | One Dark (×0.12 / 0.06 / 0.36) | One Light (×0.16 / 0.08 / 0.48) |
|---|---|---|
| added filled | `#27a6571F` | `#27a65729` |
| added hollow bg | `#27a6570F` | `#27a65714` |
| added hollow border | `#27a6575C` | `#27a6577A` |
| deleted filled | `#e06c761F` | `#e06c7629` |
| deleted hollow bg | `#e06c760F` | `#e06c7614` |
| deleted hollow border | `#e06c765C` | `#e06c767A` |

Word-diff colours are explicit in One: `version_control.word_added` `#2EA04859` (`one.json:102`),
`version_control.word_deleted` `#78081BCC` (`:103`) for One Dark; One Light uses `#2EA04859` /
`#F85149CC` (`:521`/`:522`).

The refinement chain (`$Z/crates/theme_settings/src/schema.rs:237` `fn theme_colors_refinement`) is
transitive: `editor_diff_hunk_added_background` ← explicit key ← `version_control.added` ←
`status.created`, at the appearance-appropriate opacity, falling back to the compiled-in default.
`$Z/crates/theme/src/fallback_themes.rs:29` derives any missing `status.*_background` as
`fg.opacity(0.25)`.

### 1b. Painting text with syntax highlighting in gpui

#### The line layout path

`$Z/crates/editor/src/element.rs:3072` — `fn layout_lines(rows, snapshot, style, editor_width,
is_row_soft_wrapped, bg_segments_per_row, window, cx) -> Vec<LineWithInvisibles>`. It calls
`snapshot.highlighted_chunks(rows, language_aware, style)` (`:3132`) and feeds the iterator to
`LineWithInvisibles::from_chunks` (`:3133`). `MAX_LINE_LEN = 1024` (`$Z/crates/editor/src/editor.rs:294`)
hard-truncates longer lines.

The core loop of `from_chunks` (`$Z/crates/editor/src/element.rs:7083`, body `:7253`-`:7290`) is
exactly the shape we want:

```rust
let text_style = if let Some(style) = highlighted_chunk.style {
    Cow::Owned(text_style.clone().highlight(style))   // HighlightStyle folded onto TextStyle
} else {
    Cow::Borrowed(text_style)
};
styles.push(TextRun {
    len: line_chunk.len(),                 // UTF-8 BYTES
    font: text_style.font(),
    color: text_style.color,
    background_color: text_style.background_color,
    underline: text_style.underline,
    strikethrough: text_style.strikethrough,
});
line.push_str(line_chunk);
```

and at each `\n` (`:7218`-`:7231`) it flushes with **one `shape_line` call per display row**:

```rust
let shaped_line = window.text_system().shape_line(line.clone().into(), font_size, text_runs, None);
fragments.push(LineFragment::Text(shaped_line));
```

So: one `String` + one `Vec<TextRun>` per row, one shape per row. That is the same budget a
per-line `StyledText` costs us.

Painting (`$Z/crates/editor/src/element.rs:7485` `draw_with_custom_offset`) applies horizontal
scroll as a **negative x offset on the paint origin**, not by slicing the string:

```rust
let mut fragment_origin = content_origin
    + gpui::point(Pixels::from(-layout.position_map.scroll_pixel_position.x), line_y);
```

`$Z/crates/editor/src/element.rs:7331` `split_runs_by_bg_segments` re-splits the run vector at
selection/highlight background boundaries and rewrites `TextRun::color` through
`ensure_minimum_contrast(run.color, segment_color, min_contrast)` — this is how syntax colours stay
legible on a selection background. Worth remembering when we put syntax colours on top of a diff
tint (§3d).

#### `TextRun` and `shape_line` — exact definitions

`$Z/crates/gpui/src/text_system.rs:1003`:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextRun {
    /// A number of utf8 bytes
    pub len: usize,
    pub font: Font,
    pub color: Hsla,
    pub background_color: Option<Hsla>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
}
```

**There is no `TextSystem::shape_line` on this tag.** The shaping API is on `WindowTextSystem`
(`$Z/crates/gpui/src/text_system.rs:380`, which `Deref`s to `Arc<TextSystem>`):

```rust
// text_system.rs:413
pub fn shape_line(&self, text: SharedString, font_size: Pixels, runs: &[TextRun],
                  force_width: Option<Pixels>) -> ShapedLine;   // debug_asserts no '\n'
// text_system.rs:464 — cache-key-by-hash; ShapedLine.text is a placeholder on a cache hit
pub fn shape_line_by_hash(&self, text_hash: u64, text_len: usize, font_size: Pixels,
                          runs: &[TextRun], force_width: Option<Pixels>,
                          materialize_text: impl FnOnce() -> SharedString) -> ShapedLine;
// text_system.rs:525 — multi-line + soft wrap
pub fn shape_text(&self, text: SharedString, font_size: Pixels, runs: &[TextRun],
                  wrap_width: Option<Pixels>, line_clamp: Option<usize>)
    -> Result<SmallVec<[WrappedLine; 1]>>;
```

Access is `window.text_system()`. The `force_width` parameter is the monospace-grid trick our
`TerminalGrid` already uses (`crates/fleet-ui-kit/src/components/terminal_grid.rs`).

`ShapedLine` (`$Z/crates/gpui/src/text_system/line.rs:43`) has `len()`, `width()`,
`with_len(usize)` (render text A as text B), `paint(origin, line_height, align, align_width, window, cx)`,
`paint_background(...)`, and `split_at(byte_index) -> (ShapedLine, ShapedLine)` (`:141`).

#### Composing highlight styles

`$Z/crates/gpui/src/style.rs:580`:

```rust
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct HighlightStyle {
    pub color: Option<Hsla>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub background_color: Option<Hsla>,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: Option<StrikethroughStyle>,
    pub fade_out: Option<f32>,   // like CSS opacity
}
```

Two composition operators, and they differ:

- `TextStyle::highlight(self, impl Into<HighlightStyle>) -> Self` (`style.rs:510`) —
  **`color` blends** (`self.color = self.color.blend(color)`); weight/style/background/underline
  overwrite when `Some`; `fade_out` calls `self.color.fade_out(factor)`.
- `HighlightStyle::highlight(self, other) -> Self` (`style.rs:924`) — `color` blends if both set;
  everything else is `other.X.or(self.X)`; `fade_out` multiplies as `(dest * (1. + source)).clamp(0., 1.)`.

**`gpui::combine_highlights` (`$Z/crates/gpui/src/style.rs:990`) is the function you will actually
need:**

```rust
pub fn combine_highlights(
    a: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    b: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
) -> impl Iterator<Item = (Range<usize>, HighlightStyle)>
```

It sweeps endpoints, folds all active styles with `HighlightStyle::highlight`, and emits
**disjoint, sorted, non-overlapping** ranges. This is exactly how we merge syntax highlights with
word-diff highlights before handing them to `StyledText` — see the gotcha in §3f.

#### `StyledText` — the exact API on this tag

`$Z/crates/gpui/src/elements/text.rs:391`:

```rust
impl StyledText {
    pub fn new(text: impl Into<SharedString>) -> Self;                          // :401
    pub fn layout(&self) -> &TextLayout;                                        // :412
    pub fn with_default_highlights(                                             // :418
        mut self,
        default_style: &TextStyle,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self;
    pub fn with_highlights(                                                     // :433
        mut self,
        highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
    ) -> Self;
    pub fn with_font_family_overrides(                                          // :488
        mut self, overrides: impl IntoIterator<Item = (Range<usize>, SharedString)>,
    ) -> Self;
    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self;                     // :526
}
```

The semantics that matter:

- **`with_highlights` takes no `&TextStyle`.** It is *lazy* — it stores the ranges and resolves them
  at `request_layout` time against the **inherited** `window.text_style()` from the parent element
  chain (`:562`-`:567`).
- **`with_default_highlights` is eager** — computes `Vec<TextRun>` immediately via `compute_runs`
  (`:453`) against the style you pass, then routes into `with_runs`.
- **They are mutually exclusive**; both carry `debug_assert!` guards (`:419`-`:422`, `:434`-`:437`).
- **Ranges are UTF-8 byte offsets**, asserted on char boundaries (`:441`-`:442`, `:459`, `:463`).
- **Overlapping or unsorted ranges are not handled.** `compute_runs` (`:453`-`:479`) walks a
  monotonic cursor:

  ```rust
  let mut ix = 0;
  for (range, highlight) in highlights {
      if ix < range.start { runs.push(default_style.clone().to_run(range.start - ix)); }
      runs.push(default_style.clone().highlight(highlight).to_run(range.len()));
      ix = range.end;
  }
  if ix < text.len() { runs.push(default_style.to_run(text.len() - ix)); }
  ```

  Via `with_default_highlights` the malformed result hits the `with_runs` assertion at `:527`-`:537`
  and **panics** (`"invalid text run"`). Via `with_highlights` there is no validation at all —
  `shape_text` logs `"TextRun`s do not cover the entire to be shaped text"` and you get silently
  misaligned colours.
- **`with_runs` asserts the run lengths exactly tile the string.**

`TextLayout` (`:614`) is the hit-testing surface: `index_for_position(Point<Pixels>) -> Result<usize, usize>`
(`:830`), `position_for_index(usize) -> Option<Point<Pixels>>` (`:864`), `bounds()` (`:930`),
`line_height()` (`:935`), `len()` (`:940`). All panic if called before prepaint.

Wrapping is decided inside `TextLayout::layout` (`:625`): `wrap_width` is only computed when
`text_style.white_space == WhiteSpace::Normal` (`:650`-`:658`). **Setting `WhiteSpace::Nowrap` on the
line style is how you turn wrapping off**, and it short-circuits the wrap machinery entirely.

`InteractiveText` (`:981`):

```rust
pub fn new(id: impl Into<ElementId>, text: StyledText) -> Self;                          // :1008
pub fn on_click(mut self, ranges: Vec<Range<usize>>,
                listener: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self;      // :1022
pub fn on_hover(mut self,
                listener: impl Fn(Option<usize>, MouseMoveEvent, &mut Window, &mut App) + 'static) -> Self; // :1041
pub fn tooltip(mut self,
               builder: impl Fn(usize, &mut Window, &mut App) -> Option<AnyView> + 'static) -> Self;        // :1050
```

`on_click`'s `usize` is the **index into `ranges`**, not a byte offset, and it only fires when
mouse-down and mouse-up land in the same range (`:1029`-`:1033`). `on_hover`/`tooltip` receive a byte
index. **There is no `TextState` type on this tag** — the request-layout state is `TextLayout` and
the element state is `InteractiveTextState` (`:999`).

#### Tabs and wide characters

**Zed expands tabs to spaces in the display map, before shaping.** `$Z/crates/editor/src/display_map/tab_map.rs`,
`TabChunks::next` (`:653`, expansion at `:677`-`:696`):

```rust
let tab_size = if self.input_column < self.max_expansion_column { self.tab_size.get() } else { 1 };
let mut len = tab_size - self.column % tab_size;      // to next tab stop
return Some(Chunk { text: &SPACES[..len], is_tab: true, .. });
```

The shaper never sees `\t`. **Do the same**: expand with `tab_size - (col % tab_size)`, counting
columns in chars, and shift your highlight ranges accordingly.

**There is no CJK / double-width handling in Zed's layout at this tag** — `rg 'is_cjk|chars_wide'`
over the whole tree returns zero hits. Wide glyphs are handled entirely by the shaper: `shape_line`
returns the true advance. Column arithmetic stays char-based and Zed simply lets the mismatch exist.

#### Metrics

`$Z/crates/editor/src/element.rs:8025`-`:8033` computes everything in one place:

```rust
let rem_size    = window.rem_size();
let font_id     = window.text_system().resolve_font(&style.text.font());
let font_size   = style.text.font_size.to_pixels(rem_size);
let line_height = style.text.line_height_in_pixels(rem_size);
let em_width         = window.text_system().em_width(font_id, font_size).unwrap();
let em_advance       = window.text_system().em_advance(font_id, font_size).unwrap();
let em_layout_width  = window.text_system().em_layout_width(font_id, font_size);
let glyph_grid_cell  = size(em_advance, line_height);
```

The three "em" flavours are **not** interchangeable in Zed's code: `em_width` (typographic bounds of
`m`, `text_system.rs:242`) for UI padding, `em_advance` (advance of `m`, `:249`) for the glyph grid,
`em_layout_width` (fully shaped advance, `:730`) as the **horizontal-scroll unit**. There are also
`ch_width` (`:256`) and `ch_advance` (`:263`) measuring `0`.

`line_height_in_pixels` (`$Z/crates/gpui/src/style.rs:553`) `.round()`s, while `StyledText`'s own
layout uses `window.pixel_snap(...)` on the same quantity (`text.rs:632`-`:636`). **Pass the same
`TextStyle` to your gutter and your content** or they will disagree by a fraction of a pixel.

#### Horizontal scrolling

`$Z/crates/editor/src/scroll.rs:32`-`:33`:

```rust
pub type ScrollOffset      = f64;   // x = columns (em_layout_width units), y = display rows
pub type ScrollPixelOffset = f64;
```

The longest line is measured once via `layout_line(snapshot.longest_row(), …).width`
(`$Z/crates/editor/src/element.rs:8652`), the max is computed in **columns**
(`:8786`, `(scroll_width - editor_width) / em_layout_width`), and converted to pixels at `:8691`.

#### Mouse wheel

`$Z/crates/gpui/src/interactive.rs:513`:

```rust
#[derive(Clone, Debug, Default)]
pub struct ScrollWheelEvent {
    pub position: Point<Pixels>,
    pub delta: ScrollDelta,
    pub modifiers: Modifiers,
    pub touch_phase: TouchPhase,
}
impl Deref for ScrollWheelEvent { type Target = Modifiers; }   // :535 — event.shift works directly

#[derive(Clone, Copy, Debug)]
pub enum ScrollDelta {          // :545
    Pixels(Point<Pixels>),      // precise — trackpad
    Lines(Point<f32>),          // imprecise — notched wheel
}
impl Default for ScrollDelta { fn default() -> Self { Self::Lines(Default::default()) } }
impl ScrollDelta {
    pub fn precise(&self) -> bool;                                     // :597
    pub fn pixel_delta(&self, line_height: Pixels) -> Point<Pixels>;   // :605
    pub fn coalesce(self, other: ScrollDelta) -> ScrollDelta;          // :616
}
```

Zed's conversion — `$Z/crates/editor/src/element/mouse.rs:478` `paint_scroll_wheel_listener`,
closure at `:499`. The load-bearing details:

```rust
let mut delta = ScrollDelta::default();      // captured, PERSISTS across events
// …
delta = delta.coalesce(event.delta);
let scroll_sensitivity = if event.modifiers.alt { fast_scroll_sensitivity } else { base_scroll_sensitivity };
let delta = match delta {
    ScrollDelta::Pixels(mut pixels) => {                    // trackpad
        editor.scroll_manager.filter_scroll_delta(&mut pixels, event.touch_phase);
        pixels
    }
    ScrollDelta::Lines(lines) => point(lines.x * glyph_width, lines.y * line_height),
};
let current = position_map.snapshot.scroll_position();
let x = (current.x * glyph_width - delta.x * scroll_sensitivity) / glyph_width;
let y = (current.y * line_height - delta.y * scroll_sensitivity) / line_height;
let mut scroll_position = point(x, y).clamp(&point(0., 0.), &position_map.scroll_max);
```

Note: the delta is **subtracted**, sensitivity applies *after* the pixel conversion, `Lines` are
multiplied by `em_layout_width` / `line_height`, and the result is clamped.

`filter_scroll_delta` → `OngoingScroll::filter` (`$Z/crates/gpui/src/gestures.rs:31`) is the
**axis lock** (`UNLOCK_PERCENT = 1.9`, `UNLOCK_LOWER_BOUND = px(6.)`, reset on
`TouchPhase::Ended/Cancelled`). Without it, trackpad diagonal drift makes horizontal code scrolling
unusable. Copy this.

### 1c. The syntax highlighting pipeline

The chain, concretely:

```
tree-sitter Query (highlights.scm)
  └─ Query::capture_names() -> &[&str]      index i == tree-sitter capture_id
        ▼
SyntaxTheme::highlight_id(capture_name) -> Option<u32>      // dotted-prefix fallback
        ▼
HighlightId::new(u32)                       // NonZeroU32(capture_id + 1)
        ▼
HighlightMap(Arc<[Option<HighlightId>]>)    // indexed BY tree-sitter capture_id
        ▼
Grammar::highlight_map: Mutex<HighlightMap> // rebuilt on every theme change
        ▼
BufferChunks::next() -> Chunk { syntax_highlight_id: Option<HighlightId>, .. }
        ▼
SyntaxTheme::get(id) -> Option<&HighlightStyle>
        ▼
TextStyle::highlight(..) -> TextRun
```

`$Z/crates/language_core/src/highlight_map.rs` is the whole module, 40 lines, `std`-only:

```rust
pub struct HighlightMap(Arc<[Option<HighlightId>]>);            // :4
pub struct HighlightId(NonZeroU32);                             // :7
impl HighlightId {
    pub fn new(capture_id: u32) -> Self {                       // :13
        Self(NonZeroU32::new(capture_id + 1).unwrap_or(NonZeroU32::MAX))
    }
}
impl From<HighlightId> for usize {                              // :18
    fn from(value: HighlightId) -> Self { value.0.get() as usize - 1 }
}
```

Built by `$Z/crates/language/src/language.rs:1189` `build_highlight_map(capture_names, theme)`,
rebuilt on theme change by `Language::set_theme` (`:1151`) and `LanguageRegistry::set_theme`
(`$Z/crates/language/src/language_registry.rs:491`).

**`SyntaxTheme` has moved out of the `theme` crate into its own crate** at this tag.
`$Z/crates/theme/src/styles/syntax.rs:1` is one line (`pub use syntax_theme::SyntaxTheme;`); the real
definition is `$Z/crates/syntax_theme/src/syntax_theme.rs` (336 lines, whole crate, deps: `gpui`,
`serde`, `serde_json`).

```rust
pub fn get(&self, highlight_index: impl Into<usize>) -> Option<&HighlightStyle>;   // :60
pub fn style_for_name(&self, name: &str) -> Option<HighlightStyle>;                // :64
pub fn highlight_id(&self, capture_name: &str) -> Option<u32>;                     // :78
#[cfg(feature = "bundled-themes")]
pub fn one_dark() -> Arc<Self>;                                                    // :220
```

`highlight_id` (`:78`-`:93`) is the **dotted-prefix fallback**: it `BTreeMap::range`s from the first
dot-segment up to the name, then finds the longest key that is a prefix ending on a `.` boundary. So
`@string.special.symbol` resolves to `string.special.symbol`, else `string.special`, else `string`.
**That is why ~46 theme keys cover hundreds of grammar captures**, and it is the mechanism we want if
we ever map TextMate scopes to our own token names.

#### The One Dark syntax table

Source of truth is `$Z/assets/themes/one/one.json`, `"One Dark"` at `:7`, `"syntax"` at `:191`. The
Rust fallback in `fallback_themes.rs` is **classic Atom One Dark and differs materially** — do not
use it as a reference. 46 keys, every entry only `color` / `font_style` / `font_weight`; no
`background_color`, `underline`, `strikethrough` or `fade_out` anywhere.

| category | color | | category | color |
|---|---|---|---|---|
| `attribute` | `#74ade8` | | `preproc` | `#b477cf` |
| `boolean` | `#bf956a` | | `primary` | `#acb2be` |
| `comment` | `#5d636f` | | `property` | `#d07277` |
| `comment.doc` | `#878e98` | | `punctuation` | `#acb2be` |
| `constant` | `#dfc184` | | `punctuation.bracket` | `#b2b9c6` |
| `constructor` | `#73ade9` | | `punctuation.delimiter` | `#b2b9c6` |
| `embedded` | `#dce0e5` | | `punctuation.list_marker` | `#d07277` |
| `emphasis` | `#74ade8` | | `punctuation.markup` | `#d07277` |
| `emphasis.strong` | `#bf956a` **700** | | `punctuation.special` | `#b1574b` |
| `enum` | `#6eb4bf` | | `selector` | `#dfc184` |
| `function` | `#73ade9` | | `selector.pseudo` | `#74ade8` |
| `hint` | `#788ca6` | | `string` | `#a1c181` |
| `keyword` | `#b477cf` | | `string.escape` | `#878e98` |
| `label` | `#74ade8` | | `string.regex` | `#bf956a` |
| `link_text` | `#73ade9` *italic* | | `string.special` | `#bf956a` |
| `link_uri` | `#6eb4bf` | | `string.special.symbol` | `#bf956a` |
| `namespace` | `#dce0e5` | | `tag` | `#74ade8` |
| `number` | `#bf956a` | | `text.literal` | `#a1c181` |
| `operator` | `#6eb4bf` | | `title` | `#d07277` **400** |
| `predictive` | `#5a6a87` *italic* | | `type` | `#6eb4bf` |
| | | | `variable` | `#acb2be` |
| | | | `variable.parameter` | `#d07277` |
| | | | `variable.special` | `#bf956a` |
| | | | `variant` | `#73ade9` |
| | | | `diff.plus` | `#98c379` |
| | | | `diff.minus` | `#e06c75` |

The closest canonical enum is `ZedSyntaxToken` (`$Z/crates/theme_importer/src/vscode/syntax.rs:28`-`:66`),
**39 tokens**; `syntax` itself is an open `IndexMap<String, HighlightStyleContent>`
(`$Z/crates/settings_content/src/theme.rs:555`) with only four expressible fields (`color`,
`background_color`, `font_style`, `font_weight`) — `underline`, `strikethrough` and `fade_out` exist
on `gpui::HighlightStyle` but cannot be set from theme JSON.

Note the deliberately small palette: **eight hues do all the work**, and half the categories share a
colour. That is the calibration to match in §3e — not one colour per token type.

#### Reusing this outside Zed

`tree-sitter` is pinned as a **git rev**, not a crates.io version — workspace `Cargo.toml:837`:

```toml
tree-sitter = { git = "https://github.com/tree-sitter/tree-sitter", rev = "43623ec9bf0eaaf7113285c46e8a09018f181b18" }
```

resolving to `tree-sitter` **v0.27.0** (`Cargo.lock:19061`-`:19063`).

Reusing Zed's `language` crate would give us `Language::highlight_text(&Arc<Self>, text: &Rope, range)
-> Vec<(Range<usize>, HighlightId)>` (`$Z/crates/language/src/language.rs:1113`) — the cheapest
standalone entry point Zed has. But `crates/language/Cargo.toml` drags in `lsp`, `rpc`, `fs`,
`settings`, project-adjacent crates, `imara-diff`, `ec4rs`, `chardetng`, `encoding_rs`. **Far too
heavy for a diff viewer**, and it would tie our build to Zed's git rev for a second crate. Rejected;
see §3a.

One thing *is* worth copying verbatim: `HighlightedText`
(`$Z/crates/language/src/buffer.rs:619`), the "text + highlight ranges" bundle:

```rust
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct HighlightedText {
    pub text: SharedString,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,   // byte ranges into `text`
}
impl HighlightedText {
    pub fn to_styled_text(&self, default_style: &TextStyle) -> StyledText {   // :649
        gpui::StyledText::new(self.text.clone())
            .with_default_highlights(default_style, self.highlights.iter().cloned())
    }
}
pub struct HighlightedTextBuilder { /* :625 */ }
impl HighlightedTextBuilder {
    pub fn build(self) -> HighlightedText;                                   // :692
    pub fn push_styled(&mut self, value: impl Display, style: HighlightStyle); // :701
    pub fn push_plain(&mut self, value: impl Display);                        // :711
}
```

And `EditPreview::highlight_edits(...) -> HighlightedText` (`$Z/crates/language/src/buffer.rs:824`)
is **Zed's own inline diff renderer**: it interleaves unchanged text with runs carrying
`background_color: created_background` / `deleted_background`, and is rendered per-line with
`StyledText::with_default_highlights` at `$Z/crates/editor/src/edit_prediction.rs:2482`. That is the
closest existing analogue to what we are building, and it confirms per-line `StyledText` is the
sanctioned path — the full editor uses `shape_line` directly only because it needs invisibles,
inline elements and pixel-exact cursor mapping.

### 1d. Word-level diff and deleted/added alignment

#### Zed does word diff, with `imara-diff`, on by default

**One engine for both line- and word-level: `imara-diff` 0.2.0** (`$Z/Cargo.toml:655`,
`$Z/Cargo.lock:8786`), same `Algorithm::Histogram`, differing only in the tokenizer. `similar` 2.7.0
is present but **eval tooling only** (`$Z/crates/edit_prediction_cli`, `edit_prediction_metrics`),
never in the editor path. `diffy` is used only to *apply* patches
(`$Z/crates/language/src/text_diff.rs:311`-`:319`).

Line hunks — `$Z/crates/buffer_diff/src/buffer_diff.rs:1184` `fn compute_hunks(...)`, core at
`:1211`-`:1219`:

```rust
let input = InternedInput::new(lines(diff_base.as_ref()), lines(buffer_text.as_str()));
let mut diff = Diff::compute(Algorithm::Histogram, &input);
// Canonicalize the placement of ambiguous hunks (git's slider/indent heuristic). Without this,
// diffs of the same buffer against different base texts (e.g. HEAD vs index) can anchor the same
// logical change at different rows … hunks render as staged when they aren't, and staging or
// unstaging them corrupts the index.
diff.postprocess_lines(&input);
```

That comment is worth reading twice — it is a real correctness trap, not a cosmetic nicety. We
sidestep it entirely by taking git's own hunks rather than computing our own.

#### The word tokenizer

`$Z/crates/language/src/text_diff.rs:181`:

```rust
pub fn word_diff_ranges(old_text: &str, new_text: &str, options: DiffOptions)
    -> (Vec<Range<usize>>, Vec<Range<usize>>)
```

and the tokenizer at `:383`:

```rust
fn tokenize(text: &str, language_scope: Option<LanguageScope>) -> impl Iterator<Item = &str> {
    let classifier = CharClassifier::new(language_scope)
        .scope_context(Some(CharScopeContext::Completion));
    // … emits a token at every CharKind transition, and each distinct
    //   punctuation character as its own token …
}
```

Boundaries are `CharClassifier::kind` transitions (word / whitespace / punctuation), with **each
distinct punctuation char its own token** — and it is *language-aware* via `CharScopeContext::Completion`.
`CharClassifier` lives at `$Z/crates/language/src/buffer.rs:5861`.

Budgets (`$Z/crates/language/src/text_diff.rs:6`-`:7`):
`MAX_WORD_DIFF_LEN = 512`, `MAX_WORD_DIFF_LINE_COUNT = 8` — but **`buffer_diff` overrides the line
cap down to 5** (`$Z/crates/buffer_diff/src/buffer_diff.rs:20`).

#### The three gating conditions (this is the important part)

`$Z/crates/buffer_diff/src/buffer_diff.rs:1294`-`:1330`:

```rust
let (base_word_diffs, buffer_word_diffs) = if let Some(diff_options) = self.diff_options
    && !buffer_row_range.is_empty()
    && base_line_count == buffer_line_count                        // <- EQUAL line counts ONLY
    && diff_options.max_word_diff_line_count >= base_line_count    // <- <= 5 lines
{ /* word_diff_ranges(...) */ } else { (Vec::default(), Vec::default()) };
```

**A hunk whose old and new line counts differ gets no word highlighting at all.** Plus
`max_word_diff_len = 512` bytes. Zed's answer to "which removed line pairs with which added line" is
therefore: *only diff them at all when the counts match, and then pair by index*. Confirmed by the
tests — `$Z/crates/multi_buffer/src/multi_buffer_tests.rs:5644`
`test_word_diff_modified_lines_with_deletion_between` expects **empty** output for unequal counts.

Enabled by `word_diff_enabled`, default **`true`** (`$Z/assets/settings/default.json:1569`,
`$Z/crates/settings_content/src/language.rs:731`).

#### How word highlights are painted

`$Z/crates/editor/src/element.rs:4589` — `fn layout_word_diff_highlights(...)`:

- collects only `DisplayDiffHunk::Unfolded { word_diffs, status, .. } if status.is_modified()`,
  **filtered to the viewport** (`:4608`-`:4618`) — "A hunk stores the word diffs for its entire
  range, so without this filter a large hunk that is only partially scrolled into view would cost
  work proportional to its whole size every frame";
- sorts by `(start, end)` (`:4623`) because the display-map converter is forward-only;
- picks the colour from the **row's** status, not the hunk's (`:4636`-`:4646`):
  `Added → version_control_word_added`, `Deleted → version_control_word_deleted`.

They are pushed into `highlighted_ranges` and painted by
`$Z/crates/editor/src/element.rs:5603` `fn paint_highlights(...)` with
`line_end_overshoot = 0.15 * line_height` and **`corner_radius = Pixels::ZERO`** — word highlights
are square-cornered, unlike selections. (Pierre rounds theirs; see §2.)

#### Deleted-vs-added positioning: two modes

**Unified.** As shown in §1a, `recompute_diff_transforms_for_edit` pushes the *entire* deleted run as
one contiguous transform, then the added rows. **There is zero pairing or alignment of deleted line N
with added line N.**

**Split (the default).** RHS is the working copy with `set_show_deleted_hunks(false)`
(`$Z/crates/editor/src/split.rs:901`); LHS is the base text via `add_inverted_diff`
(`:1410`), whose rows are tagged `is_logically_deleted: true`
(`$Z/crates/multi_buffer/src/multi_buffer.rs:2872`-`:2903`).

Alignment is generated filler rows — **`$Z/crates/editor/src/display_map/block_map.rs:1305`
`fn spacer_blocks(...)`**, inner logic at `:1357`:

```rust
fn determine_spacer(
    our_wrapper: &mut dyn FnMut(Point, Bias) -> WrapRow,
    companion_wrapper: &mut dyn FnMut(Point, Bias) -> WrapRow,
    our_point: Point, their_point: Point, delta: i32, bias: Bias,
) -> (i32, Option<(WrapRow, u32)>) {
    let our_wrap = our_wrapper(our_point, bias);
    let companion_wrap = companion_wrapper(their_point, bias);
    let new_delta = companion_wrap.0 as i32 - our_wrap.0 as i32;
    let spacer = if new_delta > delta { Some((our_wrap, (new_delta - delta) as u32)) } else { None };
    (new_delta, spacer)
}
```

Crucially it compares **wrap rows**, so soft-wrapped lines of differing height stay aligned too.
Custom blocks are mirrored across sides by `balancing_block` (`:1756`), which returns `None` for
`Near`/`Replace` placements.

Spacers render as **diagonal hatching** — `$Z/crates/editor/src/element.rs:3468`
`render_spacer_block`:

```rust
let pattern_size = Self::spacer_pattern_period(f32::from(line_height) * scale, target_size * scale);
let color = cx.theme().colors().panel_background;
let background = pattern_slash(color, 2.0, pattern_size - 2.0);
```

with `fn spacer_pattern_period(line_height, target_height) -> f32` (`:3453`) choosing a period that
divides the line height an integral number of times **so the hatching does not drift between rows**.
`pub fn pattern_slash(color, width, interval) -> Background` is at `$Z/crates/gpui/src/color.rs:827`
— it exists in gpui and we can use it.

Split responsively collapses to unified — `$Z/crates/editor/src/split.rs:1431` `fn width_changed`:

```rust
let min_width = em_advance * EditorSettings::get_global(cx).minimum_split_diff_width;
self.too_narrow_for_split = min_ems > 0.0 && width < min_width;
```

with `"minimum_split_diff_width": 100` (`$Z/assets/settings/default.json:381`). The user-facing
toggle (`$Z/crates/editor/src/split.rs:500` `impl RenderOnce for DiffStyleControls`) is two
`IconButton`s using `IconName::DiffUnified`, `IconName::DiffSplit`, and a third
`IconName::DiffSplitAuto` shown when split is configured but currently collapsed — with the tooltip
`"Split when wider than {min_columns} columns"`. **That third state is a nice touch worth copying:
it explains the collapse instead of just doing it.**

### 1e. gpui v1.18.1 primitives, with signatures

Everything in this section was read from the pinned checkout. **Several of these differ from what
you probably remember** — the drift table at the end lists them.

#### `uniform_list`

`$Z/crates/gpui/src/elements/uniform_list.rs:21`. **Three args: id, count, closure. No entity.**

```rust
#[track_caller]
pub fn uniform_list<R>(
    id: impl Into<ElementId>,
    item_count: usize,
    f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>,
) -> UniformList
where R: IntoElement,
```

It presets `base_style.overflow.y = Some(Overflow::Scroll)` and `interactivity.element_id = Some(id)`
(`:31`-`:50`), so the result is already stateful — **do not call `.id()` on it.**

```rust
uniform_list.rs:622  pub fn with_width_from_item(mut self, item_index: Option<usize>) -> Self
uniform_list.rs:628  pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self
uniform_list.rs:636  pub fn with_horizontal_sizing_behavior(mut self, behavior: ListHorizontalSizingBehavior) -> Self
uniform_list.rs:653  pub fn with_decoration(mut self, decoration: impl UniformListDecoration + 'static) -> Self
uniform_list.rs:683  pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self
uniform_list.rs:690  pub fn y_flipped(mut self, y_flipped: bool) -> Self
```

**It is `with_decoration` (singular)** — `rg with_decorations crates/` → 0 hits. Callable repeatedly;
it pushes onto a `Vec`. `with_horizontal_sizing_behavior` has a side effect (`:643`-`:650`):
`FitList` ⇒ `overflow.x = None`, `Unconstrained` ⇒ `overflow.x = Some(Overflow::Scroll)`.
`track_scroll` must be called **before** `y_flipped`, which reads `self.scroll_handle`.

```rust
// uniform_list.rs:580
pub trait UniformListDecoration {
    fn compute(&self, visible_range: Range<usize>, bounds: Bounds<Pixels>,
               scroll_offset: Point<Pixels>, item_height: Pixels, item_count: usize,
               window: &mut Window, cx: &mut App) -> AnyElement;
}
// :595 — blanket impl, so an Entity<T: UniformListDecoration> works directly
impl<T: UniformListDecoration + 'static> UniformListDecoration for Entity<T> { … }
```

```rust
// uniform_list.rs:79
pub struct UniformListScrollHandle(pub Rc<RefCell<UniformListScrollState>>);
// :113
pub struct UniformListScrollState {
    pub base_handle: ScrollHandle,
    pub deferred_scroll_to_item: Option<DeferredScrollToItem>,
    pub last_item_size: Option<ItemSize>,
    pub y_flipped: bool,
}
// :83
pub enum ScrollStrategy { Top, Center, Bottom, Nearest }

:136  pub fn new() -> Self
:150  pub fn scroll_to_item(&self, ix: usize, strategy: ScrollStrategy)
:163  pub fn scroll_to_item_strict(&self, ix: usize, strategy: ScrollStrategy)
:182  pub fn scroll_to_item_with_offset(&self, ix, strategy, offset: usize)
:216  pub fn y_flipped(&self) -> bool
:222  pub fn logical_scroll_top_index(&self) -> usize    // #[cfg(any(test, feature = "test-support"))] ONLY
:231  pub fn is_scrollable(&self) -> bool
:241  pub fn is_scrolled_to_end(&self) -> Option<bool>
:252  pub fn scroll_to_bottom(&self)
```

**Gotchas:** there is **no** `logical_scroll_top`, `scroll_top`, `set_offset`/`offset`, or
`base_handle()` accessor. Reach them through the public tuple field:
`handle.0.borrow().base_handle.offset()`. And `logical_scroll_top_index()` is **test-gated** — do not
build production logic on it. Scroll requests are *deferred*: `scroll_to_item` only writes
`deferred_scroll_to_item`, consumed in the element's `prepaint`, so it takes a frame and the index is
clamped then.

The idiom for the closure is `cx.processor` (`$Z/crates/gpui/src/app/context.rs:264`):

```rust
pub fn processor<E, R>(&self, f: impl Fn(&mut T, E, &mut Window, &mut Context<T>) -> R + 'static)
    -> impl Fn(E, &mut Window, &mut App) -> R + 'static
```

#### `list` (variable height)

`$Z/crates/gpui/src/elements/list.rs:24`. **No id, no count — the state carries both.**

```rust
pub fn list(state: ListState,
            render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static) -> List
// :46
impl List { pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self }
// List has NO id() / InteractiveElement impl; Element::id() returns None (list.rs:1443)

// :314 — 3 args on this tag; no render_item, no cx
pub fn ListState::new(item_count: usize, alignment: ListAlignment, overdraw: Pixels) -> Self
// :163
pub enum ListAlignment { Top, Bottom }
// :187
pub enum ListSizingBehavior { Infer, #[default] Auto }
// :1432
pub struct ListOffset { pub item_ix: usize, pub offset_in_item: Pixels }
```

Selected methods:

```rust
:336  pub fn measure_all(self) -> Self                    // consuming builder; expensive first frame
:347  pub fn with_uniform_item_height(self, height: Pixels) -> Self   // cheap convergence hint
:355  pub fn reset(&self, element_count: usize)
:412  pub fn remeasure_items(&self, range: Range<usize>)
:503  pub fn splice(&self, old_range: Range<usize>, count: usize)
:552  pub fn set_scroll_handler(&self, handler: impl FnMut(&ListScrollEvent, &mut Window, &mut App) + 'static)
:560  pub fn logical_scroll_top(&self) -> ListOffset
:565  pub fn scroll_by(&self, distance: Pixels)
:660  pub fn scroll_to(&self, mut scroll_top: ListOffset)
:677  pub fn scroll_to_reveal_item(&self, ix: usize)
:711  pub fn bounds_for_item(&self, ix: usize) -> Option<Bounds<Pixels>>
:781  pub fn scroll_px_offset_for_scrollbar(&self) -> Point<Pixels>   // negative y
```

**No `scroll_px_position`** — the nearest is `scroll_px_offset_for_scrollbar()`.

**The tradeoff, stated plainly.** `uniform_list` measures **one** item and extrapolates: O(1) layout,
exact content height, correct scrollbar from frame one, exact `scroll_to_item`. `list` uses a
`SumTree` for per-item heights but **only measures visible + overdraw**, so total content height (and
therefore thumb size and absolute position) is an estimate that converges as you scroll. Its module
doc (`list.rs:1`-`:8`) warns that items **outside** the scrolled area must not change height without
a `splice`/`reset`/`remeasure_items`. `list` also has **no decoration hook and no `ElementId`**.

For our diff view this settles it: **keep uniform rows and stay on `uniform_list`** (§3d).

#### `ScrollHandle`

`$Z/crates/gpui/src/elements/div.rs:4055`, full public surface:

```rust
:4065  pub fn new() -> Self
:4070  pub fn offset(&self) -> Point<Pixels>            // negative y as you scroll down
:4075  pub fn max_offset(&self) -> Point<Pixels>
:4080  pub fn top_item(&self) -> usize
:4099  pub fn bottom_item(&self) -> usize
:4118  pub fn bounds(&self) -> Bounds<Pixels>
:4123  pub fn bounds_for_item(&self, ix: usize) -> Option<Bounds<Pixels>>
:4128  pub fn scroll_to_item(&self, ix: usize)
:4138  pub fn scroll_to_top_of_item(&self, ix: usize)
:4198  pub fn scroll_to_bottom(&self)
:4206  pub fn set_offset(&self, mut position: Point<Pixels>)   // NOT clamped here
:4212  pub fn logical_scroll_top(&self) -> (usize, Pixels)
:4227  pub fn logical_scroll_bottom(&self) -> (usize, Pixels)
:4242  pub fn children_count(&self) -> usize
```

**There is no public `child_bounds()`** — use `bounds_for_item(ix)` + `children_count()`.
`set_offset` writes raw; clamping happens each frame in `Interactivity::clamp_scroll_position`
(`div.rs:2325`-`:2381`), which also writes `max_offset` and `bounds` back into the handle. So
`max_offset()`/`bounds()` are only valid **after at least one layout**, and an out-of-range
`set_offset` is silently clamped next frame.

#### Scrollbars — **not in gpui**

`rg Scrollbar crates/gpui/src` matches only `list.rs`'s `*_for_scrollbar` helpers.
**There is no `gpui::Scrollbar`.** Everything lives in `$Z/crates/ui/src/components/scrollbar.rs`
(1722 lines), and the API is **not** `Scrollbar::vertical()` / `ScrollbarState::new()` /
`parent_entity()` — those names do not exist on this tag. It is a config struct + extension trait:

```rust
// scrollbar.rs:382
pub struct Scrollbars<T: ScrollableHandle = ScrollHandle> { /* private */ }
// :395
impl Scrollbars {
    pub fn new(show_along: ScrollAxes) -> Self;
    pub fn always_visible(show_along: ScrollAxes) -> Self;
}
impl<H: ScrollableHandle> Scrollbars<H> {
    pub fn id(mut self, id: impl Into<ElementId>) -> Self;                          // :425
    pub fn tracked_entity(mut self, entity_id: EntityId) -> Self;                   // :444
    pub fn tracked_scroll_handle<T: ScrollableHandle>(self, h: &T) -> Scrollbars<T>; // :449 type-changing
    pub fn show_along(mut self, along: ScrollAxes) -> Self;                          // :478
    pub fn style(mut self, style: ScrollbarStyle) -> Self;                           // :483
    pub fn reveal_policy(mut self, p: ScrollbarRevealPolicy) -> Self;                // :488
    pub fn with_track_along(mut self, along: ScrollAxes, background_color: Hsla) -> Self; // :493
}
// :297
pub enum ScrollAxes { Horizontal, Vertical, Both }
// :350, widths at :372
pub enum ScrollbarStyle { #[default] Regular /* px(6.) */, Editor /* px(15.) */ }
// :357
pub enum ScrollbarRevealPolicy { #[default] ScrollOrContentChange, ScrollOnly }

// :97 — attach via the extension trait
pub trait WithScrollbar: Sized {
    type Output;
    fn custom_scrollbars<T: ScrollableHandle>(self, config: Scrollbars<T>,
                                              window: &mut Window, cx: &mut App) -> Self::Output;
    #[track_caller]
    fn vertical_scrollbar_for<H: ScrollableHandle + Clone>(self, scroll_handle: &H,
                                              window: &mut Window, cx: &mut App) -> Self::Output;
}
impl WithScrollbar for Stateful<Div> { type Output = Self; }         // :145
impl WithScrollbar for Div          { type Output = Stateful<Div>; } // :166 — auto-assigns an id

// :1018
pub trait ScrollableHandle: 'static + Any + Sized + Clone {
    fn max_offset(&self) -> Point<Pixels>;
    fn set_offset(&self, point: Point<Pixels>);
    fn offset(&self) -> Point<Pixels>;
    fn viewport(&self) -> Bounds<Pixels>;
    fn drag_started(&self) {}
    fn drag_ended(&self) {}
    fn scrollable_along(&self, axis: ScrollbarAxis) -> bool { … }
    fn content_size(&self) -> Size<Pixels> { … }
}
// impls: UniformListScrollHandle :956, ListState :974, ScrollHandle :1000
```

`horizontal_scrollbar()` / `vertical_scrollbar()` (no-handle variants) are **commented out**
(`:111`-`:127`), and so is `impl WithScrollbar for UniformList` (`:247`-`:271`) — for a
`uniform_list` you go through the decoration path, since `ScrollbarStateWrapper<T>` implements
`UniformListDecoration` (`:225`). That is what `$Z/crates/picker/src/render.rs:247` does.

**Usability outside Zed's `ui` crate: no, not as-is.** It does `use theme::ActiveTheme as _;`
(`:16`) and paint reads `cx.theme().colors()` for `scrollbar_thumb_background`,
`scrollbar_thumb_hover_background`, `scrollbar_thumb_active_background`, `surface_background`,
`border_variant` (`:1397`, `:1420`-`:1428`, `:1473`). We must **write our own** — it is ~5 colour
lookups plus a thumb quad, and we already have `theme.colors.scroll_thumb` and
`theme.metrics.scroll_thumb_w = px(3.0)`.

The thumb recipe worth copying (`:1418`-`:1500`): an optional track quad with a 1px leading border at
`border_variant.opacity(0.6)`, then the thumb with
`Corners::all(Pixels::MAX).clamp_radii_for_quad_size(...)` for `Regular`, colour
`blending_color.blend(thumb_base_color)` capped at `MAXIMUM_OPACITY = 0.7` when not hovered, then
`fade_out(fade)` for autohide.

#### `canvas`

`$Z/crates/gpui/src/elements/canvas.rs:10`:

```rust
pub fn canvas<T>(
    prepaint: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T,
    paint:    impl 'static + FnOnce(Bounds<Pixels>, T, &mut Window, &mut App),
) -> Canvas<T>
```

Both closures are `FnOnce` and are `.take().unwrap()`-ed (`:71`, `:86`), so **a `Canvas` is
single-use per frame** — construct it fresh in every `render`. `Element::paint` wraps the closure in
`style.paint(...)`, so `Styled` background/border paint first. Real drawing example at
`$Z/crates/git_ui/src/git_graph.rs:3227`; bounds-capture-only idiom at
`$Z/crates/picker/src/picker.rs:1417`.

#### Overflow and scroll on `div()`

On **`StatefulInteractiveElement`** (`div.rs:1250`) — these require `.id()` first:

```rust
div.rs:1465  fn overflow_scroll(mut self) -> Self
div.rs:1472  fn overflow_x_scroll(mut self) -> Self
div.rs:1478  fn overflow_y_scroll(mut self) -> Self
div.rs:1487  fn restrict_scroll_to_axis(mut self) -> Self
div.rs:1492  fn track_scroll(mut self, scroll_handle: &ScrollHandle) -> Self
div.rs:1498  fn anchor_scroll(mut self, scroll_anchor: Option<ScrollAnchor>) -> Self
```

On plain `Styled` (no id needed): `overflow_hidden`, `overflow_x_hidden`, `overflow_y_hidden`
(generated by `overflow_style_methods!()`, `$Z/crates/gpui/src/styled.rs:31`).

**The scroll-state persistence rule is at `div.rs:2169`-`:2188`** and it matters:

1. `tracked_scroll_handle` set → the offset `Rc` is borrowed from **your** handle. Survives
   everything, including the element being re-created, because you own the `Rc`.
2. Else, `overflow.*` is `Scroll` **and** an `ElementId` was assigned → the offset comes from
   id-keyed frame state. Persists across frames **only while the `GlobalElementId` path is
   identical**; change the id or any ancestor id and the position resets to 0.
3. Else (`overflow_*_scroll` without an id) → `scroll_offset` stays `None` and
   `clamp_scroll_position` returns `Point::default()` (`div.rs:2378`) — **the div silently never
   scrolls.**

For a diff view that must jump to a hunk and sync two panes, **always use case 1.**

#### `SharedString`

Own crate: `$Z/crates/gpui_shared_string/gpui_shared_string.rs`, re-exported as `gpui::SharedString`.

```rust
pub struct SharedString(SmolStr);                        // :14
impl std::ops::Deref for SharedString { type Target = str; }
:27  pub const fn new_static(str: &'static str) -> Self  // const fn, free
:32  pub fn new(str: impl AsRef<str>) -> Self
:37  pub fn as_str(&self) -> &str
```

`From` impls for `&str` (`:117`, **any lifetime**, copies), `String` (`:145`), `char`, `Box<str>`,
`Arc<str>`, `&String`. **Clone cost:** backed by `SmolStr`, so it is either a memcpy of an inline
buffer (≤22 bytes, no allocation) or an `Arc` refcount bump — **never a heap copy of the text**.
Construction from an owned/borrowed string does copy once. Prefer `new_static` for literals.

#### `Hsla` — blending semantics, exactly

`$Z/crates/gpui/src/color.rs:334`. All four components are 0..=1.

```rust
:525  pub fn to_rgb(self) -> Rgba
:560  pub fn is_transparent(&self) -> bool     // a == 0.0
:565  pub fn is_opaque(&self) -> bool          // a == 1.0
:580  pub fn blend(self, other: Hsla) -> Hsla
:596  pub fn grayscale(&self) -> Self
:607  pub fn fade_out(&mut self, factor: f32)  // &mut self! returns ()
:637  pub fn opacity(&self, factor: f32) -> Self
:667  pub fn alpha(&self, a: f32) -> Self
```

Free constructors: `pub const fn hsla(h, s, l, a) -> Hsla` (`:424`), `rgb(hex: u32) -> Rgba` (`:14`),
`rgba(hex: u32) -> Rgba` (`:20`). **There is no `Hsla::mix`** — `rg 'fn mix' crates/gpui/src` → 0 hits.

**`blend` composites `other` OVER `self`. `self` is the backdrop.** (`:580`)

```rust
pub fn blend(self, other: Hsla) -> Hsla {
    let alpha = other.a;
    if alpha >= 1.0 { other }          // other opaque -> other wins entirely
    else if alpha <= 0.0 { self }
    else { Hsla::from(Rgba::from(self).blend(Rgba::from(other))) }
}
```

with the actual maths in `Rgba::blend` (`:58`):

```rust
r: (self.r * (1.0 - other.a)) + (other.r * other.a),   // g, b likewise
a: self.a,                                              // <-- backdrop's alpha is PRESERVED
```

Two things to internalise. It round-trips HSLA→RGBA→HSLA, so hue and saturation drift slightly. And
**the result's alpha is `self.a`, not a proper source-over** — this is deliberate "paint onto an
opaque backdrop" behaviour. For us that is exactly right for
`theme.colors.bg.blend(tint.opacity(0.14))`; it is **wrong** if you try to accumulate two translucent
overlays. Zed's own idiom, verbatim from `scrollbar.rs:1443`:
`let mut thumb_color = blending_color.blend(thumb_base_color);`

**`opacity(factor)` MULTIPLIES** the existing alpha (`a: self.a * factor.clamp(0., 1.)`); the doctest
at `:628` asserts `hsla(_,_,_, 0.7).opacity(0.16).a == 0.112`. **`alpha(a)` REPLACES** it.
**`fade_out(factor)` takes `&mut self`**, multiplies by the complement, and returns `()` — **it does
not chain**, which trips everyone up once.

Also available for hunk fills: `solid_background` (`:851`), `linear_gradient` (`:865`),
`linear_color_stop` (`:893`), `pattern_slash(color, width, interval)` (`:827`),
`checkerboard(color, size)` (`:841`).

#### Element and event plumbing

```rust
// $Z/crates/gpui/src/element.rs:163
pub trait Render: 'static + Sized {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}
// :179
pub trait RenderOnce: 'static {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;   // App, not Context
}
// :160 — every IntoElement gets .when() / .when_some() / .map()
impl<T: IntoElement> FluentBuilder for T {}
// div.rs:747
fn id(mut self, id: impl Into<ElementId>) -> Stateful<Self>
// div.rs:3872
pub struct Stateful<E> { pub(crate) element: E }
```

Element-local state without your own `Entity` — `$Z/crates/gpui/src/window.rs:3749`/`:3778`:

```rust
pub fn use_keyed_state<S: 'static>(&mut self, key: impl Into<ElementId>, cx: &mut App,
    init: impl FnOnce(&mut Self, &mut Context<S>) -> S) -> Entity<S>;
#[track_caller] pub fn use_state<S: 'static>(&mut self, cx: &mut App,
    init: impl FnOnce(&mut Self, &mut Context<S>) -> S) -> Entity<S>;
```

(This is how `ui`'s scrollbar stashes its own state, `scrollbar.rs:81`.)

```rust
// $Z/crates/gpui/src/interactive.rs:138
pub struct MouseDownEvent {
    pub button: MouseButton,
    pub position: Point<Pixels>,       // WINDOW coordinates
    pub modifiers: Modifiers,
    pub click_count: usize,
    pub first_mouse: bool,
}
// :485
pub struct MouseMoveEvent { pub position, pub pressed_button: Option<MouseButton>, pub modifiers }
impl MouseMoveEvent { pub fn dragging(&self) -> bool }   // :505
```

#### Overlay helpers

```rust
// $Z/crates/gpui/src/elements/deferred.rs:7
pub fn deferred(child: impl IntoElement) -> Deferred
// :25 / :92 — both exist and are identical; with_priority is the one used in-tree
impl Deferred { pub fn with_priority(mut self, priority: usize) -> Self }
```

Layout stays in place; paint is deferred until after all ancestors via
`window.defer_draw(child, element_offset, self.priority, None)` (`:65`). Higher priority paints on top.

```rust
// $Z/crates/gpui/src/elements/anchored.rs:27
pub fn anchored() -> Anchored
:40  pub fn anchor(mut self, anchor: Anchor) -> Self
:47  pub fn position(mut self, anchor: Point<Pixels>) -> Self
:54  pub fn offset(mut self, offset: Point<Pixels>) -> Self
:68  pub fn snap_to_window(mut self) -> Self
// :242  pub enum AnchoredFitMode { SnapToWindow, SnapToWindowWithMargin(Edges<Pixels>), SwitchAnchor }
```

Canonical overlay idiom, from the gpui test at `anchored.rs:311`:

```rust
deferred(
    anchored().snap_to_window().position(self.position).child(
        div().id("menu").w(px(200.)).h(px(300.))
    ),
).with_priority(1)
```

**`sticky` is not in gpui** (`rg -i sticky crates/gpui/src` → 0 hits). It is
`$Z/crates/ui/src/components/sticky_items.rs`:

```rust
pub trait StickyCandidate { fn depth(&self) -> usize; }        // :10
pub fn sticky_items<V, T>(                                      // :20
    entity: Entity<V>,
    compute_fn: impl Fn(&mut V, Range<usize>, &mut Window, &mut Context<V>) -> SmallVec<[T; 8]> + 'static,
    render_fn: impl Fn(&mut V, T, &mut Window, &mut Context<V>) -> SmallVec<[AnyElement; 8]> + 'static,
) -> StickyItems<T>
where V: Render, T: StickyCandidate + Clone + 'static;
```

`StickyItems<T>` implements `UniformListDecoration`, so it plugs straight into
`uniform_list(...).with_decoration(sticky_items(...))`. **This is the direct answer for a sticky
file/hunk header in a `uniform_list`-based diff view**: make your row type report `depth()` (0 file
header, 1 hunk header, 2 line) and it pins the drifting ancestor rows. Unlike `scrollbar.rs` its
imports (`:1`-`:8`) are only `gpui` + `smallvec` — **no theme dependency, so we can copy this file
nearly verbatim.**

`$Z/crates/ui/src/components/indent_guides.rs:210` is a second, smaller `UniformListDecoration`
example if you want a simpler template.

#### API drift table

| what you may remember | what v1.18.1 actually has |
|---|---|
| `uniform_list(entity, id, count, f)` | **`uniform_list(id, count, f)`** — no entity; use `cx.processor()` |
| `UniformList::with_decorations` | **`with_decoration`** (singular), call repeatedly |
| `UniformListScrollHandle::{logical_scroll_top, scroll_top, set_offset, offset, base_handle()}` | **none exist** — go via the public field `handle.0.borrow().base_handle` |
| `ListState::scroll_px_position` | **`scroll_px_offset_for_scrollbar()`** |
| `gpui::Scrollbar`, `Scrollbar::vertical/horizontal`, `ScrollbarState::new`, `parent_entity` | **none exist anywhere.** It is `ui::Scrollbars<T>` + `ui::WithScrollbar`; `tracked_entity(EntityId)` replaces `parent_entity`; `ScrollableHandle` lives in `ui`, not gpui |
| `ScrollHandle::child_bounds()` | **`bounds_for_item(ix)`** + `children_count()` |
| `Hsla::mix` | **does not exist** — use `blend` / `opacity` / `alpha` |
| `TextSystem::shape_line` | on **`WindowTextSystem`** — `window.text_system().shape_line(..)` |
| `StyledText::with_highlights(&style, ranges)` | **no style param** — it uses the inherited style; the styled variant is `with_default_highlights` |
| `TextState` | does not exist — `TextLayout` + `InteractiveTextState` |
| `ViewContext<T>` | does not exist — `Window` + `Context<'a, T>` + `App` |
| `gpui::sticky` | `ui::sticky_items` (copyable; gpui + smallvec only) |

---

## 2. Diffs by Pierre (diffs.com)

`@pierre/diffs` is an open-source diff and code rendering library for the web, built on Shiki, by
The Pierre Computer Company.

| | |
|---|---|
| npm | **`@pierre/diffs`** — <https://www.npmjs.com/package/@pierre/diffs> |
| version researched | **1.4.1** (`packages/diffs/package.json`) |
| licence | **Apache-2.0** |
| homepage | <https://diffs.com> |
| repo | <https://github.com/pierrecomputer/pierre> (monorepo; package at `packages/diffs`) |
| design essay | <https://pierre.computer/writing/on-rendering-diffs> |

**Practical note for anyone re-checking this:** <https://diffs.com/docs> is a single page that
exceeds 10 MB and a plain fetch fails. Use **<https://diffs.com/llms-full.txt>** (286 KB, the
complete docs in one file), with the section index at <https://diffs.com/llms.txt>. The org is
`pierrecomputer` — `github.com/pierre-co` and `pierrecodes/diffs` do not exist.

Findings below are tagged **[DOC]** (stated in the docs), **[SRC]** (read from the v1.4.1 source),
**[COMPUTED]** (calculated from a formula in the source), **[INFERRED]** (reasoning, stated nowhere).

### 2.1 Architecture and framework

Dual-target, not framework-agnostic in the usual sense. The stated design **[DOC]**:

> "browsers are rather efficient at rendering raw HTML. We lean into this by having all the lower
> level APIs purely rendering strings (the raw HTML) that are then consumed by higher-order
> components and utilities."

Low-level renderers emit HAST/HTML strings; higher-order components render into **Shadow DOM + CSS
Grid**. React peer deps exist (`react ^18.3.1 || ^19`) but the vanilla entry needs neither. Entry
points **[SRC]**: `.` (vanilla), `./react`, `./ssr`, `./worker`, plus `@pierre/diffs/edit` split out
**[DOC]** "so it can be loaded only when the feature is used".

Runtime deps **[SRC]**: `shiki`, `@shikijs/core`, `@shikijs/transformers`,
`@shikijs/engine-javascript`, **`diff`** (the diff algorithm), `hast-util-to-html`, `lru_map`.

### 2.2 Input — three ways

**a) Two blobs** **[DOC]**: `parseDiffFromFile(oldFile, newFile)` where each side is
`{ name, contents, lang?, cacheKey? }`. `null` for an added or deleted side; both `null` throws.
Rules **[DOC]**: *"An omitted side is not the same as `null`"*; *"Empty file contents are still a
real file; represent them with `contents: ''`, not `null`."* **This is the only mode that produces
`oldLines`/`newLines`, which is what makes context expansion possible.**

**b) Unified patch text** **[DOC]**: `parsePatchFiles(patchString, cacheKeyPrefix?)` →
`ParsedPatch[]`. But *"Diffs from patch files don't include oldLines/newLines"* — patch-derived
diffs are **partial** and need an async `loadDiffFiles` hook to hydrate full contents before
context can be expanded. Helper `trimPatchContext(patch, contextLines)`.

**c) Pre-parsed metadata**, or a bare file with no diff at all.

The parsed shape **[DOC]**, worth comparing against our `fleet_git::Diff`:

```typescript
interface FileDiffMetadata {
  name: string;
  prevName: string | undefined;    // renames/moves
  lang?: SupportedLanguages;
  type: ChangeTypes;               // 'change' | 'rename-pure' | 'rename-changed' | 'new' | 'deleted'
  hunks: Hunk[];
  splitLineCount: number;          // precomputed height for split layout
  unifiedLineCount: number;        // precomputed height for unified layout
  oldLines?: string[];             // only from parseDiffFromFile
  newLines?: string[];
  cacheKey?: string;
  newObjectId?, prevObjectId?, mode?, prevMode?
}
interface Hunk {
  additionCount, additionStart, additionLines: number;
  deletionCount, deletionStart, deletionLines: number;
  hunkContent: (ContextContent | ChangeContent)[];
  hunkContext: string | undefined;
  splitLineStart, splitLineCount, unifiedLineStart, unifiedLineCount: number;
}
interface ChangeContent { type: 'change'; deletions: string[]; additions: string[];
                          noEOFCRDeletions: boolean; noEOFCRAdditions: boolean }
```

**Two ideas here we should adopt.** `splitLineCount` / `unifiedLineCount` are **precomputed row
heights per file** — exactly what a virtualiser needs, and exactly what our `flatten()` recomputes
every frame. And `ChangeContent` groups a run of deletions with its run of additions **as one
object**, which is precisely the pairing unit for word diff (§3c).

### 2.3 The options object — [SRC] `packages/diffs/src/types.ts`

Defaults come from source comments; where the docs disagree, the source wins (noted below).

```typescript
export type HunkSeparators = 'simple' | 'metadata' | 'line-info' | 'line-info-basic' | 'custom';
export type LineDiffTypes  = 'word-alt' | 'word' | 'char' | 'none';
export type DiffIndicators = 'classic' | 'bars' | 'none';
export type ThemeTypes     = 'system' | 'light' | 'dark';
export type HunkLineType   = 'context' | 'expanded' | 'addition' | 'deletion' | 'metadata';

export interface BaseCodeOptions {
  theme?: DiffsThemeNames | ThemesType;
  disableLineNumbers?: boolean;
  overflow?: 'scroll' | 'wrap';    // 'scroll' default
  themeType?: ThemeTypes;          // 'system' default
  collapsed?: boolean;
  disableFileHeader?: boolean;
  stickyHeader?: boolean;
  useCSSClasses?: boolean;
  tokenizeMaxLineLength?: number;
  unsafeCSS?: string;
}
export interface BaseDiffOptions extends BaseCodeOptions {
  diffStyle?: 'unified' | 'split';        // split default
  diffIndicators?: DiffIndicators;        // bars default
  disableBackground?: boolean;
  hunkSeparators?: HunkSeparators;        // 'line-info' default   [SRC only]
  expandUnchanged?: boolean;              // false default
  loadDiffFiles?: FileDiffContentsLoader;
  collapsedContextThreshold?: number;     // 2 default   [SRC; docs say 1 — trust source]
  lineDiffType?: LineDiffTypes;           // 'word-alt' default
  maxLineDiffLength?: number;             // 1000 default
  expansionLineCount?: number;            // 100 default
}
```

Point by point:

- **Unified vs split:** `diffStyle`, **default `'split'`**. Split is CSS Grid
  `grid-template-columns: 1fr 1fr`, each side with its own gutter+content subgrid. **Note both Zed
  and Pierre default to split.**
- **`lineDiffType` default is `'word-alt'`** — **[DOC]** it *"attempts to join word regions that are
  separated by a single character"*, avoiding shredded highlights. `maxLineDiffLength: 1000` skips
  intra-line diffing on very long lines. This "join across a single separator" post-pass is the
  single most valuable behavioural detail on this page; see §3c.
- **Line numbers:** one `disableLineNumbers` boolean. **No separate old/new option** — in split each
  side has its own gutter; in unified there is one.
- **Wrapping:** `overflow: 'scroll' | 'wrap'`, **default `'scroll'`**.
- **Expand context:** `expansionLineCount` = lines per click (100); `collapsedContextThreshold` =
  auto-expand gaps at or below this size (2); `expandUnchanged` forces everything. Programmatic
  `expandHunk(hunkIndex, 'up' | 'down' | 'both', lineCount?)`;
  `expandHunk(0, 'both', Number.POSITIVE_INFINITY)` expands all.
- **Separators [DOC]:** `line-info` = *"Rounded corner separator with collapsed line count and
  expansion controls"*; `line-info-basic` = compact full-width variant with controls; `metadata` =
  *"Patch-style separator (`@@ -x,y +a,b @@`) with no expansion controls"*; `simple` = minimal bar.
- **File header:** change icon, path, `[data-prev-name]` for renames, and
  `[data-additions-count]`/`[data-deletions-count]` coloured green/red. `disableFileHeader` is
  **all-or-nothing** — no per-element toggles; customise via `renderHeaderPrefix`,
  `renderHeaderFilenameSuffix`, `renderHeaderMetadata`, `renderCustomHeader`.
- **Collapsed large diffs:** `collapsed` hides the body, keeps the header.
- **Renames:** first class — `type: 'rename-pure' | 'rename-changed'`, `prevName`, `prevMode`,
  distinct header icons; `loadDiffFiles` returns `{ oldFile: null, newFile }` for pure renames.
- **Binary files: NOT SUPPORTED [SRC].** Every "binary" hit in the source is a binary *search*.
  There is no binary `ChangeTypes` value, option or renderer. A real gap — and one we already handle
  better, since `fleet_git::DiffFile::binary` exists.
- **Light/dark:** `themeType` default `'system'`; `theme` takes a name **or** `{ dark, light }`. The
  CSS uses `color-scheme: light dark` + `light-dark()`, so **light/dark switches with no class
  toggling**.

### 2.4 Syntax highlighting engine — it is Shiki, throughout

**[SRC]** Import counts across `packages/diffs/src`: 9× `shiki`, 4× `shiki/core`, 1× `shiki/textmate`,
1× `shiki/engine/oniguruma`, 1× `shiki/engine/javascript`, 1× `@shikijs/transformers`.

Two things that sound like a custom engine but are not:

- **"AST" means HAST** (the standard unist/rehype hypertext AST); the dep is `hast-util-to-html`. The
  "AST LRU cache" caches Shiki's rendered HAST keyed by `cacheKey`.
- **`tokenizeMaxLineLength` is a Shiki guard**, not a custom-tokenizer knob — it skips highlighting
  on lines longer than N (default 1000).

Edit mode *does* have its own incremental tokenizer (`src/editor/tokenizer.ts`) — but it drives
**Shiki's TextMate grammars directly**, importing `IGrammar`, `StateStack`, `INITIAL`,
`EncodedTokenMetadata` from `shiki/textmate`, with a `TOKENIZE_TIME_LIMIT = 500` ms budget.

**This matters for §3a.** Pierre — a library whose entire purpose is beautiful diffs — uses
**TextMate grammars with per-line incremental tokenisation and a time budget**, not tree-sitter.
That is the same architecture syntect gives us in Rust.

Engine choice: `preferredHighlighter: 'shiki-js' | 'shiki-wasm'`, default **`'shiki-js'`** (JS regex
engine); WASM Oniguruma is opt-in. The diff algorithm itself is the npm `diff` package.

### 2.5 Visual design — quoted from `packages/diffs/src/style.css`

**[SRC]** 1843 lines, layered `@layer base, theme, rendered, unsafe;`

**The four semantic base colours — verbatim, lines 24–31:**

```css
--diffs-added-light:    #0dbe4e;   --diffs-added-dark:    #5ecc71;
--diffs-modified-light: #009fff;   --diffs-modified-dark: #69b1ff;
--diffs-deleted-light:  #ff2e3f;   --diffs-deleted-dark:  #ff6762;
--diffs-warning-light:  #d5a910;   --diffs-warning-dark:  #ffd452;
```

**Every other diff colour is derived at runtime** from those plus the theme background:

```css
--diffs-bg-addition: var(--diffs-bg-addition-override,
  light-dark(
    color-mix(in lab, var(--diffs-bg) 88%, var(--diffs-addition-base)),
    color-mix(in lab, var(--diffs-bg) 80%, var(--diffs-addition-base))));

--diffs-bg-addition-emphasis: var(--diffs-bg-addition-emphasis-override,
  light-dark(
    rgb(from var(--diffs-addition-base) r g b / 0.15),
    rgb(from var(--diffs-addition-base) r g b / 0.2)));
```

with a source comment explaining the colour space:

```css
/* NOTE(amadeus): we cannot use 'in oklch' because current versions of cursor
 * and vscode use an older build of chrome that appears to have a bug with
 * color-mix and 'in oklch', so use 'in lab' instead */
```

**The gutter gets a lighter tint than the code row** — verbatim, ~456–468:

```css
:where([data-background]) {
  [data-gutter-buffer], [data-column-number] { --mix-light: 91%; --mix-dark: 85%; }
  [data-line], [data-no-newline]             { --mix-light: 88%; --mix-dark: 80%; }
}
```

**The word-level highlight mechanism — verbatim, 1473–1484. This is the single most copyable idea
in the whole stylesheet:**

```css
[data-diff-span] {
  border-radius: 3px;
  box-decoration-break: clone;
}
[data-line-type='change-addition'] [data-diff-span] {
  background-color: var(--diffs-bg-addition-emphasis);
}
[data-line-type='change-deletion'] [data-diff-span] {
  background-color: var(--diffs-bg-deletion-emphasis);
}
```

Changed words get a **rounded 3 px pill** tinted on top of the already-tinted row — a genuinely
stronger inner highlight, not just a darker shade. (Contrast Zed, which paints word highlights
square-cornered with `corner_radius = Pixels::ZERO`.)

**Resolved values [COMPUTED]** — the researcher implemented CSS Lab (D50) mixing to resolve the
formulas above; these are computed, not literals. Pierre theme backgrounds are `#ffffff` / `#0a0a0a`
**[SRC]** `packages/theme/themes/*.json`.

| | Light (bg `#ffffff`) | Dark (bg `#0a0a0a`) |
|---|---|---|
| Added row bg | `#e9f8e9` | `#1e2e1f` |
| Removed row bg | `#ffeae6` | `#371f1d` |
| Added gutter bg | `#eefaef` | `#1a251b` |
| Removed gutter bg | `#ffefec` | `#2c1b19` |
| **Added word emphasis** | **`#c8efd2`** | **`#2b4e2f`** |
| **Removed word emphasis** | **`#ffcecd`** | **`#5f2d2b`** |
| `--diffs-bg-context` | `#fbfbfb` | `#1c1c1c` |
| `--diffs-bg-separator` | `#f3f3f3` | `#2b2b2b` |
| `--diffs-fg-number` | `#525252` | `#a0a0a0` |

**Typography and spacing — verbatim:**

```css
--diffs-font-fallback:
  'SF Mono', Monaco, Consolas, 'Ubuntu Mono', 'Liberation Mono', 'Courier New', monospace;
--diffs-header-font-fallback:
  system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', 'Noto Sans', 'Liberation Sans', Arial, sans-serif;

font-size:   var(--diffs-font-size, 13px);
line-height: var(--diffs-line-height, 20px);
--diffs-gap-fallback: 8px;
--diffs-scrollbar-gutter-fallback: 6px;
tab-size: var(--diffs-tab-size, 2);
```

**Row height 20 px, font 13 px mono.** Note the header and separators are deliberately
**non-monospace** (system UI stack) and reset `font-family` explicitly because they sit inside
`<pre>`. Our `theme.text.data` is 12.5/18 mono and `ROW_H = 18.0` — very close.

**Gutter — verbatim:**

```css
[data-column-number] {
  box-sizing: content-box;
  text-align: right;
  user-select: none;
  color: var(--diffs-fg-number);
  padding-left: 2ch;
}
[data-line-number-content] {
  display: inline-block;
  min-width: var(--diffs-min-number-column-width, var(--diffs-min-number-column-width-default, 3ch));
}
```

The gutter width **auto-sizes to the digit count**: `--diffs-min-number-column-width-default` is set
inline per file to `` `${totalLines}`.length + 'ch' `` **[SRC]** `utils/createPreElement.ts:42`,
fallback `3ch`. **Our `gutter_width()` is a hard-coded `ch(9.0)`** — auto-sizing is a small win.

**Borders and sticky behaviour — verbatim:**

```css
[data-gutter] {
  display: grid; grid-template-rows: subgrid; grid-template-columns: subgrid;
  grid-column: 1; z-index: 3; background-color: var(--diffs-bg);
  [data-gutter-buffer], [data-column-number] {
    border-right: var(--diffs-gap-style, 2px solid var(--diffs-bg));
  }
}
[data-overflow='scroll'] [data-gutter] { position: sticky; left: 0; }
```

The gutter/content divider is **2 px of the background colour** — it reads as a gap, not a rule.
**[INFERRED]** deliberately, so the divider disappears into any theme. And in horizontal-scroll mode
the gutter is `position: sticky; left: 0` — **line numbers stay pinned while code scrolls.** Our
current renderer gets this right by accident (the gutters sit outside the shifted payload).

**Radii: only four values in 1843 lines** — `3px` (the diff-span pill), `4px`, `6px`. Very restrained.

Header — verbatim:

```css
[data-diffs-header='default'] {
  position: relative; background-color: var(--diffs-bg);
  display: flex; justify-content: space-between; align-items: center;
  gap: var(--diffs-gap-inline, var(--diffs-gap-fallback));
  min-height: calc(1lh + (var(--diffs-gap-block, var(--diffs-gap-fallback)) * 3));
  padding-inline: 16px; top: 0; z-index: 2;
}
[data-diffs-header='default'] [data-additions-count] { color: var(--diffs-addition-base); }
[data-diffs-header='default'] [data-deletions-count] { color: var(--diffs-deletion-base); }
[data-change-icon='new']     { color: var(--diffs-addition-base); }
[data-change-icon='deleted'] { color: var(--diffs-deletion-base); }
[data-change-icon='change'], [data-change-icon='rename-pure'],
[data-change-icon='rename-changed'] { color: var(--diffs-modified-base); }
```

Separators: `[data-separator='line-info' | 'line-info-basic' | 'metadata'] { height: 32px; }`;
`[data-separator='simple'] { min-height: 4px; }`. Expand buttons `min-width: 32px; cursor: pointer`.

Indicators — `'classic'` uses pseudo-content so the `+`/`−` is unselectable by construction:

```css
[data-indicators='classic'] [data-line] { padding-inline-start: 2ch; }
[data-indicators='classic'] [data-line-type='change-addition']::before {
  content: '+'; color: var(--diffs-addition-base);
  display: inline-block; width: 1ch; height: 1lh;
  position: absolute; top: 0; left: 0; user-select: none;
}
```

`'bars'` (the default) draws a 4 px bar on the number column with a line-height-snapped dash:

```css
background-image: linear-gradient(0deg, var(--diffs-bg-deletion) 50%, var(--diffs-deletion-base) 50%);
background-size: calc(1lh / round(1lh / 2px)) calc(1lh / round(1lh / 2px));
```

Note the `round(1lh / 2px)` — the same trick as Zed's `spacer_pattern_period`: snap the pattern
period so it divides the row height and does not drift.

Theming API: ~40 override variables listed in a comment block at `style.css:34`-`:78`
(`--diffs-bg-addition-emphasis-override`, `--diffs-fg-number-override`, `--diffs-gap-style`,
`--diffs-tab-size`, `--diffs-min-number-column-width`, …). The `unsafeCSS` escape hatch is injected
into `@layer unsafe` with an explicit **[DOC]** warning that backwards compatibility is not
guaranteed even in patch versions.

### 2.6 Keyboard, mouse, and clean-copy

**Read-only viewing has NO keyboard handling at all [SRC].** A grep of
`managers/InteractionManager.ts` for `keydown` / `keyup` / `KeyboardEvent` / `tabIndex` / `role=`
returns **zero hits**. No arrow-key line navigation, no next-hunk key, no focus management, no ARIA
in the read-only diff view. The docs are silent on it too, so this is a genuine gap, not merely
undocumented. **We are already far ahead here** — lazygit keybindings are the whole point of our view.

Every shortcut Pierre has is **edit-mode only**, from `packages/diffs/src/editor/command.ts:124`-`:177`
**[SRC]**. Worth knowing: **the published docs contain no shortcut table at all** — the
`### Keyboard shortcuts` heading is followed by three lines of prose, so the source list is the only
complete record that exists anywhere.

```js
Tab: 'indent',                        'shift+Tab': 'outdent',
'cmdOrCtrl+[': 'indentLess',          'cmdOrCtrl+]': 'indentMore',
'cmdOrCtrl+z': 'undo',                'cmdOrCtrl+shift+z': 'redo',
'cmdOrCtrl+a': 'selectAll',           'cmdOrCtrl+d': 'findNextMatch',
'cmdOrCtrl+f': 'openSearchPanel',     'cmdOrCtrl+alt+f': 'openSearchReplacePanel',
'alt+ArrowUp': 'moveLineUp',          'alt+ArrowDown': 'moveLineDown',
'shift+alt+ArrowUp': 'copyLineUp',    'shift+alt+ArrowDown': 'copyLineDown',
Escape: 'simplifySelection',          'cmdOrCtrl+Enter': 'insertBlankLine',
'cmdOrCtrl+/': 'toggleComment',       'shift+alt+a': 'toggleBlockComment',
'cmdOrCtrl+Home': 'moveCursorToDocStart',  'cmdOrCtrl+End': 'moveCursorToDocEnd',
// mac: ctrl+k deleteHardLineForward, ctrl+alt+p/n moveLineUp/Down, cmd+Up/Down doc start/end
// windows/linux: ctrl+y redo
```

**Mouse [DOC]:** line selection is opt-in (`enableLineSelection`, **default `false`** [SRC]
`InteractionManager.ts:663`) with click-and-drag on the number column, firing
start/change/end/selected callbacks carrying `{ start, end, side, endSide }` where side is
`'additions' | 'deletions'` so it works in split view. `lineHoverHighlight: 'disabled' | 'both' |
'number' | 'line'` (default `'disabled'`). Hover CSS is wrapped in `@media (pointer: fine)` **[SRC]**
so it does not fire on touch. Gutter utility exposes the hovered line via a **getter**,
`getHoveredLine()`, with **[DOC]** *"This is NOT reactive - render is not called on every mouse
move."* CodeView **disables pointer events on content while scrolling** by default.

**Clean-copy — the best finding on this page, and undocumented [SRC].** There is **no custom
clipboard handler**. Instead `user-select: none` is applied to everything that is not code — at
lines 725, 912, 981, 1086, 1423, 1561: `[data-column-number]` (line numbers), `[data-gutter-buffer]`,
`[data-content-buffer]`, `[data-separator-wrapper]`, `[data-separator-content]` (all hunk
separators), `[data-no-newline]`, and the classic `+`/`−` indicators are `::before` pseudo-content
so unselectable by construction.

**Net effect: drag-select across a diff, copy, and you get clean code — no line numbers, no gutter,
no `@@` separators, no `+`/`−` prefixes — with zero JavaScript.** The docs say nothing about copy or
selection behaviour; this is an emergent property of the CSS.

### 2.7 Virtualization and performance

Two tiers **[DOC]**. **`CodeView`** is the recommended path when the scroll region is only code:
*"it owns the entire code region, only renders what you can actually see, and is generally more
performant and less prone to blanking."* It gives virtualization, measured-layout reconciliation,
sticky headers, viewer-wide selection, and `scrollTo` by item / line / range / position — resolved
against **live measured layout**, so targets survive reflow. Non-virtualized
`renderCodeViewHeader` / `renderCodeViewFooter` regions are *"ideal for PR summary cards and
approval bars"*.

Its data model deliberately avoids deep equality: every item needs a stable `id`, and **you must bump
`version`** when content, annotations, `collapsed`, or `edit` change — *"an intentional escape hatch
to avoid potentially expensive deep object equality checks."* Compare our situation: `flatten()`
re-runs every frame precisely because there is no version/identity to compare (§0.2 item 2).

The low-level **`Virtualizer`** costs more: *"every top-level file or diff container stays mounted,
and the experience is more likely to blank during fast scroll."* Defaults **[DOC]**:
`overscrollSize: 1000` px above/below viewport, `intersectionObserverMargin: 4000` px.

Worker pool **[DOC]** (experimental): `poolSize` default **8**, `totalASTLRUCacheSize` default **100
per cache**. Gotcha: *"When using the worker pool, the `theme`, `lineDiffType`,
`tokenizeMaxLineLength`, and `useTokenTransformer` render options are controlled by
`WorkerPoolManager`, not individual components. Passing these options into component instances will
be ignored."* Render caching is keyed by `cacheKey`, and *"The `cacheKey` must change whenever the
content changes!"* — use commit SHAs / file IDs, never file contents or bare filenames.

The architecture essay (<https://pierre.computer/writing/on-rendering-diffs>) states the goal —
*"You should be able to just render any diff"* — and the techniques: **inverse-sticky
virtualization** (negative sticky offsets of `(contentHeight - viewportHeight) * -1` so the rendered
region sticks to the viewport edge instead of blanking when JS falls behind, preserving native
scrolling); estimated heights refined with binary-searchable position→line checkpoints;
**string detachment** to release the source patch (Linux v6→v7 diff: **2.4 GB → 1.15 GB**, ~80%
faster parse); Shadow-DOM element pooling; deferred worker highlighting that shows plain text first;
`overflow-anchor: none` with a self-managed scroll anchor. Acknowledged limits: CSS paint cost during
aggressive scroll, and **no horizontal virtualization** (minified long lines still hurt).

### 2.8 Gaps in Pierre, stated honestly

- **No binary file rendering** (verified absent in source).
- **No keyboard navigation or ARIA in read-only mode** (verified absent in `InteractionManager`).
- **No horizontal virtualization** (author-acknowledged).
- Docs/source conflict on `collapsedContextThreshold` (docs 1, source 2 — trust source).
- Default `hunkSeparators` never stated in docs (`'line-info'` is source-only).
- No keyboard shortcut table in the docs; no documented copy/selection semantics.
- No per-element header toggles — only all-or-nothing `disableFileHeader` plus render callbacks.
- No SSR path for `CodeView`; edit-state retention does not survive reload; undo history is not
  serializable; remote carets are render-only (*"Diffs does not synchronize documents, edits,
  selections, or presence"*).

### 2.9 The three ideas to steal regardless

1. **Derive all diff tints from the theme background** by mixing, rather than hardcoding, so any
   theme works and light/dark needs no branching.
2. **A rounded 3 px pill (`border-radius: 3px` + `box-decoration-break: clone`) on word-level spans**,
   with a stronger alpha tint layered over the already-tinted row.
3. **Make everything that is not code unselectable**, for free clean-copy with zero code.

---

## 3. Recommendation for this codebase

### 3a. Syntax highlighting: pick `syntect` + `two-face`

All numbers below were **measured on this machine** (M-series, 15 cores, release build, rustc
1.94.1) by the research pass, not quoted from documentation.

#### The three options

**(i) `syntect` 5.3.0 + `two-face` 0.5.2** — TextMate grammars, pure Rust.

- `syntect` **5.3.0** (2025-09-27), MIT. Maintained but slow-moving (5.2.0 → 5.3.0 was ~19 months).
- To stay pure Rust: `default-features = false, features = ["default-fancy"]`. That selects
  `regex-fancy` → `fancy-regex ^0.16.2` and **drops `onig`/`onig_sys` entirely** (verified by
  building it). The alternative `default-onig` pulls a C dependency.
- `two-face` **0.5.2+bat-0.26.1** (2026-08-07), MIT OR Apache-2.0, MSRV 1.79, actively maintained
  (5 releases in 9 months, tracks bat versions in the metadata suffix). Repo has moved to
  **Codeberg**: <https://codeberg.org/CosmicHarper/two-face>. It adds bat's curated grammars on top
  of syntect's 75 → **213 syntaxes measured**, including TypeScript/TSX, TOML and Dockerfile. Its
  only deps are `serde`, `serde_derive`, and syntect with `default-features = false`.
  - Feature-flag trap: `two-face`'s `default = ["syntect-onig"]`, and there are two similar pairs —
    `syntect-fancy` only sets `syntect/regex-fancy`, while `syntect-default-fancy` also enables
    `syntect/default-fancy`. Use `default-features = false, features = ["syntect-fancy"]`.

Measured load and throughput:

| call | time | result |
|---|---|---|
| `SyntaxSet::load_defaults_newlines()` (fancy) | **438 µs** | 75 syntaxes |
| `SyntaxSet::load_defaults_newlines()` (onig) | 557 µs | 75 syntaxes |
| `ThemeSet::load_defaults()` | 310 µs | 7 themes |
| `two_face::syntax::extra_newlines()` | **1.38 ms** | 213 syntaxes |

| per-line highlight (70-char Rust line, ~36 spans) | µs/line |
|---|---|
| `regex-fancy` (pure Rust) | **93.7** |
| `regex-onig` (C oniguruma) | 33.3 |

**(ii) tree-sitter per language.** `tree-sitter` **0.27.0** and `tree-sitter-highlight` **0.27.0**
(both 2026-08-30, MIT, edition 2024, **MSRV 1.90** — above our declared 1.97.1 floor, so fine).

**One premise in the original question needs correcting: the ABI churn is no longer painful.** Every
current grammar crate depends only on **`tree-sitter-language ^0.1`**, a 5 KB zero-dependency shim —
not on the runtime — so there is no version-unification conflict. Verified empirically: `tree-sitter
0.27.0` + 17 grammar crates spanning 0.23.x–0.25.x compiled and ran clean in one binary. Runtime
constants confirm the window: `LANGUAGE_VERSION = 15`, `MIN_COMPATIBLE_LANGUAGE_VERSION = 13`;
observed grammar ABIs were 14–15. The 0.20↔0.22 pain was real and was fixed by the
`tree-sitter-language` split around 0.23. The **one** genuine casualty is `tree-sitter-toml 0.20.0`
(last published 2022-01-05), which depends on `tree-sitter ^0.20` directly — use
**`tree-sitter-toml-ng` 0.7.0** instead.

The residual papercuts are naming inconsistency across crates (`HIGHLIGHTS_QUERY` vs
`HIGHLIGHT_QUERY` vs `HIGHLIGHT_QUERY_BLOCK`) and missing consts (`tree-sitter-typescript` has **no**
`INJECTIONS_QUERY`), so you write a per-language match arm regardless.

```rust
impl HighlightConfiguration {
    pub fn new(language: Language, name: impl Into<String>,
               highlights_query: &str, injection_query: &str, locals_query: &str)
        -> Result<Self, QueryError>;                    // 5 args — no trailing bool in 0.27
    pub fn configure(&mut self, recognized_names: &[impl AsRef<str>]);
}
impl Highlighter {
    pub fn highlight<'a>(&'a mut self, config: &'a HighlightConfiguration, source: &'a [u8],
        encoding: Option<u32>, cancellation_flag: Option<&'a AtomicUsize>,
        injection_callback: impl FnMut(&str) -> Option<&'a HighlightConfiguration> + 'a,
    ) -> Result<impl Iterator<Item = Result<HighlightEvent, Error>> + 'a, Error>;
}
pub enum HighlightEvent {
    Source { start: usize, end: usize },
    HighlightStart(Highlight),   // Highlight(pub usize) — index into recognized_names
    HighlightEnd,
}
```

Measured: **9.2 µs/line** — ~10× faster than syntect-fancy, ~3.6× faster than syntect-onig. But note
the shape: `highlight` takes a **whole document** and returns a document-wide event stream. **There
is no per-line entry point and no resumable state.** You cannot cheaply highlight only rows
4000–4060 of a file.

Build cost, measured on clean `cargo build --release`:

| config | wall | CPU | binary Δ |
|---|---|---|---|
| empty baseline | 0.1 s | — | 404 KB |
| `syntect` fancy + `two-face` | **5.8 s** | 46 s | **+1.05 MB** |
| `syntect` onig (defaults) | 7.7 s | 36 s | +437 KB |
| `tree-sitter` + `tree-sitter-highlight` + **17 grammars** | 8.4 s | 41 s | **+6.05 MB** |
| `similar` + `imara-diff` | 7.4 s | 14 s | +114 KB |

That tree-sitter figure is ~65 MB of generated C through `cc` — 41 CPU-seconds on every CI cache
miss, ~6 MB of binary, for ~15 languages versus two-face's 213. `cpp` (17.5 MB of C), `ruby`
(15.2 MB) and `bash` (10.0 MB) alone are half of it.

**(iii) Reuse Zed's `language` crate.** Rejected. It gives us
`Language::highlight_text(&Arc<Self>, &Rope, range) -> Vec<(Range<usize>, HighlightId)>`
(`$Z/crates/language/src/language.rs:1113`), which is genuinely the nicest API of the three, but
`crates/language/Cargo.toml` drags in `lsp`, `rpc`, `fs`, `settings`, project-adjacent crates,
`imara-diff`, `ec4rs`, `chardetng`, `encoding_rs`. It would also pin a second crate to Zed's git rev
and to the tree-sitter git rev underneath it. Far too heavy, and it couples our build to Zed's
release cadence for something we can do in 80 lines.

**Bundle crates** were checked too, since "many grammars, one dependency" is the real appeal of
tree-sitter:

| crate | latest | verdict |
|---|---|---|
| `lumis` | **0.13.1** (2026-09-03) | The live option; ~90 grammars behind `lang-*` features. API is formatter-oriented (`highlight(source, formatter) -> String`) but re-exports `lumis_core::events`. Span-level API **not verified**. |
| `autumnus` | 0.9.0 | **DEPRECATED** — description literally says "Use `lumis` instead". |
| `syntastica` | 0.6.1 (2025-06-19) | **Best-shaped output** (see below) but stale, MPL-2.0, pinned to `tree-sitter ^0.25.2`, and `syntastica-parsers` pins **exact `=` versions** of every grammar crate — conflicts with any direct grammar dep. |
| `inkjet` | 0.11.1 | **Dead** — GitHub repo archived; 24.5 MB crate. |
| `tree-sitter-language-pack` | 1.16.1 | 371 languages, very active, but **architecturally wrong for a shipped desktop app**: `default = ["dynamic-loading", "download"]` either downloads a tarball from GitHub releases at build time or `dlopen`s `.so` grammars at runtime. Pulls `ureq`, `zstd`, `tar`, `sha2`. |

`syntastica`'s output type deserves a mention because it is the shape we want:
`pub type Highlights<'src> = Vec<Vec<(&'src str, Option<&'static str>)>>` — **already split per
line**, each line a `Vec<(text, token_name)>`, which maps onto gpui text runs with no
post-processing. If it were maintained and not MPL, it would be a serious contender. It is not.

#### Recommendation: `syntect` 5.3.0 (`default-fancy`) + `two-face` 0.5.2 (`syntect-fancy`)

Five reasons, in order of weight:

1. **Only syntect supports genuinely lazy per-visible-line highlighting.** `ParseState`,
   `HighlightState` and `ScopeStack` all derive `Clone`, and **cloning the `(ParseState,
   HighlightState)` pair measured 48 ns.** So: keep a checkpoint every ~100 lines, clone the nearest
   preceding one, replay forward. Worst case on a scroll jump is 99 × 93.7 µs ≈ 9.3 ms, paid only on
   the jump; sequential scrolling reuses the running state for free. `HighlightLines::from_state` /
   `.state()` exist precisely for this. tree-sitter cannot do it — no resumable state, no per-line
   entry point.
2. **Pure Rust, no C.** `default-fancy` eliminates `onig_sys`. tree-sitter compiles ~65 MB of C.
3. **213 languages for +1.05 MB and 5.8 s of build**, versus ~15 for +6.05 MB and 41 CPU-seconds.
   For a git client that must render *whatever* the repo contains, breadth is the feature.
4. **It is what the reference implementation does.** Pierre — a library whose only job is beautiful
   diffs — runs Shiki's TextMate grammars with per-line incremental tokenisation and a time budget
   (§2.4). Same architecture.
5. **Sub-millisecond asset load** (438 µs / 1.38 ms) means a `OnceLock` on first use is fine even on
   the main thread.

The API to use:

```rust
// syntect::parsing
impl ParseState {
    pub fn new(syntax: &SyntaxReference) -> ParseState;
    pub fn parse_line(&mut self, line: &str, syntax_set: &SyntaxSet)
        -> Result<Vec<(usize, ScopeStackOp)>, ParsingError>;
}
// syntect::highlighting
impl HighlightState { pub fn new(highlighter: &Highlighter<'_>, initial_stack: ScopeStack) -> HighlightState; }
impl<'a> Highlighter<'a> { pub fn new(theme: &'a Theme) -> Highlighter<'a>; }
// THE ONE TO USE: yields byte ranges, so you can build HighlightStyle runs
// against the original line string instead of re-measuring substrings.
impl<'a,'b> RangedHighlightIterator<'a,'b> {   // Iterator<Item = (Style, &'b str, Range<usize>)>
    pub fn new(state: &'a mut HighlightState, changes: &'a [(usize, ScopeStackOp)],
               text: &'b str, highlighter: &'a Highlighter<'_>) -> Self;
}
// two-face
pub fn two_face::syntax::extra_newlines() -> syntect::parsing::SyntaxSet;
```

**Use `RangedHighlightIterator`, not `HighlightLines::highlight_line`.** The latter wraps
`HighlightIterator` and throws the byte ranges away, which is exactly what we need to build
`(Range<usize>, HighlightStyle)` pairs for `StyledText`.

Thread-safety was verified by compiling actual `Send`/`Sync` probes, not read from docs:
`SyntaxSet` (both fancy **and** onig), `ParseState`, `Theme`, `HighlightState`,
`tree_sitter::{Parser, Tree, Language}` and `tree_sitter_highlight::{Highlighter, HighlightConfiguration}`
are **all `Send + Sync`**. Nothing here forces a `!Send` design; everything can live behind a
`OnceLock`/`Arc` shared with a background executor.

Two syntect caveats to respect: `parse_line` needs **one line contiguous in memory**, and syntect's
own docs note Sublime avoids pathological cost by simply not highlighting very long lines. **Cap
line length (~1000 chars, matching Pierre's `tokenizeMaxLineLength`) and fall back to plain text** —
minified JS in a diff will otherwise hurt. Also: with `*_newlines` syntax sets you must pass lines
**with** their trailing `\n`.

If 93.7 µs/line ever hurts in practice, switching `regex-fancy` → `regex-onig` is a one-line feature
change for a measured **2.8×**, at the cost of the `onig_sys` C build. Do not do it pre-emptively.

### 3b. Intra-line diff: pick `similar`

**`similar` 3.2.0** (2026-08-17), Apache-2.0, edition 2024, MSRV 1.85. Very actively maintained —
five releases in 2026. Enable `features = ["inline", "unicode"]`.

**`similar` 3.x is much newer than most write-ups assume**, and the changes matter for a git client:
3.0.0 (2026-04-01) added `Algorithm::Histogram` (git's algorithm), `Algorithm::Hunt`, and
`InlineChangeOptions`/`InlineChangeMode` with semantic cleanup; 3.2.0 added `WhitespaceMode`
(`git diff -b`/`-w` equivalence) and three-way merge, and changed `Algorithm::Myers` to git-style
bounded non-minimal splits. **3.1.1/3.1.2 fixed real `DiffOp` range panics — do not pin below 3.1.2.**

The API that makes it the right answer:

```rust
pub enum Algorithm { Myers, RawMyers, Patience, Lcs, Hunt, Histogram }   // default Myers
pub enum ChangeTag { Equal, Delete, Insert }

impl<'old,'new,'bufs> TextDiff<'old,'new,'bufs, str> {
    pub fn from_lines<Old, New, T>(old: Old, new: New) -> TextDiff<'old,'new,T>;
    pub fn from_words<Old, New, T>(old: Old, new: New) -> TextDiff<'old,'new,T>;
    pub fn from_chars<Old, New, T>(old: Old, new: New) -> TextDiff<'old,'new,T>;
    #[cfg(feature = "unicode")]
    pub fn from_unicode_words<Old, New, T>(old: Old, new: New) -> TextDiff<'old,'new,T>;
    #[cfg(feature = "unicode")]
    pub fn from_graphemes<Old, New, T>(old: Old, new: New) -> TextDiff<'old,'new,T>;
}
impl<'old,'new,T: DiffableStr + ?Sized> TextDiff<'old,'new,T> {
    pub fn ratio(&self) -> f32;                                    // 0.0..=1.0 — the pairing heuristic
    pub fn ops(&self) -> &[DiffOp];
    pub fn iter_all_changes(&self) -> impl Iterator<Item = Change<&T>> + '_;
    #[cfg(feature = "inline")]
    pub fn iter_inline_changes(&self, op: &DiffOp) -> impl Iterator<Item = InlineChange<'_,T>> + '_;
    #[cfg(feature = "inline")]
    pub fn iter_inline_changes_with_options_deadline(&self, op: &DiffOp,
        options: InlineChangeOptions, deadline: Option<Instant>)
        -> impl Iterator<Item = InlineChange<'_,T>> + '_;
}
impl<'s,T: DiffableStr + ?Sized> InlineChange<'s,T> {
    pub fn tag(&self) -> ChangeTag;
    pub fn values(&self) -> &[(bool, &'s T)];        // bool = emphasize this segment
}
pub enum InlineChangeMode { Auto, Words, Chars, UnicodeWords, Graphemes }
impl InlineChangeOptions {
    pub fn algorithm(&mut self, alg: Algorithm) -> &mut Self;
    pub fn mode(&mut self, mode: InlineChangeMode) -> &mut Self;
    pub fn min_ratio(&mut self, min_ratio: f32) -> &mut Self;
    pub fn semantic_cleanup(&mut self, yes: bool) -> &mut Self;
}
impl TextDiffConfig {
    pub fn algorithm(&mut self, alg: Algorithm) -> &mut Self;
    pub fn whitespace_mode(&mut self, mode: WhitespaceMode) -> &mut Self;  // Exact | IgnoreChanges | IgnoreAll
}
```

**`InlineChange::values() -> &[(bool, &str)]` *is* the feature we need** — segments pre-tagged with
an emphasise flag, ready to map straight onto `(Range<usize>, HighlightStyle)`. Plus `min_ratio` as a
floor so wildly different lines are not falsely word-matched, `semantic_cleanup` to merge noisy
micro-edits into readable spans, and `ratio()` for the pairing decision in §3c.

**Watch the hidden deadline.** `iter_inline_changes` has a **hardcoded 500 ms deadline** — a 30-frame
stall waiting to happen. **Always use `iter_inline_changes_with_options_deadline` with a 5–20 ms
budget** so a pathological line degrades to a whole-line highlight instead of dropping frames.

Measured: `TextDiff::from_words` + `iter_all_changes` on a realistic ~55-char code line pair,
5000 iterations → **0.85 µs per intra-line diff**. A 60-line viewport is 51 µs. Negligible.

#### Why not `imara-diff`

**`imara-diff` 0.2.0** (2025-06-14), Apache-2.0, MSRV 1.71 — the leanest deps of the two
(`hashbrown`, `memchr`). Its README benchmarks against `similar` on line diffs across
Linux/Rust/VSCode/Helix histories on a **logarithmic scale** *"due to the large runtime of `similar`
for complex diffs"*. Genuinely faster on large line diffs.

But for **intra-line** work it is the wrong tool, for two reasons straight from its own source docs:

1. `Algorithm::Histogram`'s doc says **"character diffs do not work well"** — its heuristic degrades
   on small repeated token alphabets and auto-detects and falls back to Myers "with nontrivial
   overhead". You must explicitly pick `Algorithm::Myers` for char granularity.
2. **There is no word tokenizer.** `sources` provides only `lines` and `byte_lines`. Word / grapheme
   / unicode-word splitting is entirely your problem, as is the inline-emphasis logic that `similar`
   already ships.

Also note **the API changed substantially at 0.2.0 and the CHANGELOG was not updated for it** (it
stops at 0.1.8). The free function `imara_diff::diff(algorithm, &input, sink)` and
`UnifiedDiffBuilder` are **gone**, replaced by `Diff::compute(alg, &input)` + `.hunks()` and
`BasicLineDiffPrinter`. Anything you read about imara-diff's API is probably describing 0.1.x.

**Decision: `similar` only.** Our input is git's own hunks — a few dozen lines at a time — which is
squarely where `similar` is fast enough and its ergonomics win outright. Reach for `imara-diff`
only if we later compute whole-file line diffs ourselves and profiling shows `similar` dominating;
even then, keep `similar` for the intra-line pass.

Note that this is a deliberate divergence from Zed, which uses `imara-diff` for both — but Zed
*computes* its own line diffs against a base text buffer, and we do not.

#### Final dependency set for `crates/fleet-lazygit/Cargo.toml`

```toml
syntect  = { version = "5.3", default-features = false, features = ["default-fancy"] }
two-face = { version = "0.5", default-features = false, features = ["syntect-fancy"] }
similar  = { version = "3.2", features = ["inline", "unicode"] }
```

~1.2 MB of binary, ~6 s of clean build, zero C dependencies, 213 languages. **None of this may go
into `fleet-ui-kit`** (`crates/fleet-ui-kit/src/lib.rs:11`).

### 3c. Pairing removed and added lines inside a hunk

Word-level highlighting needs to know *which* removed line to compare against *which* added line.
Neither git nor `fleet_git` tells us. The algorithm below is Zed/GitHub-shaped, with Pierre's
refinement.

**Step 1 — segment the hunk into change blocks.** Walk `hunk.lines` and group maximal runs:
consecutive `Removed` lines followed immediately by consecutive `Added` lines form one block. This is
exactly Pierre's `ChangeContent { deletions: string[], additions: string[] }` (§2.2). Context lines
and `NoNewline` terminate a block.

**Step 2 — pair by index within the block.** Removed[i] pairs with Added[i] for
`i in 0..min(removed.len(), added.len())`. Unpaired lines on either side are pure insertions or
deletions and get a plain row tint with no word highlighting.

This is what Zed effectively does, arrived at from the other direction: it only word-diffs a hunk at
all when `base_line_count == buffer_line_count` (`$Z/crates/buffer_diff/src/buffer_diff.rs:1294`-`:1330`),
which *is* index pairing over the whole hunk. Our block-level version is strictly better: a hunk
containing several separate change blocks still gets word diff on each, where Zed gives up on the
whole hunk.

**Step 3 — gate on similarity.** Compute `TextDiff::from_words(old, new).ratio()` and **only emit
word highlights when `ratio > 0.5`**. Below that the two lines are different code rather than an
edit of the same line, and per-word highlighting is noise — it looks like confetti. `similar` also
offers `InlineChangeOptions::min_ratio` to express the same floor inside the inline API; use one or
the other, not both.

**Step 4 — apply the budget caps, taken from Zed and Pierre.**

| cap | value | source |
|---|---|---|
| max block line count for word diff | **5** | `$Z/crates/buffer_diff/src/buffer_diff.rs:20` |
| max line length for word diff | **512 bytes** | `$Z/crates/language/src/text_diff.rs:6` |
| max line length for syntax highlighting | **~1000 chars** | Pierre `tokenizeMaxLineLength` |
| inline-diff deadline | **5–20 ms** | ours; `similar`'s default 500 ms is unusable in a UI |

Above any cap, fall back to a whole-line tint. This is not a performance dodge — beyond a few lines,
index pairing stops being *correct* often enough to be worth showing.

**Step 5 — coalesce the spans.** Use `InlineChangeMode::Words` (or `UnicodeWords` for non-ASCII
identifiers) plus `semantic_cleanup(true)`. Then apply **Pierre's `'word-alt'` rule**: join two
emphasised regions separated by a single non-emphasised character (§2.3). Without it, `foo.bar()` →
`foo.baz()` highlights `bar` and `)` as two pills with a lone `(` between them. With it, you get one
readable span. This is the highest-value cheap refinement on the whole list.

Sketch, for `crates/fleet-lazygit/src/views/diff.rs`:

```rust
/// Word-level spans for one paired removed/added line, as byte ranges into each line's text.
/// `None` when the pair is too dissimilar, too long, or the block is too tall to pair reliably.
pub fn word_spans(old: &str, new: &str) -> Option<(Vec<Range<usize>>, Vec<Range<usize>>)>;

/// Groups a hunk's lines into (removed run, added run) blocks and pairs them by index.
pub fn change_blocks(hunk: &Hunk) -> Vec<ChangeBlock>;
```

Both are pure functions over `&str` / `&Hunk`, so they unit-test without opening a window — the same
discipline `crates/fleet-lazygit/src/views/mod.rs:1` already sets for this module.

### 3d. Target design

#### Architecture: keep uniform rows, cache the model, StyledText per line

```
Arc<Diff>
  └─ DiffModel  (cached, keyed by Arc::as_ptr + view mode + context lines)
       ├─ rows: Vec<Row>                 one per rendered line, UNIFORM height
       ├─ files: Vec<FileMeta>           path, kind, +n/-m counts, binary flag
       └─ per-row lazily-filled:
            ├─ syntax: Vec<(Range<usize>, HighlightStyle)>   from syntect, per visible line
            └─ words:  Vec<Range<usize>>                     from similar, per change block
```

Three decisions, each with a reason:

1. **Stay on `uniform_list` / `ListView`.** `gpui::list`'s variable heights cost an estimated content
   height, no decoration hook, and no `ElementId` (§1e). Everything we want *can* be a uniform row:
   the file header becomes **two rows** (Zed's `FILE_HEADER_HEIGHT: u32 = 2` is measured in rows for
   exactly this reason, `$Z/crates/editor/src/editor.rs:290`), the hunk separator one row, a
   collapsed region one row. Wrapping is a **mode**, not the default — see below.
2. **Cache `DiffModel`.** Key it on `Arc::as_ptr(&diff)` plus view mode plus context-line count. This
   fixes §0.2 item 2 and is a prerequisite for anything else; without it, syntax highlighting runs
   on every keystroke.
3. **One `StyledText` per line**, built with
   `StyledText::new(text).with_default_highlights(&line_style, ranges)`. This is precisely what Zed
   does for its own inline diff previews (`$Z/crates/editor/src/edit_prediction.rs:2482`,
   `$Z/crates/language/src/buffer.rs:649`); the full editor's `shape_line` path exists only because
   it needs invisibles, inline elements and pixel-exact cursor mapping. If profiling later demands
   it, `crates/fleet-ui-kit/src/components/terminal_grid.rs` is our own worked example of the canvas
   route — but do not start there.

#### Row model

```rust
pub enum Row {
    /// File header card. Two rows tall; rendered as one element spanning both.
    FileHeader { file: usize, continued: bool },
    /// `@@` separator with expand affordances. One row.
    HunkSeparator { file: usize, hunk: usize, hidden_above: u32, hidden_below: u32 },
    /// A payload line.
    Line {
        file: usize, hunk: usize, line: usize,
        kind: LineKind,
        old_no: Option<u32>, new_no: Option<u32>,
        /// Set only for the visible side in split mode.
        side: Side,
    },
    /// Binary file, mode-only change, or an empty rename. One row.
    Note { file: usize, text: SharedString },
    /// Split-mode alignment filler. One row, hatched.
    Spacer { file: usize },
}
```

`DiffRow`'s existing `(file, hunk, line)` triple **must survive** — `state::hunk_range` and
`state::selection_hunks` (`crates/fleet-lazygit/src/state.rs:1601`, `:1619`) key on it, and
`crates/fleet-git/src/patch.rs` builds patches from it.

#### Unified and split modes

**Unified** is the default for us, unlike Zed and Pierre. Reason: lazygit is a keyboard-driven
staging tool in a narrow pane, and unified keeps one cursor over one row sequence, which is what
`Staging { cursor, anchor, line_mode }` already models. Split becomes a toggle.

**Split** aligns the two sides with `Row::Spacer` filler rows, computed exactly like Zed's
`determine_spacer` (`$Z/crates/editor/src/display_map/block_map.rs:1357`) but simplified: we have no
soft wrap in the default mode, so we compare **row counts, not wrap rows**. Within a change block,
emit `max(0, added.len() - removed.len())` spacers on the removed side and vice versa. Render spacers
with `gpui::pattern_slash(theme.colors.surface, 2.0, period)` where the period divides `ROW_H`
integrally — copy `spacer_pattern_period` (`$Z/crates/editor/src/element.rs:3453`) so the hatch does
not drift row to row.

Follow Zed's responsive collapse: below a minimum width (their default is 100 columns × `em_advance`,
`$Z/crates/editor/src/split.rs:1431`) fall back to unified, and **show the third
"split-when-wider" icon state** rather than silently collapsing.

#### Per-line rendering

Each `Row::Line` composes three layers, in this order:

1. **Row background tint.** `diff_added_bg` / `diff_removed_bg`, painted on the whole row including
   the gutters. **Coalesce runs**: consecutive rows with the same tint should be one quad, per Zed's
   `paint_background` loop (`$Z/crates/editor/src/element.rs:5019`-`:5057`). With `uniform_list` this
   means the tint belongs on a container behind the rows, or is accepted as per-row `div().bg(..)` —
   start with per-row and only optimise if it shows.
2. **Syntax runs** from syntect, mapped scope → `HighlightStyle { color: Some(..) }`.
3. **Word-diff spans** from `similar`, as `HighlightStyle { background_color: Some(diff_*_emphasis) }`.

**Merge 2 and 3 with `gpui::combine_highlights`** (`$Z/crates/gpui/src/style.rs:990`) before calling
`with_default_highlights` — see the gotcha in §3f. Note that a run can carry both a foreground colour
(syntax) and a background (word emphasis), so they compose rather than conflict.

Give word-emphasis spans the visual treatment from Pierre: **a stronger tint over the row tint**, and
if we can afford it, rounded corners. gpui `TextRun::background_color` paints square, like Zed —
rounded pills would need a separate quad pass under the text (a `canvas` or absolutely-positioned
`div` behind the `StyledText`, positioned via `TextLayout::position_for_index`). **Ship square
first**; the pill is a polish pass, and Zed ships square.

Line-number gutters: two columns, `old_no` and `new_no`, right-aligned, `Tone::Muted`,
`user-select`-equivalent behaviour (we do not have text selection yet, so nothing to do). **Auto-size
the gutter to the digit count** rather than the current hard-coded `ch(9.0)` — Pierre sets
`min-width` from `${totalLines}.length + 'ch'`. Keep the gutters outside the horizontally-scrolled
payload so they stay pinned (which the current code already achieves).

#### File header card

Two rows tall, `theme.colors.surface`, `theme.radii` small, hairline border. Contents, left to right:

- status glyph coloured by `DiffKind` — added → success hue, deleted → danger hue, modified/renamed →
  the modified hue (Pierre uses a distinct blue `#009fff`/`#69b1ff` for modified; Zed uses yellow
  `#D3AF1D`);
- path, with the **directory portion at `Tone::Muted` and the filename at `Tone::Default`** — Zed
  splits these into two `Label`s with `.truncate_start()` on the directory
  (`$Z/crates/editor/src/element/header.rs:877`-`:892`);
- for renames, `old_path → new_path` with a real arrow;
- `+n / −m` counts, using Zed's `DiffStat` formatting verbatim:
  `format!("+\u{2009}{added}")` in the added hue and `format!("\u{2012}\u{2009}{removed}")` in the
  removed hue — **U+2009 THIN SPACE and U+2012 FIGURE DASH**, because a figure dash is digit-width
  and the columns line up (`$Z/crates/ui/src/components/diff_stat.rs:36`-`:59`);
- staging affordance for the file. Zed uses a **tri-state `Checkbox`**, not a button
  (`$Z/crates/git_ui/src/git_panel.rs:7313`), which is the right control for
  staged / partially-staged / unstaged. Our `DiffSide` plus a partial-staging state maps onto it
  directly.

Counts are **not** in `fleet_git` (§0.3) — compute them by counting `LineKind::Added` /
`LineKind::Removed` when building `DiffModel`, and cache them on `FileMeta`.

Make it sticky with a copy of `ui::sticky_items` (`$Z/crates/ui/src/components/sticky_items.rs`),
which is `UniformListDecoration`-shaped and depends only on `gpui` + `smallvec` (§1e). `Row` reports
`depth()`: 0 for `FileHeader`, 1 for `HunkSeparator`, 2 for everything else.

#### Hunk separator with expand-context

One row: the `@@` range summary plus `hunk.header` (git's function context, which we already parse
into `Hunk::header`), and an **"expand N lines"** affordance on each side that has hidden content.

Two mechanisms, and we want both:

- **Local expand** — clicking "expand 10 lines above" re-runs the diff for that file with a larger
  `-U<n>` and re-slices. This requires the `fleet-git` change in §0.2 item 5: thread a
  `context: u32` parameter through `read.rs`'s `DIFF_ARGS` users (`diff_file`, `diff_commit`,
  `diff_stash`, `diff_range`, `diff_branch`). Keep the default at 3.
- **Global context adjustment** — bind `{` and `}` to decrease/increase the context line count for
  the whole view and re-run, mirroring lazygit. This is a single `u32` in `GitUiState`, and it
  invalidates the `DiffModel` cache key.

Pierre's numbers are good defaults: `expansionLineCount: 100` per click is too coarse for a TUI-style
pane — use 10, matching common practice, with `shift`-click or a repeat for more. And adopt
`collapsedContextThreshold: 2`: if the gap between two hunks is ≤ 2 lines, **just show it** rather
than rendering a separator to hide two lines.

#### Horizontal scroll, wrapping, and the scrollbar

**Replace `payload.chars().skip(h_scroll)`** (`crates/fleet-lazygit/src/views/diff.rs:168`) with a
real pixel offset, like Zed: a negative x translation on the payload container, with the gutters
outside it. That fixes the missing clamp (§0.2 item 6) for free, because the clamp becomes
`max_offset` on a `ScrollHandle`.

Measure the widest line once when building `DiffModel` (Zed measures `snapshot.longest_row()`,
`$Z/crates/editor/src/element.rs:8652`) and use it as the scroll extent. Track it with **our own
`ScrollHandle` passed to `.track_scroll(..)`** — case 1 of the persistence rule in §1e, so the
offset survives re-renders and we can set it programmatically.

**Thin scrollbar**: we must write our own — there is no `gpui::Scrollbar` (§1e), and Zed's
`ui::Scrollbars` hard-depends on the `theme` crate. We already have `theme.colors.scroll_thumb` and
`theme.metrics.scroll_thumb_w = px(3.0)`. Copy the thumb recipe: rounded via
`Corners::all(Pixels::MAX).clamp_radii_for_quad_size(..)`, colour
`backdrop.blend(thumb_color)` capped around 0.7 opacity when not hovered, fading out when idle. It
can be a `UniformListDecoration`, which is how Zed's picker does it
(`$Z/crates/picker/src/render.rs:247`).

**Wrap toggle.** Default off (`overflow: 'scroll'` is Pierre's default too). When on, set
`WhiteSpace::Normal` on the line style so `TextLayout::layout` computes a wrap width
(`$Z/crates/gpui/src/elements/text.rs:650`-`:658`) — **but this breaks `uniform_list`'s uniform
height.** Two honest options: (a) make wrap mode use `gpui::list` with
`with_uniform_item_height(ROW_H)` as a convergence hint, accepting the estimated scrollbar; or
(b) pre-measure wrapped heights when the mode is enabled and keep rows uniform-per-row by rendering
a wrapped line as *n* rows. **Recommend (a)** and treat wrap as the deliberately lower-fidelity mode;
this is the same trap `main_panel.rs:572` already documents for the command log.

#### Mouse wheel and keyboard scrolling

Copy Zed's `paint_scroll_wheel_listener` formula verbatim (§1b), including:

- `ScrollDelta::coalesce` accumulation **across events** in a captured variable;
- `Lines` deltas multiplied by cell width / `ROW_H`, `Pixels` deltas passed through;
- the **axis lock** for trackpads (`OngoingScroll::filter`, `$Z/crates/gpui/src/gestures.rs:31`,
  `UNLOCK_PERCENT = 1.9`, `UNLOCK_LOWER_BOUND = px(6.)`) — without it, diagonal trackpad drift makes
  horizontal code scrolling unusable;
- clamping into `[0, scroll_max]`.

Keyboard scrolling already exists via `ListCursor` + `ListView::reveal`
(`crates/fleet-ui-kit/src/components/list_view.rs:352`), which calls
`scroll_to_item(cursor.scroll_target(moving_down), ScrollStrategy::Nearest)` with `SCROLLOFF = 2`.
Keep it. **`reveal` must be called from an action handler, never from `render`** (the component's own
doc says so, and §3f explains why).

#### Selection and staging compatibility

The existing contract is: `Staging { path, side, cursor, anchor, line_mode }`
(`crates/fleet-lazygit/src/state.rs:246`), `staging_range()` (`root.rs:1859`) resolving to
anchor-range → hunk mode → line mode, `hunk_range` / `selection_hunks` keyed by `(file, hunk)`, keys
`space` / `d` / `v` / `a` / `tab` / `h` / `l` (`keymap.rs:166`-`:175`).

**All of that keeps working** provided `Row` retains `(file, hunk, line)` and `is_change()`. Two
additions:

- Selection background must **layer over** the diff tint rather than replace it. Today `selected`
  replaces the background entirely (`views/diff.rs:174`-`:180`). Instead paint
  `row_tint` then `theme.colors.row_selected` composited over it, and use Zed's
  `ensure_minimum_contrast` idea (`$Z/crates/editor/src/element.rs:7331`) if syntax colours become
  illegible — or more simply, keep the selection tint low-alpha.
- The staging cursor should render as a **2 px left bar** (`theme.colors.cursor_bar`) in addition to
  the row background, so it stays visible on top of an added-line tint.

Hunk-level controls: adopt Zed's **hover-or-cursor visibility** rule
(`$Z/crates/editor/src/element.rs:4674`) — show a small stage/unstage affordance on the hovered or
cursor hunk only, never on all of them, with the `rounded_b_lg` + `border_x_1 border_b_1` +
`shadow_md` tab treatment (`$Z/crates/editor/src/git.rs:3076`-`:3089`) and the sticky clamp
(`:4718`-`:4726`). And copy the **pending state**: Zed's five-state
`DiffHunkSecondaryStatus` includes `SecondaryHunkAdditionPending` / `RemovalPending`, rendered as
`.alpha(0.66)` on the button (`$Z/crates/editor/src/git.rs:3093`). We shell out to `git apply`, so we
have exactly that in-flight window.

#### Binary, renames, and empty diffs

- **Binary**: `Row::Note` with a dedicated glyph and "Binary file differs", plus the byte-size delta
  if we ever parse it. Do **not** offer line staging — `crates/fleet-git/src/patch.rs:38` rejects it,
  so the keybindings must be disabled for those rows rather than failing at apply time. Pierre does
  not support binary at all; we can be better here cheaply.
- **Pure rename with no content change**: `FileHeader` with `old → new` and no body rows at all.
- **Mode-only change**: `Row::Note` with the `ModeChange` rendered as `100644 → 100755`.
- **Empty**: keep the existing `EmptyState`.

### 3e. Theme token additions

Read: `crates/fleet-ui-kit/src/theme/tokens.rs` (534 lines), `crates/fleet-ui-kit/src/tone.rs`
(60 lines), `crates/fleet-ui-kit/src/theme/palette.rs` (102 lines).

#### The constraint we must respect

`crates/fleet-ui-kit/src/tone.rs:3` states the rule: *"§1.4 of the UX spec: four colors, all
semantic, never decorative."* And `crates/fleet-lazygit/src/views/mod.rs:12` reserves the semantic
tokens for Fleet-level state, pointing diff colours at `theme.terminal.ansi[..]`.

**A proposal to add `diff_added_bg` to `ColorTokens` contradicts that second rule, so it needs an
explicit argument.** Here it is: the ANSI palette is the right source for diff *foreground* colours
because lazygit names ANSI colours in its presentation code. But it has no background entries at all
(`TerminalPalette` is `ansi: [Hsla; 16]` plus `foreground`, `background`, `cursor`, `selection` —
`crates/fleet-ui-kit/src/theme/palette.rs:13`), and a diff background tint is not something lazygit
expresses. Two options:

**Option A (recommended) — derive, add nothing.** Compute the tints in `fleet-lazygit` from the ANSI
hues that already carry the diff meaning, exactly as Pierre derives everything from four base colours
(§2.5) and Zed derives its six `editor_diff_hunk_*` fields from two `version_control_*` colours
(§1a):

```rust
// crates/fleet-lazygit/src/views/diff.rs
/// Row and word tints, derived from the ANSI hue that already names this diff colour.
/// Mirrors Zed's ladder (`theme_settings/src/schema.rs:16`-`:21`) and Pierre's
/// derive-from-background approach (`packages/diffs/src/style.css:24`-`:31`).
struct DiffTints { row: Hsla, emphasis: Hsla, gutter: Hsla }

fn tints(theme: &Theme, ansi: Ansi) -> DiffTints {
    let hue = ansi.color(theme);
    let (row_a, emph_a, gutter_a) = match theme.mode {
        ThemeMode::Dark  => (0.12, 0.22, 0.08),
        ThemeMode::Light => (0.16, 0.28, 0.10),
    };
    DiffTints {
        row:      theme.colors.bg.blend(hue.opacity(row_a)),
        emphasis: theme.colors.bg.blend(hue.opacity(emph_a)),
        gutter:   theme.colors.bg.blend(hue.opacity(gutter_a)),
    }
}
```

The alphas are Zed's filled-hunk ladder (0.12 dark / 0.16 light) with an emphasis step chosen to sit
between Zed's word colours (α 0.35 added, 0.80 deleted) and Pierre's (0.15–0.2 over an already-tinted
row). The gutter step lighter than the row is Pierre's `--mix-light: 91%` vs `88%` (§2.5).

`Hsla::blend` composites the second argument **over** the first and keeps the backdrop's alpha
(§1e), so `bg.blend(hue.opacity(a))` yields an opaque tinted background — exactly what we want, and
exactly Zed's `flattened_background_color` idiom.

Note that `Tone::fill` already does almost this at α 0.14 (`crates/fleet-ui-kit/src/tone.rs:52`), so
the ladder is consistent with existing kit behaviour.

**Advantages:** zero kit churn, no rule to renegotiate, automatically correct for both theme modes,
and any future theme gets sane diff colours for free.

**Option B — add six fields to `ColorTokens`.** If we decide diff tints are a first-class design
token, insert after `skeleton` (`crates/fleet-ui-kit/src/theme/tokens.rs:78`):

```rust
    /// Row wash behind an added line.
    pub diff_added_bg: Hsla,
    /// Stronger wash behind changed words inside an added line.
    pub diff_added_emphasis: Hsla,
    /// Row wash behind a removed line.
    pub diff_removed_bg: Hsla,
    /// Stronger wash behind changed words inside a removed line.
    pub diff_removed_emphasis: Hsla,
    /// Hairline / glyph colour for a modified or renamed file.
    pub diff_modified: Hsla,
    /// Hatch colour for split-view alignment filler.
    pub diff_spacer: Hsla,
```

with values in `ColorTokens::dark()` (`:83`) and `::light()` (`:111`). Concrete proposals, computed
from the existing dark `bg = 0x0E1013` / light `bg = 0xFBFBFC` and the existing ANSI green
(`0x3FB950` dark, `0x1A7F37` light) and red (`0xF85149` dark, `0xCF222E` light):

| token | dark | light |
|---|---|---|
| `diff_added_bg` | `0x14211A` | `0xEAF6EC` |
| `diff_added_emphasis` | `0x1F3D28` | `0xCDEBD4` |
| `diff_removed_bg` | `0x241518` | `0xFCEBEC` |
| `diff_removed_emphasis` | `0x45201F` | `0xF7D2D4` |
| `diff_modified` | `0xD29922` | `0x9A6700` |
| `diff_spacer` | `0x1A1D23` | `0xF1F2F4` |

(`diff_modified` reuses the existing `warning` values, which are already the amber Zed uses for
modified; `diff_spacer` sits between `bg` and `surface`.)

**Recommendation: Option A.** Ship derived tints in `fleet-lazygit` first. If a second surface ever
needs diff colours — a commit-view, an inline blame diff — promote them to `ColorTokens` then, with
the measured values from Option A as the seed. Adding six semantic-looking tokens for one consumer is
the wrong trade, and `docs/DESIGN-SYSTEM.md` would need to change in the same commit either way
(`crates/fleet-ui-kit/src/theme/tokens.rs:1`-`:4` says so explicitly).

#### Syntax colours

**Do not add a syntax palette to `ColorTokens`.** Map syntect scopes to a small table local to
`fleet-lazygit`, keyed off the ANSI palette so it tracks the theme:

| scope prefix | colour |
|---|---|
| `comment` | `theme.colors.text_muted` |
| `string`, `constant.character` | `Ansi::Green` |
| `constant.numeric`, `constant.language` | `Ansi::Yellow` |
| `keyword`, `storage` | `Ansi::Magenta` |
| `entity.name.function`, `support.function` | `Ansi::Blue` |
| `entity.name.type`, `support.type`, `storage.type` | `Ansi::Cyan` |
| `variable.parameter`, `entity.name.tag` | `Ansi::Red` |
| `punctuation`, everything else | `theme.colors.text` |

Eight buckets, deliberately. Zed's One Dark uses eight hues across 46 categories and half of them
share a colour (§1c) — resist the urge to colour every token type differently, especially inside a
diff where the row tint is already carrying information. Implement the lookup with a
**longest-dotted-prefix match**, copying `SyntaxTheme::highlight_id`'s approach
(`$Z/crates/syntax_theme/src/syntax_theme.rs:78`-`:93`) so `string.quoted.double.rust` falls back to
`string` without an entry of its own.

Alternatively, load a syntect `Theme` and use its colours directly — but then the diff view stops
matching the rest of Fleet, and we own two palettes. Not worth it.

### 3f. gpui gotchas for this work

Ordered by how much time each one will cost you if you hit it unaware.

1. **`StyledText` silently mis-renders unsorted or overlapping highlight ranges.** `compute_runs`
   (`$Z/crates/gpui/src/elements/text.rs:453`-`:479`) walks a monotonic cursor and assumes sorted,
   disjoint, ascending ranges. Via `with_default_highlights` you get a **panic**
   (`"invalid text run"`, `:527`-`:537`); via `with_highlights` you get **no validation at all** —
   just a log line from `shape_text` and wrong colours. Our syntax ranges and word-diff ranges *will*
   overlap. **Always merge with `gpui::combine_highlights`** (`$Z/crates/gpui/src/style.rs:990`)
   first. This is the single most likely bug in this work.
2. **Highlight ranges are UTF-8 byte offsets and must land on char boundaries.** `DiffLine::content`
   is `Vec<u8>` and we `String::from_utf8_lossy` it — lossy replacement changes byte lengths, so
   compute ranges **against the converted string**, never the original bytes.
3. **`with_highlights` and `with_default_highlights` are mutually exclusive** (`debug_assert!` at
   `text.rs:419`-`:422`, `:434`-`:437`), and `with_highlights` resolves against the *inherited*
   `window.text_style()`, not one you pass. Pick `with_default_highlights` for explicit control.
4. **`Hsla::fade_out` takes `&mut self` and returns `()`** (`$Z/crates/gpui/src/color.rs:607`) — it
   does not chain. And **`opacity(f)` multiplies** the existing alpha while **`alpha(a)` replaces**
   it (`:637`, `:667`). Chaining `.opacity()` twice compounds.
5. **`Hsla::blend` keeps the *backdrop's* alpha, not a proper source-over** (`:58`). Correct for
   "paint over an opaque background", wrong for accumulating two translucent overlays. It also
   round-trips through RGBA, so hue drifts slightly.
6. **`uniform_list` requires uniform row height.** Not "approximately uniform" — it measures one item
   and extrapolates. A single taller row corrupts scroll position and thumb size. This is why the
   file header is *two rows* rather than a taller row.
7. **`ListView::reveal` must never be called from `render`** — it calls
   `scroll_to_item`, which is *deferred* (written to `deferred_scroll_to_item`, consumed in
   `prepaint`). Calling it during render either does nothing this frame or fights the current scroll.
   `crates/fleet-ui-kit/src/components/list_view.rs:352` documents this; obey it.
8. **`.track_scroll(&handle)` with your own handle is the only reliable scroll state.** Without it,
   the offset lives in id-keyed frame state and **resets whenever the element id or any ancestor id
   changes**; without an id at all, the div silently never scrolls
   (`$Z/crates/gpui/src/elements/div.rs:2169`-`:2188`, `:2378`).
9. **`ScrollHandle::max_offset()` and `bounds()` are only valid after a layout pass**, and
   `set_offset` is clamped next frame, not at call time (`div.rs:2325`-`:2381`). Do not compute a
   clamp from them during the first render.
10. **`UniformListScrollHandle::logical_scroll_top_index()` is `#[cfg(test)]`-gated**
    (`uniform_list.rs:222`). To read the scroll position in production, go through
    `handle.0.borrow().base_handle`.
11. **There is no `gpui::Scrollbar`.** Zed's lives in its `ui` crate and hard-depends on the `theme`
    crate. Write our own (§3d).
12. **`canvas`'s closures are `FnOnce` and are consumed on first use** — construct a fresh `Canvas`
    every `render` (`$Z/crates/gpui/src/elements/canvas.rs:71`, `:86`).
13. **`shape_line` `debug_assert!`s that the text contains no `\n`** and there is **no
    `TextSystem::shape_line`** — it is on `WindowTextSystem`, via `window.text_system()`.
14. **`TextRun`s must exactly tile the string.** `with_runs` asserts it; `shape_text` merely logs
    `"TextRun`s do not cover the entire to be shaped text"` and renders wrong.
15. **Expand tabs to spaces yourself before shaping**, and shift highlight ranges accordingly. Zed
    never lets `\t` reach the shaper (`$Z/crates/editor/src/display_map/tab_map.rs:677`-`:696`);
    `tab_size - (col % tab_size)`.
16. **Disable ligatures on the mono font.** `crates/fleet-ui-kit/src/components/terminal_grid.rs:495`
    already learned this the hard way: a face rendering `!=` as `≠` silently moves the column grid.
    Use `FontFeatures::disable_ligatures()`.
17. **Round or `pixel_snap` your metrics consistently.** `TextStyle::line_height_in_pixels` `.round()`s
    (`$Z/crates/gpui/src/style.rs:553`) while `StyledText`'s internal layout `pixel_snap`s
    (`text.rs:632`). Pass the same `TextStyle` to gutter and content or they disagree sub-pixel.
18. **`em_width` ≠ `em_advance` ≠ `em_layout_width`** and Zed uses all three for different purposes
    (§1b). Use `em_advance` for the glyph grid.
19. **`ScrollDelta::Lines` vs `Pixels` need different scaling**, and the delta must be **accumulated
    across events** with `coalesce` and **axis-locked** for trackpads
    (`$Z/crates/gpui/src/gestures.rs:31`). Skipping the axis lock is the difference between
    "scrolls nicely" and "drifts sideways constantly".
20. **`similar::iter_inline_changes` has a hardcoded 500 ms deadline.** Use
    `iter_inline_changes_with_options_deadline` with 5–20 ms.
21. **syntect at `regex-fancy` is 93.7 µs/line.** Never highlight a whole file synchronously in
    `render` — a 5000-line file is ~470 ms. Checkpoint + replay per visible range (§3a), and cap
    line length.
22. **`InteractiveText::on_click` passes the index into your `ranges` vector, not a byte offset**
    (`$Z/crates/gpui/src/elements/text.rs:1022`), and only fires when mouse-down and mouse-up land in
    the same range.
23. **`TextLayout`'s accessors panic before prepaint** (`index_for_position`, `position_for_index`,
    `bounds`, `line_height`, `len`). If you use them for a rounded word-pill pass, do it in `paint`.
24. **`with_decoration` is singular** and **`uniform_list` takes no entity** — see the drift table in
    §1e before you write the call.

---

## Appendix: primary sources

| what | where |
|---|---|
| Zed source, pinned tag | `/Users/danny/.cargo/git/checkouts/zed-a70e2ad075855582/bebe92f/` (`crates/zed/Cargo.toml:5` → 1.18.1) |
| gpui crate version at that tag | `crates/gpui/Cargo.toml:3` → `0.2.2` |
| One Dark / One Light | `$Z/assets/themes/one/one.json` (One Dark `:7`, syntax `:191`; One Light `:426`) |
| Zed diff opacity ladder | `$Z/crates/theme_settings/src/schema.rs:16`-`:21` |
| Pierre docs, whole | <https://diffs.com/llms-full.txt> (286 KB; `/docs` exceeds 10 MB and fails to fetch) |
| Pierre docs index | <https://diffs.com/llms.txt> |
| Pierre source | <https://github.com/pierrecomputer/pierre>, `packages/diffs` (v1.4.1, Apache-2.0) |
| Pierre stylesheet | `packages/diffs/src/style.css` (1843 lines) — better reference than the prose docs |
| Pierre options/types | `packages/diffs/src/types.ts` |
| Pierre architecture essay | <https://pierre.computer/writing/on-rendering-diffs> |
| npm metadata | <https://registry.npmjs.org/@pierre%2Fdiffs> |
| syntect | <https://crates.io/crates/syntect> 5.3.0 |
| two-face | <https://crates.io/crates/two-face> 0.5.2+bat-0.26.1, repo now at <https://codeberg.org/CosmicHarper/two-face> |
| similar | <https://crates.io/crates/similar> 3.2.0 |
| imara-diff | <https://crates.io/crates/imara-diff> 0.2.0 (CHANGELOG stops at 0.1.8; API changed at 0.2) |

All timings in §3a and §3b were measured locally on this machine during the research pass
(M-series, 15 cores, release build, rustc 1.94.1), not quoted from documentation.
