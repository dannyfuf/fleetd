//! §3.8.2 Clone repo — *find a GitHub repo by typing a few letters and get it cloning*.

use std::time::Duration;

use fleet_core::{cache::RepoCache, config::CloneProtocol, github::RemoteRepo, ids::ContextId};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{DialogHost, field, notify, root, step, type_into, with_host},
    presentation::{age_label, now_unix},
    state::AppState,
};

/// How long the input rests before a search is issued (§3.8.2).
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// How many result rows the list shows (§3.8.2: "8 is swarm's cap").
pub const RESULT_ROWS: usize = 8;

type Reply = async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>;

trait CloneTransport: Clone + 'static {
    fn send(&self, body: RequestBody);
    fn request(&self, body: RequestBody) -> Reply;
}

impl CloneTransport for Bridge {
    fn send(&self, body: RequestBody) {
        Bridge::send(self, body);
    }

    fn request(&self, body: RequestBody) -> Reply {
        Bridge::request(self, body)
    }
}

/// The Clone dialog's draft.
#[derive(Debug)]
pub struct CloneState {
    /// The context the repository is cloned into.
    pub(crate) context: Option<ContextId>,
    /// That context's display name, for the header.
    pub(crate) context_name: String,
    /// The owners the search is scoped to.
    pub(crate) owners: Vec<String>,
    /// The search input.
    pub(crate) query: TextFieldState,
    /// The ranked results, capped at [`RESULT_ROWS`].
    pub(crate) results: Vec<RemoteRepo>,
    /// Which result carries the cursor.
    pub(crate) cursor: usize,
    /// Whether a search is in flight.
    pub(crate) searching: bool,
    /// The verbatim `gh` failure, when the last search failed.
    pub(crate) error: Option<String>,
    /// When the results came out of a cache rather than a live query.
    pub(crate) cached_at: Option<String>,
    /// `github.cloneProtocol`: the footer names it and `Enter` clones with it.
    pub(crate) protocol: CloneProtocol,
    /// Bumps on every keystroke; a late answer to a superseded query is dropped.
    pub(crate) search_seq: u64,
    /// Bumps on every opening; config replies never compete with query generations.
    pub(crate) seq: u64,
}

impl Default for CloneState {
    fn default() -> Self {
        Self {
            context: None,
            context_name: String::new(),
            owners: Vec::new(),
            query: TextFieldState::default(),
            results: Vec::new(),
            cursor: 0,
            searching: false,
            error: None,
            cached_at: None,
            // Replaced by the effective `github.cloneProtocol` as soon as the daemon answers.
            protocol: CloneProtocol::Ssh,
            search_seq: 0,
            seq: 0,
        }
    }
}

impl CloneState {
    /// The rows to draw: a typed `owner/name` first when the query is one, then the results.
    #[must_use]
    pub fn rows(&self) -> Vec<RemoteRepo> {
        let mut rows = Vec::with_capacity(RESULT_ROWS);
        if let Some(manual) = manual_entry(self.query.text())
            && !self
                .results
                .iter()
                .any(|repo| repo.full_name == manual.full_name)
        {
            rows.push(manual);
        }
        rows.extend(
            self.results
                .iter()
                .take(RESULT_ROWS.saturating_sub(rows.len()))
                .cloned(),
        );
        rows
    }

    /// The row `Enter` would clone.
    #[must_use]
    pub fn selected(&self) -> Option<RemoteRepo> {
        self.rows().get(self.cursor).cloned()
    }

    fn begin_search(&mut self) -> u64 {
        self.search_seq = self.search_seq.wrapping_add(1);
        self.searching = !self.query.is_empty();
        self.error = None;
        self.cached_at = None;
        self.results.clear();
        self.cursor = 0;
        self.search_seq
    }

    /// The word the footer names the protocol with.
    #[must_use]
    pub const fn protocol_word(&self) -> &'static str {
        match self.protocol {
            CloneProtocol::Ssh => "ssh",
            CloneProtocol::Https => "https",
        }
    }
}

