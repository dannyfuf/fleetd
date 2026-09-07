//! The `Theme` global: one token set, resolved for one appearance.

use gpui::{App, BoxShadow, Global, Pixels, SharedString, point, px};

use super::palette::TerminalPalette;
use super::tokens::{
    ColorTokens, Elevation, Metrics, Motion, Radii, ShadowToken, Spacing, TypeScale,
};

/// The size the monospace probe measures at. Any size works; the comparison is a ratio.
const MONO_PROBE_SIZE: Pixels = px(12.5);
/// How far two advances may differ and still count as the same cell width.
const MONO_PROBE_EPSILON: Pixels = px(0.01);

/// Which appearance the theme is resolved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Light appearance.
    Light,
    /// Dark appearance. Fleet's default.
    #[default]
    Dark,
}

impl ThemeMode {
    /// The other mode.
    pub fn toggled(self) -> Self {
        match self {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    }

    /// Whether this mode is dark.
    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }
}

/// Every token the design system exposes, resolved for one [`ThemeMode`].
///
/// Installed as a gpui [`Global`] by [`Theme::init`] and read through [`ActiveTheme::theme`].
/// Components must never hold a `Theme` across frames; read it from `cx` each render.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Which appearance this instance was built for.
    pub mode: ThemeMode,
    /// Semantic colors.
    pub colors: ColorTokens,
    /// Terminal palette and default fg/bg.
    pub terminal: TerminalPalette,
    /// Type scale.
    pub text: TypeScale,
    /// 4 px spacing scale.
    pub space: Spacing,
    /// Corner radii.
    pub radii: Radii,
    /// Shadow recipes.
    pub elevation: Elevation,
    /// Durations.
    pub motion: Motion,
    /// Fixed geometry from the UX spec.
    pub metrics: Metrics,
    /// UI font family. `.SystemUIFont` resolves to SF Pro via CoreText with no registration.
    pub font_ui: SharedString,
    /// Mono font family, resolved from [`Theme::MONO_STACK`] by [`Theme::init`].
    pub font_mono: SharedString,
}

impl Global for Theme {}

