# Building a standalone macOS desktop app with GPUI — research report

**Date of research:** 2026-09-04
**Target machine (verified locally, this box):**

| Fact | Value | How verified |
|---|---|---|
| macOS | 26.3 (build 25D2125) | `sw_vers` |
| Arch | arm64 | `uname -m` |
| Xcode | `/Applications/Xcode.app/Contents/Developer` | `xcode-select -p` |
| macOS SDK | 26.5 | `xcrun --sdk macosx --show-sdk-version` |
| Rust installed | 1.94.1 (e408947bf, 2026-03-25), `stable-aarch64-apple-darwin` active | `rustup toolchain list`, `rustc --version` |
| Current Rust stable upstream | **1.98.1 (48a229cea, 2026-09-01)** | `curl https://static.rust-lang.org/dist/channel-rust-stable.toml` |
| Metal compiler | **WAS MISSING**, now installed | see §1.4 |

> Everything marked **[VERIFIED LOCALLY]** was compiled/executed on this machine during this research, not just read about.

---

## 1. Getting gpui

### 1.1 Is `gpui` on crates.io? Yes — but it is ~11 months stale.

`https://crates.io/api/v1/crates/gpui` (fetched 2026-09-04):

| Version | Published | Notes |
|---|---|---|
| **0.2.2** | **2025-10-22** | current `max_version`, 239,989 downloads |
| 0.2.1 | 2025-10-14 | |
| 0.2.0 | 2025-10-09 | |
| 0.1.0 | 2022-06-23 | yanked |

- Total downloads 253,600. `edition = "2024"`. No `rust-version` (MSRV) field declared.
- Repository: <https://github.com/zed-industries/zed>, homepage <https://gpui.rs>, license **Apache-2.0**.
- Sources: <https://crates.io/crates/gpui>, <https://lib.rs/crates/gpui>, <https://docs.rs/gpui>

**Nothing has been published to the official `gpui` crate since 2025-10-22.** Zed has shipped ~30 releases since (v1.18.1 is the current release as of 2026-09-04). So crates.io `gpui` is a snapshot of the framework as of Oct 2025, not "current gpui".

### 1.2 The big 2026 change: gpui was split into many crates

On `main` today, `crates/gpui` is no longer the whole framework. The crate list under `crates/` now contains:

```
gpui              gpui_apple        gpui_linux     gpui_macos
gpui_macros       gpui_platform     gpui_shared_string
gpui_tokio        gpui_util         gpui_web       gpui_wgpu    gpui_windows
```
(`https://api.github.com/repos/zed-industries/zed/contents/crates?ref=main`)

- `gpui` = core (elements, layout, entities, scene). Platform-agnostic.
- `gpui_platform` = the *dispatcher* that picks a backend per OS. **This is now the entry point.**
- `gpui_macos` → `gpui_apple` = the Metal renderer + Cocoa windowing + CoreText.

`crates/gpui_platform/Cargo.toml` (main):
```toml
[features]
default = []
font-kit = ["gpui_macos/font-kit"]
runtime_shaders = ["gpui_macos/runtime_shaders"]
wayland = ["gpui_linux/wayland"]
x11    = ["gpui_linux/x11"]
screen-capture = [...]

[target.'cfg(target_os = "macos")'.dependencies]
gpui_macos.workspace = true
```
Source: <https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml>

**Consequence:** the current `crates/gpui/README.md` on `main` tells you to write

```toml
gpui = { version = "*" }
gpui_platform = { version = "*", features = ["font-kit", "wayland", "x11"] }
```

…but **`gpui_platform` IS NOT PUBLISHED ON CRATES.IO** (verified: `https://crates.io/api/v1/crates/gpui_platform` → 404/errors). Neither are `gpui_shared_string`, `gpui_tokio`, `gpui_macos`, `gpui_apple`. Only `gpui` 0.2.2 and `gpui_macros` 0.2.2 exist there. **The README on `main` documents an API you cannot get from crates.io.** This is the single biggest trap for an agent scaffolding this project.

Sources: <https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/README.md>, crates.io API.

### 1.3 The three ways to actually get gpui

| Option | Dependency | API era | Status |
|---|---|---|---|
| **A. crates.io official** | `gpui = "0.2.2"` | Oct 2025 — `Application::new().run(...)`, single crate | **[VERIFIED] builds & runs on Rust 1.94.1**; 70 s, 5.0 MB |
| **B. crates.io mirror** | `gpui-unofficial = "1.18"` | tracks Zed release tags — `gpui_platform::application()` | **[VERIFIED] builds & runs — but needs Rust ≥ 1.97**; 8.5 MB |
| **C. git dep on zed** | `gpui = { git=…, tag="v1.18.1" }` + `gpui_platform` | current `main` | **[VERIFIED] builds & runs on Rust 1.97.1**; 91 s, 8.4 MB, 380 MB git cache |
| **D. community fork** | `gpui-ce` 0.2.2 + `gpui_ce_platform` 0.1.0 | forked from Zed `main`, actively synced | **[VERIFIED] builds & runs on Rust 1.97.1**; 58 s, 9.4 MB; 1,007★, Apache-2.0 — §1.3.4 |
| ~~E. gpui-component / gpui-kit~~ | `gpui-pre` 0.3.x | a *fourth* unofficial republish | **incompatible with A and C** — see §3.2 |

> There are, as of Sept 2026, **four separate republishings of gpui on crates.io** (`gpui` 0.2.2 official,
> `gpui-unofficial`, `gpui-ce`, `gpui-pre`) because Zed stopped publishing `gpui_platform`. Picking one
> is a supply-chain decision, not just a version decision.

#### 1.3.1 Option A — official crates.io `gpui 0.2.2` **[VERIFIED LOCALLY]**

```toml
[dependencies]
gpui = { version = "0.2.2", default-features = false, features = ["font-kit"] }
```

`default-features = false` is required on macOS: the default feature set is
`["font-kit", "wayland", "x11", "windows-manifest"]` and the `wayland`/`x11` features pull
in `cosmic-text`, `xkbcommon`, `wayland-client`, `blade-graphics`, `x11rb`, … which you do not
want on a Mac. (Feature list read from the downloaded `gpui-0.2.2.crate`.)

Measured on this machine, Rust 1.94.1, cold `~/.cargo` for these crates:

- `cargo build --release` from scratch: **1 m 10 s wall** (419 s user, ~6× parallelism)
- Resulting binary: **5,225,200 bytes ≈ 5.0 MB** (unstripped, default release profile)
- 606 package entries in `Cargo.lock`
- The unbundled binary **launches and stays alive** with a live NSApplication run loop (~57 MB RSS after 7 s). No `.app` bundle needed to run it.

This is the *smallest, fastest, most reproducible* way to get a working GPUI app today.

#### 1.3.2 Option B — `gpui-unofficial` (automated crates.io mirror of Zed `main`)

- Crate: <https://crates.io/crates/gpui-unofficial> — repo <https://github.com/iamnbutler/gpui-unofficial> (Nate Butler, a Zed employee — but the repo says **"This is an unofficial project not affiliated with Zed Industries."**)
- Versions mirror Zed's release tags verbatim: Zed `v1.18.0` → `gpui-unofficial 1.18.0`.
- Latest: **1.19.0-pre (2026-09-02)**, **1.18.0 (2026-09-02)**, 1.17.2 (2026-08-26). 4,261 downloads total.
- It publishes the *whole* split family, renamed:
  `gpui-unofficial`, `gpui-platform-gpui-unofficial`, `gpui-macos-gpui-unofficial`, `gpui-apple-*`, `gpui-linux-*`, `gpui-windows-*`, `gpui-web-*`, `gpui-wgpu-*`, `gpui-macros-*`, `collections-*`, `scheduler-*`, `refineable-*`, `sum-tree-*`, `http-client-*`, `zlog-*`, `ztracing-*`, `util-*`, `perf-*`, `media-*`, `reqwest-client-*`, …
- Mechanism: "GitHub Actions checks for new Zed releases every 6 hours, transforms the crates (renaming packages, updating dependencies), opens a PR, and on merge publishes to crates.io." (`cargo xtask transform --zed-tag v0.230.1`)
- The lib target is named `gpui`, so `use gpui::*;` works if you rename the dependency:
  ```toml
  gpui = { package = "gpui-unofficial", version = "1.18" }
  gpui_platform = { package = "gpui-platform-gpui-unofficial", version = "1.18", features = ["font-kit"] }
  ```
- Known limitation stated in its README: because versions are taken verbatim from Zed's semver, **there is no way to publish a fix for an already-released version**.

**[VERIFIED LOCALLY]** This exact `Cargo.toml` was built on this machine:
```toml
gpui          = { package = "gpui-unofficial",               version = "1.18", default-features = false }
gpui_platform = { package = "gpui-platform-gpui-unofficial", version = "1.18", features = ["font-kit"] }
```
- **On Rust 1.94.1 it FAILS to compile**: `error[E0658]: use of unstable library feature 'cold_path'`
  in `gpui-unofficial`'s scheduler (`std::hint::cold_path()`, rust-lang/rust#136873), 2 errors.
- **On Rust 1.97.1 it compiles and runs.** Wall time 52.5 s (partially warm cargo cache), 427 s user.
  Binary **8,468,736 bytes ≈ 8.5 MB**; the unbundled binary launches and stays alive.
- The `gpui_apple/build.rs` sibling-dir concern (`CARGO_MANIFEST_DIR/../gpui`) is handled by the
  transform pipeline — `gpui-apple-gpui-unofficial` built fine straight from crates.io.

This is a concrete, reproducible datum for the toolchain question: **current-era gpui needs Rust ≥ 1.97; the Oct-2025 crates.io gpui 0.2.2 does not.**

Risk: a third-party automated republish of a fast-moving pre-1.0 framework. It is the only way to get *current* gpui from crates.io, but you are trusting an unofficial pipeline. Also note `crates/gpui_apple/build.rs` locates the gpui source with `CARGO_MANIFEST_DIR/../gpui` — a sibling-directory assumption that only holds inside the Zed workspace, so the transform pipeline has to patch it. That's a structural fragility worth knowing about.

#### 1.3.4 Option D — `gpui-ce`, the community fork

This is the most significant ecosystem development of 2026 and the reason the other republishes exist.

