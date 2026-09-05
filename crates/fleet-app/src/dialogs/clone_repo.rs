//! §3.8.2 Clone repo — *find a GitHub repo by typing a few letters and get it cloning*.
//!
//! Search is debounced by [`DEBOUNCE`] and runs on the daemon; the input never waits for it.
//! `Esc` aborts the pending search and closes the dialog, and a clone that has already
//! started keeps running in fleetd — §3.8.2 is explicit that the two are different things.

use std::time::Duration;

use fleet_core::{cache::RepoCache, config::CloneProtocol, github::RemoteRepo, ids::ContextId};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::dialog,
    bridge::Bridge,
    dialogs::{TextInput, age_label, notify, now_epoch, root, step, type_into, with_host},
    state::AppState,
};

/// How long the input rests before a search is issued (§3.8.2).
pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// How many result rows the list shows (§3.8.2: "8 is swarm's cap").
pub const RESULT_ROWS: usize = 8;

/// The Clone dialog's draft.
#[derive(Debug, Clone)]
pub struct CloneState {
    /// The context the repository is cloned into.
    pub context: Option<ContextId>,
    /// That context's display name, for the header.
    pub context_name: String,
    /// The owners the search is scoped to.
    pub owners: Vec<String>,
    /// The search input.
    pub query: TextInput,
    /// The ranked results, capped at [`RESULT_ROWS`].
    pub results: Vec<RemoteRepo>,
    /// Which result carries the cursor.
    pub cursor: usize,
    /// Whether a search is in flight.
    pub searching: bool,
    /// The verbatim `gh` failure, when the last search failed.
    pub error: Option<String>,
    /// When the results came out of a cache rather than a live query.
    pub cached_at: Option<String>,
    /// `github.cloneProtocol`: the footer names it and `Enter` clones with it.
    pub protocol: CloneProtocol,
    /// Bumps on every keystroke; a late answer to a superseded query is dropped.
    pub seq: u64,
}

impl Default for CloneState {
    fn default() -> Self {
        Self {
            context: None,
            context_name: String::new(),
            owners: Vec::new(),
            query: TextInput::default(),
            results: Vec::new(),
            cursor: 0,
            searching: false,
            error: None,
            cached_at: None,
            // Replaced by the effective `github.cloneProtocol` as soon as the daemon answers.
            protocol: CloneProtocol::Ssh,
            seq: 0,
        }
    }
}

impl CloneState {
    /// The rows to draw: a typed `owner/name` first when the query is one, then the results.
    #[must_use]
    pub fn rows(&self) -> Vec<RemoteRepo> {
        let mut rows = Vec::with_capacity(RESULT_ROWS);
        if let Some(manual) = manual_entry(self.query.value())
            && !self
                .results
                .iter()
                .any(|repo| repo.full_name == manual.full_name)
        {
            rows.push(manual);
        }
        rows.extend(self.results.iter().cloned());
        rows.truncate(RESULT_ROWS);
        rows
    }

    /// The row `Enter` would clone.
    #[must_use]
    pub fn selected(&self) -> Option<RemoteRepo> {
        self.rows().get(self.cursor).cloned()
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

// ---------------------------------------------------------------------------- seeding

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
    let seq = with_host(cx, |host| {
        draft.seq = host.clone.seq.wrapping_add(1);
        host.clone = draft;
        host.clone.seq
    });
    // §SWARM-INVENTORY `github.cloneProtocol` decides the URL, so the dialog reads the
    // effective configuration rather than assuming SSH. The answer lands before the user can
    // finish typing a repository name, and a late one for a superseded opening is dropped.
    let reply = bridge.request(RequestBody::GetConfig);
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(ResponseBody::Config(config))) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let changed = with_host(cx, |host| {
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
    })
    .detach();
}

