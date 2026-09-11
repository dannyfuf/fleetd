//! Fixed-geometry native-agent tool row.
//!
//! §5 of `docs/NATIVE-AGENTS.md`: *every* tool call is one 30 px row —
//! `state glyph · 60 px kind column · one-line summary · right-aligned result · expand hint` —
//! and it keeps that geometry while it streams, so the transcript does not jitter under the
//! reader. The row is the most repeated element of an agent thread, so its anatomy is defined
//! once here and reused verbatim by the nested children region of a subagent call.
//!
//! Two rules carry the design and both are testable without a window:
//!
//! - **Five states and no more** — `Running | Done | Failed | Denied | Stopped` — plus
//!   [`ToolRowState::Severe`], reserved for a runtime error or a broken side effect. *A `git
//!   grep` finding nothing is not red*, and a nonzero exit is a structured `exit 1` field, not
//!   a colour.
//! - **The click target is the 30 px line only**, so clicking inside an expanded body or a
//!   nested child never folds the row the reader is reading.

use std::rc::Rc;

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

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
///
/// The vocabulary is closed at five states plus one severity escalation. It mirrors the
/// harness-neutral lifecycle (`inProgress | completed | failed | declined | stopped`) rather
/// than any one harness's spelling, and nothing widens it: a sixth visual state would need a
/// sixth thing for the reader to learn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRowState {
    /// The tool is pending or executing.
    Running,
    /// The tool completed.
    Done,
    /// A routine tool failure: a nonzero exit, a harness `failed` status.
    Failed,
    /// The user denied the call through a gate.
    Denied,
    /// The turn was interrupted before the call settled.
    Stopped,
    /// A runtime error or a broken side effect — *the turn or a core side effect broke, not
    /// that a command exited nonzero*. This is the only state that paints the heading red.
    Severe,
}

/// The glyph one state draws, resolved against the row's own kind icon.
///
/// A settled row shows *what it did* (the kind icon); only the states that need a distinct
/// shape — running, denied, stopped, severe — replace it. Returned as data rather than as an
/// element so the state table is unit-testable and the renderer stays the only thing that
/// animates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolGlyph {
    /// Which glyph to draw.
    pub icon: Icon,
    /// Its tint.
    pub tone: Tone,
    /// An opacity to apply to the tint, when the state calls for a dimmed one.
    pub opacity: Option<f32>,
    /// Whether the glyph turns.
    pub spinning: bool,
}

impl ToolRowState {
    /// The glyph and tone this state draws, given the row's kind icon.
    ///
    /// [`ToolRowState::Running`] reports the spinner glyph and `spinning`; the renderer is what
    /// makes it turn, because a spinner needs a stable element id and this is a pure query.
    #[must_use]
    pub fn glyph(self, kind: Icon, dimmed_opacity: f32) -> ToolGlyph {
        let plain = |icon, tone| ToolGlyph {
            icon,
            tone,
            opacity: None,
            spinning: false,
        };
        match self {
            // Gray, not amber: §5 reserves amber for "needs you", and progress is gray.
            ToolRowState::Running => ToolGlyph {
                icon: Icon::LoaderCircle,
                tone: Tone::Secondary,
                opacity: None,
                spinning: true,
            },
            ToolRowState::Done => plain(kind, Tone::Muted),
            // A routine failure keeps the kind icon — the reader still wants to know *what*
            // failed — and states the failure in the result chip instead of shouting in red.
            ToolRowState::Failed => ToolGlyph {
                icon: kind,
                tone: Tone::Danger,
                opacity: Some(dimmed_opacity),
                spinning: false,
            },
            ToolRowState::Denied => plain(Icon::CircleSlash, Tone::Warning),
            ToolRowState::Stopped => plain(Icon::CircleStop, Tone::Muted),
            ToolRowState::Severe => plain(Icon::TriangleAlert, Tone::Danger),
        }
    }

    /// The tone the summary is written in.
    #[must_use]
    pub fn heading_tone(self) -> Tone {
        match self {
            ToolRowState::Running => Tone::Default,
            ToolRowState::Done | ToolRowState::Failed | ToolRowState::Denied => Tone::Secondary,
            ToolRowState::Stopped => Tone::Muted,
            ToolRowState::Severe => Tone::Danger,
        }
    }

    /// Whether the summary is set in the emphasised body weight. Only [`Self::Severe`] is.
    #[must_use]
    pub fn is_severe(self) -> bool {
        matches!(self, ToolRowState::Severe)
    }

    /// Whether this state animates.
    #[must_use]
    pub fn is_running(self) -> bool {
        matches!(self, ToolRowState::Running)
    }

    /// Whether a row in this state must stay expandable even with an empty body.
    ///
    /// §B1.2: a **failed** row is always expandable, so its truncated label can be read in
    /// full; when expanded the label stops truncating and becomes selectable.
    #[must_use]
    pub fn always_expandable(self) -> bool {
        matches!(self, ToolRowState::Failed | ToolRowState::Severe)
    }
}

