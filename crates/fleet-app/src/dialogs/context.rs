//! §3.8.4 New / Edit context.

use fleet_core::{
    ids::ContextId,
    model::{Repo, Worktree},
    sessions::{Session, SessionKind},
    slug::normalize_context_id,
};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, AppContext, Entity, FocusHandle, Window, div};

use crate::{
    actions::{context_dialog, dialog},
    bridge::Bridge,
    dialogs::{
        ConfirmRequest, DialogHost, Dialogs, footer, notify, read_host, request_confirm, root,
        with_host,
    },
    state::{AppState, Overlay},
};

/// Which field owns the keyboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Field {
    /// The name input.
    #[default]
    Name,
    /// The comma-separated owners input.
    Owners,
}

/// The New / Edit context draft.
///
/// The two editors live on the [`DialogHost`]; `name` and `owners` are the mirrors their
/// `Changed` events keep up to date, and everything below reads only those.
#[derive(Debug, Default)]
pub struct ContextState {
    /// The context being edited, or `None` when creating one.
    pub(crate) editing: Option<ContextId>,
    /// The display name.
    pub(crate) name: String,
    /// The comma-separated owner list.
    pub(crate) owners: String,
    /// Which field owns the keyboard.
    pub(crate) field: Field,
    /// Every existing context id, for the duplicate check.
    pub(crate) existing: Vec<String>,
    /// How many repositories, worktrees and sessions a delete would cascade to.
    pub(crate) cascade: (usize, usize, usize),
}

impl ContextState {
    /// The `ContextId` the typed name produces (§1 slugify rules).
    #[must_use]
    pub fn preview_id(&self) -> String {
        self.editing.as_ref().map_or_else(
            || normalize_context_id(&self.name),
            |id| id.as_str().to_owned(),
        )
    }

    /// The owners, split and trimmed the way the daemon stores them.
    #[must_use]
    pub fn owner_list(&self) -> Vec<String> {
        self.owners
            .split(',')
            .map(str::trim)
            .filter(|owner| !owner.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The duplicate-id message, when the preview collides with another context.
    #[must_use]
    pub fn duplicate(&self) -> Option<String> {
        let id = self.preview_id();
        if id.is_empty() {
            return None;
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|open| open.as_str() == id)
        {
            return None;
        }
        self.existing
            .iter()
            .any(|existing| existing == &id)
            .then(|| format!("A context with id \"{id}\" already exists."))
    }

    /// Whether `Enter` may create or save.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        !self.name.trim().is_empty()
            && !self.preview_id().is_empty()
            && self.duplicate().is_none()
            && ContextId::try_from(self.preview_id()).is_ok()
    }
}

fn cascade_counts(
    context: &ContextId,
    repos: &[Repo],
    worktrees: &[Worktree],
    sessions: &[Session],
) -> (usize, usize, usize) {
    let context_repos = repos
        .iter()
        .filter(|repo| &repo.context_id == context)
        .collect::<Vec<_>>();
    let context_worktrees = worktrees
        .iter()
        .filter(|worktree| context_repos.iter().any(|repo| repo.id == worktree.repo_id))
        .collect::<Vec<_>>();
    let session_count = sessions
        .iter()
        .filter(|session| {
            let SessionKind::Worktree(id) = &session.kind else {
                return false;
            };
            context_worktrees.iter().any(|worktree| worktree.id == *id)
        })
        .count();
    (context_repos.len(), context_worktrees.len(), session_count)
}

/// Fills the draft: empty for `N`, the active context's values for `E`.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App, editing: bool) {
    let mut draft = ContextState::default();
    {
        let app = state.read(cx);
        if let Some(snapshot) = app.snapshot.as_ref() {
            draft.existing = snapshot
                .contexts
                .iter()
                .map(|entry| entry.id.as_str().to_owned())
                .collect();
            if editing
                && let Some(context) = snapshot
                    .contexts
                    .iter()
                    .find(|entry| Some(&entry.id) == app.active_context())
            {
                draft.editing = Some(context.id.clone());
                draft.name = context.name.clone();
                draft.owners = context.owners.join(", ");
                draft.cascade = cascade_counts(
                    &context.id,
                    &snapshot.repos,
                    &snapshot.worktrees,
                    &snapshot.sessions,
                );
            }
        }
    }
    let name_text = draft.name.clone();
    let owners_text = draft.owners.clone();
    let name = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_label(Some("Name".into()), cx);
        input.set_placeholder("Buk HR", cx);
        input.set_text(name_text, cx);
        input
    });
    let owners = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_label(Some("Owners".into()), cx);
        input.set_placeholder("bukhr, dannyfuf", cx);
        input.set_preview(Some("GitHub orgs/users used to scope PRs".into()), cx);
        input.set_text(owners_text, cx);
        input
    });
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, _| {
        host.context = draft;
        host.context_name = Some(name.clone());
        host.context_owners = Some(owners.clone());
    });
    let subscriptions = vec![
        {
            let weak_state = state.downgrade();
            cx.subscribe(&name, move |input, event, cx| {
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                if matches!(event, TextInputEvent::Focused) {
                    claim_field(&state, Field::Name, cx);
                    return;
                }
                if !matches!(event, TextInputEvent::Changed) {
                    return;
                }
                let typed = input.read(cx).text().to_owned();
                with_host(&state, cx, |host| host.context.name = typed);
                sync_name_status(&state, cx);
                notify(&state, cx);
            })
        },
        {
            let weak_state = state.downgrade();
            cx.subscribe(&owners, move |input, event, cx| {
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                if matches!(event, TextInputEvent::Focused) {
                    claim_field(&state, Field::Owners, cx);
                    return;
                }
                if !matches!(event, TextInputEvent::Changed) {
                    return;
                }
                let typed = input.read(cx).text().to_owned();
                with_host(&state, cx, |host| host.context.owners = typed);
                notify(&state, cx);
            })
        },
    ];
    host.update(cx, |host, _| {
        host.context_input_subscriptions = subscriptions
    });
    sync_name_status(state, cx);
}

