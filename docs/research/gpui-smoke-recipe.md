# GPUI 0.2.2 smoke-test recipe (macOS)

Verified 2026-09-04 on macOS 26.3 (25D2125), arm64, with `rustc 1.94.1` / `cargo 1.94.1` from `~/.cargo/bin`.

## Dependency choice

Commands checked:

```sh
~/.cargo/bin/cargo search gpui --limit 10
~/.cargo/bin/cargo info gpui
```

Both report Zed's official `gpui` 0.2.2 as the latest crates.io release. Docs.rs dates 0.2.2 to 2025-10-22, and the Zed repository's `main` branch still declares `crates/gpui` version 0.2.2 on 2026-09-04. Because the published crate is the current declared version, contains the current API requested here, and builds and launches successfully, use crates.io rather than coupling the smoke test to the much larger Zed git workspace or an unstable main revision. `Cargo.lock` resolves exactly 0.2.2.

## Project

Project directory:

```text
/private/tmp/claude-501/-Users-danny--swarm-worktrees-dannyfuf-fleetd-setup/8d260d57-60e7-49da-8183-bc4465483b3d/scratchpad/gpui-smoke
```

Final `Cargo.toml`:

```toml
[package]
name = "gpui-smoke"
version = "0.1.0"
edition = "2024"

[dependencies]
gpui = "0.2.2"
```

Final `src/main.rs`:

```rust
use gpui::{
    App, Application, Context, IntoElement, Render, Window, WindowOptions, div, prelude::*,
};

struct HelloView;

impl Render for HelloView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child("hello").child(div())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.open_window(WindowOptions::default(), |_window, cx| {
            cx.new(|_cx| HelloView)
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
```

## Build

Exact normal build command:

```sh
cd /private/tmp/claude-501/-Users-danny--swarm-worktrees-dannyfuf-fleetd-setup/8d260d57-60e7-49da-8183-bc4465483b3d/scratchpad/gpui-smoke
~/.cargo/bin/cargo build
```

Timed commands used `/usr/bin/time -p ~/.cargo/bin/cargo build`. From an empty local target, dependencies and GPUI compiled in 33.57 s before the app hit two missing-trait-import errors; adding the official example's `gpui::prelude::*` import made the incremental build succeed in 1.11 s. Total wall time from empty target to first successful binary was 34.68 s. After `cargo fmt`, the final app-only rebuild succeeded in 1.46 s (`cargo` reported 1.43 s). A final plain `cargo build` also exited 0.

The only Rust compile errors were `Div::child` needing `ParentElement` and `App::new` needing `AppContext`. Both traits are exported by `gpui::prelude::*`, exactly as used by `gpui-0.2.2/examples/hello_world.rs`.

### System dependencies and Metal

This machine already had full Xcode selected at `/Applications/Xcode.app/Contents/Developer`, Apple clang 21, the macOS SDK, libclang needed by bindgen, and the downloadable Metal toolchain. No packages or features had to be added. SDK-qualified probes succeeded:

```text
xcrun -sdk macosx -f metallib
/var/run/com.apple.security.cryptexd/mnt/com.apple.MobileAsset.MetalToolchain-v17.6.109.0.n2UL3o/Metal.xctoolchain/usr/bin/metallib
xcrun -sdk macosx metal --version
Apple metal version 32023.883 (metalfe-32023.883)
xcrun -sdk macosx metallib --version
AIR-LLD 32023.883 (metalfe-32023.883) (compatible with legacy metallib linker)
```

An unqualified `xcrun --find metallib` misleadingly failed, but GPUI's build script uses `xcrun -sdk macosx metallib`, which succeeded. Therefore `runtime_shaders` was not enabled and no Metal fix was applied. If an SDK-qualified `xcrun -sdk macosx metal --version` actually fails, install/select Xcode and its Metal toolchain (`xcode-select --install`, select the intended Xcode with `xcode-select`, and let Xcode install its Metal component), or avoid the build-time Metal compiler with:

```toml
gpui = { version = "0.2.2", features = ["runtime_shaders"] }
```

