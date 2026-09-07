use gpui::{App, Div, ElementId, ScrollHandle, SharedString, Window, div, prelude::*};

use super::line::{caret_bar, line_row, reveal_caret};

use crate::{
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
};

/// How many rows a [`TextArea`] shows before it grows, unless [`TextArea::rows`] says otherwise.
pub const TEXT_AREA_ROWS: u32 = 6;

/// A multi-line text input. The caller owns the string, the caret and the keys.
#[derive(IntoElement)]
pub struct TextArea {
    value: SharedString,
    cursor: Option<usize>,
    focused: bool,
    placeholder: Option<SharedString>,
    label: Option<SharedString>,
    rows: u32,
    max_rows: Option<u32>,
    scroll_row: usize,
    scroll: Option<(ElementId, ScrollHandle)>,
    mono: bool,
    invalid: bool,
}

impl TextArea {
    /// An area showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            cursor: None,
            focused: false,
            placeholder: None,
            label: None,
            rows: TEXT_AREA_ROWS,
            max_rows: None,
            scroll_row: 0,
            scroll: None,
            mono: false,
            invalid: false,
        }
    }

    /// The caret position as a **byte** offset — [`super::TextAreaState::cursor`] verbatim. Drawn
    /// only while focused, and snapped to a `char` boundary before use.
    pub fn cursor(mut self, byte_offset: usize) -> Self {
        self.cursor = Some(byte_offset);
        self
    }

    /// Whether the area has keyboard focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Text shown when the value is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// A label above the box.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The minimum number of visible rows. [`TEXT_AREA_ROWS`] by default.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows.max(1);
        self
    }

    /// Cap the box at `rows` rows and clip the rest.
    ///
    /// Additive to the §7 contract: a description editor inside a dialog cannot be allowed to
    /// push the dialog's footer off screen. The caller keeps the caret visible with
    /// [`super::TextAreaState::reveal_cursor`] **and** [`TextArea::scroll`]: the row window is counted
    /// in logical lines, the cap is measured in pixels, and the body word-wraps, so a capped box
    /// without a scroll handle clips whatever the wrapping pushed past the cap — the caret
    /// included.
    pub fn max_rows(mut self, rows: u32) -> Self {
        self.max_rows = Some(rows.max(1));
        self
    }

    /// Own the box's pixel scrolling, so the caret stays visible when lines wrap.
    ///
    /// [`super::TextAreaState::scroll_row`] moves a window of whole logical lines, which is what bounds
    /// the work per keystroke; it cannot say how tall those lines became once the box wrapped
    /// them. The handle closes that gap: the element measures the caret's line and scrolls to it,
    /// so a paragraph that occupies six visual rows never hides the row being typed on.
    ///
    /// `id` must be unique among the caller's elements, and the handle must outlive the frame —
    /// keep it in the dialog's draft, next to the [`super::TextAreaState`].
    pub fn scroll(mut self, id: impl Into<ElementId>, handle: ScrollHandle) -> Self {
        self.scroll = Some((id.into(), handle));
        self
    }

    /// First visible logical line, from `super::TextAreaState::scroll_row`.
    pub fn scroll_row(mut self, row: usize) -> Self {
        self.scroll_row = row;
        self
    }

    /// Render the value in the data face (descriptions that are really code, commit bodies).
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Draw the box in the danger color: the value is rejected.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// The type role the value renders in.
    fn role(&self) -> TextRole {
        if self.mono {
            TextRole::Data
        } else {
            TextRole::Ui
        }
    }
}

impl RenderOnce for TextArea {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let role = self.role();
        let line_height = role.style(theme).line_height;
        let padding = theme.space.sm;
        // Danger beats focus, focus beats rest — the ladder `TextField` uses.
        let border = if self.invalid {
            theme.colors.danger
        } else if self.focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        };

        let value = self.value.clone();
        let empty = value.is_empty();
        // The caret is a byte offset into the whole value; find its line, its offset inside that
        // line and how far along the line it sits, once, instead of per rendered row.
        let caret = self.focused.then(|| {
            let mut offset = self.cursor.unwrap_or(value.len()).min(value.len());
            while offset > 0 && !value.is_char_boundary(offset) {
                offset -= 1;
            }
            let line_start = value[..offset].rfind('\n').map_or(0, |index| index + 1);
            let line_end = value[line_start..]
                .find('\n')
                .map_or(value.len(), |index| line_start + index);
            let progress = if line_end > line_start {
                (offset - line_start) as f32 / (line_end - line_start) as f32
            } else {
                0.0
            };
            (
                value[..line_start].matches('\n').count(),
                offset - line_start,
                progress,
            )
        });

        let content: Vec<Div> = if empty {
            // The placeholder never carries a caret inside it: the caret sits before it.
            vec![
                div()
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .items_center()
                    .w_full()
                    .min_h(line_height)
                    .children(self.focused.then(|| caret_bar(theme, role)))
                    .children(
                        self.placeholder
                            .map(|placeholder| Text::new(role, placeholder).faint()),
                    ),
            ]
        } else {
            // `max_rows` caps the box; without the take, a pasted thousand-line description
            // still builds a thousand rows, and a word div per word in each, on every keystroke.
            value
                .split('\n')
                .enumerate()
                .skip(self.scroll_row)
                .take(self.max_rows.map_or(usize::MAX, |rows| rows as usize))
                .map(|(index, line)| {
                    let caret = caret.and_then(|(line_index, offset, _)| {
                        (line_index == index).then_some(offset)
                    });
                    line_row(theme, role, line, caret)
                })
                .collect()
        };

        // The rows are the scroll container's own children, so gpui measures each one and can
        // reveal the caret's line whatever the wrapping did to it.
        let base = styled_with(div(), role.style(theme), theme)
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .min_h(line_height * (self.rows as f32))
            .text_color(theme.colors.text);
        let body = match (self.max_rows, self.scroll) {
            (Some(rows), Some((id, handle))) => {
                if let Some((line, _, progress)) = caret {
                    reveal_caret(
                        &handle,
                        line.saturating_sub(self.scroll_row),
                        progress,
                        line_height,
                    );
                }
                base.id(id)
                    .max_h(line_height * (rows as f32))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .children(content)
                    .into_any_element()
            }
            (Some(rows), None) => base
                .max_h(line_height * (rows as f32))
                .overflow_hidden()
                .children(content)
                .into_any_element(),
            (None, _) => base.children(content).into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(self.label.map(Text::label))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .px(theme.space.md)
                    .py(padding)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border(theme.metrics.hairline)
                    .border_color(border)
                    .overflow_hidden()
                    .child(body),
            )
    }
}
