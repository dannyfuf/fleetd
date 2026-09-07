//! `PriorityGlyph` — the five priority marks of the board, drawn as shapes.
//!
//! §1.4 of the UX spec: color is semantic and shape carries the information. Priority is
//! therefore a **bar ladder** and not five colored dots — `High`, `Medium` and `Low` differ by
//! how many bars are lit (3 / 2 / 1), so the mark is legible in a grayscale screenshot and to
//! a red-green blind reader. Only `Urgent` earns a color, because only `Urgent` is a state the
//! board wants the eye to jump to, and it uses the one token that means "broken / act now".
//!
//! `None` is a dashed hollow dot: an empty slot that says "nobody has decided", which is not
//! the same as `Low`.

use gpui::{App, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// How many bars the ladder has in total.
pub const PRIORITY_BARS: usize = 3;

/// One priority level. Mirrors `fleet_core::board::Priority` without depending on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum PriorityLevel {
    /// Act now. The only level with a color.
    Urgent,
    /// Three bars.
    High,
    /// Two bars.
    Medium,
    /// One bar.
    Low,
    /// Undecided: a dashed hollow dot.
    #[default]
    None,
}

impl PriorityLevel {
    /// Every level, in board order (highest first).
    pub const ALL: [PriorityLevel; 5] = [
        PriorityLevel::Urgent,
        PriorityLevel::High,
        PriorityLevel::Medium,
        PriorityLevel::Low,
        PriorityLevel::None,
    ];

    /// The word a picker and [`PriorityGlyph::with_label`] show.
    pub fn label(self) -> &'static str {
        match self {
            PriorityLevel::Urgent => "Urgent",
            PriorityLevel::High => "High",
            PriorityLevel::Medium => "Medium",
            PriorityLevel::Low => "Low",
            PriorityLevel::None => "None",
        }
    }

    /// How many of the [`PRIORITY_BARS`] bars are lit. `Urgent` and `None` do not use the ladder.
    pub fn lit_bars(self) -> usize {
        match self {
            PriorityLevel::Urgent | PriorityLevel::None => 0,
            PriorityLevel::High => 3,
            PriorityLevel::Medium => 2,
            PriorityLevel::Low => 1,
        }
    }
}

/// The priority mark: a filled danger marker, a bar ladder, or a dashed dot.
#[derive(IntoElement)]
pub struct PriorityGlyph {
    level: PriorityLevel,
    with_label: bool,
}

impl PriorityGlyph {
    /// The mark for `level`.
    pub fn new(level: PriorityLevel) -> Self {
        Self {
            level,
            with_label: false,
        }
    }

    /// Draw the word next to the mark: the property row of the card detail, and the pickers.
    /// A card tile never does — there the mark alone is the word.
    pub fn with_label(mut self, with_label: bool) -> Self {
        self.with_label = with_label;
        self
    }
}

impl RenderOnce for PriorityGlyph {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let level = self.level;
        // The ladder is built from real tokens: 2 px bars (the focus-ring width) that grow
        // 4 → 6 → 8 px (the spacing step and the two dot sizes).
        let bar_w = theme.metrics.focus_ring_w;
        let heights = [
            theme.space.xs,
            theme.metrics.dot_size_small,
            theme.metrics.dot_size,
        ];
        let box_h = theme.metrics.dot_size;
        let lit = level.lit_bars();

        let mark = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_end()
            .justify_center()
            .gap(theme.space.xxs)
            .h(box_h)
            .map(|el| match level {
                // A filled square, not a bar: the shape itself is the alarm, and the color
                // only confirms it.
                PriorityLevel::Urgent => el.child(
                    div()
                        .flex_none()
                        .size(box_h)
                        .rounded(theme.radii.xs)
                        .bg(theme.colors.danger),
                ),
                // An empty slot: hollow and dashed, so it reads as "not set" and never as
                // "set to the smallest value".
                PriorityLevel::None => el.child(
                    div()
                        .flex_none()
                        .size(theme.metrics.dot_size_small)
                        .rounded(theme.radii.full)
                        .border_1()
                        .border_dashed()
                        .border_color(theme.colors.text_muted),
                ),
                _ => el.children((0..PRIORITY_BARS).map(|index| {
                    div()
                        .flex_none()
                        .w(bar_w)
                        .h(heights[index])
                        .rounded(theme.radii.xs)
                        .bg(if index < lit {
                            theme.colors.text_secondary
                        } else {
                            theme.colors.text_muted
                        })
                })),
            });

        div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(theme.space.xs)
            .child(mark)
            .when(self.with_label, |el| {
                el.child(
                    Text::ui(level.label()).tone(if level == PriorityLevel::Urgent {
                        Tone::Danger
                    } else {
                        Tone::Secondary
                    }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_lights_three_two_one() {
        assert_eq!(PriorityLevel::High.lit_bars(), 3);
        assert_eq!(PriorityLevel::Medium.lit_bars(), 2);
        assert_eq!(PriorityLevel::Low.lit_bars(), 1);
        assert_eq!(PriorityLevel::Urgent.lit_bars(), 0);
        assert_eq!(PriorityLevel::None.lit_bars(), 0);
    }

    #[test]
    fn every_level_has_a_word_and_a_place_in_all() {
        assert_eq!(PriorityLevel::ALL.len(), 5);
        assert_eq!(PriorityLevel::default(), PriorityLevel::None);
        for level in PriorityLevel::ALL {
            assert!(!level.label().is_empty());
        }
        assert!(PriorityLevel::Urgent < PriorityLevel::None);
    }
}