/// Mirrors the editor that just took focus into the marker the shell reconciles against.
///
/// `dialogs::focused_input` names the editor from this marker, and a click focuses one without
/// asking the dialog, so without this the next `AppState` notify would move the caret back.
fn claim_field(state: &Entity<AppState>, field: Field, cx: &mut App) {
    let changed = with_host(state, cx, |host| {
        let changed = host.context.field != field;
        host.context.field = field;
        changed
    });
    if changed {
        notify(state, cx);
    }
}

/// Publishes §3.8.4's collision message, or the id the typed name would produce, on the name
/// editor.
///
/// It is derived state, so it is computed here — on every `Changed` and once at seeding — and
/// never inside `render`.
fn sync_name_status(state: &Entity<AppState>, cx: &mut App) {
    let Some(input) = read_host(state, cx, |host, _| host.context_name.clone()) else {
        return;
    };
    let (duplicate, preview) = read_host(state, cx, |host, _| {
        let draft = &host.context;
        let preview_id = draft.preview_id();
        let preview = match (draft.editing.is_some(), preview_id.is_empty()) {
            // \u{00a7}1.2 zero-suppression: with no name there is no id to preview, and a bare
            // arrow with nothing after it is a dangling arrow, not information.
            (_, true) => None,
            // \u{00a7}3.8.4: once repos exist the id is read-only outright, and saying so beats
            // a disabled-looking input.
            (true, false) => Some(format!("\u{2192} {preview_id} (id is fixed)")),
            (false, false) => Some(format!("\u{2192} {preview_id}")),
        };
        (draft.duplicate(), preview)
    });
    let preview = duplicate.is_none().then_some(preview).flatten();
    // `Changed` is emitted from inside the editor's own update, so the status it derives is
    // published on the next update turn.
    cx.defer(move |cx| {
        input.update(cx, |input, cx| {
            input.set_invalid(duplicate.map(Into::into), cx);
            input.set_preview(preview.map(Into::into), cx);
        });
    });
}

