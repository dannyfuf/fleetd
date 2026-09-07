use super::{Shell, first_run_import_allowed, focus::focus_owner};
use crate::{
    keymap::ROOT_CONTEXT,
    shell::daemon,
    state::{Overlay, Screen},
};
use fleet_ui_kit::{ActiveTheme, AppFrame, ToastStack, Veil};
use gpui::{AnyElement, Context, IntoElement, Render, Window, div, prelude::*};
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
        Some(match overlay? {
            Overlay::Jobs => self.jobs.render(state, bridge, focus, window, cx),
            Overlay::Dialog(_) | Overlay::Palette => self.dialogs.clone().into_any_element(),
            Overlay::Filter => crate::dialogs::filter::render(state, bridge, focus, window, cx),
        })
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
            Screen::Hub { .. } => {
                self.hub
                    .render(&self.state, &self.bridge, &self.body_focus, window, cx)
            }
            Screen::Workspace { .. } => {
                let veiled = state.drops_terminal_keys();
                let workspace = self.workspace.render_prepared(
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
        let state = self.state.read(cx);
        let mut frame = AppFrame::new()
            .context_bar(
                self.context_bar
                    .clone()
                    .cached(gpui::StyleRefinement::default().size_full()),
            )
            .body(body)
            .status_bar(
                self.status_bar
                    .clone()
                    .cached(gpui::StyleRefinement::default().size_full()),
            )
            .body_overlay(ToastStack::new(
                state.toasts.iter().map(|live| live.toast.clone()),
            ));
        if let Some(banner) = daemon::banner(&state.daemon, state.daemon_outdated, Instant::now()) {
            frame = frame.banner(banner);
        }
        if let Some(agent) = agent {
            frame = frame.overlay(agent);
        }
        if let Some(layer) = overlay_element {
            frame = if matches!(overlay, Some(Overlay::Jobs)) {
                frame.body_overlay(layer)
            } else {
                frame.overlay(layer)
            };
        }
        frame.into_any_element()
    }
}