/// The URL `github.cloneProtocol` selects for a repository.
///
/// `gh` only ever hands back the SSH URL, so the HTTPS form is derived from it rather than
/// hard-coding `github.com` — an enterprise host has to survive the switch.
#[must_use]
pub fn clone_url(repo: &RemoteRepo, protocol: CloneProtocol) -> String {
    match protocol {
        CloneProtocol::Ssh => repo.ssh_url.clone(),
        CloneProtocol::Https => https_url(repo),
    }
}

fn https_url(repo: &RemoteRepo) -> String {
    if repo.ssh_url.starts_with("https://") {
        return repo.ssh_url.clone();
    }
    if let Some(rest) = repo.ssh_url.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
        && !host.is_empty()
        && !path.is_empty()
    {
        return format!("https://{host}/{path}");
    }
    format!("https://github.com/{}.git", repo.full_name)
}

/// A pasted `owner/name` or GitHub URL, turned into a first result row (§3.8.2).
#[must_use]
pub fn manual_entry(query: &str) -> Option<RemoteRepo> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .unwrap_or(trimmed);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
            })
    };
    if !valid(owner) || !valid(name) {
        return None;
    }
    Some(RemoteRepo {
        owner: owner.to_owned(),
        name: name.to_owned(),
        full_name: format!("{owner}/{name}"),
        description: String::new(),
        ssh_url: format!("git@github.com:{owner}/{name}.git"),
        is_private: false,
        updated_at: String::new(),
        default_branch: String::new(),
    })
}

/// Fills the draft from the snapshot. No search runs until something is typed.
pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let mut draft = CloneState::default();
    {
        let app = state.read(cx);
        if let Some(snapshot) = app.snapshot.as_ref() {
            let active = app.active_context().cloned().or_else(|| {
                snapshot
                    .contexts
                    .get(app.cursors.repos)
                    .map(|entry| entry.id.clone())
            });
            if let Some(context) = snapshot
                .contexts
                .iter()
                .find(|entry| Some(&entry.id) == active.as_ref())
            {
                draft.context = Some(context.id.clone());
                draft.context_name = context.name.clone();
                draft.owners = context.owners.clone();
            }
        }
    }
    let seq = with_host(state, cx, |host| {
        draft.seq = host.clone.seq.wrapping_add(1);
        host.clone = draft;
        host.clone.seq
    });
    // §SWARM-INVENTORY `github.cloneProtocol` decides the URL, so the dialog reads the
    // effective configuration rather than assuming SSH. The answer lands before the user can
    // finish typing a repository name, and a late one for a superseded opening is dropped.
    let reply = bridge.request(RequestBody::GetConfig);
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Config(config))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let changed = with_host(&state, cx, |host| {
                if host.clone.seq != seq || host.clone.protocol == config.github.clone_protocol {
                    return false;
                }
                host.clone.protocol = config.github.clone_protocol;
                true
            });
            if changed {
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "clone-config", task);
}

/// Issues the debounced search for the current query.
fn schedule_search<T: CloneTransport>(state: &Entity<AppState>, transport: &T, cx: &mut App) {
    let (opening_seq, search_seq, query, owners) = with_host(state, cx, |host| {
        let search_seq = host.clone.begin_search();
        (
            host.clone.seq,
            search_seq,
            host.clone.query.text().to_owned(),
            host.clone.owners.clone(),
        )
    });
    if query.trim().is_empty() || owners.is_empty() {
        with_host(state, cx, |host| {
            host.tasks.remove("clone-search");
            if host.clone.seq == opening_seq && host.clone.search_seq == search_seq {
                host.clone.results.clear();
                host.clone.searching = false;
            }
        });
        notify(state, cx);
        return;
    }
    notify(state, cx);
    let weak_state = state.downgrade();
    let transport = transport.clone();
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(DEBOUNCE).await;
        if cx.update(|cx| {
            weak_state.upgrade().is_none_or(|state| {
                with_host(&state, cx, |host| {
                    host.clone.seq != opening_seq || host.clone.search_seq != search_seq
                })
            })
        }) {
            return;
        }
        let Some(SearchResults {
            results,
            error,
            cached_at,
        }) = search_owners(&owners, &query, |request| transport.request(request)).await
        else {
            return;
        };
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let live = with_host(&state, cx, |host| {
                if host.clone.seq != opening_seq || host.clone.search_seq != search_seq {
                    return false;
                }
                host.clone.searching = false;
                host.clone.error = error;
                host.clone.cached_at = cached_at;
                host.clone.cursor = 0;
                host.clone.results = results;
                true
            });
            if live {
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "clone-search", task);
}