**Why it exists.** GPUI development at Zed was explicitly throttled. From a Zed maintainer on Discord,
quoted verbatim in [HN comment 47003569](https://news.ycombinator.com/item?id=47003569) (2026-02-13):

> *"Hey y'all, GPUI development is getting some major brakes put on it. We gotta focus on some business
> relevant work in 2026, and so I'm going to be pushing off anything that isn't directly related to
> Zed's use case from now on. However, Nate, former employee #1 at Zed, has started a little side repo
> that people can keep iterating on…"*

[zed-industries/zed discussion #30515](https://github.com/zed-industries/zed/discussions/30515) still has
no official Zed response.

**Current state** (`gh api repos/gpui-ce/gpui-ce`, 2026-09-04):

| | |
|---|---|
| Repo | <https://github.com/gpui-ce/gpui-ce> ("GPUI – Community Edition") |
| Created | 2025-12-12 |
| Last push | **2026-09-04** (today) |
| Stars / forks | **1,007** / 113 |
| Open issues | 20 |
| License | **Apache-2.0** |
| Homepage | <https://gpui-ce.github.io/> |
| crates.io publisher | `philocalyst` |

Recent commits show it **actively tracks upstream**:
```
2026-09-04  Philocalyst        ci fixes and cleanup
2026-09-03  William Whittaker  Sync/zed 20260903 b1a7ef0 (#219)   <- synced to zed commit b1a7ef0
2026-09-02  Cameron Campbell   Better padding snapping for automatically sized axis. (#217)
2026-09-02  Cameron Campbell   ring apis (#216)
```
Its README states the intent plainly: *"For now, it is mostly API compatible, but this is changing!"* —
so expect divergence from Zed's gpui over time. Its own quickstart already uses the `main`-era entry
point: *"You can create one with `gpui_platform::application()`"*.

**Crate naming is inconsistent — this WILL trip an agent.** The root crate uses a hyphen; every other
crate in the family uses underscores:
```
gpui-ce                 0.2.2  (2026-08-28)   <- hyphen
gpui_ce_platform        0.1.0  (2026-08-29)   <- underscores
gpui_ce_macros          0.1.0    gpui_ce_util            0.2.6
gpui_ce_collections     0.2.2    gpui_ce_scheduler       0.2.2
gpui_ce_refineable      0.2.2    gpui_ce_derive_refineable 0.2.2
gpui_ce_shared_string   0.1.0    gpui_ce_sum_tree        0.2.2
gpui_ce_media           0.2.2    gpui_ce_path            0.2.2
```
`cargo add gpui-ce-platform` fails with *"no matching package… packages with similar names: gpui_ce_platform"*.
Also note versions **0.3.2 / 0.3.3 (2025-12-27) are YANKED** and the live version went *back* to 0.2.2 —
the version numbers do not tell a coherent story.

`gpui_ce_platform` 0.1.0 features:
`default = []`, `font-kit`, `runtime_shaders`, `wayland`, `x11`, `wgpu`, `screen-capture`, `test-support`.

**It also forks the component library**: `gpui_ce_components` 0.2.0 (2026-08-29,
<https://github.com/gpui-ce/gpui-component>, Apache-2.0) depends on `gpui-ce ^0.2.2` — so unlike
`gpui-component`/`gpui-pre` (§3.2) it is *compatible* with the same gpui copy you'd be using. But it has
**37 downloads**; treat it as experimental.

**Caveat on betting on the fork.** The founder himself, same HN thread,
[reply 47005761](https://news.ycombinator.com/item?id=47005761) (2026-02-13):
> *"I started the gpui-ce fork but I'm becoming somewhat more interested in a fresh framework that is
> more aligned with the rust ecosystem in general…"*

Adoption: 7,793 downloads for `gpui-ce`, 401 for `gpui_ce_platform` — i.e. most of those downloads are
not people actually running apps.

**[VERIFIED LOCALLY]** — this builds and runs, with the **exact same `src/main.rs` as the
`gpui-unofficial` and git-dep tests** (i.e. it really is `main`-era API compatible):
```toml
[dependencies]
gpui          = { package = "gpui-ce",          version = "0.2.2", default-features = false }
gpui_platform = { package = "gpui_ce_platform", version = "0.1.0", features = ["font-kit"] }
```
Rust 1.97.1, clean release build **58.2 s** (483 s user), binary **9,400,704 B ≈ 9.4 MB**, launches and
stays alive. Note the hyphen/underscore asymmetry in the two package names — it is not a typo.

#### 1.3.3 Option C — git dependency on the Zed repo

```toml
gpui          = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", default-features = false }
gpui_platform = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", features = ["font-kit"] }
```

Stable rev to pin: **tag `v1.18.1`** (commit `bebe92f46983`, released 2026-09-04) — the current *stable* Zed release. Do **not** track `main` or a `-pre` tag.

Recent tags (`https://api.github.com/repos/zed-industries/zed/tags`):
`v1.19.1-pre` (2d15631d1b42), `v1.19.0-pre` (f69e805e36a2), **`v1.18.1` (bebe92f46983)**, `v1.18.0` (49448afcab82), `v1.17.2` (c8e44cfa7bda), `v1.16.3` (03e2e21e3094).

Cost: cargo clones the entire Zed monorepo (very large) into `~/.cargo/git`. Benefit: exact, auditable, official source; you can also reach `crates/ui`, `crates/theme`, `crates/terminal` from the same checkout for reference.

**[VERIFIED LOCALLY]** built on Rust 1.97.1 with
`gpui_platform = { git = …, tag = "v1.18.1", features = ["font-kit", "runtime_shaders"] }`:
- **1 m 30 s** wall (422 s user), binary **8,373,280 bytes ≈ 8.4 MB**, launches and stays alive.
- `~/.cargo/git/db/zed-a70e2ad075855582` = **380 MB** after the clone (one-time, shared across projects).
- Cargo resolved `gpui_apple`, `gpui_macos`, `gpui_platform` v0.1.0 from `zed?tag=v1.18.1#bebe92f4`.
- This run also **validates the `runtime_shaders` escape hatch** (§1.5): the app built and ran without
  invoking `xcrun metal` at build time.

### 1.4 Rust toolchain required

- **crates.io `gpui 0.2.2` compiled fine on the installed Rust 1.94.1.** [VERIFIED LOCALLY] It declares `edition = "2024"` and no `rust-version`.
- **Zed `main` pins `channel = "1.97.1"`** in `rust-toolchain.toml`:
  ```toml
  [toolchain]
  channel = "1.97.1"
  profile = "minimal"
  components = ["rustfmt", "clippy", "rust-analyzer", "rust-src"]
  targets = ["wasm32-wasip2", "wasm32-unknown-unknown", "x86_64-unknown-linux-musl"]
  ```
  Source: <https://raw.githubusercontent.com/zed-industries/zed/main/rust-toolchain.toml>
- **`main`-era gpui genuinely requires ≥ 1.97** — proven by the failed 1.94.1 build in §1.3.2 (`std::hint::cold_path` is unstable before 1.97).
- **No nightly is needed.** GPUI is stable-only. The README says "You'll also need to use the latest version of stable Rust."
- Current upstream stable is **1.98.1 (2026-09-01)**; this box's `stable` is 1.94.1.
- **[DONE ON THIS BOX]** `rustup toolchain install 1.97.1 --profile minimal` → `1.97.1 (8bab26f4f 2026-07-14)` is now installed alongside. Use `cargo +1.97.1 …`, or better, drop a `rust-toolchain.toml` in the project root (see §DECISIONS). `rustup` also self-updated 1.29.0 → 1.29.1.

### 1.5 macOS system dependencies — the Metal Toolchain trap **[VERIFIED LOCALLY]**

GPUI's macOS backend compiles its Metal shaders **at build time** in a build script.
`crates/gpui_apple/build.rs` on main (and `crates/gpui/build.rs` in 0.2.2):

```rust
let output = Command::new("xcrun")
    .args(["-sdk", "macosx", "metal",
           "-gline-tables-only", "-mmacosx-version-min=10.15.7", "-MO", "-c"])
    .arg(&staged_shader_path)
    .args(["-include", header_path.to_str().unwrap(), "-o"])
    .arg(&air_output_path)
    .output().unwrap();
// then: xcrun -sdk macosx metallib <air> -o shaders.metallib
```
Source: <https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_apple/build.rs>

**On macOS 26 / Xcode 26 the Metal compiler is NOT installed with Xcode by default.** On this machine, before doing anything:

```
$ xcrun -sdk macosx metal --version
error: cannot execute tool 'metal' due to missing Metal Toolchain;
use: xcodebuild -downloadComponent MetalToolchain
```

Fix (run once, **no sudo required**):

```sh
xcodebuild -downloadComponent MetalToolchain
```

Measured here: **687.9 MB** download, ~2 minutes, exit 0. Afterwards:

```
$ xcrun -sdk macosx metal --version
Apple metal version 32023.883 (metalfe-32023.883)
Target: air64-apple-darwin25.3.0
```
(installed as `Metal Toolchain 17F109`, asset `022-21788-058.dmg`)

**Escape hatch if you cannot install it:** enable the `runtime_shaders` feature. `build.rs` then stitches the generated `scene.h` header onto `shaders.metal` and ships the source, compiling it with the Metal *runtime* compiler (part of `Metal.framework`, always present) at app startup instead:

```toml
# 0.2.2
gpui = { version = "0.2.2", default-features = false, features = ["font-kit", "runtime_shaders"] }
# main / git
gpui_platform = { git = "...", features = ["font-kit", "runtime_shaders"] }
```
Trade-off: shifts shader compilation to first-launch startup latency. **Measured:** with
`runtime_shaders` the binary is *slightly smaller* (3,003,248 B vs 3,069,344 B — no embedded metallib) and
first launch costs **407 ms**, subsequent launches ~110 ms. By default `build.rs` precompiles a 133,078-byte
`shaders.metallib` into `OUT_DIR` and embeds it, so **there is no first-launch shader stall** — which is why
installing the toolchain is the better default.

Verify the component with `xcodebuild -showComponent metalToolchain` (prints `Status: installed`). It lives
in a cryptex mount, **not** inside `Xcode.app`, and is unreachable from `CommandLineTools` — so
`sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer` matters. Corroboration that this is a
known Xcode 26 requirement: [Apple forums 792841](https://developer.apple.com/forums/thread/792841),
[actions/runner-images#13014](https://github.com/actions/runner-images/issues/13014), and the historical
GPUI-specific report [create-gpui-app#23](https://github.com/zed-industries/create-gpui-app/issues/23).
The `runtime_shaders` feature originates from
[zed discussion #7016](https://github.com/zed-industries/zed/discussions/7016) + PR #7148.

Other macOS requirements from the README: Xcode installed with macOS components, Xcode CLT (`xcode-select --install`), and `sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer`. All already satisfied on this box.

### 1.6 Required cargo features (macOS)

- **`font-kit` is effectively mandatory on macOS.** From the `main` README: *"On macOS — Rendering uses Metal and is always available, but glyph rasterization needs `font-kit`. Without it, GPUI falls back to a placeholder text system that lays text out but renders no glyphs."* → **no `font-kit` = laid-out but invisible text.**
- **Turn off `wayland` / `x11` / `windows-manifest`.** In 0.2.2 they are in `default`, so use `default-features = false, features = ["font-kit"]`.
- Optional: `runtime_shaders` (see above), `screen-capture`, `inspector` (a GPUI element inspector), `test-support` (dev-dependency for `#[gpui::test]`).

### 1.7 Licensing note (matters for a product)

- `gpui` itself: **Apache-2.0**.
- Zed's `crates/ui`: **`license = "GPL-3.0-or-later"`** (`https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/Cargo.toml`) and `publish = false`. **You cannot vendor Zed's `ui` crate into a closed-source app.** Read it for ideas only.

---

## 2. Minimal app skeleton

### 2.1 API shapes: what changed, and when

Agents WILL find stale tutorials. This is the rename table; anything using the left column is from before ~Jan 2025 and will not compile.

| Old (pre-2025) | Current | Notes |
|---|---|---|
| `AppContext` | **`App`** | the `App` type; `AppContext` is now a *trait* |
| `WindowContext` | **`&mut Window, &mut App`** (two params) | window context was split out entirely |
| `ViewContext<T>` | **`Context<T>`** | |
| `View<T>` / `Model<T>` | **`Entity<T>`** | unified |
| `WeakView<T>` / `WeakModel<T>` | **`WeakEntity<T>`** | |
| `cx.new_view(...)` / `cx.new_model(...)` | **`cx.new(\|cx\| …)`** | |
| `fn render(&mut self, cx: &mut ViewContext<Self>)` | **`fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement`** | the `window` param is required |
| `cx.focus(&handle)` | **`window.focus(&handle, cx)`** | takes `cx` on main |
| `overlay()` | `anchored()` / `deferred()` | |
| `Application::new().run(...)` | **`gpui_platform::application().run(...)`** | **only on `main`/git; NOT in crates.io 0.2.2** |
| `AsyncApp::update(...) -> Result<R>` | **`-> R`** on `main` | 0.2.2 still returns `Result<R>` — see §2.4 |

### 2.2 Skeleton — Option A (crates.io `gpui 0.2.2`) **[VERIFIED: this exact code compiles and runs on this machine]**

`Cargo.toml`:
```toml
[package]
name = "myapp"
version = "0.1.0"
edition = "2024"

[dependencies]
gpui = { version = "0.2.2", default-features = false, features = ["font-kit"] }

# Cuts debug-build pain massively; see §7.
[profile.dev.package."*"]
opt-level = 2
```

`src/main.rs` (adapted from the crate's own `examples/hello_world.rs`, gpui-0.2.2.crate):
```rust
use gpui::{
    App, Application, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, size,
};

struct HelloWorld {
    text: SharedString,
}

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size(px(500.0))
            .justify_center()
            .items_center()
            .shadow_lg()
            .border_1()
            .border_color(rgb(0x0000ff))
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(format!("Hello, {}!", &self.text))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| HelloWorld { text: "World".into() }),
        )
        .unwrap();
        cx.activate(true);   // <- required, see §7
    });
}
```

### 2.3 Skeleton — Option B/C (`main`-era API)

`Cargo.toml` (git form):
```toml
[dependencies]
gpui          = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", default-features = false }
gpui_platform = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", features = ["font-kit"] }
```
or (crates.io mirror form):
```toml
gpui          = { package = "gpui-unofficial",                 version = "1.18", default-features = false }
gpui_platform = { package = "gpui-platform-gpui-unofficial",   version = "1.18", features = ["font-kit"] }
```

`src/main.rs` — verbatim shape from `crates/gpui/examples/hello_world.rs` on `main`
(<https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/hello_world.rs>):
```rust
use gpui::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, size,
};
use gpui_platform::application;          // <-- the only structural difference

struct HelloWorld { text: SharedString }

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().bg(rgb(0x505050)).size(px(500.0))
            .justify_center().items_center()
            .text_color(rgb(0xffffff))
            .child(format!("Hello, {}!", self.text))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| HelloWorld { text: "World".into() }),
        ).unwrap();
        cx.activate(true);
    });
}
```

> The `main` examples also call `example_support::load_fonts(cx)` and `return` if it fails.
> **On macOS that function is a no-op** — it only embeds IBM Plex Sans / Lilex for the
> `wasm` target. `#[cfg(not(target_family = "wasm"))] pub fn load_fonts(_cx: &App) -> bool { true }`.
> Source: <https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/example_support/fonts.rs>
> So on macOS you get system fonts through CoreText/font-kit and do **not** need to embed a font
> to see text — as long as the `font-kit` feature is on.

### 2.4 `WindowOptions` — the fields you'll actually set

From `crates/gpui/src/platform.rs`:
```rust
pub struct WindowOptions {
    pub window_bounds: Option<WindowBounds>,
    pub titlebar: Option<TitlebarOptions>,
    pub focus: bool,
    pub show: bool,
    pub kind: WindowKind,               // Normal | PopUp | ...
    pub is_movable: bool,
    pub is_resizable: bool,
    pub is_minimizable: bool,
    pub display_id: Option<DisplayId>,
    pub window_background: WindowBackgroundAppearance,
    pub app_id: Option<String>,
    pub window_min_size: Option<Size<Pixels>>,
    pub window_decorations: Option<WindowDecorations>,  // Wayland only
    pub tabbing_identifier: Option<String>,             // macOS native tabs
}

pub struct TitlebarOptions {
    pub title: Option<SharedString>,
    /// Hide the system titlebar so you can draw your own (macOS + Windows)
    pub appears_transparent: bool,
    /// Where to put the macOS traffic-light buttons
    pub traffic_light_position: Option<Point<Pixels>>,
}
```
For a custom-chrome app (which is what you want for a bespoke design system), set
`titlebar: Some(TitlebarOptions { appears_transparent: true, traffic_light_position: Some(point(px(9.), px(9.))), ..Default::default() })`
and draw your own title bar with `div()`.


---

## 3. Component library: `gpui-component` / GPUI Kit, and Zed's `ui`

### 3.1 The project was renamed: it is now **GPUI Kit**

- `github.com/longbridge/gpui-component` now redirects to **<https://github.com/longbridge/gpui-kit>**.
- Docs site: **<https://gpui-kit.com>** (the old <https://longbridge.github.io/gpui-component/> serves the same site).
- crates.io `gpui-component` **0.6.0**, published **2026-09-03**, 103,202 total downloads. Also `gpui-kit` 0.6.0 (2026-09-03).
- Version history: 0.6.0 (2026-09-03) ← 0.5.1 (2026-02-05) ← 0.5.0 (2025-12-08) ← 0.4.x (2025-11).

Architecture (README):
```
gpui-kit             The one crate applications depend on
├── gpui-base        Unstyled behavior, state, and infrastructure   (crates.io: gpui-base 0.6.0)
├── gpui-shell       JavaScript/QuickJS extensions for a Rust host   (publish = false in 0.6)
└── gpui-component   GPUI Component: the complete styled UI system  (crates.io: gpui-component 0.6.0)
```
Their own analogy: GPUI ↔ HTML+Tailwind, `gpui-base` ↔ Base UI (headless), `gpui-component` ↔ shadcn's styled layer.
Stated principle: *"Behavior belongs to the foundation. Presentation belongs to the application."*

Other published crates: `gpui-component-macros` 0.6.0, `gpui-kit-assets` 0.6.0 (Lucide icons + `AssetSource`), `gpui-wry` 0.6.0 (WebView via `wry`/`lb-wry` 0.53.3).

### 3.2 ⚠️ Critical compatibility finding: it does NOT use crates.io `gpui`

`gpui-component` 0.6.0's published dependencies (`https://crates.io/api/v1/crates/gpui-component/0.6.0/dependencies`):
```
gpui-base ^0.6.0   gpui-component-macros ^0.6.0   gpui-kit-assets ^0.6.0
gpui-pre ^0.3.1    gpui-pre-macros ^0.3.1         gpui-pre-sum-tree ^0.3.1
```

**`gpui-pre` is a *third* unofficial republish of Zed's GPUI**, produced by Longbridge's own
`script/bump-gpui.ts` (<https://raw.githubusercontent.com/longbridge/gpui-kit/main/script/bump-gpui.ts>).
Its doc comment says it plainly:

> Publish a snapshot of Zed's GPUI crates to crates.io as `gpui-pre-*`. The `gpui`, `gpui_platform`
> and `gpui_macros` names on crates.io belong to Zed, and Zed only publishes them occasionally.
> This script lets GPUI Kit publish its own pre-release builds straight from any Zed commit…
> keep the original crate name as the `[lib]` name so `use gpui::*` keeps working.

- Owner of all `gpui-pre-*` crates: crates.io user **`huacnlee`** (Jason Lee) — the same person who owns `gpui-component`. Not Zed.
- `gpui-pre 0.3.1` = zed commit **`801c087`** (2026-09-03), which is **diverged from tag `v1.18.1`: 123 ahead, 15 behind**. `gpui-pre 0.3.3` = zed `5b055fa`.
- Version scheme is `0.3.<N>` from a hardcoded `const VERSION = "0.3";` — **no semver relation to Zed releases**.

**Verdict: `gpui-component` 0.6.0 is INCOMPATIBLE with both `gpui = "0.2.2"` (crates.io) and a git dep on `zed-industries/zed`.** Cargo would compile two unrelated packages (`gpui-pre` and `gpui`); every type (`App`, `Window`, `Div`, `Entity<T>`, `Hsla`…) would exist twice with no conversion. `[patch.crates-io] gpui-pre = { git = zed }` cannot fix it, because Zed's repo contains no package literally named `gpui-pre` and `package =` renames are not allowed in a patch entry.

Supported usage is all-in on their stack:
```toml
gpui-kit = "0.6"          # facade: `use gpui_kit::*` IS gpui (`pub use ::gpui::*`)
```
or
```toml
gpui-component  = "0.6"
gpui-base       = "0.6"
gpui-kit-assets = "0.6"
gpui           = { package = "gpui-pre",          version = "0.3" }
gpui_platform  = { package = "gpui-pre-platform", version = "0.3", features = ["font-kit", "x11", "wayland", "runtime_shaders"] }
```
Their `Root` element is mandatory as the first level of the window (it hosts dialogs, notifications, popovers, menus), and `gpui_kit::init(cx)` must be the first line of the `run` closure.

MSRV stated in <https://raw.githubusercontent.com/longbridge/gpui-kit/main/website/docs/installation.md>: **Rust 1.90+**, macOS 15+. No `rust-toolchain.toml`; `edition = "2024"`.

### 3.3 What it provides (for reference value)

`crates/component/src/lib.rs` public modules:
`accordion, alert, attachment, avatar, badge, breadcrumb, bubble, button, chart, checkbox, clipboard, collapsible, color_picker, combobox, command, description_list, dialog, dock, form, group_box, highlighter, history, hover_card, input, kbd, label, link, list, marker, menu, message, message_scroller, native_menu, notification, pagination, plot, popover, progress, radio, rating, resizable, scroll, searchable_list, select, separator, setting, sheet, shimmer, sidebar, skeleton, slider, spinner, status_bar, stepper, switch, tab, table, tag, text, theme, tooltip, tree` + `Icon/IconName`, `Root`, `TitleBar`, `VirtualList` / `h_virtual_list` / `v_virtual_list`, `WindowBorder`, `IndexPath`, `inspector`, `GlobalState`.

Highlights: **DataTable** (virtual scroll, fixed/resizable columns, sorting, "hundreds of thousands of rows"), **Dock** (tabs/splits/edge docks + serializable freeform Tiles), **Editor** (code editing w/ tree-sitter highlighting, gutter, folding, LSP diagnostics/completion/hover), **Command** palette, **Chart** (line/bar/area/pie/candlestick), **TextView** (Markdown + HTML), **Tree**, **VirtualList** (variable-height), **NativeMenu** (real `NSMenu` via objc2), FocusTrap, i18n via `rust-i18n`.

Feature flags: `decimal`, `inspector`, `tree-sitter`, `tree-sitter-languages` + ~34 per-language flags. **No `default` feature, and no `webview` feature** — WebView is the separate `gpui-wry` crate.

### 3.4 Theming — the part worth stealing

`crates/component/src/theme/`: `mod.rs` (29 KB), `theme_color.rs` (**136 pub fields**), `color.rs` (34.6 KB), `schema.rs` (54.3 KB), `registry.rs`, `motion.rs`, `default-colors.json` (58 KB), `default-theme.json` (13.9 KB), plus 21 themes in `/themes/*.json`.

```rust
pub trait ActiveTheme { fn theme(&self) -> &Theme; }
impl ActiveTheme for App { fn theme(&self) -> &Theme { Theme::global(self) } }
impl Global for Theme {}
impl Deref for Theme { type Target = ThemeColor; }   // cx.theme().primary reaches through
```
`Theme` fields include `colors, tokens, highlight_theme, light_theme, dark_theme, mode, font_family (".SystemUIFont"), font_size (16px), mono_font_family (Menlo/Consolas/DejaVu Sans Mono), mono_font_size (13px), radius, radius_lg, shadow, focus_ring, scrollbar_mode, motion, …`.
Switching: `Theme::init(cx)`, `Theme::change(ThemeMode::Dark, window, cx)`, `Theme::sync_system_appearance(window, cx)`.

**The semantic-token layer lives in `gpui-base`** (`crates/base/src/theme_tokens.rs`) and is exactly the shape a custom design system wants:
```rust
pub struct SemanticThemeTokens {
    pub colors: ColorTokens,        // 18 roles: background, foreground, surface, primary,
                                    // secondary, muted, accent, destructive (+ *_foreground),
                                    // border, input, ring, selection
    pub radius: RadiusTokens,       // none 0, sm 3, md 6, lg 8, xl 12, full 9999 (px)
    pub spacing: SpacingTokens,     // xxs 2, xs 4, sm 8, md 12, lg 16, xl 24, xxl 32 (px)
    pub typography: TypographyTokens,
    pub shadow: ShadowTokens,
}
```
> *"These tokens describe visual roles and scales. They intentionally do not contain component names such as `button`, `table`, or `sidebar`."*

Motion tokens worth copying verbatim (`theme/motion.rs`):
```rust
duration_fast: 120ms,  duration_normal: 180ms,  duration_slow: 280ms,
easing_enter: cubic_bezier(0.16, 1.0, 0.3, 1.0),
easing_exit:  cubic_bezier(0.4, 0.0, 1.0, 1.0),
easing_move:  cubic_bezier(0.2, 0.0, 0.0, 1.0),
spring_move:  Spring::new(280ms).with_damping(0.85).with_epsilon(0.1),
```
Nice detail: `radius_full()` returns `px(9999.)` **unless `radius == 0`**, so a squared theme stays squared everywhere.

### 3.5 Icons — the mechanism to copy

101 Lucide SVGs at `crates/assets/assets/icons/*.svg`. `gpui-component` itself embeds nothing ("we have not embedded any icon assets by default in `gpui-component`").

The chain:
1. `crates/assets/Cargo.toml`: `links = "gpui-kit-default-icons"`.
2. `crates/assets/build.rs`: `println!("cargo:icons-dir={}", icons_dir.display())` → cargo exposes `DEP_GPUI_KIT_DEFAULT_ICONS_ICONS_DIR` to dependents' build scripts.
3. `gpui-component/build.rs` re-exports it as `GPUI_KIT_DEFAULT_ICONS_DIR`.
4. `crates/component/src/icon.rs`: `icon_named!(IconName, "$GPUI_KIT_DEFAULT_ICONS_DIR");` — a proc macro that scans the dir at expansion time and generates one PascalCase variant per `.svg` (`arrow-right.svg` → `ArrowRight`) implementing `IconNamed::path() -> SharedString` returning `"icons/arrow-right.svg"`.
5. Runtime loading is separate — `crates/assets/src/native_assets.rs`:
   ```rust
   #[derive(rust_embed::RustEmbed)]
   #[folder = "assets"]
   #[include = "icons/**/*.svg"]
   pub struct Assets;
   impl AssetSource for Assets {
       fn load(&self, path: &str) -> Result<Option<Cow<'static,[u8]>>> { /* Self::get(path) */ }
       fn list(&self, path: &str) -> Result<Vec<SharedString>> { … }
   }
   ```
   registered via `application().with_assets(Assets)`.
6. `RenderOnce for Icon` is just `svg().flex_none().text_color(..).size(..).path(self.path)`.
   Size map: `XSmall→size_3, Small→size_3p5, Medium→size_4, Large→size_6`; unsized falls back to `window.text_style().font_size`.

**Escape hatch for our own set**: `icon_named!` is re-exported publicly and takes a path relative to *your* `CARGO_MANIFEST_DIR`:
```rust
icon_named!(IconName, "icons");
icon_named!(IconName, "icons", [Debug, Copy, PartialEq, Eq]);
```
We can replicate this in ~40 lines with `rust-embed` + a small proc macro (or just a `const` table), and skip the crate entirely.

### 3.6 Fonts

`gpui-component` / `gpui-kit-assets` **bundle no fonts for native**; they rely on the platform stack (`.SystemUIFont`, Menlo). The only `add_fonts` calls in the repo are WASM-only:
```rust
cx.text_system()
  .add_fonts(vec![Cow::Borrowed(include_bytes!("../fonts/JetBrainsMono-Regular.ttf").as_slice()), …])
  .expect("Failed to load fonts");
```
That is the pattern to copy when we want to ship a specific terminal font.

### 3.7 Zed's own `ui` crate — do not use

`https://raw.githubusercontent.com/zed-industries/zed/main/crates/ui/Cargo.toml`:
```toml
[package]
name = "ui"
version = "0.1.0"
publish.workspace = true      # workspace sets publish = false
license = "GPL-3.0-or-later"
```
- **GPL-3.0-or-later** and unpublished. It also depends on Zed-internal crates (`component`, `theme`, `icons`, `menu`, `ui_macros`, `documented`) that are equally unpublished.
- Usable only as a **reading reference** (and even then, be careful about copying code verbatim into a non-GPL product).

### 3.8 Recommendation

**Do not depend on `gpui-component` / `gpui-kit`. Build our own design system on raw gpui.**

Reasons:
1. **It forks the framework.** Depending on it means adopting `gpui-pre`, an unofficial republish of arbitrary Zed `main` commits by a single individual, with a version scheme unrelated to Zed's. That is a strictly worse supply-chain position than either official crates.io `gpui` or a pinned git dep on `zed-industries/zed`, and it *forecloses* both.
2. **It is the wrong layer for the stated goal.** The user wants a custom, reusable, minimal design system. `gpui-component` is the *styled* opinionated layer (their own shadcn analogy). Adopting it means fighting 136 component-named color fields.
3. **Its own docs point us at the right target anyway** — `gpui-base`'s `SemanticThemeTokens` is a 5-struct, ~40-role token system we can reimplement in an afternoon and own completely.

**Mine these four files as reference, then write our own:**
| File | Take |
|---|---|
| `crates/base/src/theme_tokens.rs` | the semantic token shape (colors/radius/spacing/typography/shadow) |
| `crates/component/src/theme/mod.rs` | `ActiveTheme` trait + `Global` + `Deref` pattern for `cx.theme().foo` |
| `crates/component/src/theme/motion.rs` | duration/easing/spring tokens, verbatim |
| `crates/component/src/icon.rs` + `crates/assets/src/native_assets.rs` | `rust-embed` `AssetSource` + typed icon enum + `svg().path(..)` |

Keep it as a git submodule/checkout for reading only; do not add it to `Cargo.toml`.

---

## 4. Key GPUI concepts (current API names)

All snippets below are from Zed `main` unless marked. Primary source for §4.1:
<https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/_ownership_and_data_flow.rs>
(also rendered at `docs.rs/gpui/latest/gpui/_ownership_and_data_flow/`).

### 4.1 Entities, Context, notify, observe, subscribe, EventEmitter

Everything is owned by the `App`. An `Entity<T>` is an `Rc`-like handle that only grants access when you have an `&App`/`&mut App`.

```rust
// create
let counter: Entity<Counter> = cx.new(|_cx: &mut Context<Counter>| Counter { count: 0 });

// mutate — the closure gets &mut T and a Context<T> (a wrapper around App tagged with this entity)
counter.update(cx, |counter, cx| {
    counter.count += 1;
    cx.notify();               // tell observers this entity changed → triggers re-render
});

// read
let n = counter.read(cx).count;
```

`observe` — "something changed":
```rust
let second = cx.new(|cx: &mut Context<Counter>| {
    cx.observe(&first_counter, |second: &mut Counter, first: Entity<Counter>, cx| {
        second.count = first.read(cx).count * 2;
    })
    .detach();                 // or hold the Subscription to cancel later
    Counter { count: 0 }
});
```

`EventEmitter` + `emit`/`subscribe` — typed events:
```rust
struct CounterChangeEvent { increment: usize }
impl EventEmitter<CounterChangeEvent> for Counter {}

cx.subscribe(&first, |second: &mut Counter, _first: Entity<Counter>, event, _cx| {
    second.count += event.increment * 2;
}).detach();

// on the emitter side:
first.update(cx, |first, cx| { first.count += 2; cx.emit(CounterChangeEvent { increment: 2 }); cx.notify(); });
```

Rule of thumb: `notify` = "I changed, re-render / re-read me"; `emit` = "this specific thing happened".
`cx.new` also exists on `AsyncApp`, so entities can be created off the main context.

Other useful `App` hooks (`crates/gpui/src/app.rs`, line numbers on `main`):
`observe_global::<G>` (2087), `observe_new::<T>` (2133), `observe_release` (2154), `observe_keystrokes` (2193), `on_window_closed` (2387), `set_global::<G>` (2062), `defer` (2001).
Globals: `struct MyState; impl Global for MyState {}` then `cx.set_global(..)`, `cx.global::<MyState>()`, `cx.global_mut::<MyState>()`.

### 4.2 Actions and key bindings

`crates/gpui/src/action.rs` (<https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/action.rs>):

```rust
use gpui::actions;
// namespaced (recommended) -> action names "term::Copy", "term::Paste"
actions!(term, [Copy, Paste, ClearScrollback, SplitRight]);
// or without a namespace
actions!([Quit]);
```
For actions with data, use the derive macro:
```rust
#[derive(Clone, PartialEq, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = term)]
pub struct SelectNext { pub replace_newest: bool }
```
`#[action(...)]` options: `namespace = …`, `name = "…"`, `no_json` (skip serde/schemars), `no_register`, `deprecated_aliases = [...]`, `deprecated = "…"`.
**Registering two actions with the same name panics during `App` creation.**

Binding keys (from `crates/gpui/examples/input.rs` on `main`):
```rust
cx.bind_keys([
    KeyBinding::new("cmd-a",           SelectAll,             None),
    KeyBinding::new("shift-left",      SelectLeft,            None),
    KeyBinding::new("ctrl-cmd-space",  ShowCharacterPalette,  None),
    KeyBinding::new("cmd-q",           Quit,                  None),
]);
cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
```
`KeyBinding::new(keystrokes, action, context_predicate)` — the third argument is the **key context predicate**, e.g. `Some("TextInput")`, `Some("Terminal && !mode_alt")`. This is how you build modal / vim-like keymaps.

Element-level handling and key contexts:
```rust
div()
    .key_context("TextInput")               // <- what the predicate above matches against
    .track_focus(&self.focus_handle(cx))
    .on_action(cx.listener(Self::backspace))
    .on_action(cx.listener(Self::select_all))
    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| { … }))
```
`cx.listener(...)` adapts a `&mut Self` method into the element's callback signature. Handler shape is
`fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>)`.

Dispatch walks the focus chain from the focused element up to the root, so an inner `key_context` wins over an outer one — that's the mechanism for "vim mode inside the terminal pane only".

`cx.observe_keystrokes(|ev, window, cx| …)` gives you every keystroke (useful for a key-hint HUD).

### 4.3 Focus

```rust
struct TextInput { focus_handle: FocusHandle, … }

// create in the entity constructor:
cx.new(|cx| TextInput { focus_handle: cx.focus_handle(), … })

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() }
}

// in render:
div().track_focus(&self.focus_handle(cx)) …

// query / set:
focus_handle.is_focused(window)
window.focus(&view.text_input.focus_handle(cx), cx);   // NOTE: takes cx on `main`
```
`.track_focus()` is what makes `key_context` + `on_action` on that element participate in dispatch. An element without `track_focus` never receives actions.

### 4.4 Background work, async, timers

`crates/gpui/src/executor.rs`, `crates/gpui/src/app.rs`, `crates/gpui/src/app/async_context.rs` on `main`.

```rust
// off the main thread; future must be Send
let task: Task<Vec<u8>> = cx.background_executor().spawn(async move { heavy_work() });
// shorthand available on Context/App/AsyncApp:
let task = cx.background_spawn(async move { … });

// on the main thread, gets an AsyncApp across await points
let task: Task<()> = cx.spawn(async move |cx: &mut AsyncApp| {
    let bytes = cx.background_spawn(async { read_a_lot() }).await;
    // hop back onto the app to mutate state:
    entity.update(cx, |this, cx| { this.data = bytes; cx.notify(); });
});
task.detach();                 // dropping a Task CANCELS it — always detach or store it
```

**API drift to watch:**
- On `main`, `AsyncApp::update` returns `R`:
  `pub fn update<R>(&self, f: impl FnOnce(&mut App) -> R) -> R`
  and `AppContext::update_entity(..) -> R`.
- On crates.io **0.2.2** they return `Result<R>`:
  `pub fn update<R>(&self, f: impl FnOnce(&mut App) -> R) -> Result<R>` (`src/app/async_context.rs:142`).
  So on 0.2.2 you must `?`/`.unwrap()` after `entity.update(cx, …)` inside a spawned task; on `main` you must NOT.
  This is the #1 compile error when porting a 0.2.2 tutorial to `main` and vice versa.

`cx.spawn`'s bound is `AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static` (both eras) — i.e. an
`async move |cx| { … }` closure, not `|cx| async move { … }`.

Executors:
```rust
cx.background_executor()   // -> BackgroundExecutor : spawn, spawn_dedicated, spawn_with_priority,
                           //    timer(Duration) -> Task<()>, now(), num_cpus(), block_on()
cx.foreground_executor()   // -> ForegroundExecutor : spawn (non-Send), spawn_when_idle,
                           //    idle_time_remaining(), block_with_timeout()
```
Timers: `cx.background_executor().timer(Duration::from_millis(16)).await` inside a spawned task is the idiomatic tick loop.
`AsyncWindowContext` adds `on_next_frame(f)`, `update(|window, cx| …) -> Result<R>`, `update_root(...)`.
GPUI's async runtime is its own executor bound to the platform run loop (`async-task` + a scheduler crate), **not** tokio; for tokio interop Zed uses a `gpui_tokio` crate (unpublished).

**Updating UI from a background thread:** never touch `App` from a background thread. Do work in
`background_executor().spawn`, `.await` it inside a `cx.spawn(async move |cx| …)`, then call
`entity.update(cx, |this, cx| { …; cx.notify(); })`. For a producer that lives entirely off-thread,
send over a channel (`futures::channel::mpsc`, `postage`) and drain it in a foreground `cx.spawn` loop.

### 4.5 Text, fonts, `SharedString`

- `SharedString` is GPUI's cheap-clone string (`Arc<str>`-backed); `.into()` from `&'static str` and `String`. Use it for all UI text.
- `cx.text_system()` / `window.text_system()` → `Arc<TextSystem>`.
- `cx.text_system().add_fonts(vec![Cow::Borrowed(include_bytes!("…ttf").as_slice())])?` registers embedded fonts.
- On macOS you do **not** need to register anything to see text: `example_support::load_fonts` is a
  no-op on non-wasm targets and the platform stack (`.SystemUIFont`, Menlo) resolves via CoreText/font-kit.
  You *do* need the `font-kit` cargo feature (§1.6).
- Styling on `div()`: `.text_xs()/.text_sm()/.text_xl()`, `.text_color(rgb(0x…))`, `.font_family("Berkeley Mono")`, `.line_height(px(18.))`, `.font_weight(FontWeight::BOLD)`.

### 4.6 `WindowOptions` / custom chrome
See §2.4.

### 4.7 Menus and quitting

`crates/gpui/examples/set_menus.rs` on `main`:
```rust
cx.activate(true);
cx.on_action(quit);                      // fn quit(_: &Quit, cx: &mut App) { cx.quit(); }
cx.set_menus([Menu::new("myapp").items([
    MenuItem::os_submenu("Services", SystemMenuType::Services),
    MenuItem::separator(),
    MenuItem::action("List Mode", ToggleCheck).checked(true),
    MenuItem::submenu(Menu::new("Mode").items([ … ])),
    MenuItem::action("Disabled Item", gpui::NoAction).disabled(true),
    MenuItem::separator(),
    MenuItem::action("Quit", Quit),
])]);
```
`MenuItem` builders: `action`, `separator`, `submenu`, `os_submenu(_, SystemMenuType::Services)`, `.disabled(bool)`, `.checked(bool)`.
Menus are **not** live-bound — `set_app_menus(cx)` is re-called after state changes to refresh check marks.

### 4.8 Custom painting — the primitives for a terminal grid

> Verified against Zed `main` @ `ee3b5558` (2026-09-04). Paths below are
> `https://raw.githubusercontent.com/zed-industries/zed/main/<path>`.

#### The `Element` trait (`crates/gpui/src/element.rs`)

```rust
pub trait Element: 'static + IntoElement {
    type RequestLayoutState: 'static;
    type PrepaintState: 'static;

    fn id(&self) -> Option<ElementId>;
    fn source_location(&self) -> Option<&'static panic::Location<'static>>;

    fn request_layout(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>,
                      window: &mut Window, cx: &mut App) -> (LayoutId, Self::RequestLayoutState);

    fn prepaint(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>,
                bounds: Bounds<Pixels>, request_layout: &mut Self::RequestLayoutState,
                window: &mut Window, cx: &mut App) -> Self::PrepaintState;

    fn paint(&mut self, id: Option<&GlobalElementId>, inspector_id: Option<&InspectorElementId>,
             bounds: Bounds<Pixels>, request_layout: &mut Self::RequestLayoutState,
             prepaint: &mut Self::PrepaintState, window: &mut Window, cx: &mut App);

    // defaulted, main only:
    fn a11y_role(&self) -> Option<accesskit::Role> { None }
    fn write_a11y_info(&self, _node: &mut accesskit::Node) {}
    fn a11y_synthetic_children(&mut self, _p: &mut Self::PrepaintState, _b: &mut A11ySubtreeBuilder) {}
    fn into_any(self) -> AnyElement { AnyElement::new(self) }
}
```
You must also `impl IntoElement for X { type Element = Self; fn into_element(self) -> Self { self } }`.

#### Quad primitives (`crates/gpui/src/window.rs` L7283-7375 — **not** `div.rs`, **not** `scene.rs`)

```rust
pub struct PaintQuad {
    pub bounds: Bounds<Pixels>, pub corner_radii: Corners<Pixels>,
    pub background: Background, pub border_widths: Edges<Pixels>,
    pub border_color: Hsla, pub border_style: BorderStyle,
}
pub fn quad(bounds, corner_radii, background, border_widths, border_color, border_style) -> PaintQuad;
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad;
pub fn outline(bounds, border_color, border_style: BorderStyle) -> PaintQuad;  // 1px, transparent bg
```
*(Identical in crates.io 0.2.2 at `window.rs` L5059-5098.)*

`Window` paint-phase entry points:
```rust
window.paint_quad(quad: PaintQuad)
window.paint_path(path: Path<Pixels>, color: impl Into<Background>)
window.paint_underline(origin, width, &UnderlineStyle)
window.paint_strikethrough(origin, width, &StrikethroughStyle)
window.paint_glyph(origin, font_id, glyph_id, font_size, color) -> Result<()>
window.paint_emoji(origin, font_id, glyph_id, font_size) -> Result<()>
window.paint_svg(bounds, path, data, transformation, color, cx) -> Result<()>
window.paint_image(bounds, image_bounds, corner_radii, data: Arc<RenderImage>, frame_index, grayscale)
window.paint_layer(bounds, |window| { … })
window.with_content_mask(Some(ContentMask { bounds }), |window| { … })
window.set_cursor_style(CursorStyle::IBeam, &hitbox)
window.insert_hitbox(bounds, HitboxBehavior)   // prepaint phase only
window.pixel_snap(v: Pixels) -> Pixels
window.rem_size() / window.line_height() / window.text_style() / window.text_system()
```

#### `canvas()` (`crates/gpui/src/elements/canvas.rs`, 95 lines — **identical in 0.2.2 and main**)

```rust
pub fn canvas<T>(
    prepaint: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T,
    paint:    impl 'static + FnOnce(Bounds<Pixels>, T, &mut Window, &mut App),
) -> Canvas<T>
```
- Both closures are `FnOnce`, consumed on first frame → the element is rebuilt each `render()`.
- `Canvas<T>: Styled` — you MUST size it (`.size_full()`); it has no intrinsic size.
- `id()` returns `None` → no element state, no hitbox. Register mouse handlers from inside the
  paint closure using the captured `bounds` (see `crates/gpui/examples/data_table.rs` L326+).

Idiomatic use, `crates/gpui/examples/painting.rs` L356-400:
```rust
canvas(
    move |_, _, _| {},
    move |_, _, window, _| {
        for (bounds, color) in background_quads.iter() {
            window.paint_quad(quad(*bounds, px(0.), *color, px(0.), gpui::transparent_black(), Default::default()));
        }
        let mut builder = PathBuilder::stroke(px(1.));
        if dashed { builder = builder.dash_array(&[px(4.), px(2.)]); }
        if let Ok(path) = builder.build() { window.paint_path(path, gpui::black()); }
    },
).size_full()
```

> For a terminal, `canvas()` is fine for a prototype but a real `impl Element` is better (you get a
> proper `PrepaintState` to carry shaped runs across the prepaint→paint boundary).
> **The best small reference is `crates/gpui/examples/input.rs` L422-580** — a complete minimal custom
> element that shapes in prepaint, stores `ShapedLine` + `PaintQuad`s, and replays them in paint.

#### Text system (`crates/gpui/src/text_system.rs`, `text_system/line.rs`)

Two distinct objects:
- `cx.text_system() -> &Arc<TextSystem>` — font level: `resolve_font(&Font) -> FontId`,
  `advance(font_id, size, ch) -> Result<Size<Pixels>>`, `em_advance`, `ch_advance`,
  `ascent/descent/cap_height/x_height`, `add_fonts(Vec<Cow<'static,[u8]>>)`, `all_font_names()`,
  `prewarm_fonts(&[Font])`.
- `window.text_system() -> &Arc<WindowTextSystem>` — shaping + a `LineLayout` cache:
```rust
pub fn shape_line(&self, text: SharedString, font_size: Pixels,
                  runs: &[TextRun], force_width: Option<Pixels>) -> ShapedLine
pub fn shape_line_by_hash(&self, text_hash: u64, text_len: usize, font_size: Pixels,
                  runs: &[TextRun], force_width: Option<Pixels>,
                  materialize_text: impl FnOnce() -> SharedString) -> ShapedLine   // main only
                  // NOTE: on a cache hit `ShapedLine.text` is an empty placeholder — don't read it.
pub fn shape_text(&self, text, font_size, runs, wrap_width: Option<Pixels>,
                  line_clamp: Option<usize>) -> Result<SmallVec<[WrappedLine; 1]>>
```
```rust
impl ShapedLine {          // Deref<Target = LineLayout>
    pub fn paint(&self, origin: Point<Pixels>, line_height: Pixels,
                 align: TextAlign, align_width: Option<Pixels>,
                 window: &mut Window, cx: &mut App) -> Result<()>   // ⚠ 0.2.2: (origin, line_height, window, cx)
    pub fn paint_background(&self, origin, line_height, align, align_width, window, cx) -> Result<()>
    pub fn width(&self) -> Pixels          // main only
    pub fn split_at(&self, byte_ix) -> (ShapedLine, ShapedLine)  // main only
}
pub struct LineLayout { pub font_size, pub width, pub ascent, pub descent,
                        pub runs: Vec<ShapedRun>, pub len: usize }   // + x_for_index / index_for_x
pub struct ShapedRun   { pub font_id: FontId, pub glyphs: Vec<ShapedGlyph> }
pub struct ShapedGlyph { pub id: GlyphId, pub position: Point<Pixels>, pub index: usize }

pub struct TextRun { pub len: usize /* utf-8 BYTES */, pub font: Font, pub color: Hsla,
                     pub background_color: Option<Hsla>, pub underline: Option<UnderlineStyle>,
                     pub strikethrough: Option<StrikethroughStyle> }   // derives Default
pub fn font(family: impl Into<SharedString>) -> Font;   // Font::default() == font(".SystemUIFont")
pub struct FontFeatures(pub Arc<Vec<(String, u32)>>);
impl FontFeatures { pub fn disable_ligatures() -> Self /* vec![("calt", 0)] */ }
pub enum TextAlign { #[default] Left, Center, Right }
```

#### How Zed's terminal actually does it — `crates/terminal_view/src/terminal_element.rs` (3005 lines)

This is the single most important reference for our use case. Shape of the solution:

`type RequestLayoutState = (); type PrepaintState = LayoutState;` — **all** shaping, run batching and
background merging happens in `prepaint`; `paint` is a pure replay loop.

`LayoutState` holds `hitbox`, `batched_text_runs: Vec<BatchedTextRun>`, `rects: Vec<LayoutRect>`,
`block_element_rects`, `relative_highlighted_ranges`, `cursor: Option<CursorLayout>`,
`background_color`, `dimensions: TerminalBounds`, `display_offset`, `base_text_style`.

**Cell size from the font** (prepaint L1281-1295):
```rust
let font_pixels = text_style.font_size.to_pixels(window.rem_size());
let line_height = f32::from(font_pixels) * line_height_multiplier;   // from settings, NOT font metrics
let font_id     = cx.text_system().resolve_font(&text_style.font());
let cell_width  = text_system.advance(font_id, font_pixels, 'm').unwrap().width;
```
Then line height and row count are snapped to whole device pixels.
```rust
pub struct TerminalBounds { pub cell_width: Pixels, pub line_height: Pixels, pub bounds: Bounds<Pixels> }
// num_lines() = (h/lh).next_up().floor();  num_columns() likewise
```

**Text: `shape_line` per merged run, then `ShapedLine::paint` — NOT `paint_glyph`.**
`BatchedTextRun::paint` (L149-183) is the whole text path:
```rust
let pos = point(origin.x + self.start_point.column as f32 * dimensions.cell_width,
                origin.y + self.start_point.line   as f32 * dimensions.line_height);
window.text_system()
    .shape_line(self.text.clone().into(),
                self.font_size.to_pixels(window.rem_size()),
                std::slice::from_ref(&self.style),
                Some(dimensions.cell_width))      // <-- force_width LOCKS glyphs to the cell grid
    .paint(pos, dimensions.line_height, gpui::TextAlign::Left, None, window, cx)
    .log_err();
```
`force_width: Some(cell_width)` is the monospace-grid trick. Shaping happens *in paint*, relying on
the `LineLayout` cache; `shape_line_by_hash` avoids materialising a `SharedString` on cache hits.

**Run batching** — `layout_grid` (L438-633) merges adjacent same-style cells into one string:
```rust
pub struct BatchedTextRun {
    pub start_point: LayoutPoint, pub text: String /* with_capacity(100) */,
    pub cell_count: usize,        // advances CELLS, not bytes
    pub style: TextRun, pub font_size: AbsoluteLength,
}
fn can_append(&self, other: &TextRun) -> bool {
    self.style.font == other.font && self.style.color == other.color
        && self.style.background_color == other.background_color
        && self.style.underline == other.underline
        && self.style.strikethrough == other.strikethrough
}
// plus: same line && batch.start_point.column + batch.cell_count == cell.column  (contiguous)
```
Flushed at line boundaries, style changes, column gaps, and block-drawing chars. Zero-width combining
marks bump `style.len` but not `cell_count`. Wide-char spacers are skipped.

**Backgrounds: a separate two-pass rectangle merge** (because blanks produce no text run):
1. horizontal run-extension while walking cells;
2. `merge_background_regions()` (L360) coalesces horizontally-adjacent regions with the same line span
   **and** vertically-adjacent regions with the same column span (true rectangle coalescing), then
   splits multi-line regions back into one single-line `LayoutRect` each.
Cells with the default background are skipped entirely.

```rust
// LayoutRect::paint, L246-269 — note the floor/ceil to avoid seams between adjacent spans
let position = point((origin.x + col as f32 * cell_width).floor(), origin.y + line as f32 * line_height);
let size     = point((cell_width * self.num_of_cells as f32).ceil(), line_height).into();
window.paint_quad(fill(Bounds::new(position, size), self.color));
```

**Sub-cell block glyphs** (`▀▄▖`, sextants, shades) are painted as **quads**, not text, on an 8×24
subcell grid (`BLOCK_SUBCELL_COLUMNS = 8`, `BLOCK_SUBCELL_LINES = 24`).

**Cursor** uses `editor::CursorLayout` (from Zed's `editor` crate, not gpui):
```rust
let bounds = window.pixel_snap_bounds(self.bounds(origin));
let cursor = if matches!(self.shape, CursorShape::Hollow) { outline(bounds, self.color, BorderStyle::Solid) }
             else { fill(bounds, self.color) };
window.paint_quad(cursor);
if let Some(block_text) = &self.block_text { block_text.paint(...)?; }   // the glyph under a block cursor
```
Bar = `size(px(2.0), line_height)`; Underline = `size(block_width, px(2.0))` at `line_height - px(2.)`.

**Paint order** (L1613-1770), all inside `window.with_content_mask(Some(ContentMask { bounds }), …)`:
1. whole-element `fill(bounds, background_color)`
2. `layout.rects` (merged cell backgrounds)
3. selection highlights (`HighlightedRange::paint` — rounded paths)
4. `layout.batched_text_runs`
5. `layout.block_element_rects`
6. IME marked text
7. cursor
8. block-below-cursor element / hyperlink tooltip

**Scroll** is not a gpui scroll container — it translates the paint origin and snaps to device pixels:
```rust
let origin  = layout.dimensions.bounds.origin - point(px(0.), scroll_top);
let snap_px = |v: Pixels| Pixels::from((f32::from(v) * scale_factor).floor() / scale_factor);
let origin  = point(snap_px(origin.x), snap_px(origin.y));
```

**Takeaway for our terminal:** one `impl Element` with `PrepaintState` = merged runs + merged
background rects; batch aggressively by style; use `shape_line(..., force_width: Some(cell_width))`
+ `ShapedLine::paint` (do **not** hand-roll `paint_glyph`); merge background quads in 2D; snap to
device pixels with floor/ceil; own your scroll offset.

### 4.9 Scrolling primitives

```rust
// crates/gpui/src/elements/uniform_list.rs — IDENTICAL in 0.2.2 and main
pub fn uniform_list<R: IntoElement>(
    id: impl Into<ElementId>,
    item_count: usize,
    f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>,
) -> UniformList
```
To get `&mut Self` back inside the closure, use `cx.processor(...)`:
```rust
uniform_list("entries", 50, cx.processor(|_this, range, _window, _cx| {
    range.map(|ix| div().id(ix).child(format!("Item {}", ix + 1))).collect::<Vec<_>>()
})).h_full()
```
```rust
// crates/gpui/src/app/context.rs
pub fn processor<E, R>(&self, f: impl Fn(&mut T, E, &mut Window, &mut Context<T>) -> R + 'static)
    -> impl Fn(E, &mut Window, &mut App) -> R + 'static                                    // L264
pub fn listener<E: ?Sized>(&self, f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static)
    -> impl Fn(&E, &mut Window, &mut App) + 'static                                        // L252
```
Builders: `.track_scroll(&UniformListScrollHandle)`, `.with_width_from_item(Option<usize>)`,
`.with_sizing_behavior(ListSizingBehavior)`, `.with_decoration(..)`, `.y_flipped(bool)`.
`UniformListScrollHandle`: `scroll_to_item(ix, ScrollStrategy)`, `scroll_to_bottom()`,
`logical_scroll_top_index()`, `is_scrolled_to_end() -> Option<bool>`.

```rust
// crates/gpui/src/elements/list.rs — variable-height virtualized list
pub fn list(state: ListState,
            render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static) -> List
ListState::new(item_count, ListAlignment::Bottom, overdraw: px(500.))
// main-only additions: .measure_all(), .with_uniform_item_height(px), .remeasure(),
//   follow-tail (set_follow_mode / pause_following_tail / is_following_tail),
//   scrollbar support (scrollbar_drag_started/ended, set_offset_from_scrollbar,
//   max_offset_for_scrollbar, viewport_bounds)
```
`ListAlignment::Bottom` + follow-tail is exactly the "chat/log scrolls with new output" behaviour.

> **Neither is right for a terminal grid.** `TerminalElement` uses neither — it takes `relative(1.)`
> height, tracks its own `scroll_top`, and translates the paint origin. `list`/`uniform_list` assume
> one element per item, which is far too much overhead per terminal row.

### 4.10 Overlays / floating layers

**`overlay()` no longer exists** (in either 0.2.2 or main). The element modules on `main` are exactly:
`anchored, animation, canvas, container_query, deferred, div, image_cache, img, list, surface, svg, text, uniform_list`.
Replacement idiom is `deferred(anchored()…)`:
```rust
pub fn deferred(child: impl IntoElement) -> Deferred;   // .with_priority(usize), default 0
pub fn anchored() -> Anchored;
impl Anchored {
    pub fn anchor(self, anchor: Anchor) -> Self       // ⚠ 0.2.2 took `Corner`; `Corner` is DELETED on main
    pub fn position(self, p: Point<Pixels>) -> Self
    pub fn offset(self, p: Point<Pixels>) -> Self
    pub fn position_mode(self, AnchoredPositionMode) -> Self   // Window | Local
    pub fn snap_to_window(self) -> Self
    pub fn snap_to_window_with_margin(self, edges: impl Into<Edges<Pixels>>) -> Self
}
pub enum Anchor { TopLeft, TopRight, BottomLeft, BottomRight,
                  TopCenter, BottomCenter, LeftCenter, RightCenter }   // main; was `Corner` (4 variants)
```
```rust
this.child(deferred(
    anchored().anchor(Anchor::TopLeft).position(pos)
        .snap_to_window_with_margin(px(8.))
        .child(my_popover())
))
```
Nested `deferred` is supported on `main` ("Now GPUI supports nested deferred!", `examples/popover.rs` L62).

### 4.11 `svg()`, `img()`, `AssetSource`

```rust
// crates/gpui/src/elements/svg.rs
pub fn svg() -> Svg;
impl Svg {
    pub fn path(self, path: impl Into<SharedString>) -> Self;   // resolved through AssetSource
    pub fn external_path(self, path: impl Into<SharedString>) -> Self;  // main only — reads from fs
    pub fn data(self, data: &[u8]) -> Self;                     // main only — inline bytes
    pub fn with_transformation(self, t: Transformation) -> Self;
}
// color = .text_color(...), size = .size_4() etc.  Svg: Styled + InteractiveElement.

pub fn img(source: impl Into<ImageSource>) -> Img;
pub enum ImageSource { Resource(Resource), Render(Arc<RenderImage>), Image(Arc<Image>), Custom(..) }
// From<&str>: parses as a URL -> Resource::Uri, otherwise Resource::Embedded (asset path)
```
```rust
// crates/gpui/src/assets.rs  (NOT asset_source.rs) — UNCHANGED between 0.2.2 and main
pub trait AssetSource: 'static + Send + Sync {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;   // note the double wrapping
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}
impl AssetSource for () { /* load -> Ok(None), list -> Ok(vec![]) */ }   // the DEFAULT
```
**`svg()` renders nothing until you register an `AssetSource`** — `with_assets` also constructs
`cx.svg_renderer = SvgRenderer::new(asset_source)`.

```rust
// Registration — the method name is the same in both eras (crates/gpui/src/app.rs L202):
pub fn with_assets(self, asset_source: impl AssetSource) -> Self
// 0.2.2:  Application::new().with_assets(Assets { .. }).run(|cx| …)
// main:   gpui_platform::application().with_assets(Assets { .. }).run(|cx| …)
```
Working `AssetSource` from `crates/gpui/examples/svg/svg.rs`:
```rust
struct Assets { base: PathBuf }
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path)).map(|d| Some(Cow::Owned(d))).map_err(Into::into)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|es| es.filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
                       .map(SharedString::from).collect())
            .map_err(Into::into)
    }
}
// usage: svg().path("svg/dragon.svg").size_8().text_color(rgb(0xff0000))
```
For production, embed with `rust-embed` (what gpui-kit does) or Zed's `util::fs_embed!` (embedded in
release, read-from-checkout in dev — `crates/assets/src/assets.rs`). Zed also loads fonts through the
same source: `Assets::load_fonts(cx)` iterates `list("fonts")`, filters `.ttf`, and calls
`cx.text_system().add_fonts(..)`.

### 4.12 Mouse & interaction

`InteractiveElement` / `StatefulInteractiveElement` on `div()`:
`.id("x")` (required for stateful hover/click/active), `.hover(|s| s.bg(..))`, `.active(|s| s.bg(..))`,
`.on_click(|ev: &ClickEvent, window, cx| …)`, `.on_mouse_down(MouseButton::Left, …)`,
`.on_mouse_up`, `.on_mouse_move`, `.on_scroll_wheel`, `.on_drag`, `.on_drop`, `.cursor_pointer()`,
`.tooltip(..)`, `.occlude()`. `MouseButton::{Left, Right, Middle, Navigate(_)}`.
Inside a custom `Element`, register from `paint` with `window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| …)`
and hit-test against a `Hitbox` created in `prepaint` via `window.insert_hitbox(bounds, HitboxBehavior::Normal)`.

### 4.13 API drift table — crates.io `gpui 0.2.2` (Oct 2025) vs `main` (Sept 2026)

| API | 0.2.2 | main |
|---|---|---|
| `Application::new()` / `::headless()` | present | **removed**; `Application::with_platform(Rc<dyn Platform>)` + `gpui_platform::application()` |
| `gpui_platform` crate | n/a | required; **not on crates.io** |
| `ShapedLine::paint` | `(origin, line_height, window, cx)` | `(origin, line_height, align: TextAlign, align_width: Option<Pixels>, window, cx)` |
| `ShapedLine::width()` / `split_at()` | absent | present |
| `LineLayout::paint` (public) | absent | present |
| `Anchored::anchor(..)` | takes `Corner` (4 variants) | takes `Anchor` (8 variants); `Corner` deleted |
| `AsyncApp::update` / `update_entity` | `-> Result<R>` | `-> R` |
| `Element` a11y methods | absent | `a11y_role`, `write_a11y_info`, `a11y_synthetic_children` |
| `elements/container_query.rs` | absent | present |
| `ListState` scrollbar / follow-tail / `measure_all` / `with_uniform_item_height` | absent | present |
| `svg().external_path()` / `.data()` | absent | present |
| `shape_line_by_hash` | absent | present |
| `uniform_list(id, count, f)` | same | same |
| `list(state, render_item)` | same | same |
| `canvas(prepaint, paint)` | same | same |
| `fill` / `quad` / `outline` / `PaintQuad` | same | same |
| `AssetSource` trait + `with_assets` | same | same |
| `shape_line` / `shape_text` | same | same |
| `Element` core method signatures | same | same |
| `overlay()` | already gone | gone |

> **Version numbers are useless as a discriminator:** `crates/gpui/Cargo.toml` on `main` *still says*
> `version = "0.2.2"`, the same as the published crate — but the code is a year apart. Use the
> presence of `gpui_platform` / `Application::new()` to tell which era you are looking at.


---

## 5. Hosting a native NSView / Metal layer inside a GPUI window

**Verdict: (b) — possible today via a raw-window-handle "unsafe" recipe that is production-proven on macOS, but it is NOT a supported API, and GPUI will not clip, z-order, or composite the native view.**

### 5.1 Where the macOS platform code lives now

After the crate split, it is **not** in `crates/gpui/src/platform/mac/` and **not** in `crates/gpui_platform/`:

| What | Path on `main` |
|---|---|
| `MacWindow`, NSView/NSWindow, the `HasWindowHandle` impl | `crates/gpui_macos/src/window.rs` |
| `MacPlatform` | `crates/gpui_macos/src/platform.rs` |
| Metal renderer, `CAMetalLayer`, `draw_surfaces` | `crates/gpui_apple/src/metal_renderer.rs` |
| `PlatformWindow` trait | `crates/gpui/src/platform.rs` |
| `Window`, `paint_surface` | `crates/gpui/src/window.rs` |
| `surface()` element | `crates/gpui/src/elements/surface.rs` |
| `crates/gpui_platform/` | a 207-line re-export shim only |

### 5.2 There is no `native_window()` / `native_view()` accessor

`gpui_macos.rs` re-exports only `pub use platform::MacPlatform`. `MacWindow`'s `native_window: id` and
`native_view: NonNull<Object>` fields are `pub(crate)`. `PlatformWindow` has no such method.
(Windows is the exception: `PlatformWindow::get_raw_handle(&self) -> HWND`, cfg-gated.)

### 5.3 …but `raw-window-handle` IS implemented, publicly, on `gpui::Window`

`raw-window-handle = "0.6"` (workspace `Cargo.toml` L767; unified in
[PR #61193](https://github.com/zed-industries/zed/pull/61193), merged 2026-07-17).

```rust
// crates/gpui/src/platform.rs:816
pub trait PlatformWindow: HasWindowHandle + HasDisplayHandle { … }

// crates/gpui/src/window.rs:7098    <-- THE public entry point
impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, HandleError> {
        self.platform_window.window_handle()
    }
}

// crates/gpui_macos/src/window.rs:2205
impl rwh::HasWindowHandle for MacWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        // SAFETY: AppKitWindowHandle wraps a pointer to an NSView
        unsafe { Ok(rwh::WindowHandle::borrow_raw(rwh::RawWindowHandle::AppKit(
            rwh::AppKitWindowHandle::new(self.0.lock().native_view.cast()))))
        }
    }
}
```
Landed in [PR #24327](https://github.com/zed-industries/zed/pull/24327) (2025-02-06). Maintainer @mikayla-maki's
approval is the official stance: *"I don't think it causes any harm… it only increases GPUI's viability
as a general purpose rust UI framework."* Linux/X11 followed in
[#50768](https://github.com/zed-industries/zed/pull/50768).

**Two gotchas:**
1. `gpui` does not re-export `raw_window_handle` — add `raw-window-handle = "0.6"` to your own `Cargo.toml`.
2. `Window` also has an *inherent* `pub fn window_handle(&self) -> AnyWindowHandle` (window.rs:2174) that
   shadows the trait method. Always call it explicitly:
   ```rust
   let h = raw_window_handle::HasWindowHandle::window_handle(window)?;
   let ns_view = match h.as_raw() { RawWindowHandle::AppKit(w) => w.ns_view.as_ptr(), _ => unreachable!() };
   ```

### 5.4 No `ExternalView` element — and `surface()` is a trap

There is no `ExternalView`, `NativeView`, `PlatformView`, or `SurfaceElement`. `surface()` still exists
but is video-only:
```rust
// crates/gpui/src/elements/surface.rs
pub enum SurfaceSource { #[cfg(target_os = "macos")] Surface(CVPixelBuffer) }
#[cfg(target_os = "macos")] pub fn surface(source: impl Into<SurfaceSource>) -> Surface
// crates/gpui/src/window.rs:4777
#[cfg(target_os = "macos")] pub fn paint_surface(&mut self, bounds: Bounds<Pixels>, image_buffer: CVPixelBuffer)
```
`crates/gpui_apple/src/metal_renderer.rs::draw_surfaces` (~L1155):
```rust
assert_eq!(surface.image_buffer.get_pixel_format(),
           kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
```
A plain `assert_eq!` — **it aborts in release too**. It only accepts biplanar YCbCr video (Zed's LiveKit
screenshare path). Handing it an RGBA/BGRA IOSurface crashes the process.
[PR #61291](https://github.com/zed-industries/zed/pull/61291) adds a `kCVPixelFormatType_32BGRA` branch —
open since 2026-07-19 with **no maintainer comment**.

### 5.5 Existence proof: how gpui-component's WebView does it

`gpui-wry` 0.6.0 (crates.io 2026-09-03) → wry `lb-wry` 0.53.3. The attach is in the *example*:
```rust
use raw_window_handle::HasWindowHandle;
let window_handle = window.window_handle().expect("No window handle");
builder.build_as_child(&window_handle).unwrap();
```
wry then does, in `src/wkwebview/mod.rs`:
```rust
let ns_view = match window.window_handle()?.as_raw() { RawWindowHandle::AppKit(w) => w.ns_view.as_ptr(), … };
if is_child { ns_view.addSubview(&webview); }
webview.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
```
i.e. a plain `addSubview:` on GPUI's own `NSView` — the same view whose `makeBackingLayer` returns the
`CAMetalLayer` (`gpui_macos/src/window.rs:2933`). `WebViewElement::prepaint` re-syncs bounds every frame.

**The honest limitation, from their own README** (`crates/webview/README.md`):
> *"The WebView will render on top of the GPUI window, any GPUI elements behind the WebView bounds will
> be covered. Only supports macOS and Windows currently. So, we recommend using the webview in a
> separate window or in a Popup layer."*

Deeper analysis in `crates/webview/WEBVIEW_OVERLAY_RESEARCH.md` (805 lines, 2026-07-30,
[gpui-kit PR #2626](https://github.com/longbridge/gpui-kit/pull/2626)):
> *"GPUI elements are composed into one GPU surface, while wry creates a platform-native child view or
> window by default. Paint order inside GPUI cannot cross the native window hierarchy."*

### 5.6 Two projects already host libghostty's Metal NSView inside GPUI

**`nowledge-co/con-terminal`** — 559 stars, active 2026-09-04, shipping product.
`crates/con-app/src/ghostty_view.rs` (2,962 lines), L1186-1260:
```rust
let raw_handle = HasWindowHandle::window_handle(window)?;
let gpui_nsview = match raw_handle.as_raw() { RawWindowHandle::AppKit(h) => h.ns_view.as_ptr() as id, _ => return };
let parent_nsview: id = msg_send![gpui_nsview, superview];   // gpui view's PARENT
// NSScrollView host (setDrawsBackground:NO, clipsToBounds) -> documentView -> surface NSView
let _: () = msg_send![parent_nsview,
    addSubview: host
    positioned: NSWindowOrderingMode::NSWindowBelow          // BELOW, not above
    relativeTo: gpui_nsview];
app.new_surface(nsview as *mut c_void, scale, …);            // hand to libghostty
```
The trick is **hole-punching**: insert the native view *below* GPUI's Metal view and keep the GPUI root
transparent over the terminal rect. Their own postmortem
(`postmortem/2026-04-15-monterey-transparent-terminal.md`) documents the cost: *"Con uses a transparent
GPUI window so the embedded Ghostty NSView can render terminal glass underneath the GPUI chrome… On
macOS 12, that combination is fragile enough to produce a blank transparent surface"*, plus manual
hosted-`CALayer` frame/bounds/contentsScale re-sync on older AppKit.

**`behzade/gpui-libghostty`** — crates.io `gpui-libghostty` 0.2.1 (2026-09-02, alpha, 2 stars, 232 downloads).
`crates/gpui-ghostty/src/terminal.rs:446` + an ObjC shim:
```objc
@interface GpuiGhosttyView : NSView @end
@implementation GpuiGhosttyView
- (NSView *)hitTest:(NSPoint)point { return nil; }   // never steal AppKit input
@end
state->view = [[GpuiGhosttyView alloc] initWithFrame:NSMakeRect(0,0,800,600)];
[state->parent addSubview:state->view];
surface_config.platform_tag = GHOSTTY_PLATFORM_MACOS;
surface_config.platform.macos.nsview = state->view;
state->surface = ghostty_surface_new(state->app, &surface_config);
```
`hitTest: nil` keeps hit-testing in GPUI, forwarding via `ghostty_surface_key`/`_mouse_*`.
Pins gpui to `zed@cc053a4a`, Rust 1.95, **Zig 0.16**, vendored ghostty `9f0e1719`.

Same primitive elsewhere: `CapSoftware/Cap` (`addSubview_positioned_relativeTo`),
`futureboard/futureboard-studio` (VST3 `IPlugView`), `bokuweb/gpui-cef` (tried offscreen CEF → IOSurface →
`gpui::surface()` and hit exactly the NV12 assert above).

Also demonstrated in Zed's own PR thread: @prabirshrestha,
[comment on #61945, 2026-08-05](https://github.com/zed-industries/zed/pull/61945#issuecomment-5187898768):
*"And here is another use case, running ghostty inside gpui showcasing scrolling perf"* + video.

### 5.7 Upstream composition work — all unmerged, do not plan around it

| PR / Discussion | What | Status 2026-09-04 |
|---|---|---|
| [#24327](https://github.com/zed-industries/zed/pull/24327) | `HasWindowHandle for Window` | **merged 2025-02-06** |
| [#60574](https://github.com/zed-industries/zed/pull/60574) | `Application::run_embedded` + `ApplicationHandle` (host owns the runloop; does *not* give an embeddable NSView) | **merged 2026-07-08** |
| [#61945](https://github.com/zed-industries/zed/pull/61945) | `Window::enable_scene_overlay()`, `create_native_surface()`, `PlatformNativeSurface`, `draw_layered`; second transparent `CAMetalLayer`; `native_webview` example | open, dirty, **0 reviews**, last touched 2026-08-05 |
| [#62379](https://github.com/zed-industries/zed/pull/62379) | `CompositionTree`, `CompositionSurfaceId ∈ {Gpui, Native, ExternalGpu}`, +3445/−74 | open, dirty, **0 comments** |
| [#61291](https://github.com/zed-industries/zed/pull/61291) | BGRA `CVPixelBuffer` in `surface()` | open, **0 maintainer comment** |
| [discussion #60572](https://github.com/zed-industries/zed/discussions/60572) / [#60573](https://github.com/zed-industries/zed/pull/60573) | backend-neutral external compositor for wgpu | **closed same day**: *"the implementation is currently ahead of the design conversation"* |
| [#54433](https://github.com/zed-industries/zed/pull/54433) | in-tree wry WebView element | **closed** by @ConradIrwin 2026-04-23: *"disproportionate… the majority of our users are not going to benefit"* |

Verified against `main`: `enable_scene_overlay`, `create_native_surface`, `PlatformNativeSurface`, and
`draw_layered` **do not exist**. Zed's posture is: "here's the window handle, good luck."

### 5.8 libghostty in Sept 2026 — the API split decides this for us

`include/ghostty.h` (1,278 lines) now opens with, verbatim:
> *"Ghostty's internal embedder API, a.k.a. `libghostty-internal`. The only consumer of this API is the
> macOS app, and while it is fairly comprehensive, it is tailored to the needs of the macOS app and
> **not designed for external use**… **External embedders should instead use `libghostty-vt`**…"*

It requires a native view unconditionally:
```c
typedef enum { GHOSTTY_PLATFORM_INVALID, GHOSTTY_PLATFORM_MACOS, GHOSTTY_PLATFORM_IOS } ghostty_platform_e;
typedef struct { void* nsview; } ghostty_platform_macos_s;
typedef struct { ghostty_platform_e platform_tag; ghostty_platform_u platform; void* userdata;
                 double scale_factor; float font_size; const char* working_directory;
                 const char* command; … } ghostty_surface_config_s;
```
`ghostty_surface_new()` fails with `error.NSViewMustBeSet` otherwise (`src/apprt/embedded.zig` L382-435).
`ghostty_surface_draw()` takes **no** output target — ghostty owns the pixels. No Windows/X11/offscreen
variant upstream.

**`include/ghostty/vt.h` + 33 headers = libghostty-vt**, the sanctioned path: `terminal.h` (87 KB),
`selection.h` (43 KB), **`render.h` (33 KB)**, `kitty_graphics.h`, `search.h`, `snapshot.h`, `key.h`, `mouse.h`.
`render.h` is built exactly for our case:
> *"Render state for creating high performance renderers… 1. Create an empty render state 2. Update it
> from a terminal instance whenever you need. 3. Read from the render state to get the data needed to
> draw your frame."*

Two-phase `begin_update`/`end_update` so a render thread holds the terminal lock only briefly; two-layer
dirty tracking (global + per-row); per-cell `RAW`, `STYLE`, `GRAPHEMES_UTF8`, `FG_COLOR`, `BG_COLOR`,
`SELECTED`; cursor style/visibility/blink; 256-entry palette.

**Stability:** `vt.h` says *"WARNING: This is an incomplete, work-in-progress API. It is not yet stable and
is definitely going to change."* [1.3.0 release notes](https://ghostty.org/docs/install/release-notes/1-3-0)
(2026-03-09): *"We aren't ready to tag a versioned release… dozens of projects both free and commercial are
already using libghostty… We aren't sure yet when we'll tag the first libghostty releases."*
Latest tag **v1.3.1 (2026-03-13)**; `main` is `1.3.2-dev`.

**Rust bindings (none official — `ghostty-org` has no Rust repo):**
| Crate | Version / date | Notes |
|---|---|---|
| **`libghostty-vt` + `libghostty-vt-sys`** | **0.2.1, 2026-07-18** | [uzaaft/libghostty-rs](https://github.com/uzaaft/libghostty-rs), 382★, active 2026-09-02, ~96k/33k downloads. De-facto canonical. Safe `Terminal`, `RenderState`, `KeyEncoder`, `MouseEncoder`. **Requires Zig 0.16 on PATH**; `build.rs` clones ghostty at pin `22d1317` unless `GHOSTTY_SOURCE_DIR` is set. |
| `gpui-libghostty` | 0.2.1, 2026-09-02 | the renderer-owning NSView path |
| `gpui-ghostty` | 0.0.1 | placeholder |
| `libghostty` | 0.1.0, 2024-12-24 | dead name-squat |

**Zed itself uses Alacritty, not Ghostty** (workspace `Cargo.toml:521`):
```toml
alacritty_terminal = { git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f" }
vte = "0.15.0"    # feature "ansi"
```
Zero ghostty references in the Zed tree.

### 5.9 The three options, ranked

| | Approach | Works today? | Cost |
|---|---|---|---|
| **A ✅** | **libghostty-vt (or `alacritty_terminal`) + paint the grid yourself in GPUI** | Yes | You own font shaping/ligatures/Kitty graphics. But clipping, rounded corners, popovers, tooltips, animation, scroll sync, and Linux/Windows all just work. **This is what Ghostty upstream tells external embedders to do.** Precedents: [`Xuanwo/gpui-ghostty`](https://github.com/Xuanwo/gpui-ghostty) (86★, *"GPUI, with a custom renderer (no Ghostty renderer reuse)"*), [`k4ditano/k4term`](https://github.com/k4ditano/k4term), `ratatui-ghostty`, `egui_tty`. |
| **B ⚠️** | Host libghostty's own Metal NSView via `HasWindowHandle` + `addSubview:positioned:relativeTo:` | Yes, proven | The native view is **outside GPUI's scene**: never clipped by masks/rounded corners/`overflow`, not synced to GPUI animation or scrolling, and either covers all GPUI overlays (if above) or needs a transparent GPUI root + transparent window (if below — fragile on older macOS). macOS/iOS only. |
| **C ❌** | Offscreen render → IOSurface → `gpui::surface()` | **No** | `draw_surfaces` `assert_eq!`s on non-NV12 and aborts. Needs [PR #61291](https://github.com/zed-industries/zed/pull/61291) or a local gpui patch. This is the wall `bokuweb/gpui-cef` hit. |


---

## 6. Build & packaging

### 6.1 `cargo run` works directly — no bundle required **[VERIFIED LOCALLY]**

All three test apps were launched straight from `target/release/<bin>` with no `.app` wrapper.
The window opens, the process stays alive with a live NSApplication run loop (~57 MB RSS).

The one thing you must do is call **`cx.activate(true)`** in the `run` closure, otherwise the window
opens behind whatever has focus (see §7).

A bundle buys you: a real Dock icon and app name, a proper menu-bar title, `LSMinimumSystemVersion`,
`open -a`, Launch Services registration, file-type/URL-scheme handlers, entitlements, and notarisation.
For day-to-day development, `cargo run` is enough.

### 6.2 Making a `.app` **[VERIFIED LOCALLY — this exact script produces a bundle that launches via `open`]**

```sh
#!/usr/bin/env bash
set -euo pipefail
NAME="MyApp"
BIN="target/release/myapp"
APP="target/$NAME.app"

cargo build --release          # or --profile dist, see 6.4

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$NAME"
# cp assets/app.icns "$APP/Contents/Resources/app.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>                        <string>MyApp</string>
  <key>CFBundleDisplayName</key>                 <string>MyApp</string>
  <key>CFBundleExecutable</key>                  <string>MyApp</string>
  <key>CFBundleIdentifier</key>                  <string>dev.example.myapp</string>
  <key>CFBundlePackageType</key>                 <string>APPL</string>
  <key>CFBundleShortVersionString</key>          <string>0.1.0</string>
  <key>CFBundleVersion</key>                     <string>1</string>
  <key>CFBundleInfoDictionaryVersion</key>       <string>6.0</string>
  <key>LSMinimumSystemVersion</key>              <string>12.0</string>
  <key>NSHighResolutionCapable</key>             <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>CFBundleIconFile</key>                    <string>app.icns</string>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"     # ad-hoc; enough for local use
open "$APP"
```

Measured result on this machine:
```
$ codesign -dv MyApp.app
Identifier=dev.example.myapp
Format=app bundle with Mach-O thin (arm64)
CodeDirectory v=20400 size=10346 flags=0x2(adhoc) hashes=317+3 location=embedded
Signature=adhoc
Info.plist entries=12
$ open MyApp.app     ->  process runs, window appears
```
Bundle size 5.0 MB (same as the binary; `strip` brings it to 4.1 MB, the `dist` profile to 2.9 MB).

**Code signing:** on Apple Silicon every Mach-O must carry *at least* an ad-hoc signature, and the linker
already applies one — the binary ran unsigned-by-you. `codesign --force --deep --sign -` is enough for
local development and for handing the `.app` to a colleague who will right-click → Open. A Developer ID
signature + notarisation is only needed for frictionless distribution.

**Icon:** produce `app.icns` from a 1024×1024 PNG with `iconutil -c icns MyApp.iconset`.

### 6.3 Tooling: prefer the script

- **`cargo-bundle`** — the classic, but it has been effectively unmaintained for years and does not
  handle universal binaries or notarisation. Not recommended.
- **`cargo-packager`**, **`cargo-dist`** — heavier, aimed at multi-platform release automation. Worth
  it later if you ship installers/updates; overkill for a private tool.
- **A 25-line shell script (above)** — what Zed itself effectively does (`script/bundle-mac`), and the
  option with zero dependency risk. **Recommended.**
- `create-dmg` if you eventually want a `.dmg`.

For a universal binary: `cargo build --release --target aarch64-apple-darwin --target x86_64-apple-darwin`
then `lipo -create -output "$APP/Contents/MacOS/$NAME" target/{aarch64,x86_64}-apple-darwin/release/myapp`.

### 6.4 Binary size and startup time **[ALL MEASURED LOCALLY]**

| Configuration | Binary size | Build time (clean) |
|---|---|---|
| `gpui 0.2.2`, default `[profile.release]` | **5,229,632 B ≈ 5.0 MB** | 56 s – 70 s |
| …after `strip` | **4,287,624 B ≈ 4.1 MB** | — |
| …`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"` | **3,069,376 B ≈ 2.9 MB** | 1 m 05 s |
| git dep on zed `v1.18.1` (`main`-era), default release | 8,373,280 B ≈ 8.4 MB | 1 m 30 s |
| `gpui-unofficial 1.18`, default release | 8,468,736 B ≈ 8.5 MB | 53 s (warm cache) |
| `gpui-ce 0.2.2` + `gpui_ce_platform 0.1.0`, default release | 9,400,704 B ≈ 9.4 MB | 58 s |

Suggested shipping profile:
```toml
[profile.dist]
inherits = "release"
lto = "fat"
codegen-units = 1
strip = "symbols"
panic = "abort"
```

**Startup time** — instrumented binary that records `Instant::now()` at the top of `main` and prints on
the first run-loop idle after the window is open (i.e. window created + first frame):

| Run | Time to first idle | Total process wall |
|---|---|---|
| 1st (cold page cache) | **181.3 ms** | 0.52 s |
| 2nd | **161.0 ms** | 0.17 s |
| 3rd | **157.8 ms** | 0.17 s |
| `dist` profile (lto+strip) | **168.6 ms** | — |

So **~160 ms warm from `main()` to a visible window**, ~180 ms cold. LTO does not move startup.
No evidence of a first-launch Metal pipeline-compilation stall in the default (build-time shaders)
configuration — the `.metallib` is compiled by `build.rs` and embedded. **If you enable
`runtime_shaders`, expect to pay shader compilation at every launch instead.**


---

### 6.5 Info.plist keys my minimal plist omits — and which ones matter

Zed does **not** hand-roll a plist; `script/bundle-mac` shells out to a **pinned fork of cargo-bundle**:
```bash
cargo_bundle_version=$(cargo -q bundle --help 2>&1 | head -n 1 || echo "")
if [ "$cargo_bundle_version" != "cargo-bundle v0.6.1-zed" ]; then
    cargo install cargo-bundle --git https://github.com/zed-industries/cargo-bundle.git --branch zed-deploy
fi
```
Inputs live in `crates/zed/Cargo.toml`:
```toml
[package.metadata.bundle-stable]
icon = ["resources/app-icon@2x.png", "resources/app-icon.png"]
identifier = "dev.zed.Zed"
name = "Zed"
osx_minimum_system_version = "10.15.7"
osx_info_plist_exts = ["resources/info/*"]
osx_url_schemes = ["zed"]
```
plus three raw fragments merged from `crates/zed/resources/info/`.

| Key | Do we need it? |
|---|---|
| **`NS*UsageDescription` strings** | **Yes, if you touch any guarded API.** macOS *terminates the process* (a crash, not a denial) when you call one without the matching string. Zed's `Permissions.plist` sets 13: `NSAppleEventsUsageDescription`, `NSSystemAdministrationUsageDescription`, `NSCameraUsageDescription`, `NSMicrophoneUsageDescription`, `NSCalendarsUsageDescription`, `NSContactsUsageDescription`, `NSRemindersUsageDescription`, `NSLocation*UsageDescription`, `NSBluetoothAlwaysUsageDescription`, `NSSpeechRecognitionUsageDescription`, … |
| **`CFBundleURLTypes`** | Only for `myapp://` deep links (Zed gets it from `osx_url_schemes`). Pair with `Application::on_open_urls`. |
| `CFBundleSupportedPlatforms` = `[MacOSX]` | Harmless, add it. |
| `CFBundleDocumentTypes` | Only for Finder "Open With" / drag-onto-icon. Needs a `Document.icns` in `Contents/Resources`. |
| `NSSupportsAutomaticGraphicsSwitching` = true | **Zed does NOT set it and cargo-bundle doesn't emit it.** Free battery life on discrete-GPU Macs — my plist in §6.2 already includes it. |
| `NSPrincipalClass` = `NSApplication` | cargo-bundle doesn't emit it; harmless, some tooling expects it. |
| `LSRequiresCarbon`, `CSResourcesFileMapped` | **Skip** — obsolete cruft that cargo-bundle still emits. |

**Entitlements** — `crates/zed/resources/zed.entitlements` (1001 B), the load-bearing ones for a GPUI app:
```
com.apple.security.cs.allow-jit                        true   <- Metal shader compilation / any JIT
com.apple.security.cs.allow-unsigned-executable-memory true   <- same
com.apple.security.automation.apple-events             true
com.apple.security.files.user-selected.read-write      true   <- open/save panels
com.apple.security.files.downloads.read-write          true
# (Zed also sets device.audio-input, device.camera, and the personal-information.* family —
#  those are Zed-specific. `cs.disable-library-validation` is present but commented out.)
```

**Signing:** Zed signs **bottom-up**, not `--deep`:
```bash
codesign --deep --force --timestamp --options runtime --sign "$IDENTITY" "${app}/Contents/MacOS/cli" -v
codesign --deep --force --timestamp --options runtime --entitlements crates/zed/resources/zed.entitlements \
         --sign "$IDENTITY" "${app}/Contents/MacOS/zed" -v
codesign --force --timestamp --options runtime --entitlements crates/zed/resources/zed.entitlements \
         --sign "$IDENTITY" "${app}" -v
```
`--deep` has been **deprecated for signing since macOS 13** ([TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html),
[briefcase#1221](https://github.com/beeware/briefcase/issues/1221)); the supported form is sign-nested-first-then-bundle.
For a single-binary GPUI app the two are equivalent, which is why my §6.2 script works.
DMG path, for later: `hdiutil create -volname X -srcfolder … -ov -format UDZO` → `dmg-license` →
`notarytool submit --wait` → `stapler staple`.
Release symbols: `dsymutil --flat` **first**, then `strip -x`. Verified on macOS 26 that `strip -x` does
**not** invalidate the ad-hoc signature (Apple's `strip` re-signs); any other post-link mutation does
require a re-`codesign`.

### 6.6 Bundled vs unbundled — the one hard difference

Measured with `lsappinfo`: both are `ApplicationType=Foreground` with a Dock icon, because GPUI sets
`NSApplicationActivationPolicyRegular` itself, unconditionally
(`crates/gpui_macos/src/platform.rs:1288`, `did_finish_launching`) — there is no bundle check anywhere.
The difference is **`CFBundleIdentifier = [ NULL ]` when unbundled**, so everything keyed off bundle id
breaks: TCC/privacy grants, `UserDefaults` suite, `UNUserNotificationCenter`, URL-scheme registration,
"Open With", Sparkle-style updates. Also no icon, no usage-description strings (→ *termination* on
guarded APIs), app-menu title = binary name, and **`App::restart()` cannot work** —
`MacPlatform::app_path()` does `anyhow::ensure!(!bundle.is_null(), "app is not running inside a bundle")`.

On arm64 the **linker already ad-hoc-signs** every binary (`flags=0x20002(adhoc,linker-signed)`), which is
why a bare `cargo run` binary is runnable with no signing step. Signing the *bundle* is what binds
`Info.plist` and creates `_CodeSignature/CodeResources`.

**Bundler tooling status (Sept 2026)** — `cargo-bundle` is maintained again (**0.11.0**, 2026-05-30,
1372★, pushed 2026-08-23), `cargo-packager` 0.11.8, `cargo-dist` 0.32.0. The recommendation still stands:
a GPUI app is one binary with no frameworks or helpers, so ~95% of a bundler's value is dead weight —
and Zed itself concluded upstream cargo-bundle wasn't good enough and **forked it**. Reach for `cargo-dist`
only when you want GitHub Releases + installers.

## 7. Pitfalls

### 7.1 ⚠️ The Metal Toolchain is a separate 688 MB download on macOS 26 **[HIT LOCALLY]**

The single most likely thing to stop an agent dead on a fresh machine. See §1.5.
```
error: cannot execute tool 'metal' due to missing Metal Toolchain;
use: xcodebuild -downloadComponent MetalToolchain
```
surfaces as `cargo::error=metal shader compilation failed:` from `gpui_apple`'s (or `gpui`'s) `build.rs`.
Fix: `xcodebuild -downloadComponent MetalToolchain` (688 MB, ~2 min, no sudo). **Already done on this box.**
Workaround if you can't: the `runtime_shaders` cargo feature — verified working in §1.3.3.

### 7.2 ⚠️ Version numbers lie; the crates.io crate is a year behind the docs

`crates/gpui/Cargo.toml` on `main` says `version = "0.2.2"` — identical to the published crate, but the
code is ~11 months apart. Any agent that reads `gpui.rs`, the GitHub README, `docs.rs/gpui`, or the
`crates/gpui/examples/` directory is reading **`main`-era** API. If the project depends on crates.io
`gpui = "0.2.2"`, roughly one in five snippets will not compile.
**Rule for agents: check whether the project's `Cargo.toml` has a `gpui_platform` dependency.
If yes → `main` era. If no → 0.2.2 era.** Full drift table in §4.13.

Also: **`https://www.gpui.rs/` returns HTTP 403 to automated fetches** (bot protection). Use
<https://lib.rs/crates/gpui> or the raw README on GitHub instead.

### 7.3 ⚠️ `main`-era gpui needs Rust ≥ 1.97 **[HIT LOCALLY]**

On Rust 1.94.1: `error[E0658]: use of unstable library feature 'cold_path'` — `std::hint::cold_path()`
in the scheduler ([rust-lang/rust#136873](https://github.com/rust-lang/rust/issues/136873)).
Zed `main` pins `channel = "1.97.1"`. crates.io `gpui 0.2.2` builds fine on 1.94.1.
Pin it in the project so agents never hit this:
```toml
# rust-toolchain.toml
[toolchain]
channel = "1.97.1"
profile = "minimal"
components = ["rustfmt", "clippy", "rust-analyzer", "rust-src"]
```

### 7.4 ⚠️ `default-features = false` on macOS

`gpui 0.2.2`'s default features are `["font-kit", "wayland", "x11", "windows-manifest"]`. Leaving them on
drags `cosmic-text`, `blade-graphics`, `wayland-client`, `x11rb`, `xkbcommon`, `xim` into a Mac build for
nothing. Always `default-features = false, features = ["font-kit"]`.

### 7.5 ⚠️ `font-kit` is not optional on macOS — without it, text is invisible

From the `main` README: *"Rendering uses Metal and is always available, but glyph rasterization needs
`font-kit`. Without it, GPUI falls back to a placeholder text system that lays text out but renders no
glyphs."* This is the classic "my window is blank / my text doesn't show" report. On `main` the feature
lives on **`gpui_platform`**, not `gpui`.

Conversely: on macOS you do **not** need to register a font to see text.
`crates/gpui/examples/example_support/fonts.rs` looks load-bearing (every example calls
`if !load_fonts(cx) { return; }`) but is `#[cfg(not(target_family = "wasm"))] fn load_fonts(_) -> bool { true }`
— a literal no-op on desktop. The embedded IBM Plex Sans / Lilex path is wasm-only.
To ship a specific terminal font: `cx.text_system().add_fonts(vec![Cow::Borrowed(include_bytes!("…ttf").as_slice())])?`.

### 7.6 Missing assets do **not** panic — they render nothing **[VERIFIED LOCALLY]**

Tested a release app with **no `.with_assets(...)` at all**, rendering
`svg().path("icons/does_not_exist.svg")` and `img("images/missing.png")`:
the app ran for 1.5 s and exited 0. Nothing panicked; the elements simply drew nothing and the sibling
text rendered normally. Reason: the default `impl AssetSource for ()` returns `Ok(None)` / `Ok(vec![])`.

**So the real failure mode is silent, not loud** — an icon that doesn't appear, with no error. Two
corollaries:
1. `svg()` is completely inert until you call `.with_assets(...)` (it also builds `cx.svg_renderer`).
2. If you write an `AssetSource` that does `fs::read(...).unwrap()` (like the gpui `svg` example, which
   uses `?`/`map_err`), *your own* implementation is what will panic or error on a bad path. Make `load`
   return `Ok(None)` for a genuinely absent asset and reserve `Err` for real I/O failures — and log it,
   or you will spend an hour on an invisible icon.

### 7.7 `cx.activate(true)` — call it or the window opens unfocused

Every gpui example ends its `run` closure with `cx.activate(true);`. Without it an unbundled binary
launched from a terminal opens behind the terminal window and does not take keyboard focus. The `true`
means *"ignoring other apps"*. In a `.app` bundle it matters less, but keep it.

### 7.8 Debug builds: the `opt-level` trick

Zed's own `Cargo.toml` uses (verbatim, `https://raw.githubusercontent.com/zed-industries/zed/main/Cargo.toml`):
```toml
[profile.dev]
split-debuginfo = "unpacked"
incremental = true
codegen-units = 16
debug = "limited"

# mirror configuration for crates compiled for the build platform
# (without this cargo will compile ~400 crates twice)
[profile.dev.build-override]
codegen-units = 16
split-debuginfo = "unpacked"
debug = "limited"

[profile.dev.package]
gpui_macros      = { opt-level = 3 }
derive_refineable= { opt-level = 3 }
quote            = { opt-level = 3 }
syn              = { opt-level = 3 }
proc-macro2      = { opt-level = 3 }
tree-sitter      = { opt-level = 3 }
taffy            = { opt-level = 3 }     # <- the layout engine; slow unoptimized
resvg            = { opt-level = 3 }     # <- SVG rasterizer
serde_json       = { opt-level = 3 }
# …plus dozens of `{ codegen-units = 1 }` entries for single-file crates
```
Note `[profile.dev.build-override]` — **without it cargo compiles ~400 crates twice**. That comment is
Zed's own.

For a small app the blunt instrument is:
```toml
[profile.dev.package."*"]
opt-level = 2
```
**[MEASURED LOCALLY]**, clean debug build of the hello-world app on gpui 0.2.2:

| `cargo build` (dev) | Build time | Debug binary |
|---|---|---|
| plain `[profile.dev]` | **36.7 s** | 23,835,128 B ≈ 22.7 MB |
| + `[profile.dev.package."*"] opt-level = 2` | **90.3 s** | 7,842,360 B ≈ 7.5 MB |

So the trick costs ~2.5× on the *first* build (paid once; incremental rebuilds of your own code are
unaffected) and buys a 3× smaller debug binary plus a debug app that isn't unusably slow at layout /
SVG rasterization / text shaping. `taffy` and `resvg` in particular are painful at `opt-level = 0`.
gpui-kit's docs recommend the same thing, per-crate.

**But prefer the targeted form.** Nobody in the ecosystem actually recommends the `"*"` glob for GPUI;
both Zed and gpui-kit list the hot crates explicitly. gpui-kit's
([`Cargo.toml`](https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml)):
```toml
[profile.dev.package]
resvg = { opt-level = 3 }        rustybuzz = { opt-level = 3 }
taffy = { opt-level = 3 }        ttf-parser = { opt-level = 3 }
smol  = { opt-level = 3 }        gpui-pre = { opt-level = 3 }
gpui-pre-platform = { opt-level = 3 }   gpui-pre-macros = { opt-level = 3 }
```
Counter-example worth stealing if binary size ever matters —
[vicanso/zedis](https://github.com/vicanso/zedis/blob/main/Cargo.toml) sets `opt-level = "s"` on `exr`,
`image`, `usvg`, `png`, with a comment that gpui's `image` features drag in codecs you never use.

### 7.9 Compile times, measured **[LOCALLY]**

| Build | Time |
|---|---|
| `cargo build` (debug), gpui 0.2.2, clean | **36.7 s** (22.7 MB binary) |
| `cargo build` (debug) + `[profile.dev.package."*"] opt-level = 2` | 90.3 s (7.5 MB binary) |
| `cargo build --release`, gpui 0.2.2, clean | 56 – 70 s |
| `cargo build --release`, `--profile dist` (lto=fat, cgu=1) | 1 m 05 s |
| `cargo build --release`, git dep on zed v1.18.1, clean | 1 m 30 s |
| incremental rebuild of `src/main.rs` only (release) | **2.3 s** |

This is much better than GPUI's reputation suggests. The horror stories are about building *Zed*
(600+ workspace crates), not about building an app *on* gpui (~600 lockfile entries but a small
compile graph). The one real cost is the first build and the 380 MB `~/.cargo/git` checkout if you
use the git dependency.

### 7.10 `Task` is cancel-on-drop

`cx.spawn(...)` / `background_executor().spawn(...)` return a `Task<T>`. **Dropping it cancels the
future.** Always `.detach()` or store it in your entity. This is the #1 "my async work never runs" bug.

### 7.11 `AsyncApp::update` changed its return type

0.2.2 → `Result<R>`; `main` → `R`. Porting a snippet across this boundary produces either
"cannot use `?` on a non-Result" or "unused `Result` must be used". See §4.4.

### 7.12 `Corner` was renamed to `Anchor`

`anchored().anchor(Corner::TopLeft)` (0.2.2) → `anchored().anchor(Anchor::TopLeft)` (`main`), and
`Anchor` gained `TopCenter`, `BottomCenter`, `LeftCenter`, `RightCenter`. `overlay()` does not exist
in either version — use `deferred(anchored()…)`.

### 7.13 Elements without `track_focus` never receive actions

`on_action` on a `div()` only fires if that element (or an ancestor) is in the focus chain. If you add
`.on_action(...)` and nothing happens, you almost certainly forgot `.track_focus(&self.focus_handle(cx))`
and/or `.key_context("…")`.

### 7.14 Registering two actions with the same name panics at `App` creation

`actions!(term, [Copy])` and `actions!(editor, [Copy])` are fine (different namespaces);
`actions!([Copy])` twice is not. The panic happens during startup, before any window opens.

### 7.15 `crates/ui` is GPL-3.0-or-later

Zed's component crate cannot be vendored into a non-GPL product. See §3.7.

### 7.16 Closing the last window does **not** quit the app **[VERIFIED LOCALLY]**

Tested on gpui 0.2.2: a single-window app whose only window is closed programmatically
(`window.remove_window()`) keeps running indefinitely with zero windows.
```
on_window_closed fired; windows remaining = 0
WINDOW_CLOSED
STILL_ALIVE_1200MS_AFTER_LAST_WINDOW_CLOSED -> app does NOT auto-quit
```
This is correct macOS behaviour (an app with no windows stays in the Dock), but it surprises people
coming from other toolkits, and with no menu bar there is then **no way to quit** — the process must be
killed. Wire it up explicitly:

```rust
cx.on_window_closed(|cx: &mut App| {
    if cx.windows().is_empty() {
        cx.quit();                // or don't, if you want macOS-native behaviour
    }
}).detach();
```
`cx.on_window_closed(...)` returns a `Subscription` — `.detach()` it or it is cancelled immediately.
Also note `Application::run(...)` does **not** return after `cx.quit()`; the statement after `run()`
never executed in the test. Do not put cleanup there.

On `main` (and `gpui-ce` / `gpui-unofficial`, not on crates.io 0.2.2) there is also
`Application::with_quit_mode(...)`, which landed in
[PR #42391](https://github.com/zed-industries/zed/pull/42391) (merged **2025-11-10**, i.e. *after*
0.2.2 was published on 2025-10-22). If you are on crates.io `gpui = "0.2.2"` you must use the
`on_window_closed` pattern above.

### 7.17 Always install a `log` backend

Every soft failure in GPUI — "no text will be rendered", asset load errors, font resolution
fallbacks — is a `log::warn!`/`log::error!` call that goes nowhere without a logger. Add
`env_logger` (or `tracing-subscriber`) and call `env_logger::init()` as the first line of `main()`.
This is the single highest-leverage debugging step for a GPUI app; without it, GPUI fails silently
(see §7.6).


### 7.18 ★ THE BIGGEST TRAP: `gpui_platform`'s `default = []` — no `font-kit`, no glyphs

On `main` / `gpui-ce` / `gpui-unofficial`, the `font-kit` feature lives on **`gpui_platform`**, whose
`default = []`. Omit it and macOS silently swaps in `NoopTextSystem`.

`crates/gpui_macos/src/platform.rs:202`:
```rust
#[cfg(feature = "font-kit")]
let text_system = Arc::new(crate::MacTextSystem::new());

#[cfg(not(feature = "font-kit"))]
let text_system = {
    if !headless {
        log::warn!("gpui_macos was compiled without the `font-kit` feature, so no text will be rendered.");
    }
    Arc::new(gpui::NoopTextSystem::new())
};
```
`NoopTextSystem` (`crates/gpui/src/platform.rs:1113`) returns `Ok(FontId(1))` for any family, fabricates
metrics, and rasterizes empty bitmaps. **`add_fonts()` on it returns `Ok(())` and does nothing** — so
registering fonts *appears to succeed* and still renders nothing.

Symptom: the window opens, backgrounds/borders/boxes paint correctly, layout is correct, **zero glyphs**,
no panic, one `log::warn!` you never see without a logger (§7.17).

**And here is the causal chain that puts people there deliberately:**
- [zed#47168](https://github.com/zed-industries/zed/issues/47168) (2026-01-19) — "gpui fails to build with
  font-kit feature on MacOS due new core-text (v21.1.0)"; two `core_graphics` versions collide.
  **The reporter's workaround was disabling `font-kit`** → straight into the invisible-text bug.
- [zed#43986](https://github.com/zed-industries/zed/issues/43986) (2025-12-02) — "Cannot build gpui crate,
  stand-alone, on macOS, due to core-foundation 0.10.1 dependency conflict in font-kit". Closed as not planned.

Fix: pin/patch `core-foundation`/`core-text` — never drop `font-kit`.

### 7.19 `TextStyle::default()`'s color is **black**

`crates/gpui/src/style.rs:485`:
```rust
impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            color: black(),
            font_family: ".SystemUIFont".into(),
            font_size: rems(1.).into(),      // 16px at the default rem_size
            line_height: phi(),              // relative(1.618034) => ~25.9px
```
Text is present and correctly laid out but invisible on a dark background — **visually identical to
§7.18**. Always set `.text_color(...)` on your root.

### 7.20 Font family remapping: `.ZedSans` / `.ZedMono` silently degrade

`crates/gpui/src/text_system.rs`:
```rust
pub fn font_name_with_fallbacks<'a>(name: &'a str, system: &'a str) -> &'a str {
    match name {
        ".SystemUIFont" => system,                       // macOS passes ".AppleSystemUIFont"
        ".ZedSans" | "Zed Plex Sans" => "IBM Plex Sans",
        ".ZedMono" | "Zed Plex Mono" => "Lilex",
        _ => name,
    }
}
```
`.SystemUIFont` resolves with zero registration. But **IBM Plex Sans and Lilex are not on a stock Mac**,
so copying `.ZedMono` / `Zed Plex Mono` out of Zed's source silently gives you Helvetica. For our
terminal font, `include_bytes!` it and `cx.text_system().add_fonts(...)`.

### 7.21 An unresolvable font is a **hard panic, every frame**

`crates/gpui/src/text_system.rs:148`:
```rust
/// # Panics
/// Panics if the font and none of the fallbacks can be resolved.
pub fn resolve_font(&self, font: &Font) -> FontId { … panic!("failed to resolve font '{}' …") }
```
Fallback stack: `[".ZedMono", ".ZedSans", "Helvetica", "Segoe UI", "Ubuntu", "Adwaita Sans", "Cantarell",
"Noto Sans", "DejaVu Sans", "Arial"]`. Helvetica/Arial always exist on macOS, so this is effectively a
Linux/container problem — but note it's called from `shape_line`/`layout_line`, i.e. **mid-render**, not
at startup.

### 7.22 `svg()` renders nothing without `.text_color(...)`

Separate from the asset question (§7.6): in `crates/gpui/src/elements/svg.rs` every paint path is guarded
by `if let Some(color) = style.text.color`. **An `svg()` whose asset loads perfectly still draws nothing
if no text color is in scope.** Always `svg().path(..).size_4().text_color(cx.theme().foreground)`.

Also, for `img()` the failure *is* logged — `ImageCacheError::Asset("Embedded resource not found: {path}")`
via `AssetLogger` → `log::error!` — whereas `svg()` is completely silent because the default `()` source
returns `Ok(None)` and there is nothing to log. Use `.with_fallback()` / `.with_loading()` on images.

### 7.23 There is exactly one `AssetSource` per app — libraries cannot ship assets

[zed#8713](https://github.com/zed-industries/zed/issues/8713) ("gpui: Defining multiple AssetSources").
This is why `.with_assets(...)` is mandatory boilerplate and why "Icon not found" is the classic
gpui-component newcomer failure. Plan our design-system crate accordingly: it must *expose* its asset
bytes for the app's single `AssetSource` to serve, not register its own.

### 7.24 `env!("CARGO_MANIFEST_DIR")` asset paths die inside a `.app`

Both official examples (`crates/gpui/examples/svg/svg.rs`, `examples/image/image.rs`) build the source as
`base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")`. That absolute path is baked at compile
time and points into your source tree — fine under `cargo run` **on your own machine**, dead on any other
machine and dead in a shipped bundle. Symptom: everything works in dev, all icons vanish in the `.app`.
Fix: `rust-embed` (what gpui-kit and the sheerluck tutorial use), or resolve via `NSBundle` →
`Contents/Resources`.

### 7.25 There is no default menu bar, so ⌘Q does not work

`setMainMenu_` appears exactly once in `crates/gpui_macos/src/platform.rs` — inside `fn set_menus`.
`create_menu_bar` starts from an empty `NSMenu::new(nil)` and appends **only what you pass**; it does not
synthesize an App menu, a Quit item, or an Edit menu. Symptom: only the Apple menu appears, and ⌘Q, ⌘W,
⌘C/⌘V key equivalents are all inert.

Empirical confirmation: [zed#11362](https://github.com/zed-industries/zed/issues/11362) (2024-05-03,
closed not-planned) — `cx.on_action(quit)` + `KeyBinding::new("cmd-q", Quit, None)` stopped quitting and
**only worked once `cx.set_menus()` was added**.

You must do *both* `cx.on_action(quit)` **and** `cx.set_menus(...)` — a `MenuItem::action(Quit)` with no
registered handler is inert. `create_menu_bar` special-cases `if menu_config.name == "Window"` →
`app.setWindowsMenu_(menu)`. Also available: `cx.get_menus()`, `cx.set_dock_menu(...)`. See §4.7 for the
current API shape.

### 7.26 `cx.activate(true)` may not actually raise the window on modern macOS

`cx.activate(ignoring_other_apps)` → `app.activateIgnoringOtherApps_(...)`
(`gpui_macos/src/platform.rs:591`). **29 of 34** top-level gpui examples call it.
But: [zed#37145](https://github.com/zed-industries/zed/issues/37145) (2025-08-29, Sequoia, M2) —
"GPUI activate window function not working". Independently measured on macOS 26.3: **neither** the bundled
nor the unbundled app stole focus from the already-frontmost app. Apple restricted cross-app focus
stealing from Sonoma on ([forums/739524](https://developer.apple.com/forums/thread/739524)).
**Call it, but don't build UX that depends on it.**

### 7.27 `create-gpui-app` is actively harmful

[zed-industries/create-gpui-app](https://github.com/zed-industries/create-gpui-app) — 380★, last push
2025-04-13, marked ⚪ dormant on Zed's own awesome list. Its template emits
`gpui = { git = "https://github.com/zed-industries/zed" }` (resolves to `main`) **plus
`Application::new()`, which no longer exists on `main`**. It also omits fonts, `activate`, and menus.
Do not use it, and tell agents not to.

### 7.28 Dating a tutorial: the rename PRs

Nearly every rename landed in **one** PR:
[**#22632** "Eliminate GPUI View, ViewContext, and WindowContext types"](https://github.com/zed-industries/zed/pull/22632),
merged **2025-01-26** (`6fca1d2b`), +36,251/−28,211 across 648 files.

| Change | Date | PR |
|---|---|---|
| `AppContext` → `App`; `ViewContext<T>`/`ModelContext<T>` → `Context<T>`; `View<T>`+`Model<T>` → `Entity<T>`; `cx.new_view()`/`cx.new_model()` → `cx.new()`; `WindowContext` → `&mut Window, &mut App`; one-arg `render` → `render(&mut self, window, cx)`; `AsyncAppContext` → `AsyncApp`; `gpui::App::new()` → `gpui::Application::new()` | **2025-01-26** | [#22632](https://github.com/zed-industries/zed/pull/22632) |
| missed-rename sweep | 2025-01-26 | [#23688](https://github.com/zed-industries/zed/pull/23688) |
| `model` vars → `entity` | 2025-02-04 | [#24198](https://github.com/zed-industries/zed/pull/24198) |
| `QuitMode` added | **2025-11-10** | [#42391](https://github.com/zed-industries/zed/pull/42391) |
| `Entity<T>: Element` removed (entities go via `AnyElement`) | 2026-02-03 | [#48217](https://github.com/zed-industries/zed/pull/48217) |
| **`Application::new()` → `gpui_platform::application()`; the crate split** | **2026-02-19** | [#49277](https://github.com/zed-industries/zed/pull/49277) (`bc31ad4a`) |
| `gpui_web` added (wasm) | 2026-02-26 | [#50228](https://github.com/zed-industries/zed/pull/50228) |
| `gpui_apple` split out of `gpui_macos` | 2026-08-14 | [#62649](https://github.com/zed-industries/zed/pull/62649) |

**Telltales for an agent reading a snippet:**
- `ViewContext` / `WindowContext` / `cx.new_view` / one-arg `render` → **pre-2025-01-26**, dead everywhere.
- `Application::new()` → **2025-01-26 … 2026-02-19**. This is what crates.io 0.2.2, `docs.rs/gpui`, and
  `https://www.gpui.rs/` **still show today**.
- `gpui_platform::application()` → post-2026-02-19. ← what we use.
- `.child(some_entity)` directly → pre-2026-02-03.

**There is no migration guide and no release notes.** PR #22632's Release Notes field is "N/A"; Zed's GPUI
blog tag stops at 2024-01; `crates/gpui/docs/` on `main` holds only `contexts.md` and `key_dispatch.md`.

---

## 7.29 Resources, with currency (Sept 2026)

| Resource | State |
|---|---|
| [zed-industries/awesome-gpui](https://github.com/zed-industries/awesome-gpui) (1,245★, pushed 2026-09-04) | **CURRENT — start here.** Per-project 🟢/🟡/⚪ staleness badges over ~90 apps/libs. Flags `create-gpui-app` ⚪ dormant. |
| [`crates/gpui/README.md`](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md) + `examples/README.md` | **CURRENT and the single most useful doc** — the only place the `font-kit`/no-glyphs trap is written down. |
| [blog.sheerluck.dev "Your First GPUI App"](https://blog.sheerluck.dev/posts/your-first-gpui-app-building-a-desktop-ui-in-rust/) (**2026-08-30**) | **CURRENT.** `cx.new()`, `Context<Self>`, `rust-embed` + `AssetSource`. Doesn't cover activate/menus/quit. |
| [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit), docs <https://gpui-kit.com/docs/getting-started/> | **CURRENT.** Best real-world reference for asset + profile setup. Not a dependency for us (§3.2). |
| [dev.to "Building a Desktop Time App in Rust with GPUI"](https://dev.to/dev-tngsh/building-a-desktop-time-app-in-rust-with-gpui-a-beginners-journey-181m) (2026-01-01) | Semi-stale API; the only source with concrete cost data (133 h over 6 weeks; *"compilation takes 10+ minutes"* first build). |
| [zed-industries/create-gpui-app](https://github.com/zed-industries/create-gpui-app) | **STALE / HARMFUL** — §7.27. |
| <https://www.gpui.rs/> | **STALE** — front-page Hello World uses `Application::new()`, omits `cx.activate`, won't compile against `main`. Also **HTTP 403 to automated fetches**. |
| [MatinAniss/gpui-book](https://matinaniss.github.io/gpui-book/) | ⚪ dormant. Stale for `main`; still valid against crates.io 0.2.2. |
| [medium: "GPUI: Handling macOS Updates"](https://medium.com/rustaceans/gpui-handling-macos-updates-b99f2a05698b) | Real-world `MTLCompilerService` / render-pipeline crash-on-launch after macOS updates. **Read before shipping.** |

---

# DECISIONS FOR THE TEAM

Everything below is verified on this machine unless noted. Copy it verbatim.

## D1. Toolchain — pin it

Create `rust-toolchain.toml` at the repo root so no agent ever debugs a toolchain error:

```toml
[toolchain]
channel = "1.97.1"
profile = "minimal"
components = ["rustfmt", "clippy", "rust-analyzer", "rust-src"]
```
Already installed here (`rustup toolchain install 1.97.1 --profile minimal` → `1.97.1 (8bab26f4f 2026-07-14)`).
This is Zed `main`'s own pin. **Rust 1.94.1 is NOT enough** for `main`-era gpui (§7.3).

**One-time system prerequisite (already done on this box):**
```sh
xcodebuild -downloadComponent MetalToolchain     # 688 MB, ~2 min, no sudo
xcrun -sdk macosx metal --version                # must print "Apple metal version …"
```

## D2. Dependency — git dep on Zed, pinned to tag `v1.18.1`

```toml
[dependencies]
gpui          = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", default-features = false }
gpui_platform = { git = "https://github.com/zed-industries/zed", tag = "v1.18.1", features = ["font-kit"] }

# terminal emulation (see D4)
alacritty_terminal = "0.26.0"

# assets (icons/fonts) — embed, so it works inside a .app
rust-embed = "8"
anyhow  = "1"
log     = "0.4"
env_logger = "0.11"
raw-window-handle = "0.6"   # only if you ever need the NSView; see §5.3
```

**Why this and not the alternatives** (all four were built and run here):

| Option | Verdict |
|---|---|
| **git dep on zed `v1.18.1`** ✅ | **Chosen.** Official Apache-2.0 source, no third-party publisher, exact reproducible pin. Decisively: its API matches *every* current reference an agent will read — the `main` README, `docs.rs`, `crates/gpui/examples/*`, and above all **`crates/terminal_view/src/terminal_element.rs`**, which is our blueprint (§4.8). Upgrading is a one-line tag bump with a readable diff. |
| crates.io `gpui = "0.2.2"` ❌ | Official and the lightest (5.0 MB, 70 s, builds on 1.94.1) — but **11 months stale**. `Application::new()` instead of `gpui_platform::application()`, `AsyncApp::update -> Result<R>`, `Corner` instead of `Anchor`, old `ShapedLine::paint` signature, no `QuitMode`. Agents will keep generating `main`-era code that doesn't compile against it (§4.13). Not worth the 3 MB. |
| `gpui-unofficial = "1.18"` 🟡 | **Viable fallback.** Identical `main`-era API — switching is a `Cargo.toml`-only change. Take it if the 380 MB git checkout hurts in CI. Cost: a third-party automated republish that, by its own README, cannot patch a released version. |
| `gpui-ce` 🟡 | Real, active (1,007★, pushed today, Apache-2.0), and it's the only fork with a *compatible* component library. But its README says *"mostly API compatible, but this is changing!"*, its versioning is incoherent (0.3.x yanked, back to 0.2.2), and its founder has publicly said he's more interested in a different framework. Revisit in 6 months. |
| `gpui-component` / `gpui-kit` ❌ | **Structurally incompatible** with the choice above — it pins `gpui-pre`, a *fourth* republish, and two gpui copies cannot interoperate (§3.2). |

One-time costs already paid here: 380 MB in `~/.cargo/git/db/zed-*`, and the Metal Toolchain.

## D3. Build profiles

```toml
# Debug: optimize only the hot dependencies. This is what Zed and gpui-kit both do.
# (The blanket `[profile.dev.package."*"] opt-level = 2` also works, but I measured it at
#  90 s vs 37 s for the first build; targeted is the better trade.)
[profile.dev.package]
taffy      = { opt-level = 3 }   # layout engine — painful at opt-level 0
resvg      = { opt-level = 3 }   # SVG rasterizer
rustybuzz  = { opt-level = 3 }   # text shaping
ttf-parser = { opt-level = 3 }
smol       = { opt-level = 3 }
gpui       = { opt-level = 3 }
gpui_platform = { opt-level = 3 }
gpui_macros   = { opt-level = 3 }
syn = { opt-level = 3 }
quote = { opt-level = 3 }
proc-macro2 = { opt-level = 3 }

# Without this cargo compiles ~400 crates twice (Zed's own comment).
[profile.dev.build-override]
codegen-units = 16
split-debuginfo = "unpacked"
debug = "limited"

# Shipping: 8.4 MB -> ~3 MB, startup unchanged.
[profile.dist]
inherits = "release"
lto = "fat"
codegen-units = 1
strip = "symbols"
panic = "abort"          # verified to work with GPUI
```

## D4. Terminal rendering — **self-painted grid, not native-view hosting**

**Decision: parse with `alacritty_terminal` 0.26.0 and paint the grid ourselves as a custom `impl Element`.**

Why not host libghostty's own Metal `NSView` (which *does* work — §5.6):
- The native view lives **outside GPUI's scene**. It is never clipped by masks, rounded corners, or
  `overflow`; it is not synced to GPUI animation or scrolling; and it either covers every GPUI overlay
  (popovers, tooltips, command palette) or requires a fully transparent GPUI root + transparent window
  (con-terminal's hole-punch, which their own postmortem calls fragile on older macOS).
- For a project whose whole point is *a custom, reusable design system*, an unclippable opaque rectangle
  that our own overlays can't draw over is disqualifying.
- macOS/iOS only upstream; `ghostty_platform_e` has no Windows or Linux variant.
- And Ghostty upstream says so themselves: `include/ghostty.h` — *"not designed for external use…
  External embedders should instead use `libghostty-vt`."*

Why `alacritty_terminal` over `libghostty-vt`:
- Pure Rust. **`libghostty-vt` 0.2.1 requires Zig 0.16 on `PATH`** and clones ghostty in its `build.rs`.
- **It is exactly what Zed's `terminal_element.rs` is written against**, so that 3,005-line file becomes a
  near drop-in blueprint rather than a loose analogy. That is worth a lot of agent-hours.
- crates.io `alacritty_terminal` **0.26.0** (2026-04-06, 1.38M downloads). Zed pins its own fork
  (`git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f"`);
  start with the crates.io release and only fork if we hit a real gap.
- Keep `libghostty-vt` 0.2.1 (+ its `render.h` dirty-row model) as a documented future swap if we ever
  want Ghostty's VT fidelity and are willing to add Zig to the build.

**Implementation shape** (port of §4.8):
```
impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;   // merged runs + merged bg rects + cursor
}
```
1. `prepaint`: cell_width = `cx.text_system().advance(font_id, font_px, 'm').width`;
   line_height = `font_px * multiplier`, snapped to whole device pixels.
2. Walk the grid once, producing (a) `BatchedTextRun`s — adjacent cells merged while
   `font/color/background/underline/strikethrough` are equal *and* columns are contiguous — and
   (b) background `LayoutRect`s, merged horizontally then coalesced vertically.
3. `paint`, inside `window.with_content_mask(Some(ContentMask { bounds }), …)`:
   whole-element `fill` → background rects (`floor` the x, `ceil` the width, or you get seams) →
   `window.text_system().shape_line(text, size, &[run], Some(cell_width)).paint(pos, line_height, TextAlign::Left, None, window, cx)`
   → block glyphs as quads → cursor (`fill`, or `outline` when unfocused).
4. Own the scroll offset; translate + device-pixel-snap the paint origin. Do **not** use
   `uniform_list`/`list` — one element per row is far too much overhead.
5. **Do not hand-roll `window.paint_glyph`.** `shape_line(..., force_width: Some(cell_width))` +
   `ShapedLine::paint` is both faster (there's a `LineLayout` cache) and correct for ligatures,
   combining marks, and wide characters.

## D5. Design system — build our own; do not depend on `gpui-component`

See §3.8. Keep a read-only checkout of <https://github.com/longbridge/gpui-kit> and mine exactly four files:

| File | Take |
|---|---|
| `crates/base/src/theme_tokens.rs` | `SemanticThemeTokens { colors(18 roles), radius, spacing, typography, shadow }` |
| `crates/component/src/theme/mod.rs` | `trait ActiveTheme { fn theme(&self) -> &Theme }` + `impl Global for Theme` + `Deref<Target = ThemeColor>` → `cx.theme().primary` |
| `crates/component/src/theme/motion.rs` | duration 120/180/280 ms, `cubic_bezier` enter/exit/move, spring params — copy verbatim |
| `crates/component/src/icon.rs` + `crates/assets/src/native_assets.rs` | `rust-embed` `AssetSource` + typed icon enum + `svg().path(..)` |

Do **not** vendor Zed's `crates/ui` — it is **GPL-3.0-or-later** (§3.7).

## D6. Non-negotiable boilerplate in `main()`

```rust
fn main() {
    env_logger::init();                       // §7.17 — GPUI fails SILENTLY without this
    gpui_platform::application()
        .with_assets(Assets)                  // §4.11 — svg() is inert without it
        .run(|cx: &mut App| {
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.set_menus([...]);              // §7 — cmd-Q is unreliable without a menu (zed#11362)
            cx.open_window(WindowOptions { /* titlebar.appears_transparent = true */ ..Default::default() },
                           |_, cx| cx.new(|cx| Root::new(cx))).unwrap();
            cx.activate(true);                // §7.7 — or the window opens unfocused
            cx.on_window_closed(|cx| if cx.windows().is_empty() { cx.quit() }).detach();  // §7.16
        });
}
```

## D7. Packaging

`cargo run` works unbundled (§6.1). For a `.app`, use the 25-line script in §6.2 — not `cargo-bundle`
(unmaintained). `codesign --force --deep --sign -` is enough locally. Verified: the bundle launches
via `open`.

## D8. Reference checkout for agents

After the first `cargo build`, the pinned Zed source is on disk at
`~/.cargo/git/checkouts/zed-<hash>/<rev>/`. Point agents at these files rather than at web tutorials:

| Need | File |
|---|---|
| Terminal grid painting | `crates/terminal_view/src/terminal_element.rs` |
| Minimal custom `Element` | `crates/gpui/examples/input.rs` (L422-580) |
| Actions / keybindings / focus | `crates/gpui/examples/input.rs` |
| Menus | `crates/gpui/examples/set_menus.rs` |
| Low-level painting | `crates/gpui/examples/painting.rs` |
| Overlays | `crates/gpui/examples/anchor.rs`, `popover.rs` |
| Virtual lists | `crates/gpui/examples/uniform_list.rs`, `list_example.rs` |
| `AssetSource` | `crates/gpui/examples/svg/svg.rs`, `crates/assets/src/assets.rs` |
| Entities/Context prose | `crates/gpui/src/_ownership_and_data_flow.rs` |
| Quad/text primitives | `crates/gpui/src/window.rs`, `crates/gpui/src/text_system/line.rs` |

**Rule for agents: any snippet that uses `Application::new()`, `ViewContext`, `AppContext` as a type,
`View<T>`, `cx.new_view`, `Corner::`, or `overlay()` is stale — see §4.13 before copying anything.**
