use super::{Shell, first_run_import_allowed, focus::focus_owner};
use crate::{
    actions::fleet,
    dialogs,
    keymap::ROOT_CONTEXT,
    presentation::{hub_jobs_key, workspace_keys},
    shell::daemon,
    state::{AppState, Overlay, Screen, ToastTarget},
};
use fleet_ui_kit::{ActiveTheme, AppFrame, Kbd, ToastStack, Veil};
use gpui::{AnyElement, App, Context, IntoElement, Render, Window, div, prelude::*};
use std::{rc::Rc, time::Instant};

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let chain = focus_owner(state);
        let overlay = state.overlay.clone();
        let agent_open = state.agent_popup.is_some();
        let splash = state
            .doctor
            .is_none()
            .then(|| daemon::splash(state, Instant::now(), &self.diagnostics))
            .flatten();
        let generation = self.prepare_key_replay(&chain, window, cx);
        // Overlays remain mounted on startup/failure surfaces with their normal key contexts.
        let overlay_element = self.render_overlay(overlay.as_ref(), window, cx);
        let (content, base_exposed) = if let Some(splash) = splash {
            (self.render_splash(splash, overlay_element, &chain), false)
        } else {
            (
                self.render_frame(overlay, overlay_element, &chain, window, cx),
                !agent_open,
            )
        };
        let pointer_gate = Self::pointer_gate(
            self.state.clone(),
            Rc::clone(&self.focus_owner_keys),
            generation,
            base_exposed,
        );
        let root = Self::with_actions(
            div()
                .relative()
                .size_full()
                .key_context(ROOT_CONTEXT)
                .bg(cx.theme().colors.bg),
            Rc::clone(&self.focus_owner_keys),
            cx,
        )
        .child(content);
        Self::after_pointer_handlers(root, pointer_gate)
    }
}

impl Shell {
    fn render_overlay(
        &mut self,
        overlay: Option<&Overlay>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = &self.state;
        let bridge = &self.bridge;
        let focus = &self.overlay_focus;
        match overlay? {
            Overlay::Jobs => Some(self.jobs.render(state, bridge, focus, window, cx)),
            Overlay::Dialog(_) | Overlay::Palette => Some(self.dialogs.clone().into_any_element()),
            // §3.10 replaces the pane header *in place*: the filter editor is drawn in the body
            // and owns the keyboard from there, so there is no overlay layer to mount and the
            // `Filter` key context wraps the body instead ("no overlay, no reflow").
            Overlay::Filter => None,
        }
    }

    fn render_splash(
        &self,
        splash: AnyElement,
        overlay: Option<AnyElement>,
        chain: &[&'static str],
    ) -> AnyElement {
        let surface = div()
            .track_focus(&self.body_focus)
            .size_full()
            .child(splash)
            .into_any_element();
        let (surface, overlay) = match overlay {
            Some(overlay) => (surface, Some(Self::overlay_contexts(chain, overlay))),
            None => (Self::contexts(chain, surface), None),
        };
        div()
            .size_full()
            .child(surface)
            .children(overlay)
            .into_any_element()
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        if state.doctor.is_some() {
            return div()
                .track_focus(&self.body_focus)
                .size_full()
                .child(
                    self.diagnostics
                        .clone()
                        .cached(gpui::StyleRefinement::default().size_full()),
                )
                .into_any_element();
        }
        if state.is_first_run() {
            let has_swarm = first_run_import_allowed(true, self.local_files.fleet_state_exists)
                && self.local_files.swarm_state_exists;
            return div()
                .track_focus(&self.body_focus)
                .size_full()
                .child(crate::views::first_run::card_with_home(
                    &state.home,
                    state
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.daemon.version.as_str()),
                    has_swarm,
                    self.user_home.as_deref(),
                    cx,
                ))
                .into_any_element();
        }
        match state.screen {
            Screen::Hub { .. } => self.hub.render(
                &mut self.board,
                &self.state,
                &self.bridge,
                &self.body_focus,
                window,
                cx,
            ),
            Screen::Workspace { .. } => {
                let veiled = state.drops_terminal_keys();
                let workspace = self.workspace.render_prepared(
                    &mut self.board,
                    &self.state,
                    &self.bridge,
                    &self.body_focus,
                    window,
                    cx,
                );
                Veil::new(veiled).content(workspace).into_any_element()
            }
        }
    }

