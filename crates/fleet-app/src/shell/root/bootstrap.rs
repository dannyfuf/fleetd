use super::Shell;
use crate::{actions::fleet, drive, keymap};
use fleet_ui_kit::{ActiveTheme, KitAssets, Theme, ThemeMode};
use gpui::{
    App, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);
const MIN_SIZE: (f32, f32) = (900.0, 560.0);

/// `$FLEET_HOME`, or `~/.fleet`.
#[must_use]
fn fleet_home() -> PathBuf {
    if let Some(home) = std::env::var_os("FLEET_HOME") {
        return PathBuf::from(home);
    }
    crate::presentation::home_dir()
        .map_or_else(|| PathBuf::from(".fleet"), |home| home.join(".fleet"))
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

            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Fleet".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(
                        cx.theme().space.md,
                        cx.theme().space.md,
                    )),
                }),
                ..Default::default()
            };
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
                    *reported_window_error.borrow_mut() = Some(error.to_string());
                    cx.quit();
                }
            }
        });
    let error = window_error.borrow_mut().take();
    finish_run(error)
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
