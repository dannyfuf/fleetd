//! Media paste and file-drop behavior shared by both terminal surfaces.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use fleet_core::ids::{HostId, TerminalId};
use fleet_proto::{event::ToastLevel, request::MediaAnchor};
use fleet_ui_kit::{EmptyState, Icon, Toast, ToastDuration, Veil};
use gpui::{App, Div, Entity, ExternalPaths, SharedString, Window, div, prelude::*};

use crate::{
    bridge::Bridge,
    media::{self, Attachment},
    state::AppState,
    terminal::surface::{self, PendingInput, TerminalSurface},
};

/// Pointer-local hover state for an operating-system file drag.
///
/// This follows the board drag pattern: listeners mutate screen-owned state and refresh the
/// window only when the target changes. It does not belong in `AppState`, because no other
/// surface observes it and repainting the whole shell on every drag movement would be wasteful.
#[derive(Clone, Default)]
pub(crate) struct MediaDropState(Rc<Cell<bool>>);

impl MediaDropState {
    /// Whether this frame should draw the affordance.
    #[must_use]
    pub(crate) fn hovered(&self, cx: &App) -> bool {
        cx.has_active_drag() && self.0.get()
    }

    /// Installs the typed external-path target and consumes a completed drop.
    pub(crate) fn install(
        &self,
        root: Div,
        on_drop: impl Fn(&ExternalPaths, &mut App) + 'static,
    ) -> Div {
        let moving = self.clone();
        let dropping = self.clone();
        root.can_drop(|value, _window, _cx| value.is::<ExternalPaths>())
            .on_drag_move::<ExternalPaths>(move |event, window, _cx| {
                moving.set_hovered(event.bounds.contains(&event.event.position), window);
            })
            .on_drop::<ExternalPaths>(move |paths, window, cx| {
                dropping.set_hovered(false, window);
                on_drop(paths, cx);
                cx.stop_propagation();
            })
    }

    fn set_hovered(&self, hovered: bool, window: &mut Window) {
        if self.0.replace(hovered) != hovered {
            window.refresh();
        }
    }
}

/// Wraps terminal content in the shared, token-backed hover treatment.
pub(crate) fn drop_affordance(content: Div, label: SharedString, hovered: bool) -> Div {
    if !hovered {
        return content;
    }
    div()
        .relative()
        .size_full()
        .min_w_0()
        .min_h_0()
        .child(Veil::new(true).content(content))
        .child(div().absolute().inset_0().child(EmptyState::new(label)))
}

/// The sentence prepared for a terminal drop target.
#[must_use]
pub(crate) fn drop_label(host: Option<&HostId>) -> SharedString {
    match host {
        Some(host) => SharedString::from(format!("Drop to copy to {host}")),
        None => SharedString::new_static("Drop to paste path"),
    }
}

/// Stages one attachment for `terminal`, then routes its quoted path through the surface gate.
pub(crate) fn stage_for_terminal<S>(
    surface: &Rc<RefCell<TerminalSurface<S>>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    terminal: TerminalId,
    attachment: Attachment,
    deliver: impl FnOnce(&Entity<AppState>, PendingInput, &mut App) -> bool + 'static,
    cx: &mut App,
) {
    if surface.borrow_mut().clear_selections() {
        state.update(cx, |_, cx| cx.notify());
    }
    let delivery_state = state.downgrade();
    drop(media::stage(
        state,
        bridge,
        MediaAnchor::Terminal { terminal },
        attachment,
        false,
        move |outcomes, cx| {
            let Some(state) = delivery_state.upgrade() else {
                return;
            };
            let mut paths = Vec::new();
            let mut failures = Vec::new();
            for outcome in outcomes {
                match outcome.result {
                    Ok(path) => paths.push(path),
                    Err(message) => failures.push(message),
                }
            }
            if !failures.is_empty() {
                state.update(cx, |app, cx| {
                    app.apply_toast_event(ToastLevel::Error, failures.join("; "), Instant::now());
                    cx.notify();
                });
            }
            if !paths.is_empty() {
                finish_staged_paths(&state, terminal, paths, deliver, cx);
            }
        },
        cx,
    ));
}

fn finish_staged_paths(
    state: &Entity<AppState>,
    terminal: TerminalId,
    paths: Vec<PathBuf>,
    deliver: impl FnOnce(&Entity<AppState>, PendingInput, &mut App) -> bool,
    cx: &mut App,
) {
    if !terminal_exists(state.read(cx), terminal) {
        let count = paths.len();
        let destination = media::insert_text(&paths);
        state.update(cx, |app, cx| {
            let noun = if count == 1 { "file" } else { "files" };
            app.toast(
                Toast::new(format!("Terminal closed. Copied {noun} to {destination}"))
                    .icon(Icon::CloudUpload),
                Instant::now(),
                crate::state::dwell_for(ToastDuration::Normal),
            );
            cx.notify();
        });
        return;
    }

    let accepted = deliver(state, PendingInput::Paste(media::insert_text(&paths)), cx);
    surface::report_input_delivery(accepted, state, cx);
}

fn terminal_exists(app: &AppState, terminal: TerminalId) -> bool {
    app.snapshot.as_ref().is_some_and(|snapshot| {
        snapshot.sessions.iter().any(|session| {
            session
                .terminals
                .iter()
                .any(|candidate| candidate.id == terminal)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_name_local_and_remote_destinations() {
        assert_eq!(drop_label(None).as_ref(), "Drop to paste path");
        let host = HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(drop_label(Some(&host)).as_ref(), "Drop to copy to devbox");
    }

    #[gpui::test]
    fn a_closed_terminal_reports_where_staging_finished(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new("/tmp/fleet-media-closed", Instant::now()));
        let delivered = Rc::new(Cell::new(false));
        let observed = Rc::clone(&delivered);

        cx.update(|cx| {
            finish_staged_paths(
                &state,
                TerminalId(9),
                vec![PathBuf::from("/remote/fleet/design file.png")],
                move |_state, _input, _cx| {
                    observed.set(true);
                    true
                },
                cx,
            );
        });

        assert!(!delivered.get());
        state.read_with(cx, |app, _| {
            assert!(app.toasts.iter().any(|toast| {
                toast.toast.text.as_ref()
                    == "Terminal closed. Copied file to '/remote/fleet/design file.png'"
            }));
        });
    }
}
