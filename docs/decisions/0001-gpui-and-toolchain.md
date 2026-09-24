# 0001 — GPUI dependency, toolchain and boot sequence

**Adopted.** Fleet renders with GPUI taken from the Zed monorepo at git tag **`v1.18.1`**:
`gpui` with `default-features = false` and `gpui_platform` with `font-kit`, pinned in the
workspace manifest. Rust **1.97.1** is a hard floor and is pinned in `rust-toolchain.toml`.

- **crates.io `gpui 0.2.2` is not used.** It predates the February 2026 crate split, so it still
  exposes `Application::new()`, `Corner`, the old `ShapedLine::paint` signature and no
  `QuitMode`. Every current reference an agent will read — the `main` README, `docs.rs`,
  `crates/gpui/examples/*`, and above all Zed's own `terminal_view/src/terminal_element.rs`, which
  is the blueprint for our grid — is written against the newer API.
- Third-party republishes (`gpui-unofficial`, `gpui-ce`) and component libraries built on them
  (`gpui-component` / `gpui-kit`) are rejected: two GPUI copies in one binary cannot
  interoperate, and a republish cannot patch a released version.
- Upgrading is a one-line tag bump with a readable diff.

**macOS prerequisite.** The Metal Toolchain is a separate download:
`xcodebuild -downloadComponent MetalToolchain`, verified with `xcrun -sdk macosx metal --version`.

**Boot sequence** (both binaries; `fleet-app` and `fleet-lazygit` follow the same shape):
`gpui_platform::application()` — not `gpui::Application::new()` — with the asset source attached
before `run`, then key bindings, application menus (`cmd-q` is unreliable without them),
`open_window`, `cx.activate(true)`, and `on_window_closed` quitting when no window remains.
`svg()` renders nothing without an `AssetSource`, and only one may be installed per application.

**Build profiles.** Zed's shape: `[profile.dev] opt-level = 1, debug = "limited"` for workspace
crates, `[profile.dev.package."*"] opt-level = 0` for dependencies, and a named list at
`opt-level = 3` for the ones GPUI spends its frames in — layout (`taffy`), shaping and font parsing,
SVG, image decoding, the `wgpu` renderer, `syntect`, `serde_json` and the proc-macros. Those are
unusable at `opt-level = 0`; the rest of the ~800 are not hot. Optimizing every dependency (the
original setting) measured 14.5 min for a cold build on a 12-core, 16 GB Linux machine, swapping
to 12 GB, against 6 min without swap for this profile. A dependency that turns out hot in a
profile joins the list in `Cargo.toml`; `[profile.dev.package."*"]` does not go back to 3.

**Reading stale snippets.** `ViewContext`, `WindowContext`, `cx.new_view`, a one-argument
`render`, `View<T>`/`Model<T>`, `Corner::`, `overlay()`, or `.child(some_entity)` mark a snippet
as pre-`v1.18.1`. `Application::new()` marks the crates.io era. The pinned Zed source is on disk
after the first build at `~/.cargo/git/checkouts/zed-<hash>/<rev>/` and is the reference to read
instead of web tutorials.

Provenance (removed from the tree; read them in git history): `docs/research/gpui.md` @ b5741b7,
`docs/research/gpui-smoke-recipe.md` @ 79d7574.
