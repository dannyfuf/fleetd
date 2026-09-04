//! §3.8.1 Create worktree — *name a branch, pick a base, go*.
//!
//! The dialog never blocks: `Enter` closes it inside one frame, the daemon answers as an
//! event, and the base-ref fetch keeps running after the dialog is gone (§3.8.1 "closed while
//! a base fetch is running").

use std::time::{Duration, Instant};

use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    model::RepoHooks,
    slug::slugify,
    validate::{ValidationError, validate_branch},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{create_worktree as create_actions, dialog},
    bridge::Bridge,
    dialogs::{TextInput, notify, root, step, type_into, with_host},
    state::{AppState, Screen},
};

/// How many base rows the list shows (§3.8.1: "6 is swarm's number").
pub const BASE_ROWS: usize = 6;
/// How often the dialog re-asks the daemon while a base fetch is still running.
const FETCH_POLL: Duration = Duration::from_millis(700);
/// How many times it re-asks before giving up on the indicator.
const FETCH_POLL_LIMIT: u32 = 30;

/// Which field owns the keyboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Field {
    /// The branch input.
    #[default]
    Branch,
    /// The base list.
    Base,
    /// The host cycler.
    Host,
}

/// The Create dialog's draft.
#[derive(Debug, Clone, Default)]
pub struct CreateState {
    /// The repository the worktree is created in.
    pub repo: Option<RepoId>,
    /// `origin/<defaultBranch>`, always the first and preselected base row.
    pub default_base: String,
    /// The base of the most recently created worktree in this repo, when there is one.
    pub previous_base: Option<String>,
    /// `prepare` then `postCreate`, as the footer previews them.
    pub hooks: Vec<String>,
    /// Whether the pool has a prepared copy ready for this repo.
    pub prepared_ready: bool,
    /// `local` plus every configured host; empty when no host is configured.
    pub hosts: Vec<String>,
    /// Which host the cycler shows.
    pub host_index: usize,
    /// The branch input.
    pub branch: TextInput,
    /// Which field owns the keyboard.
    pub field: Field,
    /// Which base row carries the cursor.
    pub base_cursor: usize,
    /// `origin/*` candidates, as last reported by the daemon.
    pub base_refs: Vec<String>,
    /// Whether a fetch is still updating the candidates.
    pub fetching: bool,
    /// The exact conflict message from a refused create.
    pub error: Option<String>,
    /// Bumps on every seed; a late answer to a superseded dialog is dropped.
    pub seq: u64,
}

impl CreateState {
    /// The base rows, in the §3.8.1 order: default, previous base, then filtered `origin/*`.
    ///
    /// The `origin/*` tail is filtered by the typed branch as a subsequence match, and the
    /// filter is abandoned when it would empty the list — a base list you cannot reach is
    /// worse than one that ignores your typing.
    #[must_use]
    pub fn base_candidates(&self) -> Vec<String> {
        let mut rows: Vec<String> = Vec::with_capacity(BASE_ROWS);
        if !self.default_base.is_empty() {
            rows.push(self.default_base.clone());
        }
        if let Some(previous) = self
            .previous_base
            .as_ref()
            .filter(|previous| *previous != &self.default_base)
        {
            rows.push(previous.clone());
        }
        let query = self.branch.value().to_ascii_lowercase();
        let rest: Vec<&String> = self
            .base_refs
            .iter()
            .filter(|candidate| !rows.contains(candidate))
            .collect();
        let matching: Vec<&&String> = rest
            .iter()
            .filter(|candidate| subsequence(&candidate.to_ascii_lowercase(), &query))
            .collect();
        let tail: Vec<String> = if matching.is_empty() {
            rest.iter().map(|value| (*value).clone()).collect()
        } else {
            matching.iter().map(|value| (**value).clone()).collect()
        };
        rows.extend(tail);
        rows.truncate(BASE_ROWS);
        rows
    }

    /// The base the cursor is on.
    #[must_use]
    pub fn selected_base(&self) -> Option<String> {
        self.base_candidates().get(self.base_cursor).cloned()
    }

