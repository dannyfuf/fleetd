use std::collections::HashMap;

use fleet_core::{
    config::Agent,
    ids::{SessionId, WorktreeId},
};
use fleet_proto::{error::ProtoError, request::RequestBody, response::ResponseBody};

use super::*;
use crate::state::Overlay;
use gpui::{
    App, Context, Entity, EntityId, FocusHandle, Global, Render, Subscription, Task, WeakEntity,
    Window,
};

/// Every dialog's mutable draft, plus the dialog the drafts were seeded for.
#[derive(Default)]
pub(crate) struct DialogHost {
    /// The dialog the drafts below belong to, or `None` when no dialog is open.
    pub open: Option<Dialogs>,
    /// The dialog the open palette replaced, so a palette command can act on its draft.
    ///
    /// The palette does not stack on the dialog it is opened over: it replaces it, and the
    /// dialog's draft is all that is left of it. Every `Card detail:` palette row saves or
    /// cancels an edit that is already typed, so it must reopen that dialog rather than seed a
    /// fresh one over the user's text.
    pub behind_palette: Option<Dialogs>,
    /// Board settings draft (BOARD §8).
    pub board_settings: board_settings::BoardSettingsState,
    /// Card property draft (BOARD §8).
    pub card_picker: card_picker::CardPickerState,
    /// New card draft (BOARD §8).
    pub card_create: card_create::CardCreateState,
    /// Card detail draft (BOARD §8).
    pub card_detail: card_detail::CardDetailState,
    /// Whether the palette's draft has been seeded for the currently open palette.
    pub palette_open: bool,
    pub create: create_worktree::CreateState,
    pub clone: clone_repo::CloneState,
    pub confirm: confirm::ConfirmState,
    pub context: context::ContextState,
    pub assign: assign_repo::AssignState,
    /// Repository hook editor.
    pub edit_hooks: edit_hooks::EditHooksState,
    pub settings: settings::SettingsState,
    /// Rename-terminal draft.
    pub rename_terminal: rename_terminal::RenameState,
    pub palette: palette::PaletteState,
    /// What the next Confirm dialog asks about, published by whoever opens it.
    pub pending_confirm: Option<ConfirmRequest>,
    /// Repository the next hook editor should load.
    pub pending_hooks_repo: Option<RepoId>,
    subscription: Option<Subscription>,
    pub(super) tasks: HashMap<&'static str, Task<()>>,
    completions: HashMap<u64, Task<()>>,
    next_completion: u64,
}

#[derive(Default)]
struct DialogRegistry {
    hosts: HashMap<EntityId, WeakEntity<DialogHost>>,
    pending_confirm: Option<ConfirmRequest>,
    pending_hooks_repo: Option<RepoId>,
}
impl Global for DialogRegistry {}

pub(crate) trait SessionTransport: Clone + 'static {
    fn send(&self, body: RequestBody);
    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, ProtoError>>;
}

impl SessionTransport for Bridge {
    fn send(&self, body: RequestBody) {
        Bridge::send(self, body);
    }

    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, ProtoError>> {
        Bridge::request(self, body)
    }
}

// One AppState per window owns one host: its release listener retains the draft entity
// until that window closes, so the registry holds only weak lookup handles.
pub(crate) fn host_for(state: &Entity<AppState>, cx: &mut App) -> Entity<DialogHost> {
    if let Some(host) = cx
        .default_global::<DialogRegistry>()
        .hosts
        .get(&state.entity_id())
        .and_then(WeakEntity::upgrade)
    {
        return host;
    }
    let host = cx.new(|_| DialogHost::default());
    let id = state.entity_id();
    cx.default_global::<DialogRegistry>()
        .hosts
        .insert(id, host.downgrade());
    let retained = host.clone();
    cx.observe_release(state, move |_, cx| {
        cx.default_global::<DialogRegistry>().hosts.remove(&id);
        drop(retained);
    })
    .detach();
    host
}

pub(crate) fn with_host<R>(
    state: &Entity<AppState>,
    cx: &mut App,
    edit: impl FnOnce(&mut DialogHost) -> R,
) -> R {
    host_for(state, cx).update(cx, |host, _| edit(host))
}

pub(crate) fn read_host<R>(
    state: &Entity<AppState>,
    cx: &mut App,
    read: impl FnOnce(&DialogHost, &App) -> R,
) -> R {
    let host = host_for(state, cx);
    read(host.read(cx), cx)
}

