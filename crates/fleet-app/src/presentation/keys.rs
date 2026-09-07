/// Action-name spelling shared with the Git UI: `hub::MoveDown` reads "move down".
pub use fleet_lazygit::keymap::humanize;

/// Prepared once per query; matching borrows each candidate without allocating a lowercase copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FuzzyQuery(Vec<char>);
impl FuzzyQuery {
    pub fn new(query: &str) -> Self {
        Self(
            query
                .chars()
                .filter(|character| !character.is_whitespace())
                .map(|character| character.to_ascii_lowercase())
                .collect(),
        )
    }
    pub fn matches(&self, label: &str) -> bool {
        let mut chars = label
            .chars()
            .map(|character| character.to_ascii_lowercase());
        self.0
            .iter()
            .all(|wanted| chars.any(|actual| actual == *wanted))
    }
}

/// Case-insensitive substring test against an already-lowercased needle.
///
/// The list filters run this over every visible row on every keystroke, so neither side is
/// copied: the haystack is folded lazily, one character at a time.
#[must_use]
pub fn contains_folded(haystack: &str, lowered_needle: &str) -> bool {
    if lowered_needle.is_empty() {
        return true;
    }
    haystack.char_indices().any(|(start, _)| {
        let mut folded = haystack[start..].chars().flat_map(char::to_lowercase);
        lowered_needle
            .chars()
            .all(|wanted| folded.next() == Some(wanted))
    })
}

/// The Help and Palette spelling of a keymap chord.
///
/// The substitutions are ordered longest-name-first, so `backspace` becomes `⌫` instead of
/// being eaten by the `space` rule. Unlike [`fleet_lazygit::keymap::pretty_keys`], `cmd-` is
/// left spelled out: Fleet's clipboard rows read `cmd-c`, not `⌘c`.
pub fn pretty_keys(keys: &str) -> String {
    keys.replace("ctrl-", "^")
        .replace("shift-", "S-")
        .replace("alt-", "⌥")
        .replace("backspace", "⌫")
        .replace("escape", "esc")
        .replace("enter", "⏎")
        .replace("tab", "⇥")
        .replace("space", "␣")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_queries_preserve_ascii_and_whitespace_policy() {
        let query = FuzzyQuery::new(" O p s ");
        assert!(query.matches("Open Session"));
        assert!(!query.matches("Stop"));
        assert!(FuzzyQuery::new(" \t").matches("anything"));
        assert!(!FuzzyQuery::new("é").matches("É"));
    }
    #[test]
    fn folded_containment_matches_a_lowercased_copy() {
        for (haystack, needle) in [
            ("feat/RUT-validator", "rut"),
            ("buk/payroll", "pay"),
            ("Ärger", "ärger"),
            ("plain", ""),
        ] {
            assert!(
                contains_folded(haystack, needle),
                "{haystack:?} contains {needle:?}"
            );
            assert_eq!(
                contains_folded(haystack, needle),
                haystack.to_lowercase().contains(needle)
            );
        }
        assert!(!contains_folded("buk/payroll", "rut"));
    }

    #[test]
    fn key_descriptions_preserve_existing_spelling() {
        assert_eq!(humanize("fleet::OpenJobs"), "open jobs");
        assert_eq!(pretty_keys("ctrl-s shift-tab alt-enter"), "^s S-⇥ ⌥⏎");
        assert_eq!(
            pretty_keys("backspace"),
            "⌫",
            "the longer name wins over `space`"
        );
        assert_eq!(pretty_keys("space"), "␣");
    }
}
