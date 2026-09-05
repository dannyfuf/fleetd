# Native lazygit in this repo — implementer's brief

This is the briefing for whoever writes `crates/fleet-lazygit`: a native gpui UI that clones
lazygit's ergonomics on top of `crates/fleet-git`, reusing `crates/fleet-ui-kit`.

Read `docs/research/lazygit-reference.md` first — it is the *what* (lazygit's layout,
keybindings, visual language and git plumbing). This document is the *how* in this workspace.

Ground rules that shaped every recommendation below:

- gpui is pinned to the Zed tag **v1.18.1** (`Cargo.toml:26`-`Cargo.toml:27`):
  ```toml
  gpui = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", default-features = false }
  gpui_platform = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", features = ["font-kit"] }
  ```
  Anything you read on the internet about gpui may be newer than this tag. Trust `fleet-app`
  and `fleet-ui-kit` in-tree over any external example.
- The workspace is Rust edition 2024, `rust-version = "1.97.1"` (`Cargo.toml:16`-`Cargo.toml:17`).
- `crates/fleet-git` is already a workspace member (`Cargo.toml:5`). `crates/fleet-lazygit`
  must be added to the same `members` list.

Contents:

- [a. How fleet-app boots gpui](#a-how-fleet-app-boots-gpui)
- [b. The keymap / actions pattern](#b-the-keymap--actions-pattern)
- [c. fleet-ui-kit component inventory](#c-fleet-ui-kit-component-inventory)
- [d. Background work: the bridge pattern and the central state entity](#d-background-work-the-bridge-pattern-and-the-central-state-entity)
- [e. Build, test and lint conventions](#e-build-test-and-lint-conventions)
- [f. Recommended UI architecture for crates/fleet-lazygit](#f-recommended-ui-architecture-for-cratesfleet-lazygit)

## a. How fleet-app boots gpui

### a.1 Entry point: `gpui_platform::application()`, not `gpui::Application::new()`

`crates/fleet-app/src/main.rs` is a CLI/GUI switch (whole file, `crates/fleet-app/src/main.rs:1`-`:12`):

```rust
//! Entry point that selects Fleet's CLI or native GPUI application mode.

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).is_some() {
        let exit_code = fleet_cli::run();
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        return Ok(());
    }
    fleet_app::run()
}
```

`run` lives in `crates/fleet-app/src/shell/root.rs` and is re-exported through
`crates/fleet-app/src/shell/mod.rs:20` and `crates/fleet-app/src/lib.rs:14`.

```rust
// crates/fleet-app/src/shell/root.rs:1036-1046
/// Opens the window and runs the app. Returns when the last window closes.
pub fn run() -> anyhow::Result<()> {
    init_tracing();
    let home = fleet_home();
    tracing::info!(home = %home.display(), "fleet: starting");
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(move |cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            keymap::init(cx);
```

**There is no `gpui::Application::new()` in v1.18.1.** The pinned checkout exposes only
`Application::with_platform` / `new_inaccessible`, and `gpui_platform::application()` is the
wrapper that supplies the platform:

```rust
// gpui_platform/src/gpui_platform.rs:13-21  (pinned zed v1.18.1)
pub fn application() -> gpui::Application {
    #[cfg(target_family = "wasm")]
    { application_with_web_backend(gpui_web::WebBackendPreference::Auto) }
    #[cfg(not(target_family = "wasm"))]
    gpui::Application::with_platform(current_platform(false))
}
```

`gpui_platform::headless()` also exists, for a windowless app.

**Your new crate must depend on both** `gpui` and `gpui_platform`, exactly as
`crates/fleet-app/Cargo.toml:20`-`:21` does. Note the manifest shape to copy
(`crates/fleet-app/Cargo.toml:1`-`:25`), including the explicit `[[bin]]` block:

```toml
[[bin]]
name = "fleet"
path = "src/main.rs"
```

`crates/fleet-ui-kit` itself depends on **only** `gpui`
(`crates/fleet-ui-kit/Cargo.toml:8`-`:16`); `gpui_platform` is a dev-dependency there purely so
`examples/kit_gallery.rs` can open a window.

### a.2 Asset source: `KitAssets`, and the one-per-Application rule

```rust
// crates/fleet-ui-kit/src/assets.rs:1-12
//! The asset source for the kit's embedded icons.
//!
//! gpui builds exactly one [`AssetSource`] per application (`zed#8713`), so a library cannot
//! register its own. Two supported shapes:
//!
//! * an app with no other assets registers [`KitAssets`] directly:
//!   `application().with_assets(KitAssets)`;
//! * an app with its own assets keeps its own source and delegates the `icons/` prefix:
//!   `if let Some(bytes) = kit_asset(path) { return Ok(Some(Cow::Borrowed(bytes))) }`.
//!
//! A missing asset must return `Ok(None)`, never `Err` — gpui logs nothing for `svg()` and an
//! erroring source turns an invisible icon into an invisible crash.
```

```rust
// crates/fleet-ui-kit/src/assets.rs:19-48
pub fn kit_asset(path: &str) -> Option<&'static [u8]> {
    if !path.starts_with("icons/") {
        return None;
    }
    Icon::from_path(path).map(Icon::bytes)
}

pub fn kit_asset_paths() -> Vec<SharedString> {
    Icon::ALL.iter().map(|icon| icon.path()).collect()
}

/// An [`AssetSource`] serving only the kit's icons.
#[derive(Clone, Copy, Debug, Default)]
pub struct KitAssets;

impl AssetSource for KitAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(kit_asset(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        if prefix.is_empty() || prefix == "icons" {
            Ok(kit_asset_paths())
        } else {
            Ok(Vec::new())
        }
    }
}
```

The constraint is *literally* true in this gpui version, because `with_assets` **replaces**
both the source and the SVG renderer, so a second call silently discards the first:

```rust
// gpui/src/app.rs:200-207  (pinned zed v1.18.1)
pub fn with_assets(self, asset_source: impl AssetSource) -> Self {
    let mut context_lock = self.0.borrow_mut();
    let asset_source = Arc::new(asset_source);
    context_lock.asset_source = asset_source.clone();
    context_lock.svg_renderer = SvgRenderer::new(asset_source);
    drop(context_lock);
    self
}
```

Icons are `include_bytes!`-compiled, not read from disk
(`crates/fleet-ui-kit/src/icons.rs:41`-`:53`), from `crates/fleet-ui-kit/assets/icons/`.

### a.3 Fonts: nothing is registered, and monospace is *not* guaranteed

There are **no font files in the repo** and **no call to `add_fonts` anywhere**. Both families
are plain family-name strings on the `Theme`:

```rust
// crates/fleet-ui-kit/src/theme/theme.rs:73-78
    /// UI font family. `.SystemUIFont` resolves to SF Pro via CoreText with no registration.
    pub font_ui: SharedString,
    /// Mono font family, resolved from [`Theme::MONO_STACK`] by [`Theme::init`].
    pub font_mono: SharedString,
```

```rust
// crates/fleet-ui-kit/src/theme/theme.rs:95-96  (defaults inside Theme::dark())
            font_ui: SharedString::new_static(".SystemUIFont"),
            font_mono: SharedString::new_static("SF Mono"),
```

The kit spells out why monospace has to be probed
(`crates/fleet-ui-kit/src/theme/theme.rs:119`-`:134`):

```rust
    /// The monospaced faces §0 accepts, best first.
    ///
    /// `SF Mono` is the spec's face but it is **not** part of a stock macOS install — it ships
    /// with Xcode / the SF font download. `Menlo` and `Monaco` do ship with every macOS, and
    /// `DejaVu Sans Mono` / `Liberation Mono` cover the Linux builds. Without a stack, a
    /// missing `SF Mono` silently resolves to the proportional UI face and the terminal grid,
    /// branch names, paths and shas stop landing on the cell grid.
    pub const MONO_STACK: &'static [&'static str] = &[
        "SF Mono",
        "SFMono-Regular",
        "Menlo",
        "Monaco",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Courier New",
    ];
```

The resolution is a **metric probe**, because gpui has no "is this family installed" query —
`resolve_font` answers with the fallback (the UI sans) for a missing family
(`crates/fleet-ui-kit/src/theme/theme.rs:136`-`:167`): it measures the advance of `i`, `M` and
`W` at `MONO_PROBE_SIZE = px(12.5)` and accepts the first family whose three advances agree
within `MONO_PROBE_EPSILON = px(0.01)` (`crates/fleet-ui-kit/src/theme/theme.rs:12`-`:15`).

**Consequence:** `Theme::init` must run *inside* `Application::run`'s callback — it needs
`cx.text_system()` (`crates/fleet-ui-kit/src/theme/theme.rs:145`).

The families reach the element tree through `AppFrame`, which is what sets the window-wide
typography (`crates/fleet-ui-kit/src/components/app_frame.rs:110`-`:122`):

```rust
impl RenderOnce for AppFrame {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(theme.colors.bg)
            .text_color(theme.colors.text)
            .font_family(theme.font_ui.clone())
            .text_size(theme.text.ui.size)
            .line_height(theme.text.ui.line_height)
```

**A root view that does not compose `AppFrame` gets no font or text defaults and must set them
itself.** For `fleet-lazygit`, compose `AppFrame` — see §c.

### a.4 Window options — the exact literal

```rust
// crates/fleet-app/src/shell/root.rs:34-39
const TICK: Duration = Duration::from_millis(250);
/// The window's default and minimum size (§0).
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);
/// The smallest window the ladders of §2.9 are defined for.
const MIN_SIZE: (f32, f32) = (900.0, 560.0);
```

```rust
// crates/fleet-app/src/shell/root.rs:1058-1068
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Fleet".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(px(12.0), px(12.0))),
                }),
                ..Default::default()
            };
```

**`window_background` and `kind` are not set** — they come from `..Default::default()`. The
full field set available in v1.18.1 is `gpui/src/platform.rs:1829`-`:1899`: `window_bounds`,
`titlebar`, `focus`, `show`, `kind`, `is_movable`, `app_owns_titlebar_drag`,
`inactive_frame_interval`, `is_resizable`, `is_minimizable`, `display_id`, `window_background`,
`app_id`, `window_min_size`, `window_decorations`, `icon`, `tabbing_identifier`.

`appears_transparent: true` has a knock-on effect on chrome layout
(`crates/fleet-app/src/shell/chrome.rs:18`-`:23`):

```rust
/// The left inset that clears the macOS traffic lights (§2.2).
#[cfg(target_os = "macos")]
const LEADING_INSET: f32 = 84.0;
/// Elsewhere the bar starts at the normal 12 px gutter.
#[cfg(not(target_os = "macos"))]
const LEADING_INSET: f32 = 12.0;
```

### a.5 Root view construction

```rust
// crates/fleet-app/src/shell/root.rs:1069-1085
            match cx.open_window(options, |_window, cx| cx.new(|cx| Shell::new(home, cx))) {
                Ok(window) => {
                    let _ignored = window.update(cx, |_, window, _| window.activate_window());
                    cx.activate(true);
                    // Developer-only: drive the GUI from a script file (docs/DEVELOPMENT.md).
                    if let Some(script) = drive::script_path() {
                        let _ignored = window.update(cx, |_, window, cx| {
                            drive::spawn(script, window, cx).detach();
                        });
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "fleet: could not open the window");
                    eprintln!("fleet: could not open the window: {error}");
                    cx.quit();
                }
            }
```

Signature (`gpui/src/app.rs:1258`-`:1262`):

```rust
pub fn open_window<V: 'static + Render>(
    &mut self,
    options: crate::WindowOptions,
    build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<WindowHandle<V>>
```

The root view implements `Focusable` and `Render`
(`crates/fleet-app/src/shell/root.rs:759`-`:766`). The gallery variant additionally focuses the
root view after opening, which is worth copying
(`crates/fleet-ui-kit/examples/kit_gallery.rs:1484`-`:1490`):

```rust
            window
                .update(cx, |view, window, cx| {
                    window.focus(&view.focus_handle(cx), cx);
                })
                .ok();
```

### a.6 Installing the theme

The `Theme` is a gpui `Global` (`crates/fleet-ui-kit/src/theme/theme.rs:80`:
`impl Global for Theme {}`) read through an extension trait
(`crates/fleet-ui-kit/src/theme/theme.rs:247`-`:260`):

```rust
/// `cx.theme()` on any context that derefs to [`App`].
pub trait ActiveTheme {
    /// The installed theme.
    ///
    /// # Panics
    /// Panics when [`Theme::init`] has not run.
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}
```

It is implemented **only for `App`** and reaches `Context<T>` / `&mut App` through `Deref`.

```rust
// crates/fleet-ui-kit/src/theme/theme.rs:169-206
    /// Install the theme global. Must run before any kit component renders.
    pub fn init(mode: ThemeMode, cx: &mut App) {
        let mono = Self::resolve_mono_family(cx);
        cx.set_global(Self::for_mode(mode).with_mono_family(mono));
    }

    /// Install the theme global from the window's OS appearance.
    pub fn init_from_system(window: &Window, cx: &mut App) { /* … */ }

    /// Replace the theme global with another mode, keeping the resolved mono face.
    pub fn change(mode: ThemeMode, cx: &mut App) { /* … */ }

    /// Flip light/dark and return the new mode.
    pub fn toggle(cx: &mut App) -> ThemeMode { /* … */ }

    /// Follow the OS appearance.
    pub fn sync_system_appearance(window: &Window, cx: &mut App) { /* … */ }
```

Install call site: `crates/fleet-app/src/shell/root.rs:1044` —
`Theme::init(ThemeMode::Dark, cx);`.

**The lifetime rule, verbatim** (`crates/fleet-ui-kit/src/theme/theme.rs:53`):

> Components must never hold a `Theme` across frames; read it from `cx` each render.

Where a component needs the theme past a borrow of `cx` it does `let theme = cx.theme().clone();`
(e.g. `crates/fleet-ui-kit/src/components/palette.rs:243`).

### a.7 Everything else before the window opens

In order, all inside `run()`:

1. **Logging** — `init_tracing()` (`crates/fleet-app/src/shell/root.rs:1023`-`:1034`), called at
   `:1038`. `$RUST_LOG` (default `info`) to stderr, errors swallowed so an embedder's
   subscriber wins. Needs `tracing-subscriber` with the `env-filter` feature
   (`crates/fleet-app/Cargo.toml:25`).
2. **Home directory** — `fleet_home()` (`crates/fleet-app/src/shell/root.rs:1010`-`:1021`):
   `$FLEET_HOME`, else `~/.fleet`.
3. **`Theme::init`** — `crates/fleet-app/src/shell/root.rs:1044`.
4. **`keymap::init(cx)`** — `crates/fleet-app/src/shell/root.rs:1045`.
5. **App menu** — `crates/fleet-app/src/shell/root.rs:1046`-`:1050`:
   ```rust
   cx.set_menus(vec![Menu {
       name: "Fleet".into(),
       items: vec![MenuItem::action("Quit", fleet::Quit)],
       disabled: false,
   }]);
   ```
6. **Quit when the last window closes** — `crates/fleet-app/src/shell/root.rs:1051`-`:1056`:
   ```rust
   cx.on_window_closed(|cx: &mut App, _window_id| {
       if cx.windows().is_empty() {
           cx.quit();
       }
   })
   .detach();
   ```
   **Without this the process lives on after the window closes.**
7. **`window.activate_window()` + `cx.activate(true)`** —
   `crates/fleet-app/src/shell/root.rs:1071`-`:1072`.
8. **No panic hook is installed anywhere.**
9. Optional dev driver, gated on `drive::script_path()`
   (`crates/fleet-app/src/shell/root.rs:1073`-`:1078`).

`run()` returns `Ok(())` at `crates/fleet-app/src/shell/root.rs:1087`, after
`Application::run` returns.

### a.8 A minimal copyable `main` for `fleet-lazygit`

Every API below is verified against this repo or the pinned checkout; each provenance comment
names the source.

`crates/fleet-lazygit/Cargo.toml`:

```toml
[package]
name = "fleet-lazygit"
version = "0.1.0"
edition.workspace = true            # Cargo.toml:16
rust-version.workspace = true       # Cargo.toml:17

[[bin]]                             # shape from crates/fleet-app/Cargo.toml:7-9
name = "fleet-lazygit"
path = "src/main.rs"

[dependencies]
anyhow.workspace = true             # Cargo.toml:20
fleet-git = { path = "../fleet-git" }
fleet-ui-kit = { path = "../fleet-ui-kit" }   # crates/fleet-app/Cargo.toml:19
gpui.workspace = true                          # Cargo.toml:26
gpui_platform.workspace = true                 # Cargo.toml:27
async-channel.workspace = true                 # Cargo.toml:21
tokio.workspace = true                         # Cargo.toml:34
tracing.workspace = true                       # Cargo.toml:36
tracing-subscriber = { workspace = true, features = ["env-filter"] }  # fleet-app/Cargo.toml:25
```

…and add `"crates/fleet-lazygit"` to `Cargo.toml:3`-`:13`.

`crates/fleet-lazygit/src/lib.rs`:

```rust
use std::path::PathBuf;

use fleet_ui_kit::{KitAssets, prelude::*};      // lib.rs:52 (KitAssets), lib.rs:61-69 (prelude)
use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, Render, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};

/// The window's default and minimum size.
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);   // shell/root.rs:36
const MIN_SIZE: (f32, f32) = (900.0, 560.0);        // shell/root.rs:38

/// The root view.
struct Lazygit {
    focus_handle: FocusHandle,
}

impl Lazygit {
    fn new(_path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),        // shell/root.rs:114
        }
    }
}

impl Focusable for Lazygit {                        // shell/root.rs:759-763
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Lazygit {                           // shell/root.rs:765-766
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep focus on the element carrying the key contexts. Guard from shell/root.rs:911-913.
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        // AppFrame supplies bg / text color / font / size / line height (app_frame.rs:110-122),
        // so a view composing it does not repeat them.
        div()
            .size_full()
            .key_context("Lazygit")                 // shell/root.rs:938
            .track_focus(&self.focus_handle)        // APP-CONTRACTS.md:75-78
            .child(
                AppFrame::new()                     // components/app_frame.rs:49
                    .body(div().size_full().child(Text::ui("fleet-lazygit")))
                    .status_bar(StatusBar::new().breadcrumb("no repo")),
            )
    }
}

/// Boots gpui and opens the window. Returns when the last window closes.
pub fn run(path: PathBuf) -> anyhow::Result<()> {   // shell/root.rs:1037
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ignored = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();                                // shell/root.rs:1023-1034

    gpui_platform::application()                    // shell/root.rs:1041
        .with_assets(KitAssets)                     // shell/root.rs:1042; assets.rs:36
        .run(move |cx: &mut App| {                  // gpui/src/app.rs:233
            Theme::init(ThemeMode::Dark, cx);       // shell/root.rs:1044; theme/theme.rs:170
            crate::keymap::init(cx);                // shell/root.rs:1045; keymap.rs:394-397

            cx.on_window_closed(|cx: &mut App, _window_id| {   // shell/root.rs:1051-1056
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds =
                Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx); // :1058
            let options = WindowOptions {                                                  // :1059
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
                titlebar: Some(TitlebarOptions {
                    title: Some("fleet-lazygit".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(px(12.0), px(12.0))),
                }),
                ..Default::default()
            };

            match cx.open_window(options, |_window, cx| cx.new(|cx| Lazygit::new(path, cx))) {
                Ok(window) => {                                          // shell/root.rs:1069
                    let _ignored = window.update(cx, |view, window, cx| {
                        window.activate_window();                        // shell/root.rs:1071
                        window.focus(&view.focus_handle(cx), cx);         // kit_gallery.rs:1490
                    });
                    cx.activate(true);                                   // shell/root.rs:1072
                }
                Err(error) => {
                    tracing::error!(%error, "fleet-lazygit: could not open the window");
                    eprintln!("fleet-lazygit: could not open the window: {error}");
                    cx.quit();                                           // shell/root.rs:1083
                }
            }
        });
    Ok(())
}
```

`crates/fleet-lazygit/src/main.rs` is in §f.1 — it must call
`fleet_git::sequence_editor::maybe_run_from_env()` before anything else.

**Boot gotchas for a second binary:**

- `Theme::init` must be inside `.run(...)`: it calls `cx.text_system()`
  (`crates/fleet-ui-kit/src/theme/theme.rs:145`), and skipping it makes `cx.theme()` panic
  (`crates/fleet-ui-kit/src/theme/theme.rs:250`-`:253`).
- Only one `.with_assets(...)` per `Application` (`gpui/src/app.rs:201`-`:207`). If you add
  assets of your own, delegate the `icons/` prefix to `kit_asset(path)` and return `Ok(None)` —
  never `Err` — for a miss (`crates/fleet-ui-kit/src/assets.rs:11`-`:12`).
- Without `cx.on_window_closed(...) -> cx.quit()` the process outlives the window.
## b. The keymap / actions pattern

This is the part of `fleet-app` a lazygit clone must copy most faithfully, because lazygit *is*
a keymap.

### b.1 Actions: one `pub mod` per namespace

```rust
// crates/fleet-app/src/actions.rs:1-10
//! GPUI actions shared across screens and modes.
//!
//! There is exactly **one action per row of `docs/KEYMAP.md`**, and the keystrokes that reach
//! them live in [`crate::keymap`]. Actions are grouped into namespaces that mirror the key
//! contexts of the keymap, so the same word (`Open`, `Delete`, `Cancel`) can mean different
//! things in disjoint panes without colliding: gpui registers an action under
//! `namespace::Name`.
//!
//! Nothing here holds state. A handler in [`crate::shell`] reduces the action against
//! [`crate::state::AppState`], and screens read the result.
```

A representative block, verbatim (`crates/fleet-app/src/actions.rs:12`-`:45`):

```rust
/// Actions in the `fleet` namespace.
pub mod fleet {
    use gpui::actions;

    actions!(
        fleet,
        [
            /// `ctrl-q` — quit the app; the daemon keeps running.
            Quit,
            /// `ctrl-shift-q` — quit the app and stop the daemon.
            QuitAndStopDaemon,
            /// `:` — open the command palette.
            OpenPalette,
            /// `,` — open settings.
            OpenSettings,
            /// `?` — open the help overlay.
            OpenHelp,
            /// `J` — open the jobs panel.
            OpenJobs,
            /// `!` — focus the sticky error slot.
            FocusStickyError,
            /// `r` — refresh status, PRs and discovery as a job.
            Refresh,
            /// `U` — update Fleet as a job.
            UpdateFleet,
            /// `Esc` — clear the filter, else close the topmost overlay, else no-op. Never quits.
            Cancel,
            /// `a` — open the Claude agent session.
            OpenAgentClaude,
            /// `A` — open the OpenCode agent session.
            OpenAgentOpencode,
        ]
    );
}
```

The pattern is exact: **one `pub mod <namespace>`, each with `use gpui::actions;` and exactly
one `actions!(<namespace>, [ … ])`**, with a doc comment naming the key on every variant (the
macro forwards `#[$attr]`, `gpui/src/action.rs:24`-`:33`).

`fleet-app` has **21 namespaces**, in file order: `fleet` (`crates/fleet-app/src/actions.rs:13`),
`hub` (`:48`), `repos` (`:123`), `worktrees` (`:146`), `prs` (`:179`), `workspace` (`:208`),
`prefix` (`:221`), `scroll` (`:284`), `filter` (`:325`), `palette` (`:350`), `jobs` (`:375`),
`dialog` (`:412`), `confirm` (`:449`), `create_worktree` (`:470`), `context_dialog` (`:487`),
`settings` (`:500`), `quit_dialog` (`:525`), `quit_daemon_dialog` (`:544`), `help` (`:559`),
`daemon` (`:572`), `first_run` (`:593`).

> **Pitfall from gpui itself** (`gpui/src/action.rs:53`): *"Registering the actions with the
> same name will result in a panic during `App` creation."* Keep `fleet-lazygit`'s namespaces
> distinct (`lazygit`, `files`, `branches`, `commits`, `stash`, `main_panel`, `staging`,
> `patch`, `merging`, `dialog`, `menu`, `prompt`, `search`, `command_log`).

### b.2 The binding table: one macro, three artifacts

```rust
// crates/fleet-app/src/keymap.rs:47-56
/// One row of the key table, in the order it is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSpec {
    /// The keystrokes, space separated for a sequence (`"g g"`).
    pub keys: &'static str,
    /// The gpui key-context predicate this row is scoped to.
    pub context: &'static str,
    /// The fully qualified action name, e.g. `hub::MoveDown`.
    pub action: &'static str,
}
```

```rust
// crates/fleet-app/src/keymap.rs:58-110  (abridged: the third arm is quoted in b.6)
macro_rules! key_table {
    ($( $keys:literal, $context:literal => $action:expr ; )*) => {
        /// Every binding, ready for [`gpui::App::bind_keys`].
        #[must_use]
        pub fn bindings() -> Vec<KeyBinding> {
            vec![$( KeyBinding::new($keys, $action, Some($context)) ),*]
        }

        /// The same table as data: keystrokes, context and action name.
        ///
        /// The help overlay and the command palette render their key hints from this, so a
        /// binding and its documentation can never drift apart.
        #[must_use]
        pub fn table() -> Vec<BindingSpec> {
            vec![$( BindingSpec {
                keys: $keys,
                context: $context,
                action: Action::name(&$action),
            } ),*]
        }

        /// Resolves one keystroke against one exact key context.
        #[must_use]
        pub fn action_for_keystroke(
            context: &str,
            keystroke: &Keystroke,
        ) -> Option<Box<dyn Action>> { /* … */ }
    };
}
```

```rust
// crates/fleet-app/src/keymap.rs:112-113
/// The root key context. Present on every screen, including the daemon surfaces.
pub const ROOT_CONTEXT: &str = "Fleet";
```

A chunk of the table itself (`crates/fleet-app/src/keymap.rs:115`-`:135`, `:170`-`:192`):

```rust
key_table! {
    // ---------------------------------------------------------------- global (§KEYMAP global)
    "ctrl-q",       "Fleet" => Quit;
    "ctrl-shift-q", "Fleet" => QuitAndStopDaemon;

    // ---------------------------------------------------------------- Hub, all panes
    "j",            "Hub" => hub::MoveDown;
    "down",         "Hub" => hub::MoveDown;
    "k",            "Hub" => hub::MoveUp;
    "up",           "Hub" => hub::MoveUp;
    "g g",          "Hub" => hub::GoTop;
    "G",            "Hub" => hub::GoBottom;
    "ctrl-d",       "Hub" => hub::HalfPageDown;
    "ctrl-u",       "Hub" => hub::HalfPageUp;
    "h",            "Hub" => hub::FocusPrevPane;
    "left",         "Hub" => hub::FocusPrevPane;
    "shift-tab",    "Hub" => hub::FocusPrevPane;
    "l",            "Hub" => hub::FocusNextPane;
    "right",        "Hub" => hub::FocusNextPane;
    "tab",          "Hub" => hub::FocusNextPane;
    "g r",          "Hub" => hub::GoRepos;
    // …
    // ---------------------------------------------------------------- Hub › Repos
    "enter",        "Hub > Repos" => repos::Open;
    "o",            "Hub > Repos" => repos::Open;
    "l",            "Hub > Repos" => repos::Open;
    "n",            "Hub > Repos" => repos::Clone;
    "d",            "Hub > Repos" => repos::Delete;
    // …
}
```

The table runs from `crates/fleet-app/src/keymap.rs:115` to `:392` — a test asserts more than
200 rows (`crates/fleet-app/src/keymap.rs:436`-`:441`).

**Registration is four lines** (`crates/fleet-app/src/keymap.rs:394`-`:397`):

```rust
/// Registers the whole table with the app.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
}
```

called once at `crates/fleet-app/src/shell/root.rs:1045`.

Space-separated keystrokes make a **sequence** (`gpui/src/keymap/binding.rs:56`-`:58`:
`keystrokes.split_whitespace()`), so `"g g"` is two keys and `"ctrl-shift-q"` is one. That is
how you get lazygit's `g g`-style chords.

### b.3 Key contexts and how nesting really works

The exact context strings `fleet-app` uses (`crates/fleet-app/src/keymap.rs:405`-`:430`):

```rust
    /// Every key context named in `docs/KEYMAP.md`.
    const REQUIRED_CONTEXTS: &[&str] = &[
        "Fleet",
        "Hub",
        "Hub > Repos",
        "Hub > Worktrees",
        "Hub > Prs",
        "Workspace > Terminal",
        "Workspace > Prefix",
        "Workspace > Scroll",
        "Filter",
        "Palette",
        "Jobs",
        "Dialog",
        "Dialog > Create",
        "Dialog > Confirm",
        "Dialog > Context",
        "Dialog > Assign",
        "Dialog > Settings",
        "Dialog > Help",
        "Dialog > Quit",
        "Dialog > QuitDaemon",
        "Daemon > Down",
        "Daemon > Banner",
        "FirstRun",
    ];
```

The chain is computed from state (`crates/fleet-app/src/state.rs:797`-`:845`,
`AppState::context_chain() -> Vec<&'static str>`); an overlay **replaces** the whole chain
rather than adding to it (`crates/fleet-app/src/state.rs:804`-`:812`,
`crates/fleet-app/src/state.rs:118`-`:128`).

**The single most important mechanical fact:** `key_context()` takes **one identifier**; the
`>` exists only in the *binding predicate*, and is satisfied by **nesting one div per word**.

```rust
// crates/fleet-app/src/shell/root.rs:697-712
    /// Wraps `child` in one div per key context, outermost first.
    ///
    /// The focused element is `child` itself — every screen and overlay tracks the shell's
    /// focus handle — so the dispatch path reads `Fleet > Hub > Worktrees > <screen>` and both
    /// the shell's listeners (above) and the screen's own (at the focus node) are on it.
    fn contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .size_full()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }
```

Overlays use an absolutely-positioned variant, and the reason is worth reading before you build
`fleet-lazygit`'s overlay stack (`crates/fleet-app/src/shell/root.rs:714`-`:734`):

```rust
    /// The same nesting, but as a **layer** rather than a flex child.
    ///
    /// An overlay is handed to [`AppFrame::overlay`] / [`AppFrame::body_overlay`], which emit
    /// their layers as children of a flex column. A `size_full` wrapper there is an in-flow
    /// item that eats the whole column, collapsing the body to zero and shoving the status bar
    /// under the context bar (§2.1: the chrome never moves). Absolute positioning takes the
    /// wrapper out of the flow, and — because gpui resolves an absolute child against its
    /// parent's box — also gives the dialog, sheet and palette inside it the frame's geometry.
    fn overlay_contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .absolute()
                .inset_0()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }
```

The root context sits on the top-level div (`crates/fleet-app/src/shell/root.rs:935`-`:943`).

**Why `>` cannot go inside `key_context()`** — gpui parses that string as identifiers and
`key = value` pairs only (`gpui/src/keymap/context.rs:59`-`:63`), and `>` is an operator
character (`gpui/src/keymap/context.rs:505`). In a *predicate* it is
`KeyBindingContextPredicate::Descendant` (`gpui/src/keymap/context.rs:178`-`:184`), which means
**ancestor-anywhere-above**, not immediate parent (`gpui/src/keymap/context.rs:300`-`:317`).

The repo's own summary (`crates/fleet-app/src/keymap.rs:1`-`:17`):

```text
Fleet > Hub > Worktrees          the Hub with the worktrees list focused
Fleet > Workspace > Prefix       a terminal, one key after ctrl-s
Fleet > Dialog > Confirm         any confirm dialog
```

> Because `Hub` is an ancestor of `Repos`, `Worktrees` and `Prs`, a binding on `Hub` is
> inherited by all three panes and a binding on `Hub > Prs` overrides it.

That inheritance is exactly what lazygit's "list panel navigation" table needs: bind `j`, `k`,
`,`, `.`, `<`, `>`, `/`, `]`, `[` once on `Lazygit`, and per-panel keys on
`Lazygit > Files` etc.

One conditional context in the wild (`crates/fleet-app/src/screens/jobs.rs:275`):

```rust
            .when(expanded.is_some(), |el| el.key_context("Log"))
```

which is how `"Jobs > Log"` becomes reachable only while a log is expanded — the same trick
`fleet-lazygit` needs for `Lazygit > Main > Staging`.

### b.4 Focus handles

`fleet-app`'s shell owns exactly **two** handles
(`crates/fleet-app/src/shell/root.rs:62`-`:76`):

```rust
pub struct Shell {
    state: Entity<AppState>,
    bridge: Bridge,
    /// Focused while the Hub or the Workspace owns the keyboard.
    body_focus: FocusHandle,
    /// Focused while a dialog, the palette, the filter or the jobs panel is open, so that an
    /// overlay's key context really does shadow the screen behind it.
    overlay_focus: FocusHandle,
    // …
}
```

Created with `cx.focus_handle()` in the constructor
(`crates/fleet-app/src/shell/root.rs:111`-`:121`).

**`track_focus` goes on the innermost element — inside the `key_context` wrappers.** The
wrappers are ancestors; the focused element is the child (`crates/fleet-app/src/shell/root.rs:847`,
`:871`, `crates/fleet-app/src/screens/hub.rs:566`, `crates/fleet-app/src/dialogs/mod.rs:310`).
Putting both on the *same* div is also legal
(`crates/fleet-ui-kit/src/components/text_field.rs:935`-`:939`).

Focus moves with `window.focus(&handle, cx)` inside `render`, guarded so it only fires on a
change (`crates/fleet-app/src/shell/root.rs:904`-`:913`):

```rust
        // Exactly one of the two handles is focused, and it is the one attached to the element
        // that carries the key-context chain.
        let wanted = if overlay_element.is_some() {
            &self.overlay_focus
        } else {
            &self.body_focus
        };
        if !wanted.is_focused(window) {
            window.focus(wanted, cx);
        }
```

`cx.focus_self()` is **not used anywhere in this repo**.

The frozen contract (`docs/APP-CONTRACTS.md:75`-`:78`):

> **The returned root element must call `.track_focus(focus)`.** That is what puts your
> `on_action` listeners on the key-dispatch path: gpui dispatches an action from the focused
> node upwards, so a listener *below* the focus node never fires.

### b.5 Overlays and dialogs

**Dialogs do not own a focus handle or a key context** — the shell hands them one of its two
handles and applies the context above them
(`crates/fleet-app/src/dialogs/mod.rs:1`-`:7`). The context word comes from
`Dialogs::context_name()` (`crates/fleet-app/src/dialogs/mod.rs:87`-`:107`), and every dialog
returns the same root (`crates/fleet-app/src/dialogs/mod.rs:307`-`:311`):

```rust
/// The root element every dialog returns: focus-tracking and full-window, so the card's own
/// scrim covers the screen behind it.
pub(crate) fn root(focus: &FocusHandle) -> Div {
    div().track_focus(focus).size_full()
}
```

**How the underlying keymap is blocked** — it is *not* an event swallow. Two mechanisms
together:

1. `context_chain()` returns **only** the overlay's chain when one is open
   (`crates/fleet-app/src/state.rs:804`-`:812`), so `Hub`'s `j`/`k` predicates no longer match
   at all;
2. focus moves to `overlay_focus`, which is attached to the overlay element
   (`crates/fleet-app/src/shell/root.rs:906`-`:913`), so the screen's listeners are off the
   dispatch path.

```rust
// crates/fleet-app/src/shell/root.rs:897-902
        // The focused element carries the key-context chain: the overlay when one is open,
        // the body otherwise, so overlays really do shadow the Hub and the Workspace.
        let (body, overlay_element) = match overlay_element {
            Some(element) => (body, Some(Self::overlay_contexts(&chain, element))),
            None => (Self::contexts(&chain, body), None),
        };
```

> **This is exactly lazygit's search-prompt rule** (reference §b.2,
> `pkg/gui/keybindings.go:331`-`:342`): while a prompt is open, *only* its bindings exist. In
> `fleet-lazygit`, model overlays as a stack whose top entry replaces the context chain.