struct SearchResults {
    results: Vec<RemoteRepo>,
    error: Option<String>,
    cached_at: Option<String>,
}

async fn search_owners(
    owners: &[String],
    query: &str,
    request: impl Fn(
        RequestBody,
    )
        -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>,
) -> Option<SearchResults> {
    let mut results: Vec<RemoteRepo> = Vec::new();
    let mut error: Option<String> = None;
    let mut cached_at: Option<String> = None;
    // Submit bounded batches in owner order and consume replies in that same order.
    for owners in owners.chunks(3) {
        let replies: Vec<_> = owners
            .iter()
            .map(|owner| {
                request(RequestBody::SearchRemoteRepos {
                    owner: owner.clone(),
                    query: query.to_owned(),
                })
            })
            .collect();
        for reply in replies {
            match reply.recv().await {
                Ok(Ok(ResponseBody::RemoteRepos(cache))) => {
                    let RepoCache { fetched_at, repos } = cache;
                    cached_at.get_or_insert(fetched_at);
                    results.extend(
                        repos
                            .into_iter()
                            .take(RESULT_ROWS.saturating_sub(results.len())),
                    );
                }
                Ok(Err(failure)) => error = Some(failure.message),
                Ok(Ok(_)) => {}
                Err(_) => return None,
            }
        }
    }
    Some(SearchResults {
        results,
        error,
        cached_at,
    })
}

/// What the result list shows instead of rows: the failure, the invitation, or the miss.
fn no_results(draft: &CloneState) -> AnyElement {
    if let Some(message) = draft.error.clone() {
        return div()
            .flex()
            .flex_col()
            .child(Text::ui(message).tone(Tone::Danger).ellipsize())
            .child(KeyHintRow::new().key("enter", "retry"))
            .into_any_element();
    }
    if draft.query.is_empty() {
        return Text::ui(format!(
            "Type to search GitHub repos in {}'s owners.",
            if draft.context_name.is_empty() {
                "this context"
            } else {
                draft.context_name.as_str()
            }
        ))
        .muted()
        .into_any_element();
    }
    if draft.searching {
        return Text::ui("Searching\u{2026}").muted().into_any_element();
    }
    Text::ui(format!("Nothing matches \"{}\".", draft.query.text()))
        .muted()
        .into_any_element()
}

/// One row per candidate repository: visibility, name, description and last push.
fn results_list(draft: &CloneState, rows: &[RemoteRepo], now: i64) -> FuzzyList {
    FuzzyList::new(rows.iter().map(|repo| {
        let mut item = FuzzyItem::new(repo.full_name.clone()).leading(
            if repo.is_private {
                Icon::Lock
            } else {
                Icon::Globe
            }
            .el()
            .size(IconSize::Medium),
        );
        if !repo.description.is_empty() {
            item = item.secondary(repo.description.clone());
        }
        if !repo.updated_at.is_empty() {
            item = item.trailing(age_label(&repo.updated_at, now));
        }
        item
    }))
    .cursor(draft.cursor)
    .cap(RESULT_ROWS)
    .under_text_field(true)
    .empty(no_results(draft))
}

