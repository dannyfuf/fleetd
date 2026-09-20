//! §3.8.1 Create worktree — *name a branch, pick a base, go*.

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    model::RepoHooks,
    slug::slugify,
    validate::{ValidationError, validate_branch},
};
use fleet_proto::{
    request::RequestBody,
    response::ResponseBody,
    snapshot::{HostStatus, LinkState},
};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, AppContext, Entity, FocusHandle, Window, div};

use crate::{
    actions::{create_worktree as create_actions, dialog},
    async_util::before_timeout,
    bridge::Bridge,
    dialogs::{DialogHost, notify, open_session, read_host, root, step, with_host},
    presentation::FuzzyQuery,
    state::{AppState, Cursors, HubPane, RepoScope, Screen},
};

mod branch;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use branch::refresh_status;
pub(crate) use view::render;

/// How many base rows the list shows (§3.8.1: "6 is swarm's number").
pub const BASE_ROWS: usize = 6;
/// Maximum time either base-ref request may leave the dialog's spinner active.
const BASE_REF_TIMEOUT: Duration = Duration::from_secs(15);

type Reply = async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>;

trait CreateTransport: Clone + 'static {
    fn send(&self, body: RequestBody);
    fn request(&self, body: RequestBody) -> Reply;
}

impl CreateTransport for Bridge {
    fn send(&self, body: RequestBody) {
        Bridge::send(self, body);
    }

    fn request(&self, body: RequestBody) -> Reply {
        Bridge::request(self, body)
    }
}

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

/// One entry of the host cycler: the local machine, then every configured host.
///
/// The daemon is the only source of truth for whether a host can be created on, so the entry
/// carries the refusal reason it reported rather than any local-path assumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostChoice {
    /// The host id, or `None` for `local`.
    pub id: Option<HostId>,
    /// What the cycler shows.
    pub label: String,
    /// The machine provider (`tailscale`, `command`, `legacy`), absent for `local`.
    pub provider: Option<String>,
    /// Why a create on this host would be refused, when it would be.
    pub blocked: Option<String>,
}

impl HostChoice {
    /// The always-present first entry.
    #[must_use]
    pub fn local() -> Self {
        Self {
            id: None,
            label: "local".to_owned(),
            provider: None,
            blocked: None,
        }
    }

    /// A configured host as its last reported status describes it.
    #[must_use]
    pub fn from_status(status: &HostStatus) -> Self {
        Self {
            id: Some(status.id.clone()),
            label: status.id.as_str().to_owned(),
            provider: (!status.provider.is_empty()).then(|| status.provider.clone()),
            blocked: host_blocker(status),
        }
    }
}

