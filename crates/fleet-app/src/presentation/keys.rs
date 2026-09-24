/// Action-name spelling shared with the Git UI: `hub::MoveDown` reads "move down".
///
/// A debugging aid only. Nothing a person reads may be spelled from an action's type name:
/// labels come from [`crate::action_catalogue`], and its tests fail on a call outside this file.
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

/// Where a [`FuzzyQuery`] hit a label, and how good the hit is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher is better. Only comparable between matches of the same query.
    pub score: i32,
    /// The character indices of the label that matched, ascending.
    pub indices: Vec<usize>,
}

impl FuzzyQuery {
    /// Whether the query has nothing to match, so every label matches with score 0.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Scores `label` against the query, or `None` when it does not match at all.
    ///
    /// The ranking a palette needs, in order: the query as a prefix of the label, then as a
    /// run starting on a word boundary, then as a run anywhere, then as a scattered
    /// subsequence, where word-initial and consecutive hits count up and the spread counts
    /// down. Earlier runs beat later ones and shorter labels break ties, so `del` puts
    /// "Delete worktree" above "Undo delete" above "model-registry".
    #[must_use]
    pub fn score(&self, label: &str) -> Option<FuzzyMatch> {
        const PREFIX: i32 = 3000;
        const BOUNDARY: i32 = 2000;
        const RUN: i32 = 1000;
        let chars: Vec<char> = label
            .chars()
            .map(|character| character.to_ascii_lowercase())
            .collect();
        let wanted = &self.0;
        let length_penalty = i32::try_from(chars.len()).unwrap_or(i32::MAX) / 4;
        if wanted.is_empty() {
            return Some(FuzzyMatch {
                score: 0,
                indices: Vec::new(),
            });
        }
        if wanted.len() <= chars.len() {
            let starts = (0..=chars.len() - wanted.len())
                .filter(|start| chars[*start..*start + wanted.len()] == wanted[..]);
            let mut best: Option<(i32, usize)> = None;
            for start in starts {
                let offset = i32::try_from(start).unwrap_or(i32::MAX);
                let score = if start == 0 {
                    PREFIX
                } else if is_boundary(&chars, start) {
                    BOUNDARY - offset
                } else {
                    RUN - offset
                };
                if best.is_none_or(|(held, _)| score > held) {
                    best = Some((score, start));
                }
            }
            if let Some((score, start)) = best {
                return Some(FuzzyMatch {
                    score: score - length_penalty,
                    indices: (start..start + wanted.len()).collect(),
                });
            }
        }
        // A scattered subsequence, taken greedily from the left.
        let mut indices = Vec::with_capacity(wanted.len());
        let mut from = 0;
        for character in wanted {
            let found = chars[from..]
                .iter()
                .position(|actual| actual == character)?;
            indices.push(from + found);
            from += found + 1;
        }
        let boundaries = indices
            .iter()
            .filter(|index| is_boundary(&chars, **index))
            .count();
        let consecutive = indices
            .windows(2)
            .filter(|pair| pair[1] == pair[0] + 1)
            .count();
        let spread = indices.last().copied().unwrap_or(0) - indices.first().copied().unwrap_or(0);
        let score = i32::try_from(boundaries * 15 + consecutive * 10).unwrap_or(0)
            - i32::try_from(spread).unwrap_or(i32::MAX / 2)
            - length_penalty;
        Some(FuzzyMatch { score, indices })
    }
}

/// Whether the character at `index` starts a word: the first one, or one after a separator.
fn is_boundary(chars: &[char], index: usize) -> bool {
    index == 0
        || chars
            .get(index - 1)
            .is_some_and(|previous| !previous.is_alphanumeric())
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

/// The retiring Help and Palette spelling of a keymap chord (`^s`, `S-⇥`), now owned by the kit
/// beside [`fleet_ui_kit::Kbd`], which replaces it surface by surface.
pub use fleet_ui_kit::pretty_keys;

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
    fn scores_rank_prefix_then_word_start_then_run_then_scatter() {
        let query = FuzzyQuery::new("del");
        let score = |label: &str| query.score(label).map(|hit| hit.score);
        let prefix = score("Delete worktree");
        let word = score("Undo delete");
        let run = score("acme/api#model-registry");
        let scattered = score("Dump the log");
        assert!(prefix > word, "{prefix:?} > {word:?}");
        assert!(word > run, "{word:?} > {run:?}");
        assert!(run > scattered, "{run:?} > {scattered:?}");
        assert_eq!(score("Stop"), None);
        assert_eq!(
            query.score("Undo delete").map(|hit| hit.indices),
            Some(vec![5, 6, 7])
        );
        assert_eq!(
            FuzzyQuery::new("").score("anything").map(|hit| hit.score),
            Some(0)
        );
    }

    #[test]
    fn a_shorter_label_wins_a_tie() {
        let query = FuzzyQuery::new("pull");
        let short = query.score("Pull requests").map(|hit| hit.score);
        let long = query
            .score("Pull the latest changes from origin")
            .map(|hit| hit.score);
        assert!(short > long);
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
