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
    let len = text.chars().count();
    if len <= budget {
        return SharedString::from(text.to_string());
    }
    if budget == 0 {
        return SharedString::new_static("");
    }
    if budget == 1 {
        return SharedString::from(ELLIPSIS.to_string());
    }
    let keep = budget - 1;
    let chars: Vec<char> = text.chars().collect();
    let out: String = match mode {
        Truncate::Tail => chars[..keep].iter().collect::<String>() + &ELLIPSIS.to_string(),
        Truncate::Head => ELLIPSIS.to_string() + &chars[len - keep..].iter().collect::<String>(),
        Truncate::Middle => {
            let front = keep.div_ceil(2);
            let back = keep - front;
            let mut s = chars[..front].iter().collect::<String>();
            s.push(ELLIPSIS);
            s.push_str(&chars[len - back..].iter().collect::<String>());
            s
        }
    };
    SharedString::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                assert!(out.chars().count() <= budget.max(1));
            }
        }
    }
}
