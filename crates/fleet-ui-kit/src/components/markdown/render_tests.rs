use super::*;
use crate::theme::Theme;

fn inlines(source: &str) -> Vec<MarkdownInline> {
    match parse_markdown(source).blocks.into_iter().next() {
        Some(MarkdownBlock::Paragraph(inlines)) => inlines,
        other => panic!("expected one paragraph, got {other:?}"),
    }
}

#[test]
fn a_long_plain_token_is_one_text_run_not_one_flex_child_per_word() {
    let theme = Theme::dark();
    let token = "abcdefghij".repeat(22);
    let nodes = inlines(&token);
    assert_eq!(render::inline_run_count(&nodes, &theme), 1);
}

#[test]
fn the_inline_code_fill_uses_the_secondary_tone_token() {
    let theme = Theme::dark();
    let fill = render::code_fill(&theme);
    assert_eq!(fill, crate::Tone::Secondary.fill(&theme));
}

#[test]
fn flattening_drops_marks_and_keeps_text() {
    let nodes = inlines("a **b** [c](https://example.com) `d`");
    assert_eq!(render::flatten(&nodes), "a b c d");
}

#[gpui::test]
fn a_code_block_is_highlighted_once_per_document(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    code::HIGHLIGHT_CALLS.with(|calls| calls.set(0));

    let document = parse_markdown("```rust\nlet x = 1; // one\n```");
    assert_eq!(code::HIGHLIGHT_CALLS.with(std::cell::Cell::get), 1);
    cx.update(|cx| {
        for _ in 0..3 {
            render::render(&document, false, cx);
        }
    });
    assert_eq!(
        code::HIGHLIGHT_CALLS.with(std::cell::Cell::get),
        1,
        "drawing the document re-lexed the fence"
    );
}

#[gpui::test]
fn an_overlong_token_wraps_inside_a_narrow_measure(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    cx.update(|cx| cx.set_global(Theme::dark()));
    let document = parse_markdown(&"abcdefghij".repeat(22));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| NarrowMarkdownHarness { document })
        })
        .unwrap_or_else(|error| panic!("test window: {error}"))
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();

    let container = visual
        .debug_bounds("markdown-narrow-container")
        .unwrap_or_else(|| panic!("narrow container was not laid out"));
    let paragraph = visual
        .debug_bounds("markdown-inline-text")
        .unwrap_or_else(|| panic!("markdown paragraph was not laid out"));
    assert!(paragraph.size.width <= container.size.width);
    assert!(
        paragraph.size.height > Theme::dark().text.ui.line_height,
        "the oversized token did not wrap"
    );
}

#[gpui::test]
fn a_vertical_wheel_over_code_scrolls_the_transcript(cx: &mut gpui::TestAppContext) {
    use gpui::{AppContext, ScrollDelta, ScrollWheelEvent, point, px};

    cx.update(|cx| cx.set_global(Theme::dark()));
    let tail = (0..24)
        .map(|index| format!("paragraph {index}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let source = format!("```text\n{}\n```\n\n{tail}", "abcdefghij".repeat(30));
    let scroll = gpui::ScrollHandle::new();
    let test_scroll = scroll.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| CodeScrollHarness {
                document: parse_markdown(&source),
                scroll,
            })
        })
        .unwrap_or_else(|error| panic!("test window: {error}"))
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let code = visual
        .debug_bounds("markdown-code-scroll")
        .unwrap_or_else(|| panic!("code block was not laid out"));
    visual.simulate_event(ScrollWheelEvent {
        position: code.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
        ..Default::default()
    });
    assert!(test_scroll.offset().y < px(0.0));
}

#[gpui::test]
fn every_construct_renders_in_both_themes(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext;

    const SOURCE: &str = "# Heading\n\nparagraph **strong** *em* `code` [link](https://x.dev)\n\n- item\n\n> quote\n\n| A | B |\n| --- | ---: |\n| one | two |\n\n```rust\nlet x = 1;\n```\n\n---";
    for theme in [Theme::dark(), Theme::light()] {
        cx.update(|cx| cx.set_global(theme));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| MarkdownHarness {
                    document: parse_markdown(SOURCE),
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

struct NarrowMarkdownHarness {
    document: MarkdownDocument,
}

impl gpui::Render for NarrowMarkdownHarness {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::div().size_full().child(
            gpui::div()
                .debug_selector(|| "markdown-narrow-container".to_owned())
                .w(gpui::px(120.0))
                .child(markdown(&self.document, cx)),
        )
    }
}

struct CodeScrollHarness {
    document: MarkdownDocument,
    scroll: gpui::ScrollHandle,
}

impl gpui::Render for CodeScrollHarness {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::div().size_full().child(
            gpui::div()
                .id("markdown-test-transcript")
                .w(gpui::px(220.0))
                .h(gpui::px(120.0))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(markdown(&self.document, cx)),
        )
    }
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
