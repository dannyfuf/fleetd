//! A native lazygit clone: `fleet-git` for the plumbing, `fleet-ui-kit` for the design system,
//! gpui for the window.
//!
//! Embedders use [`root::Lazygit`] and [`root::LazygitEvent`]. Git operations and diff
//! preparation run off the foreground thread.

#![warn(missing_docs)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use fleet_ui_kit::{KitAssets, Theme, ThemeMode};
use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

mod actions;
mod bridge;
pub mod diff_view;
pub mod drive;
pub mod keymap;
mod overlays;
mod panels;
pub mod root;
mod state;
mod views;

/// The window's default size.
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);
/// The smallest window the layout is defined for.
const MIN_SIZE: (f32, f32) = (900.0, 560.0);

/// Boots gpui and opens the window on `path`. Returns when the last window closes.
pub fn run(path: PathBuf) -> anyhow::Result<()> {
    init_tracing();
    tracing::info!(path = %path.display(), "fleet-lazygit: starting");

    let startup_error = Arc::new(Mutex::new(None));
    let reported_startup_error = startup_error.clone();
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(move |cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            keymap::init(cx);

            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
                // The titlebar is opaque on purpose: lazygit's first pane title sits at the very
                // top left, exactly where transparent-titlebar traffic lights would cover it.
                titlebar: Some(TitlebarOptions {
                    title: Some("fleet-lazygit".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            };

            match cx.open_window(options, |_window, cx| {
                cx.new(|cx| root::Lazygit::new(path, cx))
            }) {
                Ok(window) => {
                    // `q` no longer quits the process by itself: the view only reports that it
                    // wants to leave, and the standalone binary is what decides that means
                    // exiting. An embedder maps the same event onto closing its pane.
                    if let Ok(view) = window.update(cx, |_view, _window, cx| cx.entity()) {
                        cx.subscribe(&view, |_view, event, cx| match event {
                            root::LazygitEvent::Quit => cx.quit(),
                        })
                        .detach();
                    }
                    let _ignored = window.update(cx, |view, window, cx| {
                        window.activate_window();
                        view.set_active(true, window, cx);
                    });
                    cx.activate(true);
                    // Developer-only: drive the GUI from a script file (see `drive`).
                    if let Some(script) = drive::script_path() {
                        let _ignored = window.update(cx, |_view, window, cx| {
                            drive::spawn(script, window, cx).detach();
                        });
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "fleet-lazygit: could not open the window");
                    eprintln!("fleet-lazygit: could not open the window: {error}");
                    if let Ok(mut startup_error) = reported_startup_error.lock() {
                        *startup_error = Some(error.to_string());
                    }
                    cx.quit();
                }
            }
        });
    let startup_error = startup_error
        .lock()
        .map_err(|_| anyhow::anyhow!("fleet-lazygit: startup error state was poisoned"))?
        .take();
    startup_result(startup_error)
}

fn startup_result(error: Option<String>) -> anyhow::Result<()> {
    match error {
        Some(error) => Err(anyhow::anyhow!("could not open the window: {error}")),
        None => Ok(()),
    }
}

/// Installs a stderr `tracing` subscriber honouring `$RUST_LOG`. Errors are swallowed so an
/// embedder's own subscriber wins.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ignored = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_creation_failure_returns_error() {
        let error = startup_result(Some("display unavailable".to_owned()))
            .expect_err("window creation failure must reach main");
        assert_eq!(
            error.to_string(),
            "could not open the window: display unavailable"
        );
        assert!(startup_result(None).is_ok());
    }
}