Dismissal is entirely the shell's: every close handler calls one `close_overlay`
(`crates/fleet-app/src/shell/root.rs:262`-`:280`, `:193`-`:206`). Global listeners are
installed in exactly one place, on the root div, and the comment explains why it must be *all*
of them on *both* render branches (`crates/fleet-app/src/shell/root.rs:947`-`:1007`):

```rust
    /// Installs every global action listener on `root`.
    ///
    /// Both branches of [`Shell::render`] use it, splash included: a surface that registers a
    /// subset silently swallows the keys it left out — `ctrl-shift-q` and `Esc` among them —
    /// and there is no way back out of whatever the missing key was meant to leave.
    fn with_actions(root: Div, cx: &mut Context<Self>) -> Div {
```

### b.6 Printable-key text input versus action bindings

**This is the single highest-risk area for a lazygit clone**, because lazygit binds almost
every bare letter *and* has text prompts.

The governing fact: **gpui dispatches key bindings before any `on_key_down` listener.** There
are three mechanisms in this repo, and all three coexist.

**(a) `on_key_down` inspecting `keystroke.key_char` — the app's default.**

```rust
// crates/fleet-app/src/dialogs/mod.rs:435-461
/// The printable character a keystroke types, or `None` when it is not text input.
///
/// gpui dispatches key **bindings** before `on_key_down`, so a bound key never reaches this;
/// what arrives is exactly the printable set `docs/KEYMAP.md` gives to text inputs.
#[must_use]
pub fn typed_char(event: &KeyDownEvent) -> Option<String> {
    let modifiers = event.keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    let text = event.keystroke.key_char.as_deref()?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_owned())
}

/// Types a printable keystroke into `input`. Returns whether the buffer changed.
pub fn type_into(input: &mut TextInput, event: &KeyDownEvent) -> bool {
    match typed_char(event) {
        Some(text) => {
            input.insert(&text);
            true
        }
        None => false,
    }
}
```

The real closure, from the filter bar
(`crates/fleet-app/src/dialogs/filter.rs:135`-`:150`):

```rust
    div()
        .track_focus(focus)
        .size_full()
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let Some(text) = typed_char(event) else {
                    return;
                };
                state.update(cx, |app, cx| {
                    app.filter.query.push_str(&text);
                    app.cursors.worktrees = 0;
                    app.cursors.repos = 0;
                    cx.notify();
                });
            }
        })
```

Non-printable editing keys (backspace, cursor motion, confirm, cancel) stay **declarative
actions** (`crates/fleet-app/src/dialogs/filter.rs:151`-`:189`,
`crates/fleet-app/src/dialogs/palette.rs:590`-`:624`).

The app's own buffer is a plain struct with a **character** caret
(`crates/fleet-app/src/dialogs/mod.rs:313`-`:333`):

```rust
/// A single-line text buffer with a caret, edited by the keys `docs/KEYMAP.md` lists for
/// dialog inputs.
///
/// The caret is a **character** offset, never a byte offset, so multi-byte input cannot split
/// a grapheme in half.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    value: String,
    caret: usize,
}
```

**(b) The kit's `TextInput` entity + gpui's `EntityInputHandler` / `ElementInputHandler`.**
This is the real-editing path: IME, dead keys, mouse caret placement, auto-scroll. See §c for
the full API. `fleet-app` does **not** use it; the gallery does
(`crates/fleet-ui-kit/examples/gallery_input.rs:175`-`:188`).

**The double-insert pitfall, stated twice in the kit**
(`crates/fleet-ui-kit/src/components/text_field.rs:629`-`:634` and `:870`-`:871`):

> For callers that render the presentational `TextField` and receive raw key events […] A field
> that installs the platform input handler must use `TextFieldState::handle_edit_keystroke`
> instead, **or every character is inserted twice.**

**The bare-letter pitfall** (`crates/fleet-ui-kit/src/components/text_field.rs:943`-`:947`):

```rust
/// The gpui key context a [`TextInput`] pushes.
///
/// An app that binds bare letters (`h`, `q`, `y`) must shadow them in this context with
/// `gpui::NoAction`, or typing those letters fires the action instead of reaching the field.
pub const TEXT_FIELD_KEY_CONTEXT: &str = "FleetTextField";
```

**`fleet-lazygit` will hit this immediately** — it binds `d`, `c`, `p`, `s`, `f`, `r`, `n`, `e`,
`o`, `a`, `w`, `g`, `m`, `M`, `t`, `T`, `b`, `i`, `v` and more as bare letters. Every one of
them must be shadowed with `gpui::NoAction` in `FleetTextField` (and in your own prompt
context), or the commit-message field will run a rebase.

**(c) Surrendering a bound printable key inside the action handler.** When a printable key is
bound in a context that *also* hosts a focused text input, the surrender has to happen in the
action handler, because nothing else sees the key
(`crates/fleet-app/src/dialogs/settings.rs:1060`-`:1095`):

```rust
/// Every **printable** key `Dialog > Settings` binds, and therefore every key a focused text
/// input has to be handed back as a character (§3.8.6).
///
/// It is a tripwire, not a dispatch table: each handler calls [`insert_literal`] with its own
/// literal, and the test below fails the moment a new printable binding is added here without
/// that call — which is how `E` and `D` came to replace the screen mid-word instead of typing.
pub const SURRENDERED_KEYS: &[&str] = &["space", "h", "l", "j", "k", "E", "D"];

/// Inserts a bound key's literal character when a text input owns the keyboard.
///
/// §3.8.6 surrenders `j`, `k`, `h`, `l` and `Space` to a **focused** input. gpui dispatches
/// those bindings before any key listener, so the surrender has to happen inside the action
/// handler: there is no other place that sees the key.
fn insert_literal(state: &Entity<AppState>, literal: &str, cx: &mut App) -> bool {
```

The cleaner rule, enforced as a test (`crates/fleet-app/src/keymap.rs:548`-`:560`):

```rust
    #[test]
    fn palette_and_filter_never_bind_printable_keys() {
        for spec in table() {
            if spec.context == "Palette" || spec.context == "Filter" {
                assert!(
                    spec.keys.len() > 1,
                    "`{}` in `{}` would shadow typing",
                    spec.keys,
                    spec.context
                );
            }
        }
    }
```

> **Adopt this rule wholesale for `fleet-lazygit`:** a context that hosts a text field binds
> **no single-character key**. `Lazygit > Dialog > Prompt`, `Lazygit > Search` and the commit
> message panel get `<enter>`, `<esc>`, `<up>`, `<down>`, `ctrl-*` only — which is exactly
> what lazygit itself does (reference §b.2 and the "Input prompt" / "Commit summary" tables).

**(d) The one-frame-lag escape hatch.** gpui's *rendered* context tree is one frame behind a
state change, so a fast two-key sequence across a mode switch can be dropped. `fleet-app`
solves it with `cx.intercept_keystrokes` plus a table-derived resolver
(`crates/fleet-app/src/shell/root.rs:86`-`:107`, `:41`-`:55`, and the third macro arm at
`crates/fleet-app/src/keymap.rs:85`-`:109`). `fleet-lazygit` needs the same technique if it
ever wants a modal sub-context entered by a key and exited by the very next key.

### b.7 Help and key hints are generated from the table

The contract (`docs/APP-CONTRACTS.md:214`-`:222`):

> `keymap::table()` returns the whole binding table as data — `{ keys, context, action }` — in
> registration order. The help overlay (§3.8.7) and the palette's right-aligned key hints must
> render from it rather than restating keys, so a binding and its documentation cannot drift.

```rust
// crates/fleet-app/src/dialogs/help.rs:1-8
//! §3.8.7 Help (`?`) — every context side by side, grouped by mode.
//!
//! The rows are generated from [`crate::keymap::table`], never restated, so a binding and its
//! documentation cannot drift (`docs/APP-CONTRACTS.md` §6). The action label is the action's
//! own name, de-camel-cased; that is what makes the guarantee mechanical.
```

Grouping is a static map from a group title to the contexts that feed it
(`crates/fleet-app/src/dialogs/help.rs:126`-`:156`); the generator merges two bindings into one
row only when they share **both** the label and the context
(`crates/fleet-app/src/dialogs/help.rs:158`-`:198`). The two formatters are the whole trick:

```rust
// crates/fleet-app/src/dialogs/help.rs:251-282
/// `hub::MoveDown` becomes `move down`.
#[must_use]
pub fn humanize(action: &str) -> String {
    let name = action.rsplit("::").next().unwrap_or(action);
    let mut words = String::new();
    for (index, character) in name.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            words.push(' ');
        }
        words.extend(character.to_lowercase());
    }
    words
}

/// `ctrl-n` becomes `^n`, `shift-tab` becomes `S-⇥`; the mono column is narrow.
#[must_use]
pub fn pretty_keys(keys: &str) -> String {
    keys.split(' ')
        .map(|stroke| {
            stroke
                .replace("ctrl-", "^")
                .replace("shift-", "S-")
                .replace("alt-", "\u{2325}")
                .replace("escape", "esc")
                .replace("enter", "\u{23ce}")
                .replace("tab", "\u{21e5}")
                .replace("space", "\u{2423}")
                .replace("backspace", "\u{232b}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}
```

The palette reuses the same table for its right-hand key column
(`crates/fleet-app/src/dialogs/palette.rs:296`-`:303`).

> **For `fleet-lazygit` this is a gift.** lazygit's `?` menu and its bottom keybinding bar are
> both generated from its own binding registry. Do the same: `keymap::table()` feeds the `?`
> help dialog **and** the `KeyHintRow` in the status bar, filtered by the current context
> chain. Never hand-write a hint string that duplicates a binding.
>
> Note the known drift surface: `StatusBar` and `KeyHint` do **not** read the table in
> `fleet-app` — per-view hint rows there are hand-written
> (`crates/fleet-ui-kit/src/components/key_hint.rs:1`-`:11`). Close that gap in
> `fleet-lazygit` by deriving the hint row from `table()` filtered on
> `state.context_chain()`.

### b.8 Rules a new binary must follow

From `docs/KEYMAP.md`:

- `docs/KEYMAP.md:3`-`:4` — the file is authoritative; lowercase = safe action, uppercase =
  stronger variant. `docs/KEYMAP.md:11`-`:12` — where docs and code disagree, the doc wins.
- `docs/KEYMAP.md:5`-`:9` — reserved keys: **`ctrl-c` never quits**; quit is `ctrl-q`,
  quit+stop-daemon is `ctrl-shift-q`; **`Esc` never quits**; **`q` never quits** (it closes the
  topmost overlay, and is unbound when nothing is open); **no bare key over a terminal grid**.
- `docs/KEYMAP.md:16`-`:27` — the mode ⇄ context table. `:29`-`:31` — the stacking rule.
- `docs/KEYMAP.md:180`-`:183` — the text-input edit set is fixed: printable, `Backspace`,
  `ctrl-w`, `ctrl-u`, `ctrl-a`/`ctrl-e`, `←`/`→`; **a list under an input uses `ctrl-n`/`ctrl-p`
  or `↓`/`↑`, never `j`/`k`, because the text field owns them**; `Tab`/`S-Tab` between fields;
  `Enter` confirms, `Esc` cancels.

> `fleet-lazygit` deviates deliberately on `q`: lazygit binds `q` to Quit
> (`pkg/config/user_config.go:997`). Since this is a separate binary with its own root context
> and no terminal grid, binding `q` to quit inside `Lazygit` is defensible — but it must **not**
> be bound in any context hosting a text field, and the deviation belongs in a
> `docs/APP-CONTRACTS.md`-style note in the new crate.

From `docs/APP-CONTRACTS.md`:

- `:9`-`:10` — everything there is frozen; a signature change is a cross-agent change.
- `:75`-`:78` — invariant #1: the root element **must** `.track_focus(focus)`.
- `:79`-`:81` — read/write state through the entity; `cx.notify()` inside the update closure
  repaints.
- `:118`-`:120` — `context_chain()` returns the nested contexts outermost first; the shell
  renders one `div().key_context(_)` per entry; the root is always `Fleet`.
- `:136`-`:139` — a deeper context wins.
- **`:224`-`:241` — the propagation contract, the most load-bearing paragraph here:**

  > gpui dispatches an action up the focus chain and, **in the bubble phase, stops at the first
  > listener that handles it** […] A listener that does part of the work and expects the shell
  > to do the rest must therefore end with `cx.propagate()`. There is no "also run the outer
  > handler" by default, **and the failure is silent**: the key is consumed and nothing else
  > happens. […] The mirror-image rule holds too: a listener that fully handles an action and
  > must stop an outer one from *also* acting calls `cx.stop_propagation()` explicitly.

`fleet-app` also ships nine invariant tests over the table
(`crates/fleet-app/src/keymap.rs:432`-`:560`): well-formedness and row count (`:432`), every
documented context bound (`:443`), no `(context, keys)` duplicate (`:454`), chord completeness
(`:467`), the terminal has exactly one app key (`:478`), literal-prefix passthrough (`:488`),
live resolution agrees with the table (`:497`), quitting is global and `Esc` never quits
(`:521`), and no printable key in a text context (`:548`). **Port the analogous set to
`fleet-lazygit`** — especially "no `(context, keys)` duplicate" and "no printable key in a text
context".

### b.9 A minimal copyable example

```rust
// ---------------------------------------------------------------- actions.rs
// Shape: crates/fleet-app/src/actions.rs:12-45

/// Actions in the `files` namespace (lazygit's Files panel).
pub mod files {
    use gpui::actions;

    actions!(
        files,
        [
            /// `<space>` — toggle staged for the selected file.
            ToggleStaged,
            /// `a` — toggle staged/unstaged for every file in the working tree.
            ToggleStagedAll,
            /// `<enter>` — enter the staging view, or collapse/expand a directory.
            EnterFile,
        ]
    );
}
// Registered names: `files::ToggleStaged`, `files::ToggleStagedAll`, `files::EnterFile`
// (crates/fleet-app/src/actions.rs:6-7; gpui/src/action.rs:26-27).

// ---------------------------------------------------------------- keymap.rs
// Shape: crates/fleet-app/src/keymap.rs:47-113, :394-397

use gpui::{Action, App, KeyBinding};

/// One row of the key table, in the order it is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSpec {
    pub keys: &'static str,
    pub context: &'static str,
    pub action: &'static str,
}

/// The root key context.                              // keymap.rs:112-113
pub const ROOT_CONTEXT: &str = "Lazygit";

macro_rules! key_table {                                // keymap.rs:58-83
    ($( $keys:literal, $context:literal => $action:expr ; )*) => {
        #[must_use]
        pub fn bindings() -> Vec<KeyBinding> {
            // gpui/src/keymap/binding.rs:33
            vec![$( KeyBinding::new($keys, $action, Some($context)) ),*]
        }

        #[must_use]
        pub fn table() -> Vec<BindingSpec> {
            vec![$( BindingSpec {
                keys: $keys,
                context: $context,
                action: Action::name(&$action),          // keymap.rs:81
            } ),*]
        }
    };
}

key_table! {
    // Inherited by every panel, exactly like lazygit's "List panel navigation".
    "j",       "Lazygit" => list::MoveDown;
    "down",    "Lazygit" => list::MoveDown;
    "k",       "Lazygit" => list::MoveUp;
    "up",      "Lazygit" => list::MoveUp;
    "]",       "Lazygit" => list::NextTab;
    "[",       "Lazygit" => list::PrevTab;
    "1",       "Lazygit" => panel::FocusStatus;
    "2",       "Lazygit" => panel::FocusFiles;

    // Panel-specific. `>` lives ONLY in the predicate, never in key_context().
    "space",   "Lazygit > Files" => files::ToggleStaged;
    "a",       "Lazygit > Files" => files::ToggleStagedAll;
    "enter",   "Lazygit > Files" => files::EnterFile;

    // A text context binds NO single-character key (keymap.rs:548-560).
    "enter",   "Lazygit > Dialog > Prompt" => prompt::Confirm;
    "escape",  "Lazygit > Dialog > Prompt" => prompt::Cancel;
    "ctrl-u",  "Lazygit > Dialog > Prompt" => prompt::ClearLine;
}

/// Registers the whole table.                          // keymap.rs:394-397
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());                           // gpui/src/app.rs:2235
}
// Call inside Application::run, right after Theme::init — shell/root.rs:1044-1045.

// ---------------------------------------------------------------- a panel view
use fleet_ui_kit::prelude::*;
use gpui::{Context, FocusHandle, Focusable, Render, Window, div, prelude::*};

pub struct FilesPanel {
    focus_handle: FocusHandle,
    scroll: gpui::UniformListScrollHandle,
    cursor: ListCursor,
}

impl FilesPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),            // shell/root.rs:114
            scroll: gpui::UniformListScrollHandle::new(),   // screens/hub.rs:518
            cursor: ListCursor::new(0),                 // list_view.rs:132
        }
    }

    // Handler signature from shell/root.rs:208-210.
    fn move_down(&mut self, _: &list::MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        let moving_down = self.cursor.motion(ListMotion::Down);   // list_view.rs:223
        ListView::reveal(&self.scroll, &self.cursor, moving_down); // list_view.rs:352
        cx.notify();                                    // APP-CONTRACTS.md:79-81
    }

    fn toggle_staged(&mut self, _: &files::ToggleStaged, _: &mut Window, cx: &mut Context<Self>) {
        // Dispatch to the bridge; never run git here (see §d and §f.5).
        cx.notify();
    }
}

impl Focusable for FilesPanel {                         // shell/root.rs:759-763
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FilesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_handle.is_focused(window) {      // shell/root.rs:911-913
            window.focus(&self.focus_handle, cx);
        }
        // OUTER div = one context word; nesting satisfies "Lazygit > Files".
        div()
            .size_full()
            .key_context(ROOT_CONTEXT)                  // shell/root.rs:938
            .child(
                div()
                    .size_full()
                    .key_context("Files")               // -> predicate "Lazygit > Files"
                    .track_focus(&self.focus_handle)    // APP-CONTRACTS.md:75-78
                    .on_action(cx.listener(Self::move_down))       // shell/root.rs:956
                    .on_action(cx.listener(Self::toggle_staged))
                    .child(
                        ListView::new("lazygit-files", 0, |_, _, _, _| {
                            gpui::div().into_any_element()
                        })
                        .cursor(self.cursor.index())
                        .track_scroll(&self.scroll),
                    ),
            )
    }
}
```
## c. fleet-ui-kit component inventory

`crates/fleet-ui-kit` is the design system. Its module doc states the crate's three rules
(`crates/fleet-ui-kit/src/lib.rs:12`-`crates/fleet-ui-kit/src/lib.rs:18`):

1. **No domain types.** Components take `SharedString`, scalars and closures — never a
   `RepoSnapshot`, never a `Commit`. `fleet-lazygit` maps git types to strings at the view
   boundary.
2. **No literal colors, sizes or durations.** Everything goes through `theme::Theme` via the
   `ActiveTheme` extension trait.
3. **One `AssetSource` per app.** The binary must call `.with_assets(fleet_ui_kit::KitAssets)`
   or every icon renders as nothing.

### c.0 Crate root and prelude

```rust
// crates/fleet-ui-kit/src/lib.rs:43-50
pub mod assets;
pub mod components;
pub mod focus;
pub mod icons;
pub mod text;
pub mod theme;
pub mod tone;
pub mod truncate;

// crates/fleet-ui-kit/src/lib.rs:52-58
pub use assets::{KitAssets, kit_asset, kit_asset_paths};
pub use components::*;
pub use icons::{Icon, IconElement, IconSize};
pub use text::{Text, TextRole, styled_with};
pub use theme::{ActiveTheme, Theme, ThemeMode};
pub use tone::Tone;
pub use truncate::{ELLIPSIS, Truncate, truncate};

// crates/fleet-ui-kit/src/lib.rs:61-69 — "Everything a view needs in one `use`."
pub mod prelude {
    pub use crate::components::*;
    pub use crate::icons::{Icon, IconElement, IconSize};
    pub use crate::text::{Text, TextRole};
    pub use crate::theme::{ActiveTheme, Theme, ThemeMode};
    pub use crate::tone::Tone;
    pub use crate::truncate::{Truncate, truncate};
    pub use gpui::prelude::*;
}
```

**Every view file in `fleet-lazygit` should start with `use fleet_ui_kit::prelude::*;`** — it
re-exports `gpui::prelude::*`, which is what brings the `IntoElement`, `Styled`,
`ParentElement`, `InteractiveElement` and `StatefulInteractiveElement` traits into scope. Most
"method not found on `Div`" errors in gpui v1.18.1 are a missing prelude import, not a missing
API.

The full public re-export list lives at
`crates/fleet-ui-kit/src/components/mod.rs:66`-`crates/fleet-ui-kit/src/components/mod.rs:136`;
two aliases are defined below it: `pub type Modal = Dialog;`
(`crates/fleet-ui-kit/src/components/mod.rs:140`) and `pub type TabBar = SegmentedTabs;`
(`crates/fleet-ui-kit/src/components/mod.rs:145`).

### c.1 Token values you will reference constantly

From `crates/fleet-ui-kit/src/theme/tokens.rs` (all in `px`):

| Group | Values |
|---|---|
| `space` | `xxs=2 xs=4 sm=8 md=12 lg=16 xl=24 xxl=32` |
| `radii` | `xs=3 sm=4 md=6 lg=12` |
| `metrics` | `hairline=1`, `context_bar_h=36`, `status_bar_h=26`, `pane_header_h=30`, `row_h=30`, `palette_row_h=34`, `job_row_h=44`, `section_header_h=20`, `banner_h=28`, `chip_h=22`, `scroll_thumb_w=3`, `focus_ring_w=2`, `dimmed_opacity=0.40` |

### c.2 Structure and layout components

#### `Pane` — `crates/fleet-ui-kit/src/components/pane.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`pane.rs:39`-`pane.rs:40`, `impl` at `pane.rs:133`),
plus `impl Default` (`pane.rs:127`).

```rust
// crates/fleet-ui-kit/src/components/pane.rs:25-36
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaneBorder {
    None,        // :28  no border
    Right,       // :30  hairline right edge
    #[default]
    Left,        // :33  hairline left edge
    Horizontal,  // :35  both vertical edges
}

// crates/fleet-ui-kit/src/components/pane.rs:54
pub fn new() -> Self
// crates/fleet-ui-kit/src/components/pane.rs:69
pub fn fixed(width: Pixels) -> Self

// pane.rs:74   pub fn header(mut self, header: impl IntoElement) -> Self
// pane.rs:80   pub fn body(mut self, body: impl IntoElement) -> Self
// pane.rs:86   pub fn footer(mut self, footer: impl IntoElement) -> Self
// pane.rs:92   pub fn focused(mut self, focused: bool) -> Self
// pane.rs:98   pub fn width(mut self, width: Pixels) -> Self        // also sets flex = false
// pane.rs:105  pub fn border(mut self, border: PaneBorder) -> Self
// pane.rs:111  pub fn raised(mut self, raised: bool) -> Self
// pane.rs:121  pub fn scroll_thumb(mut self, offset: f32, visible: f32) -> Self  // both clamped 0..=1
```