/// Renders the dialog (§3.8.4).
pub(crate) fn render(
    dialog_kind: &Dialogs,
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let (editing, owners_empty, can_submit, name, owners) = {
        let draft = &host.read(cx).context;
        (
            draft.editing.clone(),
            draft.owner_list().is_empty(),
            draft.can_submit(),
            host.read(cx).context_name.clone(),
            host.read(cx).context_owners.clone(),
        )
    };
    let (Some(name), Some(owners)) = (name, owners) else {
        return root(focus).into_any_element();
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(name.harness_target_indexed("dialog.field", 0))
        .child(owners.harness_target_indexed("dialog.field", 1));

    let primary = footer::primary(
        "context-submit",
        if editing.is_some() { "Save" } else { "Create" },
        Box::new(dialog::Confirm),
    )
    .disabled(!can_submit);
    let mut card = Dialog::new(if editing.is_some() {
        "Edit context"
    } else {
        "New context"
    })
    .dismiss_action(dialog_kind.dismiss_action())
    .icon(Icon::Boxes)
    .width(dialog_kind.width(cx))
    .body(body)
    .actions(vec![footer::cancel(dialog_kind), primary]);
    if let Some(open) = editing.as_ref() {
        // §3.8.4: deleting hands off to the expanded `Y` confirm, so this is a way *to* the
        // question rather than the answer, set apart on the left as a destructive action.
        card = card.subtitle(open.as_str().to_owned()).footer_start(
            Button::new("context-delete", "Delete context")
                .style(ButtonStyle::GhostDanger)
                .icon(Icon::Trash2)
                .action(Box::new(context_dialog::Delete)),
        );
    }
    if owners_empty {
        // §3.8.4: empty owners is *allowed* and the footer warns. §2.4 keeps red for failures,
        // so a permitted configuration is amber, not an error band.
        card = card.warning("Without owners, GitHub repo search and PR \"mine\" are empty.");
    }

    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    let delete_state = state.clone();

    root(focus)
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, window, cx| toggle_field(&state, window, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, window, cx| toggle_field(&state, window, cx)
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            submit(&confirm_state, &confirm_bridge, cx);
        })
        .on_action(move |_: &context_dialog::Delete, _window, cx| {
            open_delete_confirm(&delete_state, cx);
        })
        .child(card)
        .into_any_element()
}

