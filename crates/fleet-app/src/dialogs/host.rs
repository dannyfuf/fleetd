use std::collections::HashMap;

use fleet_core::{
    agents::ThreadId,
    config::Agent,
    ids::{SessionId, WorktreeId},
};
use fleet_proto::{error::ProtoError, request::RequestBody, response::ResponseBody};

use super::*;
use crate::state::{FieldSnapshot, Overlay};
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
    /// The column a column's `+` asked the next New card to land in, taken by its seed.
    ///
    /// Outside the draft on purpose: the dialog transition clears every draft before it seeds
    /// the new one, and this is the one fact that has to survive that to reach the seed.
    pub card_create_in: Option<fleet_core::ids::StatusId>,
    /// The column the next Board settings opens drilled into, taken by its seed: a column's
    /// automation pill. Outside the draft for the same reason as [`Self::card_create_in`].
    pub board_settings_column: Option<usize>,
    /// Live title editor for the open new-card dialog.
    pub(super) card_create_title: Option<Entity<TextInput>>,
    /// Live description editor for the open new-card dialog.
    pub(super) card_create_description: Option<Entity<TextInput>>,
    pub(super) card_create_input_subscriptions: Vec<Subscription>,
    /// Card detail draft (BOARD §8).
    pub card_detail: card_detail::CardDetailState,
    /// The one live editor shared by card-detail title, description and comment edits.
    pub(super) card_detail_input: Option<Entity<TextInput>>,
    pub(super) card_detail_input_subscription: Option<Subscription>,
    /// The card-picker query editor, alive for the picker's whole lifetime.
    pub(super) card_picker_input: Option<Entity<TextInput>>,
    pub(super) card_picker_input_subscription: Option<Subscription>,
    /// The input materialized for the focused board-settings text row.
    pub(super) board_settings_input: Option<Entity<TextInput>>,
    pub(super) board_settings_input_subscription: Option<Subscription>,
    /// Whether the palette's draft has been seeded for the currently open palette.
    pub palette_open: bool,
    /// The palette's query editor, alive for exactly as long as the palette is open.
    pub(super) palette_input: Option<Entity<TextInput>>,
    pub(super) palette_input_subscription: Option<Subscription>,
    pub create: create_worktree::CreateState,
    /// The branch editor, alive for the whole life of the create-worktree dialog.
    pub(super) create_branch: Option<Entity<TextInput>>,
    pub(super) create_branch_subscription: Option<Subscription>,
    pub clone: clone_repo::CloneState,
    /// The clone dialog's search editor, alive for its whole lifetime.
    pub(super) clone_query: Option<Entity<TextInput>>,
    pub(super) clone_query_subscription: Option<Subscription>,
    pub confirm: confirm::ConfirmState,
    pub context: context::ContextState,
    /// The context dialog's name and owners editors.
    pub(super) context_name: Option<Entity<TextInput>>,
    pub(super) context_owners: Option<Entity<TextInput>>,
    pub(super) context_input_subscriptions: Vec<Subscription>,
    pub assign: assign_repo::AssignState,
    /// Repository hook editor.
    pub edit_hooks: edit_hooks::EditHooksState,
    /// One editor per hook row, prepare commands first and post-create after them.
    pub(super) hook_inputs: Vec<Entity<TextInput>>,
    pub(super) hook_input_subscriptions: Vec<Subscription>,
    pub settings: settings::SettingsState,
    /// The input materialized for the settings row that entered editing.
    pub(super) settings_input: Option<Entity<TextInput>>,
    pub(super) settings_input_subscription: Option<Subscription>,
    /// The settings header's search field, alive for the dialog's whole lifetime.
    pub(super) settings_search: Option<Entity<TextInput>>,
    pub(super) settings_search_subscription: Option<Subscription>,
    /// Rename-terminal draft.
    pub rename_terminal: rename_terminal::RenameState,
    /// The rename dialog's one editor, alive for its whole lifetime.
    pub(super) rename_input: Option<Entity<TextInput>>,
    pub(super) rename_input_subscription: Option<Subscription>,
    pub palette: palette::PaletteState,
    /// Help's draft, for as long as Help is open.
    pub help: Option<help::HelpState>,
    /// Help's search field.
    pub(super) help_input: Option<Entity<TextInput>>,
    pub(super) help_input_subscription: Option<Subscription>,
    /// The Changes diff sheet's diff surface.
    pub(super) changes_diff: changes_diff::ChangesDiffState,
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