/// Repaints the mounted [`ActiveDialog`] after an edit to a draft.
pub(crate) fn notify(state: &Entity<AppState>, cx: &mut App) {
    palette::refresh_query(state, cx);
    host_for(state, cx).update(cx, |_, cx| cx.notify());
}

/// Stages what the next explicit Confirm opening asks about.
///
/// The Shell's callers reach this with an `App` and nothing else, so the staged request waits
/// in the registry until [`synchronize`] hands it to the window's own [`DialogHost`]; the
/// drafts themselves are never global.
pub fn request_confirm(cx: &mut App, request: ConfirmRequest) {
    cx.default_global::<DialogRegistry>().pending_confirm = Some(request);
}

pub fn request_edit_hooks(cx: &mut App, repo: RepoId) {
    cx.default_global::<DialogRegistry>().pending_hooks_repo = Some(repo);
}

pub(crate) fn retain_task(
    state: &Entity<AppState>,
    cx: &mut App,
    key: &'static str,
    task: Task<()>,
) {
    with_host(state, cx, |host| {
        host.tasks.insert(key, task);
    });
}

/// UI continuations of accepted mutations survive dialog dismissal, but not window release.
pub(crate) fn complete_request(
    state: &Entity<AppState>,
    cx: &mut App,
    complete: impl AsyncFnOnce(WeakEntity<AppState>, &mut gpui::AsyncApp) + 'static,
) {
    let host = host_for(state, cx);
    let id = host.update(cx, |host, _| {
        let id = host.next_completion;
        host.next_completion = id.wrapping_add(1);
        id
    });
    let weak_host = host.downgrade();
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        complete(weak_state, cx).await;
        cx.update(|cx| {
            cx.defer(move |cx| {
                if let Some(host) = weak_host.upgrade() {
                    host.update(cx, |host, _| {
                        host.completions.remove(&id);
                    });
                }
            })
        });
    });
    host.update(cx, |host, _| {
        host.completions.insert(id, task);
    });
}

/// Routes the window to a session, making it the most recently used one.
pub(crate) fn open_session(session: SessionId, state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        crate::presentation::enter_session(app, session);
        cx.notify();
    });
}

/// Ensures a worktree's session exists, then routes the window to it.
pub(crate) fn open_worktree<T: SessionTransport>(
    id: WorktreeId,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    ensure_worktree_session(id, true, true, state, transport, cx);
}

pub(crate) fn ensure_worktree_session<T: SessionTransport>(
    id: WorktreeId,
    touch: bool,
    sleep_previous: bool,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    if touch {
        transport.send(RequestBody::TouchWorktreeOpened { id: id.clone() });
    }
    let reply = transport.request(RequestBody::EnsureSession {
        worktree: Some(id),
        agent: None,
        sleep_previous,
    });
    finish_session_request(reply, state, cx);
}

/// Wakes a fixed agent session, then routes the window to the daemon-confirmed session.
pub(crate) fn open_agent_session<T: SessionTransport>(
    agent: Agent,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    let reply = transport.request(RequestBody::EnsureSession {
        worktree: None,
        agent: Some(agent),
        sleep_previous: true,
    });
    finish_session_request(reply, state, cx);
}

fn finish_session_request(
    reply: async_channel::Receiver<Result<ResponseBody, ProtoError>>,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    complete_request(state, cx, async move |state, cx| {
        let result = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            match result {
                Ok(Ok(ResponseBody::Session(session))) => open_session(session.id, &state, cx),
                Ok(Ok(_)) => report_session_failure(
                    &state,
                    "daemon returned an unexpected ensure-session response",
                    cx,
                ),
                Ok(Err(error)) => report_session_failure(&state, error.message, cx),
                Err(_) => {
                    report_session_failure(&state, "the Fleet daemon reply channel closed", cx)
                }
            }
        });
    });
}

fn report_session_failure(state: &Entity<AppState>, message: impl Into<String>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.sticky_error = Some(crate::state::StickyError {
            text: message.into(),
            job: None,
            retryable: false,
        });
        cx.notify();
    });
}