Renders (`pane.rs:133`-`pane.rs:206`) a
`div().relative().flex().flex_col().h_full().min_w_0().overflow_hidden()` that is `flex_1()`
when flexible or `w(width).flex_none()` when fixed; `bg(colors.surface)` only when `raised`;
one of `border_l`/`border_r`/both at 1 px in `colors.border` per `PaneBorder`. Its child is
`FocusRing::pane(focused)` wrapping a column of: an optional `flex_none` header pinned at
`metrics.pane_header_h` (30 px), a `flex_1 min_h_0 overflow_hidden` body, and an optional
`flex_none` footer. With `scroll_thumb` set and `visible < 1.0` it appends an absolutely
positioned `right_0` bar, `w(metrics.scroll_thumb_w)` (3 px), `bg(colors.scroll_thumb)`, height
floored at 4 % so the thumb never becomes a dot (`pane.rs:192`-`pane.rs:195`).

**This is the lazygit side-panel primitive.** Each of the five side panels is one `Pane` with a
`PaneHeader` header and a `ListView` body; the main panel is a `Pane` with
`PaneBorder::Left`.

Usage — `crates/fleet-app/src/views/repos_rail.rs:287`:
```rust
    let mut pane = Pane::fixed(px(if collapsed { COLLAPSED_WIDTH } else { 240.0 }))
        .border(PaneBorder::Right)
        .focused(focused)
        .body(list);
```

#### `PaneHeader` — `crates/fleet-ui-kit/src/components/pane_header.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`pane_header.rs:36`-`:37`, `impl` at `:117`).

```rust
// crates/fleet-ui-kit/src/components/pane_header.rs:52
pub fn new(label: impl Into<SharedString>) -> Self

// pane_header.rs:68   pub fn scope(mut self, scope: impl Into<SharedString>) -> Self
// pane_header.rs:74   pub fn total(mut self, total: usize) -> Self
// pane_header.rs:81   pub fn shown(mut self, shown: usize) -> Self
// pane_header.rs:87   pub fn range(mut self, first: usize, last: usize) -> Self
// pane_header.rs:93   pub fn stale(mut self, age: impl Into<SharedString>) -> Self
// pane_header.rs:99   pub fn filter_chip(mut self, query: impl Into<SharedString>) -> Self
// pane_header.rs:105  pub fn filter(mut self, filter: impl IntoElement) -> Self
// pane_header.rs:111  pub fn trailing(mut self, trailing: impl IntoElement) -> Self
```

Renders (`:117`-`:193`) a `flex items_center justify_between size_full` row — it does **not**
set its own height, `Pane` pins it at 30 px — with `px(space.lg)` (16), `gap(space.md)` (12) and
`border_b(1px)` in `colors.border`. Left: either the `filter` element in a `flex_1 min_w_0` box
(an in-place swap with zero layout shift — exactly how lazygit's `/` filter replaces a panel
title), or `Text::label(label)` (uppercased) + `Text::ui("· {scope}").faint().ellipsize()` + an
optional accent `Icon::Search` filter chip + an optional `Tone::Warning` stale label. Right:
`"{shown}/{total}"` (or `"{total}"`), then `"{first}–{last}/{total}"` (only when `range` **and**
`total` are both set — `.range()` alone renders nothing, `:126`-`:128`), then `trailing`.

Usage — `crates/fleet-app/src/views/worktrees_list.rs:355`:
```rust
    let mut header = PaneHeader::new("Worktrees")
        .scope(scope.clone())
        .total(total);
```

#### `SplitLayout` / `SplitAxis` — `crates/fleet-ui-kit/src/components/split_layout.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:27`-`:28`, `impl` at `:90`).

```rust
// crates/fleet-ui-kit/src/components/split_layout.rs:17-24
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplitAxis { #[default] Horizontal, Vertical }

// crates/fleet-ui-kit/src/components/split_layout.rs:39
pub fn horizontal() -> Self
// crates/fleet-ui-kit/src/components/split_layout.rs:52
pub fn vertical() -> Self

// split_layout.rs:59  pub fn leading(mut self, leading: impl IntoElement) -> Self
// split_layout.rs:65  pub fn trailing(mut self, trailing: impl IntoElement) -> Self
// split_layout.rs:71  pub fn leading_size(mut self, size: Pixels) -> Self
// split_layout.rs:77  pub fn trailing_size(mut self, size: Pixels) -> Self
// split_layout.rs:84  pub fn divider(mut self, divider: bool) -> Self
```

Renders (`:90`-`:136`) `div().flex().size_full().min_w_0().min_h_0()` with `flex_row()` or
`flex_col()`. A sized region gets `w(size).h_full().flex_none()` (horizontal) or
`h(size).w_full().flex_none()` (vertical); an unsized one gets `flex_1()`. The 1 px
`bg(colors.border)` divider is **suppressed unless both `leading` and `trailing` are present**
(`:94`).

**This is how you build the lazygit frame**: an outer `SplitLayout::horizontal()` with
`leading_size(side_column_width)` (side column) and a flexible trailing (main area), and an
inner `SplitLayout::vertical()` or `::horizontal()` for main + secondary, per the
`mainPanelSplitMode` behaviour documented in the reference §a.

Usage — `crates/fleet-app/src/screens/hub.rs:766`:
```rust
        let mut split = SplitLayout::horizontal()
            .leading(rail)
            .trailing(list)
            .divider(false);
```

#### `ListView` + `ListCursor` — `crates/fleet-ui-kit/src/components/list_view.rs`

The single most important component. `ListView` is `#[derive(IntoElement)]` + `RenderOnce`
(`:270`-`:271`, `impl` at `:357`); `ListCursor` is a **plain `Copy` struct with no gpui
dependency**, fully unit-testable (`:122`-`:128`).

**Actions, bindings and constants**

```rust
// crates/fleet-ui-kit/src/components/list_view.rs:38-54
gpui::actions!(
    fleet_list,
    [
        ListDown,      // :41  j / down
        ListUp,        // :43  k / up
        ListFirst,     // :45  gg
        ListLast,      // :47  G
        ListPageDown,  // :49  ctrl-d
        ListPageUp,    // :51  ctrl-u
    ]
);

// list_view.rs:60-74
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMotion { Down, Up, First, Last, PageDown, PageUp }

// list_view.rs:79   pub fn moves_down(self) -> bool     // true for Down | Last | PageDown
// list_view.rs:93   pub fn list_key_bindings(context: Option<&str>) -> Vec<KeyBinding>
// list_view.rs:107  pub type RenderRow = Rc<dyn Fn(usize, bool, &mut Window, &mut App) -> AnyElement>;
// list_view.rs:110  pub const SCROLLOFF: usize = 2;
// list_view.rs:113  pub const DEFAULT_PAGE: usize = 10;
// list_view.rs:116  pub const SKELETON_ROWS: usize = 6;
```

`list_key_bindings` returns exactly eight bindings (`:94`-`:103`): `j`/`down` → `ListDown`,
`k`/`up` → `ListUp`, `g g` → `ListFirst`, `shift-g` → `ListLast`, `ctrl-d` → `ListPageDown`,
`ctrl-u` → `ListPageUp`. Pass a gpui key-context predicate, e.g.
`list_key_bindings(Some("Lazygit > Files"))`; `None` binds globally and is only correct in a
demo.

> **lazygit mapping:** these defaults already match lazygit's `j`/`k`/`<up>`/`<down>` and the
> `<ctrl+u>`/`<ctrl+d>` main-scroll keys. lazygit's `<`/`>`/`<home>`/`<end>` map onto
> `ListFirst`/`ListLast`, and lazygit's `,`/`.` (prev/next page) onto
> `ListPageUp`/`ListPageDown`. You will add those extra keys in your own table
> (§b) rather than editing the kit.

**`ListCursor` — the complete API** (note the names; they are *not* `selected`/`select`/
`move_by`/`move_to`/`clamp`):

```rust
// crates/fleet-ui-kit/src/components/list_view.rs:132
pub fn new(len: usize) -> Self                       // index 0, scrolloff 2, page 10

// list_view.rs:142  pub fn scrolloff(mut self, scrolloff: usize) -> Self
// list_view.rs:148  pub fn page(mut self, page: usize) -> Self                 // page.max(1)
// list_view.rs:155  pub fn set_page_from_visible(&mut self, visible_rows: usize)  // page = (visible/2).max(1)
// list_view.rs:160  pub fn index(&self) -> usize
// list_view.rs:165  pub fn len(&self) -> usize
// list_view.rs:170  pub fn is_empty(&self) -> bool
// list_view.rs:175  pub fn scrolloff_rows(&self) -> usize
// list_view.rs:180  pub fn page_rows(&self) -> usize
// list_view.rs:185  pub fn set(&mut self, index: usize)          // index.min(len-1), saturating
// list_view.rs:190  pub fn down(&mut self)
// list_view.rs:195  pub fn up(&mut self)
// list_view.rs:200  pub fn first(&mut self)
// list_view.rs:205  pub fn last(&mut self)
// list_view.rs:210  pub fn page_down(&mut self)
// list_view.rs:216  pub fn page_up(&mut self)
// list_view.rs:223  pub fn motion(&mut self, motion: ListMotion) -> bool
// list_view.rs:241  pub fn retain(&mut self, new_len: usize, find_previous: impl FnOnce(usize) -> Option<usize>)
// list_view.rs:250  pub fn set_len(&mut self, len: usize)
// list_view.rs:260  pub fn scroll_target(&self, moving_down: bool) -> usize
```

`motion` applies the motion and **returns `motion.moves_down()`** — exactly the third argument
`ListView::reveal` wants, so all six actions collapse to one call site.

**`retain` semantics** (`:241`-`:247`) — this is the cursor-stability mechanism and the answer
to "how does the selection survive a background refresh?":

```rust
    pub fn retain(&mut self, new_len: usize, find_previous: impl FnOnce(usize) -> Option<usize>) {
        let previous = self.index;
        self.len = new_len;
        self.index = find_previous(previous)
            .unwrap_or(previous)
            .min(new_len.saturating_sub(1));
    }
```

The closure receives the **previously selected index** and returns where that same item landed
in the new data, or `None` if it is gone. `Some(new_ix)` follows the item; `None` keeps the old
numeric index clamped into the new length. Test `retain_follows_the_item` at
`list_view.rs:422`-`:429`.

> **This is the single most important behaviour to get right in `fleet-lazygit`.** Every git
> mutation triggers a refresh; the file/branch/commit you were looking at must not jump. Key
> each list by a stable identity (`FileStatus.path`, `Branch.name`, `Commit.oid`) and implement
> `find_previous` as "find that identity in the new vector".

**`scroll_target` / scrolloff 2** (`:260`-`:266`): `moving_down` → `(index + 2).min(len-1)`,
else `index.saturating_sub(2)`. Both clamped, so the margin collapses at the ends rather than
refusing to scroll (test `scrolloff_keeps_context`, `:432`-`:441`).

**`ListView`**

```rust
// crates/fleet-ui-kit/src/components/list_view.rs:288-292
pub fn new(
    id: impl Into<ElementId>,
    item_count: usize,
    render_row: impl Fn(usize, bool, &mut Window, &mut App) -> AnyElement + 'static,
) -> Self

// list_view.rs:307  pub fn cursor(mut self, index: usize) -> Self
// list_view.rs:314  pub fn row_height(mut self, height: Pixels) -> Self
// list_view.rs:320  pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self
// list_view.rs:326  pub fn empty(mut self, empty: impl IntoElement) -> Self
// list_view.rs:335  pub fn loading(mut self, loading: bool) -> Self
// list_view.rs:341  pub fn skeleton_rows(mut self, rows: usize) -> Self
// list_view.rs:352  pub fn reveal(handle: &UniformListScrollHandle, cursor: &ListCursor, moving_down: bool)
```

`render_row` receives `(index, is_cursor, window, cx)` — a view never compares indices itself.
Renders (`:357`-`:391`) three exclusive branches: `loading` → `SkeletonRows`; `item_count == 0`
→ the `empty` element in a box with `min_h(row_height)`; otherwise
`gpui::uniform_list(id, item_count, ...)` `.size_full()` plus `.track_scroll(&handle)`.
`row_height` (default `metrics.row_h` = 30 px) is used **only** for the skeleton and the empty
box — `uniform_list` measures the first rendered row itself (`:312`-`:313`). `ListView` draws no
background, border, padding or selection of its own; all row chrome comes from the `Row` your
closure returns.

**`UniformListScrollHandle` lifecycle** — the handle is **owned by the view entity, never by
the element**:

- created once per list in the view constructor (`crates/fleet-app/src/screens/hub.rs:518`-`:520`,
  fields at `crates/fleet-app/src/screens/hub.rs:507`-`:509`);
- passed by reference each render via `ListView::track_scroll(&handle)` (`list_view.rs:320`),
  which clones it into `uniform_list(...).track_scroll(&handle)` (`list_view.rs:387`);
- used **only from an action handler, never from `render`** (`list_view.rs:346`-`:351`):
  `ListView::reveal` calls
  `handle.scroll_to_item(cursor.scroll_target(moving_down), ScrollStrategy::Nearest)`
  (`list_view.rs:353`). `Nearest` scrolls the minimum amount, so a cursor already inside the
  scrolloff margin does not move the viewport — which is what stops a background refresh from
  stealing the scroll position.

There is no automatic autoscroll in `ListView`. The only scroll-on-render in the whole kit is
`LogView`'s follow mode (`log_view.rs:155`-`:157`).

The canonical long-lived shape is the gallery,
`crates/fleet-ui-kit/examples/gallery_data.rs:88`-`:94`:
```rust
    /// Every list action lands here: move the cursor, then reveal it with the scrolloff.
    /// This is the shape `fleet-app` uses — the element never sees a key.
    fn motion(&mut self, motion: ListMotion, cx: &mut Context<Self>) {
        let moving_down = self.cursor.motion(motion);
        ListView::reveal(&self.scroll, &self.cursor, moving_down);
        cx.notify();
    }
```
(fields at `:67`-`:68`, init at `:77`-`:78`, the six `on_action` listeners at `:941`-`:960`,
`bindings.extend(list_key_bindings(None));` at `:997`.)

App usage — `crates/fleet-app/src/views/repos_rail.rs:273`:
```rust
    let body_rows = rows.clone();
    let list = ListView::new(
        "hub-repos-rail",
        if repo_rows == 0 { 0 } else { rows.len() },
        move |index, is_cursor, _window, _cx| {
            let Some(row) = body_rows.get(index) else {
                return gpui::div().into_any_element();
            };
            rail_row(row, is_cursor, focused, collapsed)
        },
    )
    .cursor(cursor)
    .track_scroll(scroll)
    .empty(empty);
```

#### `Row` / `RowColumn` / `ColumnAlign` — `crates/fleet-ui-kit/src/components/row.rs`

`Row` is `#[derive(IntoElement)]` + `RenderOnce` (`:119`-`:120`, `impl` at `:235`) with
`impl Default` (`:229`). `RowColumn` is a plain builder struct (`:37`). `ColumnAlign` is
`Left` (default) / `Center` / `Right` (`:25`-`:34`).

```rust
// crates/fleet-ui-kit/src/components/row.rs:21
pub const GLYPH_COLUMN_CH: f32 = 2.0;

// crates/fleet-ui-kit/src/components/row.rs:47
pub fn fixed(width: Pixels, element: impl IntoElement) -> Self
// crates/fleet-ui-kit/src/components/row.rs:58
pub fn fixed_ch(width_ch: f32, element: impl IntoElement) -> Self
// crates/fleet-ui-kit/src/components/row.rs:63
pub fn flex(element: impl IntoElement) -> Self
// crates/fleet-ui-kit/src/components/row.rs:74
pub fn auto(element: impl IntoElement) -> Self
// crates/fleet-ui-kit/src/components/row.rs:90
pub fn resolved(column: &ResolvedColumn, element: impl IntoElement) -> Self
// row.rs:101  pub fn align(mut self, align: ColumnAlign) -> Self
// row.rs:107  pub fn min_width(mut self, min_width: Pixels) -> Self
// row.rs:113  pub fn min_width_ch(self, min_width_ch: f32) -> Self

// crates/fleet-ui-kit/src/components/row.rs:135
pub fn new() -> Self
// crates/fleet-ui-kit/src/components/row.rs:151
pub fn with_id(id: impl Into<ElementId>) -> Self
// row.rs:156  pub fn id(mut self, id: impl Into<ElementId>) -> Self
// row.rs:165  pub fn leading(mut self, leading: impl IntoElement) -> Self
// row.rs:171  pub fn column(mut self, column: RowColumn) -> Self
// row.rs:177  pub fn columns(mut self, columns: impl IntoIterator<Item = RowColumn>) -> Self
// row.rs:184  pub fn second_line(mut self, line: impl IntoElement) -> Self
// row.rs:190  pub fn height(mut self, height: Pixels) -> Self
// row.rs:196  pub fn selected(mut self, selected: bool) -> Self
// row.rs:205  pub fn cursor(mut self, cursor: bool) -> Self
// row.rs:211  pub fn dimmed(mut self, dimmed: bool) -> Self
// row.rs:217  pub fn disabled(mut self, disabled: bool) -> Self
// row.rs:223  pub fn hoverable(mut self, hoverable: bool) -> Self
```

Renders (`:235`-`:320`) `div().w_full().h(height)` where height is the override, else
`metrics.job_row_h` (44) when a `second_line` exists, else `metrics.row_h` (30). `selected`
paints `bg(colors.row_selected)`; `dimmed || disabled` applies `opacity(0.40)`; hover
(`bg(colors.row_hover)`) requires `hoverable && !disabled && !selected` **and** an `id`. Its
child is `FocusRing::cursor_row(cursor)`, a permanent 2 px `border_l` painted
`colors.cursor_bar` or transparent (`focus.rs:72`-`:82`). Inside: `flex items_center h_full
w_full gap(space.md) px(space.md)` with an optional leading glyph in a `flex_none w(ch(2.0))`
box, then one box per column (`flex_1()` when flexible, `w(width).flex_none()` when fixed,
`justify_start/center/end` per align).

> **`selected` and `cursor` are separate flags.** This is precisely lazygit's behaviour: a
> side panel keeps its highlighted row when focus moves to the main panel. In `fleet-lazygit`,
> set `.selected(is_selected)` always and `.cursor(is_selected && panel_is_focused)`.

`RowColumn::fixed_ch` is how you build lazygit's fixed-width columns (short sha, ahead/behind
counters, author initials) — character units, so they stay aligned in the mono font.

Usage — `crates/fleet-app/src/views/repos_rail.rs:303`:
```rust
    let mut element = Row::new()
        .leading(glyph)
        .selected(is_cursor)
        .cursor(is_cursor && focused)
        .dimmed(row.kind == RailKind::Deleting)
        .disabled(!row.kind.selectable());
```

#### `SegmentedTabs` / `SegmentedTab` — `crates/fleet-ui-kit/src/components/segmented_tabs.rs`

`SegmentedTabs` is `#[derive(IntoElement)]` + `RenderOnce` (`:63`-`:64`, `impl` at `:111`);
`SegmentedTab` is a plain struct with public fields `label: SharedString`,
`count: Option<usize>`, `loading: bool` (`:16`-`:25`). Aliased `pub type TabBar = SegmentedTabs;`.

```rust
// crates/fleet-ui-kit/src/components/segmented_tabs.rs:29
pub fn new(label: impl Into<SharedString>, count: usize) -> Self   // SegmentedTab
// crates/fleet-ui-kit/src/components/segmented_tabs.rs:38
pub fn bare(label: impl Into<SharedString>) -> Self                // SegmentedTab
// segmented_tabs.rs:47   pub fn loading(mut self, loading: bool) -> Self
// segmented_tabs.rs:53   pub fn count_text(&self) -> Option<SharedString>   // "…" when loading

// crates/fleet-ui-kit/src/components/segmented_tabs.rs:71
pub fn new(tabs: impl IntoIterator<Item = SegmentedTab>) -> Self   // SegmentedTabs
// segmented_tabs.rs:79   pub fn active(mut self, active: usize) -> Self
// segmented_tabs.rs:85   pub fn len(&self) -> usize
// segmented_tabs.rs:90   pub fn is_empty(&self) -> bool
// segmented_tabs.rs:95   pub fn next_index(active: usize, len: usize) -> usize   // wraps
// segmented_tabs.rs:103  pub fn prev_index(active: usize, len: usize) -> usize   // wraps
```

Renders (`:111`-`:167`) a `flex items_stretch flex_none` row at `h(metrics.pane_header_h)`
(30 px) with `gap(space.xl)` (24). Each tab is a column with a label line
(`Tone::Default` + `text.ui_strong.weight` when active, `Tone::Secondary` otherwise) plus the
count, over a 2 px underline slot that **every** tab carries — painted `colors.accent` when
active, `transparent_black()` otherwise, so switching tabs never shifts the row
(`:159`-`:160`).

> **This is exactly lazygit's `]` / `[` tab strip**: Files/Worktrees/Submodules,
> Local/Remotes/Tags, Commits/Reflog. `next_index`/`prev_index` **wrap**, matching lazygit's
> `handleNextTab`. Use it as a `Pane::header` in place of `PaneHeader` for the tabbed panels.

Usage — `crates/fleet-app/src/views/prs_screen.rs:247`:
```rust
    let tabs = SegmentedTabs::new([tab_of("Mine", mine_count), tab_of("Review", review_count)])
        .active(usize::from(tab == PrTab::Review));
```

#### `FuzzyList` / `FuzzyItem` — `crates/fleet-ui-kit/src/components/fuzzy_list.rs`

`FuzzyList` is `#[derive(IntoElement)]` + `RenderOnce` (`:163`-`:164`, `impl` at `:247`);
`FuzzyItem` is a plain builder struct (`:32`).

```rust
// crates/fleet-ui-kit/src/components/fuzzy_list.rs:46
pub fn new(primary: impl Into<SharedString>) -> Self               // FuzzyItem
// fuzzy_list.rs:62   pub fn secondary(mut self, secondary: impl Into<SharedString>) -> Self
// fuzzy_list.rs:73   pub fn detail(mut self, detail: impl Into<SharedString>) -> Self
// fuzzy_list.rs:79   pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self
// fuzzy_list.rs:85   pub fn leading(mut self, leading: impl IntoElement) -> Self
// fuzzy_list.rs:92   pub fn key(mut self, key: impl Into<SharedString>) -> Self
// fuzzy_list.rs:99   pub fn matches(mut self, matches: impl IntoIterator<Item = usize>) -> Self
// fuzzy_list.rs:107  pub fn destructive(mut self, destructive: bool) -> Self
// fuzzy_list.rs:113  pub fn disabled(mut self, disabled: bool) -> Self

// crates/fleet-ui-kit/src/components/fuzzy_list.rs:175
pub fn new(items: impl IntoIterator<Item = FuzzyItem>) -> Self     // FuzzyList
// fuzzy_list.rs:188  pub fn cursor(mut self, cursor: usize) -> Self
// fuzzy_list.rs:194  pub fn cap(mut self, cap: usize) -> Self
// fuzzy_list.rs:200  pub fn under_text_field(mut self, under: bool) -> Self
// fuzzy_list.rs:206  pub fn row_height(mut self, height: Pixels) -> Self
// fuzzy_list.rs:212  pub fn empty(mut self, empty: impl IntoElement) -> Self
// fuzzy_list.rs:218  pub fn binds_jk(&self) -> bool             // == !under_text_field
// fuzzy_list.rs:223  pub fn shown(&self) -> usize               // items.len().min(cap)
// fuzzy_list.rs:231  pub fn next_cursor(cursor: usize, len: usize) -> usize   // wraps
// fuzzy_list.rs:239  pub fn prev_cursor(cursor: usize, len: usize) -> usize   // wraps
```

`matches` are **character** indices into `primary`; out-of-range indices are ignored rather
than panicking. Match highlighting (`:133`-`:160`) is a **font-weight bump only**, never a
second color. Defaults: `cap: 8`, `under_text_field: true`.

> Use this for lazygit's menus (`m`, `?`, `d`, `g`, `u`, `M`, `S`, `D`, `b`, `<ctrl+p>`,
> `<ctrl+s>`) and for branch/commit suggestion prompts. `FuzzyItem::key` fills the right-hand
> key column — that is lazygit's menu shortcut column.

Usage — `crates/fleet-app/src/dialogs/assign_repo.rs:101`:
```rust
    let list = FuzzyList::new(draft.rows.iter().map(|row| {
        let mut item = FuzzyItem::new(row.name.clone());
        if !row.owners.is_empty() {
            item = item.detail(row.owners.clone());
        }
        item
    }))
    .cursor(draft.cursor)
    .under_text_field(false)
    .empty(Text::ui("No contexts yet.").muted());
```

#### `AppFrame` — `crates/fleet-ui-kit/src/components/app_frame.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:37`-`:38`, `impl` at `:109`), `impl Default` (`:103`).

```rust
// crates/fleet-ui-kit/src/components/app_frame.rs:49
pub fn new() -> Self
// app_frame.rs:61   pub fn context_bar(mut self, bar: impl IntoElement) -> Self
// app_frame.rs:71   pub fn banner(mut self, banner: impl IntoElement) -> Self
// app_frame.rs:77   pub fn body(mut self, body: impl IntoElement) -> Self
// app_frame.rs:83   pub fn status_bar(mut self, bar: impl IntoElement) -> Self
// app_frame.rs:90   pub fn overlay(mut self, overlay: impl IntoElement) -> Self       // pushes; window-wide
// app_frame.rs:97   pub fn body_overlay(mut self, overlay: impl IntoElement) -> Self  // pushes; clipped to the body band
```

Renders (`:109`-`:165`) `div().relative().flex().flex_col().size_full().overflow_hidden()` and
is where the app's baseline typography lands: `bg(colors.bg)`, `text_color(colors.text)`,
`font_family(theme.font_ui)`, `text_size(text.ui.size)`, `line_height(text.ui.line_height)`.
Bands in order: context bar (36 px, `flex_none`), banner (28 px, `flex_none` — it **pushes** the
body down, it does not float), body (`flex_1 min_h_0`, wraps `body` then all `body_overlay`s),
status bar (26 px, `flex_none`), then all window-wide `overlay`s as siblings of the column.
`overlay` and `body_overlay` **append**, so call them repeatedly.

> **`fleet-lazygit` should use `AppFrame` as its root**: `status_bar` gets lazygit's bottom
> information/keybinding line, `body` gets the panel splits, `banner` gets the "rebasing in
> progress / conflicts" strip, `overlay` gets menus, confirmations and prompts.

Usage — `crates/fleet-app/src/shell/root.rs:915`:
```rust
        let mut frame = AppFrame::new()
            .context_bar(context_bar)
            .body(body)
            .status_bar(status_bar)
            .body_overlay(toasts);
```

#### `ContextBar` / `ContextTab` — `crates/fleet-ui-kit/src/components/context_bar.rs`

`ContextBar` is `#[derive(IntoElement)]` + `RenderOnce` (`:68`-`:69`, `impl` at `:140`).

```rust
// crates/fleet-ui-kit/src/components/context_bar.rs:36
pub const TRAFFIC_LIGHT_INSET: Pixels = px(84.0);
// crates/fleet-ui-kit/src/components/context_bar.rs:49
pub fn new(label: impl Into<SharedString>, index: usize) -> Self    // ContextTab; 1..=9 gets a digit hint
// crates/fleet-ui-kit/src/components/context_bar.rs:59
pub fn unnumbered(label: impl Into<SharedString>) -> Self           // ContextTab

// crates/fleet-ui-kit/src/components/context_bar.rs:82
pub fn new(tabs: impl IntoIterator<Item = ContextTab>) -> Self      // ContextBar
// context_bar.rs:96   pub fn active(mut self, active: usize) -> Self
// context_bar.rs:102  pub fn overflow(mut self, overflow: usize) -> Self
// context_bar.rs:110  pub fn chip(mut self, chip: impl IntoElement) -> Self    // pushes
// context_bar.rs:116  pub fn daemon(mut self, state: DaemonState) -> Self
// context_bar.rs:122  pub fn daemon_label(mut self, label: impl Into<SharedString>) -> Self
// context_bar.rs:128  pub fn leading_inset(mut self, inset: Pixels) -> Self
// context_bar.rs:134  pub fn empty(mut self, fact: impl Into<SharedString>, action: impl Into<SharedString>) -> Self
```

Renders (`:140`-`:227`) a `flex items_center justify_between size_full` row with
`pl(leading_inset)` — **84 px by default to clear the macOS traffic lights** — `pr(space.md)`,
`bg(colors.bg)`, `border_b(1px)`. Left is the tab strip (digit hints, 2 px accent underline on
the active tab) or an empty message; right is the pushed chips plus a `DaemonDot`.

> `fleet-lazygit` has no daemon, so the natural use is: repo name on the left (as a single
> `ContextTab::unnumbered`), and `.chip(...)` for the current branch and any mode word. If you
> would rather build the bar yourself, **you must still reserve `TRAFFIC_LIGHT_INSET`**
> (`context_bar.rs:36`) on the left.

Usage — `crates/fleet-app/src/shell/chrome.rs:183`:
```rust
    let mut bar = ContextBar::new(tabs)
        .active(active)
        .overflow(overflow)
        .leading_inset(px(LEADING_INSET))
        .daemon(dot_state(&state.daemon));
```

#### `StatusBar` — `crates/fleet-ui-kit/src/components/status_bar.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:29`-`:30`, `impl` at `:103`), `impl Default` (`:97`).