/// Leaves the one-shot prefix and opens the palette in native-thread mode.
pub(crate) fn open_agents_picker(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.leave_prefix();
        app.palette_seed = Some("!".to_owned());
        app.open_overlay(Overlay::Palette);
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

/// Opens a native agent thread: its tab when this window already shows its worktree, else its
/// worktree's session first. The palette's `AGENTS` rows and the title bar's `1 needs you` both
/// run this, so a thread opens the same way whichever of them was clicked.
pub(crate) fn open_agent_thread<T: SessionTransport>(
    thread: ThreadId,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    let reopen_transport = transport.clone();
    let worktree = if crate::screens::workspace::reopen_agent_tab(
        state,
        thread,
        move |command| reopen_transport.send(command.into()),
        cx,
    ) {
        None
    } else {
        state.update(cx, |app, _| {
            app.agents.summary(thread).and_then(|summary| {
                let worktree = summary.worktree.clone();
                // `select_agent_thread` also returns false when the combined strip is full.
                // That refusal already showed its toast and must not fall through to
                // EnsureSession, which could switch or wake an unrelated session.
                (app.agents.is_attached(thread) || app.workspace_has_tab_capacity(&worktree))
                    .then_some(worktree)
            })
        })
    };
    if let Some(worktree) = worktree {
        open_agent_thread_worktree(worktree, thread, state, transport, cx);
    }
}

/// Ensures a delegated thread's worktree session, then attaches and selects that thread.
pub(crate) fn open_agent_thread_worktree<T: SessionTransport>(
    worktree: WorktreeId,
    thread: ThreadId,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    transport.send(RequestBody::TouchWorktreeOpened {
        id: worktree.clone(),
    });
    let reply = transport.request(RequestBody::EnsureSession {
        worktree: Some(worktree.clone()),
        agent: None,
        sleep_previous: true,
    });
    let reopen_transport = transport.clone();
    complete_request(state, cx, async move |state, cx| {
        let result = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            match result {
                Ok(Ok(ResponseBody::Session(session))) => {
                    state.update(cx, |app, cx| {
                        crate::presentation::enter_session(app, session.id);
                        cx.notify();
                    });
                    crate::screens::workspace::reopen_agent_tab(
                        &state,
                        thread,
                        move |command| reopen_transport.send(command.into()),
                        cx,
                    );
                }
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
            Some(Overlay::Dialog(Dialogs::CardPicker))
        ) {
            card_picker::refresh(&state, cx);
        }
        if matches!(
            state.read(cx).overlay,
            Some(Overlay::Dialog(Dialogs::CardDetail))
        ) {
            card_detail::refresh(&state, cx);
        }
        // The schedules mirror moves under an open Board settings; its list is re-prepared
        // here, never in render.
        if matches!(
            state.read(cx).overlay,
            Some(Overlay::Dialog(Dialogs::BoardSettings))
        ) {
            board_settings::sync_open_list(&state, cx);
        }
        if matches!(
            state.read(cx).overlay,
            Some(Overlay::Dialog(Dialogs::ChangesDiff))
        ) {
            changes_diff::refresh(&state, cx);
        }
        // The branch preview names the worktree the create would produce, and whether that
        // worktree already exists is a snapshot fact: a create that landed elsewhere has to
        // turn this dialog's `Create` into `Open` without a keystroke.
        if matches!(
            state.read(cx).overlay,
            Some(Overlay::Dialog(Dialogs::CreateWorktree))
        ) {
            create_worktree::refresh_status(&state, cx);
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
        let host = host_for(&state, cx);
        sync_dialog_key_context(&state, &host, cx);
    });
    with_host(state, cx, |host| host.subscription = Some(subscription));
}

/// The context word selected by the dialog's existing keyboard-owner state.
fn dialog_key_context(dialog: &Dialogs, host: &DialogHost) -> &'static str {
    match dialog {
        Dialogs::CardDetail if host.card_detail.is_editing() => "CardDetailEditing",
        Dialogs::BoardSettings if host.board_settings_input.is_some() => "BoardSettingsEditing",
        Dialogs::Settings if host.settings.editing.is_some() => "SettingsEditing",
        Dialogs::Settings if host.settings.search_focused => "SettingsSearch",
        Dialogs::CreateWorktree if host.create.field == create_worktree::Field::Branch => {
            "CreateEditing"
        }
        _ => dialog.context_name(),
    }
}

fn focused_input_entity(state: &Entity<AppState>, cx: &mut App) -> Option<Entity<TextInput>> {
    let dialog = match state.read(cx).overlay.as_ref() {
        Some(Overlay::Dialog(dialog)) => dialog.clone(),
        // §3.9's query owns the keyboard for the whole life of the palette.
        Some(Overlay::Palette) => {
            return read_host(state, cx, |host, _| host.palette_input.clone());
        }
        _ => return None,
    };
    read_host(state, cx, |host, _| {
        let input = match dialog {
            Dialogs::CardCreate => match host.card_create.field {
                card_create::Field::Title => host.card_create_title.as_ref(),
                card_create::Field::Description => host.card_create_description.as_ref(),
            },
            Dialogs::CardDetail => host.card_detail_input.as_ref(),
            Dialogs::CardPicker => host.card_picker_input.as_ref(),
            Dialogs::BoardSettings => host.board_settings_input.as_ref(),
            // §3.8.1 gives the arrows to the host cycler while the branch field is not the
            // focused one, so the branch editor owns the keyboard only then.
            Dialogs::CreateWorktree => (host.create.field == create_worktree::Field::Branch)
                .then_some(host.create_branch.as_ref())
                .flatten(),
            Dialogs::CloneRepo => host.clone_query.as_ref(),
            Dialogs::NewContext | Dialogs::EditContext => match host.context.field {
                context::Field::Name => host.context_name.as_ref(),
                context::Field::Owners => host.context_owners.as_ref(),
            },
            Dialogs::EditHooks => host.hook_inputs.get(host.edit_hooks.field),
            Dialogs::RenameTerminal => host.rename_input.as_ref(),
            // The row editor while one is open; the header's search while it has the keyboard.
            Dialogs::Settings => host.settings_input.as_ref().or_else(|| {
                host.settings
                    .search_focused
                    .then_some(host.settings_search.as_ref())
                    .flatten()
            }),
            // Typing goes to Help's search: its keys are `↑`/`↓`/`⏎`/`esc`/`?`, none printable
            // but the last, which closes Help as it always has.
            Dialogs::Help => host.help_input.as_ref(),
            _ => None,
        }?;
        Some(input.clone())
    })
}

/// The live input that should receive focus for the current dialog or the palette.
pub(crate) fn focused_input(state: &Entity<AppState>, cx: &mut App) -> Option<FocusHandle> {
    focused_input_entity(state, cx).map(|input| input.read(cx).focus_handle())
}

/// The open dialog's live text editors, in the order `dialog.field[N]` numbers them.
///
/// Only the dialogs whose whole tab cycle is made of editors are reported, so that
/// `dialog.fields[N]` and the painted `targets["dialog.field[N]"]` always name the same field
/// (`docs/TESTING-HARNESS.md` §3). Create-worktree's base list and host cycler are not editors,
/// so that dialog reports nothing rather than a partial numbering that would not line up with
/// its targets. Board settings and Settings are the exception in the other direction: they
/// paint no `dialog.field[N]` target at all, so each reports its open rail section and its
/// editor — Board settings' row-scoped one, Settings' header search — without any numbering to
/// keep in step.
pub(crate) fn dialog_fields(state: &Entity<AppState>, cx: &mut App) -> Vec<FieldSnapshot> {
    let Some(Overlay::Dialog(dialog)) = state.read(cx).overlay.as_ref().cloned() else {
        return Vec::new();
    };
    let focused = focused_input_entity(state, cx).map(|input| input.entity_id());
    // Board settings paints no `dialog.field[N]` target of its own, so a leading non-editor row
    // here cannot put the documented field↔target numbering out of step.
    let mut fields = Vec::new();
    if dialog == Dialogs::BoardSettings {
        fields.push(FieldSnapshot {
            name: "section".to_owned(),
            value: read_host(state, cx, |host, _| {
                host.board_settings.section_title().to_owned()
            }),
            focused: false,
        });
    }
    // Settings, the same way: the section the pane shows and the row under the cursor with
    // the value the draft holds for it (`Sleep on switch = on`), then the header's search.
    if dialog == Dialogs::Settings {
        let (section, row) = read_host(state, cx, |host, _| {
            (
                host.settings.current_section().title().to_owned(),
                host.settings.cursor_summary(),
            )
        });
        fields.push(FieldSnapshot {
            name: "section".to_owned(),
            value: section,
            focused: false,
        });
        fields.push(FieldSnapshot {
            name: "row".to_owned(),
            value: row,
            focused: false,
        });
    }
    let inputs: Vec<(String, Entity<TextInput>)> = read_host(state, cx, |host, _| match dialog {
        Dialogs::CardCreate => named(&[
            ("title", host.card_create_title.as_ref()),
            ("description", host.card_create_description.as_ref()),
        ]),
        Dialogs::NewContext | Dialogs::EditContext => named(&[
            ("name", host.context_name.as_ref()),
            ("owners", host.context_owners.as_ref()),
        ]),
        Dialogs::RenameTerminal => named(&[("name", host.rename_input.as_ref())]),
        // The picker's whole cycle is its query field, and that field is the only place a
        // typed value — a model or an effort no catalogue offered — can be read back.
        Dialogs::CardPicker => named(&[("query", host.card_picker_input.as_ref())]),
        Dialogs::CloneRepo => named(&[("search", host.clone_query.as_ref())]),
        Dialogs::Help => named(&[("search", host.help_input.as_ref())]),
        Dialogs::Settings => named(&[("search", host.settings_search.as_ref())]),
        // The one row-scoped editor Board settings mounts, named after the row it belongs to.
        // It is the only place a value typed into that dialog can be read back — every other
        // row is a cycler the projection already carries — and it follows the `section` field
        // rather than a `dialog.field[N]` target, because this dialog paints none.
        Dialogs::BoardSettings => host
            .board_settings
            .editor_row_name()
            .zip(host.board_settings_input.clone())
            .into_iter()
            .collect(),
        Dialogs::EditHooks => host
            .hook_inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                let name = if index < host.edit_hooks.prepare_len {
                    "prepare"
                } else {
                    "post-create"
                };
                (name.to_owned(), input.clone())
            })
            .collect(),
        _ => Vec::new(),
    });
    fields.extend(inputs.into_iter().map(|(name, input)| FieldSnapshot {
        name,
        value: input.read(cx).text().to_owned(),
        focused: focused == Some(input.entity_id()),
    }));
    fields
}

