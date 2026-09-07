//! Ellipsis at a fixed character budget.
//!
//! Fleet truncates at a `ch` budget, not at a pixel width, because every column ladder in the
//! UX spec is expressed in `ch`. Pixel-level ellipsis (`Styled::text_ellipsis`) is still the
//! right tool for a flex column with no fixed budget; use [`Truncate`] when the spec names a
//! `ch` number.

use gpui::SharedString;

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

/// Shorten `text` to at most `budget` characters, placing the ellipsis per `mode`.
///
/// Counts `char`s, not bytes and not grapheme clusters: branch names, repo slugs and PR titles
/// are the only strings this is applied to, and column budgets are stated in `ch`.
pub fn truncate(text: &str, budget: usize, mode: Truncate) -> SharedString {
    shortened(text, budget, mode).unwrap_or_else(|| SharedString::new(text))
}

/// Truncate shared text, retaining its storage when it already fits the scalar budget.
pub fn truncate_shared(text: SharedString, budget: usize, mode: Truncate) -> SharedString {
    shortened(&text, budget, mode).unwrap_or(text)
}

fn shortened(text: &str, budget: usize, mode: Truncate) -> Option<SharedString> {
    text.chars().nth(budget)?;
    if budget == 0 {
        return Some(SharedString::default());
    }
    let keep = budget - 1;
    let (front, back) = match mode {
        Truncate::Tail => (keep, 0),
        Truncate::Head => (0, keep),
        Truncate::Middle => (keep.div_ceil(2), keep / 2),
    };
    let prefix_end = text
        .char_indices()
        .nth(front)
        .map_or(text.len(), |(byte, _)| byte);
    let suffix_start = if back == 0 {
        text.len()
    } else {
        text.char_indices()
            .rev()
            .nth(back - 1)
            .map_or(0, |(byte, _)| byte)
    };
    let mut shortened =
        String::with_capacity(prefix_end + ELLIPSIS.len_utf8() + text.len() - suffix_start);
    shortened.push_str(&text[..prefix_end]);
    shortened.push(ELLIPSIS);
    shortened.push_str(&text[suffix_start..]);
    Some(shortened.into())
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
    fn multibyte_scalars_keep_valid_boundaries_in_every_mode() {
        assert_eq!(truncate("é漢😀xyz", 4, Truncate::Tail), "é漢😀…");
        assert_eq!(truncate("é漢😀xyz", 4, Truncate::Head), "…xyz");
        assert_eq!(truncate("é漢😀xyz", 4, Truncate::Middle), "é漢…z");
        assert_eq!(truncate("é漢😀xyz", 0, Truncate::Tail), "");
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
                assert!(out.chars().count() <= budget);
            }
        }
    }
}
