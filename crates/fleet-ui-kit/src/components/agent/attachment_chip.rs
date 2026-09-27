//! Attachment pills shared by durable transcript messages and the composer's pending row.
//!
//! [`PendingAttachmentChip`] is removable and reports staging state. The transcript uses the
//! same private pill primitive without the state detail or remove affordance.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, SharedString, Window, div, prelude::*, relative,
};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

type RemoveHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// The lifecycle shown by a [`PendingAttachmentChip`].
#[derive(Clone, Debug, PartialEq)]
pub enum PendingAttachmentState {
    /// The attachment is being copied. Progress is clamped to `0.0..=1.0` when rendered.
    Staging(f32),
    /// Copying is paused while the destination reconnects.
    Waiting,
    /// The attachment is staged and will be included in the next send.
    Ready,
    /// Staging failed with the prepared user-facing message.
    Failed(SharedString),
}

/// An attachment waiting in the composer, with state and a remove affordance.
///
/// Use the transcript's [`super::UserRow`] attachments after a message has been sent; this chip
/// is for the transient staging lifecycle before dispatch.
#[derive(IntoElement)]
pub struct PendingAttachmentChip {
    id: ElementId,
    name: SharedString,
    state: PendingAttachmentState,
    on_remove: Option<RemoveHandler>,
}

impl PendingAttachmentChip {
    /// A pending attachment named `name` in `state`.
    pub fn new(
        id: impl Into<ElementId>,
        name: impl Into<SharedString>,
        state: PendingAttachmentState,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            state,
            on_remove: None,
        }
    }

    /// Handle a click on the chip's remove affordance.
    pub fn on_remove(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_remove = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for PendingAttachmentChip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, tone, spinning, detail, progress) = match self.state {
            PendingAttachmentState::Staging(progress) => {
                let progress = progress.clamp(0.0, 1.0);
                (
                    Icon::LoaderCircle,
                    Tone::Warning,
                    true,
                    Some(SharedString::from(format!("{:.0}%", progress * 100.0))),
                    Some(progress),
                )
            }
            PendingAttachmentState::Waiting => (
                Icon::Unplug,
                Tone::Warning,
                false,
                Some(SharedString::new_static("waiting")),
                None,
            ),
            PendingAttachmentState::Ready => (Icon::Paperclip, Tone::Secondary, false, None, None),
            PendingAttachmentState::Failed(message) => (
                Icon::TriangleAlert,
                Tone::Danger,
                false,
                Some(message),
                None,
            ),
        };
        let spinner_id = ElementId::NamedChild(
            Arc::new(self.id.clone()),
            SharedString::new_static("spinner"),
        );
        let remove_id =
            ElementId::NamedChild(Arc::new(self.id), SharedString::new_static("remove"));
        let remove_label = SharedString::new_static("Remove attachment");
        let hover = theme.colors.control_hover;
        let active = theme.colors.control_active;
        let remove = self.on_remove.map(|handler| {
            div()
                .id(remove_id)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(theme.metrics.kbd_h_small)
                .rounded(theme.radii.sm)
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(active))
                .role(gpui::Role::Button)
                .aria_label(remove_label)
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event, window, cx);
                })
                .child(Icon::X.el().size(IconSize::Small).tone(Tone::Muted))
                .into_any_element()
        });

        AttachmentPill::new(self.name)
            .icon(icon)
            .tone(tone)
            .spinning(spinning, spinner_id)
            .detail(detail)
            .progress(progress)
            .trailing(remove)
    }
}

/// The shared attachment pill visual. Interactivity stays in the pending chip's trailing slot.
#[derive(IntoElement)]
pub(super) struct AttachmentPill {
    name: SharedString,
    icon: Icon,
    tone: Tone,
    icon_tone: Tone,
    spinning: bool,
    spinner_id: Option<ElementId>,
    detail: Option<SharedString>,
    progress: Option<f32>,
    trailing: Option<AnyElement>,
}

impl AttachmentPill {
    pub(super) fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            icon: Icon::Paperclip,
            tone: Tone::Secondary,
            icon_tone: Tone::Muted,
            spinning: false,
            spinner_id: None,
            detail: None,
            progress: None,
            trailing: None,
        }
    }

    fn icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }

    fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self.icon_tone = tone;
        self
    }

    fn spinning(mut self, spinning: bool, id: ElementId) -> Self {
        self.spinning = spinning;
        self.spinner_id = Some(id);
        self
    }

    fn detail(mut self, detail: Option<SharedString>) -> Self {
        self.detail = detail;
        self
    }

    fn progress(mut self, progress: Option<f32>) -> Self {
        self.progress = progress;
        self
    }

    fn trailing(mut self, trailing: Option<AnyElement>) -> Self {
        self.trailing = trailing;
        self
    }
}

impl RenderOnce for AttachmentPill {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = self.tone;
        let icon = self
            .icon
            .el()
            .size(IconSize::Small)
            .tone(self.icon_tone)
            .spinning(self.spinning)
            .when_some(self.spinner_id, |icon, id| icon.id(id));
        let progress = self.progress.map(|progress| {
            div()
                .absolute()
                .inset_0()
                .w(relative(progress))
                .bg(Tone::Warning
                    .color(theme)
                    .opacity(theme.metrics.banner_border_opacity))
        });

        div()
            .relative()
            .flex()
            .flex_none()
            .max_w_full()
            .min_w_0()
            .h(theme.metrics.chip_h)
            .rounded(theme.radii.sm)
            .overflow_hidden()
            .bg(tone.fill(theme))
            .children(progress)
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(theme.space.xxs)
                    .size_full()
                    .min_w_0()
                    .px(theme.space.sm)
                    .child(icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(Text::hint(self.name).tone(tone).ellipsize()),
                    )
                    .children(self.detail.map(|detail| {
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .child(Text::hint(detail).tone(tone).ellipsize())
                    }))
                    .children(self.trailing),
            )
    }
}
