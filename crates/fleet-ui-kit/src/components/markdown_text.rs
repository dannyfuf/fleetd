//! `MarkdownText` — the read mode of every card description and comment.
//!
//! Two halves, and only the first one is testable:
//!
//! 1. [`parse_markdown`] — a pure `&str -> Vec<MdBlock>` scanner. No crate, no allocator
//!    tricks, no HTML: `docs/BOARD.md` §1 forbids a markdown dependency, and the subset Fleet
//!    renders is small enough that a hand-written line scanner is both shorter and easier to
//!    keep honest than a general parser.
//! 2. [`MarkdownText`] — the `RenderOnce` surface that draws those blocks with the type scale.
//!
//! The subset is exactly §7's: `#` / `##` / `###` headings, paragraphs with blank-line breaks,
//! `-` / `*` / `1.` lists, ``` fenced code, `` `inline code` ``, `**bold**` and bare
//! `http(s)://` URLs. **Everything else is text.** A parser that guesses is worse than one
//! that passes through: a card description is written by a human in a hurry, and an unclosed
//! `**` must render as two asterisks, not swallow the rest of the paragraph.
//!
//! Inline flow is drawn with [`gpui::StyledText`] and byte-range highlights rather than one
//! element per span, so a paragraph wraps like text instead of like a flex row.

use gpui::{App, FontWeight, HighlightStyle, SharedString, StyledText, Window, div, prelude::*};

use crate::{
    text::{Text, TextRole, styled_with},
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

/// The deepest heading level the renderer distinguishes. `####` and beyond are paragraphs.
pub const MAX_HEADING_LEVEL: u8 = 3;

/// One inline run of a paragraph or a list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdSpan {
    /// Plain text.
    Text(String),
    /// `` `code` `` — rendered in the data face on the sunken surface.
    Code(String),
    /// `**bold**`.
    Bold(String),
    /// A bare `http://` or `https://` URL — rendered in the accent color.
    Link(String),
}

impl MdSpan {
    /// The text this span contributes to the rendered line, marks stripped.
    pub fn text(&self) -> &str {
        match self {
            MdSpan::Text(text) | MdSpan::Code(text) | MdSpan::Bold(text) | MdSpan::Link(text) => {
                text
            }
        }
    }
}

/// One block of a parsed document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdBlock {
    /// `#` / `##` / `###`. `level` is 1, 2 or 3.
    Heading {
        /// Heading depth, 1..=[`MAX_HEADING_LEVEL`].
        level: u8,
        /// The heading text, marks stripped.
        text: String,
    },
    /// A run of non-blank lines, joined by spaces.
    Paragraph(Vec<MdSpan>),
    /// A run of `-` / `*` items, or of `1.` items.
    List {
        /// Whether the list was written with numbers.
        ordered: bool,
        /// One entry per item.
        items: Vec<Vec<MdSpan>>,
    },
    /// A ``` fenced block, verbatim, without its fences.
    Code(String),
}

/// Parse the markdown subset Fleet renders. Pure; never panics; never allocates a parser.
///
/// Unknown syntax is not an error: it stays in the text of the block it appeared in.
pub fn parse_markdown(source: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut list: Option<(bool, Vec<String>)> = None;
    let mut lines = source.lines().peekable();

    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
            let mut code: Vec<&str> = Vec::new();
            for raw in lines.by_ref() {
                if raw.trim_start().starts_with("```") {
                    break;
                }
                code.push(raw.trim_end());
            }
            blocks.push(MdBlock::Code(code.join("\n")));
            continue;
        }

        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
            continue;
        }

        if let Some((level, text)) = heading(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            flush_list(&mut list, &mut blocks);
            blocks.push(MdBlock::Heading {
                level,
                text: text.to_string(),
            });
            continue;
        }

        // `line`, not `trimmed`: a nested `  - b` is outside the documented subset, and
        // re-leveling it into the parent list silently rewrites what the author wrote.
        if let Some((ordered, item)) = list_item(line) {
            flush_paragraph(&mut paragraph, &mut blocks);
            match &mut list {
                // A `-` line right under a `1.` line starts a new list: two markers mean two
                // lists, and merging them would silently renumber the author's items.
                Some((open, items)) if *open == ordered => items.push(item.to_string()),
                Some(_) => {
                    flush_list(&mut list, &mut blocks);
                    list = Some((ordered, vec![item.to_string()]));
                }
                None => list = Some((ordered, vec![item.to_string()])),
            }
            continue;
        }

        flush_list(&mut list, &mut blocks);
        paragraph.push(trimmed);
    }

    flush_paragraph(&mut paragraph, &mut blocks);
    flush_list(&mut list, &mut blocks);
    blocks
}

