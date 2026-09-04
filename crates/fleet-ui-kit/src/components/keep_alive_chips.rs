//! `KeepAliveChips` — `⚡ claude, :3000`, with the max-3 overflow and the ch width ladder.
//!
//! §3.3 column 4: these tell you why `sleep` will refuse to close windows and what `K` or `d`
//! would kill. The ladder is 18 / 14 / 10 / 0 ch at pane widths 104 / 88 / 72 / below, and
//! [`super::DegradedChip`] **outranks** this component in the same row slot.
//!
//! Two renderings of the same labels:
//!
//! * a row slot — one `zap`, then the labels joined with `, ` inside a `ch` budget;
//! * a detail panel or a confirm — [`KeepAliveChips::show_kind_icons`], where each label keeps
//!   its own glyph (`bot` for an agent, `server` for a port, `file-pen` for an editor) because
//!   there is room for the extra information.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
    truncate::{Truncate, truncate},
};

/// How many labels render before the rest collapse into `+n`.
pub const MAX_VISIBLE: usize = 3;

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
    show_kind_icons: bool,
}

impl KeepAliveChips {
    /// The labels of one session.
    pub fn new(labels: impl IntoIterator<Item = KeepAliveLabel>) -> Self {
        Self {
            labels: labels.into_iter().collect(),
            max_visible: MAX_VISIBLE,
            width_ch: None,
            show_bolt: true,
            show_kind_icons: false,
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

    /// Give every label its own kind glyph instead of joining them into one run of text.
    ///
    /// For the detail panel and the confirms, where the width is not rationed. A row slot must
    /// stay in its `ch` budget, so it keeps the joined form.
    pub fn show_kind_icons(mut self, show: bool) -> Self {
        self.show_kind_icons = show;
        self
    }

    /// The labels this slot will show, and how many it dropped.
    fn visible(&self) -> (&[KeepAliveLabel], usize) {
        let shown = self.labels.len().min(self.max_visible);
        (&self.labels[..shown], self.labels.len() - shown)
    }

    /// The joined text this slot will render, after overflow and the `ch` budget.
    ///
    /// The budget loses two characters to the leading `zap` and its gap, which is what keeps a
    /// truncated label from colliding with the next column.
    pub fn resolved_text(&self) -> SharedString {
        let (visible, overflow) = self.visible();
        let mut text = visible
            .iter()
            .map(|label| label.label.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if overflow > 0 {
            text.push_str(&format!(" +{overflow}"));
        }
        match self.width_ch {
            Some(budget) if budget > 1.0 => {
                let reserved = if self.show_bolt { 2 } else { 0 };
                truncate(
                    &text,
                    (budget as usize).saturating_sub(reserved),
                    Truncate::Tail,
                )
            }
            _ => SharedString::from(text),
        }
    }

    /// Whether this slot draws anything at all (§1.2: no labels, no slot).
    pub fn is_visible(&self) -> bool {
        !self.labels.is_empty() && self.width_ch != Some(0.0)
    }
}

impl RenderOnce for KeepAliveChips {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.is_visible() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let color = theme.colors.text_secondary;
        let width: Option<Pixels> = self.width_ch.map(ch);
        let base = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.space.xs)
            .when_some(width, |el, width| el.w(width).overflow_hidden())
            .when(self.show_bolt, |el| {
                el.child(Icon::Zap.el().size(IconSize::Small).color(color))
            });

        if self.show_kind_icons {
            let (visible, overflow) = self.visible();
            let labels: Vec<_> = visible.to_vec();
            return base
                .children(labels.into_iter().map(|label| {
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(theme.space.xxs)
                        .children(
                            label
                                .icon
                                .map(|icon| icon.el().size(IconSize::Small).color(color)),
                        )
                        .child(Text::ui(label.label).tone(Tone::Secondary))
                }))
                .when(overflow > 0, |el| {
                    el.child(Text::ui(format!("+{overflow}")).faint())
                })
                .into_any_element();
        }

        base.child(
            Text::ui(self.resolved_text())
                .tone(Tone::Secondary)
                .ellipsize(),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<KeepAliveLabel> {
        vec![
            KeepAliveLabel::with_icon("claude", Icon::Bot),
            KeepAliveLabel::with_icon(":3000", Icon::Server),
            KeepAliveLabel::with_icon("nvim", Icon::FilePen),
            KeepAliveLabel::new("vitest"),
        ]
    }

    #[test]
    fn ladder_matches_the_spec() {
        assert_eq!(KeepAliveChips::from_pane_ch(138.0), 18.0);
        assert_eq!(KeepAliveChips::from_pane_ch(104.0), 18.0);
        assert_eq!(KeepAliveChips::from_pane_ch(88.0), 14.0);
        assert_eq!(KeepAliveChips::from_pane_ch(72.0), 10.0);
        assert_eq!(KeepAliveChips::from_pane_ch(71.9), 0.0);
    }

    #[test]
    fn overflow_collapses_after_three() {
        let text = KeepAliveChips::new(labels()).resolved_text();
        assert_eq!(text.as_ref(), "claude, :3000, nvim +1");
    }

    #[test]
    fn budget_truncates_and_reserves_the_bolt() {
        let text = KeepAliveChips::new(labels()).width_ch(10.0).resolved_text();
        assert_eq!(text.chars().count(), 8);
        assert!(text.ends_with('\u{2026}'));
    }

    #[test]
    fn empty_and_zero_width_render_nothing() {
        assert!(!KeepAliveChips::new([]).is_visible());
        assert!(!KeepAliveChips::new(labels()).width_ch(0.0).is_visible());
        assert!(KeepAliveChips::new(labels()).width_ch(18.0).is_visible());
    }
}
