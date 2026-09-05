//! Syntax highlighting for diff payload lines, on TextMate grammars.
//!
//! `syntect` 5.3 with `two-face`'s curated grammar set (213 syntaxes, TypeScript and TSX
//! included) rather than tree-sitter, for one decisive reason: syntect is the only engine of the
//! two with **resumable per-line state**. `ParseState` and `HighlightState` both `Clone`, so a
//! stream of lines can be parsed one line at a time and stopped anywhere — which is exactly the
//! shape of a diff, where we hold hunks rather than whole files. Pierre (`diffs.com`) makes the
//! same choice with Shiki's TextMate grammars for the same reason.
//!
//! Two deliberate simplifications, both because a diff is not a file:
//!
//! * **Per-hunk, two-stream parsing.** A hunk's `-` and `+` lines interleave two different
//!   versions of the file; feeding them to one parser corrupts its state inside a block comment
//!   or a multi-line string. Each hunk is therefore parsed twice — once over its *old* side
//!   (context + removed) and once over its *new* side (context + added) — with a fresh
//!   `ParseState` per hunk, since the lines above the hunk are not in the patch at all.
//! * **Buckets, not colours.** The highlighter yields a [`Bucket`] per span, not an `Hsla`, so
//!   the work can happen on a background thread with no `Theme` in hand and the colours resolve
//!   against the live theme at paint time. Eight buckets, deliberately: Zed's One Dark spends
//!   eight hues on 46 token categories, and inside a diff the row tint is already carrying
//!   information.

use std::ops::Range;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use fleet_ui_kit::Theme;
use gpui::Hsla;
use syntect::highlighting::{
    Color, Highlighter, RangedHighlightIterator, Style, StyleModifier, Theme as SyntectTheme,
    ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use super::Ansi;

/// Lines longer than this are not highlighted at all — Pierre's `tokenizeMaxLineLength`, and the
/// reason a minified bundle in a diff does not stall the highlighter.
pub const MAX_LINE_LEN: usize = 1_000;

/// How many payload lines one background pass will highlight before giving up. Past this the
/// diff is a bulk import, not something anyone reads a token at a time.
pub const MAX_LINES: usize = 40_000;

/// How long one background pass may run before it stops and leaves the rest plain.
pub const BUDGET: Duration = Duration::from_millis(1_500);

/// The eight colour buckets a scope can land in, plus plain text.
///
/// Ordered so the discriminant can be smuggled through a `syntect` theme colour (see
/// [`bucket_theme`]) and decoded again without a scope-name lookup on the hot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Bucket {
    /// No rule matched: ordinary code.
    #[default]
    Plain = 0,
    /// `comment`.
    Comment = 1,
    /// `string`, character literals.
    Text = 2,
    /// `constant.numeric`, `constant.language`.
    Number = 3,
    /// `keyword`, `storage.modifier`.
    Keyword = 4,
    /// Function names and calls.
    Function = 5,
    /// Types, classes, enums, interfaces.
    Type = 6,
    /// Parameters, members, JSX/HTML tags and attributes.
    Member = 7,
    /// Punctuation and operators.
    Punctuation = 8,
}

impl Bucket {
    /// Decode a bucket smuggled through a `syntect` colour's red channel.
    fn from_color(color: Color) -> Self {
        match color.r {
            1 => Bucket::Comment,
            2 => Bucket::Text,
            3 => Bucket::Number,
            4 => Bucket::Keyword,
            5 => Bucket::Function,
            6 => Bucket::Type,
            7 => Bucket::Member,
            8 => Bucket::Punctuation,
            _ => Bucket::Plain,
        }
    }

    /// The resolved colour for the installed theme.
    ///
    /// Keyed off the terminal palette, exactly as the rest of this module's colours are
    /// (`views/mod.rs`), so a theme change moves the syntax palette with it and no new
    /// `ColorTokens` field is needed.
    #[must_use]
    pub fn color(self, theme: &Theme) -> Hsla {
        match self {
            Bucket::Plain => theme.colors.text,
            Bucket::Comment => theme.colors.text_muted,
            Bucket::Text => Ansi::Green.color(theme),
            Bucket::Number => Ansi::Yellow.color(theme),
            Bucket::Keyword => Ansi::Magenta.color(theme),
            Bucket::Function => Ansi::Blue.color(theme),
            Bucket::Type => Ansi::Cyan.color(theme),
            Bucket::Member => Ansi::Red.color(theme),
            Bucket::Punctuation => theme.colors.text_secondary,
        }
    }
}