/// Close the open paragraph, if any. Its lines join with a space: a soft line break inside a
/// paragraph is a wrap, not a break — that is what the blank line is for.
fn flush_paragraph(paragraph: &mut Vec<&str>, blocks: &mut Vec<MdBlock>) {
    if paragraph.is_empty() {
        return;
    }
    let text = paragraph.join(" ");
    paragraph.clear();
    blocks.push(MdBlock::Paragraph(parse_inline(&text)));
}

/// Close the open list, if any.
fn flush_list(list: &mut Option<(bool, Vec<String>)>, blocks: &mut Vec<MdBlock>) {
    let Some((ordered, items)) = list.take() else {
        return;
    };
    blocks.push(MdBlock::List {
        ordered,
        items: items.iter().map(|item| parse_inline(item)).collect(),
    });
}

/// `### text` → `(3, "text")`. More than [`MAX_HEADING_LEVEL`] hashes is not a heading.
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > MAX_HEADING_LEVEL as usize {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?.trim();
    if rest.is_empty() {
        return None;
    }
    Some((hashes as u8, rest))
}

/// Keeps ordered markers in item text so rendering preserves the author's numbering.
fn list_item(line: &str) -> Option<(bool, &str)> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some((false, rest.trim()));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    line[digits..].strip_prefix(". ")?;
    // The frozen MdBlock shape has no start-number field: keep the authored marker in its text.
    Some((true, line))
}

fn list_marker(ordered: bool, spans: &mut [MdSpan]) -> String {
    if ordered
        && let Some(MdSpan::Text(text)) = spans.first_mut()
        && let Some((marker, rest)) = text.split_once(' ')
    {
        let marker = marker.to_owned();
        *text = rest.to_owned();
        return marker;
    }
    "·".to_owned()
}

/// Split one line of text into inline spans.
fn parse_inline(text: &str) -> Vec<MdSpan> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;

    while index < text.len() {
        let rest = &text[index..];

        if rest.starts_with('`')
            && let Some(end) = rest[1..].find('`')
        {
            push_plain(&mut plain, &mut spans);
            spans.push(MdSpan::Code(rest[1..1 + end].to_string()));
            index += end + 2;
            continue;
        }

        if rest.starts_with("**")
            && let Some(end) = rest[2..].find("**")
            && end > 0
        {
            push_plain(&mut plain, &mut spans);
            spans.push(MdSpan::Bold(rest[2..2 + end].to_string()));
            index += end + 4;
            continue;
        }

        // A URL only starts at a word boundary, so `shttp://x` stays text.
        let at_boundary =
            index == 0 || bytes[index - 1].is_ascii_whitespace() || bytes[index - 1] == b'(';
        if at_boundary && (rest.starts_with("http://") || rest.starts_with("https://")) {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            if url.len()
                > if rest.starts_with("https://") {
                    "https://".len()
                } else {
                    "http://".len()
                }
            {
                push_plain(&mut plain, &mut spans);
                spans.push(MdSpan::Link(url.to_string()));
                index += url.len();
                continue;
            }
        }

        let Some(character) = rest.chars().next() else {
            break;
        };
        plain.push(character);
        index += character.len_utf8();
    }

    push_plain(&mut plain, &mut spans);
    spans
}

/// Flush the pending plain-text run into `spans`.
fn push_plain(plain: &mut String, spans: &mut Vec<MdSpan>) {
    if !plain.is_empty() {
        spans.push(MdSpan::Text(std::mem::take(plain)));
    }
}

/// A rendered markdown document.
#[derive(IntoElement)]
pub struct MarkdownText {
    source: SharedString,
    muted: bool,
}

impl MarkdownText {
    /// Render `source`.
    pub fn new(source: impl Into<SharedString>) -> Self {
        Self {
            source: source.into(),
            muted: false,
        }
    }

    /// Draw the body text at secondary contrast: a description shown next to something that
    /// matters more, e.g. under the card title in a preview.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// The type role a heading of `level` renders in.
    fn heading_role(level: u8) -> TextRole {
        match level {
            1 => TextRole::Title,
            2 => TextRole::UiStrong,
            _ => TextRole::Label,
        }
    }
}

/// The flat string and the byte-range highlights one line of spans renders as.
struct InlineRuns {
    text: String,
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    mono: Vec<(std::ops::Range<usize>, SharedString)>,
}

