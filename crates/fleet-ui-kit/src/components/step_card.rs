//! `StepCard` — one clickable step of a short, ordered getting-started list.
//!
//! ```text
//! [ (1)  Create a context                                          ]
//!        Group repositories by GitHub org or client, e.g. "Acme".
//! ```
//!
//! A card is a verb, so the whole card is the control: clicking it runs its action, and its
//! tooltip names that action's key from the live keymap (ADR 0023). The number says the order;
//! the step to do now is [`StepCard::current`] (an accent badge and a tinted hairline), and a
//! step that cannot run yet is [`StepCard::unavailable`], dimmed with a note at its end
//! ("after step 2") and no click. A [`StepMark::Icon`] card drawn [`StepCard::dashed`] is an
//! optional side path next to the numbered ones (an import).
//!
//! Use a [`super::Button`] for a single action, and a [`super::Row`] for a piece of data in a
//! list; this is for the two-to-four steps of an empty, first-time screen only.

use gpui::{Action, App, ElementId, SharedString, Window, div, prelude::*};

use super::{
    Kbd,
    tooltip::{Tooltip, WithTooltip},
};
use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// What leads a [`StepCard`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepMark {
    /// The step's position, in a round badge.
    Number(usize),
    /// A glyph: a side path rather than a numbered step.
    Icon(Icon),
}

type ClickFn = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// One step: a mark, a title, one line of description, and its key in the tooltip.
#[derive(IntoElement)]
pub struct StepCard {
    id: ElementId,
    mark: StepMark,
    title: SharedString,
    description: Option<SharedString>,
    note: Option<SharedString>,
    kbd: Option<Kbd>,
    action: Option<Box<dyn Action>>,
    on_click: Option<ClickFn>,
    current: bool,
    unavailable: bool,
    dashed: bool,
}

impl StepCard {
    /// A step led by `mark` and titled `title`.
    pub fn new(id: impl Into<ElementId>, mark: StepMark, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            mark,
            title: title.into(),
            description: None,
            note: None,
            kbd: None,
            action: None,
            on_click: None,
            current: false,
            unavailable: false,
            dashed: false,
        }
    }

    /// The muted line under the title.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Run `action` on click, dispatched to the focused element as its key would be, and name
    /// that key from the live keymap in the tooltip.
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(action);
        self
    }

    /// The key to name instead of the one [`StepCard::action`] resolves.
    pub fn kbd(mut self, kbd: Kbd) -> Self {
        self.kbd = Some(kbd);
        self
    }

    /// Run `handler` on click, for a step that is not an action.
    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// This is the step to do now.
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// This step cannot run yet: dimmed, not clickable, and `note` says when it can.
    pub fn unavailable(mut self, note: impl Into<SharedString>) -> Self {
        self.unavailable = true;
        self.note = Some(note.into());
        self
    }

    /// A dashed, unfilled card: an optional path beside the numbered steps.
    pub fn dashed(mut self, dashed: bool) -> Self {
        self.dashed = dashed;
        self
    }
}

impl RenderOnce for StepCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let kbd = if self.unavailable {
            None
        } else {
            self.kbd.or_else(|| {
                self.action
                    .as_deref()
                    .and_then(|action| Kbd::for_action(action, window, cx))
            })
        };
        let theme = cx.theme();
        let colors = &theme.colors;
        let current = self.current && !self.unavailable;
        let border = if current {
            colors
                .accent_fill
                .opacity(theme.metrics.banner_border_opacity)
        } else if self.dashed {
            colors.border_strong
        } else {
            colors.border
        };
        let bg = if self.dashed {
            gpui::transparent_black()
        } else if current {
            Tone::Accent.fill(theme)
        } else {
            colors.surface_raised
        };
        let hover = colors.control_hover;

        let mark = match self.mark {
            StepMark::Number(number) => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(theme.metrics.step_badge)
                .rounded(theme.radii.full)
                .bg(if current {
                    colors.accent_fill
                } else {
                    colors.control
                })
                .child(Text::ui_strong(number.to_string()).color(if current {
                    colors.accent_fill_text
                } else {
                    colors.text_secondary
                })),
            StepMark::Icon(icon) => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .child(
                    icon.el()
                        .size(IconSize::Medium)
                        .color(colors.text_secondary),
                ),
        };

        let card = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap(theme.space.md)
            .w_full()
            .px(theme.space.lg)
            .py(theme.space.md)
            .rounded(theme.radii.card)
            .bg(bg)
            .border(theme.metrics.hairline)
            .border_color(border)
            .when(self.dashed, |el| el.border_dashed())
            .when(self.unavailable, |el| {
                el.opacity(theme.metrics.dimmed_opacity)
            })
            .child(mark)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(Text::ui_strong(self.title.clone()))
                    .children(
                        self.description
                            .map(|description| Text::caption(description).tone(Tone::Muted)),
                    ),
            )
            .children(self.note.map(|note| {
                div()
                    .flex_none()
                    .child(Text::caption(note).tone(Tone::Muted))
            }));

        if self.unavailable {
            return card.into_any_element();
        }
        let card = match kbd {
            Some(kbd) => card.with_tooltip(Tooltip::new(self.title.clone()).kbd(kbd), cx),
            None => card,
        };
        let (action, on_click) = (self.action, self.on_click);
        if action.is_none() && on_click.is_none() {
            return card.into_any_element();
        }
        super::control::on_activate(
            card.hover(move |s| s.bg(hover)),
            self.title,
            move |window, cx| {
                if let Some(on_click) = &on_click {
                    on_click(window, cx);
                }
                if let Some(action) = &action {
                    window.dispatch_action(action.boxed_clone(), cx);
                }
            },
        )
        .into_any_element()
    }
}
