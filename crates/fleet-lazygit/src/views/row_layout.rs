//! The line layout every diff surface shares: tints, gutters, the sign column and one
//! syntax-coloured, word-marked payload line.
//!
//! Both the lazygit panels and the reusable inline [`crate::diff_view::DiffView`] draw payload
//! rows from here, so a row is the same geometry in a full-window patch and inside an agent's
//! tool row. Only the wash differs, which is what [`RowPalette`] selects: lazygit keeps the
//! terminal ANSI hues its presentation code names, the inline view uses the semantic
//! `diff_added` / `diff_removed` tokens.
//!
//! Three layers compose per payload row, in this order:
//!
//! 1. **Row tint** — a wash over the theme background.
//! 2. **Syntax runs** from [`super::syntax`], as `HighlightStyle { color }`.
//! 3. **Word marks** from [`super::intraline`], as `HighlightStyle { background_color }` — a
//!    stronger wash on top of the already-tinted row.
//!
//! Layers 2 and 3 *must* go through [`gpui::combine_highlights`] before reaching
//! `StyledText::with_default_highlights`, which walks a monotonic cursor and panics on an
//! unsorted or overlapping range list. Syntax and word ranges overlap constantly.

use std::ops::Range;

use fleet_ui_kit::prelude::*;
use fleet_ui_kit::theme::ch;
use fleet_ui_kit::{Theme, ThemeMode};
use gpui::{
    AnyElement, FontFeatures, FontWeight, HighlightStyle, Hsla, SharedString, StyledText,
    TextStyle, WhiteSpace, combine_highlights, div, px,
};

use super::Ansi;
use super::diff_model::{DiffModel, RowKind, marker};
use super::syntax::Bucket;

/// The width of the `+` / `-` sign column, in characters.
pub(crate) const SIGN_CH: f32 = 1.0;

/// The gap between the sign column and the code, in characters.
pub(crate) const SIGN_GAP_CH: f32 = 1.0;

/// The three washes one diff colour contributes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiffTints {
    /// Behind the whole row.
    pub(crate) row: Hsla,
    /// Behind the line-number gutters — one step lighter, so the columns read as a rail.
    pub(crate) gutter: Hsla,
    /// Behind the words that actually changed.
    pub(crate) emphasis: Hsla,
    /// The 2 px bar at the very left edge, and the `+` / `-` sign.
    pub(crate) marker: Hsla,
}

/// Derives the washes for one diff colour from the theme, rather than adding tokens.
///
/// `Hsla::blend` composites its argument **over** the receiver and keeps the receiver's alpha,
/// so `bg.blend(hue.opacity(a))` is an opaque tinted background — Zed's own
/// `flattened_background_color` idiom.
#[must_use]
pub(crate) fn tints(theme: &Theme, ansi: Ansi) -> DiffTints {
    let hue = ansi.color(theme);
    let (row, emphasis, gutter) = match theme.mode {
        ThemeMode::Dark => (0.12, 0.26, 0.08),
        ThemeMode::Light => (0.16, 0.32, 0.10),
    };
    DiffTints {
        row: theme.colors.bg.blend(hue.opacity(row)),
        gutter: theme.colors.bg.blend(hue.opacity(gutter)),
        emphasis: theme.colors.bg.blend(hue.opacity(emphasis)),
        marker: hue,
    }
}

/// Which washes a payload row's kind resolves to.
///
/// The two palettes are the two places a diff is drawn: the lazygit panels, whose presentation
/// code names ANSI colours, and the inline agent diff, which the design system tokenises.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowPalette {
    /// The wash an added line carries.
    added: DiffTints,
    /// The wash a removed line carries.
    removed: DiffTints,
}

impl RowPalette {
    /// lazygit's palette: the terminal green and red, derived by [`tints`].
    #[must_use]
    pub(crate) fn ansi(theme: &Theme) -> Self {
        Self {
            added: tints(theme, Ansi::Green),
            removed: tints(theme, Ansi::Red),
        }
    }

