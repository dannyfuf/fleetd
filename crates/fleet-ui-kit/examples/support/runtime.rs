use fleet_ui_kit::{KitAssets, Theme, ThemeMode};
use gpui::{
    Action, App, AppContext, Bounds, Context, Focusable, Menu, MenuItem, Render, TitlebarOptions,
    WindowBounds, WindowOptions, px, size,
};

pub fn run<V: Render + Focusable + 'static, A: Action>(
    title: &'static str,
    dimensions: (f32, f32),
    quit: A,
    configure: impl FnOnce(&mut App) + 'static,
    build: impl FnOnce(&mut Context<V>) -> V + 'static,
) {
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(move |cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            configure(cx);
            cx.on_action(|_: &A, cx: &mut App| cx.quit());
            cx.set_menus(vec![Menu {
                name: "fleet-ui-kit".into(),
                items: vec![MenuItem::action("Quit", quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(dimensions.0), px(dimensions.1)), cx);
            match cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| cx.new(build),
            ) {
                Ok(window) => {
                    if let Err(error) = window.update(cx, |view, window, cx| {
                        window.focus(&view.focus_handle(cx), cx)
                    }) {
                        eprintln!("{title}: focus failed: {error}");
                    }
                    cx.activate(true);
                }
                Err(error) => {
                    eprintln!("{title}: window creation failed: {error}");
                    cx.quit();
                }
            }
        });
}
