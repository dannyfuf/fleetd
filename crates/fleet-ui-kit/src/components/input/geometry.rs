//! The painted-layout half of [`TextInput`]: offsets to points, points to offsets, and the
//! visual-row motion both of those make possible.
//!
//! Everything here reads [`super::element::LineLayoutCache`], so it answers `None` until the
//! current revision has been painted; callers fall back to a logical-line operation instead of
//! consulting stale geometry.

use gpui::{Context, Point, point};

use super::{InputMode, TextInput};

impl TextInput {
    pub(super) fn offset_for_point(&self, position: Point<gpui::Pixels>) -> Option<usize> {
        if self.buffer.is_empty() {
            return Some(0);
        }
        if !self.has_current_layout() {
            return None;
        }
        let bounds = self.last_bounds?;
        let total_rows = self.line_cache.visual_rows().max(1);
        let visual_row = if position.y < bounds.top() {
            0
        } else if position.y >= bounds.bottom() {
            total_rows.saturating_sub(1)
        } else {
            let local_y = position.y - bounds.top();
            self.scroll_row + (f32::from(local_y) / f32::from(self.line_height)).floor() as usize
        }
        .min(total_rows.saturating_sub(1));
        let (start, layout, row_in_line) = self.line_cache.line_for_visual_row(visual_row)?;
        let scroll = if matches!(self.mode(), InputMode::SingleLine) {
            self.horizontal_scroll
        } else {
            gpui::Pixels::ZERO
        };
        let y_in_row = if position.y < bounds.top() || position.y >= bounds.bottom() {
            self.line_height / 2.0
        } else {
            (position.y - bounds.top())
                - self.line_height * visual_row.saturating_sub(self.scroll_row) as f32
        };
        let local = layout.closest_index_for_position(
            point(
                position.x - bounds.left() + scroll,
                self.line_height * row_in_line as f32 + y_in_row,
            ),
            self.line_height,
        );
        Some(start + local)
    }

    pub(super) fn line_for_offset(&self, offset: usize) -> Option<(usize, usize)> {
        self.line_cache.line_index_for_offset(offset)
    }

    pub(super) fn position_for_offset(&self, offset: usize) -> Option<Point<gpui::Pixels>> {
        if !self.has_current_layout() {
            return None;
        }
        let (line_index, local) = self.line_for_offset(offset)?;
        let row_start = self.line_cache.visual_row_start(line_index)?;
        let (_, line) = self.line_cache.line(line_index)?;
        let position = line.position_for_index(local, self.line_height)?;
        Some(point(
            position.x,
            position.y + self.line_height * row_start as f32,
        ))
    }

    pub(super) fn has_current_layout(&self) -> bool {
        self.line_height > gpui::Pixels::ZERO && self.line_cache.is_current(self.buffer.revision())
    }

    pub(super) fn content_height(&self) -> gpui::Pixels {
        self.line_height * self.line_cache.visual_rows().max(1) as f32
    }

    pub(super) fn move_visual_row(&mut self, down: bool, select: bool) -> bool {
        let position = match self.position_for_offset(self.buffer.caret()) {
            Some(position) => position,
            None => return false,
        };
        let x = self.vertical_goal_x.unwrap_or(position.x);
        let y = if down {
            position.y + self.line_height
        } else {
            position.y - self.line_height
        };
        if y < gpui::Pixels::ZERO || y >= self.content_height() {
            return false;
        }
        let Some(offset) = self.offset_for_content_point(point(x, y + self.line_height / 2.0))
        else {
            return false;
        };
        let moved = self.buffer.move_to(offset, select);
        if moved {
            self.vertical_goal_x = Some(x);
        }
        moved
    }

    pub(super) fn move_to_visual_row_boundary(
        &mut self,
        end: bool,
        select: bool,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .position_for_offset(self.buffer.caret())
            .and_then(|position| {
                let x = if end {
                    self.last_bounds?.size.width
                } else {
                    gpui::Pixels::ZERO
                };
                self.offset_for_content_point(point(x, position.y + self.line_height / 2.0))
            });
        let moved = match target {
            Some(target) => self.buffer.move_to(target, select),
            None if end => self.buffer.move_to_line_end(select),
            None => self.buffer.move_to_line_start(select),
        };
        self.vertical_goal_x = None;
        if moved {
            self.reveal_caret = true;
            cx.notify();
        }
    }

    fn offset_for_content_point(&self, position: Point<gpui::Pixels>) -> Option<usize> {
        if !self.has_current_layout() {
            return None;
        }
        let total_rows = self.line_cache.visual_rows().max(1);
        let visual_row = (f32::from(position.y.max(gpui::Pixels::ZERO))
            / f32::from(self.line_height))
        .floor() as usize;
        let visual_row = visual_row.min(total_rows.saturating_sub(1));
        let (start, layout, row_in_line) = self.line_cache.line_for_visual_row(visual_row)?;
        let local = layout.closest_index_for_position(
            point(
                position.x,
                self.line_height * row_in_line as f32 + self.line_height / 2.0,
            ),
            self.line_height,
        );
        Some(start + local)
    }
}