    /// The host the cycler shows, or `None` for `local`.
    #[must_use]
    pub fn selected_host(&self) -> Option<HostId> {
        let name = self.hosts.get(self.host_index)?;
        HostId::try_from(name.as_str()).ok()
    }

    /// The worktree id the current branch would produce.
    #[must_use]
    pub fn preview_id(&self) -> Option<String> {
        let repo = self.repo.as_ref()?;
        let slug = slugify(self.branch.value());
        if slug.is_empty() {
            return None;
        }
        Some(format!("{}#{slug}", repo.as_str()))
    }

    /// The exact failing rule, in the wording §3.8.1 asks for.
    #[must_use]
    pub fn branch_error(&self) -> Option<String> {
        if self.branch.is_empty() {
            return None;
        }
        match validate_branch(self.branch.value()) {
            Ok(()) => None,
            Err(error) => Some(reason(&error)),
        }
    }

    /// Whether `Enter` may create.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        self.repo.is_some()
            && !self.branch.is_empty()
            && self.branch_error().is_none()
            && !slugify(self.branch.value()).is_empty()
    }
}

/// The rule half of a validation error: `must not contain \`..\``.
fn reason(error: &ValidationError) -> String {
    match error {
        ValidationError::Slug { reason, .. } | ValidationError::Branch { reason, .. } => {
            format!("branch {reason}")
        }
    }
}

/// Whether `query`'s characters appear in `candidate`, in order.
fn subsequence(candidate: &str, query: &str) -> bool {
    let mut chars = candidate.chars();
    query
        .chars()
        .filter(|character| !character.is_whitespace())
        .all(|wanted| chars.any(|actual| actual == wanted))
}

// ---------------------------------------------------------------------------- seeding

/// Fills the draft from the snapshot and asks the daemon for the base refs.
pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let mut draft = CreateState::default();
    {
        let app = state.read(cx);
        draft.repo = crate::dialogs::focused_repo(app);
        if let (Some(snapshot), Some(repo)) = (app.snapshot.as_ref(), draft.repo.as_ref()) {
            if let Some(record) = snapshot.repos.iter().find(|entry| &entry.id == repo) {
                draft.default_base = format!("origin/{}", record.default_branch);
                draft.hooks = record
                    .hooks
                    .prepare
                    .iter()
                    .chain(record.hooks.post_create.iter())
                    .cloned()
                    .collect();
            }
            draft.previous_base = snapshot
                .worktrees
                .iter()
                .rfind(|worktree| &worktree.repo_id == repo)
                .map(|worktree| worktree.base_ref.clone());
            draft.prepared_ready = snapshot
                .pools
                .iter()
                .any(|pool| &pool.repo == repo && pool.ready > 0);
            if !snapshot.hosts.is_empty() {
                draft.hosts.push("local".to_owned());
                draft.hosts.extend(
                    snapshot
                        .hosts
                        .iter()
                        .map(|host| host.id.as_str().to_owned()),
                );
            }
        }
    }
    let repo = draft.repo.clone();
    draft.fetching = repo.is_some();
    let seq = with_host(cx, |host| {
        let seq = host.create.seq.wrapping_add(1);
        draft.seq = seq;
        host.create = draft;
        seq
    });
    if let Some(repo) = repo {
        poll_base_refs(repo, seq, state, bridge, cx);
    }
}

/// Asks for the base refs and keeps asking while the daemon reports a running fetch.
fn poll_base_refs(repo: RepoId, seq: u64, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let state = state.clone();
    let bridge = bridge.clone();
    cx.spawn(async move |cx| {
        for attempt in 0..FETCH_POLL_LIMIT {
            if attempt > 0 {
                cx.background_executor().timer(FETCH_POLL).await;
            }
            let reply = bridge.request(RequestBody::ListBaseRefs {
                repo: repo.clone(),
                force: false,
            });
            let Ok(answer) = reply.recv().await else {
                return;
            };
            let refs = match answer {
                Ok(ResponseBody::BaseRefs(refs)) => refs,
                _ => return,
            };
            let keep_polling = cx.update(|cx| {
                let live = with_host(cx, |host| {
                    if host.create.seq != seq {
                        return false;
                    }
                    host.create.base_refs = refs.refs.clone();
                    host.create.fetching = refs.fetching;
                    host.create.base_cursor = host
                        .create
                        .base_cursor
                        .min(host.create.base_candidates().len().saturating_sub(1));
                    true
                });
                if live {
                    notify(&state, cx);
                }
                live && refs.fetching
            });
            if !keep_polling {
                return;
            }
        }
    })
    .detach();
}

