//! The title bar and the status bar: two cached views, each drawing a model prepared when
//! [`AppState`] changes.
//!
//! Render prepares nothing (APP-CONTRACTS §2): each bar keeps the model it last drew, rebuilds it
//! from the state in the observation, and repaints only when the new model differs. A terminal
//! frame, a PR poll or a cursor move that changes nothing either bar shows costs a comparison, not
//! a repaint. The model carries the input mode as well, because the key chips on the bars'
//! buttons resolve against the focused context, which follows the mode.

mod keys;
mod status;
mod title;

use fleet_ui_kit::Theme;
use gpui::{AnyElement, Entity, IntoElement, Render, Subscription, Window};

use crate::{bridge::Bridge, state::AppState};

pub(super) use status::StatusModel;
pub(super) use title::TitleModel;

/// Which bar a [`Chrome`] view draws, with the model it drew last.
enum Bar {
    Title(TitleModel),
    Status(StatusModel),
}

impl Bar {
    /// Rebuilds the model from `state`; `true` when it changed.
    fn prepare(&mut self, state: &AppState) -> bool {
        match self {
            Self::Title(model) => replace_if_changed(model, TitleModel::build(state)),
            Self::Status(model) => replace_if_changed(model, StatusModel::build(state)),
        }
    }
}

fn replace_if_changed<T: PartialEq>(current: &mut T, next: T) -> bool {
    if *current == next {
        return false;
    }
    *current = next;
    true
}

/// One of the two bars, with definite AppFrame geometry.
pub(super) struct Chrome {
    state: Entity<AppState>,
    bridge: Bridge,
    bar: Bar,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy)]
pub(super) enum ChromeKind {
    Title,
    Status,
}

impl Chrome {
    pub(super) fn new(
        state: Entity<AppState>,
        bridge: Bridge,
        kind: ChromeKind,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let mut bar = match kind {
            ChromeKind::Title => Bar::Title(TitleModel::default()),
            ChromeKind::Status => Bar::Status(StatusModel::default()),
        };
        bar.prepare(state.read(cx));
        let subscriptions = vec![
            cx.observe(&state, |chrome, state, cx| {
                if chrome.bar.prepare(state.read(cx)) {
                    cx.notify();
                }
            }),
            cx.observe_global::<Theme>(|_, cx| cx.notify()),
        ];
        Self {
            state,
            bridge,
            bar,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for Chrome {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let element: AnyElement = match &self.bar {
            Bar::Title(model) => title::render(model, &self.state, &self.bridge, cx),
            Bar::Status(model) => status::render(model, cx),
        };
        element
    }
}
