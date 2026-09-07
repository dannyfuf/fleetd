use super::*;

impl JiraBackend {
    /// Pushes every operation queued for one card, in order, reporting what was applied and
    /// the first operation that was not.
    ///
    /// Writes are sequential by construction: a transition that runs before the edit that
    /// renamed the issue would be reported against a card the user no longer recognizes. The
    /// prefix that did run is acknowledged even when a later operation fails — a comment whose
    /// ack is dropped is posted again by the next sync, and an edit whose new `updated` is
    /// never recorded turns the user's own change into a conflict.
    pub(super) async fn push_card(
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

/// A JSON file `acli` reads a body from, removed as soon as the call is over.
pub(super) struct TempJson {
    path: PathBuf,
}

impl TempJson {
    pub(super) async fn new(value: &serde_json::Value) -> Result<Self, BoardError> {
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

    pub(super) fn argument(&self) -> String {
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
fn require_key(key: Option<&str>) -> Result<&str, BoardError> {
    key.ok_or_else(|| BoardError::Backend("card has no Jira issue to write to".into()))
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
