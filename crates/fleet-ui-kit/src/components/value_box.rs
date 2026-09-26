//! `ValueBox` — a setting's text or number value in a box that becomes its editor in place.
//!
//! **Purpose.** A text, number or multi-line setting shows its value in a 26 px box at the end of
//! its [`super::SettingsRow`]. The box states the value; it is not an editor. The surface opens a
//! live [`TextInput`] on the row — its `⏎`, or a click through [`ValueBox::on_click`] — and hands
//! it back with [`ValueBox::editor`], which the box draws **inside the same box**, so opening and
//! closing the editor never moves the box nor changes the row's height. An empty value shows the
//! placeholder, which says what empty means (`Harness default`).
//!
//! It replaces the old label-beside-value text field and the old integer field with a unit and a
//! clamp. The label is now the row's; the clamp became [`number_rule`], a pure
//! function the surface runs when it commits, because the rule belongs to the value, not to the
//! box: §3.8 says an out-of-range value is refused **at the field**, stating the exact rule
//! (`Must be at least 500 ms.`), and the row shows that sentence in place of its helper.
//!
//! **Anatomy.** `bg` fill, a hairline `control_border`, `radii.control` corners, `px sm`. The
//! width is a [`ValueBoxWidth`]: 300 px text (`value_box_w`), 96 px number (`number_field_w`),
//! 120 px short identifier (`value_box_short_w`), or the row's full width. Single line: 26 px
//! (`button_h_compact`), the value ellipsised, then the muted [`unit`](ValueBox::unit). Multi-line:
//! at least three lines (`value_area_h`), the value wrapped, at most [`VALUE_BOX_MAX_ROWS`] lines;
//! the caller gives the editor the same bound (`InputMode::Multiline`).
//!
//! **States.** rest · hover (with a click handler: `border_strong`) · **editing** — an editor is
//! present and focused: a `focus_ring` border drawn `focus_ring_w` wide **inset**, so the box does
//! not grow · invalid (`danger` border, the value in `danger`). An editor that is present but not
//! focused (a hooks list, where every row is always editable) is drawn in the box at rest.
//!
//! **Usage rule.** Use it as a `SettingsRow`'s control. Hand it an **embedded** editor
//! (`TextInput::set_embedded(true)`): the box is the chrome. The caller sets the editor's mode,
//! filter and placeholder. A form field that is always editable outside a settings row is a bare
//! [`TextInput`].

use gpui::{App, ElementId, Entity, MouseButton, SharedString, Window, div, prelude::*};

use crate::{components::input::TextInput, text::Text, theme::ActiveTheme, tone::Tone};

/// The most lines a multi-line box shows, at rest and while editing.
pub const VALUE_BOX_MAX_ROWS: usize = 8;

type ValueBoxClick = dyn Fn(&mut Window, &mut App);

/// How wide a [`ValueBox`] is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValueBoxWidth {
    /// A text or multi-line value: `value_box_w`. The default.
    #[default]
    Text,
    /// A number with its unit: `number_field_w`.
    Number,
    /// A short identifier such as a board prefix: `value_box_short_w`.
    Short,
    /// The row's whole remaining width: a hooks command.
    Fill,
}

/// A setting's value, drawn in a box that holds the row's editor while it is open.
#[derive(IntoElement)]
pub struct ValueBox {
    id: ElementId,
    value: SharedString,
    placeholder: Option<SharedString>,
    mono: bool,
    unit: Option<SharedString>,
    width: ValueBoxWidth,
    multiline: bool,
    editor: Option<Entity<TextInput>>,
    invalid: bool,
    on_click: Option<Box<ValueBoxClick>>,
}

