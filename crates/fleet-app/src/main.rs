//! Entry point that selects Fleet's CLI or native GPUI application mode.

use gpui::{
    App, Bounds, Context, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size,
};
use gpui_platform::application;

struct FleetWindow;

impl Render for FleetWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(rgb(0x17191c))
            .text_xl()
            .text_color(rgb(0xf4f4f5))
            .child("Fleet")
    }
}

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).is_some() {
        return fleet_cli::run();
    }

    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.0), px(520.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Fleet".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(|_| FleetWindow),
        )
        .expect("failed to open Fleet window");
        cx.activate(true);
    });

    Ok(())
}