impl Theme {
    /// The dark theme.
    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            colors: ColorTokens::dark(),
            terminal: TerminalPalette::dark(),
            text: TypeScale::default(),
            space: Spacing::default(),
            radii: Radii::default(),
            elevation: Elevation::dark(),
            motion: Motion::default(),
            metrics: Metrics::default(),
            font_ui: SharedString::new_static(".SystemUIFont"),
            font_mono: SharedString::new_static("SF Mono"),
        }
    }

    /// The light theme.
    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            colors: ColorTokens::light(),
            terminal: TerminalPalette::light(),
            elevation: Elevation::light(),
            ..Self::dark()
        }
    }

    /// Build the theme for a mode.
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        }
    }

    /// The monospaced faces §0 accepts, best first.
    ///
    /// `SF Mono` is the spec's face but it is **not** part of a stock macOS install — it ships
    /// with Xcode / the SF font download. `Menlo` and `Monaco` do ship with every macOS, and
    /// `DejaVu Sans Mono` / `Liberation Mono` cover the Linux builds. Without a stack, a
    /// missing `SF Mono` silently resolves to the proportional UI face and the terminal grid,
    /// branch names, paths and shas stop landing on the cell grid.
    pub const MONO_STACK: &'static [&'static str] = &[
        "SF Mono",
        "SFMono-Regular",
        "Menlo",
        "Monaco",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Courier New",
    ];

    /// The first face in [`Self::MONO_STACK`] the platform resolves to something that really is
    /// monospaced.
    ///
    /// gpui has no "is this family installed" query: [`gpui::TextSystem::resolve_font`] answers
    /// with the *fallback* face when a family is missing, and that fallback is the UI sans. The
    /// only reliable probe is therefore metric: a monospaced face gives `i`, `M` and `W` the
    /// same advance, a proportional one does not.
    #[must_use]
    pub fn resolve_mono_family(cx: &App) -> SharedString {
        let text_system = cx.text_system();
        let last = Self::MONO_STACK[Self::MONO_STACK.len() - 1];
        for family in Self::MONO_STACK {
            let font_id = text_system.resolve_font(&gpui::font(*family));
            let advance = |character| {
                text_system
                    .advance(font_id, MONO_PROBE_SIZE, character)
                    .ok()
                    .map(|size| size.width)
            };
            let (Some(narrow), Some(wide), Some(widest)) =
                (advance('i'), advance('M'), advance('W'))
            else {
                continue;
            };
            if (narrow - wide).abs() < MONO_PROBE_EPSILON
                && (wide - widest).abs() < MONO_PROBE_EPSILON
            {
                return SharedString::new_static(family);
            }
        }
        SharedString::new_static(last)
    }

    /// Install the theme global. Must run before any kit component renders.
    pub fn init(mode: ThemeMode, cx: &mut App) {
        let mono = Self::resolve_mono_family(cx);
        cx.set_global(Self::for_mode(mode).with_mono_family(mono));
    }

    /// Change appearance while preserving typography, geometry and motion overrides.
    pub fn change(mode: ThemeMode, cx: &mut App) {
        let mut theme = cx.try_global::<Theme>().cloned().unwrap_or_else(|| {
            Self::for_mode(mode).with_mono_family(Self::resolve_mono_family(cx))
        });
        theme.set_mode(mode);
        cx.set_global(theme);
    }

    fn set_mode(&mut self, mode: ThemeMode) {
        let appearance = Self::for_mode(mode);
        self.mode = mode;
        self.colors = appearance.colors;
        self.terminal = appearance.terminal;
        self.elevation = appearance.elevation;
    }

    /// Override the mono family, e.g. with the face [`Self::resolve_mono_family`] settled on.
    #[must_use]
    pub fn with_mono_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_mono = family.into();
        self
    }

    /// Flip light/dark and return the new mode.
    pub fn toggle(cx: &mut App) -> ThemeMode {
        let next = cx.global::<Theme>().mode.toggled();
        Self::change(next, cx);
        next
    }

    /// Whether the installed theme is dark.
    pub fn is_dark(&self) -> bool {
        self.mode.is_dark()
    }

    /// A `Vec<BoxShadow>` ready for `Styled::shadow` from a [`ShadowToken`].
    pub fn shadow(&self, token: ShadowToken) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: token.color,
            offset: point(px(0.0), token.y),
            blur_radius: token.blur,
            spread_radius: token.spread,
            inset: false,
        }]
    }

    /// The dialog / palette shadow.
    pub fn dialog_shadow(&self) -> Vec<BoxShadow> {
        self.shadow(self.elevation.dialog)
    }

    /// The sheet / toast shadow.
    pub fn sheet_shadow(&self) -> Vec<BoxShadow> {
        self.shadow(self.elevation.sheet)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// `cx.theme()` on any context that derefs to [`App`].
pub trait ActiveTheme {
    /// The installed theme.
    ///
    /// # Panics
    /// Panics when [`Theme::init`] has not run.
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_changes_preserve_typography_and_geometry() {
        let mut theme = Theme::dark();
        theme.font_ui = "Custom UI".into();
        theme.font_mono = "Custom Mono".into();
        theme.metrics.row_h = px(42.0);
        theme.text.ui.size = px(17.0);
        let metrics = theme.metrics;
        let text = theme.text;
        theme.set_mode(ThemeMode::Light);
        assert_eq!(theme.colors, ColorTokens::light());
        assert_eq!(theme.terminal, TerminalPalette::light());
        assert_eq!(theme.font_ui, "Custom UI");
        assert_eq!(theme.font_mono, "Custom Mono");
        assert_eq!(theme.metrics, metrics);
        assert_eq!(theme.text, text);
    }
}