// ---------------------------------------------------------------------------- rendering

/// Renders the dialog (§3.8.1).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight, hair) = {
        let theme = cx.theme();
        (theme.space.md, theme.space.xs, theme.space.xxs)
    };
    let draft = with_host(cx, |host| host.create.clone());
    let preview = draft.preview_id();
    let duplicate = preview
        .as_ref()
        .filter(|id| existing_worktree(state.read(cx), id).is_some())
        .cloned();
    let invalid = draft.branch_error();
    let candidates = draft.base_candidates();

    let mut branch_field = TextField::new(draft.branch.value().to_owned())
        .label("Branch")
        .placeholder("feat/rut-validator")
        .mono(true)
        .caret(draft.branch.caret())
        .focused(draft.field == Field::Branch);
    branch_field = match (&invalid, &duplicate, &preview) {
        (Some(message), _, _) => branch_field.invalid(message.clone()),
        (None, Some(id), _) => {
            branch_field.preview(format!("{id} already exists \u{2014} \u{23ce} opens it"))
        }
        (None, None, Some(id)) => branch_field.preview(format!("\u{2192} {id}")),
        (None, None, None) => branch_field,
    };

    let base_header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Text::label("Base"))
        .children(
            draft
                .fetching
                .then(|| SpinnerWithLabel::new("create-base-fetch", "fetching")),
        );
    let previous_base = draft.previous_base.clone();
    let base_list = FuzzyList::new(candidates.iter().enumerate().map(|(index, candidate)| {
        let mut item = FuzzyItem::new(candidate.clone());
        if index == 0 {
            item = item.trailing("default");
        } else if previous_base.as_deref() == Some(candidate.as_str()) {
            item = item.trailing("(previous base)");
        }
        item
    }))
    .cursor(draft.base_cursor)
    .cap(BASE_ROWS)
    .under_text_field(true)
    .empty(Text::ui("No base refs yet.").muted());

    let expectation = if draft.prepared_ready {
        "\u{26a1} prepared copy ready \u{2014} create takes ~2 s"
    } else {
        "\u{29d6} no prepared copy \u{2014} the first create copies the repo (~40 s) in the background"
    };
    let hooks_line = if draft.hooks.is_empty() {
        "hooks: none".to_owned()
    } else {
        format!(
            "hooks: {}  (run in background)",
            draft.hooks.join(" \u{00b7} ")
        )
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(branch_field)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(tight)
                .child(base_header)
                .child(base_list),
        )
        .children((!draft.hosts.is_empty()).then(|| {
            Cycler::labeled(
                "Host",
                draft
                    .hosts
                    .get(draft.host_index)
                    .cloned()
                    .unwrap_or_else(|| "local".to_owned()),
            )
            .has_prev(draft.host_index > 0)
            .has_next(draft.host_index + 1 < draft.hosts.len())
            .focused(draft.field == Field::Host)
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(hair)
                .child(Text::ui(expectation).muted())
                .child(Text::hint(hooks_line)),
        );

    let mut card = Dialog::new("New worktree")
        .icon(Icon::GitBranchPlus)
        .width(super::Dialogs::CreateWorktree.width())
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key("\u{21e5}", "field")
                .key("\u{2303}n/\u{2303}p", "base")
                .key("esc", "cancel"),
        )
        .primary(if duplicate.is_some() {
            "\u{23ce} Open"
        } else {
            "\u{23ce} Create"
        });
    if let Some(repo) = draft.repo.as_ref() {
        card = card.subtitle(format!("\u{00b7} {}", repo.as_str()));
    }
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let create_state = state.clone();
    let create_bridge = bridge.clone();
    let alt_state = state.clone();
    let alt_bridge = bridge.clone();
    let cancel_state = state.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let changed = with_host(cx, |host| {
                    if host.create.field != Field::Branch {
                        return false;
                    }
                    let typed = type_into(&mut host.create.branch, event);
                    if typed {
                        host.create.error = None;
                        host.create.base_cursor = 0;
                    }
                    typed
                });
                if changed {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::NextField, _window, cx| move_field(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::PrevField, _window, cx| move_field(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _window, cx| move_base(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _window, cx| move_base(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &create_actions::HostPrev, _window, cx| cycle_host(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &create_actions::HostNext, _window, cx| cycle_host(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Backspace, _window, cx| {
                if with_host(cx, |host| host.create.branch.backspace()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                if with_host(cx, |host| host.create.branch.delete_word()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                if with_host(cx, |host| host.create.branch.clear()) {
                    notify(&state, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                with_host(cx, |host| host.create.branch.home());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                with_host(cx, |host| host.create.branch.end());
                notify(&state, cx);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            submit(true, &create_state, &create_bridge, cx);
        })
        .on_action(
            move |_: &create_actions::CreateWithoutOpening, _window, cx| {
                submit(false, &alt_state, &alt_bridge, cx);
            },
        )
        .on_action(move |_: &dialog::Cancel, _window, cx| {
            // §3.8.1: closing the dialog never cancels a running base fetch; it says so.
            if with_host(cx, |host| host.create.fetching) {
                cancel_state.update(cx, |state, cx| {
                    state.toast_short(
                        "\u{27f3} base fetch still running \u{00b7} J",
                        Icon::LoaderCircle,
                        Instant::now(),
                    );
                    cx.notify();
                });
            }
            // The shell owns closing the dialog.
            cx.propagate();
        })
        .child(card)
        .into_any_element()
}

fn move_field(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(cx, |host| {
        let has_hosts = !host.create.hosts.is_empty();
        let order: &[Field] = if has_hosts {
            &[Field::Branch, Field::Base, Field::Host]
        } else {
            &[Field::Branch, Field::Base]
        };
        let current = order
            .iter()
            .position(|field| *field == host.create.field)
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(order.len() as isize) as usize;
        host.create.field = order[next];
    });
    notify(state, cx);
}

fn move_base(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(cx, |host| {
        let len = host.create.base_candidates().len();
        host.create.base_cursor = step(host.create.base_cursor, delta, len);
        host.create.field = Field::Base;
    });
    notify(state, cx);
}

fn cycle_host(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(cx, |host| {
        let len = host.create.hosts.len();
        host.create.host_index = step(host.create.host_index, delta, len);
        if len > 0 {
            host.create.field = Field::Host;
        }
    });
    notify(state, cx);
}

/// The worktree an id already refers to, for §3.8.1's duplicate-id shortcut.
fn existing_worktree(state: &AppState, id: &str) -> Option<WorktreeId> {
    state.snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .worktrees
            .iter()
            .find(|worktree| worktree.id.as_str() == id)
            .map(|worktree| worktree.id.clone())
    })
}

/// `Enter` (`open_after`) and `⌥Enter`: create, then close inside the same frame.
fn submit(open_after: bool, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some((repo, branch, base, host)) = with_host(cx, |host| {
        let draft = &host.create;
        if !draft.can_submit() {
            return None;
        }
        Some((
            draft.repo.clone()?,
            draft.branch.value().to_owned(),
            draft.selected_base(),
            draft.selected_host(),
        ))
    }) else {
        return;
    };
    let slug = slugify(&branch);
    let id = format!("{}#{slug}", repo.as_str());
    if let Some(existing) = existing_worktree(state.read(cx), &id) {
        open_worktree(existing, state, bridge, cx);
        close(state, cx);
        return;
    }

    let hooks = state
        .read(cx)
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.repos.iter().find(|entry| entry.id == repo))
        .map_or_else(RepoHooks::default, |entry| entry.hooks.clone());
    let reply = bridge.request(RequestBody::CreateWorktree {
        repo,
        slug,
        branch: Some(branch),
        base,
        host,
        hooks,
    });
    let state_handle = state.clone();
    let bridge_handle = bridge.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(ResponseBody::Worktree { worktree, .. }) => {
                if open_after {
                    open_worktree(worktree.id, &state_handle, &bridge_handle, cx);
                }
            }
            Ok(_) => {}
            Err(error) => {
                state_handle.update(cx, |state, cx| {
                    state.sticky_error = Some(crate::state::StickyError {
                        text: error.message.clone(),
                        job: None,
                        retryable: false,
                    });
                    cx.notify();
                });
            }
        });
    })
    .detach();
    close(state, cx);
}

/// Opens a worktree's session and routes the app to it.
fn open_worktree(id: WorktreeId, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let reply = bridge.request(RequestBody::EnsureSession {
        worktree: Some(id),
        agent: None,
        sleep_previous: true,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Session(session))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            state.update(cx, |app, cx| {
                app.touch_session(session.id.clone());
                app.screen = Screen::Workspace {
                    session: session.id.clone(),
                };
                cx.notify();
            });
        });
    })
    .detach();
}

