//! Word-level highlighting inside a hunk: which removed line pairs with which added line, and
//! which words inside that pair actually changed.
//!
//! Neither git nor `fleet-git` tells us either thing, so both are computed here with `similar`.
//! The algorithm is GitHub/Zed-shaped with Pierre's refinement, in four steps:
//!
//! 1. **Segment the hunk into change blocks.** A maximal run of removed lines followed
//!    immediately by a maximal run of added lines is one block — Pierre's `ChangeContent`.
//!    Context and `\ No newline` terminate a block.
//! 2. **Pair by index inside the block.** `removed[i]` against `added[i]`. Leftovers on either
//!    side are pure insertions or deletions and get no word highlighting.
//! 3. **Gate on similarity.** Below [`MIN_RATIO`] the two lines are different code rather than
//!    an edit of the same line, and per-word marks read as confetti.
//! 4. **Coalesce.** Two emphasised regions separated by a *single* character are joined —
//!    Pierre's `'word-alt'` rule, without which `foo.bar()` → `foo.baz()` marks `bar` and `)`
//!    as two spans with a lone `(` between them.
//!
//! Everything here is a pure function over `&str` / `&Hunk`, so it unit-tests without opening a
//! window.

use std::ops::Range;
use std::time::Duration;

use fleet_git::{Hunk, LineKind};
use similar::{Algorithm, ChangeTag, TextDiff};

/// How many lines a block may hold and still be paired by index. Zed's cap
/// (`buffer_diff.rs:20`): past a handful of lines, index pairing stops being *correct* often
/// enough to be worth drawing.
pub const MAX_BLOCK_LINES: usize = 5;

/// How long a line may be and still be word-diffed. Zed's `MAX_WORD_DIFF_LEN`.
pub const MAX_LINE_LEN: usize = 512;

/// How similar two lines must be before their words are compared at all.
pub const MIN_RATIO: f32 = 0.5;

/// The wall-clock budget handed to `similar`.
///
/// `similar`'s own inline helpers carry a hardcoded 500 ms deadline, which in a UI is a
/// thirty-frame stall, so every diff started here is configured with an explicit
/// [`similar::TextDiffConfig::timeout`] instead. Past it the algorithm returns a coarser but
/// still correct answer.
pub const DEADLINE: Duration = Duration::from_millis(10);

/// How many tokens one side may have before word diffing is skipped. A minified line reaches
/// this long before it reaches [`MAX_LINE_LEN`].
pub const MAX_TOKENS: usize = 400;

/// One removed run paired with the added run that immediately follows it.
///
/// Indices are into [`Hunk::lines`], so they are the same coordinates the staging selection
/// uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeBlock {
    /// The removed lines, in order.
    pub removed: Vec<usize>,
    /// The added lines, in order.
    pub added: Vec<usize>,
}

impl ChangeBlock {
    /// The index pairs this block contributes: `removed[i]` with `added[i]`.
    #[must_use]
    pub fn pairs(&self) -> Vec<(usize, usize)> {
        self.removed
            .iter()
            .copied()
            .zip(self.added.iter().copied())
            .collect()
    }

    /// Whether the block is small enough for index pairing to be trustworthy.
    #[must_use]
    pub fn pairable(&self) -> bool {
        self.removed.len().max(self.added.len()) <= MAX_BLOCK_LINES
    }
}