/// The scope selectors that feed each bucket. More specific selectors win on `syntect`'s own
/// specificity score, so `keyword.operator.word` reaches [`Bucket::Keyword`] even though
/// `keyword.operator` is listed under [`Bucket::Punctuation`].
///
/// `storage.type` is deliberately **absent** from [`Bucket::Type`]: TextMate grammars scope the
/// declaration keywords that way (`const` in TypeScript, `fn` and `struct` in Rust), so leaving
/// it to `storage` under [`Bucket::Keyword`] is what makes those read as keywords rather than as
/// type names, matching One Dark.
const RULES: [(Bucket, &str); 8] = [
    (Bucket::Comment, "comment, punctuation.definition.comment"),
    (
        Bucket::Text,
        "string, constant.character, text.literal, punctuation.definition.string",
    ),
    (
        Bucket::Number,
        "constant.numeric, constant.language, constant.other",
    ),
    (Bucket::Keyword, "keyword, storage, keyword.operator.word"),
    (
        Bucket::Function,
        "entity.name.function, support.function, variable.function, meta.function-call",
    ),
    (
        Bucket::Type,
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
         entity.name.interface, entity.name.namespace, support.type, support.class",
    ),
    (
        Bucket::Member,
        "variable.parameter, variable.other.member, entity.name.tag, \
         entity.other.attribute-name, support.variable, entity.name.label",
    ),
    (
        Bucket::Punctuation,
        "punctuation, keyword.operator, meta.brace",
    ),
];

/// The grammar set: `syntect`'s 75 defaults plus `two-face`'s curated additions.
///
/// `*_newlines` variants, so every line handed to [`ParseState::parse_line`] must keep its
/// trailing `\n`. Loading measured 1.4 ms, so a `OnceLock` on first use is fine.
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// A `syntect` theme whose only job is to carry a [`Bucket`] discriminant per scope.
///
/// Building a real palette here would mean owning two colour systems; instead every rule paints
/// a sentinel colour whose red channel *is* the bucket, and [`Bucket::from_color`] reads it back.
/// That keeps the background pass free of any `Theme` and lets light/dark resolve at paint time.
fn bucket_theme() -> &'static SyntectTheme {
    static THEME: OnceLock<SyntectTheme> = OnceLock::new();
    THEME.get_or_init(|| {
        let sentinel = |bucket: Bucket| Color {
            r: bucket as u8,
            g: 0,
            b: 0,
            a: 0xFF,
        };
        let scopes = RULES
            .iter()
            .filter_map(|(bucket, selectors)| {
                Some(ThemeItem {
                    scope: selectors.parse().ok()?,
                    style: StyleModifier {
                        foreground: Some(sentinel(*bucket)),
                        background: None,
                        font_style: None,
                    },
                })
            })
            .collect();
        SyntectTheme {
            name: Some("fleet-buckets".to_owned()),
            author: None,
            settings: ThemeSettings {
                foreground: Some(sentinel(Bucket::Plain)),
                background: Some(Color::BLACK),
                ..ThemeSettings::default()
            },
            scopes,
        }
    })
}

/// The grammar name for a path, or `None` when nothing recognises it.
///
/// Extension first, then the whole file name as a token, which is how `Dockerfile` and
/// `Makefile` resolve.
#[must_use]
pub fn language_for_path(path: &str) -> Option<String> {
    let set = syntax_set();
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(extension) = name.rsplit_once('.').map(|(_, extension)| extension)
        && let Some(syntax) = set.find_syntax_by_extension(extension)
    {
        return Some(syntax.name.clone());
    }
    set.find_syntax_by_token(name)
        .map(|syntax| syntax.name.clone())
}

fn syntax_by_name(name: &str) -> Option<&'static SyntaxReference> {
    syntax_set().find_syntax_by_name(name)
}

/// One contiguous stream of lines to highlight: one side of one hunk.
#[derive(Clone, Debug)]
pub struct Job {
    /// The grammar name, from [`language_for_path`].
    pub language: String,
    /// `(row index in the model, the line's text)`, in file order.
    pub lines: Vec<(usize, String)>,
}

/// The highlight runs for one line, as byte ranges into that line's text.
pub type Runs = Vec<(Range<usize>, Bucket)>;

