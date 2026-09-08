//! Fixed-geometry native-agent tool row.
//!
//! §2 of `NATIVE-AGENTS.md`: *every* tool call is one 30 px row, `state glyph · 60 px kind
//! column · one-line summary · right-aligned result`, and it keeps that geometry while it
//! streams so the transcript does not jitter under the reader. The row is the most repeated
//! element of an agent thread, so its anatomy is defined once here and reused verbatim by the
//! nested children region of an `Agent` call.

use std::rc::Rc;

use gpui::{AnyElement, App, ElementId, SharedString, Window, div, prelude::*};

use crate::{
    components::{KeyHint, Spinner},
    focus::FocusRing,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

use super::metrics::{AGENT_BODY_MAX_H, AGENT_TOOL_KIND_W};

/// Visual lifecycle of a tool row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRowState {
    /// The tool is pending or executing.
    Running,
    /// The tool completed successfully.
    Done,
    /// The tool completed with an error.
    Error,
    /// The tool was denied by a gate answer.
    Denied,
}

impl ToolRowState {
    /// The glyph and semantic tone this state draws.
    ///
    /// [`ToolRowState::Running`] reports the spinner glyph; the renderer is the one that makes
    /// it turn, because a spinner needs a stable element id and this is a pure query.
    #[must_use]
    pub fn glyph(self) -> (Icon, Tone) {
        match self {
            // Gray, not amber: §2 reserves amber for "needs you", and progress is gray.
            ToolRowState::Running => (Icon::LoaderCircle, Tone::Secondary),
            ToolRowState::Done => (Icon::CircleCheck, Tone::Success),
            ToolRowState::Error => (Icon::CircleX, Tone::Danger),
            ToolRowState::Denied => (Icon::TriangleAlert, Tone::Warning),
        }
    }

    /// Whether this state animates.
    #[must_use]
    pub fn is_running(self) -> bool {
        matches!(self, ToolRowState::Running)
    }
}

/// Domain-neutral properties for one 30 px native-agent tool row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    /// Stable UI key.
    pub id: SharedString,
    /// Lifecycle glyph state.
    pub state: ToolRowState,
    /// Fixed kind-column label.
    pub kind: SharedString,
    /// One-line invocation summary.
    pub summary: SharedString,
    /// Optional right-aligned result.
    pub result: Option<SharedString>,
    /// Optional expanded output text.
    pub output: Option<SharedString>,
    /// Optional unified diff text.
    pub diff: Option<SharedString>,
    /// Whether nested output is currently expanded.
    pub expanded: bool,
}

impl ToolRow {
    /// A running row with no result yet.
    #[must_use]
    pub fn new(
        id: impl Into<SharedString>,
        kind: impl Into<SharedString>,
        summary: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            state: ToolRowState::Running,
            kind: kind.into(),
            summary: summary.into(),
            result: None,
            output: None,
            diff: None,
            expanded: false,
        }
    }

    /// Set the lifecycle state.
    #[must_use]
    pub fn state(mut self, state: ToolRowState) -> Self {
        self.state = state;
        self
    }

    /// Set the right-aligned result hint.
    #[must_use]
    pub fn result(mut self, result: impl Into<SharedString>) -> Self {
        self.result = Some(result.into());
        self
    }

    /// Set the expanded body text.
    #[must_use]
    pub fn output(mut self, output: impl Into<SharedString>) -> Self {
        self.output = Some(output.into());
        self
    }

    /// Set the expanded unified diff.
    #[must_use]
    pub fn diff(mut self, diff: impl Into<SharedString>) -> Self {
        self.diff = Some(diff.into());
        self
    }

    /// Expand or collapse the row.
    #[must_use]
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Whether the row has anything to show when it is expanded.
    #[must_use]
    pub fn has_body(&self) -> bool {
        self.diff.is_some() || self.output.is_some()
    }
}

/// Renders one tool row from presentation properties.
///
/// This is the read-only form: no focus ring, no children, no toggle. Use
/// [`ToolRowElement`] for the interactive row the transcript draws.
pub fn tool_row(row: &ToolRow, _cx: &App) -> impl IntoElement {
    ToolRowElement::new(row.clone())
}

/// The interactive tool row: focus ring, expanded body, and the nested children region an
/// `Agent` call draws its sub-tools in.
#[derive(IntoElement)]
pub struct ToolRowElement {
    row: ToolRow,
    focused: bool,
    body: Option<AnyElement>,
    children: Vec<AnyElement>,
    #[allow(clippy::type_complexity)]
    on_toggle: Option<Rc<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl ToolRowElement {
    /// A row from its presentation properties.
    #[must_use]
    pub fn new(row: ToolRow) -> Self {
        Self {
            row,
            focused: false,
            body: None,
            children: Vec::new(),
            on_toggle: None,
        }
    }

    /// Draw the keyboard focus affordance.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Replace the default expanded body (the diff, else the output text).
    #[must_use]
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The nested rows of an `Agent` call, already rendered in display order.
    #[must_use]
    pub fn children(mut self, children: Vec<AnyElement>) -> Self {
        self.children = children;
        self
    }

    /// What `⏎` (and a click) does. The transcript owns the focus handle, so it is the caller
    /// that turns the key into this call; the row turns a click into it directly.
    #[must_use]
    pub fn on_toggle(mut self, on_toggle: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }
}

impl RenderOnce for ToolRowElement {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let row = self.row;
        let expandable = row.has_body() || !self.children.is_empty();
        let (icon, tone) = row.state.glyph();
        let glyph_id = ElementId::from(SharedString::from(format!("tool-glyph-{}", row.id)));

