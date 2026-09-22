//! WCAG 2 contrast ratios between two theme colours.
//!
//! The palette's accessibility promises (muted text is still 4.5:1 on a surface, a primary
//! button's label reads on its fill) are checked against this in tests and shown in the
//! gallery, so a token change that breaks one is caught where it is made.

use gpui::{Hsla, Rgba};

/// The WCAG AA minimum for body text.
pub const CONTRAST_AA: f32 = 4.5;

/// Relative luminance of one sRGB channel in `0.0..=1.0`.
fn channel(value: f32) -> f32 {
    if value <= 0.039_28 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(color: Rgba) -> f32 {
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// The WCAG contrast ratio (1.0 to 21.0) of `foreground` drawn on `background`.
///
/// A translucent foreground is composited onto the background first, and the background is
/// treated as opaque, which is how every theme ground is declared.
pub fn contrast_ratio(foreground: Hsla, background: Hsla) -> f32 {
    let background = Hsla {
        a: 1.0,
        ..background
    };
    let foreground = background.blend(foreground);
    let (a, b) = (
        luminance(foreground.to_rgb()),
        luminance(background.to_rgb()),
    );
    let (light, dark) = if a > b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorTokens, c};

    #[test]
    fn black_on_white_is_twenty_one() {
        let ratio = contrast_ratio(c(0x000000), c(0xFFFFFF));
        assert!((ratio - 21.0).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn every_text_role_meets_aa_on_every_ground_it_sits_on() {
        for (mode, colors) in [
            ("dark", ColorTokens::dark()),
            ("light", ColorTokens::light()),
        ] {
            let grounds = [
                ("bg", colors.bg),
                ("chrome", colors.chrome),
                ("surface", colors.surface),
                ("surface_raised", colors.surface_raised),
                ("elevated", colors.elevated),
            ];
            for (text_name, text) in [
                ("text", colors.text),
                ("text_secondary", colors.text_secondary),
                ("text_muted", colors.text_muted),
            ] {
                for (ground_name, ground) in grounds {
                    let ratio = contrast_ratio(text, ground);
                    assert!(
                        ratio >= CONTRAST_AA,
                        "{mode}: {text_name} on {ground_name} is {ratio:.2}:1"
                    );
                }
            }
            let ratio = contrast_ratio(colors.accent_fill_text, colors.accent_fill);
            assert!(
                ratio >= CONTRAST_AA,
                "{mode}: accent_fill_text on accent_fill is {ratio:.2}:1"
            );
        }
    }
}