/// Highlights every job, returning `(row index, runs)` pairs.
///
/// Pure and `Send`: this is what runs on the background executor. It stops early once
/// [`BUDGET`] or [`MAX_LINES`] is spent and returns what it has, so a pathological diff
/// degrades to plain text instead of pinning a core.
#[must_use]
pub fn run(jobs: &[Job]) -> Vec<(usize, Runs)> {
    let set = syntax_set();
    let highlighter = Highlighter::new(bucket_theme());
    let started = Instant::now();
    let mut out = Vec::new();
    let mut seen = 0usize;
    for job in jobs {
        let Some(syntax) = syntax_by_name(&job.language) else {
            continue;
        };
        let mut parse = ParseState::new(syntax);
        let mut state = syntect::highlighting::HighlightState::new(&highlighter, ScopeStack::new());
        for (row, text) in &job.lines {
            seen += 1;
            if seen > MAX_LINES || started.elapsed() > BUDGET {
                tracing::warn!(
                    lines = seen,
                    elapsed_ms = started.elapsed().as_millis(),
                    "diff: syntax pass hit its budget; the rest stays plain"
                );
                return out;
            }
            if text.len() > MAX_LINE_LEN {
                continue;
            }
            // `*_newlines` grammars expect the terminator; the range it produces is past the
            // end of the text we render, so it is dropped below.
            let mut line = text.clone();
            line.push('\n');
            let Ok(ops) = parse.parse_line(&line, set) else {
                continue;
            };
            let mut runs: Runs = Vec::new();
            for (style, _, range) in
                RangedHighlightIterator::new(&mut state, &ops, &line, &highlighter)
            {
                let end = range.end.min(text.len());
                if range.start >= end {
                    continue;
                }
                push_run(&mut runs, range.start..end, style);
            }
            if !runs.is_empty() {
                out.push((*row, runs));
            }
        }
    }
    out
}

/// Appends one span, merging it into the previous one when the bucket is unchanged.
///
/// `StyledText::with_default_highlights` walks a monotonic cursor and panics on an unsorted or
/// overlapping range list, so the runs it receives must be sorted, disjoint and coalesced.
fn push_run(runs: &mut Runs, range: Range<usize>, style: Style) {
    let bucket = Bucket::from_color(style.foreground);
    if bucket == Bucket::Plain {
        return;
    }
    match runs.last_mut() {
        Some((previous, last)) if *last == bucket && previous.end == range.start => {
            previous.end = range.end;
        }
        _ => runs.push((range, bucket)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_languages_that_matter() {
        for (path, expected) in [
            ("src/app.ts", "TypeScript"),
            ("src/app.tsx", "TypeScriptReact"),
            ("src/app.js", "JavaScript"),
            ("crates/a/src/lib.rs", "Rust"),
            ("Cargo.toml", "TOML"),
            ("package.json", "JSON"),
            ("README.md", "Markdown"),
            ("main.go", "Go"),
            ("tool.py", "Python"),
            ("style.css", "CSS"),
            ("index.html", "HTML"),
        ] {
            assert_eq!(
                language_for_path(path).as_deref(),
                Some(expected),
                "{path} should resolve to {expected}"
            );
        }
        // Shell and YAML grammar names carry parenthetical detail, so match loosely.
        assert!(
            language_for_path("run.sh").is_some_and(|name| name.to_lowercase().contains("bash")
                || name.to_lowercase().contains("shell"))
        );
        assert!(
            language_for_path("ci.yaml").is_some_and(|name| name.to_lowercase().contains("yaml"))
        );
        assert!(language_for_path("Dockerfile").is_some());
        assert_eq!(language_for_path("no-extension-at-all-xyz"), None);
    }

    #[test]
    fn highlights_typescript_into_buckets() {
        let job = Job {
            language: "TypeScript".to_owned(),
            lines: vec![
                (0, "// a comment".to_owned()),
                (1, "const name: string = \"hello\";".to_owned()),
            ],
        };
        let runs = run(&[job]);
        let comment = runs
            .iter()
            .find(|(row, _)| *row == 0)
            .expect("the comment line has runs");
        assert!(
            comment
                .1
                .iter()
                .all(|(_, bucket)| *bucket == Bucket::Comment)
        );
        let code = runs
            .iter()
            .find(|(row, _)| *row == 1)
            .expect("the const line has runs");
        assert!(code.1.iter().any(|(_, bucket)| *bucket == Bucket::Keyword));
        assert!(code.1.iter().any(|(_, bucket)| *bucket == Bucket::Text));
        // Sorted, disjoint and inside the text: what `with_default_highlights` demands.
        let mut end = 0;
        for (range, _) in &code.1 {
            assert!(range.start >= end, "runs must be sorted and disjoint");
            assert!(range.end <= "const name: string = \"hello\";".len());
            end = range.end;
        }
    }

    #[test]
    fn skips_a_line_past_the_length_cap() {
        let long = "x".repeat(MAX_LINE_LEN + 1);
        let runs = run(&[Job {
            language: "Rust".to_owned(),
            lines: vec![(0, long)],
        }]);
        assert!(runs.is_empty());
    }

    #[test]
    fn an_unknown_grammar_yields_nothing() {
        let runs = run(&[Job {
            language: "Not A Grammar".to_owned(),
            lines: vec![(0, "fn main() {}".to_owned())],
        }]);
        assert!(runs.is_empty());
    }
}
