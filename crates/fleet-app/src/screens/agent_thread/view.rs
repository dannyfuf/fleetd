//! Drawing one agent tab: the transcript column, the docked drawer, the composer.
//!
//! Every value this module composes was prepared in an update path — the rows behind their
//! revision key, the decisions behind the priority ladder, the composer's settings and the link
//! line behind its per-width fit memo — so nothing here parses, measures, notifies or mutates.
//! The measured values `render` is allowed to consult are the frame's own width and which
//! element holds the focus, because both are the geometry of the frame it is drawing
//! (`docs/APP-CONTRACTS.md`, "Render prepares nothing"). The key chips are read from the live
//! keymap, as every control's are (ADR 0023).

use fleet_core::agents::PermissionMode;
use fleet_ui_kit::{
    AGENT_CONTENT_W, ActiveTheme, Button, ButtonSize, ButtonStyle, ComposerChip, ContextMeter,
    Decision, DecisionAction, DecisionDock, DecisionKind, HarnessTargetExt, Icon, IconSize, Kbd,
    Menu, MenuItem, MetadataRow, PopoverMenu, Segment, SegmentedControl, Text, Tone,
};
use gpui::{Action, Context, Entity, SharedString, Window, div, prelude::*};

use super::{AgentThreadView, composer::InteractionMode, presentation, presentation::mode_label};
use crate::{actions::native_agent, keymap};

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
        // Under an approval the line is unmounted, not dimmed: the drawer is what you read.
        let links = (mode.metadata_shown() && !self.metadata.is_empty()).then(|| {
            let fit = self.metadata_fit.fit(
                available,
                self.metadata_rev,
                &self.metadata,
                theme.space.sm,
                theme.metrics.chip_h,
            );
            let entity = cx.entity();
            MetadataRow::new(self.metadata.clone(), fit).on_target(move |target, _window, cx| {
                // Two kinds of jump ride one callback: a bare thread id, and a card under
                // `presentation::CARD_TARGET_PREFIX`.
                if let Some(card) = target.strip_prefix(presentation::CARD_TARGET_PREFIX) {
                    let Ok(card) = card.parse() else {
                        return;
                    };
                    entity.update(cx, |_view, cx| {
                        cx.emit(super::AgentThreadEvent::SelectCard(card));
                    });
                    return;
                }
                let Ok(thread) = target.parse() else {
                    return;
                };
                entity.update(cx, |_view, cx| {
                    cx.emit(super::AgentThreadEvent::SelectThread(thread));
                });
            })
        });

        let dock = self.dock_element(window, cx);
        let docked = dock.is_some();
        let focused = !dimmed && self.input.read(cx).focus_handle().is_focused(window);
        let composer = div()
            .flex()
            .flex_col()
            .w_full()
            .flex_none()
            .opacity(composer_opacity)
            .bg(theme.colors.surface)
            .border(theme.metrics.hairline)
            .border_color(if focused {
                theme.colors.focus_ring
            } else {
                theme.colors.border_strong
            })
            .rounded_b(theme.radii.lg)
            // Under a decision the drawer is the composer's top edge, so only an undocked
            // composer rounds its own.
            .when(!docked, |el| el.rounded_t(theme.radii.lg))
            .px(theme.space.md)
            .pb(theme.space.sm)
            .child(
                // The name is on the editor's own row rather than on the stack below it: a
                // target is a thing the pointer is aimed at, and `click agents.composer` has to
                // land where a hand would put the caret. Named on the whole composer — editor
                // and settings strip — its centre falls in the strip, where the mouse down
                // never reaches the editor and the keys that follow go nowhere.
                div()
                    .flex()
                    .items_center()
                    .min_h(theme.metrics.text_field_h)
                    .w_full()
                    .child(div().flex_1().min_w_0().child(self.input.clone()))
                    .harness_target("agents.composer"),
            )
            .child(self.composer_strip(dimmed, window, cx));

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
                            .pb(theme.space.lg)
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .child(self.transcript.clone())
                                    .harness_target("agents.transcript"),
                            )
                            // The links this thread works for stay a muted line above the
                            // composer: `for [2] claude — rounding fix`, `for FLT-5 · In review`.
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.space.md)
                                    .children(links)
                                    .children(self.host_badge(&theme)),
                            )
                            .children(self.picker_element(&theme))
                            .children(dock)
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
    fn dock_element(&self, _window: &Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let head = Decision::head(&self.decisions)?.clone();
        let diff = self.decision_diff(&head);
        let entity = cx.entity();
        // The context the decision's keys are bound in, or none while a draft owns the keyboard
        // and the bare letters type into it instead (`AppState::agent_context_chain`).
        let context = (!self.is_composing(cx)).then(|| decision_key_context(&head.kind));
        let mut dock = DecisionDock::new(head)
            .on_action(move |action, _window, cx| {
                entity.update(cx, |view, cx| view.act_on_decision(&action, cx));
            })
            // Each control shows the key its action is bound to in the decision's own context,
            // so a chip can never name a key that would not do what the button does. It is read
            // from the key table rather than from the focused element: the palette or a dialog
            // taking the focus must not strip the chips and reflow the drawer under it.
            .kbd_for(move |action, _window, _cx| {
                let action = decision_action(action)?;
                keymap::keystrokes_in(context?, action.as_ref()).map(|strokes| Kbd::new(&strokes))
            });
        if let Some((header, diff)) = diff {
            dock = dock.diff(diff).diff_header(header);
        }
        // The drawer is one slot with one occupant, so one name covers an approval, a question
        // and a ready plan alike; the individual approval buttons inside it are the kit's, and
        // are named there.
        Some(dock.harness_target("agents.decision").into_any_element())
    }

    /// The diff of an edit approval and its `README.md  +1 −1` header, joined to its item **by
    /// id**, which is the only join the approval params allow: they carry no diff of their own.
    fn decision_diff(&self, decision: &Decision) -> Option<(SharedString, gpui::AnyElement)> {
        let gate = self
            .projection
            .gates
            .iter()
            .find(|gate| gate.id.to_string() == decision.id.as_ref())?;
        let fleet_core::agents::GateKind::Permission {
            item: Some(item), ..
        } = &gate.kind
        else {
            return None;
        };
        let header = self
            .projection
            .item(*item)
            .and_then(|item| match &item.kind {
                fleet_core::agents::ItemKind::Tool(call) => call.diff.as_ref(),
                _ => None,
            })
            .map(|diff| {
                SharedString::from(format!(
                    "{}  {}",
                    diff.path.display(),
                    fleet_ui_kit::format_file_delta(diff.added, diff.removed)
                ))
            })
            .unwrap_or_default();
        let item = SharedString::from(item.to_string());
        self.diffs
            .borrow()
            .get(&item)
            .map(|view| (header, view.clone().into_any_element()))
    }

    /// The composer's settings strip: model and access chips, Build/Plan, the context meter,
    /// the facts, and Send — each the pointer's way to a key the composer already has.
    fn composer_strip(
        &self,
        dimmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = cx.theme().clone();
        let controls = &self.composer_controls;
        let entity = cx.entity();

        let model = self.model_offered().then(|| {
            let label = controls
                .model
                .clone()
                .unwrap_or_else(|| SharedString::new_static("model"));
            let kbd = Kbd::for_action(&native_agent::Model, window, cx);
            let entity = entity.clone();
            PopoverMenu::new("composer-model")
                .trigger_with(move |open, _window, _cx| {
                    ComposerChip::new("composer-model-chip", label)
                        .open(open)
                        .tooltip("Switch the agent's model", kbd)
                })
                .menu(move |menu, _window, cx| entity.read(cx).model_menu(menu, &entity))
        });

        let access = {
            let label = controls.access.clone();
            let kbd = Kbd::for_action(&native_agent::AccessMode, window, cx);
            let entity = entity.clone();
            PopoverMenu::new("composer-access")
                .trigger_with(move |open, _window, _cx| {
                    ComposerChip::new("composer-access-chip", label)
                        .icon(Icon::Shield)
                        .open(open)
                        .tooltip("Change what the agent may do", kbd)
                })
                .menu(move |menu, _window, cx| entity.read(cx).access_menu(menu, &entity))
        };

        // Build | Plan: `⇧⇥` toggles, a click names the side it wants, so the side already
        // chosen is a no-op rather than a flip. The key rides on the side it would switch to.
        let interaction = controls.interaction;
        let plan_kbd = Kbd::for_action(&native_agent::PlanMode, window, cx);
        let build_plan = {
            let entity = entity.clone();
            let modes = [InteractionMode::Build, InteractionMode::Plan];
            SegmentedControl::new(
                "composer-interaction",
                modes.map(|mode| {
                    Segment::new(match mode {
                        InteractionMode::Build => "Build",
                        InteractionMode::Plan => "Plan",
                    })
                    .kbd(plan_kbd.clone().filter(|_| mode != interaction))
                }),
            )
            .active(modes.iter().position(|mode| *mode == interaction))
            .on_select(move |ix, _window, cx| {
                let Some(mode) = modes.get(ix).copied() else {
                    return;
                };
                entity.update(cx, |view, cx| {
                    if view.interaction_mode() != mode {
                        view.toggle_plan_mode(cx);
                    }
                });
            })
        };

        div()
            .flex()
            .items_center()
            .gap(theme.space.xs)
            .w_full()
            .children(model)
            .child(access)
            .child(build_plan)
            .child(div().flex_1())
            .children(
                controls
                    .context
                    .clone()
                    .map(|(pct, label)| ContextMeter::new(pct, label)),
            )
            .children(controls.facts.clone().map(|facts| {
                // The facts give way first on a narrow composer: they ellipsize, Send never
                // leaves the frame.
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .px(theme.space.sm)
                    .child(Text::hint(facts).faint().ellipsize())
            }))
            .child(self.send_button(dimmed, window, cx))
    }

    /// `Send ⏎`, or `Steer ⏎` with a draft while the agent works, or `Stop esc` without one.
    ///
    /// Each calls the very method its key's action calls, so the button and the key share one
    /// path.
    fn send_button(
        &self,
        dimmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        let working = self.is_working();
        let draft = !self.input.read(cx).is_empty(cx);
        let (label, action, style): (&'static str, Box<dyn Action>, ButtonStyle) =
            match (working, draft) {
                (true, false) => ("Stop", Box::new(native_agent::Stop), ButtonStyle::Secondary),
                (true, true) => ("Steer", Box::new(native_agent::Steer), ButtonStyle::Primary),
                (false, _) => ("Send", Box::new(native_agent::Send), ButtonStyle::Primary),
            };
        let stop = working && !draft;
        Button::new("composer-send", label)
            .style(style)
            .size(ButtonSize::Compact)
            .when(stop, |button| button.icon(Icon::CircleStop))
            .when_some(Kbd::for_action(action.as_ref(), window, cx), Button::kbd)
            // A decision owns the keyboard: sending waits until it is answered.
            .disabled(dimmed && !stop)
            .on_click(move |_, _window, cx| {
                entity.update(cx, |view, cx| {
                    if stop {
                        view.stop(cx);
                    } else {
                        view.send(cx);
                    }
                });
            })
            .harness_target("agents.send")
            .into_any_element()
    }

    /// Whether the model chip has anything to say or to offer.
    fn model_offered(&self) -> bool {
        self.composer_controls.model.is_some() || !self.projection.models.is_empty()
    }

    /// The model chip's menu: the harness's models, then its efforts for the chosen one — the
    /// same candidates `^s m` and `^s e` offer, with a check on what the next send carries.
    fn model_menu(&self, mut menu: Menu, entity: &Entity<Self>) -> Menu {
        let current = self
            .controls
            .model()
            .or(self.projection.model.as_ref())
            .cloned();
        for candidate in self.model_candidates() {
            let value = candidate.value().to_owned();
            let checked = current.as_ref().is_some_and(|model| model.model == value);
            let entity = entity.clone();
            menu = menu.item(
                MenuItem::new(candidate.label.clone())
                    .checked(checked)
                    .on_select(move |_window, cx| {
                        let value = value.clone();
                        entity.update(cx, |view, cx| view.pick_model(value, cx));
                    }),
            );
        }
        let efforts = self.trait_candidates();
        if !efforts.is_empty() {
            menu = menu.separator().header("Effort");
            for candidate in efforts {
                let value = candidate.value().to_owned();
                let checked = current
                    .as_ref()
                    .and_then(|model| model.effort.as_deref())
                    .is_some_and(|effort| effort == value);
                let entity = entity.clone();
                menu = menu.item(
                    MenuItem::new(candidate.label.clone())
                        .checked(checked)
                        .on_select(move |_window, cx| {
                            let value = value.clone();
                            entity.update(cx, |view, cx| view.pick_trait(value, cx));
                        }),
                );
            }
        }
        menu
    }

    /// The access chip's menu: the ladder the live harness declares, with a check on the one a
    /// Build send carries. Plan is not on it — it is the Build/Plan control beside the chip.
    fn access_menu(&self, mut menu: Menu, entity: &Entity<Self>) -> Menu {
        let current = self.controls.access(self.projection.mode);
        for mode in self
            .modes
            .iter()
            .copied()
            .filter(|mode| *mode != PermissionMode::Plan)
        {
            let entity = entity.clone();
            menu = menu.item(
                MenuItem::new(mode_label(mode))
                    .checked(mode == current)
                    .on_select(move |_window, cx| {
                        entity.update(cx, |view, cx| view.pick_access(mode_label(mode), cx));
                    }),
            );
        }
        menu
    }

    /// The `⛅ dev-box` badge of a remote thread, on the link line above the composer.
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
                    .child(Text::ui(row.label.clone()))
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
                        .child(Text::hint(picker.kind.title()).muted()),
                )
                .children(rows),
        )
    }
}