That feature stitches the `.metal` source at build time and compiles it through the Metal API at runtime (`build.rs:72-75,175-193`; `src/platform/mac/metal_renderer.rs:33-36,156-163`).

## Launch probe

Equivalent command used (stdout and stderr were captured separately):

```sh
./target/debug/gpui-smoke >launch.stdout 2>launch.stderr &
gpui_pid=$!
sleep 5
kill "$gpui_pid"
wait "$gpui_pid"
```

Result: the process was alive after five seconds, then exited 143 from the intentional SIGTERM. `launch.stdout` was empty. `launch.stderr` contained only host LaunchServices/XPC diagnostics, with no Rust panic, missing asset, font, or Metal error:

```text
2026-09-04 15:01:19.098 gpui-smoke[25336:83555033] Error received in message reply handler: Connection invalid
2026-09-04 15:01:19.098 gpui-smoke[25336:83555387] Connection Invalid error for service com.apple.hiservices-xpcservice.
2026-09-04 15:01:19.119 gpui-smoke[25336:83555033] Failure on line 688 in function id scheduleApplicationNotification(LSNotificationCode, NSWorkspaceNotificationCenter *): noErr == _LSModifyNotification(notificationID, 1, &code, 0, NULL, NULL, NULL)
```

The command runner separately printed `nice(5) failed: operation not permitted`; that sandbox warning was not emitted by the app and is not in `launch.stderr`.

## API verification against the exact registry source

Source root: `/Users/danny/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-0.2.2`.