impl ValueBox {
    /// A box showing `value`.
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            placeholder: None,
            mono: false,
            unit: None,
            width: ValueBoxWidth::default(),
            multiline: false,
            editor: None,
            invalid: false,
            on_click: None,
        }
    }

    /// What an empty value means, drawn faint in the UI face.
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Draw the value in the data face: a command, a path, an identifier.
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// A muted caption after the value: `ms`, `min`, `of 8`. It is a label, not a value, so it
    /// never competes with the number.
    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// How wide the box is. [`ValueBoxWidth::Text`] unless set.
    pub fn width(mut self, width: ValueBoxWidth) -> Self {
        self.width = width;
        self
    }

    /// A multi-line value: at rest the value wraps in a box three lines tall, at most
    /// [`VALUE_BOX_MAX_ROWS`]; while editing the editor grows to the same bound. Put the box in a
    /// `tall` row.
    pub fn multiline(mut self, multiline: bool) -> Self {
        self.multiline = multiline;
        self
    }

    /// The live editor, embedded, drawn inside the box in the value's place.
    ///
    /// The editor owns the text, the caret and the filter of what is typed; the box keeps only
    /// its chrome, its width and the unit around it.
    pub fn editor(mut self, editor: Entity<TextInput>) -> Self {
        self.editor = Some(editor);
        self
    }

    /// The value breaks the row's rule: a `danger` border and the value in `danger`. The row
    /// states the rule itself ([`super::SettingsRow::invalid`]).
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// A click on the box. Point it at what the row's `⏎` does: open the editor. The click stops
    /// there, so the row's own press handler does not also run; the caller lands the cursor.
    pub fn on_click(mut self, on_click: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
    }

    /// Whether the box holds an editor.
    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// What the box reads at rest: the value, or the placeholder (and `true`) when the value is
    /// empty.
    pub fn shown(&self) -> (SharedString, bool) {
        match (&self.placeholder, self.value.is_empty()) {
            (Some(placeholder), true) => (placeholder.clone(), true),
            _ => (self.value.clone(), false),
        }
    }
}

/// The rule an integer setting's `value` breaks, stated as the row shows it, or `None` when the
/// value is inside `min..=max`.
///
/// Sentence case with a full stop, and the unit when there is one: `Must be at least 500 ms.`,
/// `Must be between 1 and 8.`, `Must be at most 120 min.`. A bound that is `None` is open.
pub fn number_rule(
    value: i64,
    min: Option<i64>,
    max: Option<i64>,
    unit: Option<&str>,
) -> Option<SharedString> {
    let below = min.is_some_and(|min| value < min);
    let above = max.is_some_and(|max| value > max);
    if !below && !above {
        return None;
    }
    let unit = unit.map(|unit| format!(" {unit}")).unwrap_or_default();
    let rule = match (min, max) {
        (Some(min), Some(max)) => format!("Must be between {min} and {max}{unit}."),
        (Some(min), None) => format!("Must be at least {min}{unit}."),
        (None, Some(max)) => format!("Must be at most {max}{unit}."),
        // Unreachable: a value is out of range only against a bound.
        (None, None) => return None,
    };
    Some(rule.into())
}