/// `Tab` / `S-Tab`: two fields, so both keys swap them.
fn toggle_field(state: &Entity<AppState>, window: &mut Window, cx: &mut App) {
    let input = with_host(state, cx, |host| {
        host.context.field = match host.context.field {
            Field::Name => Field::Owners,
            Field::Owners => Field::Name,
        };
        match host.context.field {
            Field::Name => host.context_name.clone(),
            Field::Owners => host.context_owners.clone(),
        }
    });
    if let Some(input) = input {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
}

/// `Enter`: create or save. A duplicate id makes it inert (§3.8.4).
fn submit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some((editing, name, owners)) = with_host(state, cx, |host| {
        if !host.context.can_submit() {
            return None;
        }
        Some((
            host.context.editing.clone(),
            host.context.name.clone(),
            host.context.owner_list(),
        ))
    }) else {
        return;
    };
    match editing {
        Some(id) => bridge.send(RequestBody::UpdateContext {
            id,
            name: Some(name),
            owners: Some(owners),
        }),
        None => bridge.send(RequestBody::CreateContext { name, owners }),
    }
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

/// `ctrl-shift-d`: hand the delete to the expanded `Y` confirm instead of doing it here.
fn open_delete_confirm(state: &Entity<AppState>, cx: &mut App) {
    let Some((context, name, cascade)) = with_host(state, cx, |host| {
        Some((
            host.context.editing.clone()?,
            host.context.name.clone(),
            host.context.cascade,
        ))
    }) else {
        return;
    };
    request_confirm(
        cx,
        ConfirmRequest::DeleteContext {
            context,
            name,
            repos: cascade.0,
            worktrees: cascade.1,
            sessions: cascade.2,
        },
    );
    state.update(cx, |app, cx| {
        app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_preview_is_the_slugified_name() {
        let mut draft = ContextState {
            name: "Buk HR".to_owned(),
            ..ContextState::default()
        };
        assert_eq!(draft.preview_id(), "buk-hr");
        draft.name = "  ".to_owned();
        assert_eq!(draft.preview_id(), "");
        assert!(!draft.can_submit());
    }

    #[test]
    fn owners_are_split_on_commas_and_trimmed() {
        let draft = ContextState {
            owners: " bukhr ,dannyfuf, ".to_owned(),
            ..ContextState::default()
        };
        assert_eq!(draft.owner_list(), vec!["bukhr", "dannyfuf"]);
    }

    #[test]
    fn a_duplicate_id_blocks_enter_but_editing_its_own_id_does_not() {
        let mut draft = ContextState {
            name: "Buk".to_owned(),
            existing: vec!["buk".to_owned()],
            ..ContextState::default()
        };
        assert_eq!(
            draft.duplicate().as_deref(),
            Some("A context with id \"buk\" already exists.")
        );
        assert!(!draft.can_submit());
        draft.editing = ContextId::try_from("buk").ok();
        assert_eq!(draft.duplicate(), None);
        assert!(draft.can_submit());
    }

    #[test]
    fn empty_owners_are_allowed() {
        let draft = ContextState {
            name: "Personal".to_owned(),
            ..ContextState::default()
        };
        assert!(draft.owner_list().is_empty());
        assert!(draft.can_submit());
    }

    #[test]
    fn editing_keeps_persisted_context_id() {
        let draft = ContextState {
            editing: ContextId::try_from("buk-hr").ok(),
            name: "People Operations".to_owned(),
            existing: vec!["buk-hr".to_owned(), "people-operations".to_owned()],
            ..ContextState::default()
        };
        assert_eq!(draft.preview_id(), "buk-hr");
        assert_eq!(draft.duplicate(), None);
        assert!(draft.can_submit());
    }

    #[test]
    fn cascade_counts_only_context_sessions() {
        use fleet_core::{
            config::Agent,
            ids::{RepoId, SessionId, WorktreeId},
            sessions::agent_session_id,
        };

        let context = ContextId::try_from("one").unwrap();
        let other_context = ContextId::try_from("two").unwrap();
        let repo = |id: &str, context_id: ContextId| Repo {
            id: RepoId::try_from(id).unwrap(),
            owner: "acme".to_owned(),
            name: id.rsplit('/').next().unwrap_or(id).to_owned(),
            url: String::new(),
            context_id,
            default_branch: "main".to_owned(),
            path: String::new(),
            cloned_at: String::new(),
            hooks: Default::default(),
        };
        let repos = vec![
            repo("acme/one", context.clone()),
            repo("acme/two", other_context),
        ];
        let worktree = |id: &str, repo_id: &str| Worktree {
            id: WorktreeId::try_from(id).unwrap(),
            repo_id: RepoId::try_from(repo_id).unwrap(),
            slug: "feature".to_owned(),
            branch: "feature".to_owned(),
            base_ref: "origin/main".to_owned(),
            path: String::new(),
            session: id.to_owned(),
            host: None,
            created_at: String::new(),
            last_opened_at: None,
            degraded: None,
        };
        let worktrees = vec![
            worktree("acme/one#feature", "acme/one"),
            worktree("acme/two#feature", "acme/two"),
        ];
        let session = |id: &str, kind| Session {
            id: SessionId::try_from(id).unwrap(),
            host: None,
            kind,
            cwd: String::new(),
            terminals: Vec::new(),
            active_terminal: None,
            slept_at: None,
            kept_terminals: Vec::new(),
        };
        let sessions = vec![
            session(
                "acme/one#feature",
                SessionKind::Worktree(worktrees[0].id.clone()),
            ),
            session(
                "acme/two#feature",
                SessionKind::Worktree(worktrees[1].id.clone()),
            ),
            session(
                agent_session_id(Agent::Claude).unwrap().as_str(),
                SessionKind::Agent(Agent::Claude),
            ),
        ];
        assert_eq!(
            cascade_counts(&context, &repos, &worktrees, &sessions),
            (1, 1, 1)
        );
    }
}