fn watch(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    if with_host(state, cx, |host| host.subscription.is_some()) {
        return;
    }
    let bridge = bridge.clone();
    let subscription = cx.observe(state, move |state, cx| {
        synchronize(&state, &bridge, cx);
        if matches!(state.read(cx).overlay, Some(Overlay::Palette)) {
            palette::refresh(&state, cx);
        }
        if matches!(
            state.read(cx).overlay,
            Some(Overlay::Dialog(Dialogs::Settings))
        ) && with_host(&state, cx, |host| {
            matches!(
                host.settings.current_section(),
                settings::Section::Pool | settings::Section::About
            )
        }) {
            settings::refresh_rows(&state, cx);
        }
    });
    with_host(state, cx, |host| host.subscription = Some(subscription));
}

fn synchronize(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let overlay = state.read(cx).overlay.clone();
    match overlay {
        Some(Overlay::Dialog(dialog)) => {
            let pending = cx.default_global::<DialogRegistry>();
            let replacement = (dialog == Dialogs::Confirm && pending.pending_confirm.is_some())
                || (dialog == Dialogs::EditHooks && pending.pending_hooks_repo.is_some());
            let changing =
                replacement || with_host(state, cx, |host| host.open.as_ref() != Some(&dialog));
            if changing {
                close(state, cx);
                with_host(state, cx, |host| host.behind_palette = None);
                let pending = cx.default_global::<DialogRegistry>();
                let confirm = pending.pending_confirm.take();
                let hooks = pending.pending_hooks_repo.take();
                with_host(state, cx, |host| {
                    host.pending_confirm = confirm;
                    host.pending_hooks_repo = hooks;
                });
                super::seed(&dialog, state, bridge, cx);
            }
        }
        Some(Overlay::Palette) => {
            if !with_host(state, cx, |host| host.palette_open) {
                // Read before closing: `close` is what makes the host forget which dialog the
                // palette replaced, and the `Card detail:` rows are judged against it.
                let behind = with_host(state, cx, |host| host.open.clone());
                close(state, cx);
                with_host(state, cx, |host| host.behind_palette = behind);
                palette::seed(state, cx);
            }
        }
        _ => {
            close(state, cx);
            with_host(state, cx, |host| host.behind_palette = None);
        }
    }
}

pub(crate) fn close(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        if host.open.is_none() && !host.palette_open {
            return;
        }
        host.open = None;
        host.palette_open = false;
        host.tasks.clear();
        // Counters survive closing so queued replies cannot alias the next opening.
        host.create = create_worktree::CreateState {
            seq: host.create.seq.wrapping_add(1),
            ..Default::default()
        };
        host.clone = clone_repo::CloneState {
            seq: host.clone.seq.wrapping_add(1),
            ..Default::default()
        };
        host.confirm = confirm::ConfirmState {
            seq: host.confirm.seq.wrapping_add(1),
            ..Default::default()
        };
        host.settings = settings::SettingsState {
            seq: host.settings.seq.wrapping_add(1),
            ..Default::default()
        };
        host.context = Default::default();
        host.assign = Default::default();
        host.edit_hooks = Default::default();
        host.rename_terminal = Default::default();
        host.palette = Default::default();
    });
}

/// The Shell's overlay view for dialogs and the palette.
///
/// It observes `AppState.overlay` and seeds the matching draft on every transition, so its
/// `Render` only composes what seeding prepared.
pub struct ActiveDialog {
    state: Entity<AppState>,
    bridge: Bridge,
    focus: FocusHandle,
    host: Entity<DialogHost>,
    _subscriptions: Vec<Subscription>,
}

impl ActiveDialog {
    pub fn new(
        state: Entity<AppState>,
        bridge: Bridge,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let host = host_for(&state, cx);
        watch(&state, &bridge, cx);
        synchronize(&state, &bridge, cx);
        let subscriptions = vec![
            cx.on_release(|this, cx| close(&this.state, cx)),
            cx.observe(&host, |_, _, cx| cx.notify()),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        Self {
            state,
            bridge,
            focus,
            host,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for ActiveDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.state.read(cx).overlay.clone() {
            Some(Overlay::Dialog(dialog)) => dialog.render(
                &self.state,
                &self.bridge,
                &self.focus,
                &self.host,
                window,
                cx,
            ),
            Some(Overlay::Palette) => palette::render(
                &self.state,
                &self.bridge,
                &self.focus,
                &self.host,
                window,
                cx,
            ),
            _ => gpui::div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests;