| API | Exists? | Exact signature and source |
|---|---:|---|
| `svg()` and embedded assets | Yes | `pub fn svg() -> Svg`; `Svg::path(mut self, path: impl Into<SharedString>) -> Self` — `src/elements/svg.rs:18,28`. `pub trait AssetSource: 'static + Send + Sync { fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>; fn list(&self, path: &str) -> Result<Vec<SharedString>>; }` — `src/assets.rs:13-19`. Register an embedded implementation with `Application::with_assets(self, asset_source: impl AssetSource) -> Self` — `src/app.rs:155-162`. |
| `canvas()` paint callback | Yes | `pub fn canvas<T>(prepaint: impl 'static + FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> T, paint: impl 'static + FnOnce(Bounds<Pixels>, T, &mut Window, &mut App)) -> Canvas<T>` — `src/elements/canvas.rs:10-19`. |
| `window.text_system().shape_line(...)` | Yes | `Window::text_system(&self) -> &Arc<WindowTextSystem>` — `src/window.rs:1435-1437`; `WindowTextSystem::shape_line(&self, text: SharedString, font_size: Pixels, runs: &[TextRun], force_width: Option<Pixels>) -> ShapedLine` — `src/text_system.rs:365-371`. |
| `uniform_list` | Yes | `pub fn uniform_list<R>(id: impl Into<ElementId>, item_count: usize, f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>) -> UniformList where R: IntoElement` — `src/elements/uniform_list.rs:22-29`. |
| `deferred` / `anchored` | Yes | `pub fn deferred(child: impl IntoElement) -> Deferred` — `src/elements/deferred.rs:7-12`; `pub fn anchored() -> Anchored` — `src/elements/anchored.rs:27-36`. |
| `KeyBinding::new` / `cx.bind_keys` | Yes | `pub fn new<A: Action>(keystrokes: &str, action: A, context: Option<&str>) -> Self` — `src/keymap/binding.rs:33-45`; `App::bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>)` — `src/app.rs:1677-1680`. |
| `actions!` | Yes | Exported `macro_rules! actions` accepts `($namespace:path, [$(... $name:ident),*])` or `([$(... $name:ident),*])` — `src/action.rs:23-40`. |
| `key_context` | Yes | `InteractiveElement::key_context<C, E>(mut self, key_context: C) -> Self where C: TryInto<KeyContext, Error = E>, E: Debug` — `src/elements/div.rs:658-667`. |
| `cx.background_executor().spawn` | Yes | `App::background_executor(&self) -> &BackgroundExecutor` — `src/app.rs:1402-1404`; `BackgroundExecutor::spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R> where R: Send + 'static` — `src/executor.rs:145-150`. `AsyncApp` exposes the same executor accessor at `src/app/async_context.rs:132-134`. |
| `cx.spawn` / `AsyncApp` | Yes | `App::spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R> where AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static, R: 'static` — `src/app.rs:1417-1430`. Entity `Context<T>::spawn` passes `(WeakEntity<T>, &mut AsyncApp)` — `src/app/context.rs:237-245`. `pub struct AsyncApp` — `src/app/async_context.rs:17-21`. |
| `cx.observe` / `cx.subscribe` / `EventEmitter` | Yes | `Context<T>::observe<W>(&mut self, entity: &Entity<W>, on_notify: impl FnMut(&mut T, Entity<W>, &mut Context<T>) + 'static) -> Subscription` — `src/app/context.rs:63-81`; `Context<T>::subscribe<T2, Evt>(&mut self, entity: &Entity<T2>, on_event: impl FnMut(&mut T, Entity<T2>, &Evt, &mut Context<T>) + 'static) -> Subscription where T2: 'static + EventEmitter<Evt>` — `src/app/context.rs:98-117`; `pub trait EventEmitter<E: Any>: 'static {}` — `src/gpui.rs:237`. App-level forms are at `src/app.rs:780-792,865-878`. |
| `FocusHandle` / `track_focus` | Yes | `pub struct FocusHandle` — `src/window.rs:266-273`; create via `App::focus_handle(&self) -> FocusHandle` — `src/app.rs:2029-2031`; `InteractiveElement::track_focus(mut self, focus_handle: &FocusHandle) -> Self` — `src/elements/div.rs:616-620`. |
| `Entity<T>` | Yes | `pub struct Entity<T>` (strong typed GPUI-managed reference) — `src/app/entity_map.rs:374-382`. |
| `cx.new` | Yes | `AppContext::new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Self::Result<Entity<T>>` — `src/gpui.rs:115-128`; for `App`, `type Result<T> = T`, so `App::new` returns `Entity<T>` — `src/app.rs:2106-2118`. |
| `WindowOptions`, titlebar, kind, bounds | Yes | `WindowOptions` has `pub window_bounds: Option<WindowBounds>`, `pub titlebar: Option<TitlebarOptions>`, and `pub kind: WindowKind` — `src/platform.rs:1089-1105`. `TitlebarOptions { title: Option<SharedString>, appears_transparent: bool, traffic_light_position: Option<Point<Pixels>> }` — `src/platform.rs:1248-1258`. `WindowKind::{Normal, PopUp, Floating}` — `src/platform.rs:1262-1272`. `WindowBounds::{Windowed(Bounds<Pixels>), Maximized(Bounds<Pixels>), Fullscreen(Bounds<Pixels>)}` — `src/platform.rs:1188-1197`. |
| Raw/native macOS handles and native view hosting | Partial | Public `Window` implements `raw_window_handle::HasWindowHandle`: `fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, HandleError>` — `src/window.rs:4845-4848`. The macOS implementation returns `RawWindowHandle::AppKit(AppKitWindowHandle::new(native_view))`, i.e. an `NSView` pointer — `src/platform/mac/window.rs:1548-1555`. There is no public direct `NSWindow` getter or native-child-view hosting API. `PlatformWindow` is `pub(crate)` (`src/platform.rs:460`), `MacWindow` is `pub(crate)` (`src/platform/mac/window.rs:568`), and its `native_window`/`native_view` fields are private (`src/platform/mac/window.rs:388-393`). A consumer can unsafely extract the raw AppKit `NSView` and ask it for its `window`, then manually attach native subviews, but GPUI provides no supported lifecycle/layout wrapper for that. |

## Reusable artifacts

The compiled dependency cache is intentionally left at:

```text
/private/tmp/claude-501/-Users-danny--swarm-worktrees-dannyfuf-fleetd-setup/8d260d57-60e7-49da-8183-bc4465483b3d/scratchpad/gpui-smoke/target
```

Later agents can set `CARGO_TARGET_DIR` to that exact directory. Build and launch logs are beside the crate source: `build-output.log`, `build-success.log`, `build-final.log`, `launch.stdout`, `launch.stderr`, and `launch-result.txt`.