/// The open dialog's message body, which today is the Confirm dialog's consequence sentence.
///
/// The same seam as [`dialog_fields`], and for the same reason: the draft lives on the
/// `DialogHost` entity, so the command about to answer a `dump`, an `assert` or an `await` poll
/// reads it across in an update path. Every other dialog reports `null` — its body is elements,
/// not a sentence, and naming one of them "the message" would be a claim no reader could check
/// (`docs/TESTING-HARNESS.md` §3).
pub(crate) fn dialog_message(state: &Entity<AppState>, cx: &mut App) -> Option<String> {
    if state.read(cx).overlay != Some(Overlay::Dialog(Dialogs::Confirm)) {
        return None;
    }
    read_host(state, cx, |host, _| confirm::consequence(&host.confirm))
}

/// Pairs each present editor with its field name, dropping the ones this opening never made.
fn named(fields: &[(&str, Option<&Entity<TextInput>>)]) -> Vec<(String, Entity<TextInput>)> {
    fields
        .iter()
        .filter_map(|(name, input)| Some(((*name).to_owned(), (*input)?.clone())))
        .collect()
}

#[cfg(test)]
/// How many hook rows the open editor holds, for the blank-row regression.
pub(crate) fn hook_row_count(state: &Entity<AppState>, cx: &mut App) -> usize {
    read_host(state, cx, |host, _| host.hook_inputs.len())
}