/// Domain-neutral properties for one 30 px native-agent tool row.
///
/// Everything is presentation: the daemon's projection has already truncated the summary and
/// the preview, mapped the harness's tool name onto a kind, and turned an exit status into the
/// `result` field. The row invents nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    /// Stable row key — the item id, so a `tool.updated` merging into a `tool.completed` keeps
    /// the same identity and the virtualizer does not remount it.
    pub id: SharedString,
    /// Lifecycle glyph state.
    pub state: ToolRowState,
    /// The glyph the row's kind draws when the state does not replace it.
    pub icon: Icon,
    /// Fixed kind-column label.
    pub kind: SharedString,
    /// One-line invocation summary.
    pub summary: SharedString,
    /// Optional right-aligned result.
    pub result: Option<SharedString>,
    /// Optional expanded body, assembled once by the projection — never in render.
    pub body: Option<SharedString>,
    /// Whether the row's body is currently exposed.
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
            icon: Icon::Wrench,
            kind: kind.into(),
            summary: summary.into(),
            result: None,
            body: None,
            expanded: false,
        }
    }

    /// Set the lifecycle state.
    #[must_use]
    pub const fn state(mut self, state: ToolRowState) -> Self {
        self.state = state;
        self
    }

    /// Set the kind glyph.
    #[must_use]
    pub const fn icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }

    /// Set the right-aligned result field.
    #[must_use]
    pub fn result(mut self, result: impl Into<SharedString>) -> Self {
        self.result = Some(result.into());
        self
    }

    /// Set the expanded body text.
    #[must_use]
    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Expand or collapse the row.
    #[must_use]
    pub const fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Whether `⏎` on this row shows anything.
    #[must_use]
    pub fn is_expandable(&self) -> bool {
        self.body.is_some() || self.state.always_expandable()
    }
}