    /// The inline palette: the semantic `diff_added` / `diff_removed` tokens, which are already
    /// the canvas's 14 % washes.
    ///
    /// They composite onto `surface` rather than `bg`, because the inline diff is drawn inside a
    /// panel-background card; blending onto the ground would leave every row a shade off it.
    #[must_use]
    pub(crate) fn tokens(theme: &Theme) -> Self {
        let ground = theme.colors.surface;
        Self {
            added: token_tints(ground, theme.colors.diff_added, theme.colors.success),
            removed: token_tints(ground, theme.colors.diff_removed, theme.colors.danger),
        }
    }

    /// The washes for one row kind, or `None` for a row that keeps the plain background.
    #[must_use]
    pub(crate) fn for_kind(self, kind: RowKind) -> Option<DiffTints> {
        match kind {
            RowKind::Added => Some(self.added),
            RowKind::Removed => Some(self.removed),
            _ => None,
        }
    }
}

/// The three washes a semantic diff token contributes.
///
/// The token itself is the row wash. The gutter is half of it and the emphasis is the same hue
/// at double the alpha, so the ordering [`RowPalette`] promises — emphasis over row over gutter —
/// holds for the tokens exactly as it does for the ANSI hues.
#[must_use]
fn token_tints(ground: Hsla, wash: Hsla, marker: Hsla) -> DiffTints {
    // `Hsla::opacity` *scales* the alpha it is given, so the token's own 14 % has to be lifted
    // back to a solid hue first; otherwise the emphasis wash comes out fainter than the row it
    // is supposed to stand out from.
    let hue = Hsla { a: 1.0, ..wash };
    let alpha = wash.a;
    DiffTints {
        row: ground.blend(hue.opacity(alpha)),
        gutter: ground.blend(hue.opacity(alpha * 0.5)),
        emphasis: ground.blend(hue.opacity((alpha * 2.0).min(1.0))),
        marker,
    }
}

/// The one `TextStyle` every payload line is shaped with.
///
/// Ligatures are off: a face that renders `!=` as a single glyph silently moves the column grid.
/// `WhiteSpace::Nowrap` short-circuits `TextLayout`'s wrap machinery entirely.
pub(crate) fn line_style(theme: &Theme, color: Hsla) -> TextStyle {
    TextStyle {
        color,
        font_family: theme.font_mono.clone(),
        font_features: FontFeatures::disable_ligatures(),
        font_size: theme.text.data.size.into(),
        line_height: gpui::DefiniteLength::Absolute(theme.text.data.line_height.into()),
        font_weight: FontWeight::NORMAL,
        white_space: WhiteSpace::Nowrap,
        ..TextStyle::default()
    }
}

/// Applies the mono type role to a container, so `StyledText`'s own layout agrees with the runs
/// we hand it — it reads font size and line height from the *inherited* style.
pub(crate) fn mono(element: gpui::Div, theme: &Theme) -> gpui::Div {
    element
        .font_family(theme.font_mono.clone())
        .text_size(theme.text.data.size)
        .line_height(theme.text.data.line_height)
        .whitespace_nowrap()
}

/// One payload line, syntax-coloured with its changed words marked.
pub(crate) fn payload(
    text: &SharedString,
    runs: &[(Range<usize>, Bucket)],
    words: &[Range<usize>],
    base: Hsla,
    emphasis: Option<Hsla>,
    theme: &Theme,
) -> AnyElement {
    if text.is_empty() {
        return div().into_any_element();
    }
    let style = line_style(theme, base);
    let syntax = runs.iter().map(|(range, bucket)| {
        (
            range.clone(),
            HighlightStyle {
                color: Some(bucket.color(theme)),
                ..HighlightStyle::default()
            },
        )
    });
    let marks = words.iter().filter_map(|range| {
        Some((
            range.clone(),
            HighlightStyle {
                background_color: Some(emphasis?),
                ..HighlightStyle::default()
            },
        ))
    });
    // `combine_highlights` sweeps the endpoints and emits disjoint, sorted ranges. Without it
    // `with_default_highlights` panics on the first overlap between a syntax run and a word mark.
    let highlights: Vec<(Range<usize>, HighlightStyle)> =
        combine_highlights(syntax, marks).collect();
    StyledText::new(text.clone())
        .with_default_highlights(&style, highlights)
        .into_any_element()
}