fn close(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> CreateState {
        CreateState {
            repo: RepoId::try_from("buk/payroll").ok(),
            default_base: "origin/main".to_owned(),
            previous_base: Some("pull/412/head".to_owned()),
            base_refs: vec![
                "origin/main".to_owned(),
                "origin/release-2026".to_owned(),
                "origin/feat/payroll-import".to_owned(),
            ],
            ..CreateState::default()
        }
    }

    #[test]
    fn default_base_is_first_and_previous_base_second() {
        let rows = draft().base_candidates();
        assert_eq!(rows[0], "origin/main");
        assert_eq!(rows[1], "pull/412/head");
        assert!(rows.contains(&"origin/release-2026".to_owned()));
        assert!(rows.len() <= BASE_ROWS);
    }

    #[test]
    fn base_rows_are_filtered_by_the_typed_branch_but_never_emptied() {
        let mut state = draft();
        state.branch = TextInput::new("import");
        let rows = state.base_candidates();
        assert_eq!(rows[2], "origin/feat/payroll-import");
        state.branch = TextInput::new("zzzz");
        let rows = state.base_candidates();
        assert!(
            rows.len() > 2,
            "an unmatchable branch must not hide every base"
        );
    }

    #[test]
    fn the_preview_is_the_worktree_id_the_create_will_produce() {
        let mut state = draft();
        state.branch = TextInput::new("feat/RUT validator");
        assert_eq!(
            state.preview_id().as_deref(),
            Some("buk/payroll#feat-rut-validator")
        );
    }

    #[test]
    fn validation_states_the_failing_rule_and_blocks_enter() {
        let mut state = draft();
        state.branch = TextInput::new("feat/..bad");
        assert_eq!(
            state.branch_error().as_deref(),
            Some("branch must not contain `..`")
        );
        assert!(!state.can_submit());
        state.branch = TextInput::new("feat/ok");
        assert_eq!(state.branch_error(), None);
        assert!(state.can_submit());
    }

    #[test]
    fn an_empty_branch_is_neither_invalid_nor_submittable() {
        let state = draft();
        assert_eq!(state.branch_error(), None);
        assert!(!state.can_submit());
    }

    #[test]
    fn the_host_cycler_maps_local_to_no_host() {
        let mut state = draft();
        state.hosts = vec!["local".to_owned(), "devbox".to_owned()];
        assert_eq!(state.selected_host(), None, "`local` is not a host id");
        state.host_index = 1;
        assert_eq!(
            state.selected_host().map(|host| host.as_str().to_owned()),
            Some("devbox".to_owned())
        );
    }

    #[test]
    fn subsequence_matches_in_order_only() {
        assert!(subsequence("origin/feat/payroll-import", "import"));
        assert!(subsequence("origin/main", ""));
        assert!(!subsequence("origin/main", "z"));
        assert!(!subsequence("abc", "cb"));
    }
}
