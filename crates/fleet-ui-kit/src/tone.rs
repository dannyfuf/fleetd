//! Semantic tone: the only way a component is allowed to pick a color.
//!
//! §1.4 of the UX spec: four colors, all semantic, never decorative. `Draft`, `disabled` and
//! `not applicable` are expressed by lowering contrast (`Secondary` / `Muted`), never by a hue.

use gpui::Hsla;

use crate::theme::Theme;

/// A named color intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tone {
    /// Primary text contrast. The value the eye must land on.
    #[default]
    Default,
    /// Secondary contrast: repo, host, age, counts, labels.
    Secondary,
    /// Lowest contrast: draft, disabled, key hints, the null dash.
    Muted,
    /// Cursor and focus. Never "how is it going".
    Accent,
    /// Healthy / done / approved.
    Success,
    /// Needs attention / in flight / unknown / degraded.
    Warning,
    /// Broken / destructive.
    Danger,
    /// Neutral information.
    Info,
    /// Text on top of an accent or semantic fill.
    Inverse,
}

impl Tone {
    /// The foreground color for this tone.
    pub fn color(self, theme: &Theme) -> Hsla {
        let c = &theme.colors;
        match self {
            Tone::Default => c.text,
            Tone::Secondary => c.text_secondary,
            Tone::Muted => c.text_muted,
            Tone::Accent => c.accent,
            Tone::Success => c.success,
            Tone::Warning => c.warning,
            Tone::Danger => c.danger,
            Tone::Info => c.info,
            Tone::Inverse => c.text_inverse,
        }
    }

    /// A low-alpha fill of the same hue, for chip and badge backgrounds.
    pub fn fill(self, theme: &Theme) -> Hsla {
        match self {
            Tone::Default | Tone::Secondary | Tone::Muted | Tone::Inverse => theme
                .colors
                .text
                .opacity(theme.metrics.neutral_fill_opacity),
            other => other
                .color(theme)
                .opacity(theme.metrics.semantic_fill_opacity),
        }
    }
}