```rust
// crates/fleet-ui-kit/src/components/status_bar.rs:41
pub fn new() -> Self
// status_bar.rs:55   pub fn breadcrumb(mut self, breadcrumb: impl Into<SharedString>) -> Self
// status_bar.rs:61   pub fn breadcrumb_ch(mut self, budget: usize) -> Self
// status_bar.rs:67   pub fn mode(mut self, mode: Mode) -> Self          // wraps in ModeWord::new(mode)
// status_bar.rs:73   pub fn mode_element(mut self, mode: impl IntoElement) -> Self
// status_bar.rs:79   pub fn ticker(mut self, ticker: impl IntoElement) -> Self
// status_bar.rs:85   pub fn error(mut self, error: impl IntoElement) -> Self
// status_bar.rs:91   pub fn trailing(mut self, trailing: impl IntoElement) -> Self
```

Renders (`:103`-`:147`) a `flex items_center size_full` row with `px(space.md)`,
`gap(space.md)`, `bg(colors.bg)`, `border_t(1px)`. Three slots: a `flex_1 min_w_0` left box with
`Text::ui(breadcrumb).muted()` (middle-truncated when `breadcrumb_ch` is set), a non-flex mode
element in the middle, and a `flex_1 min_w_0 justify_end` right box. **The error outranks the
ticker**: `.when(!has_error, |el| el.children(self.ticker))` at `:143` drops the ticker
entirely whenever `error` is set.

> This is lazygit's bottom information line. Put the repo name + branch in `breadcrumb`, the
> mode word (`rebasing`/`merging`/`cherry-picking`) in `mode_element`, the running git command
> in `ticker`, and the last failed command in `error`. Use `mode_element` rather than `mode`
> unless one of the kit's `Mode` variants happens to fit — see §c.3.

Usage — `crates/fleet-app/src/shell/chrome.rs:239`:
```rust
    let mut bar = StatusBar::new()
        .breadcrumb(SharedString::from(breadcrumb_text(state)))
        .mode(state.mode().word());
```

#### `SectionHeader` — `crates/fleet-ui-kit/src/components/section_header.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:8`-`:9`, `impl` at `:30`).

```rust
// crates/fleet-ui-kit/src/components/section_header.rs:16
pub fn new(label: impl Into<SharedString>) -> Self
// crates/fleet-ui-kit/src/components/section_header.rs:24
pub fn trailing(mut self, trailing: impl IntoElement) -> Self
```

Renders (`:30`-`:41`) a bare `flex items_center justify_between w_full` strip fixed at
`h(metrics.section_header_h)` (20 px) with an uppercased `Text::label` on the left and
`trailing` on the right. No padding, border or background — it inherits its parent's horizontal
padding. Use it for group headings inside the Status panel and inside menus.

Usage — `crates/fleet-app/src/views/detail_panel.rs:209`.

#### `Divider` / `DividerAxis` — `crates/fleet-ui-kit/src/components/divider.rs`

```rust
// crates/fleet-ui-kit/src/components/divider.rs:8-15
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DividerAxis { #[default] Horizontal, Vertical }
// crates/fleet-ui-kit/src/components/divider.rs:26
pub fn horizontal() -> Self
// crates/fleet-ui-kit/src/components/divider.rs:33
pub fn vertical() -> Self
// divider.rs:42  pub fn inset(mut self, inset: bool) -> Self
```

Renders (`:48`-`:63`) a single `div().bg(colors.border).flex_none()` — 1 px tall and `w_full`
(horizontal) or 1 px wide and `h_full` (vertical), with `mx/my(space.lg)` when inset. It is the
only separator in the system.

Usage — `crates/fleet-app/src/dialogs/help.rs:411` (`.child(Divider::horizontal())`);
vertical at `crates/fleet-app/src/dialogs/settings.rs:759`.

#### `LogView` / `LogCommand` — `crates/fleet-ui-kit/src/components/log_view.rs`

`LogView` is `#[derive(IntoElement)]` + `RenderOnce` (`:38`-`:39`, `impl` at `:133`).

```rust
// crates/fleet-ui-kit/src/components/log_view.rs:20
pub const LOG_TAIL_LINES: usize = 200;
// crates/fleet-ui-kit/src/components/log_view.rs:27-35
pub enum LogCommand {
    ToggleFollow,      // :30  `f`
    Follow,            // :32  `G`
    ScrollTo(usize),   // :34  `j`/`k`; always ends follow
}

// crates/fleet-ui-kit/src/components/log_view.rs:54
pub fn new(id: impl Into<ElementId>, lines: impl IntoIterator<Item = SharedString>) -> Self
// log_view.rs:69   pub fn following(mut self, following: bool) -> Self
// log_view.rs:78   pub fn top(mut self, top: usize) -> Self
// log_view.rs:84   pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self
// log_view.rs:94   pub fn focus(mut self, focus: &FocusHandle) -> Self
// log_view.rs:100  pub fn on_command(mut self, on_command: impl Fn(LogCommand, &mut Window, &mut App) + 'static) -> Self
// log_view.rs:109  pub fn empty(mut self, empty: impl Into<SharedString>) -> Self
// log_view.rs:115  pub fn show_badge(mut self, show: bool) -> Self
// log_view.rs:121  pub fn is_following(&self) -> bool
// log_view.rs:128  pub fn line_count(&self) -> usize
```

Renders (`:133`-`:207`) a `uniform_list` of `div().px(space.md).child(Text::data_small(line))`
(mono 11.5). When `following` and a handle exists it calls
`handle.scroll_to_item(count - 1, ScrollStrategy::Top)` **during render** (`:155`-`:157`) — the
one deliberate exception to "never scroll from render", so a batched tail reads as a live
console. Optional `following`/`paused` badge bottom-right. Its own `on_key_down` (wired only
when `.focus(..)` is supplied) ignores any chord with control/platform/alt (`:214`-`:245`).

> **This is lazygit's command log panel.** Feed it `CommandRecord::display_argv` joined with
> spaces, plus stderr previews on failure. `LOG_TAIL_LINES = 200` is a sensible ring-buffer
> size for `fleet-lazygit`'s command log.

Usage — `crates/fleet-app/src/screens/jobs.rs:299`:
```rust
        LogView::new("jobs-panel-log", log.iter().cloned())
            .following(following)
            .track_scroll(&self.log_scroll)
            .into_any_element()
```

### c.3 Overlays, input, feedback and atoms

Every component in this group is `#[derive(IntoElement)]` + `RenderOnce` **except** `TextInput`
(a stateful `Render` view) and the plain data structs `PaletteRow`, `PaletteSection`, `Toast`
and `TextFieldState`. **No component owns keys or timers** — dwell times, the 400 ms prefix
delay and every cursor live in the caller's state.

#### `OverlayLayer` — the single source of paint order

```rust
// crates/fleet-ui-kit/src/components/overlay.rs:27-50
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OverlayLayer {
    /// The right-docked panel.
    Sheet,
    /// A top-anchored card: the command palette.
    #[default]
    Anchored,
    /// A centered modal with a scrim.
    Dialog,
    /// The bottom-right toast stack.
    Toast,
}

    /// The `deferred` priority this layer paints at.
    pub const fn priority(self) -> usize {          // overlay.rs:42
        match self {
            OverlayLayer::Sheet => 100,
            OverlayLayer::Anchored => 200,
            OverlayLayer::Dialog => 300,
            OverlayLayer::Toast => 400,
        }
    }
```

**Never invent a `deferred` priority.** Sheet(100) → palette/anchored(200) → Dialog(300) →
Toast(400) is the whole ordering, and every floating component reads it.

#### `Overlay` — `crates/fleet-ui-kit/src/components/overlay.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:53`-`:54`, `impl` at `:114`).

```rust
// crates/fleet-ui-kit/src/components/overlay.rs:64
pub fn new() -> Self
// overlay.rs:75   pub fn top(mut self, top: Pixels) -> Self
// overlay.rs:81   pub fn width(mut self, width: Pixels) -> Self
// overlay.rs:90   pub fn scrim(mut self, scrim: bool) -> Self
// overlay.rs:96   pub fn layer(mut self, layer: OverlayLayer) -> Self
// overlay.rs:102  pub fn child(mut self, child: impl IntoElement) -> Self
```

Renders (`:114`-`:147`) `deferred(...).with_priority(self.layer.priority())` wrapping an
`absolute inset_0 flex flex_col items_center` root (plus `bg(colors.overlay)` + `.occlude()`
when `scrim`), holding a card at `mt(top).mb(top)` — default `metrics.palette_top` = 120 px,
the "thinking position", **not** screen center — `w(width)` (default `metrics.palette_w` = 640),
`rounded(radii.lg)`, `bg(colors.elevated)`, `border_1` in `colors.border_strong`,
`shadow(theme.dialog_shadow())`, `overflow_hidden().occlude()`.

Usage — `crates/fleet-app/src/dialogs/palette.rs:626`:
```rust
            fleet_ui_kit::Overlay::new()
                .top(top)
                .width(width)
                .scrim(true)
                .child(card),
```

> Use `Overlay` for lazygit's option menus (`m`, `d`, `g`, `u`, `M`, `S`, `D`, `b`,
> `<ctrl+p>`, `<ctrl+s>`) — top-anchored, scrim on.

#### `Dialog` (alias `Modal`) — `crates/fleet-ui-kit/src/components/dialog.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:29`-`:30`, `impl` at `:137`).

```rust
// crates/fleet-ui-kit/src/components/dialog.rs:45
pub const DEFAULT_WIDTH: Pixels = px(560.0);
// crates/fleet-ui-kit/src/components/dialog.rs:48
pub fn new(title: impl Into<SharedString>) -> Self
// dialog.rs:64   pub fn icon(mut self, icon: Icon) -> Self
// dialog.rs:70   pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self
// dialog.rs:76   pub fn width(mut self, width: Pixels) -> Self
// dialog.rs:83   pub fn height(mut self, height: Pixels) -> Self
// dialog.rs:89   pub fn tone(mut self, tone: Tone) -> Self
// dialog.rs:95   pub fn body(mut self, body: impl IntoElement) -> Self
// dialog.rs:101  pub fn hints(mut self, hints: impl IntoElement) -> Self
// dialog.rs:107  pub fn hint_row(self, hints: KeyHintRow) -> Self
// dialog.rs:112  pub fn primary(mut self, primary: impl Into<SharedString>) -> Self
// dialog.rs:120  pub fn error(mut self, error: impl Into<SharedString>) -> Self
// dialog.rs:131  pub fn warning(mut self, warning: impl Into<SharedString>) -> Self
```

`error` and `warning` share one `footer_note` slot — **the last one wins**.

Renders (`:137`-`:238`) at `OverlayLayer::Dialog` priority (300): an `absolute inset_0`
`items_center justify_center` scrim (`bg(colors.overlay)`, `.occlude()` so clicks never reach
the frozen screen) around a card with `w(self.width)`, `max_h_full().min_h_0()`,
`rounded(radii.lg)`, `bg(colors.elevated)`, `border_1(colors.border_strong)`,
`shadow(theme.dialog_shadow())`. A fixed 44 px header (`metrics.dialog_header_h`) with a
tone-tinted `IconSize::Large` glyph + `Text::title` + muted subtitle; a
`flex_1 min_h_0 p(space.lg) gap(space.md)` body; and a footer whose optional error/warning line
is its own 22 px (`metrics.strip_h`) strip **above** the hint row, so the hints never shift
(`:177`-`:195`), then a 44 px `justify_between` row with hints left and
`Text::ui_strong(primary)` right. **There are no buttons anywhere** — the primary label is text
naming the key.

Usage — `crates/fleet-app/src/dialogs/create_worktree.rs:414`:
```rust
    let mut card = Dialog::new("New worktree")
        .icon(Icon::GitBranchPlus)
        .width(super::Dialogs::CreateWorktree.width())
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key("\u{21e5}", "field")
                .key("\u{2303}n/\u{2303}p", "base")
                .key("esc", "cancel"),
        )
        .primary("\u{23ce} Create");
```

> This is lazygit's commit-message panel, its rename/tag/branch-name prompts, and its
> "confirmation panel". `primary` carries the `⏎` glyph, matching lazygit's `<enter> Confirm`.

#### `ConfirmDialog` — `crates/fleet-ui-kit/src/components/confirm_dialog.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:36`-`:37`, `impl` at `:181`); its `render` **returns a
`Dialog`** (`:232`-`:246`).

```rust
// crates/fleet-ui-kit/src/components/confirm_dialog.rs:54
pub fn new(title: impl Into<SharedString>, facts: FactList) -> Self
// confirm_dialog.rs:72   pub fn target(mut self, target: impl Into<SharedString>) -> Self
// confirm_dialog.rs:78   pub fn consequence(mut self, consequence: impl Into<SharedString>) -> Self
// confirm_dialog.rs:84   pub fn stamp(mut self, stamp: FreshnessStamp) -> Self
// confirm_dialog.rs:90   pub fn icon(mut self, icon: Icon) -> Self
// confirm_dialog.rs:97   pub fn hints(mut self, hints: KeyHintRow) -> Self
// confirm_dialog.rs:103  pub fn force_confirm_key(mut self, key: ConfirmKey) -> Self
// confirm_dialog.rs:109  pub fn action_label(mut self, label: impl Into<SharedString>) -> Self
// confirm_dialog.rs:115  pub fn width(mut self, width: Pixels) -> Self
// confirm_dialog.rs:122  pub fn body(mut self, body: impl IntoElement) -> Self
// confirm_dialog.rs:128  pub fn error(mut self, error: impl Into<SharedString>) -> Self
// confirm_dialog.rs:134  pub fn is_compact(&self) -> bool
// confirm_dialog.rs:139  pub fn confirm_key(&self) -> ConfirmKey
// confirm_dialog.rs:145  pub fn resolved_width(&self) -> Pixels
```

The escalation logic is the point: compact → 480 px (`COMPACT_W`, `:30`), `Tone::Default`,
default `Icon::Trash`, facts on one wrapped line; expanded → 560 px (`EXPANDED_W`, `:33`),
`Tone::Warning`, default `Icon::TriangleAlert`, the full target on its own line as
`Text::data(target).ellipsize()`, facts one per line. A multi-target body is **never** compact.
Footer hints emit `⏎ confirm` only when `key.accepts_enter()` (`:196`-`:199`), always followed
by `KeyHint::labeled("n / esc", "cancel")`; caller hints are **prepended**. Default
`action_label` is `"Delete"` (`:64`).

Usage — `crates/fleet-app/src/dialogs/confirm.rs:607`:
```rust
    let mut card = ConfirmDialog::new(title.clone(), facts.list.clone())
        .consequence(request.consequence(&facts))
        .icon(request.icon(compact))
        .hints(hints)
        .action_label(request.action_label(0));
```

> Use it for lazygit's destructive confirmations: `d` discard, `d` drop commit, `D` reset,
> `d` delete branch, force-push. The `force_confirm_key(ConfirmKey::Upper)` escalation maps
> onto lazygit's lowercase/uppercase safe/strong convention.

#### `Sheet` — `crates/fleet-ui-kit/src/components/sheet.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:26`-`:27`, `impl` at `:96`).

```rust
// crates/fleet-ui-kit/src/components/sheet.rs:38
pub fn new(open: bool) -> Self
// sheet.rs:50   pub fn expanded(mut self, expanded: bool) -> Self
// sheet.rs:57   pub fn width(mut self, width: Pixels) -> Self
// sheet.rs:63   pub fn header(mut self, header: impl IntoElement) -> Self
// sheet.rs:69   pub fn body(mut self, body: impl IntoElement) -> Self
// sheet.rs:75   pub fn footer(mut self, footer: impl IntoElement) -> Self
// sheet.rs:81   pub fn is_open(&self) -> bool
// sheet.rs:87   pub fn resolved_width(&self, theme: &crate::theme::Theme) -> Pixels
```

Renders a bare `div()` when closed; otherwise `deferred` at `OverlayLayer::Sheet` (100) — the
**lowest** floating layer — as a right-docked `absolute inset_0 justify_end` panel with **no
scrim**, so the list behind stays readable. `border_l(1px)` in `colors.border_strong`,
`shadow(theme.sheet_shadow())`. Width defaults to `metrics.sheet_w` (440) or
`metrics.sheet_expanded_w` (640) when expanded. **No slide animation** — the caller animates the
width it passes over `theme.motion.sheet` (160 ms). Its module doc (`:17`-`:19`) says to place
it in `AppFrame::body_overlay`, not `overlay`, so both bars stay reachable.

Usage — `crates/fleet-app/src/views/detail_panel.rs:72`:
```rust
        Sheet::new(true)
            .width(px(OVERLAY_WIDTH))
            .body(body)
            .into_any_element()
```

> A natural fit for lazygit's command-log panel if you prefer it docked rather than as a
> bottom band.

#### `Palette` — `crates/fleet-ui-kit/src/components/palette.rs`

`Palette` is `#[derive(IntoElement)]` + `RenderOnce` (`:168`-`:169`, `impl` at `:241`);
`PaletteRow` (`:52`) and `PaletteSection` (`:142`) are plain structs.

```rust
// crates/fleet-ui-kit/src/components/palette.rs:29-49
/// The three sections, in their fixed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteSectionKind {
    /// Objects: worktrees, PRs, repos. Ranked above commands.
    Go,
    /// Valid commands only, each with its bound key.
    Do,
    /// Context switches, with their digit.
    Context,
}
// palette.rs:42   pub fn title(self) -> &'static str      // "go" / "do" / "context"

// PaletteRow — palette.rs:64
pub fn new(label: impl Into<SharedString>) -> Self
// palette.rs:77   pub fn icon(mut self, icon: Icon) -> Self
// palette.rs:83   pub fn leading(mut self, leading: impl IntoElement) -> Self
// palette.rs:89   pub fn detail(mut self, detail: impl Into<SharedString>) -> Self
// palette.rs:95   pub fn key(mut self, key: impl Into<SharedString>) -> Self
// palette.rs:102  pub fn matches(mut self, matches: impl IntoIterator<Item = usize>) -> Self
// palette.rs:108  pub fn destructive(mut self, destructive: bool) -> Self

// PaletteSection — palette.rs:149
pub fn new(kind: PaletteSectionKind, rows: impl IntoIterator<Item = PaletteRow>) -> Self
// palette.rs:157  pub fn len(&self) -> usize
// palette.rs:162  pub fn is_empty(&self) -> bool

// Palette — palette.rs:181
pub fn new(query: impl Into<SharedString>) -> Self
// palette.rs:194  pub fn section(mut self, section: PaletteSection) -> Self
// palette.rs:200  pub fn caret(mut self, caret: usize) -> Self
// palette.rs:206  pub fn cursor(mut self, cursor: usize) -> Self
// palette.rs:212  pub fn cap(mut self, cap: usize) -> Self       // default 10 (palette.rs:188)
// palette.rs:218  pub fn total(mut self, total: usize) -> Self
// palette.rs:224  pub fn empty(mut self, empty: impl Into<SharedString>) -> Self
// palette.rs:230  pub fn flat_len(&self) -> usize
// palette.rs:236  pub fn shown(&self) -> usize                   // flat_len().min(cap)
```

`Ord` on `PaletteSectionKind` is load-bearing: `sections.sort_by_key(|section| section.kind)`
(`palette.rs:251`) means **Go always renders first regardless of push order**. The 10-row cap is
consumed **across** sections (`palette.rs:257`-`:292`), so a flat cursor is offset per section.

Renders (`:241`-`:362`) a `flex_col` card: a `TextField` query line at
`metrics.palette_input_h` (44 px) with `.prefix(":")` and `.hide_status_line(true)`
(`:331`-`:337`); per section a 20 px header plus a `FuzzyList` at `metrics.palette_row_h`
(34 px) with `.under_text_field(true)`; and a 20 px footer with `{shown} of {total}` plus a
hard-coded `KeyHintRow` `⏎ run · ⌃n/⌃p move · esc cancel` (`:356`-`:359`). It paints **no scrim
and no shadow** — wrap it in an `Overlay`.

Usage — `crates/fleet-app/src/dialogs/palette.rs:529`:
```rust
    let mut card = fleet_ui_kit::Palette::new(query.value().to_owned())
        .cursor(cursor)
        .cap(ROW_CAP)
        .total(total)
        .empty(format!("Nothing matches \"{}\".", query.value()));
```

> lazygit has no command palette, but this is the obvious home for a "go to branch / commit /
> file" jump and for `<ctrl+r>` recent-repo switching.

#### `TextField` / `TextFieldState` / `TextInput` — `crates/fleet-ui-kit/src/components/text_field.rs`

**Three layers; pick exactly one** (module doc `:1`-`:18`).

**(1) `TextField` — presentational `RenderOnce`** (`:163`-`:164`, `impl` at `:302`). The caller
owns the string **and** the caret and passes both every frame.

```rust
// crates/fleet-ui-kit/src/components/text_field.rs:181
pub fn new(value: impl Into<SharedString>) -> Self
// text_field.rs:199  pub fn label(mut self, label: impl Into<SharedString>) -> Self
// text_field.rs:205  pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self
// text_field.rs:211  pub fn caret(mut self, caret: usize) -> Self       // CHARACTER index
// text_field.rs:217  pub fn focused(mut self, focused: bool) -> Self
// text_field.rs:223  pub fn icon(mut self, icon: Icon) -> Self
// text_field.rs:233  pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self
// text_field.rs:239  pub fn preview(mut self, preview: impl Into<SharedString>) -> Self
// text_field.rs:245  pub fn invalid(mut self, message: impl Into<SharedString>) -> Self
// text_field.rs:251  pub fn mono(mut self, mono: bool) -> Self
// text_field.rs:257  pub fn height(mut self, height: Pixels) -> Self
// text_field.rs:267  pub fn hide_status_line(mut self, hide: bool) -> Self
// text_field.rs:273  pub fn is_invalid(&self) -> bool
```

Chrome comes from the private `FieldChrome::wrap` (`:77`-`:146`), shared with `TextInput`:
`h(metrics.text_field_h)` (36 px), `px(space.md)`, `rounded(radii.sm)`, `bg(colors.bg)`,
`border_1`. **Border precedence** (`:66`-`:74`): `invalid` → `colors.danger`; else `focused` →
`colors.focus_ring`; else `colors.border`. A fixed `metrics.field_status_h` (18 px) status slot
holds the preview, **replaced in place** by the validation message so there is zero layout
shift (`:129`-`:145`). The caret is a **fake 2 px bar** (`:294`-`:300`) inserted between head
and tail via `split_at_char` (`:150`-`:156`). **No selection, no IME, no
cursor-follows-scroll.**

**(2) `TextFieldState` — the pure editing model** (plain struct, `:358`-`:363`): owned `String`
+ **byte** cursor + optional IME marked range; no gpui, no theme, unit-testable.

```rust
// text_field.rs:367  pub fn new() -> Self
// text_field.rs:372  pub fn from_text(text: impl Into<String>) -> Self     // caret at end
// text_field.rs:383  pub fn text(&self) -> &str
// text_field.rs:388  pub fn shared_text(&self) -> SharedString
// text_field.rs:393  pub fn is_empty(&self) -> bool
// text_field.rs:398  pub fn cursor(&self) -> usize                          // BYTE offset
// text_field.rs:403  pub fn caret_chars(&self) -> usize                     // CHAR index — what TextField::caret wants
// text_field.rs:408  pub fn marked_range(&self) -> Option<Range<usize>>
// text_field.rs:413  pub fn set_text(&mut self, text: impl Into<String>)
// text_field.rs:420  pub fn clear(&mut self)
// text_field.rs:427  pub fn set_cursor(&mut self, offset: usize)            // snapped to a char boundary
// text_field.rs:432  pub fn insert(&mut self, text: &str)
// text_field.rs:443  pub fn backspace(&mut self) -> bool
// text_field.rs:455  pub fn delete_forward(&mut self) -> bool
// text_field.rs:466  pub fn delete_word_before(&mut self) -> bool           // ctrl-w
// text_field.rs:494  pub fn delete_to_start(&mut self) -> bool              // ctrl-u
// text_field.rs:505  pub fn delete_to_end(&mut self) -> bool                // ctrl-k
// text_field.rs:515  pub fn move_left(&mut self) -> bool
// text_field.rs:523  pub fn move_right(&mut self) -> bool
// text_field.rs:531  pub fn move_to_start(&mut self) -> bool                // ctrl-a / Home
// text_field.rs:538  pub fn move_to_end(&mut self) -> bool                  // ctrl-e / End
// text_field.rs:548  pub fn replace_range(&mut self, range: Range<usize>, text: &str)
// text_field.rs:558  pub fn replace_and_mark(&mut self, range: Range<usize>, text: &str)
// text_field.rs:566  pub fn unmark(&mut self)
// text_field.rs:575  pub fn handle_edit_keystroke(&mut self, keystroke: &Keystroke) -> bool
// text_field.rs:635  pub fn handle_keystroke(&mut self, keystroke: &Keystroke) -> bool
// text_field.rs:653  pub fn offset_to_utf16(&self, offset: usize) -> usize
// text_field.rs:659  pub fn offset_from_utf16(&self, offset: usize) -> usize
// text_field.rs:671  pub fn len_utf16(&self) -> usize
```

**The key-handling split is the one trap.** `handle_edit_keystroke` (`:575`-`:627`) consumes
**only control keys**; `handle_keystroke` (`:635`-`:650`) is that **plus** printable insertion
from `keystroke.key_char`. Use `handle_keystroke` when you render the presentational
`TextField` and receive raw key events; use `handle_edit_keystroke` when the platform input
handler is installed, **or every character is inserted twice** (`:629`-`:634`).

`sanitize` (`:714`-`:718`) strips `\n`, `\r`, `\t` on every insert — this is a single-line
field. Every cursor mutation goes through `floor_boundary`/`previous_boundary`/`next_boundary`
(`:686`-`:710`), so it can never index-panic. **There is no selection at all**, deliberately
(`:355`-`:357`).

**(3) `TextInput` — the stateful `Render` view** (`:747`, `Render` at `:926`). **This is the one
you use for real editing.** It owns its own `FocusHandle` (`:748`), implements `Focusable`
(`:920`), `EventEmitter<TextInputEvent>` (`:918`), `EntityInputHandler` (`:949`-`:1066`), and
paints through a custom `Element` (`TextInputElement`, `:1083`, `impl Element` at `:1102`) that
shapes a real line, slides it to keep the caret in view, and installs `ElementInputHandler` so
**dead keys and IME composition work** (marked text is underlined). Construct with
`cx.new(|cx| TextInput::new(cx))`.

```rust
// text_field.rs:765  pub fn new(cx: &mut Context<Self>) -> Self
// text_field.rs:784  pub fn with_text(mut self, text: impl Into<String>) -> Self
// text_field.rs:790  pub fn with_label(mut self, label: impl Into<SharedString>) -> Self
// text_field.rs:796  pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self
// text_field.rs:802  pub fn with_icon(mut self, icon: Icon) -> Self
// text_field.rs:808  pub fn with_mono(mut self, mono: bool) -> Self
// text_field.rs:814  pub fn with_height(mut self, height: Pixels) -> Self
// text_field.rs:820  pub fn with_hidden_status_line(mut self, hide: bool) -> Self
// text_field.rs:826  pub fn text(&self) -> &str
// text_field.rs:831  pub fn state(&self) -> &TextFieldState
// text_field.rs:836  pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>)
// text_field.rs:842  pub fn clear(&mut self, cx: &mut Context<Self>)
// text_field.rs:848  pub fn set_preview(&mut self, preview: Option<SharedString>, cx: &mut Context<Self>)
// text_field.rs:854  pub fn set_invalid(&mut self, message: Option<SharedString>, cx: &mut Context<Self>)
// text_field.rs:860  pub fn is_invalid(&self) -> bool

// text_field.rs:725-731
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The value changed. Re-derive the preview, the validation and any filtered list.
    Changed,
    /// The caret moved but the value did not.
    CaretMoved,
}

// text_field.rs:947
pub const TEXT_FIELD_KEY_CONTEXT: &str = "FleetTextField";
```

Note the API-shape difference: `TextField` uses `.label()`/`.placeholder()`, `TextInput` uses
`.with_label()`/`.with_placeholder()`, and `TextInput` has **no `prefix`** (`chrome()`
hard-codes `prefix: None`, `:907`). Its `Render` (`:926`-`:940`) wires
`.key_context(TEXT_FIELD_KEY_CONTEXT).track_focus(&self.focus_handle).cursor(CursorStyle::IBeam)`
plus `on_key_down` and `on_mouse_down`; the `on_key_down` (`:872`-`:884`) calls
**`handle_edit_keystroke` only**, then emits `CaretMoved` or `Changed`.

**How `fleet-app` drives it: it doesn't.** `fleet-app` has its own buffer type
(`crates/fleet-app/src/dialogs/mod.rs:321`-`:433`) fed by `typed_char`/`type_into`, and renders
the presentational `TextField` with `.caret(...)` every frame
(`crates/fleet-app/src/dialogs/create_worktree.rs:302`-`:315`). Kit `TextInput` is exercised
only by the gallery (`crates/fleet-ui-kit/examples/gallery_input.rs:175`-`:188`).

> **Recommendation for `fleet-lazygit`: use kit `TextInput`.** The commit-message panel needs
> IME, dead keys and mouse caret placement; the hand-rolled buffer has none of that. And see
> §b.6: you **must** shadow every bare-letter binding with `gpui::NoAction` inside
> `FleetTextField`.

#### `KeyHint` / `KeyHintRow` — `crates/fleet-ui-kit/src/components/key_hint.rs`

