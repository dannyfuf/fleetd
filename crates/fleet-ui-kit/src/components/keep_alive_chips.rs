//! `KeepAliveChips` — `⚡ claude, :3000`, with the max-3 overflow and the ch width ladder.
//!
//! §3.3 column 4: these tell you why `sleep` will refuse to close windows and what `K` or `d`
//! would kill. The ladder is 18 / 14 / 10 / 0 ch at pane widths 104 / 88 / 72 / below.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
    truncate::{Truncate, truncate},
};

/// One keep-alive label with the glyph that names its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeepAliveLabel {
    /// The label as the daemon reports it (`claude`, `:3000`, `nvim`).
    pub label: SharedString,
    /// The glyph: `bot` for agents, `server` for ports, `file-pen` for editors.
    pub icon: Option<Icon>,
}

impl KeepAliveLabel {
    /// A label with no glyph.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
        }
    }

    /// A label with a kind glyph.
    pub fn with_icon(label: impl Into<SharedString>, icon: Icon) -> Self {
        Self {
            label: label.into(),
            icon: Some(icon),
        }
    }
}

/// The keep-alive slot of a worktree row.
#[derive(IntoElement)]
pub struct KeepAliveChips {
    labels: Vec<KeepAliveLabel>,
    max_visible: usize,
    width_ch: Option<f32>,
    show_bolt: bool,
}

impl KeepAliveChips {
    /// The labels of one session.
    pub fn new(labels: impl IntoIterator<Item = KeepAliveLabel>) -> Self {
        Self {
            labels: labels.into_iter().collect(),
            max_visible: 3,
            width_ch: None,
            show_bolt: true,
        }
    }

    /// Change the overflow threshold. 3 by default.
    pub fn max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    /// Fix the slot width in `ch`. `0.0` hides the slot entirely.
    pub fn width_ch(mut self, width: f32) -> Self {
        self.width_ch = Some(width);
        self
    }

    /// Resolve the ladder from the owning pane's width in `ch` (§3.3 column 4).
    pub fn from_pane_ch(pane_ch: f32) -> f32 {
        if pane_ch >= 104.0 {
            18.0
        } else if pane_ch >= 88.0 {
            14.0
        } else if pane_ch >= 72.0 {
            10.0
        } else {
            0.0
        }
    }

    /// Hide the leading `zap`.
    pub fn show_bolt(mut self, show: bool) -> Self {
        self.show_bolt = show;
        self
    }

    /// The joined text this slot will render, after overflow and the ch budget.
    pub fn resolved_text(&self) -> SharedString {
        let shown = self.labels.len().min(self.max_visible);
        let mut text = self
            .labels
            .iter()
            .take(shown)
            .map(|l| l.label.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let overflow = self.labels.len() - shown;
        if overflow > 0 {
            text.push_str(&format!(" +{overflow}"));
        }
        match self.width_ch {
            Some(budget) if budget > 1.0 => {
                truncate(&text, (budget as usize).saturating_sub(2), Truncate::Tail)
            }
            _ => SharedString::from(text),
        }
    }
}

impl RenderOnce for KeepAliveChips {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.labels.is_empty() || self.width_ch == Some(0.0) {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let color = theme.colors.text_secondary;
        let width: Option<Pixels> = self.width_ch.map(ch);
        let text = self.resolved_text();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .when_some(width, |el, width| el.w(width))
            .when(self.show_bolt, |el| {
                el.child(Icon::Zap.el().size(IconSize::Small).color(color))
            })
            .child(Text::ui(text).tone(Tone::Secondary).ellipsize())
            .into_any_element()
    }
}