impl RenderOnce for ValueBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (shown, is_placeholder) = self.shown();
        let editing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.read(cx).focus_handle().is_focused(window));
        let theme = cx.theme();
        let invalid = self.invalid;
        let hover_border = theme.colors.border_strong;
        let border = if invalid {
            theme.colors.danger
        } else if editing {
            theme.colors.focus_ring
        } else {
            theme.colors.control_border
        };
        let ui_line = theme.text.ui.line_height;
        let pad_y = theme.space.xs + theme.space.xxs;

        let has_editor = self.editor.is_some();
        let has_unit = self.unit.is_some();
        let value = match self.editor {
            // The editor is embedded (no chrome of its own), so the box around it is this one
            // and the row keeps its height.
            Some(editor) => div().flex_1().min_w_0().child(editor).into_any_element(),
            None => {
                let text = if self.mono && !is_placeholder {
                    Text::data(shown.clone())
                } else {
                    Text::ui(shown.clone())
                };
                let text = if is_placeholder {
                    text.faint()
                } else if invalid {
                    text.tone(Tone::Danger)
                } else {
                    text
                };
                let text = if self.multiline {
                    text
                } else {
                    text.ellipsize()
                };
                div()
                    // The unit reads right after the number (`2  of 8`), so a value with a
                    // unit takes only its own width and the spacer after the unit fills the
                    // box; a value without one fills the box itself and ellipsises inside it.
                    .when(!has_unit, |el| el.flex_1())
                    .min_w_0()
                    .overflow_hidden()
                    .when(self.multiline, |el| {
                        el.max_h(ui_line * VALUE_BOX_MAX_ROWS as f32)
                    })
                    .child(text)
                    .into_any_element()
            }
        };

        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .gap(theme.space.xs)
            .px(theme.space.sm)
            .map(|el| match self.width {
                ValueBoxWidth::Text => el.w(theme.metrics.value_box_w),
                ValueBoxWidth::Number => el.w(theme.metrics.number_field_w),
                ValueBoxWidth::Short => el.w(theme.metrics.value_box_short_w),
                ValueBoxWidth::Fill => el.w_full().flex_1().min_w_0(),
            })
            .map(|el| {
                if self.multiline {
                    el.items_start().min_h(theme.metrics.value_area_h).py(pad_y)
                } else {
                    el.items_center().h(theme.metrics.button_h_compact)
                }
            })
            .rounded(theme.radii.control)
            .bg(theme.colors.bg)
            .border(theme.metrics.hairline)
            .border_color(border)
            .overflow_hidden()
            .child(value)
            .children(
                self.unit
                    .map(|unit| Text::caption(unit).muted().flex_none()),
            )
            .when(has_unit && !has_editor, |el| el.child(div().flex_1()))
            // Editing thickens the border *inward*: a second ring painted over the box's own
            // edge, so the box keeps its size and the value inside it never shifts.
            .when(editing, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme.radii.control)
                        .border(theme.metrics.focus_ring_w)
                        .border_color(border),
                )
            })
            .when(!editing, |el| {
                el.role(gpui::Role::TextInput).aria_label(shown)
            })
            // A press on the box is the box's, whether it opens the editor or places the caret
            // in one: the row behind it lands its cursor on a press and opens on the second
            // press of a double click, and neither is what a press inside a box asks for. The
            // editor's own handler runs first (it is the child), so the caret still lands.
            .when(self.on_click.is_some() || has_editor, |el| {
                el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .when_some(self.on_click, |el, on_click| {
                el.cursor_text()
                    .when(!invalid && !editing, |el| {
                        el.hover(move |style| style.border_color(hover_border))
                    })
                    .on_click(move |_, window, cx| {
                        on_click(window, cx);
                        cx.stop_propagation();
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::InputMode;

    #[test]
    fn an_empty_value_shows_its_placeholder_and_a_set_one_shows_itself() {
        let empty = ValueBox::new("model", "").placeholder("Harness default");
        assert_eq!(empty.shown(), ("Harness default".into(), true));
        let set = ValueBox::new("model", "opus").placeholder("Harness default");
        assert_eq!(set.shown(), ("opus".into(), false));
        assert_eq!(ValueBox::new("command", "").shown(), ("".into(), false));
    }

    #[test]
    fn a_box_without_an_editor_is_not_editing() {
        assert!(!ValueBox::new("command", "claude").is_editing());
    }

    #[gpui::test]
    fn a_box_with_an_editor_is_editing(cx: &mut gpui::TestAppContext) {
        let editor = cx.new(|cx| TextInput::new(InputMode::SingleLine, cx));
        let value_box = ValueBox::new("command", "claude").editor(editor);
        assert!(value_box.is_editing());
    }

    #[test]
    fn a_value_inside_its_range_breaks_no_rule() {
        assert!(number_rule(3, Some(1), Some(8), None).is_none());
        assert!(number_rule(500, Some(500), None, Some("ms")).is_none());
        assert!(number_rule(-40, None, None, None).is_none());
    }

    #[test]
    fn a_value_below_a_lone_minimum_states_the_minimum_with_its_unit() {
        assert_eq!(
            number_rule(200, Some(500), None, Some("ms")).as_deref(),
            Some("Must be at least 500 ms.")
        );
        assert_eq!(
            number_rule(-1, Some(0), None, None).as_deref(),
            Some("Must be at least 0.")
        );
    }

    #[test]
    fn a_value_outside_a_closed_range_states_both_ends() {
        assert_eq!(
            number_rule(9, Some(1), Some(8), None).as_deref(),
            Some("Must be between 1 and 8.")
        );
        assert_eq!(
            number_rule(100, Some(500), Some(5_000), Some("ms")).as_deref(),
            Some("Must be between 500 and 5000 ms.")
        );
    }

    #[test]
    fn a_value_above_a_lone_maximum_states_the_maximum() {
        assert_eq!(
            number_rule(121, None, Some(120), Some("min")).as_deref(),
            Some("Must be at most 120 min.")
        );
    }
}
