use super::*;
use crate::theme::Theme;

// -----------------------------------------------------------------------------------------
// A plain-text tree of the block structure, which is what the snapshot tests compare.
// -----------------------------------------------------------------------------------------

/// Render a document's block structure as an indented tree.
fn outline(document: &MarkdownDocument) -> String {
    let mut out = String::new();
    write_blocks(&mut out, &document.blocks, 0);
    out
}

fn write_blocks(out: &mut String, blocks: &[MarkdownBlock], depth: usize) {
    for block in blocks {
        let pad = "  ".repeat(depth);
        match block {
            MarkdownBlock::Paragraph(inlines) => {
                out.push_str(&format!("{pad}paragraph {}\n", inline_tree(inlines)));
            }
            MarkdownBlock::Heading { level, inlines } => {
                out.push_str(&format!("{pad}heading{level} {}\n", inline_tree(inlines)));
            }
            MarkdownBlock::Code { lang, text, .. } => {
                let lang = lang.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "{pad}code[{lang}] {:?}\n",
                    text.lines().collect::<Vec<_>>().join("\\n")
                ));
            }
            MarkdownBlock::List { ordered, items } => {
                let kind = if *ordered { "ordered" } else { "bullet" };
                out.push_str(&format!("{pad}list.{kind}\n"));
                for item in items {
                    out.push_str(&format!("{pad}  item\n"));
                    write_blocks(out, item, depth + 2);
                }
            }
            MarkdownBlock::Quote(inner) => {
                out.push_str(&format!("{pad}quote\n"));
                write_blocks(out, inner, depth + 1);
            }
            MarkdownBlock::Rule => out.push_str(&format!("{pad}rule\n")),
        }
    }
}