/// Flatten spans into one string plus the highlight and font-family runs that style it.
fn inline_runs(spans: &[MdSpan], theme: &Theme) -> InlineRuns {
    let mut out = InlineRuns {
        text: String::new(),
        highlights: Vec::new(),
        mono: Vec::new(),
    };
    for span in spans {
        let start = out.text.len();
        out.text.push_str(span.text());
        let range = start..out.text.len();
        match span {
            MdSpan::Text(_) => {}
            MdSpan::Bold(_) => out.highlights.push((
                range,
                HighlightStyle {
                    color: Some(theme.colors.text),
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            )),
            MdSpan::Code(_) => {
                out.highlights.push((
                    range.clone(),
                    HighlightStyle {
                        color: Some(theme.colors.text),
                        background_color: Some(Tone::Secondary.fill(theme)),
                        ..Default::default()
                    },
                ));
                out.mono.push((range, theme.font_mono.clone()));
            }
            MdSpan::Link(_) => out.highlights.push((
                range,
                HighlightStyle {
                    color: Some(theme.colors.accent),
                    ..Default::default()
                },
            )),
        }
    }
    out
}

/// One wrapped line of inline spans.
fn inline_line(spans: &[MdSpan], theme: &Theme, body: gpui::Hsla) -> gpui::Div {
    let runs = inline_runs(spans, theme);
    let text = StyledText::new(SharedString::from(runs.text))
        .with_highlights(runs.highlights)
        .with_font_family_overrides(runs.mono);
    styled_with(div(), TextRole::Ui.style(theme), theme)
        .w_full()
        .min_w_0()
        .text_color(body)
        .child(text)
}

impl RenderOnce for MarkdownText {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = if self.muted {
            theme.colors.text_secondary
        } else {
            theme.colors.text
        };
        let blocks = parse_markdown(self.source.as_ref());
        let marker_width = crate::theme::ch(3.0);

        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(theme.space.sm)
            .children(blocks.into_iter().map(|block| {
                match block {
                    MdBlock::Heading { level, text } => {
                        let role = MarkdownText::heading_role(level);
                        div()
                            .w_full()
                            .child(Text::new(role, text))
                            .into_any_element()
                    }
                    MdBlock::Paragraph(spans) => {
                        inline_line(&spans, &theme, body).into_any_element()
                    }
                    MdBlock::List { ordered, items } => div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(theme.space.xxs)
                        .children(items.into_iter().map(|mut spans| {
                            let marker = list_marker(ordered, &mut spans);
                            div()
                                .flex()
                                .flex_row()
                                .w_full()
                                .items_start()
                                .gap(theme.space.xs)
                                .child(
                                    div()
                                        .flex_none()
                                        .w(marker_width)
                                        .child(Text::ui(marker).faint()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(inline_line(&spans, &theme, body)),
                                )
                        }))
                        .into_any_element(),
                    // A fenced block is data, so it goes on the sunken ground in the data face,
                    // exactly like a log tail.
                    MdBlock::Code(code) => styled_with(div(), TextRole::Data.style(&theme), &theme)
                        .w_full()
                        .min_w_0()
                        .p(theme.space.sm)
                        .rounded(theme.radii.sm)
                        .bg(theme.colors.bg)
                        .border_1()
                        .border_color(theme.colors.border)
                        .text_color(theme.colors.text)
                        // A long command wraps instead of clipping: a description that hides
                        // half of `fleet board sync --board work` is worse than one that
                        // reflows it.
                        .children(code.split('\n').map(|line| {
                            div()
                                .min_h(TextRole::Data.style(&theme).line_height)
                                .child(line.to_string())
                        }))
                        .into_any_element(),
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> MdSpan {
        MdSpan::Text(value.to_string())
    }

    #[test]
    fn ordered_list_rendering_preserves_authored_numbers() {
        let blocks = parse_markdown("3. a\n7. **b**");
        let MdBlock::List { ordered, mut items } = blocks.into_iter().next().unwrap() else {
            panic!("list");
        };
        assert_eq!(list_marker(ordered, &mut items[0]), "3.");
        assert_eq!(list_marker(ordered, &mut items[1]), "7.");
        assert_eq!(items[0], vec![text("a")]);
    }

    #[test]
    fn an_empty_document_has_no_blocks() {
        assert!(parse_markdown("").is_empty());
        assert!(parse_markdown("\n\n   \n").is_empty());
    }

    #[test]
    fn headings_carry_their_level() {
        assert_eq!(
            parse_markdown("# One\n## Two\n### Three"),
            vec![
                MdBlock::Heading {
                    level: 1,
                    text: "One".into()
                },
                MdBlock::Heading {
                    level: 2,
                    text: "Two".into()
                },
                MdBlock::Heading {
                    level: 3,
                    text: "Three".into()
                },
            ]
        );
    }

    #[test]
    fn a_fourth_level_heading_is_a_paragraph() {
        assert_eq!(
            parse_markdown("#### deep\n#nospace"),
            vec![MdBlock::Paragraph(vec![text("#### deep #nospace")])]
        );
    }

    #[test]
    fn blank_lines_break_paragraphs() {
        assert_eq!(
            parse_markdown("one\ntwo\n\nthree"),
            vec![
                MdBlock::Paragraph(vec![text("one two")]),
                MdBlock::Paragraph(vec![text("three")]),
            ]
        );
    }

    #[test]
    fn unordered_and_ordered_lists_do_not_merge() {
        assert_eq!(
            parse_markdown("- a\n* b\n1. c\n2. d"),
            vec![
                MdBlock::List {
                    ordered: false,
                    items: vec![vec![text("a")], vec![text("b")]],
                },
                MdBlock::List {
                    ordered: true,
                    items: vec![vec![text("1. c")], vec![text("2. d")]],
                },
            ]
        );
    }

    #[test]
    fn a_paragraph_closes_an_open_list() {
        assert_eq!(
            parse_markdown("- a\nprose"),
            vec![
                MdBlock::List {
                    ordered: false,
                    items: vec![vec![text("a")]],
                },
                MdBlock::Paragraph(vec![text("prose")]),
            ]
        );
    }

    #[test]
    fn fenced_code_keeps_its_lines_verbatim() {
        assert_eq!(
            parse_markdown("```sh\nfleet board sync\n  --force\n```\nafter"),
            vec![
                MdBlock::Code("fleet board sync\n  --force".into()),
                MdBlock::Paragraph(vec![text("after")]),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(
            parse_markdown("```\nleft open"),
            vec![MdBlock::Code("left open".into())]
        );
    }

    #[test]
    fn inline_code_bold_and_links_become_spans() {
        assert_eq!(
            parse_markdown("run `make ci` then **ship** via https://fleet.dev/docs ok"),
            vec![MdBlock::Paragraph(vec![
                text("run "),
                MdSpan::Code("make ci".into()),
                text(" then "),
                MdSpan::Bold("ship".into()),
                text(" via "),
                MdSpan::Link("https://fleet.dev/docs".into()),
                text(" ok"),
            ])]
        );
    }

    #[test]
    fn a_trailing_period_stays_out_of_a_url() {
        assert_eq!(
            parse_markdown("see http://x.dev/a.\nand shttp://y.dev"),
            vec![MdBlock::Paragraph(vec![
                text("see "),
                MdSpan::Link("http://x.dev/a".into()),
                text(". and shttp://y.dev"),
            ])]
        );
    }

    #[test]
    fn unterminated_marks_stay_text() {
        assert_eq!(
            parse_markdown("**bold and `code"),
            vec![MdBlock::Paragraph(vec![text("**bold and `code")])]
        );
        assert_eq!(
            parse_markdown("empty **** marks"),
            vec![MdBlock::Paragraph(vec![text("empty **** marks")])]
        );
    }

    #[test]
    fn multibyte_text_survives_span_splitting() {
        assert_eq!(
            parse_markdown("añ **ñu** `ñ` fin"),
            vec![MdBlock::Paragraph(vec![
                text("añ "),
                MdSpan::Bold("ñu".into()),
                text(" "),
                MdSpan::Code("ñ".into()),
                text(" fin"),
            ])]
        );
    }

    #[test]
    fn list_items_carry_inline_spans() {
        assert_eq!(
            parse_markdown("- **one** two\n- `three`"),
            vec![MdBlock::List {
                ordered: false,
                items: vec![
                    vec![MdSpan::Bold("one".into()), text(" two")],
                    vec![MdSpan::Code("three".into())],
                ],
            }]
        );
    }

    #[test]
    fn a_heading_closes_the_open_block() {
        assert_eq!(
            parse_markdown("prose\n# head\n- a"),
            vec![
                MdBlock::Paragraph(vec![text("prose")]),
                MdBlock::Heading {
                    level: 1,
                    text: "head".into()
                },
                MdBlock::List {
                    ordered: false,
                    items: vec![vec![text("a")]],
                },
            ]
        );
    }

    #[test]
    fn span_text_reads_the_payload() {
        assert_eq!(MdSpan::Code("x".into()).text(), "x");
        assert_eq!(MdSpan::Link("u".into()).text(), "u");
    }
}
