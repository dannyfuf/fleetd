//! Ellipsis at a fixed display-column budget.
//!
//! Fleet truncates at a `ch` budget, not at a pixel width, because every column ladder in the
//! UX spec is expressed in `ch`. Pixel-level ellipsis (`Styled::text_ellipsis`) is still the
//! right tool for a flex column with no fixed budget; use [`Truncate`] when the spec names a
//! `ch` number.

use gpui::SharedString;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Where the ellipsis goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Truncate {
    /// `…/payroll` — keep the end. Used for `owner/name` columns.
    Head,
    /// `feat/pay…-fix` — keep both ends. Used for branches and paths.
    Middle,
    /// `Fix RUT valid…` — keep the start. Used for PR titles.
    #[default]
    Tail,
}

/// The single-character ellipsis Fleet uses everywhere.
pub const ELLIPSIS: char = '\u{2026}';

/// Shorten `text` to at most `budget` display columns, placing the ellipsis per `mode`.
/// Grapheme clusters are never split.
pub fn truncate(text: &str, budget: usize, mode: Truncate) -> SharedString {
    shortened(text, budget, mode).unwrap_or_else(|| SharedString::new(text))
}

/// Truncate shared text, retaining its storage when it already fits the column budget.
pub fn truncate_shared(text: SharedString, budget: usize, mode: Truncate) -> SharedString {
    shortened(&text, budget, mode).unwrap_or(text)
}

fn shortened(text: &str, budget: usize, mode: Truncate) -> Option<SharedString> {
    if UnicodeWidthStr::width(text) <= budget {
        return None;
    }
    if budget == 0 {
        return Some(SharedString::default());
    }
    let keep_columns = budget.saturating_sub(UnicodeWidthStr::width(ELLIPSIS_STR));
    let (front_columns, back_columns) = match mode {
        Truncate::Tail => (keep_columns, 0),
        Truncate::Head => (0, keep_columns),
        Truncate::Middle => (keep_columns.div_ceil(2), keep_columns / 2),
    };
    let prefix_end = prefix_end_for_columns(text, front_columns);
    let suffix_start = suffix_start_for_columns(text, back_columns).max(prefix_end);
    let mut shortened =
        String::with_capacity(prefix_end + ELLIPSIS.len_utf8() + text.len() - suffix_start);
    shortened.push_str(&text[..prefix_end]);
    shortened.push(ELLIPSIS);
    shortened.push_str(&text[suffix_start..]);
    Some(shortened.into())
}

const ELLIPSIS_STR: &str = "…";

fn prefix_end_for_columns(text: &str, budget: usize) -> usize {
    let mut columns = 0;
    text.grapheme_indices(true)
        .take_while(|(_, grapheme)| {
            let next = columns + UnicodeWidthStr::width(*grapheme);
            let fits = next <= budget;
            if fits {
                columns = next;
            }
            fits
        })
        .map(|(start, grapheme)| start + grapheme.len())
        .last()
        .unwrap_or(0)
}

fn suffix_start_for_columns(text: &str, budget: usize) -> usize {
    let mut columns = 0;
    text.grapheme_indices(true)
        .rev()
        .take_while(|(_, grapheme)| {
            let next = columns + UnicodeWidthStr::width(*grapheme);
            let fits = next <= budget;
            if fits {
                columns = next;
            }
            fits
        })
        .map(|(start, _)| start)
        .last()
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_text_keeps_storage_when_it_fits() {
        let text =
            SharedString::from("a long shared label that exceeds inline storage".to_string());
        let result = truncate_shared(text.clone(), 100, Truncate::Tail);
        assert_eq!(text.as_ptr(), result.as_ptr());
    }

    #[test]
    fn graphemes_and_wide_cells_obey_column_budget() {
        assert_eq!(truncate("é漢😀xyz", 4, Truncate::Tail), "é漢…");
        assert_eq!(truncate("é漢😀xyz", 4, Truncate::Head), "…xyz");
        assert_eq!(truncate("é漢😀xyz", 4, Truncate::Middle), "é…z");
        assert_eq!(truncate("e\u{301}x", 1, Truncate::Tail), "…");
        assert_eq!(truncate("界界", 3, Truncate::Tail), "界…");
        assert_eq!(truncate("é漢😀xyz", 0, Truncate::Tail), "");
        for mode in [Truncate::Head, Truncate::Middle, Truncate::Tail] {
            let output = truncate("e\u{301}界😀xyz", 5, mode);
            assert!(UnicodeWidthStr::width(output.as_ref()) <= 5);
        }
    }

    #[test]
    fn keeps_short_strings() {
        assert_eq!(truncate("main", 8, Truncate::Tail).as_ref(), "main");
    }

    #[test]
    fn truncates_each_way() {
        assert_eq!(
            truncate("feat/payroll-fix", 8, Truncate::Tail).as_ref(),
            "feat/pa\u{2026}"
        );
        assert_eq!(
            truncate("feat/payroll-fix", 8, Truncate::Head).as_ref(),
            "\u{2026}oll-fix"
        );
        assert_eq!(
            truncate("feat/payroll-fix", 8, Truncate::Middle).as_ref(),
            "feat\u{2026}fix"
        );
    }

    #[test]
    fn budget_is_respected() {
        for mode in [Truncate::Head, Truncate::Middle, Truncate::Tail] {
            for budget in 0..12 {
                let out = truncate("feat/payroll-fix", budget, mode);
                assert!(UnicodeWidthStr::width(out.as_ref()) <= budget);
            }
        }
    }
}
