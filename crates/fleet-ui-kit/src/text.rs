//! `Text` — the four type roles, with fixed line heights.
//!
//! Views never call `text_size`, `font_family` or `line_height` themselves; they pick a role.
//! That is what keeps a branch name in one pane pixel-identical to the same branch name in
//! another pane, which is the whole point of §2.5 ("identical on every screen").

use gpui::{App, Div, FontWeight, Hsla, Pixels, SharedString, Window, div, prelude::*};

use crate::{
    theme::{ActiveTheme, FontRole, Theme, TypeStyle},
    tone::Tone,
    truncate::{Truncate, truncate},
};

/// One of the type roles of the design system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextRole {
    /// 13 / 18 regular, system face. The default for prose and values.
    #[default]
    Ui,
    /// 13 / 18 medium, system face. Titles inside rows, active tabs.
    UiStrong,
    /// 15 / 20 medium, system face. Dialog and detail-panel titles.
    Title,
    /// mono 12.5 / 18. Branch, path, sha, target, PR head ref.
    Data,
    /// mono 11.5 / 16. Job progress sub-lines, log tails.
    DataSmall,
    /// 11 / 14 uppercase medium. Pane and section labels.
    Label,
    /// mono 11 / 14. Key hints.
    Hint,
}

impl TextRole {
    /// The token entry for this role.
    pub fn style(self, theme: &Theme) -> TypeStyle {
        match self {
            TextRole::Ui => theme.text.ui,
            TextRole::UiStrong => theme.text.ui_strong,
            TextRole::Title => theme.text.title,
            TextRole::Data => theme.text.data,
            TextRole::DataSmall => theme.text.data_small,
            TextRole::Label => theme.text.label,
            TextRole::Hint => theme.text.hint,
        }
    }

    /// The default tone for this role, used when the caller sets none.
    pub fn default_tone(self) -> Tone {
        match self {
            TextRole::Ui | TextRole::UiStrong | TextRole::Title | TextRole::Data => Tone::Default,
            TextRole::DataSmall | TextRole::Label => Tone::Secondary,
            TextRole::Hint => Tone::Muted,
        }
    }
}

/// A run of text in one role.
#[derive(IntoElement)]
pub struct Text {
    text: SharedString,
    role: TextRole,
    tone: Tone,
    color: Option<Hsla>,
    opacity: Option<f32>,
    weight: Option<FontWeight>,
    budget: Option<(usize, Truncate)>,
    flex_ellipsis: bool,
    width: Option<Pixels>,
    ch_width: Option<f32>,
}

impl Text {
    /// A run of text in an explicit role.
    pub fn new(role: TextRole, text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            role,
            tone: role.default_tone(),
            color: None,
            opacity: None,
            weight: None,
            budget: None,
            flex_ellipsis: false,
            width: None,
            ch_width: None,
        }
    }

    /// 13 / 18 regular.
    pub fn ui(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::Ui, text)
    }

    /// 13 / 18 medium.
    pub fn ui_strong(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::UiStrong, text)
    }

    /// 15 / 20 medium.
    pub fn title(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::Title, text)
    }

    /// mono 12.5 / 18.
    pub fn data(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::Data, text)
    }

    /// mono 11.5 / 16.
    pub fn data_small(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::DataSmall, text)
    }

    /// 11 / 14 uppercase.
    pub fn label(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::Label, text)
    }

    /// mono 11 / 14, muted.
    pub fn hint(text: impl Into<SharedString>) -> Self {
        Self::new(TextRole::Hint, text)
    }

    /// Pick a semantic tone.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Secondary contrast.
    pub fn muted(self) -> Self {
        self.tone(Tone::Secondary)
    }

    /// Lowest contrast.
    pub fn faint(self) -> Self {
        self.tone(Tone::Muted)
    }

    /// An explicit color. Prefer [`Text::tone`]; use this only for terminal / palette colors
    /// that are already resolved from the theme.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Lower the whole run's opacity (the §2.6 freshness ladder uses 55 %).
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }

    /// Override the role's weight. Reserved for the palette's match-weight bump.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Truncate at a `ch` budget with an explicit ellipsis position.
    pub fn truncate_at(mut self, budget: usize, mode: Truncate) -> Self {
        self.budget = Some((budget, mode));
        self
    }

    /// Let the layout truncate with a tail ellipsis when the flex column runs out of room.
    pub fn ellipsize(mut self) -> Self {
        self.flex_ellipsis = true;
        self
    }

    /// Fix the run's width in pixels.
    pub fn w(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Fix the run's width in `ch` of the data face (the unit every column ladder uses).
    pub fn w_ch(mut self, width: f32) -> Self {
        self.ch_width = Some(width);
        self
    }

    /// The string this run will render, after the `ch` budget is applied.
    pub fn resolved_text(&self) -> SharedString {
        match self.budget {
            Some((budget, mode)) => truncate(self.text.as_ref(), budget, mode),
            None => self.text.clone(),
        }
    }
}

/// Apply a [`TypeStyle`] to any styled element. Exposed so composite components can style a
/// container once instead of wrapping every child in a [`Text`].
pub fn styled_with<E: Styled>(element: E, style: TypeStyle, theme: &Theme) -> E {
    let family = match style.font {
        FontRole::Ui => theme.font_ui.clone(),
        FontRole::Mono => theme.font_mono.clone(),
    };
    element
        .font_family(family)
        .text_size(style.size)
        .line_height(style.line_height)
        .font_weight(style.weight)
}

impl RenderOnce for Text {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let style = self.role.style(theme);
        let color = self.color.unwrap_or_else(|| self.tone.color(theme));
        let mut text = self.resolved_text();
        if style.uppercase {
            text = SharedString::from(text.to_uppercase());
        }

        let mut el: Div = styled_with(div(), style, theme).text_color(color);
        if let Some(weight) = self.weight {
            el = el.font_weight(weight);
        }
        if let Some(opacity) = self.opacity {
            el = el.opacity(opacity);
        }
        if let Some(width) = self.width {
            el = el.w(width);
        }
        if let Some(width) = self.ch_width {
            el = el.w(crate::theme::ch(width));
        }
        if self.flex_ellipsis {
            el = el.overflow_hidden().whitespace_nowrap().text_ellipsis();
        }
        el.child(text)
    }
}