/// Issues the debounced search for the current query.
fn schedule_search(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let (seq, query, owners) = with_host(cx, |host| {
        host.clone.seq = host.clone.seq.wrapping_add(1);
        host.clone.searching = !host.clone.query.is_empty();
        (
            host.clone.seq,
            host.clone.query.value().to_owned(),
            host.clone.owners.clone(),
        )
    });
    notify(state, cx);
    if query.trim().is_empty() || owners.is_empty() {
        with_host(cx, |host| {
            if host.clone.seq == seq {
                host.clone.results.clear();
                host.clone.searching = false;
            }
        });
        notify(state, cx);
        return;
    }
    let state = state.clone();
    let bridge = bridge.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(DEBOUNCE).await;
        if cx.update(|cx| with_host(cx, |host| host.clone.seq != seq)) {
            return;
        }
        let mut results: Vec<RemoteRepo> = Vec::new();
        let mut error: Option<String> = None;
        let mut cached_at: Option<String> = None;
        for owner in owners {
            let reply = bridge.request(RequestBody::SearchRemoteRepos {
                owner,
                query: query.clone(),
            });
            match reply.recv().await {
                Ok(Ok(ResponseBody::RemoteRepos(cache))) => {
                    let RepoCache { fetched_at, repos } = cache;
                    cached_at.get_or_insert(fetched_at);
                    results.extend(repos);
                }
                Ok(Err(failure)) => error = Some(failure.message),
                Ok(Ok(_)) => {}
                Err(_) => return,
            }
        }
        results.truncate(RESULT_ROWS);
        cx.update(|cx| {
            let live = with_host(cx, |host| {
                if host.clone.seq != seq {
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
    })
    .detach();
}

// ---------------------------------------------------------------------------- rendering

/// Renders the dialog (§3.8.2).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.md;
    let draft = with_host(cx, |host| host.clone.clone());
    let offline = !state.read(cx).daemon.is_connected();
    let now = now_epoch();
    let rows = draft.rows();

    let search_field = TextField::new(draft.query.value().to_owned())
        .placeholder("Type to search GitHub repos in this context's owners.")
        .icon(if draft.searching {
            Icon::LoaderCircle
        } else {
            Icon::Search
        })
        .caret(draft.query.caret())
        .focused(true);

    let empty: AnyElement = if let Some(message) = draft.error.clone() {
        div()
            .flex()
            .flex_col()
            .child(Text::ui(message).tone(Tone::Danger).ellipsize())
            .child(KeyHintRow::new().key("r", "retry"))
            .into_any_element()
    } else if draft.query.is_empty() {
        Text::ui(format!(
            "Type to search GitHub repos in {}'s owners.",
            if draft.context_name.is_empty() {
                "this context"
            } else {
                draft.context_name.as_str()
            }
        ))
        .muted()
        .into_any_element()
    } else if draft.searching {
        Text::ui("Searching\u{2026}").muted().into_any_element()
    } else {
        Text::ui(format!("Nothing matches \"{}\".", draft.query.value()))
            .muted()
            .into_any_element()
    };

    let list = FuzzyList::new(rows.iter().map(|repo| {
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
    .empty(empty);

    let mut body = div().flex().flex_col().gap(gap).child(search_field);
    if offline && let Some(cached) = draft.cached_at.as_ref() {
        body = body
            .child(Text::hint(format!("cached {}", age_label(cached, now))).tone(Tone::Warning));
    }
    let body = body.child(list);

    let card = Dialog::new("Clone repo")
        .icon(Icon::CloudDownload)
        .width(super::Dialogs::CloneRepo.width())
        .height(px(420.0))
        .subtitle(format!("\u{00b7} into context \"{}\"", draft.context_name))
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key("\u{2303}n/\u{2303}p", "move")
                .key("esc", "cancel")
                .key(draft.protocol_word(), "clones in the background"),
        )
        .primary("\u{23ce} Clone");

    let confirm_state = state.clone();
    let confirm_bridge = bridge.clone();

    root(focus)
        .on_key_down({
            let state = state.clone();
            let bridge = bridge.clone();
            move |event, _window, cx| {
                if with_host(cx, |host| type_into(&mut host.clone.query, event)) {
                    schedule_search(&state, &bridge, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &dialog::Backspace, _window, cx| {
                if with_host(cx, |host| host.clone.query.backspace()) {
                    schedule_search(&state, &bridge, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &dialog::DeleteWord, _window, cx| {
                if with_host(cx, |host| host.clone.query.delete_word()) {
                    schedule_search(&state, &bridge, cx);
                }
            }
        })
        .on_action({
            let state = state.clone();
            let bridge = bridge.clone();
            move |_: &dialog::ClearInput, _window, cx| {
                if with_host(cx, |host| host.clone.query.clear()) {
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
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineStart, _window, cx| {
                with_host(cx, |host| host.clone.query.home());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::LineEnd, _window, cx| {
                with_host(cx, |host| host.clone.query.end());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorLeft, _window, cx| {
                with_host(cx, |host| host.clone.query.left());
                notify(&state, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorRight, _window, cx| {
                with_host(cx, |host| host.clone.query.right());
                notify(&state, cx);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            submit(&confirm_state, &confirm_bridge, cx);
        })
        .on_action(move |_: &dialog::Cancel, _window, cx| {
            // §3.8.2: `Esc` aborts the search request only. Bumping the sequence orphans the
            // pending answer; a `CloneJob` already accepted by the daemon is untouched.
            with_host(cx, |host| {
                host.clone.seq = host.clone.seq.wrapping_add(1);
                host.clone.searching = false;
            });
            cx.propagate();
        })
        .child(card)
        .into_any_element()
}

fn move_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(cx, |host| {
        let len = host.clone.rows().len();
        host.clone.cursor = step(host.clone.cursor, delta, len);
    });
    notify(state, cx);
}

/// `Enter`: hand the clone to the daemon and close; the rail shows a `⟳` row from the moment
/// the job is persisted (§3.8.2).
fn submit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some((repo, context)) = with_host(cx, |host| {
        Some((host.clone.selected()?, host.clone.context.clone()?))
    }) else {
        return;
    };
    let protocol = with_host(cx, |host| host.clone.protocol);
    let url = clone_url(&repo, protocol);
    bridge.send(RequestBody::CloneRepo {
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
    use super::*;

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
            query: TextInput::new("bukhr/payroll"),
            results: vec![repo("bukhr/payroll")],
            ..CloneState::default()
        };
        assert_eq!(state.rows().len(), 1);
    }

    #[test]
    fn results_are_capped_at_eight_rows() {
        let state = CloneState {
            query: TextInput::new("pay"),
            results: (0..12).map(|n| repo(&format!("acme/pay{n}"))).collect(),
            ..CloneState::default()
        };
        assert_eq!(state.rows().len(), RESULT_ROWS);
        assert_eq!(
            state.selected().map(|repo| repo.full_name),
            Some("acme/pay0".to_owned())
        );
    }
}
