//! `JobTicker` — the newest running job as one status-bar line, with a `+n` suffix.
//!
//! The status bar has one slot for background work, and this is what fills it while things are
//! going well. When something failed, [`StickyErrorSlot`](crate::components::StickyErrorSlot)
//! takes the slot instead and holds it until dismissed — successes are transient, errors are
//! not (§1.8).

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};

use crate::{components::Spinner, icons::IconSize, text::Text, theme::ActiveTheme, tone::Tone};

/// `⟳ clone nixos 40%  +1`
#[derive(IntoElement)]
pub struct JobTicker {
    id: ElementId,
    kind: SharedString,
    target: SharedString,
    percent: Option<u8>,
    elapsed: Option<SharedString>,
    extra: usize,
}

impl JobTicker {
    /// The newest running job.
    pub fn new(kind: impl Into<SharedString>, target: impl Into<SharedString>) -> Self {
        Self {
            id: ElementId::Name(SharedString::new_static("job-ticker")),
            kind: kind.into(),
            target: target.into(),
            percent: None,
            elapsed: None,
            extra: 0,
        }
    }

    /// A stable id for the spinner. Only needed when two tickers share a window; the animation
    /// restarts every frame if two elements claim the same id.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Percent, when parseable.
    pub fn percent(mut self, percent: u8) -> Self {
        self.percent = Some(percent);
        self
    }

    /// `m:ss`, shown after 30 s like the Jobs panel's column. Zero-suppressed when unset.
    pub fn elapsed(mut self, elapsed: impl Into<SharedString>) -> Self {
        self.elapsed = Some(elapsed.into());
        self
    }

    /// How many other jobs are running. Rendered `+n`, zero-suppressed.
    pub fn extra(mut self, extra: usize) -> Self {
        self.extra = extra;
        self
    }
}

/// Wrap a run that must never be the one that shrinks.
fn fixed(child: impl IntoElement) -> gpui::Div {
    div().flex_none().child(child)
}

impl RenderOnce for JobTicker {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .min_w_0()
            .child(Spinner::new(self.id).size(IconSize::Small))
            // The kind keeps its contrast: it is the word that says *what* is happening.
            .child(fixed(Text::ui(self.kind)))
            // The target is the one run allowed to lose its tail — the kind and the percent
            // are what the status bar is for, and a long worktree id must not push them off
            // the end of the line.
            .child(Text::ui(self.target).muted().ellipsize())
            .children(self.elapsed.map(|elapsed| fixed(Text::ui(elapsed).faint())))
            .children(
                self.percent
                    .map(|percent| fixed(Text::ui(format!("{}%", percent.min(100))).muted())),
            )
            .children(
                (self.extra > 0)
                    .then(|| fixed(Text::ui(format!("+{}", self.extra)).tone(Tone::Muted))),
            )
    }
}