Both `#[derive(IntoElement)]` + `RenderOnce` (`:12`-`:13`/`:55`, `:69`-`:70`/`:104`).

```rust
// key_hint.rs:22   pub fn new(keys: impl Into<SharedString>) -> Self
// key_hint.rs:32   pub fn labeled(keys: impl Into<SharedString>, label: impl Into<SharedString>) -> Self
// key_hint.rs:37   pub fn label(mut self, label: impl Into<SharedString>) -> Self
// key_hint.rs:43   pub fn tone(mut self, tone: Tone) -> Self          // label tone
// key_hint.rs:49   pub fn key_tone(mut self, tone: Tone) -> Self      // key tone

// key_hint.rs:77   pub fn new() -> Self                               // KeyHintRow
// key_hint.rs:81   pub fn hint(mut self, hint: KeyHint) -> Self
// key_hint.rs:87   pub fn key(self, keys: impl Into<SharedString>, label: impl Into<SharedString>) -> Self
// key_hint.rs:92   pub fn merge(mut self, other: KeyHintRow) -> Self
```

`KeyHint` renders `flex flex_none items_center gap(space.xs)` with `Text::hint(keys)` then the
optional label — mono 11/14 in the lowest contrast the theme has. `KeyHintRow` (`:104`-`:122`)
is `flex items_center gap(space.sm)` with a faint `·` between hints but **not after the last**
(`:118`).

Usage — `crates/fleet-app/src/shell/daemon.rs:126`:
```rust
        let mut hints = KeyHintRow::new();
        for (key, label) in spec.hints {
            hints = hints.key(*key, *label);
        }
```

> **This is lazygit's bottom keybinding bar.** Generate it from `keymap::table()` filtered on
> the current context chain (§b.7) rather than hand-writing strings.

#### `PrefixHint` — `crates/fleet-ui-kit/src/components/prefix_hint.rs`

`#[derive(IntoElement)]` + `RenderOnce` (`:36`-`:37`, `impl` at `:72`).

```rust
// prefix_hint.rs:45   pub fn new(visible: bool) -> Self
// prefix_hint.rs:54   pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self   // default "^S"
// prefix_hint.rs:61   pub fn hints(mut self, hints: KeyHintRow) -> Self
// prefix_hint.rs:67   pub fn is_visible(&self) -> bool
```

Returns a bare `div()` when not visible — **zero cost until the caller's timer fires**;
otherwise an `absolute` pill at `left(space.md).bottom(space.md)`, `h(metrics.chip_h)` (22 px),
`bg(colors.elevated)` + `shadow(theme.sheet_shadow())` + `border_1(colors.border_strong)`, with
an amber sub-pill holding `Icon::Command` + the prefix, followed by the hint row. **The 400 ms
delay is the caller's timer** (`theme.motion.prefix_hint_delay`, module doc `:7`-`:9`).

Usage — `crates/fleet-app/src/screens/workspace.rs:533`:
```rust
                PrefixHint::new(model.mode == TerminalMode::Prefix && hint_visible)
                    .hints(prefix_hints()),
```

> Exactly right for lazygit's pending-chord feedback: after `g` in a `g g` sequence, or after
> `<ctrl+p>`.

#### `Toast` / `ToastStack` / `ToastDuration` — `crates/fleet-ui-kit/src/components/toast_stack.rs`

`ToastStack` is `#[derive(IntoElement)]` + `RenderOnce` (`:126`-`:127`, `impl` at `:204`);
`Toast` is a plain data struct with public fields (`:56`-`:73`).

```rust
// toast_stack.rs:33
pub const COALESCE_WINDOW_MS: u64 = 1_000;

// toast_stack.rs:36-53
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastDuration {
    /// 1.6 s: instant-action acknowledgements (clipboard, "already running", mode no-ops).
    Short,
    /// 3.2 s: everything else the law allows.
    #[default]
    Normal,
}
// toast_stack.rs:47   pub fn millis(self, theme: &crate::theme::Theme) -> u64

// Toast — public fields at toast_stack.rs:57-73
// toast_stack.rs:77   pub fn new(text: impl Into<SharedString>) -> Self
// toast_stack.rs:89   pub fn icon(mut self, icon: Icon) -> Self
// toast_stack.rs:99   pub fn tone(mut self, tone: Tone) -> Self     // Danger -> coerced to Warning + debug_assert!
// toast_stack.rs:113  pub fn short(mut self) -> Self
// toast_stack.rs:119  pub fn raised_at(mut self, millis: u64) -> Self

// ToastStack
// toast_stack.rs:135  pub const MAX: usize = 3;
// toast_stack.rs:138  pub fn new(toasts: impl IntoIterator<Item = Toast>) -> Self
// toast_stack.rs:147  pub fn max(mut self, max: usize) -> Self
// toast_stack.rs:153  pub fn bottom_inset(mut self, inset: gpui::Pixels) -> Self   // default px(12.0)
// toast_stack.rs:159  pub fn is_visible(&self) -> bool
// toast_stack.rs:169  pub fn push(toasts: &mut Vec<Toast>, toast: Toast, max: usize)
// toast_stack.rs:179  pub fn push_at(toasts: &mut Vec<Toast>, toast: Toast, max: usize, now_ms: u64)
// toast_stack.rs:195  pub fn resolved_text(toast: &Toast) -> SharedString           // appends " ×n" when count > 1
```

**Errors are never toasts**: `Tone::Danger` is `debug_assert!`-ed and silently downgraded to
`Warning` (`:99`-`:110`, repeated defensively in `render` at `:232`-`:236`). `push_at`
coalesces an identical text raised within `COALESCE_WINDOW_MS`, bumping both `count` and
`raised_at_ms` so dwell restarts. Renders at `OverlayLayer::Toast` (400) — the highest layer,
readable over an open dialog — bottom-right, `w(metrics.toast_w)` (320 px). **No title, no
close button**; dwell is the caller's timer.

Usage — `crates/fleet-app/src/state.rs:1266`:
```rust
            self.toast(
                Toast::new(text).icon(Icon::CircleCheck),
                now,
                dwell_for(ToastDuration::Normal),
            );
```

> Use toasts for `MutationResult::warning` and for "copied to clipboard". Put a **failed git
> command** in a sticky slot in the status bar (`StatusBar::error`), not a toast.

#### `EmptyState` — `crates/fleet-ui-kit/src/components/empty_state.rs`

```rust
// empty_state.rs:19   pub fn new(fact: impl Into<SharedString>) -> Self
// empty_state.rs:27   pub fn action(mut self, action: impl Into<SharedString>) -> Self
```

Renders (`:33`-`:45`) exactly two centered lines — `Text::ui(fact).muted()` then
`Text::hint(action).faint()` — filling its parent (`size_full`), so it goes **inside the
affected pane only, never full-screen**.

Usage — `crates/fleet-app/src/views/jobs_panel.rs:461`:
```rust
        Some(label) => EmptyState::new(format!("No {label} jobs."))
            .action("f  show every job")
            .into_any_element(),
```

> One per panel: "No changed files.", "No stashes.", "No tags." — with the action line naming
> the key that would create one.

#### `Spinner` / `SpinnerWithLabel` — `crates/fleet-ui-kit/src/components/spinner.rs`

```rust
// spinner.rs:22   pub fn new(id: impl Into<ElementId>) -> Self       // id MUST be stable across frames
// spinner.rs:32   pub fn size(mut self, size: IconSize) -> Self      // default IconSize::Large
// spinner.rs:38   pub fn tone(mut self, tone: Tone) -> Self          // default Tone::Warning
// spinner.rs:44   pub fn color(mut self, color: Hsla) -> Self
// spinner.rs:71   pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self
```

`Icon::LoaderCircle` spinning one turn per `theme.motion.spinner` (1000 ms) — **the only
looping animation in the system**, amber because "in flight" is amber everywhere. **If the
`ElementId` changes between frames the animation restarts every frame.**

Usage — `crates/fleet-app/src/dialogs/create_worktree.rs:323`:
```rust
            draft
                .fetching
                .then(|| SpinnerWithLabel::new("create-base-fetch", "fetching")),
```

> Use it for a running network operation (`f`, `p`, `P`) in the status bar — with a **constant**
> id.

#### `Banner` — `crates/fleet-ui-kit/src/components/banner.rs`

```rust
// banner.rs:55   pub fn warning(text: impl Into<SharedString>) -> Self    // amber, Icon::TriangleAlert
// banner.rs:66   pub fn danger(text: impl Into<SharedString>) -> Self     // red, Icon::Unplug
// banner.rs:77   pub fn icon(mut self, icon: Icon) -> Self
// banner.rs:84   pub fn countdown(mut self, countdown: impl Into<SharedString>) -> Self
// banner.rs:90   pub fn hints(mut self, hints: KeyHintRow) -> Self
// banner.rs:96   pub fn resolved_tone(&self) -> Tone
```

**There is no `new`** — start from `warning` or `danger`. Renders (`:101`-`:138`) a full-width
strip, `bg(self.tone.fill(theme))` (a 14 % tint) with `border_b` at 35 % opacity, so it reads as
chrome rather than content. It has **no intrinsic height** — `AppFrame` gives it 28 px. The
`countdown` sits in its **own `flex_none` slot** so the sentence never reflows while the number
changes.

Usage — `crates/fleet-app/src/shell/daemon.rs:122`:
```rust
    let mut banner = Banner::warning(spec.text).icon(Icon::Unplug);
```

> **This is lazygit's "rebasing / merging / conflicts" strip.** Feed it from
> `OperationState`, with `Rebasing { done, total }` as the countdown ("3/7") and a hint row of
> `m continue · esc abort`.

#### `ModeWord` / `Mode` — `crates/fleet-ui-kit/src/components/mode_word.rs`

```rust
// crates/fleet-ui-kit/src/components/mode_word.rs:22-41
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Lists. `NORMAL`.
    #[default]
    Normal,
    /// Keys go to the PTY. `TERMINAL`.
    Terminal,
    /// One-shot after `ctrl-s`. `^S`, amber.
    Prefix,
    /// Scrollback / copy mode. `SCROLL`.
    Scroll,
    /// Filter input. `FILTER`.
    Filter,
    /// Command palette. `PALETTE`.
    Palette,
    /// Any dialog. `DIALOG`.
    Dialog,
    /// Jobs panel. `JOBS`.
    Jobs,
}
// mode_word.rs:45   pub const ALL: &'static [Mode]
// mode_word.rs:57   pub const fn word(self) -> &'static str   // NORMAL/TERMINAL/^S/SCROLL/FILTER/PALETTE/DIALOG/JOBS
// mode_word.rs:71   pub const fn tone(self) -> Tone           // Prefix => Warning, else Secondary
// mode_word.rs:80   pub const fn keys_reach_pty(self) -> bool

// mode_word.rs:94   pub fn new(mode: Mode) -> Self
// mode_word.rs:102  pub fn word(word: impl Into<SharedString>) -> Self   // for a mode the kit doesn't know
// mode_word.rs:110  pub fn tone(mut self, tone: Tone) -> Self
```

Renders (`:116`-`:127`) a single `Text::label` centered in a **fixed
`metrics.mode_word_w` = 84 px** slot — fixed on purpose, so the word changing never shoves the
rest of the status bar sideways.

> **The kit's eight `Mode` variants do not include `REBASING`, `MERGING` or `CHERRY-PICKING`.**
> Use `ModeWord::word("REBASING").tone(Tone::Warning)` (`mode_word.rs:102`) with
> `StatusBar::mode_element` (`status_bar.rs:73`) rather than `StatusBar::mode`. That keeps the
> 84 px slot and the label typography while letting you name lazygit's modes — but note
> `CHERRY-PICKING` is 14 characters and will be clipped by `overflow_hidden` at 84 px, so
> shorten it (lazygit itself shows a short word here; see the reference §c).

#### `Badge` / `BadgeStyle` — `crates/fleet-ui-kit/src/components/badge.rs`

```rust
// crates/fleet-ui-kit/src/components/badge.rs:11-21
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BadgeStyle {
    /// Tinted text only. The default: zero chrome for a word that is already short.
    #[default]
    Bare,
    /// Tinted text on a low-alpha fill of the same hue.
    Filled,
    /// Tinted text inside a 1 px hairline.
    Outlined,
}
// badge.rs:34   pub fn new(text: impl Into<SharedString>) -> Self    // tone defaults to Tone::Secondary
// badge.rs:44   pub fn tone(mut self, tone: Tone) -> Self
// badge.rs:50   pub fn style(mut self, style: BadgeStyle) -> Self
// badge.rs:56   pub fn color(mut self, color: Hsla) -> Self
```

**`Bare` gets no box at all** — `h(metrics.chip_h).px(space.xs)` is applied only when the style
is not `Bare` (`:74`-`:76`), because a box costs 22 px of row height. Square corners, no icon,
no count.

No direct app usage; gallery at `crates/fleet-ui-kit/examples/gallery_data.rs:355`-`:367`.

> **This is lazygit's tag and branch decoration on a commit row.** `Bare` keeps a 30 px row at
> 30 px — use it, not `Filled`.

#### `Chip` — `crates/fleet-ui-kit/src/components/chip.rs`

```rust
// chip.rs:31   pub fn new() -> Self                                  // tone Secondary, zero_suppress true
// chip.rs:46   pub fn counter(icon: Icon, count: usize) -> Self
// chip.rs:51   pub fn labeled(icon: Icon, text: impl Into<SharedString>) -> Self
// chip.rs:56   pub fn icon(mut self, icon: Icon) -> Self
// chip.rs:62   pub fn text(mut self, text: impl Into<SharedString>) -> Self
// chip.rs:68   pub fn count(mut self, count: usize) -> Self
// chip.rs:74   pub fn tone(mut self, tone: Tone) -> Self
// chip.rs:80   pub fn color(mut self, color: Hsla) -> Self
// chip.rs:86   pub fn filled(mut self, filled: bool) -> Self          // OFF by default
// chip.rs:92   pub fn spinning(mut self, spinning: bool) -> Self      // requires .id()
// chip.rs:98   pub fn id(mut self, id: impl Into<SharedString>) -> Self
// chip.rs:104  pub fn zero_suppress(mut self, suppress: bool) -> Self
// chip.rs:114  pub fn is_visible(&self) -> bool
```

**Zero-suppression is built in** (`:114`-`:119`): a chip with a suppressed `0` count and no word
renders nothing. The intended pattern is that a view passes **every** chip, including
zero-valued ones, and the chip decides. Flat by default; the pill appears only with
`.filled(true)`. The count uses the **`label` role (11 px)**, matching every other number in
the chrome. Pass a real `id` or all spinning chips share one animation.

Usage — `crates/fleet-app/src/shell/chrome.rs:196`:
```rust
    let jobs_chip = if counts.failed > 0 {
        Chip::counter(Icon::TriangleAlert, counts.failed).tone(Tone::Danger)
    } else {
        Chip::counter(Icon::LoaderCircle, counts.running)
            .tone(Tone::Warning)
            .spinning(true)
            .id("context-bar-jobs")
    };
```

> **Perfect for lazygit's ahead/behind counters** in the context bar:
> `Chip::counter(Icon::CircleArrowUp, ahead)` and
> `Chip::counter(Icon::CircleArrowDown, behind)`, both zero-suppressed — plus a
> `Chip::counter(Icon::FileDiff, files.len())` and
> `Chip::counter(Icon::Boxes, stashes.len())`.

### c.4 The theme token API

#### Reading the theme

Covered in §a.6: `cx.theme()` from the `ActiveTheme` trait
(`crates/fleet-ui-kit/src/theme/theme.rs:247`-`:259`), a gpui `Global`
(`crates/fleet-ui-kit/src/theme/theme.rs:80`), installed by `Theme::init`
(`crates/fleet-ui-kit/src/theme/theme.rs:170`). The `Theme` struct itself is
`crates/fleet-ui-kit/src/theme/theme.rs:54`-`:78`, with sub-structs `colors: ColorTokens`,
`terminal: TerminalPalette`, `text: TypeScale`, `space: Spacing`, `radii: Radii`,
`elevation: Elevation`, `motion: Motion`, `metrics: Metrics`, `font_ui`, `font_mono`.

Helpers: `shadow(&self, token: ShadowToken) -> Vec<BoxShadow>`
(`crates/fleet-ui-kit/src/theme/theme.rs:214`), `dialog_shadow()` (`:225`), `sheet_shadow()`
(`:230`), `faded(&self, color: Hsla, factor: f32) -> Hsla` (`:236`), `is_dark()` (`:209`).

#### The complete color token list

**These 22 field names are the entire palette; nothing else exists**
(`crates/fleet-ui-kit/src/theme/tokens.rs:32`-`:79`):

```rust
/// Semantic color roles. Exactly one instance per theme mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTokens {
    /// App ground. The largest area on screen.
    pub bg: Hsla,
    /// Rails, panes, lists — one step above the ground.
    pub surface: Hsla,
    /// Dialogs, sheets, toasts, palette — the floating layer.
    pub elevated: Hsla,
    /// Scrim painted over the base screen behind a dialog.
    pub overlay: Hsla,
    /// Cursor row background.
    pub row_selected: Hsla,
    /// Pointer hover on a row. Never used to express state.
    pub row_hover: Hsla,
    /// Primary text: branch, title, value.
    pub text: Hsla,
    /// Secondary text: repo, host, age, count, section label.
    pub text_secondary: Hsla,
    /// Muted text: draft, disabled, key hint, the null dash.
    pub text_muted: Hsla,
    /// Text drawn on top of an accent or semantic fill.
    pub text_inverse: Hsla,
    /// Cursor and focus. The only decorative-looking color, and it is never decorative.
    pub accent: Hsla,
    /// Healthy / done / approved.
    pub success: Hsla,
    /// Needs attention / in flight / unknown / degraded.
    pub warning: Hsla,
    /// Broken / destructive.
    pub danger: Hsla,
    /// Neutral information. Shares the accent hue; never used for state.
    pub info: Hsla,
    /// 1 px hairlines.
    pub border: Hsla,
    /// Hairline that has to survive on top of `elevated`.
    pub border_strong: Hsla,
    /// 2 px inset ring on the focused pane.
    pub focus_ring: Hsla,
    /// 2 px left bar on the cursor row.
    pub cursor_bar: Hsla,
    /// Text selection fill (inputs and terminal).
    pub selection: Hsla,
    /// Scroll thumb on a pane edge.
    pub scroll_thumb: Hsla,
    /// Cold-load placeholder rows.
    pub skeleton: Hsla,
}
```

Resolved values — `ColorTokens::dark()` at `crates/fleet-ui-kit/src/theme/tokens.rs:83`-`:108`,
`::light()` at `:111`-`:136`:

| Group | Field | Dark | Light |
|---|---|---|---|
| Surfaces | `bg` (`:85`/`:113`) | `#0E1013` | `#FBFBFC` |
| | `surface` (`:86`/`:114`) | `#16181D` | `#FFFFFF` |
| | `elevated` (`:87`/`:115`) | `#1B1E24` | `#FFFFFF` |
| | `overlay` (`:88`/`:116`) | `#00000073` (45 %) | `#00000040` (25 %) |
| Rows | `row_selected` (`:89`/`:117`) | `#1E2430` | `#EDF2FB` |
| | `row_hover` (`:90`/`:118`) | `#191C22` | `#F3F4F6` |
| Text | `text` (`:91`/`:119`) | `#E6E8EB` | `#16181D` |
| | `text_secondary` (`:92`/`:120`) | `#8A9099` | `#6B7280` |
| | `text_muted` (`:93`/`:121`) | `#5A6069` | `#9CA3AF` |
| | `text_inverse` (`:94`/`:122`) | `#0E1013` | `#FBFBFC` |
| Semantic | `accent` (`:95`/`:123`) | `#58A6FF` | `#0969DA` |
| | `success` (`:96`/`:124`) | `#3FB950` | `#1A7F37` |
| | `warning` (`:97`/`:125`) | `#D29922` | `#9A6700` |
| | `danger` (`:98`/`:126`) | `#F85149` | `#CF222E` |
| | `info` (`:99`/`:127`) | `#58A6FF` | `#0969DA` |
| Lines | `border` (`:100`/`:128`) | `#22262E` | `#E3E5E9` |
| | `border_strong` (`:101`/`:129`) | `#2C313A` | `#D3D6DC` |
| | `focus_ring` (`:102`/`:130`) | `#58A6FF` | `#0969DA` |
| | `cursor_bar` (`:103`/`:131`) | `#58A6FF` | `#0969DA` |
| | `selection` (`:104`/`:132`) | `#58A6FF47` | `#0969DA33` |
| | `scroll_thumb` (`:105`/`:133`) | `#2C313A` | `#D3D6DC` |
| | `skeleton` (`:106`/`:134`) | `#1E222A` | `#EEF0F3` |

Hex helpers: `pub fn c(hex: u32) -> Hsla` (`crates/fleet-ui-kit/src/theme/tokens.rs:10`,
`0xRRGGBB`) and `pub fn ca(hex: u32) -> Hsla` (`:16`, `0xRRGGBBAA`).

**Terminal colors are separate** — `TerminalPalette`
(`crates/fleet-ui-kit/src/theme/palette.rs:12`-`:24`): `pub ansi: [Hsla; 16]`, `foreground`,
`background`, `cursor`, `selection`, plus `pub fn color(&self, index: u8) -> Hsla`
(`crates/fleet-ui-kit/src/theme/palette.rs:85`) resolving the whole xterm-256 space.

> **Use `terminal.ansi` to map lazygit's colors.** lazygit's presentation code names ANSI
> colors (`style.FgGreen`, `style.FgRed`, `style.FgCyan`, …; see the reference §c). The faithful
> mapping is `theme.terminal.ansi[n]`, **not** the semantic tokens — `ansi[2]` for green,
> `ansi[1]` for red, `ansi[6]` for cyan, `ansi[3]` for yellow, `ansi[5]` for magenta, and the
> bright variants at `ansi[8..16]`. Reserve `colors.success`/`danger`/`warning` for genuine
> Fleet-level state (a failed command, an in-flight fetch) and `colors.accent` for focus only.

#### Spacing, radii and the `ch` unit

```rust
// crates/fleet-ui-kit/src/theme/tokens.rs:20-30
/// The base unit of the whole system. Every spacing and size token is a multiple of it.
pub const BASE_UNIT: f32 = 4.0;

/// Width of one monospace cell at the data type size, in logical pixels (`1 ch`).
pub const CH: f32 = 7.5;

/// Convert a `ch` budget (the unit the UX spec measures column ladders in) to pixels.
#[inline]
pub fn ch(n: f32) -> Pixels {
    px(n * CH)
}
```

`BASE_UNIT` is 4.0 **logical pixels**; `CH` is 7.5 **logical pixels per monospace cell** at the
12.5 px data size. `ch(n)` converts a character budget into pixels — exported from
`crates/fleet-ui-kit/src/theme/mod.rs:10`-`:13`.

> **`ch()` is the function you will use most in `fleet-lazygit`.** Every lazygit column is
> measured in characters: a 7-char short sha is `ch(8.0)`, a 4-char ahead/behind pair is
> `ch(6.0)`, the graph column is `ch(graph_width as f32)`. Never compute a pixel width from an
> assumed advance.