/// Renders the dialog (§3.8.2).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let draft = &host.read(cx).clone;
    let offline = !state.read(cx).daemon.is_connected();
    let now = now_unix();
    let rows = draft.rows();

    // §3.8.2: the field carries a search affordance (the magnifier), not a sentence. The one
    // instruction is the *idle* body line below, which also names the context; printing it
    // twice, once with `this context` and once with the real name, says nothing extra.
    let search_field = field(&draft.query)
        .icon(if draft.searching {
            Icon::LoaderCircle
        } else {
            Icon::Search
        })
        .focused(true);

    let list = results_list(draft, &rows, now);

    let mut body = div().flex().flex_col().gap(gap).child(search_field);
    if offline && let Some(cached) = draft.cached_at.as_ref() {
        body = body
            .child(Text::hint(format!("cached {}", age_label(cached, now))).tone(Tone::Warning));
    }
    let body = body.child(list);

    let card = Dialog::new("Clone repo")
        .icon(Icon::CloudDownload)
        .width(super::Dialogs::CloneRepo.width(cx))
        .when_some(super::Dialogs::CloneRepo.height(), Dialog::height)
        .subtitle(format!("\u{00b7} into context \"{}\"", draft.context_name))
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key("\u{2303}n/\u{2303}p", "move")
                .key("esc", "cancel")
                .key(draft.protocol_word(), "clones in the background"),
        )
        .primary("\u{23ce} Clone");

    let cancel_state = state.clone();
    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();

    super::input::actions(root(focus), state, |host| &mut host.clone.query, {
        let bridge = bridge.clone();
        move |state, cx| schedule_search(state, &bridge, cx)
    })
    .on_key_down({
        let state = state.clone();
        let bridge = bridge.clone();
        move |event, _window, cx| {
            if with_host(&state, cx, |host| type_into(&mut host.clone.query, event)) {
                schedule_search(&state, &bridge, cx);
            }
        }
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::CursorDown, _window, cx| move_cursor(&state, 1, cx)
    })
    .on_action({
        let state = state.clone();
        move |_: &dialog::CursorUp, _window, cx| move_cursor(&state, -1, cx)
    })
    .on_action(move |_: &dialog::Confirm, _window, cx| {
        submit(&confirm_state, &confirm_bridge, cx);
    })
    .on_action(move |_: &dialog::Cancel, _window, cx| {
        // §3.8.2: `Esc` aborts the search request only. Bumping the sequence orphans the
        // pending answer; a `CloneJob` already accepted by the daemon is untouched.
        with_host(&cancel_state, cx, |host| {
            host.clone.seq = host.clone.seq.wrapping_add(1);
            host.clone.searching = false;
        });
        cx.propagate();
    })
    .child(card)
    .into_any_element()
}

fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        let len = host.clone.rows().len();
        host.clone.cursor = step(host.clone.cursor, delta, len);
    });
    notify(state, cx);
}

/// `Enter`: hand the clone to the daemon and close; the rail shows a `⟳` row from the moment
/// the job is persisted (§3.8.2).
fn submit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    submit_with_transport(state, bridge, cx);
}