/// How a row is selected, and where the payload is scrolled to.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RowStyle {
    /// Whether the row is inside the current selection.
    pub(crate) selected: bool,
    /// Whether the row is the cursor itself, which also earns the 2 px left bar.
    pub(crate) cursor: bool,
    /// Whether the panel owning this list has the keyboard.
    pub(crate) focused: bool,
    /// Horizontal payload offset, in pixels.
    pub(crate) h_scroll: f32,
}

/// The background of a row, with the selection composited over the diff tint rather than
/// replacing it — a selected added line must still read as added.
///
/// The wash is the accent hue at a low alpha rather than `row_selected` at a high one: the
/// latter is an opaque blue-grey and swallows the green or red the row is carrying, which is
/// precisely the information a staging selection must not hide.
pub(crate) fn background(theme: &Theme, tint: Option<Hsla>, style: RowStyle) -> Option<Hsla> {
    if !style.selected {
        return tint;
    }
    let base = tint.unwrap_or(theme.colors.bg);
    let alpha = if style.focused { 0.26 } else { 0.10 };
    Some(base.blend(theme.colors.accent.opacity(alpha)))
}

/// One line-number gutter cell.
pub(crate) fn gutter(
    number: Option<u32>,
    digits: usize,
    tint: Option<Hsla>,
    theme: &Theme,
) -> AnyElement {
    let text = number.map_or_else(String::new, |number| number.to_string());
    let mut cell = mono(div(), theme)
        .flex_none()
        .w(ch(digits as f32 + 1.0))
        .h_full()
        .flex()
        .items_center()
        .justify_end()
        .pr(theme.space.xs)
        .text_color(theme.colors.text_muted);
    if let Some(tint) = tint {
        cell = cell.bg(tint);
    }
    cell.child(text).into_any_element()
}

/// Which line-number gutters a payload row draws.
///
/// Unified shows both, so a reader can follow either side. Split shows one per column, because
/// the column *is* the side and repeating the other number is noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gutters {
    /// Old and new, in that order.
    Both,
    /// The old side only — the left column of a split.
    Old,
    /// The new side only — the right column of a split.
    New,
}

