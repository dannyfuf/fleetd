//! §3.8.4 New / Edit context.

use fleet_core::{
    ids::ContextId,
    model::{Repo, Worktree},
    sessions::{Session, SessionKind},
    slug::normalize_context_id,
};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{context_dialog, dialog},
    bridge::Bridge,
    dialogs::{
        ConfirmRequest, DialogHost, Dialogs, field, notify, request_confirm, root, type_into,
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
#[derive(Debug, Default)]
pub struct ContextState {
    /// The context being edited, or `None` when creating one.
    pub(crate) editing: Option<ContextId>,
    /// The display name.
    pub(crate) name: TextFieldState,
    /// The comma-separated owner list.
    pub(crate) owners: TextFieldState,
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
            || normalize_context_id(self.name.text()),
            |id| id.as_str().to_owned(),
        )
    }

    /// The owners, split and trimmed the way the daemon stores them.
    #[must_use]
    pub fn owner_list(&self) -> Vec<String> {
        self.owners
            .text()
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
        !self.name.text().trim().is_empty()
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
                draft.name = TextFieldState::from_text(context.name.clone());
                draft.owners = TextFieldState::from_text(context.owners.join(", "));
                draft.cascade = cascade_counts(
                    &context.id,
                    &snapshot.repos,
                    &snapshot.worktrees,
                    &snapshot.sessions,
                );
            }
        }
    }
    with_host(state, cx, |host| host.context = draft);
}

/// The name input, carrying either the collision or the id it would produce.
fn name_field(draft: &ContextState, duplicate: Option<&str>) -> TextField {
    let input = field(&draft.name)
        .label("Name")
        .placeholder("Buk HR")
        .focused(draft.field == Field::Name);
    let preview_id = draft.preview_id();
    match (duplicate, draft.editing.is_some(), preview_id.is_empty()) {
        (Some(message), _, _) => input.invalid(message.to_owned()),
        // §1.2 zero-suppression: with no name there is no id to preview, and a bare `→` with
        // nothing after it is a dangling arrow, not information.
        (None, _, true) => input,
        // §3.8.4: once repos exist the id is read-only outright, and saying so beats a
        // disabled-looking input.
        (None, true, false) => input.preview(format!("\u{2192} {preview_id} (id is fixed)")),
        (None, false, false) => input.preview(format!("\u{2192} {preview_id}")),
    }
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
    let draft = &host.read(cx).context;
    let editing = draft.editing.clone();
    let duplicate = draft.duplicate();

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(name_field(draft, duplicate.as_deref()))
        .child(
            field(&draft.owners)
                .label("Owners")
                .placeholder("bukhr, dannyfuf")
                .focused(draft.field == Field::Owners)
                .preview("GitHub orgs/users used to scope PRs"),
        );

    let mut hints = KeyHintRow::new()
        .key("\u{21e5}", "field")
        .key("esc", "cancel");
    if editing.is_some() {
        hints = hints.key("\u{2303}d", "delete context");
    }
    let mut card = Dialog::new(if editing.is_some() {
        "Edit context"
    } else {
        "New context"
    })
    .icon(Icon::Boxes)
    .width(dialog_kind.width(cx))
    .body(body)
    .hint_row(hints)
    .primary(if editing.is_some() {
        "\u{23ce} Save"
    } else {
        "\u{23ce} Create"
    });
    if let Some(open) = editing.as_ref() {
        card = card.subtitle(format!("\u{00b7} {}", open.as_str()));
    }
    if draft.owner_list().is_empty() {
        // §3.8.4: empty owners is *allowed* and the footer warns. §2.4 keeps red for failures,
        // so a permitted configuration is amber, not an error band.
        card = card.warning("Without owners, GitHub repo search and PR \"mine\" are empty.");
    }

    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();
    let delete_state = state.clone();

    super::input::actions(
        root(focus),
        state,
        |host| match host.context.field {
            Field::Name => &mut host.context.name,
            Field::Owners => &mut host.context.owners,
        },
        notify,
    )
    .on_key_down({
        let state = state.clone();
        move |event, _window, cx| {
            let changed = with_host(&state, cx, |host| match host.context.field {
                Field::Name => type_into(&mut host.context.name, event),
                Field::Owners => type_into(&mut host.context.owners, event),
            });
            if changed {
                notify(&state, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::NextField, _window, cx| toggle_field(&state, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::PrevField, _window, cx| toggle_field(&state, cx)
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

fn toggle_field(state: &Entity<AppState>, cx: &mut App) {
    with_host(state, cx, |host| {
        host.context.field = match host.context.field {
            Field::Name => Field::Owners,
            Field::Owners => Field::Name,
        };
    });
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
            host.context.name.text().to_owned(),
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

/// `ctrl-d`: hand the delete to the expanded `Y` confirm instead of doing it here.
fn open_delete_confirm(state: &Entity<AppState>, cx: &mut App) {
    let Some((context, name, cascade)) = with_host(state, cx, |host| {
        Some((
            host.context.editing.clone()?,
            host.context.name.text().to_owned(),
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
            name: TextFieldState::from_text("Buk HR"),
            ..ContextState::default()
        };
        assert_eq!(draft.preview_id(), "buk-hr");
        draft.name = TextFieldState::from_text("  ");
        assert_eq!(draft.preview_id(), "");
        assert!(!draft.can_submit());
    }

    #[test]
    fn owners_are_split_on_commas_and_trimmed() {
        let draft = ContextState {
            owners: TextFieldState::from_text(" bukhr ,dannyfuf, "),
            ..ContextState::default()
        };
        assert_eq!(draft.owner_list(), vec!["bukhr", "dannyfuf"]);
    }

    #[test]
    fn a_duplicate_id_blocks_enter_but_editing_its_own_id_does_not() {
        let mut draft = ContextState {
            name: TextFieldState::from_text("Buk"),
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
            name: TextFieldState::from_text("Personal"),
            ..ContextState::default()
        };
        assert!(draft.owner_list().is_empty());
        assert!(draft.can_submit());
    }

    #[test]
    fn editing_keeps_persisted_context_id() {
        let draft = ContextState {
            editing: ContextId::try_from("buk-hr").ok(),
            name: TextFieldState::from_text("People Operations"),
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