/// Why this host cannot take a create, in the daemon's own words where it has them.
///
/// A legacy entry has no daemon to create on at all; an unreachable host and a dropped daemon
/// link are the same refusal to the user.
#[must_use]
fn host_blocker(status: &HostStatus) -> Option<String> {
    if status.link == LinkState::Legacy {
        return Some("legacy entry \u{2014} migrate it to a tailscale host".to_owned());
    }
    if status.reachable && status.link != LinkState::Down {
        return None;
    }
    Some(
        status
            .error
            .clone()
            .filter(|error| !error.is_empty())
            .unwrap_or_else(|| "unreachable".to_owned()),
    )
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
    pub(crate) hosts: Vec<HostChoice>,
    /// Which host the cycler shows.
    pub(crate) host_index: usize,
    /// Whether the user moved the cycler, which freezes the late `defaultHost` seeding.
    pub(crate) host_touched: bool,
    /// The branch input, mirrored from the live editor on every `Changed`.
    pub(crate) branch: String,
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
    /// The exact failure from refreshing base refs.
    pub(crate) base_error: Option<String>,
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
        let query = FuzzyQuery::new(&self.branch);
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

    /// The entry the cycler shows.
    #[must_use]
    pub fn selected_choice(&self) -> Option<&HostChoice> {
        self.hosts.get(self.host_index)
    }

    /// The host the cycler shows, or `None` for `local`.
    #[must_use]
    pub fn selected_host(&self) -> Option<HostId> {
        self.selected_choice()?.id.clone()
    }

    /// Why `Enter` is refused by the host cycler, when it is.
    #[must_use]
    pub fn host_blocked(&self) -> Option<&str> {
        self.selected_choice()?.blocked.as_deref()
    }

    /// Moves the cycler onto `default`, unless the user already moved it or that host is
    /// unreachable — seeding a picker onto an entry that refuses `Enter` is worse than
    /// leaving it on `local`.
    pub(crate) fn select_default_host(&mut self, default: Option<&HostId>) -> bool {
        if self.host_touched {
            return false;
        }
        let Some(default) = default else {
            return false;
        };
        let Some(index) = self
            .hosts
            .iter()
            .position(|choice| choice.id.as_ref() == Some(default))
        else {
            return false;
        };
        if self.hosts[index].blocked.is_some() {
            return false;
        }
        self.host_index = index;
        true
    }

    /// The worktree id the current branch would produce.
    #[must_use]
    pub fn preview_id(&self) -> Option<String> {
        let repo = self.repo.as_ref()?;
        let slug = slugify(&self.branch);
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
        match validate_branch(&self.branch) {
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
            && !slugify(&self.branch).is_empty()
            && self.host_blocked().is_none()
    }

    fn start_base_ref_fetch(&mut self) {
        self.fetching = true;
        self.base_error = None;
    }

    fn fail_base_ref_fetch(&mut self, message: String) {
        self.fetching = false;
        self.base_error = Some(message);
    }

    fn should_retry_base_refs(&self) -> bool {
        self.base_error.is_some() && (self.field == Field::Base || !self.can_submit())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NavigationIntent {
    screen: Screen,
    hub_pane: HubPane,
    scope: RepoScope,
    cursors: Cursors,
}

struct NavigationGuard {
    intent: NavigationIntent,
    valid: Rc<Cell<bool>>,
    _subscription: gpui::Subscription,
}

impl NavigationGuard {
    fn new(state: &Entity<AppState>, cx: &mut App) -> Self {
        let intent = NavigationIntent::capture(state.read(cx));
        let observed_intent = intent.clone();
        let valid = Rc::new(Cell::new(true));
        let observed_valid = Rc::clone(&valid);
        let subscription = cx.observe(state, move |state, cx| {
            if !observed_intent.matches(state.read(cx)) {
                observed_valid.set(false);
            }
        });
        Self {
            intent,
            valid,
            _subscription: subscription,
        }
    }

    fn is_current(&self, state: &AppState) -> bool {
        self.valid.get() && self.intent.matches(state)
    }
}

impl NavigationIntent {
    fn capture(state: &AppState) -> Self {
        Self {
            screen: state.screen.clone(),
            hub_pane: state.hub_pane,
            scope: state.scope.clone(),
            cursors: state.cursors.clone(),
        }
    }

    fn matches(&self, state: &AppState) -> bool {
        state.overlay.is_none() && self == &Self::capture(state)
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
    seed_with_transport(state, bridge, cx);
}

fn seed_with_transport<T: CreateTransport>(state: &Entity<AppState>, transport: &T, cx: &mut App) {
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
                draft.hosts.push(HostChoice::local());
                draft
                    .hosts
                    .extend(snapshot.hosts.iter().map(HostChoice::from_status));
            }
        }
    }
    let repo = draft.repo.clone();
    let has_hosts = !draft.hosts.is_empty();
    if repo.is_some() {
        draft.start_base_ref_fetch();
    }
    let seq = with_host(state, cx, |host| {
        let seq = host.create.seq.wrapping_add(1);
        draft.seq = seq;
        host.create = draft;
        seq
    });
    branch::seed_input(state, cx);
    if let Some(repo) = repo {
        poll_base_refs(repo, seq, state, transport, cx);
    }
    if has_hosts {
        poll_default_host(seq, state, transport, cx);
    }
}

/// Seeds the cycler onto `config.defaultHost`.
///
/// `defaultHost` is configuration, not snapshot state, so it costs one request. The answer is
/// dropped when the dialog moved on (`seq`) or when the user already touched the cycler, so a
/// late reply never moves the selection under the user's hands.
fn poll_default_host<T: CreateTransport>(
    seq: u64,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    let reply = transport.request(RequestBody::GetConfig);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Config(config))) = reply.recv().await else {
            return;
        };
        let default = config.default_host().cloned();
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let moved = with_host(&state, cx, |host| {
                host.create.seq == seq && host.create.select_default_host(default.as_ref())
            });
            if moved {
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "create-default-host", task);
}