/// The interactive tool row: focus ring, expanded body, and the nested children region a
/// subagent call draws its sub-tools in.
///
/// `key` rather than an [`ElementId`] because the row needs **three** stable ids — the line,
/// the body scroller and the row itself — and `("tool-line", key)` tuples produce them with no
/// allocation. Building them with `format!` is the exact per-frame-`String` anti-pattern
/// `gpui-performance` names.
#[derive(IntoElement)]
pub struct ToolRowElement {
    row: ToolRow,
    key: usize,
    focused: bool,
    body: Option<AnyElement>,
    children: Vec<AnyElement>,
    #[allow(clippy::type_complexity)]
    on_toggle: Option<Rc<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl ToolRowElement {
    /// A row from its presentation properties, keyed by its index in the list.
    #[must_use]
    pub fn new(row: ToolRow, key: usize) -> Self {
        Self {
            row,
            key,
            focused: false,
            body: None,
            children: Vec::new(),
            on_toggle: None,
        }
    }

    /// Draw the keyboard focus affordance.
    #[must_use]
    pub const fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Replace the default expanded body — a `DiffView`, an image, an MCP payload.
    #[must_use]
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The nested rows of a subagent call, already rendered in display order.
    #[must_use]
    pub fn children(mut self, children: Vec<AnyElement>) -> Self {
        self.children = children;
        self
    }

    /// What `⏎` (and a click on the 30 px line) does. The transcript owns the focus handle, so
    /// it is the caller that turns the key into this call; the row turns a click into it.
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
        let key = self.key;
        let expandable = row.is_expandable() || !self.children.is_empty();
        let glyph = row.state.glyph(row.icon, theme.metrics.dimmed_opacity);

        let drawn_glyph = if glyph.spinning {
            Spinner::new(("tool-glyph", key))
                .size(IconSize::Large)
                .tone(glyph.tone)
                .into_any_element()
        } else {
            let mut element = glyph.icon.el().size(IconSize::Large).tone(glyph.tone);
            if let Some(opacity) = glyph.opacity {
                element = element.opacity(opacity);
            }
            element.into_any_element()
        };

        let summary = Text::data(row.summary.clone()).tone(row.state.heading_tone());
        let summary = if row.state.is_severe() {
            summary.weight(theme.text.ui_strong.weight)
        } else {
            summary
        };
        // An expanded row stops truncating: §B1.2 makes the full label the reason a failed row
        // is expandable at all.
        let summary = if row.expanded {
            summary
        } else {
            summary.ellipsize()
        };

        let line = div()
            .id(("tool-line", key))
            .h(theme.metrics.row_h)
            .w_full()
            .flex()
            .items_center()
            .gap(theme.space.md)
            .child(div().flex_none().child(drawn_glyph))
            .child(
                Text::data_small(row.kind.clone())
                    .muted()
                    .w(AGENT_TOOL_KIND_W)
                    .flex_none()
                    .ellipsize(),
            )
            .child(div().flex().flex_1().min_w_0().child(summary))
            .children(
                row.result
                    .clone()
                    .map(|result| Text::hint(result).faint().flex_none()),
            )
            // §7: an affordance states its key. The hint is `invisible`, not absent, when the
            // row cannot expand — a row that gains a body must not shift its own alignment.
            .child(
                div()
                    .flex_none()
                    .when(!expandable, |el| el.invisible())
                    .child(expand_hint(row.expanded)),
            )
            // The click belongs to the 30 px line, never to the body it opens: clicking inside
            // an expanded diff or a nested tool must not fold the row being read.
            .when_some(self.on_toggle, |el, on_toggle| {
                el.on_click(move |_, window, cx| on_toggle(window, cx))
            });

        let body = (row.expanded && (self.body.is_some() || row.body.is_some())).then(|| {
            let content = self.body.unwrap_or_else(|| {
                row.body.clone().map_or_else(
                    || div().into_any_element(),
                    |text| Text::data_small(text).muted().into_any_element(),
                )
            });
            div()
                .id(("tool-body", key))
                .ml(theme.space.lg)
                .max_h(AGENT_BODY_MAX_H)
                .overflow_y_scroll()
                .child(content)
        });

        let nested = (row.expanded && !self.children.is_empty()).then(|| {
            div()
                .ml(theme.space.lg)
                .pl(theme.space.md)
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
            .children(body)
            .children(nested);

        div()
            .id(("tool-row", key))
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

    const DIMMED: f32 = 0.4;

    #[test]
    fn the_five_states_and_the_severity_escalation_each_draw_their_own_glyph() {
        let kind = Icon::Terminal;
        assert_eq!(
            ToolRowState::Running.glyph(kind, DIMMED),
            ToolGlyph {
                icon: Icon::LoaderCircle,
                tone: Tone::Secondary,
                opacity: None,
                spinning: true
            }
        );
        // A settled row shows what it did, not a generic tick.
        assert_eq!(ToolRowState::Done.glyph(kind, DIMMED).icon, kind);
        assert_eq!(ToolRowState::Done.glyph(kind, DIMMED).tone, Tone::Muted);
        // A routine failure keeps the kind icon, dimmed — not a red cross.
        let failed = ToolRowState::Failed.glyph(kind, DIMMED);
        assert_eq!(failed.icon, kind);
        assert_eq!(failed.tone, Tone::Danger);
        assert_eq!(failed.opacity, Some(DIMMED));
        assert_eq!(
            ToolRowState::Denied.glyph(kind, DIMMED).icon,
            Icon::CircleSlash
        );
        assert_eq!(
            ToolRowState::Stopped.glyph(kind, DIMMED).icon,
            Icon::CircleStop
        );
        assert_eq!(
            ToolRowState::Severe.glyph(kind, DIMMED).icon,
            Icon::TriangleAlert
        );
    }

    /// §5: only a runtime error or a broken side effect is red. A command that exited nonzero
    /// states `exit 1` and stays in the settled tone.
    #[test]
    fn only_the_severe_state_writes_the_summary_in_danger() {
        assert_eq!(ToolRowState::Severe.heading_tone(), Tone::Danger);
        assert!(ToolRowState::Severe.is_severe());
        for state in [
            ToolRowState::Running,
            ToolRowState::Done,
            ToolRowState::Failed,
            ToolRowState::Denied,
            ToolRowState::Stopped,
        ] {
            assert_ne!(state.heading_tone(), Tone::Danger, "{state:?} is not red");
            assert!(!state.is_severe());
        }
    }

    #[test]
    fn only_a_running_row_animates() {
        assert!(ToolRowState::Running.glyph(Icon::Eye, DIMMED).spinning);
        for state in [
            ToolRowState::Done,
            ToolRowState::Failed,
            ToolRowState::Denied,
            ToolRowState::Stopped,
            ToolRowState::Severe,
        ] {
            assert!(!state.is_running());
            assert!(!state.glyph(Icon::Eye, DIMMED).spinning);
        }
    }

    #[test]
    fn a_row_expands_when_it_has_a_body() {
        let row = ToolRow::new("t1", "bash", "cargo test");
        assert!(!row.is_expandable());
        assert!(row.clone().body("exit 0").is_expandable());
    }

    /// §B1.2: a failed row is always expandable so its truncated label can be read in full,
    /// even when the label is the only thing it carries.
    #[test]
    fn a_failed_row_expands_even_with_nothing_underneath() {
        for state in [ToolRowState::Failed, ToolRowState::Severe] {
            let row = ToolRow::new("t1", "bash", "cargo test").state(state);
            assert!(row.body.is_none());
            assert!(row.is_expandable(), "{state:?} must stay readable");
        }
        for state in [
            ToolRowState::Running,
            ToolRowState::Done,
            ToolRowState::Denied,
            ToolRowState::Stopped,
        ] {
            assert!(
                !ToolRow::new("t1", "bash", "cargo test")
                    .state(state)
                    .is_expandable()
            );
        }
    }

    #[test]
    fn builders_keep_the_id_stable() {
        let row = ToolRow::new("t1", "edit", "src/lib.rs")
            .icon(Icon::FilePen)
            .state(ToolRowState::Done)
            .result("+14 \u{2212}3")
            .expanded(true);
        assert_eq!(row.id, "t1");
        assert_eq!(row.state, ToolRowState::Done);
        assert_eq!(row.icon, Icon::FilePen);
        assert!(row.expanded);
    }
}