/// One payload row: marker bar, the gutters, the sign column and the scrolled payload.
pub(crate) fn line_row(
    model: &DiffModel,
    index: usize,
    style: RowStyle,
    gutters: Gutters,
    palette: RowPalette,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let Some(row) = model.rows.get(index) else {
        return div().h(theme.metrics.diff_row_h).into_any_element();
    };
    let tints = palette.for_kind(row.kind);
    let base = match row.kind {
        RowKind::Context => theme.colors.text_secondary,
        RowKind::Other => theme.colors.text_muted,
        _ => theme.colors.text,
    };
    let runs = model.runs_for(index);
    let mut element = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(theme.metrics.diff_row_h)
        .overflow_hidden();
    if let Some(background) = background(theme, tints.map(|tint| tint.row), style) {
        element = element.bg(background);
    }
    // Zed's gutter strip: a thin bar flush at x = 0, so a change is legible even when the row
    // tint is washed out by a selection on top of it. Every row of a *selection* carries the
    // cursor colour, which is what makes a multi-row range read as one block.
    let bar = if style.selected {
        Some(theme.colors.cursor_bar)
    } else {
        tints.map(|tint| tint.marker)
    };
    if let Some(bar) = bar {
        element = element.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(theme.metrics.focus_ring_w)
                .h(theme.metrics.diff_row_h)
                .bg(bar),
        );
    }
    let wash = tints.map(|tint| tint.gutter);
    if matches!(gutters, Gutters::Both | Gutters::Old) {
        element = element.child(gutter(row.old_no, model.digits, wash, theme));
    }
    if matches!(gutters, Gutters::Both | Gutters::New) {
        element = element.child(gutter(row.new_no, model.digits, wash, theme));
    }
    element
        // The sign gets its own fixed `SIGN_CH` cell and the code a fixed `SIGN_GAP_CH` of
        // padding after it, so the code column starts at the same x on every row whatever its
        // kind — a `+`/`-` glued to an unindented line and floating away from an indented one is
        // how the columns drift by a character. GitHub, diffs.com and Zed all do this.
        .child(
            mono(div(), theme)
                .flex_none()
                .w(ch(SIGN_CH))
                .text_color(tints.map_or(theme.colors.text_muted, |tint| tint.marker))
                .child(marker(row.kind)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pl(ch(SIGN_GAP_CH))
                .overflow_hidden()
                .child(if let Some(line) = model.long_lines.get(&index) {
                    line.element(style.h_scroll, theme.text.data.line_height)
                } else {
                    mono(div(), theme)
                        .ml(px(-style.h_scroll))
                        .flex_none()
                        .child(payload(
                            &row.text,
                            &runs,
                            &row.words,
                            base,
                            tints.map(|tint| tint.emphasis),
                            theme,
                        ))
                        .into_any_element()
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_match_the_patch_sign_column() {
        assert_eq!(marker(RowKind::Added), "+");
        assert_eq!(marker(RowKind::Removed), "-");
        assert_eq!(marker(RowKind::Context), " ");
        assert_eq!(marker(RowKind::FileHeader), "");
    }

    /// Every wash a palette hands a row must satisfy the same three invariants, whichever hue
    /// it came from: opaque, and emphasis further from the background than the row, which is
    /// further from it than the gutter.
    fn assert_ordered(ground: Hsla, washes: DiffTints) {
        assert_eq!(washes.row.a, 1.0, "a row wash must be opaque");
        assert_eq!(washes.gutter.a, 1.0);
        assert_eq!(washes.emphasis.a, 1.0);
        let distance = |colour: Hsla| (colour.l - ground.l).abs() + (colour.s).abs();
        assert!(distance(washes.emphasis) > distance(washes.row));
        assert!(distance(washes.row) > distance(washes.gutter));
    }

    #[test]
    fn tints_stay_opaque_and_ordered() {
        let theme = Theme::dark();
        assert_ordered(theme.colors.bg, tints(&theme, Ansi::Green));
    }

    #[test]
    fn the_token_palette_keeps_the_same_ordering_in_both_modes() {
        for theme in [Theme::dark(), Theme::light()] {
            let palette = RowPalette::tokens(&theme);
            let added = palette
                .for_kind(RowKind::Added)
                .expect("an added row is washed");
            let removed = palette
                .for_kind(RowKind::Removed)
                .expect("a removed row is washed");
            assert_ordered(theme.colors.surface, added);
            assert_ordered(theme.colors.surface, removed);
            // A context line keeps the ground it sits on.
            assert!(palette.for_kind(RowKind::Context).is_none());
            assert!(palette.for_kind(RowKind::HunkHeader).is_none());
        }
    }

    #[test]
    fn the_token_palette_uses_the_semantic_diff_tokens() {
        let theme = Theme::dark();
        let palette = RowPalette::tokens(&theme);
        let added = palette.for_kind(RowKind::Added).expect("washed");
        let removed = palette.for_kind(RowKind::Removed).expect("washed");
        // The canvas fixes these as the 14 % washes; the row is exactly the token over the
        // ground, so a theme edit moves the diff with it rather than needing a local recipe.
        assert_eq!(
            added.row,
            theme.colors.surface.blend(theme.colors.diff_added)
        );
        assert_eq!(
            removed.row,
            theme.colors.surface.blend(theme.colors.diff_removed)
        );
        assert_eq!(added.marker, theme.colors.success);
        assert_eq!(removed.marker, theme.colors.danger);
    }
}
