//! `DaemonSplash` — the full-window daemon surfaces of §3.12 cases A and B.
//!
//! `Banner` covers case C (the daemon died while attached) because that one is a 28 px strip
//! under the context bar. Cases A and B are **full-window, chrome-less** surfaces, and neither
//! fits `EmptyState`: A needs a spinner plus a delayed detail line, and B needs a mono tail of
//! `~/.fleet/logs/fleetd.log` — three lines of log a two-line pane-scoped empty state cannot
//! render.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::{KeyHintRow, Spinner},
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// Which of the two full-window daemon states this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonSplashKind {
    /// §3.12 A: cold start. A spinner and a title, nothing else until 3 s have passed.
    Starting,
    /// §3.12 B: fleetd will not start. A red title, the log tail and the recovery keys.
    Failed,
}

/// A centered full-window daemon surface.
#[derive(IntoElement)]
pub struct DaemonSplash {
    kind: DaemonSplashKind,
    title: SharedString,
    detail: Option<SharedString>,
    log_lines: Vec<SharedString>,
    hints: Option<KeyHintRow>,
}

impl DaemonSplash {
    /// §3.12 A — `Starting fleetd…` with a spinner. Bind nothing: fleetd is auto-spawned.
    pub fn starting(title: impl Into<SharedString>) -> Self {
        Self {
            kind: DaemonSplashKind::Starting,
            title: title.into(),
            detail: None,
            log_lines: Vec::new(),
            hints: None,
        }
    }

    /// §3.12 B — `fleetd could not start.` Bind `r` retry · `L` open log · `D` doctor ·
    /// `ctrl-q` quit.
    pub fn failed(title: impl Into<SharedString>) -> Self {
        Self {
            kind: DaemonSplashKind::Failed,
            title: title.into(),
            detail: None,
            log_lines: Vec::new(),
            hints: None,
        }
    }

    /// The one faint line under the title: the socket path after 3 s in case A, or
    /// `The socket ~/.fleet/fleetd.sock is stale.` in case B.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The last lines of `~/.fleet/logs/fleetd.log`, mono. The spec shows three; the caller
    /// tails and truncates.
    pub fn log_lines(mut self, lines: impl IntoIterator<Item = SharedString>) -> Self {
        self.log_lines = lines.into_iter().collect();
        self
    }

    /// The recovery keys. This surface is full-window, so these are bare keys, not `^s`
    /// prefixed — the terminal is not what is on screen.
    pub fn hints(mut self, hints: KeyHintRow) -> Self {
        self.hints = Some(hints);
        self
    }
}

impl RenderOnce for DaemonSplash {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let failed = self.kind == DaemonSplashKind::Failed;
        let title_color = if failed {
            theme.colors.danger
        } else {
            theme.colors.text
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme.space.md)
            .size_full()
            .bg(theme.colors.bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .children(failed.then(|| {
                        Icon::Unplug
                            .el()
                            .size(IconSize::Medium)
                            .color(theme.colors.danger)
                    }))
                    .child(Text::title(self.title).color(title_color))
                    .children(
                        (!failed).then(|| Spinner::new("daemon-splash").size(IconSize::Medium)),
                    ),
            )
            .children(
                self.detail
                    .map(|d| Text::data(d).tone(Tone::Muted).into_any_element()),
            )
            .children((!self.log_lines.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.space.xxs)
                    .px(theme.space.lg)
                    .py(theme.space.sm)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.surface)
                    .border(theme.metrics.hairline)
                    .border_color(theme.colors.border)
                    .children(
                        self.log_lines
                            .into_iter()
                            .map(|line| Text::data_small(line).faint()),
                    )
            }))
            .children(self.hints)
    }
}