#[cfg(test)]
/// The current migrated dialog input's text, for root-level focus regressions.
pub(crate) fn focused_input_text(state: &Entity<AppState>, cx: &mut App) -> Option<String> {
    focused_input_entity(state, cx).map(|input| input.read(cx).text().to_owned())
}

/// Mirrors only the derived word into `AppState`; the dialog draft remains the source of truth.
fn sync_dialog_key_context(state: &Entity<AppState>, host: &Entity<DialogHost>, cx: &mut App) {
    let dialog = match state.read(cx).overlay.as_ref() {
        Some(Overlay::Dialog(dialog)) => Some(dialog.clone()),
        _ => None,
    };
    let context = dialog
        .as_ref()
        .map(|dialog| dialog_key_context(dialog, host.read(cx)));
    state.update(cx, |state, cx| {
        if state.set_dialog_key_context(dialog, context) {
            cx.notify();
        }
    });
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
                close_with(state, true, cx);
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
    close_with(state, false, cx);
}

fn close_with(state: &Entity<AppState>, preserve_card_detail: bool, cx: &mut App) {
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
        host.changes_diff = Default::default();
        host.assign = Default::default();
        host.edit_hooks = Default::default();
        host.rename_terminal = Default::default();
        host.palette = Default::default();
        host.help = None;
        host.help_input = None;
        host.help_input_subscription = None;
        host.card_create_title = None;
        host.card_create_description = None;
        host.card_create_input_subscriptions.clear();
        host.card_picker_input = None;
        host.card_picker_input_subscription = None;
        host.board_settings_input = None;
        host.board_settings_input_subscription = None;
        host.palette_input = None;
        host.palette_input_subscription = None;
        host.create_branch = None;
        host.create_branch_subscription = None;
        host.clone_query = None;
        host.clone_query_subscription = None;
        host.context_name = None;
        host.context_owners = None;
        host.context_input_subscriptions.clear();
        host.hook_inputs.clear();
        host.hook_input_subscriptions.clear();
        host.settings_input = None;
        host.settings_input_subscription = None;
        host.settings_search = None;
        host.settings_search_subscription = None;
        host.rename_input = None;
        host.rename_input_subscription = None;
        if !preserve_card_detail {
            host.card_detail_input = None;
            host.card_detail_input_subscription = None;
        }
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
        sync_dialog_key_context(&state, &host, cx);
        let subscriptions = vec![
            cx.on_release(|this, cx| close(&this.state, cx)),
            cx.observe(&host, |this, _, cx| {
                sync_dialog_key_context(&this.state, &this.host, cx);
                cx.notify();
            }),
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