/// Groups a hunk's lines into removed/added blocks.
#[must_use]
pub fn change_blocks(hunk: &Hunk) -> Vec<ChangeBlock> {
    let mut blocks: Vec<ChangeBlock> = Vec::new();
    let mut current = ChangeBlock::default();
    for (index, line) in hunk.lines.iter().enumerate() {
        match line.kind {
            LineKind::Removed => {
                // A removal after an addition starts a new block: the addition run has closed.
                if !current.added.is_empty() {
                    blocks.push(std::mem::take(&mut current));
                }
                current.removed.push(index);
            }
            LineKind::Added => current.added.push(index),
            LineKind::Context | LineKind::NoNewline | LineKind::Other => {
                if !current.removed.is_empty() || !current.added.is_empty() {
                    blocks.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.removed.is_empty() || !current.added.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// The changed-word spans of a paired line: byte ranges into the removed line, then into the
/// added one.
pub type WordSpans = (Vec<Range<usize>>, Vec<Range<usize>>);

/// The changed words of one paired removed/added line, as byte ranges into each line's text.
///
/// `None` when either line is too long, when the pair is too dissimilar for word marks to mean
/// anything, or when every word changed — the row tint already says "this whole line differs",
/// and marking it twice is noise.
#[must_use]
pub fn word_spans(old: &str, new: &str) -> Option<WordSpans> {
    if old == new || old.len() > MAX_LINE_LEN || new.len() > MAX_LINE_LEN {
        return None;
    }
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    if old_tokens.len() > MAX_TOKENS || new_tokens.len() > MAX_TOKENS {
        return None;
    }
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DEADLINE)
        .diff_slices(&old_tokens, &new_tokens);
    if diff.ratio() < MIN_RATIO {
        return None;
    }
    let mut removed: Vec<Range<usize>> = Vec::new();
    let mut added: Vec<Range<usize>> = Vec::new();
    let mut old_at = 0usize;
    let mut new_at = 0usize;
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                old_at += len;
                new_at += len;
            }
            ChangeTag::Delete => {
                push(&mut removed, old_at..old_at + len);
                old_at += len;
            }
            ChangeTag::Insert => {
                push(&mut added, new_at..new_at + len);
                new_at += len;
            }
        }
    }
    let removed = join_single_char(old, removed);
    let added = join_single_char(new, added);
    if removed.is_empty() && added.is_empty() {
        return None;
    }
    if covers_everything(old, &removed) && covers_everything(new, &added) {
        return None;
    }
    Some((removed, added))
}

/// Splits a line the way Zed's `CharClassifier` does: a token boundary at every character-class
/// transition, and each punctuation character its own token.
///
/// Whitespace-only `similar::TextDiff::from_words` would treat `foo.bar(x)` as a single token
/// and mark the whole call changed; unicode word segmentation folds `foo.bar` into one word for
/// the same reason it folds `e.g.`. Neither is right for code.
#[must_use]
pub fn tokenize(text: &str) -> Vec<&str> {
    #[derive(PartialEq, Eq, Clone, Copy)]
    enum Kind {
        Word,
        Space,
    }
    fn kind(character: char) -> Option<Kind> {
        if character.is_alphanumeric() || character == '_' || character == '$' {
            Some(Kind::Word)
        } else if character.is_whitespace() {
            Some(Kind::Space)
        } else {
            // Punctuation: never merged, so `);` is two tokens.
            None
        }
    }
    let mut tokens = Vec::new();
    let mut run: Option<(Kind, usize)> = None;
    for (at, character) in text.char_indices() {
        match (kind(character), run) {
            (Some(this), Some((previous, _))) if this == previous => {}
            (Some(this), Some((_, start))) => {
                tokens.push(&text[start..at]);
                run = Some((this, at));
            }
            (Some(this), None) => run = Some((this, at)),
            (None, Some((_, start))) => {
                tokens.push(&text[start..at]);
                run = None;
                tokens.push(&text[at..at + character.len_utf8()]);
            }
            (None, None) => tokens.push(&text[at..at + character.len_utf8()]),
        }
    }
    if let Some((_, start)) = run {
        tokens.push(&text[start..]);
    }
    tokens
}

/// Appends one span, merging it into the previous one when they touch.
fn push(spans: &mut Vec<Range<usize>>, range: Range<usize>) {
    match spans.last_mut() {
        Some(previous) if previous.end == range.start => previous.end = range.end,
        _ => spans.push(range),
    }
}

/// Pierre's `'word-alt'` rule: join two emphasised regions separated by a single character.
fn join_single_char(text: &str, spans: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        match out.last_mut() {
            Some(previous) if previous.end <= span.start => {
                let gap = text.get(previous.end..span.start).unwrap_or_default();
                if gap.chars().count() <= 1 {
                    previous.end = span.end;
                    continue;
                }
                out.push(span);
            }
            Some(previous) => previous.end = previous.end.max(span.end),
            None => out.push(span),
        }
    }
    out
}