        let glyph = if row.state.is_running() {
            Spinner::new(glyph_id)
                .size(IconSize::Large)
                .tone(tone)
                .into_any_element()
        } else {
            icon.el()
                .size(IconSize::Large)
                .tone(tone)
                .into_any_element()
        };

        let line = div()
            .id(ElementId::from(SharedString::from(format!(
                "tool-line-{}",
                row.id
            ))))
            .h(theme.metrics.row_h)
            .w_full()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .child(div().flex_none().child(glyph))
            .child(
                Text::data_small(row.kind.clone())
                    .muted()
                    .w(AGENT_TOOL_KIND_W)
                    .flex_none()
                    .ellipsize(),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .child(Text::data(row.summary.clone()).ellipsize()),
            )
            .children(
                row.result
                    .clone()
                    .map(|result| Text::hint(result).faint().flex_none()),
            )
            // §7: an affordance states its key. A row with a diff, an output or nested children
            // says how to see them; a row with nothing underneath stays silent.
            .when(expandable, |el| el.child(expand_hint(row.expanded)))
            // The click belongs to the 30 px line, never to the body it opens: clicking inside
            // an expanded diff or a nested tool must not fold the row the reader is reading.
            .when_some(self.on_toggle, |el, on_toggle| {
                el.on_click(move |_, window, cx| on_toggle(window, cx))
            });

        let body = row.expanded.then(|| {
            let content = self.body.or_else(|| {
                row.diff
                    .clone()
                    .or_else(|| row.output.clone())
                    .map(|text| Text::data_small(text).muted().into_any_element())
            });
            content.map(|content| {
                div()
                    .id(ElementId::from(SharedString::from(format!(
                        "tool-body-{}",
                        row.id
                    ))))
                    .ml(theme.space.lg)
                    .max_h(AGENT_BODY_MAX_H)
                    .overflow_y_scroll()
                    .child(content)
            })
        });

        let nested = (row.expanded && !self.children.is_empty()).then(|| {
            div()
                .ml(theme.space.lg)
                .p(theme.space.md)
                .border_l(theme.metrics.hairline)
                .border_color(theme.colors.border)
                .flex()
                .flex_col()
                .children(self.children)
        });

        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .child(line)
            .children(body.flatten())
            .children(nested);

        div()
            .id(ElementId::from(SharedString::from(format!(
                "tool-row-{}",
                row.id
            ))))
            .w_full()
            .child(FocusRing::cursor_row(self.focused).content(content))
    }
}

/// The `[⏎] show` hint a collapsed row with a body draws.
#[must_use]
pub fn expand_hint(expanded: bool) -> KeyHint {
    KeyHint::labeled("⏎", if expanded { "hide" } else { "show" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_maps_to_its_canvas_glyph() {
        assert_eq!(
            ToolRowState::Running.glyph(),
            (Icon::LoaderCircle, Tone::Secondary)
        );
        assert_eq!(
            ToolRowState::Done.glyph(),
            (Icon::CircleCheck, Tone::Success)
        );
        assert_eq!(ToolRowState::Error.glyph(), (Icon::CircleX, Tone::Danger));
        assert_eq!(
            ToolRowState::Denied.glyph(),
            (Icon::TriangleAlert, Tone::Warning)
        );
    }

    #[test]
    fn only_a_running_row_animates() {
        assert!(ToolRowState::Running.is_running());
        for state in [
            ToolRowState::Done,
            ToolRowState::Error,
            ToolRowState::Denied,
        ] {
            assert!(!state.is_running());
        }
    }

    #[test]
    fn a_row_has_a_body_only_when_it_carries_output_or_a_diff() {
        let row = ToolRow::new("t1", "bash", "cargo test");
        assert!(!row.has_body());
        assert!(row.clone().output("ok").has_body());
        assert!(row.diff("@@ -1 +1 @@").has_body());
    }

    #[test]
    fn builders_keep_the_id_stable() {
        let row = ToolRow::new("t1", "edit", "src/lib.rs")
            .state(ToolRowState::Done)
            .result("+14 \u{2212}3")
            .expanded(true);
        assert_eq!(row.id, "t1");
        assert_eq!(row.state, ToolRowState::Done);
        assert!(row.expanded);
    }
}
