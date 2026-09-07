# 0005 — Diff rendering

**Adopted** for `fleet-lazygit`'s diff and staging views.

- **Syntax highlighting: `syntect` 5.3 + `two-face` 0.5.** Pure Rust — `syntect` with
  `default-features = false, features = ["parsing", "regex-fancy"]` drops `onig`/`onig_sys`, and
  `two-face` with `default-features = false, features = ["syntect-fancy"]` adds bat's curated
  grammar set (~213 syntaxes, including TSX, TOML and Dockerfile). `two-face`'s default feature is
  `syntect-onig`; selecting it would pull the C dependency back in.
- **Intra-line diff: `similar` 3.2** with `inline` and `unicode`. Do not pin below 3.1.2 —
  earlier 3.x versions panic on some `DiffOp` ranges.
- **Uniform rows.** Every rendered line is one fixed-height row: a file header spans two rows, a
  hunk separator one, a collapsed region one. Wrapping is a mode, not the default. GPUI's
  variable-height `list` costs an estimated content height and gives no row `ElementId`.
- **The model is prepared, not rendered.** A `DiffModel` is keyed by its content, view mode and
  context-line count, built on the background executor and cancellable, so render only borrows a
  finished model. Without that cache, highlighting would rerun on every keystroke.
- **One `StyledText` per line**, with highlight ranges applied as default highlights. The
  canvas/`shape_line` route is reserved for the terminal grid, which needs pixel-exact cells.

Provenance (removed from the tree; read them in git history): `docs/research/diff-view-brief.md` @
d5bc677.
