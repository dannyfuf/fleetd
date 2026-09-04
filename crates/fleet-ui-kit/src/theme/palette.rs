//! Base color palettes from which semantic themes are derived.
//!
//! Fleet has exactly one: the terminal table a `Palette(u8)` cell resolves through. The app's
//! own colors are semantic roles ([`super::ColorTokens`]), not a palette, because §1.4 of the
//! UX spec allows four colors and three neutrals and nothing else.

use gpui::Hsla;

use super::tokens::{c, ca};

/// The terminal color table a `Palette(u8)` cell resolves through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalPalette {
    /// ANSI 0-15: black, red, green, yellow, blue, magenta, cyan, white, then the bright 8.
    pub ansi: [Hsla; 16],
    /// What a `Default` foreground cell resolves to.
    pub foreground: Hsla,
    /// What a `Default` background cell resolves to.
    pub background: Hsla,
    /// Block / bar / underline cursor color.
    pub cursor: Hsla,
    /// Selection fill on the mirror grid.
    pub selection: Hsla,
}

impl TerminalPalette {
    /// Dark terminal palette.
    pub fn dark() -> Self {
        Self {
            ansi: [
                c(0x0E1013),
                c(0xF85149),
                c(0x3FB950),
                c(0xD29922),
                c(0x58A6FF),
                c(0xBC8CFF),
                c(0x39C5CF),
                c(0xB1BAC4),
                c(0x5A6069),
                c(0xFF7B72),
                c(0x56D364),
                c(0xE3B341),
                c(0x79C0FF),
                c(0xD2A8FF),
                c(0x56D4DD),
                c(0xF0F6FC),
            ],
            foreground: c(0xE6E8EB),
            background: c(0x0E1013),
            cursor: c(0x58A6FF),
            selection: ca(0x58A6FF47),
        }
    }

    /// Light terminal palette.
    pub fn light() -> Self {
        Self {
            ansi: [
                c(0x24292F),
                c(0xCF222E),
                c(0x1A7F37),
                c(0x9A6700),
                c(0x0969DA),
                c(0x8250DF),
                c(0x1B7C83),
                c(0x6E7781),
                c(0x57606A),
                c(0xA40E26),
                c(0x116329),
                c(0x7D4E00),
                c(0x0550AE),
                c(0x6639BA),
                c(0x3192AA),
                c(0x8C959F),
            ],
            foreground: c(0x16181D),
            background: c(0xFBFBFC),
            cursor: c(0x0969DA),
            selection: ca(0x0969DA33),
        }
    }

    /// Resolve any xterm palette index: 0-15 from the table, 16-231 from the 6x6x6 cube,
    /// 232-255 from the 24-step grayscale ramp.
    pub fn color(&self, index: u8) -> Hsla {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let i = index - 16;
                let steps = [0u32, 95, 135, 175, 215, 255];
                let r = steps[(i / 36) as usize];
                let g = steps[((i % 36) / 6) as usize];
                let b = steps[(i % 6) as usize];
                c((r << 16) | (g << 8) | b)
            }
            _ => {
                let level = 8 + 10 * (index as u32 - 232);
                c((level << 16) | (level << 8) | level)
            }
        }
    }
}