`Spacing` (`crates/fleet-ui-kit/src/theme/tokens.rs:248`-`:279`, values `:267`-`:279`):
`xxs: px(2.0)`, `xs: px(4.0)`, `sm: px(8.0)`, `md: px(12.0)` ("the gap between list columns and
the row padding"), `lg: px(16.0)` ("pane padding"), `xl: px(24.0)`, `xxl: px(32.0)`.

`Radii` (`crates/fleet-ui-kit/src/theme/tokens.rs:281`-`:309`, values `:298`-`:309`):
`none: px(0.0)` ("rows, panes, the terminal grid"), `xs: px(3.0)`, `sm: px(4.0)`,
`md: px(6.0)`, `lg: px(12.0)`, `full: px(9999.0)`.

#### `Metrics` — the fixed geometry

`crates/fleet-ui-kit/src/theme/tokens.rs:404`-`:487` (declarations), `:489`-`:533` (values).
The doc rule at `:404` is explicit: **"Components must not hard-code these."** The ones you
will use:

| Field | Lines (decl/value) | Value |
|---|---|---|
| `hairline` | `:409`/`:492` | `px(1.0)` |
| `context_bar_h` | `:415`/`:495` | `px(36.0)` |
| `status_bar_h` | `:417`/`:496` | `px(26.0)` |
| `pane_header_h` | `:419`/`:497` | `px(30.0)` |
| `row_h` | `:421`/`:498` | `px(30.0)` |
| `palette_row_h` | `:423`/`:499` | `px(34.0)` |
| `job_row_h` | `:425`/`:500` | `px(44.0)` |
| `section_header_h` | `:427`/`:501` | `px(20.0)` |
| `dialog_header_h` / `dialog_footer_h` | `:429`,`:431`/`:502`,`:503` | `px(44.0)` |
| `banner_h` | `:433`/`:504` | `px(28.0)` |
| `strip_h` | `:435`/`:505` | `px(22.0)` |
| `chip_h` | `:437`/`:506` | `px(22.0)` |
| `toast_w` | `:447`/`:511` | `px(320.0)` |
| `palette_w` / `palette_top` | `:449`,`:451`/`:512`,`:513` | `px(640.0)` / `px(120.0)` |
| `mode_word_w` | `:453`/`:514` | `px(84.0)` |
| `scroll_thumb_w` | `:455`/`:515` | `px(3.0)` |
| `focus_ring_w` | `:457`/`:516` | `px(2.0)` |
| `cell_w` / `cell_h` | `:459`,`:461`/`:517`,`:518` | `px(CH)` = 7.5 / `px(18.0)` |
| `text_field_h` | `:469`/`:522` | `px(36.0)` |
| `field_status_h` | `:471`/`:523` | `px(18.0)` |
| `palette_input_h` | `:473`/`:524` | `px(44.0)` |

Plus six bare `f32` opacities (not `Pixels`): `veil_opacity` 0.55 (`:477`/`:526`),
`dimmed_opacity` 0.40 (`:479`/`:527`), `refreshing_opacity` 0.60 (`:481`/`:528`),
`stale_opacity` 0.55 (`:483`/`:529`), `skeleton_opacity` 0.30 (`:485`/`:530`),
`no_session_opacity` 0.30 (`:487`/`:531`).

> **`refreshing_opacity` = 0.60 is the token for "this diff is being reloaded"** — dim the
> cached main panel rather than blanking it (§f.4 rule 7).

`Elevation` / `ShadowToken` (`crates/fleet-ui-kit/src/theme/tokens.rs:311`-`:369`): consume via
`theme.sheet_shadow()` / `theme.dialog_shadow()`, never by building a `BoxShadow`.

`Motion` (`crates/fleet-ui-kit/src/theme/tokens.rs:371`-`:402`, values `:390`-`:401`), all
`u64` **milliseconds**: `toast: 140`, `sheet: 160`, `highlight: 120`,
`prefix_hint_delay: 400`, `spinner: 1000`, `toast_short: 1600`, `toast_normal: 3200`.

#### Fonts and the type scale

`theme.font_ui` (`crates/fleet-ui-kit/src/theme/theme.rs:75`) is `".SystemUIFont"`;
`theme.font_mono` (`:77`) is metric-probed at startup (§a.3).

```rust
// crates/fleet-ui-kit/src/theme/tokens.rs:139-146
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    /// The system UI face.
    Ui,
    /// The monospace face used for data: branches, paths, shas, logs, terminal.
    Mono,
}

// crates/fleet-ui-kit/src/theme/tokens.rs:148-164
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyle {
    /// Font size in logical pixels.
    pub size: Pixels,
    /// Fixed line height in logical pixels. Never relative.
    pub line_height: Pixels,
    /// Weight.
    pub weight: FontWeight,
    /// Which family this role resolves to.
    pub font: FontRole,
    /// Whether callers must uppercase the string before rendering.
    pub uppercase: bool,
    /// Letter spacing in `em`. Recorded for the spec; gpui 1.18.1 exposes no tracking setter,
    /// so `Text` cannot apply it yet.
    pub tracking: f32,
}
```

**`tracking` is inert in gpui 1.18.1.** `uppercase` is applied by `Text::render`
(`crates/fleet-ui-kit/src/text.rs:222`-`:224`), not by the caller.

`TypeScale` (`crates/fleet-ui-kit/src/theme/tokens.rs:166`-`:183`, values `:185`-`:246`):

| Role | Decl | Values | size/line-height | weight | family | uppercase |
|---|---|---|---|---|---|---|
| `ui` | `:169` | `:188`-`:195` | 13 / 18 | `NORMAL` | Ui | no |
| `ui_strong` | `:171` | `:196`-`:203` | 13 / 18 | `MEDIUM` | Ui | no |
| `title` | `:173` | `:204`-`:211` | 15 / 20 | `MEDIUM` | Ui | no |
| `data` | `:175` | `:212`-`:219` | **12.5** / 18 | `NORMAL` | **Mono** | no |
| `data_small` | `:177` | `:220`-`:227` | **11.5** / 16 | `NORMAL` | **Mono** | no |
| `label` | `:179` | `:228`-`:235` | 11 / 14 | `MEDIUM` | Ui | **yes** |
| `hint` | `:181` | `:236`-`:243` | 11 / 14 | `NORMAL` | **Mono** | no |

Applying a role to a container rather than wrapping every child
(`crates/fleet-ui-kit/src/text.rs:202`-`:214`):

```rust
/// Apply a [`TypeStyle`] to any styled element. Exposed so composite components can style a
/// container once instead of wrapping every child in a [`Text`].
pub fn styled_with<E: Styled>(element: E, style: TypeStyle, theme: &Theme) -> E {
    let family = match style.font {
        FontRole::Ui => theme.font_ui.clone(),
        FontRole::Mono => theme.font_mono.clone(),
    };
    element
        .font_family(family)
        .text_size(style.size)
        .line_height(style.line_height)
        .font_weight(style.weight)
}
```

Get the `TypeStyle` from a role with `TextRole::style(self, theme: &Theme) -> TypeStyle`
(`crates/fleet-ui-kit/src/text.rs:37`). Real usage —
`crates/fleet-app/src/dialogs/help.rs:310`:

```rust
    styled_with(div(), TextRole::Ui.style(theme), theme)
        .text_color(theme.colors.text_secondary)
        .child(
            StyledText::new(text)
                .with_highlights(highlights)
                .with_font_family_overrides(families),
        )
        .into_any_element()
```

> **`styled_with(div(), TextRole::Data.style(theme), theme)` on a diff-line container is the
> right move**: set the mono face once on the row, then emit per-color sibling `Text` runs
> inside it. That is how you get around "a `Text` shapes as a single run" (§f.5 gotcha 5)
> without paying for a font lookup per fragment.

#### `Tone`

```rust
// crates/fleet-ui-kit/src/tone.rs:10-32
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    /// Primary text contrast. The value the eye must land on.
    #[default]
    Default,
    /// Secondary contrast: repo, host, age, counts, labels.
    Secondary,
    /// Lowest contrast: draft, disabled, key hints, the null dash.
    Muted,
    /// Cursor and focus. Never "how is it going".
    Accent,
    /// Healthy / done / approved.
    Success,
    /// Needs attention / in flight / unknown / degraded.
    Warning,
    /// Broken / destructive.
    Danger,
    /// Neutral information.
    Info,
    /// Text on top of an accent or semantic fill.
    Inverse,
}
```

Nine variants. Two mapping functions:

```rust
// crates/fleet-ui-kit/src/tone.rs:35-49
    /// The foreground color for this tone.
    pub fn color(self, theme: &Theme) -> Hsla {
        let c = &theme.colors;
        match self {
            Tone::Default => c.text,
            Tone::Secondary => c.text_secondary,
            Tone::Muted => c.text_muted,
            Tone::Accent => c.accent,
            Tone::Success => c.success,
            Tone::Warning => c.warning,
            Tone::Danger => c.danger,
            Tone::Info => c.info,
            Tone::Inverse => c.text_inverse,
        }
    }

// crates/fleet-ui-kit/src/tone.rs:51-59
    /// A low-alpha fill of the same hue, for chip and badge backgrounds.
    pub fn fill(self, theme: &Theme) -> Hsla {
        match self {
            Tone::Default | Tone::Secondary | Tone::Muted | Tone::Inverse => {
                theme.colors.text.opacity(0.08)
            }
            other => other.color(theme).opacity(0.14),
        }
    }
```

The module doc (`crates/fleet-ui-kit/src/tone.rs:1`-`:4`) states the law: `Tone` is the **only**
way a component picks a color; draft/disabled/N-A are expressed by *lowering contrast*
(`Secondary`/`Muted`), never by a hue. For a faithful lazygit palette you will need
`Text::color(theme.terminal.ansi[n])` as the documented escape hatch — see
`crates/fleet-ui-kit/src/text.rs:146`, whose doc says `color` is for "already-theme-resolved
terminal/palette colors", which is exactly this case.

#### `Text` helpers — `crates/fleet-ui-kit/src/text.rs`

```rust
// crates/fleet-ui-kit/src/text.rs:15-33
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextRole { #[default] Ui, UiStrong, Title, Data, DataSmall, Label, Hint }
// text.rs:37   pub fn style(self, theme: &Theme) -> TypeStyle
// text.rs:50   pub fn default_tone(self) -> Tone
```

`default_tone` (`:50`-`:56`): `Ui | UiStrong | Title | Data => Tone::Default`;
`DataSmall | Label => Tone::Secondary`; `Hint => Tone::Muted`. **So `Text::hint(..)` is already
muted and `Text::label(..)` already secondary — you rarely need `.muted()` on them.**

Constructors:

| Fn | Line | Role | Purpose |
|---|---|---|---|
| `Text::new(role, text)` | `:77` | explicit | escape hatch for a dynamic role |
| `Text::ui(text)` | `:94` | `Ui` | 13/18 regular — prose and values |
| `Text::ui_strong(text)` | `:99` | `UiStrong` | 13/18 medium — titles in rows, active tabs |
| `Text::title(text)` | `:104` | `Title` | 15/20 medium — dialog titles |
| `Text::data(text)` | `:109` | `Data` | **mono 12.5/18 — branch, path, sha** |
| `Text::data_small(text)` | `:114` | `DataSmall` | mono 11.5/16 — log tails |
| `Text::label(text)` | `:119` | `Label` | 11/14 **auto-uppercased** |
| `Text::hint(text)` | `:124` | `Hint` | mono 11/14 muted — key hints |

Builders:

| Fn | Line | Purpose |
|---|---|---|
| `tone(mut self, tone: Tone) -> Self` | `:129` | pick a semantic tone |
| `muted(self) -> Self` | `:135` | **shorthand for `Tone::Secondary`** (not `Muted`) |
| `faint(self) -> Self` | `:140` | shorthand for `Tone::Muted` |
| `color(mut self, color: Hsla) -> Self` | `:146` | explicit color; for already-resolved terminal/palette colors |
| `opacity(mut self, opacity: f32) -> Self` | `:152` | whole-run opacity |
| `weight(mut self, weight: FontWeight) -> Self` | `:158` | override the role weight |
| `truncate_at(mut self, budget: usize, mode: Truncate) -> Self` | `:164` | **`ch`-budget truncation with explicit ellipsis position** |
| `ellipsize(mut self) -> Self` | `:170` | layout-driven tail ellipsis |
| `w(mut self, width: Pixels) -> Self` | `:176` | fix width in pixels |
| `w_ch(mut self, width: f32) -> Self` | `:182` | **fix width in `ch` — the column-ladder unit** |
| `flex_none(mut self) -> Self` | `:188` | keep the run from shrinking in a flex row |
| `resolved_text(&self) -> SharedString` | `:194` | the string after the `ch` budget (testable) |

> `Text::muted()` maps to `Tone::Secondary` and `Text::faint()` to `Tone::Muted` — **the naming
> is off by one from the tone names.** `resolved_text()` is what lets you unit-test a
> truncation decision without a window.

#### `icons.rs`

`Icon` is generated by the `lucide_icons!` macro
(`crates/fleet-ui-kit/src/icons.rs:19`-`:63`, invoked at `:65`-`:134`) — **68 variants**, each
an embedded Lucide SVG with `stroke-width` rewritten to 1.5. **The set is closed on purpose**
(`crates/fleet-ui-kit/src/icons.rs:21`-`:25`): adding one means downloading the SVG into
`crates/fleet-ui-kit/assets/icons/` and documenting it.

The git-relevant variants already present: `GitBranch` (`:100`), `GitBranchPlus` (`:99`),
`GitCommitHorizontal` (`:101`), `GitFork` (`:102`), `GitMerge` (`:103`),
`GitPullRequest` (`:105`), `GitPullRequestDraft` (`:104`), `FileDiff` (`:95`),
`FilePen` (`:96`), `FolderGit2` (`:98`), `CloudUpload` (`:88`), `CloudDownload` (`:86`),
`CircleArrowUp` (`:75`), `CircleArrowDown` (`:74`), `Scissors` (`:120`), `Trash` (`:128`),
`Trash2` (`:129`), `TriangleAlert` (`:130`), `Check` (`:70`), `Plus` (`:116`),
`Minus` (`:114`), `Boxes` (`:69`), `Clock` (`:85`), `Search` (`:121`), `Tag`-less but
`Flag` (`:97`), `LoaderCircle` (`:110`), `CircleDot` (`:77`), `CircleCheck` (`:76`),
`CircleX` (`:81`), `Ellipsis` (`:93`), `RefreshCw` (`:118`).

```rust
// icons.rs:34   pub const ALL: &'static [Icon]
// icons.rs:37   pub const fn name(self) -> &'static str        // "git-branch"
// icons.rs:42   pub const fn path(self) -> SharedString        // "icons/git-branch.svg"
// icons.rs:49   pub const fn bytes(self) -> &'static [u8]
// icons.rs:56   pub fn from_path(path: &str) -> Option<Icon>
// icons.rs:161  pub fn el(self) -> IconElement
// icons.rs:166  pub fn size(self, size: IconSize) -> IconElement
// icons.rs:171  pub fn color(self, color: Hsla) -> IconElement

// crates/fleet-ui-kit/src/icons.rs:136-157
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IconSize {
    /// 12 px: status bar, inline marks inside a row, exited-tab cross.
    Small,
    /// 14 px: inside chips, pane headers, filter bar.
    Medium,
    /// 16 px: the default glyph column of every list, dialog headers.
    #[default]
    Large,
}
// icons.rs:150  pub fn px(self) -> Pixels

// IconElement — icons.rs:192
pub fn new(icon: Icon) -> Self
// icons.rs:204  pub fn size(mut self, size: IconSize) -> Self
// icons.rs:210  pub fn color(mut self, color: Hsla) -> Self
// icons.rs:216  pub fn opacity(mut self, opacity: f32) -> Self
// icons.rs:222  pub fn spinning(mut self, spinning: bool) -> Self    // REQUIRES .id()
// icons.rs:228  pub fn id(mut self, id: impl Into<ElementId>) -> Self
```

`render` (`:234`-`:259`) builds `svg().flex_none().size(..).path(..).text_color(color)`, the
color **defaulting to `theme.colors.text`, never gpui's black** — because
`docs/research/gpui.md:2067`-`:2075` records that **`svg()` renders nothing without a text
color**, silently.

> **lazygit's files panel uses letters (`M`, `A`, `??`), not icons.** Render those as
> `Text::data` in a fixed `ch(2.0)` column (`GLYPH_COLUMN_CH`,
> `crates/fleet-ui-kit/src/components/row.rs:21`), not as `Icon`s — that is the faithful
> reproduction and it avoids the icon set entirely for the most common row type.

#### `truncate.rs` and `focus.rs`

```rust
// crates/fleet-ui-kit/src/truncate.rs:10-23
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Truncate {
    /// `…/payroll` — keep the end. Used for `owner/name` columns.
    Head,
    /// `feat/pay…-fix` — keep both ends. Used for branches and paths.
    Middle,
    /// `Fix RUT valid…` — keep the start. Used for PR titles.
    #[default]
    Tail,
}

/// The single-character ellipsis Fleet uses everywhere.
pub const ELLIPSIS: char = '\u{2026}';

// crates/fleet-ui-kit/src/truncate.rs:29
pub fn truncate(text: &str, budget: usize, mode: Truncate) -> SharedString
```

Counts **`char`s**, not bytes and not grapheme clusters (`:29`-`:55`): `len <= budget` returns
the string unchanged, `budget == 0` returns `""`, `budget == 1` returns just the ellipsis, and
`Middle` splits `front = keep.div_ceil(2)`. Tested invariant (`:83`-`:90`): output is always
`<= budget.max(1)` chars.

> **`Truncate::Middle` is right for a branch name** (`feat/pay…-fix`) and
> **`Truncate::Head` for a file path** (`…/src/main.rs`) — matching lazygit, which keeps the
> filename visible. Commit subjects take `Truncate::Tail`.

```rust
// crates/fleet-ui-kit/src/focus.rs:11-18
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusRingKind {
    /// 2 px inset ring around a pane.
    Pane,
    /// 2 px bar on the leading edge of a row.
    CursorRow,
}
// focus.rs:30   pub fn pane(focused: bool) -> Self
// focus.rs:39   pub fn cursor_row(active: bool) -> Self
// focus.rs:48   pub fn child(mut self, child: impl IntoElement) -> Self
```

`render` (`:55`-`:84`) uses `theme.metrics.focus_ring_w` (2 px). **The element is always present
with a transparent border when unfocused**, so focusing costs zero layout shift. You will not
call these directly — `Pane` and `Row` wrap themselves.

### c.5 Cross-cutting rules for the whole kit

- **Nothing in the structural set is a stateful `Render` view.** Every component above is
  `RenderOnce`: it owns no state. Cursor indices, scroll handles, follow flags, filter queries
  and active tabs all live in *your* entity and are passed down on every render. The single
  "brain" type is `ListCursor`, a `Copy` struct with no gpui dependency (so it is directly unit
  testable — do that).
- **Accent blue appears in exactly two places**, both via `crates/fleet-ui-kit/src/focus.rs`:
  `FocusRing::pane` (2 px inset border, applied by `Pane` itself at `pane.rs:187`) and
  `FocusRing::cursor_row` (2 px left bar, applied by `Row` itself at `row.rs:310`), plus the
  accent underline on active `SegmentedTabs`/`ContextBar` tabs. No other component may reach
  for `colors.accent`. This is how you render lazygit's "focused panel has a colored frame"
  without inventing a new visual.
- **Fixed heights are pinned by the container, not the component.** `AppFrame` pins 36/28/26;
  `Pane` pins 30 for its header. `PaneHeader`, `ContextBar` and `StatusBar` all render
  `size_full` and would collapse outside their band.
- **Two cursor idioms**: pane lists *clamp* (`ListCursor`), fuzzy/tab surfaces *wrap*
  (`FuzzyList::next_cursor`/`prev_cursor`, `SegmentedTabs::next_index`/`prev_index`). This
  matches lazygit exactly — its list panels clamp, its tabs wrap.
- **`j`/`k` ownership**: bind `list_key_bindings(Some("<KeyContext>"))` for pane lists; a list
  under a text field must use `ctrl-n`/`ctrl-p` and keep `.under_text_field(true)` (the
  default). `FuzzyList::binds_jk()` is the predicate to branch on.

## d. Background work: the bridge pattern and the central state entity

### d.1 The bridge: one thread, one Tokio runtime, two `async_channel` streams

`crates/fleet-app/src/bridge.rs:1`-`:24` states the whole contract:

```rust
//! The bridge between gpui's foreground executor and `fleet-client`'s tokio runtime.
//!
//! gpui owns the main thread and is not a tokio runtime; `fleet-client` is tokio through and
//! through. [`Bridge::start`] therefore parks a multi-threaded tokio runtime on **one**
//! background thread, runs `ensure_daemon` and the connection actor there, and exchanges two
//! `async-channel` streams with the UI:
//!
//! ```text
//!   gpui foreground                    background thread (tokio)
//!   ──────────────                     ─────────────────────────
//!   Bridge::request(body) ──command──▶ Client::request(body) ─socket─▶ fleetd
//!   AppState  ◀──BridgeEvent────────── daemon events, health pings
//! ```
//!
//! `async-channel` is the only channel type both sides can await, so neither thread blocks the
//! other. The shell drains [`Bridge::events`] in one `cx.spawn` loop and applies each event to
//! the [`crate::state::AppState`] entity; nothing else in the app talks to the daemon.
```

**`fleet-lazygit` needs exactly this shape**, with `Repository` in place of `Client`. gpui's
async runtime is *not* Tokio (`docs/research/gpui.md:863`), and every `fleet-git` method is
`async` on Tokio, so a dedicated Tokio thread is not optional.

**Thread spawn** (`crates/fleet-app/src/bridge.rs:136`-`:157`):

```rust
    pub fn start(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let (commands, command_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::unbounded();
        let thread_home = home.clone();
        let thread_events = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-daemon-bridge".to_owned())
            .spawn(move || run_thread(thread_home, command_rx, thread_events))
        {
            let _ignored = event_tx.try_send(BridgeEvent::ConnectFailed {
                message: format!("could not start the daemon bridge thread: {error}"),
                log_tail: Vec::new(),
                stale_socket: false,
            });
        }
        Self { commands, events, home }
    }
```

**Runtime construction** (`crates/fleet-app/src/bridge.rs:225`-`:241`):

```rust
fn run_thread(home: PathBuf, commands: Receiver<Command>, events: Sender<BridgeEvent>) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ignored = events.try_send(BridgeEvent::ConnectFailed {
                message: format!("could not start the client runtime: {error}"),
                log_tail: Vec::new(),
                stale_socket: false,
            });
            return;
        }
    };
    runtime.block_on(run(&home, &commands, &events));
}
```

Note the failure style: a thread or runtime that cannot start reports itself as a **domain
event**, never a panic and never a silent hang on a splash screen.

**Channel inventory** — all `async_channel` (`crates/fleet-app/src/bridge.rs:33`:
`use async_channel::{Receiver, Sender};`):

| Direction | Type | Construction | file:line |
|---|---|---|---|
| UI → worker (commands) | `Sender<Command>` / `Receiver<Command>` | `async_channel::unbounded()` | `bridge.rs:138` |
| worker → UI (events) | `Sender<BridgeEvent>` / `Receiver<BridgeEvent>` | `async_channel::unbounded()` | `bridge.rs:139` |
| worker → one caller (single reply) | `Sender<Result<ResponseBody, ProtoError>>` | `async_channel::bounded(1)` | `bridge.rs:193` |
| daemon events into the worker | `tokio::sync::broadcast::Receiver<Event>` | `client.events()` | `bridge.rs:43`, `:458` |

```rust
// crates/fleet-app/src/bridge.rs:121-127
/// The handle the UI keeps. Cloning it is cheap and safe from any thread.
#[derive(Clone, Debug)]
pub struct Bridge {
    commands: Sender<Command>,
    events: Receiver<BridgeEvent>,
    home: PathBuf,
}
```

```rust
// crates/fleet-app/src/bridge.rs:95-106
/// A command sent to the background thread.
enum Command {
    /// Send a request; the answer is forwarded when a channel was supplied.
    Request {
        body: Box<RequestBody>,
        reply: Option<Sender<Result<ResponseBody, ProtoError>>>,
    },
    /// Retry `ensure_daemon` now (`r` on either daemon surface).
    Reconnect,
    /// Stop the runtime; the app is quitting.
    Shutdown,
}
```

Three UI-side APIs (`crates/fleet-app/src/bridge.rs:165`-`:215`): `events()` returns the
receiver; `send(body)` is fire-and-forget ("the right call for anything whose outcome arrives
as an event anyway"); `request(body)` returns a `bounded(1)` receiver for the single answer;
plus `reconnect()` and `shutdown()`.

**Every UI-side send is `try_send` on an unbounded channel with the error deliberately
discarded** (`let _ignored = …`). The UI thread never awaits and never blocks on the worker.

**One ordering subtlety worth copying** (`crates/fleet-app/src/bridge.rs:333`-`:340`):

```rust
                        None => {
                            // Fire-and-forget mutations still have an ordering contract. In
                            // particular, one TerminalInput request is one key press; spawning
                            // each request before it reaches the client's FIFO can scramble a
                            // fast typist's bytes. Await only the enqueue, never the response.
                            if let Some(client) = link.as_ref().map(|link| link.client.clone()) {
                                let _ignored = client.request_background(*body).await;
                            }
                        }
