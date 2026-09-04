//! The `Theme` global: one token set, resolved for one appearance.

use gpui::{App, BoxShadow, Global, Hsla, SharedString, Window, WindowAppearance, point, px};

use super::palette::TerminalPalette;
use super::tokens::{
    ColorTokens, Elevation, Metrics, Motion, Radii, ShadowToken, Spacing, TypeScale,
};

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
    /// Map the OS appearance onto a Fleet mode. Vibrant variants collapse onto their base.
    pub fn from_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        }
    }

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
    /// Mono font family. `SF Mono` on macOS; set to `Menlo` if the system lacks it.
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

    /// Install the theme global. Must run before any kit component renders.
    pub fn init(mode: ThemeMode, cx: &mut App) {
        cx.set_global(Self::for_mode(mode));
    }

    /// Install the theme global from the window's OS appearance.
    pub fn init_from_system(window: &Window, cx: &mut App) {
        Self::init(ThemeMode::from_appearance(window.appearance()), cx);
    }

    /// Replace the theme global with another mode.
    pub fn change(mode: ThemeMode, cx: &mut App) {
        cx.set_global(Self::for_mode(mode));
    }

    /// Flip light/dark and return the new mode.
    pub fn toggle(cx: &mut App) -> ThemeMode {
        let next = cx.global::<Theme>().mode.toggled();
        Self::change(next, cx);
        next
    }

    /// Follow the OS appearance.
    pub fn sync_system_appearance(window: &Window, cx: &mut App) {
        Self::change(ThemeMode::from_appearance(window.appearance()), cx);
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

    /// Fade a token toward the background, used for the >10 min freshness ladder (55 %) and
    /// the "deleting" row (40 %).
    pub fn faded(&self, color: Hsla, factor: f32) -> Hsla {
        color.opacity(factor)
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
