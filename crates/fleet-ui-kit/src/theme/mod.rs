//! Tokens and the `Theme` global.

mod palette;
#[allow(clippy::module_inception)]
mod theme;
mod tokens;

pub use palette::TerminalPalette;
pub use theme::{ActiveTheme, Theme, ThemeMode};
pub use tokens::{
    BASE_UNIT, CH, ColorTokens, Elevation, FontRole, Metrics, Motion, Radii, ShadowToken, Spacing,
    TypeScale, TypeStyle, c, ca, ch,
};