```

For `fleet-lazygit` the analogue is: a rapid `<space> <space> <space>` over three files must
reach `Repository` in order. `Repository` already serializes mutations internally, but the
*enqueue* must stay ordered — so `await` the enqueue in the worker loop and never
`tokio::spawn` a mutation.

### d.2 Getting results back into gpui entities — the full receive loop

The pattern to copy verbatim (`crates/fleet-app/src/shell/root.rs:136`-`:165`):

```rust
    fn spawn_event_loop(bridge: &Bridge, cx: &mut Context<Self>) -> Task<()> {
        let events = bridge.events();
        cx.spawn(async move |shell, cx| {
            while let Ok(event) = events.recv().await {
                let now = Instant::now();
                // A lagged broadcast leaves every mirror with a hole no later diff can fill;
                // only a full frame repairs it, and nothing else in the app asks for one.
                let lagged = matches!(event, BridgeEvent::EventsLagged { .. });
                let updated = shell.update(cx, |shell, cx| {
                    let stale = shell.state.update(cx, |state, cx| {
                        state.apply_bridge_event(event, now);
                        cx.notify();
                        if lagged {
                            state.grids.keys().copied().collect()
                        } else {
                            Vec::new()
                        }
                    });
                    for terminal in stale {
                        shell
                            .bridge
                            .send(RequestBody::RequestFullFrame { terminal });
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        })
    }
```

The nesting is the whole idiom:
`cx.spawn(async move |weak_entity, cx| …)` → `weak.update(cx, |this, cx| …)` →
`this.state.update(cx, |state, cx| { …mutate…; cx.notify(); })`.

**On this gpui version `Entity::update` inside an async task returns `Result<R>`** — hence
`if updated.is_err() { return; }`. (`docs/research/gpui.md:349`-`:361` claims `main` returns a
bare `R`; the code in this repo disagrees, and the code wins:
`crates/fleet-app/src/shell/root.rs:160`-`:162`, `crates/fleet-app/src/drive.rs:299`-`:310`.)

The periodic-tick loop has the same shape
(`crates/fleet-app/src/shell/root.rs:167`-`:189`), driven by
`cx.background_executor().timer(TICK).await` with
`const TICK: Duration = Duration::from_millis(250);`
(`crates/fleet-app/src/shell/root.rs:34`-`:35`).

**Both tasks are stored, not detached** (`crates/fleet-app/src/shell/root.rs:74`-`:75`, `:109`,
`:119`-`:120`):

```rust
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
```
```rust
        let tasks = vec![Self::spawn_event_loop(&bridge, cx), Self::spawn_ticker(cx)];
```

This is mandatory. `docs/research/gpui.md:1918`-`:1921`:

> ### 7.10 `Task` is cancel-on-drop
>
> `cx.spawn(...)` / `background_executor().spawn(...)` return a `Task<T>`. **Dropping it cancels
> the future.** Always `.detach()` or store it in your entity. This is the #1 "my async work
> never runs" bug.

The same applies to `Subscription` (`docs/research/gpui.md:1969`).

**The single-reply pattern**, for a request whose answer is not also broadcast
(`crates/fleet-app/src/shell/root.rs:544`-`:581`, abridged):

```rust
        let reply = self.bridge.request(RequestBody::Doctor);
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let answer = reply.recv().await;
            cx.update(|cx| {
                state.update(cx, |app, cx| {
                    match answer {
                        Ok(Ok(ResponseBody::Doctor(checks))) => app.doctor = Some(checks),
                        Ok(Err(error)) => { /* into the sticky error slot */ }
                        Ok(Ok(_)) | Err(_) => { /* "the daemon did not answer" */ }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
```

Note the **three-way match** — `Ok(Ok(payload))` / `Ok(Err(domain_error))` /
`Err(channel_closed)` — and that **every** arm produces user-visible state. Nothing is dropped.
`fleet-lazygit` should do the same for every `Result<MutationResult>`.

For a one-shot *blocking* child process, `cx.background_spawn` is the right tool
(`crates/fleet-app/src/drive.rs:342`-`:349` shells out to `screencapture` that way). But git
subprocesses belong on the bridge thread, not here.

### d.3 The worker loop: errors, disconnects, shutdown

The whole loop is `crates/fleet-app/src/bridge.rs:287`-`:410`. Four rules to copy:

**1. A closed event channel returns immediately.** Every `events.send(...).await.is_err()` is
followed by `return` (`crates/fleet-app/src/bridge.rs:294`, `:301`, `:348`, `:355`, `:377`,
`:387`, `:403`). When the UI drops the receiver, the worker exits on its next send.

**2. The command channel closing is treated exactly like `Shutdown`**
(`crates/fleet-app/src/bridge.rs:362`):

```rust
                Ok(Command::Shutdown) | Err(_) => return,
```

**3. Retry is a *state*, never an inline sleep.** This comment is the single most valuable
lesson in the file (`crates/fleet-app/src/bridge.rs:307`-`:321`):

```rust
    let mut ticker = tokio::time::interval(HEALTH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The reconnect is a *state*, not an inline loop. Sleeping through the backoff inside the
    // ticker arm stopped `commands.recv()` from being polled at all, so `Shutdown` and every
    // pending `Request` piled up unanswered — and `ctrl-shift-q`, which awaits its reply
    // before quitting, hung the window for as long as fleetd stayed down.
    let mut backoff = Backoff::Idle;
```

with the "disabled select arm" helper (`crates/fleet-app/src/bridge.rs:277`-`:285`):

```rust
/// Resolves after `delay`, or never when there is nothing scheduled.
///
/// A `select!` arm needs a future either way; `pending()` is how "this arm is disabled this
/// round" is spelled without duplicating the whole loop.
async fn after(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending().await,
    }
}
```

**4. A request that cannot be served answers immediately rather than hanging**
(`crates/fleet-app/src/bridge.rs:322`-`:342`, `:422`-`:436`):

```rust
                Ok(Command::Request { body, reply }) => {
                    // With no link this answers `offline` immediately, which is what lets a
                    // caller awaiting its reply make progress while the daemon is down.
```

`dispatch` never blocks the loop: it `tokio::spawn`s the request and sends the answer down the
reply channel (`crates/fleet-app/src/bridge.rs:431`-`:435`).

Resource cleanup is RAII on the worker side (`crates/fleet-app/src/bridge.rs:243`-`:254`):

```rust
/// One connected daemon plus the task forwarding its events.
struct Link {
    client: Client,
    pid: u32,
    forwarder: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}
```

**Broadcast lag is an event, not a silent gap** — directly relevant to `fleet-lazygit`, because
`Repository::subscribe_commands` is also a `tokio::sync::broadcast`
(`crates/fleet-git/src/repository.rs:75`). `crates/fleet-app/src/bridge.rs:482`-`:519`:

```rust
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    if events
                        .send(BridgeEvent::EventsLagged { dropped })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
```

with the reasoning at `crates/fleet-app/src/bridge.rs:484`-`:490`: a lagged broadcast is **not
self-healing** — a snapshot resynchronises, but incrementally-rebuilt state has a hole no later
diff can fill.

> **For `fleet-lazygit`:** a dropped `CommandEvent` leaves a hole in the command-log panel.
> Handle `RecvError::Lagged` explicitly and recover by re-reading
> `Repository::recent_commands()` (`crates/fleet-git/src/repository.rs:81`), not by hoping the
> next event repairs it.

App-side shutdown (`crates/fleet-app/src/shell/root.rs:677`-`:693`) calls
`self.bridge.shutdown()` then `cx.quit()`; the quit-and-stop variant awaits its reply first.

### d.4 The central state entity and its reducers

```rust
// crates/fleet-app/src/state.rs:1-11
//! The application's snapshot and terminal-grid state mirror.
//!
//! [`AppState`] is a gpui entity owned by [`crate::shell::Shell`]. It holds the daemon's
//! authoritative [`Snapshot`], one [`MirrorGrid`] per attached terminal, and every piece of
//! purely local state the UX spec calls for: which screen and mode we are in, the cursor of
//! each list, the MRU orders that make `ctrl-s w` and `ctrl-s Tab` one keystroke, the live
//! toasts, the sticky error slot, and the daemon connection state of §3.12.
//!
//! Everything that decides *what the next keystroke does* lives here as a plain function or a
//! `&mut self` reducer with no gpui and no I/O, so it can be unit tested. Screens read the
//! result; they never derive it a second time.
```

`AppState` is declared at `crates/fleet-app/src/state.rs:663`-`:752`. Its field groups map
almost one-to-one onto what `GitUiState` needs (§f.2):

| Group | Representative fields | Lines |
|---|---|---|
| Server mirror (authoritative) | `snapshot`, `snapshot_at`, `grids` | `state.rs:675`, `:682`, `:684` |
| Connection lifecycle | `daemon`, `daemon_since`, `link_generation` | `:669`-`:673`, `:747` |
| Navigation / routing | `screen`, `hub_pane`, `pr_tab`, `scope`, `cursors` | `:686`-`:694` |
| Input mode / keyboard ownership | `terminal_mode`, `overlay`, `filter` | `:696`-`:700` |
| View chrome toggles | `detail_open`, `rail_collapsed`, `zoomed` | `:702`-`:706` |
| Transient feedback | `toasts`, `sticky_error`, `seen_failed`, `jobs_focus` | `:712`-`:718` |
| Client-owned overrides | `renamed_terminals`, `pr_badges`, `breadcrumb_row`, `doctor` | `:720`-`:751` |
| Private monotonic guard | `has_seen_non_empty_state` (**not `pub`**) | `:680` |

**The constructor takes the clock as a parameter and holds no context**
(`crates/fleet-app/src/state.rs:754`-`:758`):

```rust
impl AppState {
    /// The state the app starts in: cold start, Hub, worktrees, nothing open.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, now: Instant) -> Self {
```

That is what makes every reducer testable with no gpui at all. **Copy this.**

It lives as `Entity<AppState>` on the root view
(`crates/fleet-app/src/shell/root.rs:62`-`:76`) and is created in `Shell::new`
(`crates/fleet-app/src/shell/root.rs:78`-`:85`):

```rust
impl Shell {
    /// Builds the shell, starts the daemon bridge and wires both background loops.
    pub fn new(home: PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = Bridge::start(home.clone());
        let state = cx.new(|_| AppState::new(home, Instant::now()));

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe(&state, |_, _, cx| cx.notify()));
```

**There is exactly one `cx.observe` in the entire app** — that line
(`crates/fleet-app/src/shell/root.rs:85`). Everything else reads with `.read(cx)`
(`crates/fleet-app/src/shell/root.rs:297`, `:513`, `:773`, `:793`-`:794`, `:876`). One observer
on the root plus `cx.notify()` inside every `state.update` is the whole reactivity model — no
per-view subscriptions.

**The reducer shape.** There is **no `fn reduce(state, msg)` free function**. Instead:

1. **One message-dispatch reducer** taking an enum plus a clock, on `&mut self` —
   `AppState::apply_bridge_event(&mut self, event: BridgeEvent, now: Instant)`
   (`crates/fleet-app/src/state.rs:1147`), which delegates to
   `apply_daemon_event` (`:1224`), `apply_snapshot` (`:991`), `apply_job` (`:1246`) and friends.
2. **Small `&mut self` intent reducers** — `enter_prefix` (`:892`), `leave_prefix` (`:902`),
   `open_overlay`, `close_overlay` (`:~940`), `cancel` (`:~952`) — **most returning `bool`
   meaning "did anything change", so the caller decides whether to `cx.notify()`**:
   ```rust
   // crates/fleet-app/src/shell/root.rs:200-206
       fn close_overlay(&mut self, cx: &mut Context<Self>) {
           self.state.update(cx, |state, cx| {
               if state.close_overlay() {
                   cx.notify();
               }
           });
       }
   ```
3. **Pure free functions** for anything that is a computation over data —
   `reconnect_backoff` (`crates/fleet-app/src/state.rs:228`-`:233`),
   `move_cursor` (`:602`-`:623`), `filter_escape` (`:651`-`:659`), `clamp_cursor`,
   `expire_toasts`.
4. **A time-driven reducer** returning "repaint needed":
   `pub fn tick(&mut self, now: Instant) -> bool` (`crates/fleet-app/src/state.rs:1124`).
5. **A derived key-context chain**, so *state* rather than the element tree is authoritative:
   `pub fn context_chain(&self) -> Vec<&'static str>`
   (`crates/fleet-app/src/state.rs:797`).

One reducer arm worth reading in full, because `fleet-lazygit`'s snapshot handler is its twin
(`crates/fleet-app/src/state.rs:991`-`:1005`):

```rust
    /// Replaces the snapshot mirror and re-derives everything that hangs off it.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot, now: Instant) {
        self.has_seen_non_empty_state |= !snapshot.contexts.is_empty()
            || !snapshot.repos.is_empty()
            || !snapshot.clones.is_empty();
        self.sticky_error =
            crate::views::sticky_error::sticky_error_for(&snapshot.jobs, &self.seen_failed);
        self.cursors.repos = clamp_cursor(self.cursors.repos, snapshot.repos.len() + 1);
        self.cursors.worktrees = clamp_cursor(self.cursors.worktrees, snapshot.worktrees.len());
        self.cursors.jobs = clamp_cursor(self.cursors.jobs, snapshot.jobs.len());
        self.forget_vanished(&snapshot);
        self.snapshot = Some(snapshot);
        self.snapshot_at = Some(now);
    }
```

Note the order: re-derive dependent state, **clamp every cursor**, garbage-collect vanished
keys, *then* install the new snapshot and stamp the time. `fleet-lazygit`'s version replaces
`clamp_cursor` with `ListCursor::retain` (§c) so the cursor *follows the item* rather than
merely staying in range.

### d.5 Where the responsibilities sit

| File | Owns | Never does |
|---|---|---|
| `crates/fleet-app/src/state.rs` (~2025 lines) | `AppState` + every reducer + pure derivations + the mirror grid + MRU + toast law + cursor math. Imports gpui only for `SharedString` (`state.rs:960`). | no gpui contexts, no I/O, no `cx` |
| `crates/fleet-app/src/shell/root.rs` (~1152 lines) | The `Shell` view: creates the entity, spawns both loops, holds the two focus handles, registers ~45 `on_action` listeners (`:953`-`:1007`), renders the frame, owns the quit flow, and `run()` opens the window (`:1037`-`:1088`) | holds no domain state of its own |
| `crates/fleet-app/src/shell/daemon.rs` (265 lines) | **Pure presentation functions over `&DaemonLink`** — `banner_spec`, `dot_state`, `splash` — taking `now: Instant` as a parameter, with their own `#[cfg(test)] mod tests` (`:191`-`:265`) | no state mutation |
| `crates/fleet-app/src/drive.rs` (485 lines) | The `FLEET_DRIVE` developer-only scripted-input driver | **no reducers, no `AppState`** |

> **Correction worth flagging:** `drive.rs` contains **no reducers** — it is the scripted-input
> driver (§e.4). All reducers live in `state.rs`. `crates/fleet-app/src/shell/daemon.rs` is the
> file to copy for "render decisions as unit-testable pure functions".

### d.6 Tests over the reducers

`crates/fleet-app/src/state.rs` carries a large in-file `#[cfg(test)] mod tests` — ~25 tests
between `crates/fleet-app/src/state.rs:1485` and `:2010`, each a plain function over
`AppState::new(home, now)` with an injected `Instant`, no gpui and no window. Representative
names, which double as a specification:

- `mru_moves_entries_to_the_front_and_exposes_the_alternate` (`:1485`)
- `toasts_coalesce_within_a_second_and_cap_at_three` (`:1499`)
- `toasts_are_never_errors` (`:1535`)
- `cursor_movement_clamps_instead_of_wrapping` (`:1562`)
- `context_chain_follows_the_screen_the_overlay_and_the_daemon` (`:1661`)
- `prefix_is_one_shot` (`:1825`)
- `escape_clears_the_filter_in_two_stages_and_never_quits` (`:1849`)
- `a_daemon_restart_drops_the_mirror_grids_and_says_so` (`:1882`)
- `the_reconnect_banner_expires_on_a_tick` (`:1914`)
- `a_snapshot_without_a_terminal_frees_its_mirror_and_its_mru_entry` (`:1993`)

Integration tests live in `crates/fleet-app/tests/`, and their doc comment explains the trick
that makes a GUI testable (`crates/fleet-app/tests/jobs_panel.rs:1`-`:6`):

```rust
//! End-to-end checks for the Jobs panel and the daemon states it renders (UX-SPEC §3.7, §3.12).
//!
//! These tests run a **real `fleetd`** against a temporary `FLEET_HOME` with a fake `gh` on
//! `PATH` (see `tests/common/mod.rs`), connect with `fleet-client`, and then feed the daemon's
//! own snapshot and job records through the panel's pure logic. No window is opened: everything
//! §3.7 decides is a function of `&[JobRecord]`, which is exactly why it is testable this way.
```

> **The direct analogue for `fleet-lazygit`:** build a real repository in a `tempfile::TempDir`
> (as `crates/fleet-git/tests/repository.rs` already does), take a `RepoSnapshot`, and feed it
> through the panels' pure row-building functions. **Never open a window in a test.** Keep
> every rendering decision — glyph, tone, column text, ahead/behind string — in a free function
> over `&FileStatus` / `&Branch` / `&Commit`, exactly as `shell/daemon.rs` does over
> `&DaemonLink`.

### d.7 A copyable skeleton for `fleet-lazygit`

```rust
// crates/fleet-lazygit/src/bridge.rs
use std::path::PathBuf;
use std::thread;

use async_channel::{Receiver, Sender};
use fleet_git::{
    ChangeEvent, CommandEvent, Diff, DiffSide, MutationResult, ObjectId, RepoSnapshot,
    Repository, SnapshotOptions,
};

/// A request from the UI to the git thread.
pub enum GitRequest {
    Snapshot,
    FileDiff { path: PathBuf, side: DiffSide },
    CommitDiff { oid: ObjectId },
    StagePaths(Vec<PathBuf>),
    UnstagePaths(Vec<PathBuf>),
    Shutdown,
}

/// A message from the git thread to the UI.
pub enum GitEvent {
    Opened { root: PathBuf },
    OpenFailed { message: String },
    Snapshot(Box<RepoSnapshot>),
    FileDiff { path: PathBuf, side: DiffSide, diff: Box<Diff> },
    CommitDiff { oid: ObjectId, diff: Box<Diff> },
    Mutated { label: String, result: Box<MutationResult> },
    /// A read or mutation failed. Always followed by a fresh snapshot request.
    Failed { label: String, message: String },
    Command(Box<CommandEvent>),
    /// The command-log broadcast dropped `dropped` events; re-read `recent_commands()`.
    CommandsLagged { dropped: u64 },
    /// The watcher saw the worktree change. One event, one snapshot.
    Changed(Box<ChangeEvent>),
}

/// The handle the UI keeps. Cloning it is cheap and safe from any thread.
#[derive(Clone)]
pub struct GitBridge {
    requests: Sender<GitRequest>,
    events: Receiver<GitEvent>,
}

impl GitBridge {
    /// Parks a multi-threaded Tokio runtime on one background thread.
    /// Shape copied from crates/fleet-app/src/bridge.rs:136-157.
    pub fn start(path: PathBuf) -> Self {
        let (requests, request_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::unbounded();
        let thread_events = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-lazygit-git".to_owned())
            .spawn(move || run_thread(path, request_rx, thread_events))
        {
            // A thread that cannot start is a domain failure, not a panic (bridge.rs:145-151).
            let _ignored = event_tx.try_send(GitEvent::OpenFailed {
                message: format!("could not start the git thread: {error}"),
            });
        }
        Self { requests, events }
    }

    /// The stream the root view drains in one `cx.spawn` loop.
    pub fn events(&self) -> Receiver<GitEvent> {
        self.events.clone()
    }

    /// Fire-and-forget: the outcome arrives as an event. (bridge.rs:171-177)
    pub fn send(&self, request: GitRequest) {
        let _ignored = self.requests.try_send(request);
    }
}

/// Builds the runtime and blocks on the loop. (bridge.rs:225-241)
fn run_thread(path: PathBuf, requests: Receiver<GitRequest>, events: Sender<GitEvent>) {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ignored = events.try_send(GitEvent::OpenFailed {
                message: format!("could not start the git runtime: {error}"),
            });
            return;
        }
    };
    runtime.block_on(run(path, &requests, &events));
}

async fn run(path: PathBuf, requests: &Receiver<GitRequest>, events: &Sender<GitEvent>) {
    let repository = match Repository::discover(&path).await {   // fleet-git/src/repository.rs:23
        Ok(repository) => repository,
        Err(error) => {
            let _ignored = events.send(GitEvent::OpenFailed { message: error.to_string() }).await;
            return;
        }
    };
    if events
        .send(GitEvent::Opened { root: repository.paths().root.clone() })
        .await
        .is_err()
    {
        return;   // The UI dropped the receiver. (bridge.rs:294)
    }

    let mut commands = repository.subscribe_commands();      // fleet-git/src/repository.rs:75
    let (_watcher, changes) = match fleet_git::watch::RepoWatcher::new(repository.paths()) {
        Ok(pair) => pair,                                    // fleet-git/src/watch.rs:20
        Err(_) => return,
    };

    loop {
        tokio::select! {
            request = requests.recv() => match request {
                // A closed request channel is a shutdown. (bridge.rs:362)
                Ok(GitRequest::Shutdown) | Err(_) => return,
                Ok(GitRequest::Snapshot) => {
                    let event = match repository.snapshot(SnapshotOptions::default()).await {
                        Ok(snapshot) => GitEvent::Snapshot(Box::new(snapshot)),
                        Err(error) => GitEvent::Failed {
                            label: "snapshot".to_owned(),
                            message: error.to_string(),
                        },
                    };
                    if events.send(event).await.is_err() { return; }
                }
                Ok(GitRequest::StagePaths(paths)) => {
                    // Await the mutation, then ALWAYS re-snapshot — even on failure (§f.4).
                    let event = match repository.stage_paths(&paths).await {
                        Ok(result) => GitEvent::Mutated {
                            label: "stage".to_owned(),
                            result: Box::new(result),
                        },
                        Err(error) => GitEvent::Failed {
                            label: "stage".to_owned(),
                            message: error.to_string(),
                        },
                    };
                    if events.send(event).await.is_err() { return; }
                    match repository.snapshot(SnapshotOptions::default()).await {
                        Ok(snapshot) => {
                            if events.send(GitEvent::Snapshot(Box::new(snapshot))).await.is_err() {
                                return;
                            }
                        }
                        Err(_) => {}
                    }
                }
                Ok(_) => { /* the remaining reads, same shape */ }
            },

            // The command log is a tokio broadcast: lag must be surfaced. (bridge.rs:506-515)
            command = commands.recv() => match command {
                Ok(event) => {
                    if events.send(GitEvent::Command(Box::new(event))).await.is_err() { return; }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
                    if events.send(GitEvent::CommandsLagged { dropped }).await.is_err() {
                        return;
                    }
                }
            },

            // fleet-git already debounces; one ChangeEvent means one snapshot. (§f.4)
            change = changes.recv() => match change {
                Ok(change) => {
                    if events.send(GitEvent::Changed(Box::new(change))).await.is_err() { return; }
                }
                Err(_) => return,
            },
        }
    }
}
```

```rust
// crates/fleet-lazygit/src/root.rs — the UI side of the loop
impl Lazygit {
    /// Shape copied verbatim from crates/fleet-app/src/shell/root.rs:136-165.
    fn spawn_event_loop(bridge: &GitBridge, cx: &mut Context<Self>) -> gpui::Task<()> {
        let events = bridge.events();
        cx.spawn(async move |root, cx| {
            while let Ok(event) = events.recv().await {
                let now = std::time::Instant::now();
                let updated = root.update(cx, |root, cx| {
                    let follow_up = root.state.update(cx, |state, cx| {
                        let follow_up = state.apply_git_event(event, now);   // the reducer
                        cx.notify();
                        follow_up
                    });
                    for request in follow_up {
                        root.bridge.send(request);
                    }
                });
                if updated.is_err() {
                    return;     // Entity::update returns Result in an async task.
                }
            }
        })
    }

    pub fn new(path: std::path::PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = GitBridge::start(path.clone());
        let state = cx.new(|_| GitUiState::new(path, std::time::Instant::now()));

        let mut subscriptions = Vec::new();
        // One observer on the root; every reducer calls cx.notify(). (root.rs:85)
        subscriptions.push(cx.observe(&state, |_, _, cx| cx.notify()));

        // Tasks MUST be stored or they are cancelled on drop (gpui.md:1918-1921).
        let tasks = vec![Self::spawn_event_loop(&bridge, cx), Self::spawn_refresh_timer(cx)];

        bridge.send(GitRequest::Snapshot);

        Self {
            state,
            bridge,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
            _tasks: tasks,
        }
    }
}
```

The reducer returns the follow-up requests rather than sending them itself, so it stays pure
and unit-testable:

```rust
// crates/fleet-lazygit/src/state.rs
impl GitUiState {
    /// Applies one message from the git bridge and returns the requests it implies.
    ///
    /// Pure: no gpui, no I/O, no clock of its own. Shape from
    /// crates/fleet-app/src/state.rs:1147-1210.
    pub fn apply_git_event(&mut self, event: GitEvent, now: Instant) -> Vec<GitRequest> {
        match event {
            GitEvent::Snapshot(snapshot) => {
                // Epoch check: a late snapshot is dropped. (§f.4 rule 4)
                if snapshot.generation <= self.epoch && self.snapshot.is_some() {
                    return Vec::new();
                }
                self.epoch = snapshot.generation;
                self.refreshing = false;
                self.last_error = None;
                self.adopt_snapshot(Arc::new(*snapshot));   // re-derives every cursor
                self.main_requests()                         // re-fetch the selected diff
            }
            GitEvent::Failed { label, message } => {
                self.last_error = Some(format!("{label}: {message}"));
                self.refreshing = true;
                // Refresh after a FAILED mutation too — the index may have moved.
                vec![GitRequest::Snapshot]
            }
            GitEvent::Mutated { result, .. } => {
                self.command_log.extend(result.records.iter().cloned());
                if let Some(warning) = result.warning.clone() {
                    self.toast(warning, now);
                }
                vec![GitRequest::Snapshot]
            }
            GitEvent::Changed(_) => {
                // fleet-git already debounced. One event, one snapshot, no second timer.
                if self.refreshing { Vec::new() } else { vec![GitRequest::Snapshot] }
            }
            GitEvent::CommandsLagged { .. } => Vec::new(),   // caller re-reads recent_commands()
            _ => Vec::new(),
        }
    }
}
```
## e. Build, test and lint conventions

### e.1 Makefile targets

The whole workspace contract is seven targets (`Makefile:1`-`Makefile:22`):

```make
.PHONY: check build run-app run-daemon test fmt clippy      # Makefile:1

check:                                                       # Makefile:3
	cargo check --workspace                                   # Makefile:4

build:                                                       # Makefile:6
	cargo build --workspace                                   # Makefile:7

run-app:                                                     # Makefile:9
	cargo run -p fleet-app                                    # Makefile:10

run-daemon:                                                  # Makefile:12
	cargo run -p fleet-daemon                                 # Makefile:13

test:                                                        # Makefile:15
	cargo test --workspace                                    # Makefile:16

fmt:                                                         # Makefile:18
	cargo fmt --all                                           # Makefile:19

clippy:                                                      # Makefile:21
	cargo clippy --workspace --all-targets --all-features -- -D warnings   # Makefile:22
```

`make clippy` is the gate: `-D warnings` means **any** clippy lint fails the build. Run
`make fmt && make clippy && make test` before every commit.

Once `crates/fleet-lazygit` exists and is listed in `Cargo.toml:3`-`:13`, it is picked up by
`--workspace` automatically. Add a `run-lazygit:` target mirroring `run-app`:

```make
run-lazygit:
	cargo run -p fleet-lazygit
```

### e.2 Workspace manifest facts

```toml
# Cargo.toml:1-18
[workspace]
resolver = "3"
members = [
    "crates/fleet-core",
    "crates/fleet-git",
    "crates/fleet-proto",
    "crates/fleet-term",
    "crates/fleet-daemon",
    "crates/fleet-client",
    "crates/fleet-ui-kit",
    "crates/fleet-cli",
    "crates/fleet-app",
]

[workspace.package]
edition = "2024"
rust-version = "1.97.1"
```

Dependencies you will want, already pinned in `[workspace.dependencies]`
(`Cargo.toml:19`-`Cargo.toml:40`) — always use `x.workspace = true` rather than a fresh
version: `anyhow` (`:20`), `async-channel` (`:21`), `gpui` (`:26`), `gpui_platform` (`:27`),
`serde` (`:30`), `thiserror` (`:33`), `tokio` (`:34`), `tracing` (`:36`),
`tracing-subscriber` (`:37`), `notify` (`:40`).

Dev profile (`Cargo.toml:42`-`Cargo.toml:46`) builds the workspace at `opt-level = 1` but all
dependencies at `opt-level = 3`, so a debug `fleet-lazygit` is still usable interactively —
you generally do **not** need `--release` for day-to-day work.

`crates/fleet-git/Cargo.toml` is the model to copy for the new crate's manifest, including how
it declares its extra binary (`crates/fleet-git/Cargo.toml:19`-`:21`):

```toml
[[bin]]
name = "fleet-git-seqedit"
path = "src/bin/fleet-git-seqedit.rs"
```

### e.3 Toolchain, lint and formatting configuration

```toml
# rust-toolchain.toml:1-4
[toolchain]
channel = "1.97.1"
profile = "minimal"
components = ["rustfmt", "clippy"]
```

**Rust 1.97.1 is a hard floor, not a preference.** `docs/research/gpui.md:1785`-`:1789`:

> ### 7.3 ⚠️ `main`-era gpui needs Rust ≥ 1.97 **[HIT LOCALLY]**
>
> On Rust 1.94.1: `error[E0658]: use of unstable library feature 'cold_path'` — `std::hint::cold_path()`
> in the scheduler. Zed `main` pins `channel = "1.97.1"`.

There is **no `clippy.toml`, no `rustfmt.toml`, and no `[lints]` table** anywhere in the
workspace — the lint policy is entirely the `-D warnings` on `make clippy`
(`Makefile:22`). `.cargo/config.toml` contains only a Zig cache override
(`.cargo/config.toml:1`-`:2`), nothing that affects lints.

Crate-level lint attributes are the convention instead: `crates/fleet-git/src/lib.rs:5` opens
with `#![warn(missing_docs)]`, and `crates/fleet-ui-kit/src/lib.rs:41` does the same.
**`crates/fleet-lazygit/src/lib.rs` should start with `#![warn(missing_docs)]` to match** —
which, combined with `-D warnings`, means every public item needs a doc comment.

Two more compile-environment traps to know before your first build (neither is your code's
fault):

- `docs/research/gpui.md:1762`-`:1771` — on macOS 26 the Metal Toolchain is a separate 688 MB
  download; `cargo::error=metal shader compilation failed` is the symptom, and
  `xcodebuild -downloadComponent MetalToolchain` is the fix.
- `docs/research/gpui.md:1988`-`:2020` — **the biggest trap**: `gpui_platform`'s
  `default = []`, and without the `font-kit` feature macOS silently swaps in `NoopTextSystem`,
  which renders **zero glyphs, with no panic**. The workspace already pins
  `features = ["font-kit"]` at `Cargo.toml:27`, so **depend on
  `gpui_platform.workspace = true` and never re-declare gpui with your own features**. Two
  gpui copies cannot interoperate (`docs/research/gpui.md:2221`).

### e.4 Running the app, and the smoke/screenshot recipe

`docs/DEVELOPMENT.md:3`-`:5` — the toolchain file makes plain `cargo` correct automatically.
`docs/DEVELOPMENT.md:30`-`:34` — artifacts go to the repo-local `target/`; **do not point
multiple worktrees at one external `CARGO_TARGET_DIR`**, because Cargo's relative dep-info paths
can make one worktree accept another's stale artifacts.

**Zig is only needed for `fleet-term`** (`docs/DEVELOPMENT.md:36`-`:54`): building it with its
default `ghostty` feature requires Zig 0.15.2, installed by `scripts/bootstrap-zig.sh`
(`docs/DEVELOPMENT.md:43`), which verifies the published SHA-256, installs to
`~/.local/zig-0.15.2/zig` and exposes it as `~/.cargo/bin/zig` — so `~/.cargo/bin` must be on
`PATH` (`docs/DEVELOPMENT.md:47`-`:49`).

> **`crates/fleet-lazygit` does not depend on `fleet-term`, so it does not need Zig at all.**
> Build it with `cargo build -p fleet-lazygit` rather than `--workspace` and you skip the Zig
> requirement entirely. Note that `make check` / `make build` / `make test` *are*
> `--workspace`, so they do need it.

Logging (`docs/DEVELOPMENT.md:95`-`:101`):

```sh
RUST_LOG=fleet_app=debug ./target/debug/fleet > /tmp/fleet-gui/app.log 2>&1
```

`docs/research/gpui.md:1979`-`:1985` explains why this matters more than usual:

> ### 7.17 Always install a `log` backend
>
> Every soft failure in GPUI — "no text will be rendered", asset load errors, font resolution
> fallbacks — is a `log::warn!`/`log::error!` call that goes nowhere without a logger. […] This
> is the single highest-leverage debugging step for a GPUI app; without it, GPUI fails silently.

**The real smoke/screenshot recipe is the `FLEET_DRIVE` driver in `docs/DEVELOPMENT.md:56`-`:91`**,
not `docs/research/gpui-smoke-recipe.md` (which documents crates.io `gpui 0.2.2`, a different
API era from the `v1.18.1` git dependency this repo uses, and contains no screenshot
procedure).

`docs/DEVELOPMENT.md:58`-`:60`:

> `fleet` ships a developer-only scripted-input driver so an automated reviewer can exercise the
> real GUI on a machine where `osascript` keystrokes are blocked by the macOS Accessibility
> permission. It is off unless `FLEET_DRIVE` names a file:

```sh
# docs/DEVELOPMENT.md:62-65
: > /tmp/fleet-drive/script.txt
FLEET_HOME=/tmp/fleet-drive FLEET_DRIVE=/tmp/fleet-drive/script.txt ./target/debug/fleet &
```

```sh
# docs/DEVELOPMENT.md:71-74 — the script can be appended to while the app runs
printf 'wait 500\nkey ?\nshot /tmp/fleet-drive/help.png\nkey escape\nquit\n' \
  >> /tmp/fleet-drive/script.txt
```

The command table (`docs/DEVELOPMENT.md:76`-`:82`):

| Line | Effect |
|---|---|
| `key <keystroke>...` | Dispatches each gpui keystroke to the window (`ctrl-s`, `shift-tab`, `?`, `enter`, `escape`, `j`). Several per line: `key ctrl-s ?`. |
| `type <text>` | Dispatches every character as a keystroke (`shift-` for uppercase), so text inputs and the terminal receive it. Inner spaces are kept. |
| `wait <ms>` | Pauses the script before the next line. |
| `shot <path.png>` | Raises the window, runs `/usr/sbin/screencapture -x <path>` and waits for it, then logs `done shot <path>`. With more than one display it passes one path per display. |
| `quit` | Quits the app. |

**The synchronisation contract** (`docs/DEVELOPMENT.md:84`-`:88`):

> Blank lines and lines starting with `#` are ignored. Every executed line, every parse error
> and every finished screenshot is appended to `$FLEET_DRIVE.log` with a timestamp, **so a
> script runner can wait on `done shot <path>` instead of sleeping**. Keystrokes go through
> `Window::dispatch_keystroke`, so they take the same path as real input: bindings resolve
> against the focus chain of `docs/KEYMAP.md`.

`docs/DEVELOPMENT.md:90`-`:91` — the driver is `crates/fleet-app/src/drive.rs`, wired from
`shell::run` right after the window opens; with `FLEET_DRIVE` unset no task is spawned.

Implementation constants (`crates/fleet-app/src/drive.rs:32`-`:37`): `POLL = 100 ms`,
`SETTLE = 250 ms`, `SCREENCAPTURE = "/usr/sbin/screencapture"`. Three non-obvious notes if you
port it:

- `crates/fleet-app/src/drive.rs:131`-`:135` — *"`Window::dispatch_keystroke` does not run a
  synthetic keystroke through the platform's keyboard-layout translation, so the driver has to
  fill this field itself"* (`key_char`).
- `crates/fleet-app/src/drive.rs:326`-`:331` — a driven app is usually launched into the
  background, so the window must be raised and the compositor allowed to settle before a shot.
- `crates/fleet-app/src/drive.rs:179`-`:184` — `screencapture` photographs one display per
  file, hence one path per display.

> **Port `drive.rs` into `crates/fleet-lazygit` behind a `FLEET_LAZYGIT_DRIVE` env var.** It is
> the only way to get a screenshot of the real UI in this environment, and the
> `done shot <path>` log line is what makes an automated visual check deterministic.

### e.5 Parallel-ownership and documentation rules

`docs/DEVELOPMENT.md:116`-`:122`:

> ## Parallel ownership rule
>
> Only edit files in your assigned module. Never edit `lib.rs` or `mod.rs` except to add
> `pub use` re-exports of public items from your own module. The complete module tree is
> predeclared so agents can implement separate files without creating shared-file conflicts.
> Because `cargo fmt -p` still formats an entire crate, parallel work should run `rustfmt` on
> owned files and leave the workspace wide `cargo fmt --all` pass to integration.

That is why §f.1 predeclares the whole module tree for `crates/fleet-lazygit`: declare every
`mod` up front, then fill files independently.

The crate map at `docs/DEVELOPMENT.md:105`-`:114` should gain a `fleet-lazygit` row, and
`fleet-git` is missing from it too.

### e.6 The checklist before every commit

```sh
cargo build -p fleet-lazygit      # skips the Zig requirement of --workspace
cargo test -p fleet-lazygit
make fmt                          # Makefile:19  — cargo fmt --all
make clippy                       # Makefile:22  — -D warnings, all targets, all features
make test                         # Makefile:16  — cargo test --workspace
```

`make clippy` is the gate that actually fails CI-equivalent review, and with
`#![warn(missing_docs)]` plus `-D warnings` an undocumented public item is a build failure.
## f. Recommended UI architecture for `crates/fleet-lazygit`

### f.0 What `fleet-git` already gives you

Read this first — it determines the whole shape of the UI. All references are to the crate as
it stands; re-check signatures before you code against them, since the backend crate was
written concurrently with this document.

```rust
// crates/fleet-git/src/lib.rs:21-32
pub mod sequence_editor {
    pub use crate::rebase::{SEQUENCE_INSTRUCTION_ENV, maybe_run_from_env, run_sequence_editor};
}

pub use command::{GitCommand, GitOutput, Runner};
pub use command_log::{CommandEvent, CommandOutcome, CommandRecord};
pub use error::{GitError, Result};
pub use model::*;
pub use rebase::{FixupFlag, MoveDirection, RebaseAction, RebaseBase, RebasePlan, TodoEdit};
pub use repository::Repository;
```

**The snapshot** is the single read model — one struct holding every panel's data
(`crates/fleet-git/src/model.rs:334`-`:359`):

```rust
pub struct RepoSnapshot {
    pub root: PathBuf,
    pub head: Head,
    pub operation: OperationState,
    pub files: Vec<FileStatus>,
    pub local_branches: Vec<Branch>,
    pub remote_branches: Vec<RemoteBranchGroup>,
    pub remotes: Vec<Remote>,
    pub tags: Vec<Tag>,
    pub commits: Vec<Commit>,
    pub reflog: Vec<ReflogEntry>,
    pub stashes: Vec<StashEntry>,
    /// Monotonic snapshot generation for this `Repository` instance.
    pub generation: u64,
}
```

`generation` is your **epoch**: it is monotonic per `Repository`, so a late-arriving snapshot
with a smaller `generation` than the one you already hold must be dropped.

`SnapshotOptions` (`crates/fleet-git/src/model.rs:64`-`:87`) defaults to
`commit_limit: 300`, `reflog_limit: 100`, `include_tags: true`, `include_remotes: true`.

`OperationState` (`crates/fleet-git/src/model.rs:114`-`:141`) is what drives the status-bar mode
word and the banner:

```rust
pub enum OperationState {
    None,
    Merging,
    Rebasing {
        interactive: bool,
        onto: Option<String>,
        head_name: Option<String>,
        done: Option<usize>,
        total: Option<usize>,
    },
    CherryPicking,
    Reverting,
    Bisecting,
}
```

`Head` (`crates/fleet-git/src/model.rs:88`-`:113`) is `Branch { name, oid, description }` /
`Detached { oid, description }` / `Unborn { name }` — render all three; the unborn case is the
one people forget.

**Reads** (`crates/fleet-git/src/read.rs`) — every one is `async` and every one is what a
main-panel content type maps to:

| Method | Line | Main-panel use |
|---|---|---|
| `snapshot(&self, options: SnapshotOptions) -> Result<RepoSnapshot>` | `read.rs:21` | every side panel's rows |
| `diff_file(&self, path: &Path, side: DiffSide) -> Result<Diff>` | `read.rs:52` | Files panel → file diff |
| `diff_commit(&self, oid: &ObjectId, paths: &[PathBuf]) -> Result<Diff>` | `read.rs:69` | Commits panel → commit diff |
| `diff_stash(&self, index: usize) -> Result<Diff>` | `read.rs:81` | Stash panel → stash diff |
| `diff_range(&self, from: &Ref, to: &Ref) -> Result<Diff>` | `read.rs:88` | diffing mode (`W`) |
| `diff_branch(&self, name: &Ref) -> Result<Diff>` | `read.rs:94` | Branches panel → branch diff |
| `commit_files(&self, oid: &ObjectId) -> Result<Vec<CommitFile>>` | `read.rs:101` | commit-files sub-panel (`<enter>`) |
| `show_commit_message(&self, oid: &ObjectId) -> Result<String>` | `read.rs:112` | reword prefill |
| `file_at_ref(&self, reference: &Ref, path: &Path) -> Result<Vec<u8>>` | `read.rs:118` | conflict sides |
| `conflicted_file(&self, path: &Path) -> Result<ConflictFile>` | `read.rs:125` | merge-conflict view |
| `remotes(&self) -> Result<Vec<Remote>>` | `read.rs:107` | Remotes tab |

`DiffSide` is `Unstaged` / `Staged` (`crates/fleet-git/src/model.rs:363`-`:368`) — that is
lazygit's `<tab>` "switch view (staged/unstaged changes)" in the staging main panel.

**Mutations** (`crates/fleet-git/src/mutation.rs`) all return `Result<MutationResult>` and are
serialized per `Repository`. The set you will bind keys to:
`stage_paths` (`:13`), `unstage_paths` (`:20`), `stage_all` (`:27`), `unstage_all` (`:32`),
`discard_paths` (`:37`), `discard_all` (`:83`), `apply_patch_selection` (`:92`), `commit`
(`:112`), `checkout` (`:122`), `checkout_new_branch` (`:127`), `create_branch` (`:134`),
`delete_branch` (`:141`), `rename_branch` (`:146`), `merge` (`:151`), `rebase_onto` (`:161`),
`rebase_continue`/`rebase_abort`/`rebase_skip` (`:166`/`:168`/`:170`),
`merge_abort`/`merge_continue` (`:172`/`:174`), `cherry_pick` (`:177`),
`cherry_pick_continue`/`cherry_pick_abort` (`:183`/`:185`), `revert` (`:188`), `reset` (`:193`),
`create_tag`/`delete_tag` (`:199`/`:207`), `checkout_remote_branch` (`:212`),
`set_upstream`/`unset_upstream` (`:218`/`:224`), `stash_push` (`:229`),
`stash_apply`/`stash_pop`/`stash_drop` (`:239`/`:241`/`:243`), `stash_branch` (`:246`),
`fetch` (`:251`), `pull` (`:260`), `push` (`:270`), `resolve_conflict` (`:282`).

```rust
// crates/fleet-git/src/model.rs:528-533
pub struct MutationResult {
    /// Finished command records belonging to the mutation.
    pub records: Vec<CommandRecord>,
    /// Successful but notable Git output.
    pub warning: Option<String>,
}
```

Feed `records` into the command-log panel and surface `warning` as a toast.

**Partial staging** — lazygit's `<space>` in the staging main panel maps onto
`apply_patch_selection` (`crates/fleet-git/src/mutation.rs:92`) driven by these types
(`crates/fleet-git/src/model.rs:497`-`:527`):

```rust
pub struct HunkSelection {
    pub hunk_index: usize,
    /// Zero-based changed-line indexes within the hunk; `None` selects the whole hunk.
    pub lines: Option<Vec<usize>>,
}

pub struct PatchSelection {
    pub path: PathBuf,
    pub side: DiffSide,
    pub hunks: Vec<HunkSelection>,
}

pub enum PatchAction { Stage, Unstage, Discard }
```

`lines: None` is hunk mode; `lines: Some(vec)` is line mode. That is exactly lazygit's `a`
("toggle hunk selection — line-by-line vs. hunk selection mode").

**The command log** (`crates/fleet-git/src/command_log.rs`) is already built for you:

```rust
// crates/fleet-git/src/command_log.rs:40-52
pub struct CommandRecord {
    pub id: u64,
    pub kind: CommandKind,          // Read | Mutation | Network
    pub display_argv: Vec<String>,  // redacted argv, safe to display
    pub started_at: SystemTime,
    pub elapsed: Option<std::time::Duration>,
    pub outcome: CommandOutcome,    // Running | Success | Failed | TimedOut | SpawnFailed
}

// crates/fleet-git/src/command_log.rs:57-62
pub enum CommandEvent {
    Started(CommandRecord),
    Finished(CommandRecord),
}
```

Subscribe with `Repository::subscribe_commands(&self) -> broadcast::Receiver<CommandEvent>`
(`crates/fleet-git/src/repository.rs:75`) and backfill with
`Repository::recent_commands(&self) -> Vec<CommandRecord>`
(`crates/fleet-git/src/repository.rs:81`).

**The fs watcher** (`crates/fleet-git/src/watch.rs:13`-`:20`):

```rust
pub struct RepoWatcher { /* ... */ }
impl RepoWatcher {
    pub fn new(paths: &RepoPaths) -> Result<(Self, async_channel::Receiver<ChangeEvent>)>
}
```

It already hands you an `async_channel::Receiver` — the same channel type
`crates/fleet-app/src/bridge.rs` uses — and `ChangeEvent`
(`crates/fleet-git/src/model.rs:679`-`:686`) carries `paths: Vec<PathBuf>` plus the
`debounce: Duration` it coalesced over, so **debouncing is already done in the backend**. Do
not add a second debounce in the UI.

**Interactive rebase** (`crates/fleet-git/src/rebase.rs`):

```rust
// crates/fleet-git/src/rebase.rs:15
pub const SEQUENCE_INSTRUCTION_ENV: &str = "FLEET_GIT_SEQUENCE_INSTRUCTION";
// crates/fleet-git/src/rebase.rs:178
pub fn run_sequence_editor(todo_path: &Path, encoded_plan: &str) -> Result<()>
// crates/fleet-git/src/rebase.rs:194
pub fn maybe_run_from_env() -> Option<ExitCode>
```

with `RebasePlan { base: RebaseBase, onto: Option<String>, edits: Vec<TodoEdit>, autostash: bool, keep_empty: bool }`
(`crates/fleet-git/src/rebase.rs:76`-`:87`) and the high-level operations
`interactive_rebase` (`:100`), `squash_into_previous` (`:121`), `fixup_into_previous` (`:126`),
`drop_commit` (`:131`), `reword_commit` (`:136`), `move_commit` (`:148`), `edit_commit` (`:161`)
— each taking a `helper_exe: &Path`.

`fleet-git` already ships the helper as its own tiny binary
(`crates/fleet-git/Cargo.toml:19`-`:21`), whose entire body is
(`crates/fleet-git/src/bin/fleet-git-seqedit.rs`):

```rust
fn main() -> std::process::ExitCode {
    fleet_git::sequence_editor::maybe_run_from_env().unwrap_or(std::process::ExitCode::SUCCESS)
}
```

**You still put the same call at the top of `fleet-lazygit`'s `main`**, so that
`std::env::current_exe()` can be passed as `helper_exe` and the app re-executes itself as the
sequence editor without shipping a second file. This is the direct analogue of lazygit's
daemon mechanism (reference §d).

### f.1 Crate layout

Add `"crates/fleet-lazygit"` to `Cargo.toml:3`-`:13`.

```
crates/fleet-lazygit/
  Cargo.toml
  src/
    lib.rs          // pub fn run(path: PathBuf) -> anyhow::Result<()>  — boots gpui
    main.rs         // sequence-editor short-circuit, then arg parsing, then lib::run
    actions.rs      // every gpui::actions! namespace
    keymap.rs       // the declarative binding table + register(cx)
    bridge.rs       // dedicated Tokio thread owning the Repository + RepoWatcher
    state.rs        // GitUiState + pure reducers + unit tests
    root.rs         // the Render view: AppFrame, focus routing, action handlers
    panels/
      mod.rs
      status.rs        // panel 1
      files.rs         // panel 2 (Files / Worktrees / Submodules tabs)
      branches.rs      // panel 3 (Local / Remotes / Tags tabs)
      commits.rs       // panel 4 (Commits / Reflog tabs)
      stash.rs         // panel 5
      main.rs          // the main panel: dispatches on MainContent
      secondary.rs     // the secondary panel (staged half in staging mode)
      command_log.rs   // the extras/command-log panel
    dialogs/
      mod.rs
      confirm.rs    // ConfirmDialog wrapper: destructive yes/no
      prompt.rs     // single-line text input (branch name, tag name, commit message)
      menu.rs       // FuzzyList menu — lazygit's option menus
      rebase.rs     // the interactive-rebase todo editor
      upstream.rs   // set/unset upstream, push-with-upstream prompt
    views/
      mod.rs
      diff.rs         // unified-diff renderer with per-line tones
      commit_graph.rs // the ASCII/box-drawing graph column
      conflict.rs     // conflict-marker resolution view
      rows.rs         // one fn per row type: file_row, branch_row, commit_row, stash_row...
```

**`main.rs` — the order matters.** The sequence-editor check must happen before *anything*
else, in particular before gpui touches the window server:

```rust
// crates/fleet-lazygit/src/main.rs
fn main() -> std::process::ExitCode {
    // Git re-invokes this same executable as GIT_SEQUENCE_EDITOR. When the instruction env
    // var is set we are that editor: rewrite the todo file and exit without booting a UI.
    if let Some(code) = fleet_git::sequence_editor::maybe_run_from_env() {
        return code;
    }

    let path = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    match fleet_lazygit::run(path) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fleet-lazygit: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
```

`lib.rs` exposes `pub fn run(path: PathBuf) -> anyhow::Result<()>` and contains the gpui boot
(§a). Keeping the boot in `lib.rs` means an integration test or a smoke harness can drive it.

### f.2 `GitUiState`

One central entity, one `Arc<RepoSnapshot>` shared by every panel, pure reducers, no `async`
inside the reducer.

```rust
// crates/fleet-lazygit/src/state.rs
use std::path::PathBuf;
use std::sync::Arc;

use fleet_git::{
    CommandRecord, ConflictFile, Diff, DiffSide, ObjectId, RepoSnapshot,
};

/// Which side-column panel owns the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelId {
    Status,      // jump key `1`
    Files,       // `2`
    Branches,    // `3`
    Commits,     // `4`
    Stash,       // `5`
    Main,        // `0`
    Secondary,
    CommandLog,  // reachable from the `@` menu
}

/// lazygit's `+` / `_` screen modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScreenMode {
    #[default]
    Normal,
    Half,
    Full,
}

impl ScreenMode {
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Half,
            Self::Half => Self::Full,
            Self::Full => Self::Full,   // lazygit clamps; it does not wrap
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Self::Full => Self::Half,
            Self::Half => Self::Normal,
            Self::Normal => Self::Normal,
        }
    }
}

/// Per-panel tab selection (lazygit's `]` / `[`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Tabs {
    pub files: usize,     // 0 Files, 1 Worktrees, 2 Submodules
    pub branches: usize,  // 0 Local, 1 Remotes, 2 Tags
    pub commits: usize,   // 0 Commits, 1 Reflog
}

/// What the main panel is showing right now.
#[derive(Clone, Debug)]
pub enum MainContent {
    /// Status panel selected: repo summary + branch log.
    RepoSummary,
    /// A working-tree file's diff, split into the two sides.
    FileDiff {
        path: PathBuf,
        side: DiffSide,
        unstaged: Option<Arc<Diff>>,
        staged: Option<Arc<Diff>>,
    },
    /// Staging mode: same data as FileDiff, plus a hunk/line selection.
    Staging {
        path: PathBuf,
        side: DiffSide,
        diff: Arc<Diff>,
        selection: PatchCursor,
    },
    /// A commit's diff.
    CommitDiff { oid: ObjectId, diff: Option<Arc<Diff>> },
    /// A branch's log/diff.
    BranchDiff { name: String, diff: Option<Arc<Diff>> },
    /// A stash entry's diff.
    StashDiff { index: usize, diff: Option<Arc<Diff>> },
    /// Custom patch builder (lazygit's `<ctrl+p>`).
    PatchBuilder { selection: PatchCursor },
    /// Conflict resolution for one path.
    Conflict { path: PathBuf, file: Option<Arc<ConflictFile>>, section: usize },
    /// A read is in flight and we have nothing cached yet.
    Loading,
    /// The last read failed.
    Error(String),
}

/// Cursor inside a diff, for staging and patch building.
#[derive(Clone, Debug, Default)]
pub struct PatchCursor {
    /// Selected hunk.
    pub hunk: usize,
    /// `None` = whole-hunk mode; `Some(line)` = line mode (lazygit's `a`).
    pub line: Option<usize>,
    /// Anchor for range select (lazygit's `v`).
    pub anchor: Option<usize>,
}

/// One entry in the overlay stack. The stack is LIFO; `<esc>` pops one layer.
#[derive(Clone, Debug)]
pub enum OverlayKind {
    Menu(MenuDraft),
    Confirm(ConfirmDraft),
    Prompt(PromptDraft),
    Rebase(RebaseDraft),
    Upstream(UpstreamDraft),
    Help,
    /// lazygit's `/` — swallows the entire keymap while open.
    Search { query: String, panel: PanelId },
    /// lazygit's `<ctrl+s>` filter.
    Filter { query: String, panel: PanelId },
}

/// A mutation we have dispatched and not yet seen a result for.
#[derive(Clone, Debug)]
pub struct PendingOp {
    pub id: u64,
    pub label: String,
    pub epoch: u64,
}

pub struct GitUiState {
    // ---- repository identity ----
    pub root: PathBuf,

    // ---- the read model ----
    /// The last good snapshot. `None` only before the first load completes.
    pub snapshot: Option<Arc<RepoSnapshot>>,
    /// `snapshot.generation` of the newest snapshot we have accepted. Any snapshot arriving
    /// with `generation < epoch` is stale and MUST be dropped.
    pub epoch: u64,
    /// A refresh is in flight.
    pub refreshing: bool,
    /// The last snapshot error, shown in the status bar until the next success.
    pub last_error: Option<String>,

    // ---- layout / focus ----
    pub screen_mode: ScreenMode,
    pub focused: PanelId,
    /// Where focus returns to when the main panel is escaped.
    pub previous_side_panel: PanelId,
    pub tabs: Tabs,
    /// Command log visible (lazygit's `@` menu).
    pub show_command_log: bool,

    // ---- per-panel selection ----
    /// Selections are keyed by identity, not index, so `ListCursor::retain` can follow the
    /// item across a refresh.
    pub sel_files: Option<PathBuf>,
    pub sel_local_branch: Option<String>,
    pub sel_remote: Option<String>,
    pub sel_remote_branch: Option<String>,
    pub sel_tag: Option<String>,
    pub sel_commit: Option<ObjectId>,
    pub sel_reflog: usize,
    pub sel_stash: Option<usize>,
    /// Cursor indices, kept in sync with the identities above.
    pub cursors: Cursors,
    /// Flat vs tree rendering of the files panel (lazygit's backtick key).
    pub files_tree_mode: bool,
    /// Collapsed directories in tree mode.
    pub collapsed_dirs: std::collections::BTreeSet<PathBuf>,

    // ---- main panel ----
    pub main: MainContent,
    /// Diff context lines (lazygit's `}` / `{`); default 3.
    pub diff_context: usize,
    /// Ignore whitespace in diffs (lazygit's `<ctrl+w>`).
    pub ignore_whitespace: bool,

    // ---- overlays ----
    /// LIFO. `<esc>` pops the top; only the top layer receives keys.
    pub overlays: Vec<OverlayKind>,

    // ---- command log ----
    pub command_log: std::collections::VecDeque<CommandRecord>,
    pub command_log_following: bool,
    pub command_log_top: usize,

    // ---- in-flight work and feedback ----
    pub pending: Vec<PendingOp>,
    pub next_op_id: u64,
    pub toasts: Vec<fleet_ui_kit::Toast>,
    /// Copied (cherry-picked) commits — lazygit's `C` / `V` clipboard.
    pub cherry_picked: Vec<ObjectId>,
}
```

Notes on the shape:

- **`Arc<RepoSnapshot>`, not a snapshot per panel.** Every panel reads the same `Arc`, so a
  refresh is one pointer swap and no panel can disagree with another about the repo.
- **Selection is stored by identity *and* index.** The identity (`PathBuf`, `String`,
  `ObjectId`) is the source of truth; the index exists only to feed `ListView::cursor(..)` and
  `ListCursor::scroll_target`. After every snapshot swap, recompute the index from the identity
  via `ListCursor::retain(new_len, |_| new_vec.iter().position(|x| x.id == wanted))`.
- **`epoch` is `RepoSnapshot::generation`**, not a counter you maintain. Compare, do not
  increment.
- **`overlays` is a stack, not an `Option`.** lazygit genuinely nests: a menu can open a
  confirmation which can open a prompt. `<esc>` pops one layer
  (`Universal.Return`, `pkg/config/user_config.go:1000`).
- **`MainContent` caches the previous diff while the next one loads.** Never blank the main
  panel on a refresh — that is what makes lazygit feel instant. `MainContent::Loading` is for
  the cold start only, matching `ListView::loading`'s contract
  (`crates/fleet-ui-kit/src/components/list_view.rs:331`-`:334`).

### f.3 Focus and key-context chain

One context per focusable surface, nested with `>` exactly like `fleet-app`'s
`"Fleet > Hub"` convention (§b):

```
Lazygit                          // global: q, R, P, p, +, _, @, ?, :, m, z, Z, 1-5, 0
Lazygit > Status
Lazygit > Files                  // <space>, c, d, a, A, s, S, i, e, o, backtick, -, =
Lazygit > Branches               // <space>, n, d, r, M, f, R, u, T, g, F, c, -
Lazygit > Branches > Remotes
Lazygit > Branches > Tags
Lazygit > Commits                // s, f, r, d, e, i, p, F, S, A, t, T, C, V, B, g, <ctrl+j/k>
Lazygit > Commits > Reflog
Lazygit > Commits > SubCommits
Lazygit > Stash                  // <space>, g, d, n, r
Lazygit > CommandLog             // j, k, f, G
Lazygit > Main                   // <esc>, <tab>, /, K, J
Lazygit > Main > Staging         // <space>, d, a, v, E, c, w, C, h, l, <tab>
Lazygit > Main > Patch           // <space>, d, a, v, h, l, <ctrl+o>
Lazygit > Main > Merging         // <space>, b, h, l, j, k, z, e, o, M
Lazygit > Dialog                 // <esc> only
Lazygit > Dialog > Confirm       // <enter>, <esc>, <ctrl+o>
Lazygit > Dialog > Prompt        // <enter>, <esc>, printable chars
Lazygit > Dialog > Menu          // <enter>, <esc>, /, j/k
Lazygit > Dialog > Rebase
Lazygit > Dialog > Upstream
Lazygit > Search                 // <enter>, <esc>, <up>, <down> — and NOTHING else
```

Rules:

1. **Exactly one focus handle per focusable surface**, stored on the `Render` view (or on
   `Root` in a map keyed by `PanelId` if you keep the panels as `RenderOnce` functions — which
   is the simpler design and the one `fleet-app` uses).
2. **`track_focus` goes on the same element as `key_context`**, and that element must be an
   ancestor of everything the context's keys should reach.
3. **`Lazygit > Search` is special.** While a search/filter prompt is open it must be the only
   active context — this is lazygit's explicit design
   (`pkg/gui/keybindings.go:331`-`:342`, comment at `:332`). In gpui, achieve it by rendering
   the search overlay with its own focus handle and *not* rendering the panel key contexts as
   ancestors of the focused element, or by gating your global handlers on
   `state.overlays.last()`.
4. **Overlays get their own focus handle each** and focus is moved on push
   (`window.focus(&handle)`); on pop, focus returns to `state.focused`'s handle. Store the
   handles on the root view so a push/pop is a plain state change plus one focus call.

### f.4 Refresh strategy

lazygit refreshes constantly and cheaply; copy that, with these rules:

1. **Refresh after every mutation, including failed ones.** A failed `git` command can still
   have changed the index (a partially applied patch, a merge that left conflicts). The
   handler is: dispatch mutation → on result, push `records` into the command log, show
   `warning`/error as a toast, **then** always request a snapshot.
2. **The fs watcher is the primary trigger.** `RepoWatcher::new(paths)`
   (`crates/fleet-git/src/watch.rs:20`) already debounces and reports the debounce window in
   `ChangeEvent::debounce` (`crates/fleet-git/src/model.rs:679`-`:686`). One `ChangeEvent`
   → one snapshot request. Do not add a second debounce layer.
3. **Periodic fallback**: 2 s while the window is focused, 5 s while it is not. This exists
   only to cover watcher gaps (network filesystems, missed events); it must be a no-op when a
   refresh is already in flight (`state.refreshing`).
4. **Epoch check on arrival.** Drop any snapshot whose `generation` is not greater than
   `state.epoch`. Snapshots can complete out of order.
5. **Never fetch on a timer.** lazygit is explicit that `R` "does not run `git fetch`"
   (`docs/keybindings/Keybindings_en.md:22`). Network operations happen only on `f`, `p`, `P`
   — an explicit key. This is a hard rule: a background fetch will prompt for credentials at
   random moments.
6. **Refreshing never moves the cursor, the scroll or the focus.** Use `ListCursor::retain`
   and `ScrollStrategy::Nearest` (both described in §c) and re-derive indices from the stored
   identities.
7. **Main-panel content is refreshed lazily**: when the snapshot swaps, re-request only the
   diff for the *currently selected* item, and keep showing the old one until the new one
   arrives.

### f.5 gpui v1.18.1 gotchas specific to this repo

These are the ones that will actually cost you a day each.

1. **Import the prelude.** `use fleet_ui_kit::prelude::*;` re-exports `gpui::prelude::*`
   (`crates/fleet-ui-kit/src/lib.rs:61`-`:69`). Without it `Styled`, `ParentElement`,
   `InteractiveElement` and `StatefulInteractiveElement` are not in scope and every builder
   method on `div()` "does not exist".
2. **One `AssetSource` per `Application`.** The crate doc says it outright
   (`crates/fleet-ui-kit/src/lib.rs:12`-`:18`): call
   `.with_assets(fleet_ui_kit::KitAssets)` exactly once, or every `Icon` renders as nothing —
   silently, with no error.
3. **The theme is a gpui `Global` and must outlive every render.** Install it once at startup,
   before the first window opens, and read it inside `render` via `cx.theme()` (the
   `ActiveTheme` extension trait, re-exported at `crates/fleet-ui-kit/src/lib.rs:56`). Never
   cache a `&Theme` across an `await` or into a closure that outlives the render.
4. **Monospace is not guaranteed.** Use the theme's mono font token (`theme.font_mono`) and the
   `Text::data` / `Text::data_small` roles rather than naming a family. If the family is
   missing, gpui falls back to a proportional face and every column in your diff, graph and
   sha display will drift. For column widths use the kit's `ch()` helper and
   `RowColumn::fixed_ch` (`crates/fleet-ui-kit/src/components/row.rs:58`) — never a hardcoded
   pixel width computed from an assumed advance.
5. **A `Text` shapes as a single run.** You cannot color parts of one `Text`. Multi-colored
   output — a diff line, a commit row with a colored graph column, a branch name with colored
   ahead/behind counts — must be built as **sibling elements in a flex row**, one per color.
   Budget for this in `views/diff.rs` and `views/commit_graph.rs`: a diff line is
   `div().flex().child(gutter).child(marker).child(payload)`, not one string.
6. **`uniform_list` assumes uniform row heights.** It measures the first rendered row and
   reuses that height for scroll math. Every row a list produces must be the same height. A
   diff is therefore fine (all lines equal height), but do **not** mix a 30 px file row with a
   44 px two-line row in the same list — and `Row::second_line` silently switches a row to
   44 px (`crates/fleet-ui-kit/src/components/row.rs:184`, height logic at `:235`-`:240`), so
   either all rows in a list use it or none do.
7. **`track_focus` placement.** It must be on the same element that carries the matching
   `key_context`, and that element must be an ancestor of the content the keys act on. Putting
   `key_context` on a parent and `track_focus` on a child (or vice versa) yields bindings that
   silently never fire.
8. **Prefer action bindings over `on_key_down`.** Declarative bindings are what the help
   dialog and the key hints are generated from (§b), they respect context nesting, and they
   are what the keymap contract in `docs/APP-CONTRACTS.md` is written against. Reach for
   `on_key_down` **only** for printable-character text entry, and when you do, ignore any
   keystroke carrying `control`/`platform`/`alt` so chords still reach the app — exactly what
   `LogView::handle_key` does (`crates/fleet-ui-kit/src/components/log_view.rs:214`-`:245`).
9. **`TextField` vs `TextInput` for real editing.** `TextField` is the presentational
   `RenderOnce` component; `TextInput` is the stateful path with its own focus handle and
   `TEXT_FIELD_KEY_CONTEXT`, both re-exported at
   `crates/fleet-ui-kit/src/components/mod.rs:131`-`:133`. **Use `TextInput` for anything the
   user actually types into** — the commit-message panel, branch/tag name prompts, the search
   and filter prompts. `TextField` alone will look right and do nothing.
10. **`RenderOnce` components own no state.** Everything in the kit that you will use for
    panels and rows is `RenderOnce` (§c.3): it is constructed fresh on every render and
    dropped immediately. Cursor indices, scroll handles, follow flags and drafts must live in
    `GitUiState`. In particular a `UniformListScrollHandle` must be a field on your view — one
    per list — created once and passed by reference (`ListView::track_scroll(&handle)`).
11. **Never block the foreground thread with git.** Every `Repository` method is `async` and
    shells out to a child process. All of it runs on the bridge thread (§d); the UI thread only
    ever sends a request and applies a reducer. A single synchronous `git log` on a large repo
    is a visible multi-hundred-millisecond freeze. Corollary: do not call
    `futures::executor::block_on` anywhere in a render or an action handler.
12. **Scroll only from action handlers, never from `render`.** `ListView` deliberately exposes
    scrolling as the associated function `ListView::reveal`
    (`crates/fleet-ui-kit/src/components/list_view.rs:352`) so it is called from a handler. The
    single sanctioned exception in the whole kit is `LogView`'s follow mode
    (`crates/fleet-ui-kit/src/components/log_view.rs:155`-`:157`) — mirror that only for the
    command-log panel.
13. **`ScreenMode` clamps, tabs wrap.** Match lazygit: `+`/`_` clamp at the ends,
    `]`/`[` wrap (`SegmentedTabs::next_index`/`prev_index`,
    `crates/fleet-ui-kit/src/components/segmented_tabs.rs:95`/`:103`), list cursors clamp
    (`ListCursor`), fuzzy lists wrap (`FuzzyList::next_cursor`,
    `crates/fleet-ui-kit/src/components/fuzzy_list.rs:231`).

### f.6 Suggested build order

1. `bridge.rs` + `state.rs` with `snapshot` only, and a `root.rs` that renders the five panels
   as plain `Pane` + `ListView` with no keys. Get a snapshot on screen.
2. `keymap.rs` + `actions.rs`: `1`-`5`, `0`, `<tab>`, `j`/`k`, `]`/`[`, `q`, `R`, `+`/`_`.
   Everything else can wait; this is the skeleton the rest hangs off.
3. `panels/main.rs` + `views/diff.rs`: `MainContent::FileDiff` and `CommitDiff`. Now the app is
   useful read-only.
4. Mutations without dialogs: `<space>` stage/unstage, `a` stage-all, `<space>` checkout.
   Wire the refresh-after-mutation rule and the command log at the same time.
5. `dialogs/confirm.rs` + `dialogs/prompt.rs`, then commit (`c`), branch create (`n`), discard
   (`d`).
6. `panels/main.rs` staging mode + `apply_patch_selection` — hunk mode first, line mode second.
7. `dialogs/menu.rs` and the option menus (`m`, `d`, `g`, `u`, `M`, `S`, `D`, `<ctrl+p>`).
8. `dialogs/rebase.rs` and the interactive-rebase operations.
9. `views/conflict.rs` and the merging main-panel mode.
10. `dialogs/` search/filter, then `?` help generated from the binding table.