    fn render_frame(
        &mut self,
        overlay: Option<Overlay>,
        overlay_element: Option<AnyElement>,
        chain: &[&'static str],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = self.render_body(window, cx);
        let agent = self.state.read(cx).agent_popup.is_some().then(|| {
            self.agent_popup.render_prepared(
                &self.state,
                &self.bridge,
                &self.agent_focus,
                window,
                cx,
            )
        });
        // The key-context wrappers belong to the topmost surface, so that its bindings shadow
        // everything behind it: the overlay if one is open, otherwise the popup, otherwise the
        // body. Only the last case is in flow; the other two wrap absolutely.
        let (body, agent, overlay_element) = match (agent, overlay_element) {
            (Some(agent), Some(overlay)) => (
                body,
                Some(agent),
                Some(Self::overlay_contexts(chain, overlay)),
            ),
            (Some(agent), None) => (body, Some(Self::overlay_contexts(chain, agent)), None),
            (None, Some(overlay)) => (body, None, Some(Self::overlay_contexts(chain, overlay))),
            (None, None) => (Self::contexts(chain, body), None, None),
        };
        let toasts = self.toast_stack(window, cx);
        let state = self.state.read(cx);
        let mut frame = AppFrame::new()
            .title_bar(chrome_view(&self.title_bar))
            .body(body)
            .status_bar(chrome_view(&self.status_bar))
            .body_overlay(toasts);
        if let Some(banner) = daemon::banner(
            &state.daemon,
            state.daemon_since,
            state.daemon_outdated,
            Instant::now(),
        ) {
            frame = frame.banner(banner);
        }
        if let Some(agent) = agent {
            frame = frame.overlay(agent);
        }
        if let Some(layer) = overlay_element {
            // The Jobs panel and the card detail are sheets: they dock in the band between the
            // two bars, which stay readable and reachable while they are open (UX-SPEC §3.7).
            frame = if matches!(
                overlay,
                Some(Overlay::Jobs | Overlay::Dialog(crate::dialogs::Dialogs::CardDetail))
            ) {
                frame.body_overlay(layer)
            } else {
                frame.overlay(layer)
            };
        }
        frame.into_any_element()
    }
}

impl Shell {
    /// The live toasts, wired for the pointer (§3.11): a toast that points somewhere opens it
    /// from its line or its `View` button, which shows the key that goes there too; the ✕
    /// dismisses; hovering holds the dwell. Each handler names the toast by its stable id, so a
    /// click lands on the toast it was painted for even if an older one expired in between.
    fn toast_stack(&self, window: &Window, cx: &App) -> ToastStack {
        let state = self.state.read(cx);
        let ids: Rc<[u64]> = state.toasts.iter().map(|live| live.id).collect();
        let workspace = matches!(state.screen, Screen::Workspace { .. });
        let keys = state.toasts.iter().map(|live| match live.target {
            Some(ToastTarget::Jobs) => {
                Kbd::for_action(&fleet::OpenJobs, window, cx).or_else(|| {
                    if workspace {
                        workspace_keys().jobs.clone()
                    } else {
                        hub_jobs_key()
                    }
                })
            }
            Some(ToastTarget::AgentThread(_)) | None => None,
        });
        let stack = ToastStack::new(state.toasts.iter().map(|live| live.toast.clone()))
            .bottom_inset(toast_bottom_inset(state, cx))
            .action_keys(keys.collect::<Vec<_>>());
        let (activate_state, activate_bridge, activate_ids) =
            (self.state.clone(), self.bridge.clone(), Rc::clone(&ids));
        let (dismiss_state, dismiss_ids) = (self.state.clone(), Rc::clone(&ids));
        let (hover_state, hover_ids) = (self.state.clone(), ids);
        stack
            .on_activate(move |index, window, cx| {
                let Some(&id) = activate_ids.get(index) else {
                    return;
                };
                let target = activate_state.update(cx, |state, cx| {
                    let live = state.dismiss_toast(id)?;
                    cx.notify();
                    live.target
                });
                match target {
                    Some(ToastTarget::Jobs) => {
                        window.dispatch_action(Box::new(fleet::OpenJobs), cx);
                    }
                    Some(ToastTarget::AgentThread(thread)) => {
                        dialogs::open_agent_thread(thread, &activate_state, &activate_bridge, cx);
                    }
                    None => {}
                }
            })
            .on_dismiss(move |index, _, cx| {
                let Some(&id) = dismiss_ids.get(index) else {
                    return;
                };
                dismiss_state.update(cx, |state, cx| {
                    if state.dismiss_toast(id).is_some() {
                        cx.notify();
                    }
                });
            })
            .on_hover(move |index, hovered, _, cx| {
                let Some(&id) = hover_ids.get(index) else {
                    return;
                };
                hover_state.update(cx, |state, cx| {
                    if state.hold_toast(id, hovered, Instant::now()) {
                        cx.notify();
                    }
                });
            })
    }
}

/// A bar, cached so a frame that changes nothing it draws replays its last paint.
///
/// Not while the harness records targets: a cached view replays its paint without running it,
/// so `titlebar.*`, `hub.tab[N]` and `statusbar.*` would drop out of every frame the bar did not
/// repaint in. The bar draws the same pixels either way; only the replay is skipped.
fn chrome_view(view: &gpui::Entity<crate::shell::chrome::Chrome>) -> AnyElement {
    if fleet_ui_kit::harness::is_recording() {
        view.clone().into_any_element()
    } else {
        view.clone()
            .cached(gpui::StyleRefinement::default().size_full())
            .into_any_element()
    }
}

/// How far above the body's bottom edge the toast stack starts.
///
/// §2 docks the native agent tab's composer and its 22 px metadata row at the bottom of the
/// pane, and the toast layer covers the whole body: at the default 12 px a completion toast lands
/// squarely on that metadata row and hides `$0.87 · 3m`. Everywhere else the default stands.
fn toast_bottom_inset(state: &AppState, cx: &App) -> gpui::Pixels {
    let theme = cx.theme();
    if matches!(state.screen, Screen::Workspace { .. }) && state.active_agent_thread().is_some() {
        theme.metrics.toast_inset + theme.metrics.text_field_h + theme.metrics.strip_h
    } else {
        theme.metrics.toast_inset
    }
}