fn inline_tree(inlines: &[MarkdownInline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            MarkdownInline::Text(text) => format!("{text:?}"),
            MarkdownInline::Code(text) => format!("code({text:?})"),
            MarkdownInline::Strong(inner) => format!("strong[{}]", inline_tree(inner)),
            MarkdownInline::Emphasis(inner) => format!("em[{}]", inline_tree(inner)),
            MarkdownInline::Link { label, url } => {
                format!("link({url:?})[{}]", inline_tree(label))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn text(value: &str) -> MarkdownInline {
    MarkdownInline::Text(value.to_owned())
}

/// A fixture that exercises every construct the subset supports.
const FIXTURE: &str = "\
# Rounding fix

The helper in `src/money.rs` rounded **half up**, which is *wrong* for
negative amounts. See [the issue](https://example.com/issues/12) and the
note at <https://example.com/adr>.

## What changed

1. `round_half_even` replaces the old helper.
2. Callers keep their signatures.

- covered by a table test
- covered by a property test
  - including negative zero

> Rounding is not a formatting concern.
> It is an arithmetic one.

```rust
// half to even
fn round(value: f64) -> i64 {
    let scaled = value * 100.0;
    scaled.round_ties_even() as i64
}
```

---

Done.\
";

// -----------------------------------------------------------------------------------------
// Blocks
// -----------------------------------------------------------------------------------------

#[test]
fn an_empty_source_has_no_blocks() {
    assert_eq!(parse_markdown("").blocks, Vec::new());
    assert_eq!(parse_markdown("   \n\n \t \n").blocks, Vec::new());
}

#[test]
fn paragraphs_join_soft_breaks_with_a_space() {
    let document = parse_markdown("one\ntwo\n\nthree");
    assert_eq!(
        document.blocks,
        vec![
            MarkdownBlock::Paragraph(vec![text("one two")]),
            MarkdownBlock::Paragraph(vec![text("three")]),
        ]
    );
}

#[test]
fn two_trailing_spaces_and_a_trailing_backslash_are_hard_breaks() {
    assert_eq!(
        parse_markdown("one  \ntwo").blocks,
        vec![MarkdownBlock::Paragraph(vec![text("one\ntwo")])]
    );
    assert_eq!(
        parse_markdown("one\\\ntwo").blocks,
        vec![MarkdownBlock::Paragraph(vec![text("one\ntwo")])]
    );
    // An escaped backslash is a backslash, not a break.
    assert_eq!(
        parse_markdown("one\\\\\ntwo").blocks,
        vec![MarkdownBlock::Paragraph(vec![text("one\\ two")])]
    );
}

#[test]
fn atx_headings_carry_their_level_and_drop_a_closing_sequence() {
    assert_eq!(
        parse_markdown("### Deep").blocks,
        vec![MarkdownBlock::Heading {
            level: 3,
            inlines: vec![text("Deep")],
        }]
    );
    assert_eq!(
        parse_markdown("## Closed ##").blocks,
        vec![MarkdownBlock::Heading {
            level: 2,
            inlines: vec![text("Closed")],
        }]
    );
    // A hash glued to the word is part of the word.
    assert_eq!(
        parse_markdown("# c#").blocks,
        vec![MarkdownBlock::Heading {
            level: 1,
            inlines: vec![text("c#")],
        }]
    );
    // Seven hashes is not a heading.
    assert!(matches!(
        parse_markdown("####### too deep").blocks[0],
        MarkdownBlock::Paragraph(_)
    ));
    // Nor is a hash without a space.
    assert!(matches!(
        parse_markdown("#tag").blocks[0],
        MarkdownBlock::Paragraph(_)
    ));
}

#[test]
fn fenced_code_keeps_its_language_and_its_literal_body() {
    let document = parse_markdown("```rust ignore\nlet x = 1;\n\n  indented\n```\nafter");
    assert_eq!(
        document.blocks,
        vec![
            MarkdownBlock::code(Some("rust".to_owned()), "let x = 1;\n\n  indented"),
            MarkdownBlock::Paragraph(vec![text("after")]),
        ]
    );
}

#[test]
fn a_tilde_fence_may_contain_backticks() {
    assert_eq!(
        parse_markdown("~~~\n```\n~~~").blocks,
        vec![MarkdownBlock::code(None, "```")]
    );
}

#[test]
fn thematic_breaks_win_over_list_markers() {
    for source in ["---", "***", "___", "- - -", "  ***  "] {
        assert_eq!(
            parse_markdown(source).blocks,
            vec![MarkdownBlock::Rule],
            "{source:?}"
        );
    }
}

#[test]
fn lists_group_by_kind_and_nest_one_level() {
    let document = parse_markdown("- one\n- two\n  - deep\n\n1. first\n2. second");
    assert_eq!(
        outline(&document),
        concat!(
            "list.bullet\n",
            "  item\n",
            "    paragraph \"one\"\n",
            "  item\n",
            "    paragraph \"two\"\n",
            "    list.bullet\n",
            "      item\n",
            "        paragraph \"deep\"\n",
            "list.ordered\n",
            "  item\n",
            "    paragraph \"first\"\n",
            "  item\n",
            "    paragraph \"second\"\n",
        )
    );
}

#[test]
fn a_list_item_keeps_indented_continuation_across_a_blank_line() {
    let document = parse_markdown("- one\n\n  still one\n\nafter");
    assert_eq!(
        outline(&document),
        concat!(
            "list.bullet\n",
            "  item\n",
            "    paragraph \"one\"\n",
            "    paragraph \"still one\"\n",
            "paragraph \"after\"\n",
        )
    );
}

#[test]
fn ordered_and_bullet_lists_do_not_merge() {
    let document = parse_markdown("- one\n1. two");
    assert_eq!(
        outline(&document),
        concat!(
            "list.bullet\n",
            "  item\n",
            "    paragraph \"one\"\n",
            "list.ordered\n",
            "  item\n",
            "    paragraph \"two\"\n",
        )
    );
}

#[test]
fn quotes_strip_their_marker_and_accept_lazy_continuation() {
    let document = parse_markdown("> one\ntwo\n\n> - listed");
    assert_eq!(
        outline(&document),
        concat!(
            "quote\n",
            "  paragraph \"one two\"\n",
            "quote\n",
            "  list.bullet\n",
            "    item\n",
            "      paragraph \"listed\"\n",
        )
    );
}

#[test]
fn tables_and_images_survive_as_their_own_source() {
    let document = parse_markdown("| a | b |\n| - | - |\n\n![alt](img.png)");
    assert_eq!(
        document.blocks,
        vec![
            MarkdownBlock::Paragraph(vec![text("| a | b | | - | - |")]),
            MarkdownBlock::Paragraph(vec![text("![alt](img.png)")]),
        ]
    );
}

// -----------------------------------------------------------------------------------------
// Inlines
// -----------------------------------------------------------------------------------------

fn inlines(source: &str) -> Vec<MarkdownInline> {
    match parse_markdown(source).blocks.into_iter().next() {
        Some(MarkdownBlock::Paragraph(inlines)) => inlines,
        other => panic!("expected one paragraph, got {other:?}"),
    }
}

#[test]
fn inline_code_takes_the_longest_matching_backtick_run() {
    assert_eq!(
        inlines("a `b` c"),
        vec![text("a "), MarkdownInline::Code("b".to_owned()), text(" c")]
    );
    assert_eq!(
        inlines("``a ` b``"),
        vec![MarkdownInline::Code("a ` b".to_owned())]
    );
    // One space at each end is the escape hatch for a literal backtick, and it is stripped.
    assert_eq!(
        inlines("`` ` ``"),
        vec![MarkdownInline::Code("`".to_owned())]
    );
}

#[test]
fn strong_and_emphasis_nest_by_run_length() {
    assert_eq!(
        inlines("*a*"),
        vec![MarkdownInline::Emphasis(vec![text("a")])]
    );
    assert_eq!(
        inlines("**a**"),
        vec![MarkdownInline::Strong(vec![text("a")])]
    );
    assert_eq!(
        inlines("***a***"),
        vec![MarkdownInline::Emphasis(vec![MarkdownInline::Strong(
            vec![text("a")]
        )])]
    );
    // Leftover opener delimiters stay literal, exactly as CommonMark renders them.
    assert_eq!(
        inlines("**a*"),
        vec![text("*"), MarkdownInline::Emphasis(vec![text("a")])]
    );
}

#[test]
fn intra_word_underscores_are_identifiers_not_emphasis() {
    assert_eq!(inlines("snake_case_name"), vec![text("snake_case_name")]);
    assert_eq!(
        inlines("_emphasised_"),
        vec![MarkdownInline::Emphasis(vec![text("emphasised")])]
    );
}

#[test]
fn links_carry_a_parsed_label_and_a_bare_destination() {
    assert_eq!(
        inlines("[a `b`](https://example.com \"title\")"),
        vec![MarkdownInline::Link {
            label: vec![text("a "), MarkdownInline::Code("b".to_owned())],
            url: "https://example.com".to_owned(),
        }]
    );
    assert_eq!(
        inlines("<https://example.com/x>"),
        vec![MarkdownInline::Link {
            label: vec![text("https://example.com/x")],
            url: "https://example.com/x".to_owned(),
        }]
    );
    assert_eq!(
        inlines("[wrapped](<a b>)"),
        vec![MarkdownInline::Link {
            label: vec![text("wrapped")],
            url: "a b".to_owned(),
        }]
    );
}

#[test]
fn escapes_produce_literal_punctuation() {
    assert_eq!(inlines(r"\*not emphasis\*"), vec![text("*not emphasis*")]);
    assert_eq!(inlines(r"a \` b"), vec![text("a ` b")]);
}

#[test]
fn a_lone_angle_bracket_is_text() {
    assert_eq!(inlines("a < b"), vec![text("a < b")]);
    assert_eq!(inlines("<not a link>"), vec![text("<not a link>")]);
}

// -----------------------------------------------------------------------------------------
// Streaming safety
// -----------------------------------------------------------------------------------------

#[test]
fn an_unterminated_fence_is_still_a_code_block() {
    assert_eq!(
        parse_markdown("```py\nprint(1)\n").blocks,
        vec![MarkdownBlock::code(Some("py".to_owned()), "print(1)\n")]
    );
}

#[test]
fn half_written_inline_markup_stays_literal() {
    assert_eq!(inlines("a `bc"), vec![text("a `bc")]);
    assert_eq!(inlines("a **bo"), vec![text("a **bo")]);
    assert_eq!(inlines("see [the iss"), vec![text("see [the iss")]);
    assert_eq!(inlines("see [label]"), vec![text("see [label]")]);
    assert_eq!(inlines("see [label](htt"), vec![text("see [label](htt")]);
}

#[test]
fn completed_blocks_are_stable_as_the_text_grows() {
    let full = parse_markdown(FIXTURE);
    for (end, _) in FIXTURE.char_indices() {
        let prefix = parse_markdown(&FIXTURE[..end]);
        let decided = prefix.blocks.len().saturating_sub(1);
        assert!(
            decided <= full.blocks.len(),
            "prefix of {end} bytes produced more blocks than the whole fixture"
        );
        assert_eq!(
            prefix.blocks[..decided],
            full.blocks[..decided],
            "block structure moved at a {end}-byte prefix"
        );
    }
}

/// A deterministic 64-bit xorshift, so a failure reproduces from the seed alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

/// Cut a fixture into random slices and splice random fragments together. The assertion is the
/// absence of a panic: `parse_markdown` has no error path, so any input that reaches it must
/// produce a document.
#[test]
fn random_slices_and_splices_never_panic() {
    let boundaries: Vec<usize> = FIXTURE
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(FIXTURE.len()))
        .collect();
    let mut rng = Rng(0x5DEE_CE66_D1CE_D00D);
    for _ in 0..4_000 {
        let a = boundaries[rng.below(boundaries.len())];
        let b = boundaries[rng.below(boundaries.len())];
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let slice = &FIXTURE[start..end];
        let document = parse_markdown(slice);
        assert!(document.blocks.len() <= slice.len() + 1);

        let c = boundaries[rng.below(boundaries.len())];
        let d = boundaries[rng.below(boundaries.len())];
        let (other_start, other_end) = if c <= d { (c, d) } else { (d, c) };
        let spliced = format!("{slice}{}", &FIXTURE[other_start..other_end]);
        let _ = parse_markdown(&spliced);
    }
}

#[test]
fn adversarial_shapes_terminate() {
    let cases = [
        "#".repeat(4_000),
        ">".repeat(4_000),
        "> ".repeat(2_000),
        "- ".repeat(2_000),
        "*".repeat(4_000),
        "`".repeat(4_000),
        "[".repeat(4_000),
        "](".repeat(2_000),
        "```".repeat(1_000),
        "\\".repeat(4_000),
        "\n".repeat(4_000),
        "\u{1F600}".repeat(1_000),
        format!("{}text{}", "_".repeat(500), "_".repeat(500)),
    ];
    for case in &cases {
        let _ = parse_markdown(case);
    }
}

#[test]
fn nesting_stops_instead_of_recursing_without_bound() {
    let deep = format!("{} bottom", "> ".repeat(64));
    let document = parse_markdown(&deep);
    let mut depth = 0;
    let mut blocks = &document.blocks;
    while let Some(MarkdownBlock::Quote(inner)) = blocks.first() {
        depth += 1;
        blocks = inner;
    }
    assert!(depth <= 8, "quote nesting reached {depth}");
    assert!(!blocks.is_empty(), "the bottom of the nest kept its text");
}

// -----------------------------------------------------------------------------------------
// Snapshot
// -----------------------------------------------------------------------------------------

#[test]
fn the_fixture_renders_one_stable_block_tree() {
    assert_eq!(
        outline(&parse_markdown(FIXTURE)),
        concat!(
            "heading1 \"Rounding fix\"\n",
            "paragraph \"The helper in \" code(\"src/money.rs\") \" rounded \" ",
            "strong[\"half up\"] \", which is \" em[\"wrong\"] \" for negative amounts. See \" ",
            "link(\"https://example.com/issues/12\")[\"the issue\"] \" and the note at \" ",
            "link(\"https://example.com/adr\")[\"https://example.com/adr\"] \".\"\n",
            "heading2 \"What changed\"\n",
            "list.ordered\n",
            "  item\n",
            "    paragraph code(\"round_half_even\") \" replaces the old helper.\"\n",
            "  item\n",
            "    paragraph \"Callers keep their signatures.\"\n",
            "list.bullet\n",
            "  item\n",
            "    paragraph \"covered by a table test\"\n",
            "  item\n",
            "    paragraph \"covered by a property test\"\n",
            "    list.bullet\n",
            "      item\n",
            "        paragraph \"including negative zero\"\n",
            "quote\n",
            "  paragraph \"Rounding is not a formatting concern. It is an arithmetic one.\"\n",
            "code[rust] \"// half to even\\\\nfn round(value: f64) -> i64 {\\\\n",
            "    let scaled = value * 100.0;\\\\n    scaled.round_ties_even() as i64\\\\n}\"\n",
            "rule\n",
            "paragraph \"Done.\"\n",
        )
    );
}

// -----------------------------------------------------------------------------------------
// Code colouring
// -----------------------------------------------------------------------------------------

#[test]
fn the_keyword_table_is_sorted_for_its_binary_search() {
    assert!(
        code::KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]),
        "KEYWORDS must stay sorted and free of duplicates"
    );
}

#[test]
fn an_unknown_language_is_left_plain() {
    assert!(code::highlight(None, "fn main() {}").is_empty());
    assert!(code::highlight(Some("brainfuck"), "fn main() {}").is_empty());
}

#[test]
fn the_scanner_colours_comments_strings_numbers_and_keywords() {
    let source = "let x = 12; // note \"quoted\"";
    let spans = code::highlight(Some("rust"), source);
    let named: Vec<(&str, code::CodeToken)> = spans
        .iter()
        .map(|(range, token)| (&source[range.clone()], *token))
        .collect();
    assert_eq!(
        named,
        vec![
            ("let", code::CodeToken::Keyword),
            ("12", code::CodeToken::Number),
            ("// note \"quoted\"", code::CodeToken::Comment),
        ]
    );
}

#[test]
fn a_number_inside_an_identifier_is_not_a_number() {
    let source = "utf8 = 8";
    let spans = code::highlight(Some("python"), source);
    assert_eq!(
        spans
            .iter()
            .map(|(range, _)| &source[range.clone()])
            .collect::<Vec<_>>(),
        vec!["8"]
    );
}

#[test]
fn an_unterminated_string_stops_at_the_line_break() {
    let source = "s = \"open\nnext = 1";
    let spans = code::highlight(Some("python"), source);
    assert_eq!(&source[spans[0].0.clone()], "\"open");
    assert_eq!(spans[0].1, code::CodeToken::Literal);
}

#[test]
fn every_span_is_sorted_disjoint_and_on_a_char_boundary() {
    let source = "// ✓ done\nlet s = \"héllo\"; /* 42 */\nconst π = 3.14;";
    let spans = code::highlight(Some("rust"), source);
    let mut end = 0;
    for (range, _) in &spans {
        assert!(
            range.start >= end,
            "spans overlap or are unsorted: {spans:?}"
        );
        assert!(range.start < range.end, "empty span: {range:?}");
        assert!(source.is_char_boundary(range.start));
        assert!(source.is_char_boundary(range.end));
        end = range.end;
    }
    assert!(end <= source.len());
}

#[test]
fn the_scanner_never_runs_off_the_end() {
    for lang in ["rust", "python", "sql"] {
        for source in ["\"", "'", "`", "/*", "//", "\\", "1e", "\"\\", "#"] {
            let spans = code::highlight(Some(lang), source);
            assert!(spans.iter().all(|(range, _)| range.end <= source.len()));
        }
    }
}

// -----------------------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------------------

#[test]
fn a_paragraph_becomes_one_chunk_per_word_plus_one_per_hard_break() {
    let theme = Theme::dark();
    // Three words, one chip, one hard break, then two words.
    let nodes = inlines("alpha beta gamma `chip`  \ndelta epsilon");
    assert_eq!(render::inline_chunk_count(&nodes, &theme), 7);
}

#[test]
fn the_inline_code_fill_is_a_translucent_neutral() {
    let theme = Theme::dark();
    let fill = render::code_fill(&theme);
    assert!(fill.a < 0.1, "the chip must not read as a solid block");
    assert_eq!(fill.h, theme.colors.text_secondary.h);
    assert_eq!(fill.s, theme.colors.text_secondary.s);
    assert_eq!(fill.l, theme.colors.text_secondary.l);
}

#[test]
fn flattening_drops_marks_and_keeps_text() {
    let nodes = inlines("a **b** [c](https://example.com) `d`");
    assert_eq!(render::flatten(&nodes), "a b c d");
}

/// A fence is lexed when its document is built, not when the document is drawn: the transcript
/// redraws a streaming turn every frame, and re-scanning a 300-line block each time is the
/// render-path work `gpui-performance` rule 1 forbids.
#[gpui::test]
fn a_code_block_is_highlighted_once_per_document(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    code::HIGHLIGHT_CALLS.with(|calls| calls.set(0));

    let document = parse_markdown("```rust\nlet x = 1; // one\n```");
    assert_eq!(
        code::HIGHLIGHT_CALLS.with(std::cell::Cell::get),
        1,
        "the fence must be lexed once while the document is parsed"
    );

    cx.update(|cx| {
        for _ in 0..3 {
            let _ = render::render(&document, cx);
        }
    });
    assert_eq!(
        code::HIGHLIGHT_CALLS.with(std::cell::Cell::get),
        1,
        "drawing the document re-lexed the fence"
    );
}

#[gpui::test]
fn every_construct_renders_in_both_themes(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    for theme in [Theme::dark(), Theme::light()] {
        cx.update(|cx| cx.set_global(theme));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| MarkdownHarness {
                    document: parse_markdown(FIXTURE),
                })
            })
            .unwrap_or_else(|error| panic!("test window: {error}"))
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        window
            .root(&mut visual)
            .unwrap_or_else(|error| panic!("test root: {error}"));
    }
}

struct MarkdownHarness {
    document: MarkdownDocument,
}

impl gpui::Render for MarkdownHarness {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::div().size_full().child(markdown(&self.document, cx))
    }
}
