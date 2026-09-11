//! Drawing one agent tab: the transcript column, the docked drawer, the composer.
//!
//! Every value this module composes was prepared in an update path — the rows behind their
//! revision key, the decisions behind the priority ladder, the metadata strip behind its
//! per-width fit memo — so nothing here parses, measures, notifies or mutates. The one
//! measured value `render` is allowed to consult is the frame's own width, because that is the
//! geometry of the frame it is drawing (`docs/APP-CONTRACTS.md`, "Render prepares nothing").

use fleet_core::agents::ItemKind;
use fleet_ui_kit::{
    AGENT_CONTENT_W, ActiveTheme, Decision, DecisionDock, Icon, IconSize, MetadataRow, Text, Tone,
};
use gpui::{Context, SharedString, Window, div, prelude::*};

use super::{AgentThreadView, presentation};

impl Render for AgentThreadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mode = self.composer_mode();
        let unreachable = self.is_unreachable();
        // §2: while a decision owns the bare keys the composer is not where you are, so it is
        // drawn dimmed — and at full strength again the moment it takes a draft back.
        let dimmed = unreachable || !mode.editor_enabled(self.question_is_choice_only());
        let composer_opacity = if dimmed {
            1.0 - theme.metrics.dimmed_opacity
        } else {
            1.0
        };

        // The fit is a memo read, keyed by `(width, revision)`. The width is the one measured
        // value render is allowed to consult, because it is the frame's own geometry.
        let available = window.viewport_size().width.min(AGENT_CONTENT_W);
        let fit = self.metadata_fit.fit(
            available,
            self.metadata_rev,
            &self.metadata,
            theme.space.sm,
            theme.metrics.chip_h,
        );
        let metadata = mode
            .metadata_shown()
            .then(|| MetadataRow::new(self.metadata.clone(), fit).trailing(self.trailing.clone()));

        let composer = div()
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .opacity(composer_opacity)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.sm)
                    .min_h(theme.metrics.text_field_h)
                    .w_full()
                    .child(div().flex_1().min_w_0().child(self.input.clone())),
            )
            .children(self.host_badge(&theme))
            .children(metadata);

        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(theme.colors.bg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .max_w(AGENT_CONTENT_W)
                            .px(theme.space.lg)
                            .child(div().flex_1().min_h_0().child(self.transcript.clone()))
                            .children(self.picker_element(&theme))
                            .children(self.dock_element(cx))
                            .child(composer),
                    ),
            )
    }
}

impl AgentThreadView {
    /// The docked decision drawer: one slot, one occupant, strict priority.
    ///
    /// It is attached to the composer's top edge rather than drawn in the transcript, because a
    /// transcript card can be scrolled out of the viewport while it still owns the keyboard.
    fn dock_element(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let head = Decision::head(&self.decisions)?.clone();
        let diff = self.decision_diff(&head);
        let entity = cx.entity();
        let mut dock = DecisionDock::new(head).on_action(move |action, _window, cx| {
            entity.update(cx, |view, cx| view.act_on_decision(&action, cx));
        });
        if let Some(diff) = diff {
            dock = dock.diff(diff);
        }
        Some(dock.into_any_element())
    }

    /// The diff of an edit approval, joined to its item **by id**, which is the only join the
    /// approval params allow: they carry no diff of their own.
    fn decision_diff(&self, decision: &Decision) -> Option<gpui::AnyElement> {
        let gate = self
            .projection
            .gates
            .iter()
            .find(|gate| gate.id.to_string() == decision.id.as_ref())?;
        let item = gate.turn.and_then(|turn| {
            self.projection
                .items
                .iter()
                .rev()
                .find(|item| {
                    item.turn == turn
                        && matches!(&item.kind, ItemKind::Tool(call) if call.diff.is_some())
                })
                .map(|item| SharedString::from(item.id.to_string()))
        })?;
        self.diffs
            .borrow()
            .get(&item)
            .map(|view| view.clone().into_any_element())
    }

    /// The `⛅ dev-box` badge of a remote thread, above the composer's metadata row.
    fn host_badge(&self, theme: &fleet_ui_kit::Theme) -> Option<gpui::Div> {
        let host = self.host()?;
        let (icon, tone) = if host.unreachable {
            (Icon::CloudOff, Tone::Warning)
        } else {
            (Icon::Cloud, Tone::Secondary)
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(theme.space.xxs)
                .child(icon.el().size(IconSize::Small).color(tone.color(theme)))
                .child(Text::hint(host.name.clone()).tone(tone))
                .children(host.unreachable.then(|| {
                    Text::hint(presentation::unreachable_hint(&host.name)).tone(Tone::Warning)
                })),
        )
    }

    /// The completion surface, drawn over the composer while one is open.
    fn picker_element(&self, theme: &fleet_ui_kit::Theme) -> Option<gpui::Div> {
        let picker = self.picker.as_ref()?;
        if picker.is_empty() {
            return None;
        }
        let highlight = picker.highlight();
        let rows: Vec<_> = picker
            .matches()
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                div()
                    .flex()
                    .items_center()
                    .h(theme.metrics.row_h)
                    .px(theme.space.sm)
                    .when(index == highlight, |el| el.bg(theme.colors.row_selected))
                    .child(Text::ui(row.to_owned()))
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .bg(theme.colors.elevated)
                .border(theme.metrics.hairline)
                .border_color(theme.colors.border_strong)
                .rounded(theme.radii.md)
                .shadow(theme.sheet_shadow())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.sm)
                        .px(theme.space.sm)
                        .child(Text::hint(picker.kind.title()).muted())
                        .child(
                            fleet_ui_kit::KeyHint::labeled("\u{23ce}", "accept").tone(Tone::Muted),
                        ),
                )
                .children(rows),
        )
    }
}
