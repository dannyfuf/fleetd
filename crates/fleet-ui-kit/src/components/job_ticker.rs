//! `JobTicker` — the newest running job as one status-bar line, with a `+n` suffix.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{components::Spinner, icons::IconSize, text::Text, theme::ActiveTheme};

/// `⟳ clone nixos 40%  +1`
#[derive(IntoElement)]
pub struct JobTicker {
    kind: SharedString,
    target: SharedString,
    percent: Option<u8>,
    extra: usize,
}

impl JobTicker {
    /// The newest running job.
    pub fn new(kind: impl Into<SharedString>, target: impl Into<SharedString>) -> Self {
        Self {
            kind: kind.into(),
            target: target.into(),
            percent: None,
            extra: 0,
        }
    }

    /// Percent, when parseable.
    pub fn percent(mut self, percent: u8) -> Self {
        self.percent = Some(percent);
        self
    }

    /// How many other jobs are running. Rendered `+n`, zero-suppressed.
    pub fn extra(mut self, extra: usize) -> Self {
        self.extra = extra;
        self
    }
}

impl RenderOnce for JobTicker {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .child(Spinner::new("job-ticker").size(IconSize::Small))
            .child(Text::ui(self.kind).muted())
            .child(Text::ui(self.target).muted().ellipsize())
            .children(
                self.percent
                    .map(|percent| Text::ui(format!("{percent}%")).muted()),
            )
            .children(
                (self.extra > 0).then(|| Text::ui(format!("+{}", self.extra)).faint()),
            )
    }
}
