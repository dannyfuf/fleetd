//! §3.8.1 Create worktree — *name a branch, pick a base, go*.

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
    dialogs::{DialogHost, field, notify, open_worktree, root, step, type_into, with_host},
    presentation::FuzzyQuery,
    state::AppState,
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
#[derive(Debug, Default)]
pub struct CreateState {
    /// The repository the worktree is created in.
    pub(crate) repo: Option<RepoId>,
    /// `origin/<defaultBranch>`, always the first and preselected base row.
    pub(crate) default_base: String,
    /// The base of the most recently created worktree in this repo, when there is one.
    pub(crate) previous_base: Option<String>,
    /// `prepare` then `postCreate`, as the footer previews them.
    pub(crate) hooks: Vec<String>,
    /// Whether the pool has a prepared copy ready for this repo.
    pub(crate) prepared_ready: bool,
    /// `local` plus every configured host; empty when no host is configured.
    pub(crate) hosts: Vec<String>,
    /// Which host the cycler shows.
    pub(crate) host_index: usize,
    /// The branch input.
    pub(crate) branch: TextFieldState,
    /// Which field owns the keyboard.
    pub(crate) field: Field,
    /// Which base row carries the cursor.
    pub(crate) base_cursor: usize,
    /// `origin/*` candidates, as last reported by the daemon.
    pub(crate) base_refs: Vec<String>,
    /// Whether a fetch is still updating the candidates.
    pub(crate) fetching: bool,
    /// The exact conflict message from a refused create.
    pub(crate) error: Option<String>,
    /// Bumps on every seed; a late answer to a superseded dialog is dropped.
    pub(crate) seq: u64,
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
        let query = FuzzyQuery::new(self.branch.text());
        let remaining = BASE_ROWS.saturating_sub(rows.len());
        let is_tail = |candidate: &&String| !rows.contains(candidate);
        let has_matches = self
            .base_refs
            .iter()
            .filter(is_tail)
            .any(|candidate| query.matches(candidate));
        let tail: Vec<_> = self
            .base_refs
            .iter()
            .filter(is_tail)
            .filter(|candidate| !has_matches || query.matches(candidate))
            .take(remaining)
            .cloned()
            .collect();
        rows.extend(tail);
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
        let slug = slugify(self.branch.text());
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
        match validate_branch(self.branch.text()) {
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
            && !slugify(self.branch.text()).is_empty()
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
    let seq = with_host(state, cx, |host| {
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
    let weak_state = state.downgrade();
    let bridge = bridge.clone();
    let task = cx.spawn(async move |cx| {
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
                let Some(state) = weak_state.upgrade() else {
                    return false;
                };
                let live = with_host(&state, cx, |host| {
                    if host.create.seq != seq {
                        return false;
                    }
                    host.create.base_refs = refs.refs;
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
    });
    crate::dialogs::retain_task(state, cx, "create-refs", task);
}

/// The branch input, carrying the first of validity, collision and id preview that applies.
fn branch_field(draft: &CreateState, duplicate: Option<&str>) -> TextField {
    let preview = draft.preview_id();
    let invalid = draft.branch_error();
    let input = field(&draft.branch)
        .label("Branch")
        .placeholder("feat/rut-validator")
        .mono(true)
        .focused(draft.field == Field::Branch);
    match (&invalid, duplicate, &preview) {
        (Some(message), _, _) => input.invalid(message.clone()),
        (None, Some(id), _) => {
            input.preview(format!("{id} already exists \u{2014} \u{23ce} opens it"))
        }
        (None, None, Some(id)) => input.preview(format!("\u{2192} {id}")),
        (None, None, None) => input,
    }
}

/// The `Base` label with its fetch spinner, over the ref candidates.
fn base_section(draft: &CreateState, tight: gpui::Pixels) -> Div {
    let candidates = draft.base_candidates();
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

    div()
        .flex()
        .flex_col()
        .gap(tight)
        .child(base_header)
        .child(base_list)
}

/// The two lines under the fields: how long a create will take, and what runs after it.
fn expectation(draft: &CreateState, cx: &App) -> Div {
    let hair = cx.theme().space.xxs;
    // §3.8.1 Icons: `zap` and `hourglass`, both 16 px Lucide strokes. A colour emoji here was
    // the one glyph on the screen that was not part of the icon set (§0), and §1.4 keeps amber
    // for "in flight / needs attention" rather than for decoration.
    let (expectation_icon, expectation_tone, expectation) = if draft.prepared_ready {
        (
            Icon::Zap,
            Tone::Warning,
            "prepared copy ready \u{2014} create takes ~2 s",
        )
    } else {
        (
            Icon::Hourglass,
            Tone::Muted,
            "no prepared copy \u{2014} the first create copies the repo (~40 s) in the background",
        )
    };
    let hooks_line = if draft.hooks.is_empty() {
        "hooks: none".to_owned()
    } else {
        format!(
            "hooks: {}  (run in background)",
            draft.hooks.join(" \u{00b7} ")
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(hair)
        .child(
            div()
                .flex()
                .items_center()
                .gap(hair)
                .child(
                    expectation_icon
                        .el()
                        .size(IconSize::Medium)
                        .color(expectation_tone.color(cx.theme())),
                )
                .child(Text::ui(expectation).muted()),
        )
        .child(Text::hint(hooks_line))
}

/// Renders the dialog (§3.8.1).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let theme = cx.theme();
        (theme.space.md, theme.space.xs)
    };
    let draft = &host.read(cx).create;
    let duplicate = draft
        .preview_id()
        .filter(|id| existing_worktree(state.read(cx), id).is_some());

    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(branch_field(draft, duplicate.as_deref()))
        .child(base_section(draft, tight))
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
        .child(expectation(draft, cx));

    let mut card = Dialog::new("New worktree")
        .icon(Icon::GitBranchPlus)
        .width(super::Dialogs::CreateWorktree.width(cx))
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

    // `left` / `right` cycle the host in this dialog, so only the editing half is shared.
    super::input::edit_actions(root(focus), state, |host| &mut host.create.branch, notify)
        .on_key_down({
            let state = state.clone();
            move |event, _window, cx| {
                let changed = with_host(&state, cx, |host| {
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
            move |_: &create_actions::HostPrev, _window, cx| {
                if move_branch_caret(&state, cx, TextFieldState::move_left) {
                    return;
                }
                cycle_host(&state, -1, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &create_actions::HostNext, _window, cx| {
                if move_branch_caret(&state, cx, TextFieldState::move_right) {
                    return;
                }
                cycle_host(&state, 1, cx);
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
            if with_host(&cancel_state, cx, |host| host.create.fetching) {
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

/// Moves the branch caret when the branch field owns the keyboard. Returns whether it did.
fn move_branch_caret(
    state: &Entity<AppState>,
    cx: &mut App,
    move_to: fn(&mut TextFieldState) -> bool,
) -> bool {
    let moved = with_host(state, cx, |host| {
        if host.create.field != Field::Branch {
            return false;
        }
        move_to(&mut host.create.branch);
        true
    });
    if moved {
        notify(state, cx);
    }
    moved
}

fn move_field(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
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
    with_host(state, cx, |host| {
        let len = host.create.base_candidates().len();
        host.create.base_cursor = step(host.create.base_cursor, delta, len);
        host.create.field = Field::Base;
    });
    notify(state, cx);
}

fn cycle_host(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
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
    let Some((repo, branch, base, host)) = with_host(state, cx, |host| {
        let draft = &host.create;
        if !draft.can_submit() {
            return None;
        }
        Some((
            draft.repo.clone()?,
            draft.branch.text().to_owned(),
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
    let bridge_handle = bridge.clone();
    super::host::complete_request(state, cx, async move |state_handle, cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let Some(state_handle) = state_handle.upgrade() else {
                return;
            };
            match answer {
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
            }
        });
    });
    close(state, cx);
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
    fn large_base_ref_lists_keep_default_previous_and_matching_cap() {
        let mut draft = draft();
        draft.base_refs = (0..10_000).map(|i| format!("origin/feature-{i}")).collect();
        draft.branch = TextFieldState::from_text("feature-99");
        let rows = draft.base_candidates();
        assert_eq!(rows.len(), BASE_ROWS);
        assert_eq!(&rows[..2], &["origin/main", "pull/412/head"]);
        assert_eq!(rows[2], "origin/feature-99");
        draft.branch = TextFieldState::from_text("no-match-at-all");
        assert_eq!(draft.base_candidates()[2], "origin/feature-0");
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
        state.branch = TextFieldState::from_text("import");
        let rows = state.base_candidates();
        assert_eq!(rows[2], "origin/feat/payroll-import");
        state.branch = TextFieldState::from_text("zzzz");
        let rows = state.base_candidates();
        assert!(
            rows.len() > 2,
            "an unmatchable branch must not hide every base"
        );
    }

    #[test]
    fn the_preview_is_the_worktree_id_the_create_will_produce() {
        let mut state = draft();
        state.branch = TextFieldState::from_text("feat/RUT validator");
        assert_eq!(
            state.preview_id().as_deref(),
            Some("buk/payroll#feat-rut-validator")
        );
    }

    #[test]
    fn validation_states_the_failing_rule_and_blocks_enter() {
        let mut state = draft();
        state.branch = TextFieldState::from_text("feat/..bad");
        assert_eq!(
            state.branch_error().as_deref(),
            Some("branch must not contain `..`")
        );
        assert!(!state.can_submit());
        state.branch = TextFieldState::from_text("feat/ok");
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
        assert!(FuzzyQuery::new("import").matches("origin/feat/payroll-import"));
        assert!(FuzzyQuery::new("").matches("origin/main"));
        assert!(!FuzzyQuery::new("z").matches("origin/main"));
        assert!(!FuzzyQuery::new("cb").matches("abc"));
    }
}