/// Shows cached base refs first, then replaces them with one bounded forced refresh.
fn poll_base_refs<T: CreateTransport>(
    repo: RepoId,
    seq: u64,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    let weak_state = state.downgrade();
    let transport = transport.clone();
    let task = cx.spawn(async move |cx| {
        let cached = transport.request(RequestBody::ListBaseRefs {
            repo: repo.clone(),
            force: false,
        });
        let Some(answer) = before_timeout(
            cached.recv(),
            cx.background_executor().timer(BASE_REF_TIMEOUT),
        )
        .await
        else {
            finish_base_ref_failure(&weak_state, seq, "base-ref cache timed out".to_owned(), cx);
            return;
        };
        let refs = match base_refs_from(answer) {
            Ok(refs) => refs,
            Err(message) => {
                finish_base_ref_failure(&weak_state, seq, message, cx);
                return;
            }
        };
        if !apply_base_refs(&weak_state, seq, refs.refs, true, cx) {
            return;
        }

        let refreshed = transport.request(RequestBody::ListBaseRefs { repo, force: true });
        let Some(answer) = before_timeout(
            refreshed.recv(),
            cx.background_executor().timer(BASE_REF_TIMEOUT),
        )
        .await
        else {
            finish_base_ref_failure(
                &weak_state,
                seq,
                "base-ref refresh timed out".to_owned(),
                cx,
            );
            return;
        };
        match base_refs_from(answer) {
            Ok(refs) => {
                apply_base_refs(&weak_state, seq, refs.refs, false, cx);
            }
            Err(message) => finish_base_ref_failure(&weak_state, seq, message, cx),
        }
    });
    crate::dialogs::retain_task(state, cx, "create-refs", task);
}

fn base_refs_from(
    answer: Result<Result<ResponseBody, fleet_proto::error::ProtoError>, async_channel::RecvError>,
) -> Result<fleet_proto::response::BaseRefs, String> {
    match answer {
        Ok(Ok(ResponseBody::BaseRefs(refs))) => Ok(refs),
        Ok(Err(failure)) => Err(failure.message),
        Ok(Ok(_)) => Err("unexpected base-ref response".to_owned()),
        Err(_) => Err("fleetd disconnected during base-ref refresh".to_owned()),
    }
}

fn apply_base_refs(
    state: &gpui::WeakEntity<AppState>,
    seq: u64,
    refs: Vec<String>,
    fetching: bool,
    cx: &mut gpui::AsyncApp,
) -> bool {
    cx.update(|cx| {
        let Some(state) = state.upgrade() else {
            return false;
        };
        let live = with_host(&state, cx, |host| {
            if host.create.seq != seq {
                return false;
            }
            host.create.base_refs = refs;
            host.create.fetching = fetching;
            host.create.base_cursor = host
                .create
                .base_cursor
                .min(host.create.base_candidates().len().saturating_sub(1));
            true
        });
        if live {
            notify(&state, cx);
        }
        live
    })
}

fn finish_base_ref_failure(
    state: &gpui::WeakEntity<AppState>,
    seq: u64,
    message: String,
    cx: &mut gpui::AsyncApp,
) {
    cx.update(|cx| {
        let Some(state) = state.upgrade() else {
            return;
        };
        let live = with_host(&state, cx, |host| {
            if host.create.seq != seq {
                return false;
            }
            host.create.fail_base_ref_fetch(message);
            true
        });
        if live {
            notify(&state, cx);
        }
    });
}

/// `Tab` / `S-Tab`: branch, base, then the host cycler when one is configured.
///
/// Landing on the branch hands it the keyboard, and leaving it hands the keyboard back to the
/// dialog, which is what publishes `Create` instead of `CreateEditing`.
fn move_field(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
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
    focus_field(state, focus, window, cx);
    notify(state, cx);
}

