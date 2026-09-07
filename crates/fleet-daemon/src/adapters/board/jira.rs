//! A board backed by one Jira project, driven entirely through the Atlassian CLI.
//!
//! No HTTP client and no API token: every call is an `acli` invocation through the daemon's
//! `Shell` adapter (`docs/BOARD-JIRA.md` §7 lists them). What that CLI cannot do is what shapes
//! the backend — `search` returns seven fields, so a pull costs one `view` per key; `edit`
//! writes four, so priority, estimate, due date and parent are declared read-only and the core
//! refuses local edits to them; transitions move by status *name*, so statuses are name-keyed.

use super::BoardBackend;
use crate::adapters::{clock::Clock, shell::Shell};
use fleet_core::board::{
    BackendCapabilities, BackendSchema, Board, BoardError, Card, PropertyKind, PropertyOption,
    PropertySchema, PropertySource, PullResult, PushAck, PushFailure, PushOp, PushResult,
    RemoteStatus, StatusCategory,
};
use futures_util::StreamExt;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub mod acli;
pub mod adf;
pub mod map;
mod push;
mod schema;
pub mod settings;
#[cfg(test)]
mod tests;
pub mod users;

pub use settings::{ExtraField, JiraSettings};
pub use users::{JiraUser, UserCache};

use acli::Acli;
use map::{
    Cursor, build_search_jql, issue_to_remote_card, parse_cursor, render_cursor, view_fields,
};

/// What one push batch knows: every card as it stood before the push, and the keys the batch has
/// minted so far.
///
/// The two travel together because `create` needs both: `cards` is the pre-push snapshot, so a
/// parent created earlier in this same batch still reads as unlinked there and its key lives
/// only in `minted`.
#[derive(Clone, Copy)]
struct Batch<'a> {
    cards: &'a [Card],
    minted: &'a HashMap<fleet_core::ids::CardId, String>,
}

/// The card fields `acli jira workitem edit` cannot write, so no local edit to them may stand.
pub const READONLY_FIELDS: &[&str] = &["priority", "estimate", "due_date", "parent_id"];

/// The card fields one `workitem edit` invocation can carry.
const WRITABLE_FIELDS: &[&str] = &["title", "description", "labels", "assignee"];

/// Mirrors one Jira project into a board.
pub struct JiraBackend {
    shell: Arc<dyn Shell>,
    clock: Arc<dyn Clock>,
    users: Arc<UserCache>,
    /// The site `acli` is signed in to, learned once and kept for the life of the daemon.
    ///
    /// One `acli` account per machine, so this is a property of the process, not of a board.
    site: std::sync::RwLock<Option<String>>,
}

impl JiraBackend {
    /// Creates the backend over the adapters it shells out and stamps times through.
    #[must_use]
    pub fn new(shell: Arc<dyn Shell>, clock: Arc<dyn Clock>) -> Self {
        Self {
            shell,
            clock,
            users: Arc::new(UserCache::default()),
            site: std::sync::RwLock::new(None),
        }
    }

    /// Reads one board's settings, naming the offending key when they do not parse.
    fn settings(board: &Board) -> Result<JiraSettings, BoardError> {
        JiraSettings::parse(&board.backend.settings)
    }

