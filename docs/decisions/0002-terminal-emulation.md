# 0002 — Terminal emulation and rendering

**Adopted.** `fleet-term` runs **`libghostty-vt`** (pinned `=0.2.1`) behind the `VtEngine` trait,
inside `fleetd`. PTYs come from `portable-pty`. The client never hosts a native view: it paints
its mirror grid with GPUI primitives from the `FrameUpdate` diffs described in
`docs/ARCHITECTURE.md`.

Why not host Ghostty's own Metal `NSView` (which does work):

- The native view lives outside GPUI's scene. It is never clipped by masks, rounded corners or
  `overflow`, is not synced to GPUI scrolling or animation, and either covers every GPUI overlay
  or needs a fully transparent root and window. For a product whose point is a reusable design
  system, an unclippable opaque rectangle our own overlays cannot draw over is disqualifying.
- It is macOS-only upstream, and Ghostty's own `include/ghostty.h` says the surface API is not
  designed for external use and that embedders should use `libghostty-vt`.

`alacritty_terminal` remains the documented fallback: `fleet-term`'s `ghostty` cargo feature is
the seam, and `--no-default-features` still builds. Ghostty was chosen for VT fidelity, at the
cost of a Zig build step (see `docs/DEVELOPMENT.md`).

**Painting rules for the grid** (ported from Zed's `terminal_element.rs`):

1. Measure `cell_width` from the font's `m` advance; snap line height to whole device pixels.
2. Walk the grid once, producing batched text runs (adjacent cells merged while font, colors and
   decorations are equal and columns are contiguous) plus merged background rects.
3. Paint inside a content mask: element fill, then background rects (floor x, ceil width, or you
   get seams), then `shape_line(..., force_width: Some(cell_width))` and `ShapedLine::paint`,
   then block glyphs as quads, then the cursor.
4. Own the scroll offset and device-pixel-snap the paint origin. Do **not** use `uniform_list` or
   `list` for rows, and do not hand-roll `window.paint_glyph` — `shape_line` has a layout cache
   and is correct for ligatures, combining marks and wide characters.

Provenance (removed from the tree; read them in git history): `docs/research/libghostty.md` @
79d7574, `docs/research/gpui.md` @ b5741b7.
