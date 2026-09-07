//! Long lines retain their shaping and paint only chunks intersecting the viewport.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fleet_ui_kit::{Theme, prelude::*};
use gpui::{
    AnyElement, Font, FontFeatures, Hsla, Pixels, ShapedLine, TextAlign, TextRun, WindowTextSystem,
    canvas, font, point, px,
};

use super::diff_model::RowKind;

pub(crate) const SHAPE_LIMIT: usize = 4096;
const CHUNK_BYTES: usize = 1024;

/// Every input to retained glyph shaping. Row tints remain live render inputs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    font: Font,
    size: Pixels,
    text: Hsla,
    secondary: Hsla,
    muted: Hsla,
}

impl Style {
    pub(crate) fn new(theme: &Theme) -> Self {
        let mut font = font(theme.font_mono.clone());
        font.features = FontFeatures::disable_ligatures();
        Self {
            font,
            size: theme.text.data.size,
            text: theme.colors.text,
            secondary: theme.colors.text_secondary,
            muted: theme.colors.text_muted,
        }
    }
}

#[derive(Debug)]
struct Chunk {
    left: Pixels,
    line: ShapedLine,
}

#[derive(Debug)]
pub(crate) struct LongLine {
    chunks: Vec<Chunk>,
}

impl LongLine {
    pub(crate) fn prepare(
        text: gpui::SharedString,
        kind: RowKind,
        style: &Style,
        text_system: &WindowTextSystem,
        cancelled: &AtomicBool,
    ) -> Option<Self> {
        let color = match kind {
            RowKind::Context => style.secondary,
            RowKind::Other => style.muted,
            _ => style.text,
        };
        let run = TextRun {
            len: text.len(),
            font: style.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = text_system.shape_line(text, style.size, &[run], None);
        let mut chunks = Vec::new();
        if !partition(line, px(0.0), &mut chunks, cancelled) {
            return None;
        }
        Some(Self { chunks })
    }

    fn visible(&self, left: Pixels, width: Pixels) -> std::ops::Range<usize> {
        let start = self
            .chunks
            .partition_point(|chunk| chunk.left + chunk.line.width() < left)
            .saturating_sub(1);
        let end = self
            .chunks
            .partition_point(|chunk| chunk.left <= left + width)
            .saturating_add(1)
            .min(self.chunks.len());
        start..end
    }

    pub(crate) fn element(self: &Arc<Self>, offset: f32, line_height: Pixels) -> AnyElement {
        let line = self.clone();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                let offset = px(offset);
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for chunk in &line.chunks[line.visible(offset, bounds.size.width)] {
                        let origin = bounds.origin + point(chunk.left - offset, px(0.0));
                        if let Err(error) =
                            chunk
                                .line
                                .paint(origin, line_height, TextAlign::Left, None, window, cx)
                        {
                            tracing::warn!(%error, "diff long-line paint failed");
                            break;
                        }
                    }
                });
            },
        )
        .w_full()
        .h(line_height)
        .into_any_element()
    }
}

/// Divide at glyph-cluster boundaries without reshaping either half. Balanced partitioning
/// avoids repeatedly copying the entire suffix when a minified file contains a huge line.
fn partition(
    line: ShapedLine,
    left: Pixels,
    chunks: &mut Vec<Chunk>,
    cancelled: &AtomicBool,
) -> bool {
    if cancelled.load(Ordering::Relaxed) {
        return false;
    }
    if line.len() > CHUNK_BYTES {
        let middle = line.len() / 2;
        let split = line
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|glyph| glyph.index)
            .find(|index| *index >= middle && *index < line.len());
        if let Some(split) = split {
            let (head, tail) = line.split_at(split);
            let tail_left = left + head.width();
            return partition(head, left, chunks, cancelled)
                && partition(tail, tail_left, chunks, cancelled);
        }
    }
    chunks.push(Chunk { left, line });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn long_lines_keep_all_text_and_only_visit_visible_glyph_chunks(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let text = "let café = \"你好 👩‍💻\"; ".repeat(4000);
            let style = Style::new(&Theme::dark());
            let system = WindowTextSystem::new(cx.text_system().clone());
            let prepared = LongLine::prepare(
                text.clone().into(),
                RowKind::Added,
                &style,
                &system,
                &AtomicBool::new(false),
            )
            .unwrap();
            let rebuilt: String = prepared
                .chunks
                .iter()
                .map(|chunk| chunk.line.text.as_str())
                .collect();
            assert_eq!(rebuilt, text);
            assert!(prepared.chunks.len() > 100);
            let width = prepared
                .chunks
                .last()
                .map(|chunk| chunk.left + chunk.line.width())
                .unwrap();
            for offset in [px(0.0), width / 2.0, width - px(800.0)] {
                let visible = prepared.visible(offset, px(800.0));
                assert!(visible.len() < 8, "visible chunks: {}", visible.len());
                assert!(visible.len() < prepared.chunks.len());
            }
        });
    }
}