    /// [`JiraBackend::settings`], with `site` defaulted to the one `acli` is signed in to.
    ///
    /// `site` is optional and only `browse_url` reads it, so a board created without
    /// `--setting site=…` gave every one of its cards `url: null` forever: `board card show`
    /// printed no `URL:` line and the app's `x` could never open anything. The signed-in site
    /// is the same one `validate` already refuses a mismatch against, so taking it as the
    /// default cannot point a board at somebody else's Jira.
    async fn located_settings(&self, board: &Board) -> Result<JiraSettings, BoardError> {
        let mut settings = Self::settings(board)?;
        if settings
            .site
            .as_deref()
            .map(str::trim)
            .is_some_and(|site| !site.is_empty())
        {
            return Ok(settings);
        }
        if let Ok(cached) = self.site.read()
            && let Some(site) = cached.clone()
        {
            settings.site = Some(site);
            return Ok(settings);
        }
        // A site nobody can name is not a reason to fail the sync: the cards simply keep the
        // `url: null` they have today.
        match Acli::new(self.shell.clone(), 1).auth_status().await {
            Ok(status) => {
                if let Some(site) = status.site.filter(|site| !site.trim().is_empty()) {
                    if let Ok(mut cached) = self.site.write() {
                        *cached = Some(site.clone());
                    }
                    settings.site = Some(site);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not read the site acli is signed in to");
            }
        }
        Ok(settings)
    }

    /// A runner bounded by what this board asked for.
    fn acli(&self, settings: &JiraSettings) -> Acli {
        Acli::new(self.shell.clone(), settings.max_concurrency)
    }

    /// Files every account an issue payload named, so a later push can address it.
    fn remember_users(&self, scope: &str, issue: &serde_json::Value) {
        for (display_name, account_id, email) in map::issue_users(issue) {
            self.users.remember(
                scope,
                JiraUser {
                    account_id,
                    email,
                    display_name,
                },
            );
        }
    }
}

#[async_trait::async_trait]
impl BoardBackend for JiraBackend {
    fn kind(&self) -> &'static str {
        "jira"
    }
    fn label(&self) -> &'static str {
        "Jira (acli)"
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            pull: true,
            push_updates: true,
            push_create: true,
            transitions: true,
            comments: true,
            custom_properties: true,
            incremental: true,
        }
    }
    fn settings_schema(&self) -> Vec<PropertySchema> {
        settings::settings_schema()
    }

    async fn validate(&self, settings: &serde_json::Value) -> Result<(), BoardError> {
        let settings = JiraSettings::parse(settings)?;
        let acli = Acli::new(self.shell.clone(), 1);
        acli.version().await?;
        let status = acli.auth_status().await?;
        if !status.authenticated {
            return Err(BoardError::Backend(acli::NOT_AUTHENTICATED.to_owned()));
        }
        if let (Some(wanted), Some(signed_in)) = (settings.site.as_deref(), status.site.as_deref())
            && host(wanted) != host(signed_in)
        {
            // One `acli` account per machine: a board pointing elsewhere would silently
            // mirror the wrong site's issues under the right project key.
            return Err(BoardError::Invalid {
                field: "site".into(),
                reason: format!("acli is signed in to {signed_in}"),
            });
        }
        Ok(())
    }

    async fn normalize(
        &self,
        settings: &serde_json::Value,
    ) -> Result<serde_json::Value, BoardError> {
        // Only `jql` is rewritten, and only the keys the caller supplied are touched: `parse`
        // fills every optional field with its default, and writing those back would turn each
        // unset row of the settings dialog into a value the user never chose.
        let parsed = JiraSettings::parse(settings)?;
        let mut object = match settings {
            serde_json::Value::Object(object) => object.clone(),
            _ => serde_json::Map::new(),
        };
        if object.contains_key("jql") {
            match parsed.jql {
                Some(jql) => {
                    object.insert("jql".to_owned(), serde_json::Value::String(jql));
                }
                // A filter that was nothing but an `ORDER BY` clause is no filter at all, and
                // leaving the original behind would show one every search ignores.
                None => {
                    object.remove("jql");
                }
            }
        }
        Ok(serde_json::Value::Object(object))
    }

    async fn describe(&self, board: &Board) -> Result<BackendSchema, BoardError> {
        // `located_settings`, like `pull` and `push`: `users::scope` is the site when there is
        // one and the project key otherwise, so describing a site-less board under the raw
        // settings filed every account it sampled under a scope no push ever reads — and the
        // sample exists for exactly the push that then failed with "unknown assignee".
        let settings = self.located_settings(board).await?;
        let acli = self.acli(&settings);
        let scope = users::scope(&settings);
        let mut labels: Vec<String> = Vec::new();
        let mut assignees: Vec<String> = Vec::new();
        let statuses = if settings.statuses.is_empty() {
            // Jira cannot list a project's statuses, so the board's columns are whatever the
            // most recently touched issues are actually in.
            let jql = format!("{} ORDER BY updated DESC", settings.base_jql());
            let limit = settings.sample_limit.to_string();
            let sample = acli
                .json(&[
                    "workitem",
                    "search",
                    "--jql",
                    &jql,
                    "--fields",
                    "status,labels,assignee",
                    "--limit",
                    &limit,
                    "--json",
                ])
                .await?;
            let mut seen: Vec<RemoteStatus> = Vec::new();
            for issue in issues(&sample) {
                self.remember_users(&scope, issue);
                let fields = issue.get("fields").unwrap_or(issue);
                if let Some(name) = fields
                    .get("status")
                    .and_then(|status| status.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    && !seen.iter().any(|status| status.name == name)
                {
                    let category = fields
                        .get("status")
                        .and_then(|status| status.get("statusCategory"))
                        .and_then(|category| category.get("key"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    seen.push(RemoteStatus {
                        id: name.to_owned(),
                        name: name.to_owned(),
                        category: Some(map::status_category(category, name)),
                    });
                }
                for label in fields
                    .get("labels")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|label| label.as_str())
                {
                    remember(&mut labels, label);
                }
                if let Some(assignee) = fields
                    .get("assignee")
                    .and_then(|assignee| assignee.get("displayName"))
                    .and_then(serde_json::Value::as_str)
                {
                    remember(&mut assignees, assignee);
                }
            }
            // Columns read left to right as work does; within a category, the order the
            // sample first showed them.
            seen.sort_by_key(|status| status.category.unwrap_or(StatusCategory::Unstarted));
            seen
        } else {
            // The columns are the user's, but the people and the labels are not: `edit
            // --assignee` wants an account id, and the only place this backend ever learns one
            // is an issue payload. Skipping the sample here leaves a restarted daemon unable to
            // assign anyone whose issues fall outside the first incremental window — and that
            // failure aborts the whole sync, not just the one card's push.
            self.sample_people(&acli, &settings, &scope, &mut labels, &mut assignees)
                .await;
            settings
                .statuses
                .iter()
                .map(|name| RemoteStatus {
                    id: name.clone(),
                    name: name.clone(),
                    category: Some(map::guess_category(name)),
                })
                .collect()
        };
        let mut properties = vec![
            PropertySchema {
                key: map::ISSUE_TYPE_KEY.to_owned(),
                name: "Issue type".to_owned(),
                kind: PropertyKind::Select,
                options: self.issue_types(&acli, &settings).await,
                // `acli` cannot change an issue's type after it exists.
                editable: false,
                source: PropertySource::Backend,
                show_on_card: false,
            },
            PropertySchema {
                key: map::CREATED_KEY.to_owned(),
                name: "Created".to_owned(),
                kind: PropertyKind::Date,
                options: Vec::new(),
                editable: false,
                source: PropertySource::Backend,
                show_on_card: false,
            },
        ];
        properties.extend(settings.extra_fields.iter().map(|extra| PropertySchema {
            key: map::extra_field_key(extra.id.trim()),
            name: extra.name.clone(),
            kind: extra.kind,
            options: Vec::new(),
            editable: false,
            source: PropertySource::Backend,
            show_on_card: false,
        }));
        labels.sort();
        assignees.sort();
        Ok(BackendSchema {
            statuses,
            labels,
            properties,
            assignees,
            key_prefix: Some(settings.project.trim().to_owned()),
            readonly_fields: READONLY_FIELDS
                .iter()
                .map(|field| (*field).to_owned())
                .collect(),
        })
    }

    async fn pull(&self, board: &Board, cursor: Option<&str>) -> Result<PullResult, BoardError> {
        let settings = self.located_settings(board).await?;
        let acli = self.acli(&settings);
        let previous = parse_cursor(cursor);
        // Every Nth pull is full: an incremental JQL window can never see an issue that was
        // deleted, moved out of the project, or edited out of the board's filter.
        let started = self.clock.now();
        let elapsed = started
            .signed_duration_since(previous.watermark)
            .num_minutes();
        // A watermark in the future is a clock that moved backwards or a restored `~/.fleet`;
        // an incremental window computed from it reaches back zero minutes and hides every
        // change made in between until the next full pull comes due.
        // `pulls_since_full` counts the *incremental* pulls since the last full one, so the
        // Nth of them is the one that comes due: `>= full_sync_every` on its own made every
        // N+1st pull full, one short of what `fullSyncEvery = N` says on the tin.
        let full = cursor.is_none()
            || previous.pulls_since_full.saturating_add(1) >= settings.full_sync_every
            || elapsed < 0;
        let since = (!full).then(|| {
            u32::try_from(elapsed.max(0))
                .unwrap_or(u32::MAX)
                .saturating_add(settings.overlap_minutes)
        });
        let jql = build_search_jql(&settings, since);
        // `json_all`, not `json`: the search is the one paginated call, and a CLI that answers
        // `--paginate` with one document per page rather than one merged array would have every
        // key past the first page dropped by a parser that stops at the first value — which on a
        // full pull is a card archived for a deletion that never happened.
        let pages = acli
            .json_all(&[
                "workitem",
                "search",
                "--jql",
                &jql,
                "--fields",
                "summary",
                "--paginate",
                "--json",
            ])
            .await?;
        let mut keys: Vec<String> = Vec::new();
        for issue in pages.iter().flat_map(issues) {
            // Deduplicated: pages that overlap must not make this one `view` per repetition.
            if let Some(key) = issue.get("key").and_then(serde_json::Value::as_str)
                && !keys.iter().any(|known| known == key)
            {
                keys.push(key.to_owned());
            }
        }
        // `search` answers seven fields and none of the ones a card needs, so this is one
        // `view` per key — bounded, because a sprint's worth of them would trip the site's
        // rate limit long before it finished.
        let fields = view_fields(&settings);
        let scope = users::scope(&settings);
        let views: Vec<_> = keys
            .iter()
            .map(|key| {
                let acli = &acli;
                let fields = fields.as_str();
                async move {
                    let issue = acli
                        .json_optional(&["workitem", "view", key, "--fields", fields, "--json"])
                        .await;
                    // The key travels with its own outcome: a failed `view` is reported per
                    // key, never as the failure of the pull that listed it.
                    (key.clone(), issue)
                }
            })
            .collect();
        let viewed: Vec<(String, Result<Option<serde_json::Value>, BoardError>)> =
            futures_util::stream::iter(views)
                .buffered(settings.max_concurrency.clamp(1, 8))
                .collect()
                .await;
        let mut cards = Vec::new();
        let mut deleted_keys = Vec::new();
        // One issue that will not load — a rate limit that outlived the backoff, a field this
        // account cannot read — used to discard every card that did load and leave the cursor
        // where it was, so a project big enough to hit it could never finish a pull again.
        // The key is reported and kept: `reconcile` neither updates nor archives it.
        let mut failed_keys = Vec::new();
        for (key, outcome) in viewed {
            match outcome {
                Ok(Some(issue)) => {
                    self.remember_users(&scope, &issue);
                    match issue_to_remote_card(&issue, &settings) {
                        Ok(card) => cards.push(card),
                        Err(error) => failed_keys.push(format!("{key}: {}", message_of(error))),
                    }
                }
                // Between the search and the view the issue stopped existing, which on a full
                // pull the absent card would have said anyway — but not on an incremental one.
                Ok(None) => deleted_keys.push(key),
                Err(error) => failed_keys.push(format!("{key}: {}", message_of(error))),
            }
        }
        // Nothing readable at all is the backend failing, not a partial answer.
        if cards.is_empty()
            && deleted_keys.is_empty()
            && let Some(first) = failed_keys.first()
        {
            return Err(BoardError::Backend(format!(
                "none of the {} issues this pull listed could be read: {first}",
                keys.len()
            )));
        }
        // Every key the search just listed coming back missing is not a Jira event: it is this
        // account losing sight of the project, or a `--fields` the site refuses. Reporting it
        // as a deletion archives the whole board and calls the sync a success.
        if cards.is_empty() && keys.len() > 1 && deleted_keys.len() == keys.len() {
            return Err(BoardError::Backend(format!(
                "all {} issues this pull listed came back missing; refusing to report them as deleted",
                keys.len()
            )));
        }
        Ok(PullResult {
            cards,
            deleted_keys,
            // A pull that could not read every key it listed must not move the watermark past
            // them: the next one has to offer the same window again. The *counter* still moves,
            // and this is why: rewinding the whole cursor left `pulls_since_full` frozen, so one
            // permanently unreadable key meant no full pull ever ran again — remote deletions
            // stopped being noticed for the life of that key, while the window grew without
            // bound and every sync re-`view`ed an ever-larger key set.
            cursor: match (failed_keys.is_empty(), cursor) {
                // The watermark is when the pull started, never when it finished: an issue
                // edited while it ran must be seen again by the next one.
                (true, _) => Some(render_cursor(&Cursor {
                    watermark: started,
                    pulls_since_full: if full {
                        0
                    } else {
                        previous.pulls_since_full.saturating_add(1)
                    },
                })),
                // Keys were missed and there is no earlier watermark to hold: the epoch one
                // `parse_cursor` invents would turn the next pull into an unbounded window.
                // A cursor that is *unreadable* is that same case wearing a string — it too
                // yields the invented epoch — and persisting it would both send
                // `updated >= "-~29000000m"` on every later pull and spend the `u32::MAX`
                // counter that says a full pull is overdue.
                (false, None) => None,
                (false, Some(_)) if previous.pulls_since_full == u32::MAX => None,
                (false, Some(_)) => Some(render_cursor(&Cursor {
                    watermark: previous.watermark,
                    pulls_since_full: if full {
                        0
                    } else {
                        previous.pulls_since_full.saturating_add(1)
                    },
                })),
            },
            full,
            failed_keys,
        })
    }

    async fn push(
        &self,
        board: &Board,
        cards: &[Card],
        ops: &[PushOp],
    ) -> Result<PushResult, BoardError> {
        let settings = self.located_settings(board).await?;
        // One permit: writes are never parallel, so two edits of the same issue cannot race
        // and a rate limit hit halfway leaves a prefix of the batch applied, not a scatter.
        let acli = Acli::new(self.shell.clone(), 1);
        let mut grouped: Vec<(&fleet_core::ids::CardId, Vec<&PushOp>)> = Vec::new();
        for op in ops {
            let card_id = match op {
                PushOp::Create { card_id }
                | PushOp::Update { card_id, .. }
                | PushOp::Transition { card_id, .. }
                | PushOp::AddComment { card_id, .. } => card_id,
            };
            match grouped.iter_mut().find(|(id, _)| *id == card_id) {
                Some((_, ops)) => ops.push(op),
                None => grouped.push((card_id, vec![op])),
            }
        }
        // A child whose parent this same batch is about to create can only carry `parentIssueId`
        // if the parent already has a key, so parents go first. The depth is counted over the
        // cards that are still unlinked, which is exactly the set this batch mints keys for.
        grouped.sort_by_key(|(card_id, _)| unlinked_depth(cards, card_id));
        let mut result = PushResult::default();
        let mut minted: HashMap<fleet_core::ids::CardId, String> = HashMap::new();
        for (card_id, ops) in grouped {
            let Some(card) = cards.iter().find(|card| card.id == *card_id) else {
                result.failures.push(PushFailure {
                    card_id: card_id.clone(),
                    error: "card is no longer on this board".to_owned(),
                });
                continue;
            };
            // One card's refusal is that card's refusal: the rest of the batch still runs,
            // and the service reports every failure with the message Jira gave. A card can
            // report both — the operations that landed and the one that did not.
            let (ack, failure) = self
                .push_card(
                    &acli,
                    board,
                    &settings,
                    card,
                    Batch {
                        cards,
                        minted: &minted,
                    },
                    &ops,
                )
                .await;
            if let Some(ack) = ack {
                if card.remote.is_none() {
                    minted.insert(card.id.clone(), ack.key.clone());
                }
                result.acks.push(ack);
            }
            if let Some(error) = failure {
                result.failures.push(PushFailure {
                    card_id: card_id.clone(),
                    error,
                });
            }
        }
        Ok(result)
    }
}

/// The issue list of a `search` answer, whichever envelope it arrived in.
fn issues(value: &serde_json::Value) -> &[serde_json::Value] {
    value
        .as_array()
        .or_else(|| {
            ["issues", "results", "values", "workitems", "data"]
                .iter()
                .find_map(|field| value.get(field).and_then(serde_json::Value::as_array))
        })
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Appends a value the list does not already carry.
fn remember(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && !values.iter().any(|known| known == value) {
        values.push(value.to_owned());
    }
}

/// How many still-unlinked ancestors a card has, so a push batch creates parents first.
///
/// Only unlinked ancestors count: a parent that already has a key needs no ordering. The walk is
/// bounded by the card count, so a `parent_id` cycle a corrupt document smuggled in terminates.
fn unlinked_depth(cards: &[Card], card_id: &fleet_core::ids::CardId) -> usize {
    let mut depth = 0;
    let mut seen = std::collections::BTreeSet::new();
    let mut at = cards.iter().find(|card| card.id == *card_id);
    while let Some(card) = at {
        if !seen.insert(card.id.clone()) {
            break;
        }
        let Some(parent) = card
            .parent_id
            .as_ref()
            .and_then(|id| cards.iter().find(|card| card.id == *id))
        else {
            break;
        };
        if parent.remote.is_some() {
            break;
        }
        depth += 1;
        at = Some(parent);
    }
    depth
}

/// A failure as the push reports it: Jira's own sentence, with no wrapper in front of it.
///
/// `PushFailure.error` is joined into the sync's message and shown verbatim, so a
/// `backend error:` prefix here reads twice by the time a user sees it.
fn message_of(error: BoardError) -> String {
    match error {
        BoardError::Backend(message) => message,
        other => other.to_string(),
    }
}

/// The bare host of a site, however the user spelled it.
fn host(site: &str) -> String {
    site.trim()
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_lowercase()
}