/// Whether the spans mark everything but leading and trailing whitespace.
fn covers_everything(text: &str, spans: &[Range<usize>]) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let start = text.len() - text.trim_start().len();
    let end = start + trimmed.len();
    spans
        .first()
        .zip(spans.last())
        .is_some_and(|(first, last)| first.start <= start && last.end >= end && spans.len() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_git::parse;

    fn hunk_of(patch: &str) -> Hunk {
        let diff = parse::diff::parse(patch.as_bytes()).expect("parses");
        diff.files
            .into_iter()
            .next()
            .expect("one file")
            .hunks
            .into_iter()
            .next()
            .expect("one hunk")
    }

    #[test]
    fn a_removed_run_pairs_with_the_added_run_after_it() {
        let hunk = hunk_of(concat!(
            "diff --git a/a b/a\n",
            "--- a/a\n",
            "+++ b/a\n",
            "@@ -1,6 +1,6 @@\n",
            " keep\n",
            "-one\n",
            "-two\n",
            "+ONE\n",
            "+TWO\n",
            " tail\n",
            "-solo\n",
        ));
        let blocks = change_blocks(&hunk);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].removed, vec![1, 2]);
        assert_eq!(blocks[0].added, vec![3, 4]);
        assert_eq!(blocks[0].pairs(), vec![(1, 3), (2, 4)]);
        assert_eq!(blocks[1].removed, vec![6]);
        assert!(blocks[1].added.is_empty());
        assert!(blocks[1].pairs().is_empty());
        assert!(blocks[0].pairable());
    }

    #[test]
    fn an_addition_followed_by_a_removal_starts_a_new_block() {
        let hunk = hunk_of(concat!(
            "diff --git a/a b/a\n",
            "--- a/a\n",
            "+++ b/a\n",
            "@@ -1,3 +1,3 @@\n",
            "-one\n",
            "+ONE\n",
            "-two\n",
            "+TWO\n",
        ));
        let blocks = change_blocks(&hunk);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].pairs(), vec![(0, 1)]);
        assert_eq!(blocks[1].pairs(), vec![(2, 3)]);
    }

    #[test]
    fn a_tall_block_is_not_pairable() {
        let block = ChangeBlock {
            removed: (0..MAX_BLOCK_LINES + 1).collect(),
            added: Vec::new(),
        };
        assert!(!block.pairable());
    }

    #[test]
    fn word_spans_mark_only_the_changed_word() {
        let (removed, added) =
            word_spans("const total = price * 2;", "const total = price * 3;").expect("spans");
        assert_eq!(removed.len(), 1);
        assert_eq!(added.len(), 1);
        assert_eq!(&"const total = price * 2;"[removed[0].clone()], "2");
        assert_eq!(&"const total = price * 3;"[added[0].clone()], "3");
    }

    #[test]
    fn a_single_separator_between_two_marks_is_joined() {
        let old = "foo.bar(x)";
        let new = "foo.baz(y)";
        let (removed, added) = word_spans(old, new).expect("spans");
        // Without the join rule this is two spans with a lone `(` between them.
        assert_eq!(removed.len(), 1, "{removed:?}");
        assert_eq!(added.len(), 1, "{added:?}");
        assert_eq!(&old[removed[0].clone()], "bar(x");
        assert_eq!(&new[added[0].clone()], "baz(y");
    }

    #[test]
    fn wildly_different_lines_get_no_marks() {
        assert_eq!(
            word_spans(
                "let x = 1;",
                "impl std::fmt::Display for VeryDifferentThing {}"
            ),
            None
        );
    }

    #[test]
    fn identical_and_overlong_lines_are_skipped() {
        assert_eq!(word_spans("same", "same"), None);
        let long = "x".repeat(MAX_LINE_LEN + 1);
        assert_eq!(word_spans(&long, "short"), None);
    }

    #[test]
    fn tokenizes_code_at_every_class_transition() {
        assert_eq!(
            tokenize("foo.bar(x, 1)"),
            vec!["foo", ".", "bar", "(", "x", ",", " ", "1", ")"]
        );
        assert_eq!(tokenize(""), Vec::<&str>::new());
        assert_eq!(tokenize("  a"), vec!["  ", "a"]);
        assert_eq!(tokenize("héllo→x"), vec!["héllo", "→", "x"]);
    }

    #[test]
    fn spans_stay_inside_the_line() {
        let old = "  indented(1)";
        let new = "  indented(2)";
        let (removed, added) = word_spans(old, new).expect("spans");
        assert!(removed.iter().all(|span| span.end <= old.len()));
        assert!(added.iter().all(|span| span.end <= new.len()));
    }
}