fn submit_with_transport<T: CloneTransport>(state: &Entity<AppState>, transport: &T, cx: &mut App) {
    let selection = with_host(state, cx, |host| {
        host.clone.selected().zip(host.clone.context.clone())
    });
    let Some((repo, context)) = selection else {
        if with_host(state, cx, |host| host.clone.error.is_some()) {
            schedule_search(state, transport, cx);
        }
        return;
    };
    let protocol = with_host(state, cx, |host| host.clone.protocol);
    let url = clone_url(&repo, protocol);
    transport.send(RequestBody::CloneRepo {
        owner: repo.owner,
        name: repo.name,
        url,
        context,
        default_branch: Some(repo.default_branch),
    });
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[derive(Clone, Default)]
    struct FakeTransport {
        requests: Rc<RefCell<Vec<RequestBody>>>,
    }

    impl CloneTransport for FakeTransport {
        fn send(&self, body: RequestBody) {
            self.requests.borrow_mut().push(body);
        }

        fn request(&self, body: RequestBody) -> Reply {
            let (_sender, receiver) = async_channel::bounded(1);
            self.requests.borrow_mut().push(body);
            receiver
        }
    }

    struct RetryView {
        state: Entity<AppState>,
        transport: FakeTransport,
        focus: FocusHandle,
    }

    impl gpui::Render for RetryView {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let state = self.state.clone();
            let transport = self.transport.clone();
            root(&self.focus).on_action(move |_: &dialog::Confirm, _window, cx| {
                submit_with_transport(&state, &transport, cx);
            })
        }
    }

    #[test]
    fn the_clone_url_follows_the_configured_protocol() {
        let repo = manual_entry("acme/widgets").unwrap_or_else(|| panic!("a valid owner/name"));
        assert_eq!(
            clone_url(&repo, CloneProtocol::Ssh),
            "git@github.com:acme/widgets.git"
        );
        assert_eq!(
            clone_url(&repo, CloneProtocol::Https),
            "https://github.com/acme/widgets.git"
        );
    }

    #[test]
    fn the_https_url_keeps_an_enterprise_host() {
        let mut repo = manual_entry("acme/widgets").unwrap_or_else(|| panic!("a valid name"));
        repo.ssh_url = "git@github.acme.internal:acme/widgets.git".to_owned();
        assert_eq!(
            clone_url(&repo, CloneProtocol::Https),
            "https://github.acme.internal/acme/widgets.git",
            "the host comes from the URL gh gave us, never from a hard-coded github.com"
        );
        repo.ssh_url = "https://github.com/acme/widgets.git".to_owned();
        assert_eq!(
            clone_url(&repo, CloneProtocol::Https),
            "https://github.com/acme/widgets.git"
        );
    }

    #[test]
    fn the_footer_names_the_protocol_it_will_use() {
        let mut draft = CloneState::default();
        assert_eq!(draft.protocol_word(), "ssh");
        draft.protocol = CloneProtocol::Https;
        assert_eq!(draft.protocol_word(), "https");
    }
    fn repo(full_name: &str) -> RemoteRepo {
        let (owner, name) = full_name.split_once('/').unwrap_or(("acme", "repo"));
        RemoteRepo {
            owner: owner.to_owned(),
            name: name.to_owned(),
            full_name: full_name.to_owned(),
            description: String::new(),
            ssh_url: format!("git@github.com:{full_name}.git"),
            is_private: false,
            updated_at: "2026-09-01T00:00:00Z".to_owned(),
            default_branch: "main".to_owned(),
        }
    }

    #[test]
    fn a_typed_owner_name_becomes_the_first_row() {
        let manual = manual_entry("bukhr/payroll").unwrap_or_else(|| panic!("expected a row"));
        assert_eq!(manual.full_name, "bukhr/payroll");
        assert_eq!(manual.ssh_url, "git@github.com:bukhr/payroll.git");
    }

    #[test]
    fn pasted_urls_are_recognised_in_every_shape() {
        for query in [
            "https://github.com/bukhr/payroll",
            "https://github.com/bukhr/payroll.git",
            "git@github.com:bukhr/payroll.git",
        ] {
            assert_eq!(
                manual_entry(query).map(|repo| repo.full_name),
                Some("bukhr/payroll".to_owned()),
                "{query}"
            );
        }
        assert_eq!(manual_entry("payroll"), None);
        assert_eq!(manual_entry(""), None);
    }

    #[test]
    fn the_manual_row_never_duplicates_a_result() {
        let state = CloneState {
            query: TextFieldState::from_text("bukhr/payroll"),
            results: vec![repo("bukhr/payroll")],
            ..CloneState::default()
        };
        assert_eq!(state.rows().len(), 1);
    }

    #[test]
    fn results_are_capped_at_eight_rows() {
        let state = CloneState {
            query: TextFieldState::from_text("pay"),
            results: (0..12).map(|n| repo(&format!("acme/pay{n}"))).collect(),
            ..CloneState::default()
        };
        assert_eq!(state.rows().len(), RESULT_ROWS);
        assert_eq!(
            state.selected().map(|repo| repo.full_name),
            Some("acme/pay0".to_owned())
        );
    }

    #[test]
    fn typing_does_not_discard_config_protocol() {
        let mut state = CloneState {
            seq: 9,
            protocol: CloneProtocol::Https,
            query: TextFieldState::from_text("pay"),
            ..CloneState::default()
        };
        state.begin_search();
        assert_eq!(state.seq, 9, "query generations must not supersede config");
        assert_eq!(state.protocol, CloneProtocol::Https);
    }

    #[test]
    fn pending_query_cannot_select_stale_result() {
        let mut state = CloneState {
            query: TextFieldState::from_text("new query"),
            results: vec![repo("acme/old-result")],
            ..CloneState::default()
        };
        state.begin_search();
        assert_eq!(state.selected(), None);
        assert!(state.searching);
    }

    #[test]
    fn retry_reissues_query_without_editing_text() {
        let mut state = CloneState {
            query: TextFieldState::from_text("payroll"),
            error: Some("offline".to_owned()),
            ..CloneState::default()
        };
        let previous = state.search_seq;
        state.begin_search();
        assert_eq!(state.query.text(), "payroll");
        assert_eq!(state.search_seq, previous + 1);
        assert_eq!(state.error, None);
    }

    #[gpui::test]
    fn confirm_retries_errored_query_without_editing_text(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new("/tmp/fleet", std::time::Instant::now()));
        cx.update(|cx| {
            with_host(&state, cx, |host| {
                host.clone.query = TextFieldState::from_text("payroll");
                host.clone.owners = vec!["buk".to_owned()];
                host.clone.error = Some("offline".to_owned());
            })
        });
        let transport = FakeTransport::default();
        let window = cx.add_window(|_, cx| RetryView {
            state: state.clone(),
            transport: transport.clone(),
            focus: cx.focus_handle(),
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(&mut visual, |view, window, cx| {
                window.focus(&view.focus, cx);
                window.dispatch_action(Box::new(dialog::Confirm), cx);
            })
            .unwrap();
        cx.executor().advance_clock(DEBOUNCE);
        cx.run_until_parked();
        let requests = transport.requests.borrow();
        assert!(matches!(
            requests.as_slice(),
            [RequestBody::SearchRemoteRepos { owner, query }]
                if owner == "buk" && query == "payroll"
        ));
        visual.update(|_, cx| {
            with_host(&state, cx, |host| {
                assert_eq!(host.clone.query.text(), "payroll");
            })
        });
    }

    #[gpui::test]
    async fn owner_searches_are_bounded_and_preserve_result_order(cx: &mut gpui::TestAppContext) {
        let pending = Rc::new(RefCell::new(Vec::new()));
        let requested = pending.clone();
        let task = cx.spawn(async move |_| {
            search_owners(
                &["one".into(), "two".into(), "three".into(), "four".into()],
                "pay",
                |request| {
                    let RequestBody::SearchRemoteRepos { owner, .. } = request else {
                        panic!("search request");
                    };
                    let (sender, receiver) = async_channel::bounded(1);
                    requested.borrow_mut().push((owner, sender));
                    receiver
                },
            )
            .await
            .expect("search results")
        });
        cx.run_until_parked();
        assert_eq!(pending.borrow().len(), 3);
        let replies: Vec<_> = pending
            .borrow()
            .iter()
            .map(|(owner, reply)| (owner.clone(), reply.clone()))
            .collect();
        for (owner, reply) in replies.into_iter().rev() {
            reply
                .try_send(Ok(ResponseBody::RemoteRepos(RepoCache {
                    fetched_at: owner.clone(),
                    repos: vec![repo(&format!("{owner}/pay"))],
                })))
                .unwrap();
        }
        cx.run_until_parked();
        assert_eq!(pending.borrow().len(), 4);
        let (owner, reply) = pending.borrow()[3].clone();
        reply
            .try_send(Ok(ResponseBody::RemoteRepos(RepoCache {
                fetched_at: owner.clone(),
                repos: vec![repo(&format!("{owner}/pay"))],
            })))
            .unwrap();
        let found = task.await;
        assert_eq!(
            found
                .results
                .iter()
                .map(|repo| repo.full_name.as_str())
                .collect::<Vec<_>>(),
            ["one/pay", "two/pay", "three/pay", "four/pay"]
        );
        assert_eq!(found.cached_at.as_deref(), Some("one"));
    }
}
