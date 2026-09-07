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

/// The Atlassian CLI boundary.
pub mod acli;
/// Atlassian Document Format conversion.
pub mod adf;
/// Jira JSON ⇄ core model mapping.
pub mod map;
/// Board settings and their generic schema.
pub mod settings;
/// Display name → account id cache.
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

    /// The issue types a project accepts, for the `jira.issue_type` property's options.
    ///
    /// A site that refuses the project read still has a syncable board, so this is the one
    /// call whose failure is a warning: it costs option labels, not cards.
    async fn issue_types(&self, acli: &Acli, settings: &JiraSettings) -> Vec<PropertyOption> {
        let value = match acli
            .json(&["project", "view", "--key", &settings.project, "--json"])
            .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(project = %settings.project, %error, "could not read the project's issue types");
                return Vec::new();
            }
        };
        value
            .get("issueTypes")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|issue_type| {
                let name = issue_type.get("name")?.as_str()?.trim();
                (!name.is_empty()).then(|| PropertyOption {
                    value: name.to_owned(),
                    label: name.to_owned(),
                    color: None,
                })
            })
            .collect()
    }

    /// Pushes every operation queued for one card, in order, reporting what was applied and
    /// the first operation that was not.
    ///
    /// Writes are sequential by construction: a transition that runs before the edit that
    /// renamed the issue would be reported against a card the user no longer recognizes. The
    /// prefix that did run is acknowledged even when a later operation fails — a comment whose
    /// ack is dropped is posted again by the next sync, and an edit whose new `updated` is
    /// never recorded turns the user's own change into a conflict.
    async fn push_card(
        &self,
        acli: &Acli,
        board: &Board,
        settings: &JiraSettings,
        card: &Card,
        batch: Batch<'_>,
        ops: &[&PushOp],
    ) -> (Option<PushAck>, Option<String>) {
        let mut key = card.remote.as_ref().map(|link| link.key.clone());
        let mut comment_ids = Vec::new();
        let mut failure = None;
        let mut applied = 0_usize;
        // A transition right behind a create is the column the card was born in, not a move the
        // user made: Jira has already filed the issue in its workflow's opening status, and
        // asking it to transition to the status it is in answers "no allowed transitions found"
        // and fails a push that had nothing left to do.
        let mut created_now = false;
        for op in ops {
            let outcome = match op {
                PushOp::Create { .. } => {
                    self.create(acli, board, settings, card, batch)
                        .await
                        .map(|created| {
                            key = Some(created);
                            created_now = true;
                        })
                }
                PushOp::Update { fields, .. } => match require_key(key.as_deref()) {
                    Ok(key) => self.edit(acli, board, settings, card, fields, key).await,
                    Err(error) => Err(error),
                },
                PushOp::Transition { remote_status, .. } => match require_key(key.as_deref()) {
                    Ok(key) => match self.status_of(acli, key, created_now).await {
                        Ok(Some(current)) if current.eq_ignore_ascii_case(remote_status.trim()) => {
                            Ok(())
                        }
                        Ok(_) => match acli
                            .json(&[
                                "workitem",
                                "transition",
                                "--key",
                                key,
                                "--status",
                                remote_status,
                                "--yes",
                                "--json",
                            ])
                            .await
                        {
                            Ok(_) => Ok(()),
                            // A move Jira applied and then failed to answer for — a 504, a
                            // killed call, the retry of either — comes back as "no allowed
                            // transitions found": a refusal to move an issue that is already
                            // where it was asked to go. Reporting that as a push failure
                            // abandons the rest of this card's batch, and names an error for
                            // a transition that landed. What Jira holds now settles it.
                            Err(error) => match self.current_status(acli, key).await {
                                Ok(Some(current))
                                    if current.eq_ignore_ascii_case(remote_status.trim()) =>
                                {
                                    Ok(())
                                }
                                _ => Err(error),
                            },
                        },
                        Err(error) => Err(error),
                    },
                    Err(error) => Err(error),
                },
                PushOp::AddComment { comment_id, .. } => match require_key(key.as_deref()) {
                    Ok(key) => {
                        let minted: Vec<String> = comment_ids
                            .iter()
                            .map(|(_, remote): &(String, String)| remote.clone())
                            .collect();
                        self.comment(acli, card, comment_id, key, &minted)
                            .await
                            .map(|remote| comment_ids.push((comment_id.clone(), remote)))
                    }
                    Err(error) => Err(error),
                },
            };
            match outcome {
                Ok(()) => applied += 1,
                Err(error) => {
                    // The rest of this card's batch is abandoned: an edit applied after a
                    // failed create has nothing to write to, and a transition after a failed
                    // edit reports a card the user no longer recognizes.
                    failure = Some(message_of(error));
                    break;
                }
            }
        }
        // Nothing landed, so there is nothing to acknowledge and no version worth a call: the
        // failure is the whole answer. One operation landing is what makes the ack mandatory —
        // an unacknowledged comment is posted again, and an unacknowledged edit becomes a
        // conflict against the user's own change.
        let (Some(key), true) = (key, applied > 0) else {
            return (None, failure);
        };
        // One view closes the card: `updated` is the version every later conflict check
        // compares against, and reading it back is the only way to learn the one Jira kept.
        // It is also the remote's own stamp, so it fills `remoteUpdatedAt` too — a card fleet
        // just created has no earlier link to inherit one from.
        let version = match acli
            .json(&["workitem", "view", &key, "--fields", "updated", "--json"])
            .await
        {
            Ok(issue) => issue
                .get("fields")
                .unwrap_or(&issue)
                .get("updated")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            Err(error) => {
                // Without a version the service fetches the baseline itself; the write that
                // did land must still be acknowledged.
                failure.get_or_insert_with(|| message_of(error));
                None
            }
        };
        (
            Some(PushAck {
                card_id: card.id.clone(),
                url: settings.browse_url(&key),
                key,
                // The same guard the pull path puts on `updated`, for the same reason: this
                // value is persisted as `remoteUpdatedAt` and published as RFC3339 by
                // `board show --json`, and `normalize_time` hands back whatever it was given
                // when it does not parse.
                remote_updated_at: version
                    .as_deref()
                    .filter(|updated| map::parse_time(updated).is_some())
                    .map(map::normalize_time),
                version,
                comment_ids,
            }),
            failure,
        )
    }

    /// Fills the account cache and the label list from the board's most recently touched issues.
    ///
    /// A board whose columns are configured by hand never samples for statuses, so this is the
    /// only thing that teaches a cold daemon the accounts its next push has to address. It costs
    /// one `search`, and a site that refuses it still has a syncable board.
    async fn sample_people(
        &self,
        acli: &Acli,
        settings: &JiraSettings,
        scope: &str,
        labels: &mut Vec<String>,
        assignees: &mut Vec<String>,
    ) {
        let jql = format!("{} ORDER BY updated DESC", settings.base_jql());
        let limit = settings.sample_limit.to_string();
        let sample = match acli
            .json(&[
                "workitem",
                "search",
                "--jql",
                &jql,
                "--fields",
                "labels,assignee",
                "--limit",
                &limit,
                "--json",
            ])
            .await
        {
            Ok(sample) => sample,
            Err(error) => {
                tracing::warn!(%error, "could not sample this board's people and labels");
                return;
            }
        };
        for issue in issues(&sample) {
            self.remember_users(scope, issue);
            let fields = issue.get("fields").unwrap_or(issue);
            for label in fields
                .get("labels")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(|label| label.as_str())
            {
                remember(labels, label);
            }
            if let Some(assignee) = fields
                .get("assignee")
                .and_then(|assignee| assignee.get("displayName"))
                .and_then(serde_json::Value::as_str)
            {
                remember(assignees, assignee);
            }
        }
    }

    /// The status name an issue is in, read back only when a create in this batch just chose it.
    ///
    /// Every other transition comes from a move the user made against a status a pull reported,
    /// and is worth the round trip it saves.
    async fn status_of(
        &self,
        acli: &Acli,
        key: &str,
        created_now: bool,
    ) -> Result<Option<String>, BoardError> {
        if !created_now {
            return Ok(None);
        }
        self.current_status(acli, key).await
    }

    /// The status name Jira currently holds for `key`.
    async fn current_status(&self, acli: &Acli, key: &str) -> Result<Option<String>, BoardError> {
        let issue = acli
            .json(&["workitem", "view", key, "--fields", "status", "--json"])
            .await?;
        Ok(issue
            .get("fields")
            .unwrap_or(&issue)
            .get("status")
            .and_then(|status| status.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(|name| name.trim().to_owned()))
    }

    /// Applies the writable half of a card's changed fields with one `edit`.
    async fn edit(
        &self,
        acli: &Acli,
        board: &Board,
        settings: &JiraSettings,
        card: &Card,
        fields: &[String],
        key: &str,
    ) -> Result<(), BoardError> {
        let unsupported: Vec<&str> = fields
            .iter()
            .map(String::as_str)
            .filter(|field| !WRITABLE_FIELDS.contains(field))
            .collect();
        if !unsupported.is_empty() {
            // The core refuses local edits to the read-only fields, so anything landing here
            // is a field Jira owns (properties, or a status pushed as a transition instead).
            tracing::warn!(
                key,
                fields = unsupported.join(", "),
                "acli cannot write these fields; leaving them to Jira"
            );
        }
        let wants = |field: &str| fields.iter().any(|name| name == field);
        // The order is fixed rather than the caller's, so one board edit is one argv.
        let mut args = vec![
            "workitem".to_owned(),
            "edit".to_owned(),
            "--key".to_owned(),
            key.to_owned(),
        ];
        let _description = if wants("description") {
            let file = TempJson::new(&adf::markdown_to_adf(&card.description)).await?;
            args.push("--description-file".to_owned());
            args.push(file.argument());
            Some(file)
        } else {
            None
        };
        if wants("title") {
            args.push("--summary".to_owned());
            args.push(card.title.clone());
        }
        if wants("labels") {
            let local: Vec<String> = card
                .labels
                .iter()
                .filter_map(|id| board.labels.iter().find(|label| label.id == *id))
                .map(|label| label.name.clone())
                .collect();
            // `edit` adds and removes labels; it never replaces the set, so the current one
            // has to be read back before a removal can be named.
            let current = acli
                .json(&["workitem", "view", key, "--fields", "labels", "--json"])
                .await?;
            let current: Vec<String> = current
                .get("fields")
                .unwrap_or(&current)
                .get("labels")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(|label| label.as_str().map(str::to_owned))
                .collect();
            let added: Vec<&String> = local
                .iter()
                .filter(|label| !current.contains(label))
                .collect();
            let removed: Vec<&String> = current
                .iter()
                .filter(|label| !local.contains(label))
                .collect();
            // Only what this push is about to write: a label Jira already holds and fleet is
            // not touching must not fail the edit of a title.
            for label in added.iter().chain(removed.iter()) {
                check_label(label)?;
            }
            if !added.is_empty() {
                args.push("--labels".to_owned());
                args.push(join(&added));
            }
            if !removed.is_empty() {
                args.push("--remove-labels".to_owned());
                args.push(join(&removed));
            }
        }
        if wants("assignee") {
            match card.assignee.as_deref() {
                Some(assignee) => {
                    args.push("--assignee".to_owned());
                    args.push(self.resolve_assignee(settings, assignee)?);
                }
                None => args.push("--remove-assignee".to_owned()),
            }
        }
        if args.len() == 4 {
            // Nothing writable changed; the ack still refreshes the baseline, and an `edit`
            // with no field would only make Jira ask what to change.
            return Ok(());
        }
        args.push("--yes".to_owned());
        args.push("--json".to_owned());
        acli.json(&borrowed(&args)).await?;
        Ok(())
    }

    /// Creates the remote issue a local-only card is missing.
    async fn create(
        &self,
        acli: &Acli,
        board: &Board,
        settings: &JiraSettings,
        card: &Card,
        batch: Batch<'_>,
    ) -> Result<String, BoardError> {
        let issue_type = settings
            .issue_type
            .as_deref()
            .map(str::trim)
            .filter(|issue_type| !issue_type.is_empty())
            .ok_or_else(|| BoardError::Invalid {
                field: "issueType".into(),
                reason: "set an issue type before this board can create Jira issues".into(),
            })?;
        let mut payload = serde_json::json!({
            "projectKey": settings.project.trim(),
            "type": issue_type,
            "summary": card.title,
        });
        if !card.description.trim().is_empty() {
            payload["description"] = adf::markdown_to_adf(&card.description);
        }
        let labels: Vec<String> = card
            .labels
            .iter()
            .filter_map(|id| board.labels.iter().find(|label| label.id == *id))
            .map(|label| label.name.clone())
            .collect();
        for label in &labels {
            check_label(label)?;
        }
        if !labels.is_empty() {
            payload["labels"] = serde_json::json!(labels);
        }
        if let Some(assignee) = card.assignee.as_deref() {
            payload["assignee"] = serde_json::json!(self.resolve_assignee(settings, assignee)?);
        }
        // `cards` is the pre-push snapshot, so a parent created earlier in this same batch still
        // reads as unlinked there and its key lives only in `minted`. Without it the hierarchy
        // is dropped in silence and never recoverable: `parent_id` is read-only, so no later
        // `Update` can carry it, and the next pull clears the child's parent to match.
        if let Some(parent_key) = card.parent_id.as_ref().and_then(|id| {
            batch.minted.get(id).cloned().or_else(|| {
                batch
                    .cards
                    .iter()
                    .find(|card| card.id == *id)
                    .and_then(|parent| parent.remote.as_ref())
                    .map(|link| link.key.clone())
            })
        }) {
            payload["parentIssueId"] = serde_json::json!(parent_key);
        }
        let file = TempJson::new(&payload).await?;
        // Never retried: a create Jira accepted and failed to answer files a second issue.
        let created = acli
            .json_once(&[
                "workitem",
                "create",
                "--from-json",
                &file.argument(),
                "--json",
            ])
            .await?;
        ["key", "issueKey"]
            .iter()
            .find_map(|field| {
                created
                    .get(field)
                    .or_else(|| created.get("issue").and_then(|issue| issue.get(field)))
                    .and_then(serde_json::Value::as_str)
            })
            .map(str::to_owned)
            .ok_or_else(|| BoardError::Backend("acli created an issue without a key".into()))
    }

    /// Publishes one local comment and returns the id Jira filed it under.
    ///
    /// `minted` is the remote ids this batch has already bound to a comment of this card,
    /// which the pre-push `card` snapshot cannot know about.
    async fn comment(
        &self,
        acli: &Acli,
        card: &Card,
        comment_id: &str,
        key: &str,
        minted: &[String],
    ) -> Result<String, BoardError> {
        let comment = card
            .comments
            .iter()
            .find(|comment| comment.id == comment_id)
            .ok_or_else(|| BoardError::Backend(format!("comment {comment_id} is gone")))?;
        let file = TempJson::new(&adf::markdown_to_adf(&comment.body)).await?;
        // Never retried, for the reason `create` is not: a repeated comment is a duplicate.
        let created = acli
            .json_once(&[
                "workitem",
                "comment",
                "create",
                "--key",
                key,
                "--body-file",
                &file.argument(),
                "--json",
            ])
            .await?;
        if let Some(id) = created
            .get("id")
            .or_else(|| created.get("comment").and_then(|comment| comment.get("id")))
            .and_then(serde_json::Value::as_str)
        {
            return Ok(id.to_owned());
        }
        // Without an id in the answer the comment still exists, and a local comment with no
        // remote id is pushed again on the next sync: read it back and match it by body.
        let issue = acli
            .json(&["workitem", "view", key, "--fields", "comment", "--json"])
            .await?;
        let comments = issue
            .get("fields")
            .unwrap_or(&issue)
            .get("comment")
            .and_then(|comment| comment.get("comments"))
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        comments
            .iter()
            .rev()
            .find(|remote| {
                remote
                    .get("body")
                    .map(adf::adf_plain_text)
                    .is_some_and(|body| {
                        body.trim()
                            == adf::adf_plain_text(&adf::markdown_to_adf(&comment.body)).trim()
                    })
            })
            // The fallback never reaches a comment this card is already holding: those are
            // somebody else's, bound to a local comment of their own, and binding a second
            // local comment to the same id makes the next pull merge one body onto both.
            // Failing outright is not the alternative — an unacknowledged comment is posted
            // again by every later sync, which duplicates it in Jira without bound.
            .or_else(|| {
                // `card` is the pre-push snapshot, so the ids this same batch already minted
                // are not on it: two id-less `comment create`s on one card both matched the
                // same remote comment, which is precisely the merge this fallback exists to
                // avoid.
                let known: std::collections::BTreeSet<&str> = card
                    .comments
                    .iter()
                    .filter_map(|comment| comment.remote_id.as_deref())
                    .chain(minted.iter().map(String::as_str))
                    .collect();
                comments.iter().rev().find(|remote| {
                    remote
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| !known.contains(id))
                })
            })
            .and_then(|remote| remote.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                BoardError::Backend(format!("acli filed the comment on {key} without an id"))
            })
    }

    /// Turns whatever the card calls its assignee into something `--assignee` accepts.
    fn resolve_assignee(
        &self,
        settings: &JiraSettings,
        assignee: &str,
    ) -> Result<String, BoardError> {
        let assignee = assignee.trim();
        if let Some(user) = self.users.resolve(&users::scope(settings), assignee) {
            return Ok(user.account_id);
        }
        // A user who typed an email or an account id typed something Jira takes directly; any
        // other unknown word is a display name Jira would reject or silently ignore, and the
        // card would keep an assignee no push ever carries.
        if is_account_handle(assignee) {
            return Ok(assignee.to_owned());
        }
        Err(BoardError::Backend(format!(
            "unknown assignee `{assignee}`: no Jira account with that display name has been seen on this board"
        )))
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

/// A JSON file `acli` reads a body from, removed as soon as the call is over.
struct TempJson {
    path: PathBuf,
}

impl TempJson {
    async fn new(value: &serde_json::Value) -> Result<Self, BoardError> {
        let path = std::env::temp_dir().join(format!("fleet-jira-{}.json", uuid::Uuid::new_v4()));
        // A body that cannot be serialized must fail the push: staging an empty file instead
        // would run `create --from-json` against zero bytes.
        let body = serde_json::to_vec(value).map_err(|error| {
            BoardError::Backend(format!("could not encode a Jira body: {error}"))
        })?;
        // The staged body is the issue description or the comment text; on a shared host the
        // default 0644 publishes it to everyone for the length of the call.
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let staged = |error: std::io::Error| {
            BoardError::Backend(format!("could not stage {}: {error}", path.display()))
        };
        let mut file = options.open(&path).await.map_err(staged)?;
        // The guard owns the path from the moment the path exists: a `write_all` that fails
        // half way would otherwise leave an unlinked file holding a description or a comment
        // body in the system temp directory, with nothing left to remove it.
        let guard = Self { path };
        let write = async {
            tokio::io::AsyncWriteExt::write_all(&mut file, &body).await?;
            tokio::io::AsyncWriteExt::flush(&mut file).await
        };
        write.await.map_err(|error| {
            BoardError::Backend(format!("could not stage {}: {error}", guard.path.display()))
        })?;
        Ok(guard)
    }

    fn argument(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempJson {
    fn drop(&mut self) {
        // Nothing useful can be done about a failed unlink, and a leaked temp file must not
        // turn a successful push into a failed one.
        let _ = std::fs::remove_file(&self.path);
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

/// The comma-separated form `--labels` and `--remove-labels` take.
fn join(labels: &[&String]) -> String {
    labels
        .iter()
        .map(|label| label.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Refuses a label `--labels` cannot carry as one value.
///
/// The flag is comma-separated, so a fleet label spelled `uno,dos` arrives at Jira as the two
/// labels `uno` and `dos` — two labels the user never made, a card that is neither dirty nor
/// conflicted, and a divergence no later sync closes. A space is the same class the other way
/// round: Jira refuses the label outright, and with it the whole `workitem edit`, so every
/// other field in that card's batch is abandoned too. Named here, as one push failure with a
/// sentence that says which label, instead of written wrong or thrown at the site.
fn check_label(label: &str) -> Result<(), BoardError> {
    if let Some(character) = label
        .chars()
        .find(|character| *character == ',' || character.is_whitespace())
    {
        let what = if character == ',' {
            "a comma"
        } else {
            "a space"
        };
        return Err(BoardError::Backend(format!(
            "label `{label}` contains {what}, which Jira labels cannot carry"
        )));
    }
    Ok(())
}

/// Borrows an owned argument vector for one `acli` call.
fn borrowed(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// The remote key an operation needs, or the reason it cannot run.
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

fn require_key(key: Option<&str>) -> Result<&str, BoardError> {
    key.ok_or_else(|| BoardError::Backend("card has no Jira issue to write to".into()))
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

/// Whether a string is something `--assignee` takes as it stands.
///
/// `acli` addresses a user by email or by Atlassian account id (`5b10ac8d82e05b22cc7d4ef5`, or
/// the `712020:<uuid>` form). A bare word that is neither is a display name the pull never saw,
/// and handing it over means Jira refuses the whole edit or drops the assignee silently.
fn is_account_handle(assignee: &str) -> bool {
    if assignee.contains(char::is_whitespace) || assignee.is_empty() {
        return false;
    }
    if assignee.contains('@') && !assignee.starts_with('@') && !assignee.ends_with('@') {
        return true;
    }
    let (prefix, id) = assignee
        .split_once(':')
        .map_or((None, assignee), |(prefix, id)| (Some(prefix), id));
    let hexish = id.len() >= 16
        && id
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-');
    hexish
        && prefix.is_none_or(|prefix| {
            !prefix.is_empty() && prefix.chars().all(|character| character.is_ascii_digit())
        })
}

/// The bare host of a site, however the user spelled it.
fn host(site: &str) -> String {
    site.trim()
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{adapters::clock::SystemClock, testing::fakes::FakeShell};

    fn backend() -> JiraBackend {
        JiraBackend::new(Arc::new(FakeShell::new()), Arc::new(SystemClock))
    }

    /// The identity half of the trait is real from this stage on: the registry, the settings
    /// dialog and `fleet board backends` all read it before anything talks to Jira.
    #[test]
    fn identity_and_capabilities_are_answered_without_touching_jira() {
        let backend = backend();
        assert_eq!(backend.kind(), "jira");
        assert_eq!(backend.label(), "Jira (acli)");
        let caps = backend.capabilities();
        assert!(caps.pull && caps.incremental && caps.transitions && caps.comments);
        assert!(caps.push_updates && caps.push_create && caps.custom_properties);
        assert_eq!(backend.settings_schema(), settings::settings_schema());
    }

    /// Every read-only field must be a field name the core knows, or the guard silently passes.
    #[test]
    fn the_read_only_fields_are_card_field_names() {
        for field in READONLY_FIELDS {
            assert!(
                [
                    "title",
                    "description",
                    "status_id",
                    "priority",
                    "labels",
                    "assignee",
                    "estimate",
                    "due_date",
                    "parent_id"
                ]
                .contains(field),
                "{field} is not a standard card field"
            );
        }
        // The two halves must not overlap: a field that is both written and refused would be
        // pushed by `edit` and rejected by the core in the same sync.
        for field in WRITABLE_FIELDS {
            assert!(!READONLY_FIELDS.contains(field), "{field}");
        }
    }

    #[tokio::test]
    async fn a_staged_body_is_removed_once_the_call_is_over() {
        let path = {
            let file = TempJson::new(&serde_json::json!({"type": "doc"}))
                .await
                .unwrap();
            let path = PathBuf::from(file.argument());
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "{\"type\":\"doc\"}"
            );
            path
        };
        assert!(!path.exists(), "a staged ADF body outlived its call");
    }
}