/// Hands the keyboard to the branch editor while it is the focused field, and to the dialog
/// itself otherwise.
fn focus_field(state: &Entity<AppState>, focus: &FocusHandle, window: &mut Window, cx: &mut App) {
    let input = read_host(state, cx, |host, _| {
        (host.create.field == Field::Branch)
            .then(|| host.create_branch.clone())
            .flatten()
    });
    match input {
        Some(input) => input.update(cx, |input, cx| input.focus(window, cx)),
        None => window.focus(focus, cx),
    }
}

fn move_base(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    with_host(state, cx, |host| {
        let len = host.create.base_candidates().len();
        host.create.base_cursor = step(host.create.base_cursor, delta, len);
        host.create.field = Field::Base;
    });
    focus_field(state, focus, window, cx);
    notify(state, cx);
}

fn cycle_host(
    state: &Entity<AppState>,
    delta: isize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    with_host(state, cx, |host| {
        let len = host.create.hosts.len();
        host.create.host_touched = true;
        host.create.host_index = step(host.create.host_index, delta, len);
        if len > 0 {
            host.create.field = Field::Host;
        }
    });
    focus_field(state, focus, window, cx);
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
    submit_with_transport(open_after, state, bridge, cx);
}

fn submit_with_transport<T: CreateTransport>(
    open_after: bool,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    if with_host(state, cx, |host| host.create.should_retry_base_refs()) {
        retry_base_refs(state, transport, cx);
        return;
    }
    let Some((repo, branch, base, host)) = with_host(state, cx, |host| {
        let draft = &host.create;
        if !draft.can_submit() {
            return None;
        }
        Some((
            draft.repo.clone()?,
            draft.branch.clone(),
            draft.selected_base(),
            draft.selected_host(),
        ))
    }) else {
        return;
    };
    let slug = slugify(&branch);
    let id = format!("{}#{slug}", repo.as_str());
    if let Some(existing) = existing_worktree(state.read(cx), &id) {
        close(state, cx);
        if open_after {
            let guard = NavigationGuard::new(state, cx);
            open_worktree_if_intended(existing, guard, state, transport, cx);
        }
        return;
    }

    let hooks = state
        .read(cx)
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.repos.iter().find(|entry| entry.id == repo))
        .map_or_else(RepoHooks::default, |entry| entry.hooks.clone());
    close(state, cx);
    let guard = NavigationGuard::new(state, cx);
    let reply = transport.request(RequestBody::CreateWorktree {
        repo,
        slug,
        branch: Some(branch),
        base,
        host,
        hooks,
    });
    let transport_handle = transport.clone();
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
                    if open_after && guard.is_current(state_handle.read(cx)) {
                        open_worktree_if_intended(
                            worktree.id,
                            guard,
                            &state_handle,
                            &transport_handle,
                            cx,
                        );
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
}

fn retry_base_refs<T: CreateTransport>(state: &Entity<AppState>, transport: &T, cx: &mut App) {
    let next = with_host(state, cx, |host| {
        let repo = host.create.repo.clone()?;
        host.create.seq = host.create.seq.wrapping_add(1);
        host.create.start_base_ref_fetch();
        Some((repo, host.create.seq))
    });
    let Some((repo, seq)) = next else { return };
    notify(state, cx);
    poll_base_refs(repo, seq, state, transport, cx);
}

fn open_worktree_if_intended<T: CreateTransport>(
    id: WorktreeId,
    guard: NavigationGuard,
    state: &Entity<AppState>,
    transport: &T,
    cx: &mut App,
) {
    if !guard.is_current(state.read(cx)) {
        return;
    }
    transport.send(RequestBody::TouchWorktreeOpened { id: id.clone() });
    let reply = transport.request(RequestBody::EnsureSession {
        worktree: Some(id),
        agent: None,
        sleep_previous: true,
    });
    super::host::complete_request(state, cx, async move |state, cx| {
        let Ok(Ok(ResponseBody::Session(session))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            if guard.is_current(state.read(cx)) {
                open_session(session.id, &state, cx);
            }
        });
    });
}

fn close(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}