/// The key context a decision's keys are bound in (`keymap.rs`, `Agent > AgentDecision > *`).
fn decision_key_context(kind: &DecisionKind) -> &'static str {
    match kind {
        DecisionKind::Approval(_) => "Agent > AgentDecision > AgentPermission",
        DecisionKind::Question(_) => "Agent > AgentDecision > AgentQuestion",
        DecisionKind::PlanReady { .. } => "Agent > AgentDecision > AgentPlan",
    }
}

/// The app action a decision control shares with its key, so the control's chip is read from
/// the same binding the key dispatches through.
fn decision_action(action: &DecisionAction) -> Option<Box<dyn Action>> {
    Some(match action {
        DecisionAction::AllowOnce => Box::new(native_agent::AllowOnce),
        DecisionAction::AllowSession => Box::new(native_agent::AllowSession),
        DecisionAction::Deny => Box::new(native_agent::Deny),
        DecisionAction::DenyAndStop => Box::new(native_agent::DenyAndStop),
        DecisionAction::Edit => Box::new(native_agent::EditCommand),
        DecisionAction::Choose(0) => Box::new(native_agent::Choose1),
        DecisionAction::Choose(1) => Box::new(native_agent::Choose2),
        DecisionAction::Choose(2) => Box::new(native_agent::Choose3),
        DecisionAction::Choose(3) => Box::new(native_agent::Choose4),
        DecisionAction::Choose(4) => Box::new(native_agent::Choose5),
        DecisionAction::Choose(_) => return None,
        DecisionAction::Toggle => Box::new(native_agent::Toggle),
        DecisionAction::Answer => Box::new(native_agent::Answer),
        DecisionAction::Previous => Box::new(native_agent::Previous),
        DecisionAction::Implement => Box::new(native_agent::Implement),
        DecisionAction::Refine => Box::new(native_agent::Refine),
    })
}
