//! `AgeLabel` — a relative time in exactly one unit.
//!
//! §3.3 and §3.5: `2h`, `1d`, `3w`. Never two units, never an absolute timestamp in a row.
//! The `–` for "no age yet" is a dash, never `0`.

use gpui::{App, SharedString, Window, prelude::*};

use crate::{text::Text, tone::Tone};

/// Format a duration in seconds as a single-unit relative age.
///
/// `0s`-`59s` -> `Ns`; minutes -> `Nm`; hours -> `Nh`; days -> `Nd`; from 14 days -> `Nw`;
/// from 365 days -> `Ny`.
pub fn format_age(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        86_400..=1_209_599 => format!("{}d", s / 86_400),
        1_209_600..=31_535_999 => format!("{}w", s / 604_800),
        _ => format!("{}y", s / 31_536_000),
    }
}

/// One relative age, right-aligned in the age column.
#[derive(IntoElement)]
pub struct AgeLabel {
    text: SharedString,
    tone: Tone,
    mono: bool,
}

impl AgeLabel {
    /// From an age in seconds.
    pub fn from_secs(seconds: i64) -> Self {
        Self {
            text: SharedString::from(format_age(seconds)),
            tone: Tone::Secondary,
            mono: false,
        }
    }

    /// The "no age" rendering: an en dash, never `0`.
    ///
    /// A nullable *fact* is a different thing and uses [`super::FactValue::Null`], which draws
    /// an em dash — the two are distinguishable on purpose.
    pub fn none() -> Self {
        Self {
            text: SharedString::new_static("\u{2013}"),
            tone: Tone::Muted,
            mono: false,
        }
    }

    /// From an already-formatted string (the daemon may format ages itself).
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Secondary,
            mono: false,
        }
    }

    /// Set the tone. Lower it to [`Tone::Muted`] for an age derived from a stale fact (§2.6).
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Render in the data face, so a column of ages lines up digit for digit.
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }
}

impl RenderOnce for AgeLabel {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if self.mono {
            Text::data(self.text).tone(self.tone)
        } else {
            Text::ui(self.text).tone(self.tone)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_age;

    #[test]
    fn single_unit_only() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(7_200), "2h");
        assert_eq!(format_age(432_000), "5d");
        assert_eq!(format_age(1_814_400), "3w");
        assert_eq!(format_age(63_072_000), "2y");
    }
}
