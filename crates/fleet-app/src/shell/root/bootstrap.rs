use super::Shell;
use crate::{actions::fleet, drive, keymap};
use fleet_ui_kit::{ActiveTheme, KitAssets, Theme, ThemeMode};
use gpui::{
    App, Bounds, Menu, MenuItem, SharedString, TitlebarOptions, WindowBounds, WindowOptions,
    prelude::*, px, size,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);
const MIN_SIZE: (f32, f32) = (900.0, 560.0);

/// `$FLEET_HOME`, or `~/.fleet`.
///
/// Resolved through `fleet_core` so a leading `~` expands exactly as `fleetd` and `fleet` expand
/// it. Falls back to a relative `.fleet` when `$HOME` is unset, as an app has nowhere to report
/// that failure before its window exists.
#[must_use]
fn fleet_home() -> PathBuf {
    let selected = std::env::var_os("FLEET_HOME").map(PathBuf::from);
    fleet_core::paths::resolve_home_with(selected, crate::presentation::home_dir())
        .unwrap_or_else(|_| PathBuf::from(".fleet"))
}

/// Installs the process-wide `tracing` subscriber: `$RUST_LOG` (default `info`), to stderr.
///
/// Called once from [`run`], so an app-side error is visible in whatever the launcher redirected
/// stderr into. Errors are swallowed: a subscriber already installed by an embedder is fine.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ignored = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Opens the window and runs the app. Returns when the last window closes.
pub fn run() -> anyhow::Result<()> {
    init_tracing();
    let home = fleet_home();
    let window_error = Rc::new(RefCell::new(None));
    let reported_window_error = Rc::clone(&window_error);
    tracing::info!(home = %home.display(), "fleet: starting");
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(move |cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            keymap::init(cx);
            // The native git pane brings its own key table. Its contexts all sit under its own
            // `Lazygit` root, which the Workspace renders inside `Fleet > Workspace > Native`,
            // so its bindings are reachable exactly there and nowhere else. Theme and assets
            // stay installed once, above, because there is one window and one design system.
            fleet_lazygit::keymap::init(cx);
            cx.set_menus(vec![Menu {
                name: "Fleet".into(),
                items: vec![MenuItem::action("Quit", fleet::Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            // Developer-only end-to-end harness (docs/TESTING-HARNESS.md). `None` in every
            // normal launch, and nothing below this line runs again after startup.
            let harness = drive::harness();
            if let Some(harness) = &harness {
                harness.prepare(cx);
            }
            let options = window_options(harness.as_ref(), cx);
            match cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut shell = Shell::new(home, cx);
                    shell.observe_window(window, cx);
                    shell
                })
            }) {
                Ok(window) => {
                    let _ignored = window.update(cx, |_, window, _| window.activate_window());
                    cx.activate(true);
                    if let Some(harness) = harness {
                        let _ignored = window.update(cx, |shell, window, cx| {
                            // Infallible, and cancelled by its own window-close subscription.
                            let state = shell.state.clone();
                            if let Some(driver) = drive::spawn(harness, state, window, cx) {
                                driver.detach();
                            }
                        });
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "fleet: could not open the window");
                    eprintln!("fleet: could not open the window: {error}");
                    *reported_window_error.borrow_mut() = Some(error.to_string());
                    cx.quit();
                }
            }
        });
    let error = window_error.borrow_mut().take();
    // The GPU driver tears its EGL context down in a thread-local destructor after `main`
    // returns, and logs while it does: `wgpu_hal`'s EGL debug callback emits a `log` record,
    // `tracing-log` forwards it, and `tracing_subscriber`'s formatter reaches for a
    // thread-local buffer of its own that has already been destroyed. The access panics, a
    // panic raised during thread-local destruction cannot unwind, and the process aborts with
    // "fatal runtime error: failed to initiate panic" instead of exiting 0 — on every quit, in
    // every Wayland session. Closing the bridge here is the whole fix: nothing after this line
    // has anything left to say, and `tracing` events from Fleet's own code are already done.
    log::set_max_level(log::LevelFilter::Off);
    finish_run(error)
}

/// Builds the one window's options.
///
/// In harness mode the size and the title are pinned, so two runs of the same scenario lay out
/// identically and the compositor can find this window by name. The production minimum size still
/// applies — the harness drives the real app — and `meta` reports the bounds the window actually
/// got rather than the ones that were asked for.
fn window_options(harness: Option<&drive::Harness>, cx: &mut App) -> WindowOptions {
    let window_size = harness
        .and_then(drive::Harness::window_size)
        .unwrap_or_else(|| size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)));
    let title = harness
        .and_then(drive::Harness::window_title)
        .unwrap_or_else(|| SharedString::new_static("Fleet"));
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        titlebar: Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(cx.theme().space.md, cx.theme().space.md)),
        }),
        ..Default::default()
    }
}

fn finish_run(window_error: Option<String>) -> anyhow::Result<()> {
    match window_error {
        Some(error) => Err(anyhow::anyhow!("could not open the Fleet window: {error}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_open_failure_returns_error() {
        let error = finish_run(Some("display unavailable".to_owned()))
            .expect_err("a window failure must reach main");
        assert!(error.to_string().contains("display unavailable"));
        assert!(finish_run(None).is_ok());
    }
}
