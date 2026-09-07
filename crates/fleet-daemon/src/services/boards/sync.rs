use super::*;

impl Boards {
    /// Submits a detached schema/pull/push job; local boards do not submit jobs.
    ///
    /// `full` clears the stored cursor **before** the job starts, so the pull that runs is a
    /// full one even though `sync_document` reads the cursor minutes later, and a full sync
    /// that fails halfway still leaves the board asking for a full one next time.
    ///
    /// The job target is the board id with no per-attempt suffix, so a second request while one
    /// sync is in flight coalesces onto the running job instead of pulling the same board twice.
    ///
    /// The job is submitted **retryable and not cancellable** (`docs/BOARD.md` §8). That is why
    /// `sync_document` carries no `check_cancelled` calls: `JobManager::cancel` refuses a
    /// non-cancellable job, so nothing ever cancels this token and a check between phases would
    /// be unreachable. A sync that must stop fails instead, and `run_sync` reloads the last valid
    /// checkpoint.
    pub async fn sync(&self, id: &BoardId, full: bool) -> DaemonResult<JobId> {
        let doc = self.load(id)?;
        if doc.board.backend.is_local() {
            return Err(BoardError::Unsupported("pull").into());
        }
        // A backend this build does not register is a bad request, not a job that fails
        // minutes later: `create` and `update` already refuse it up front.
        self.backends.get(&doc.board.backend.kind)?;
        if full {
            // Under the same gate `run_sync` takes, and re-read inside it: a sync already
            // running would otherwise write the cursor back over the clearing.
            let _guard = self.gate(id).await;
            let mut doc = self.load(id)?;
            if doc.board.sync.cursor.take().is_some() {
                doc.board.updated_at = self.now();
                self.persist(&doc).await?;
            }
        }
        let service = self.clone();
        let id = id.clone();
        Ok(self.jobs.submit(
            JobKind::Custom("board.sync".into()),
            id.to_string(),
            format!("Sync {}", doc.board.name),
            false,
            true,
            move |context| async move { service.run_sync(&id, &context).await },
        ))
    }

    async fn run_sync(&self, id: &BoardId, context: &JobCtx) -> DaemonResult<()> {
        let _guard = self.gate(id).await;
        let result = self.sync_document(id, context).await;
        if let Err(error) = &result {
            // Reload the last valid checkpoint, never persist a partially invalid pull.
            if let Ok(mut doc) = self.load(id) {
                doc.board.sync.last_error = Some(error.to_string());
                doc.board.updated_at = self.now();
                if let Err(save_error) = self.persist(&doc).await {
                    tracing::warn!(%id, %save_error, "failed to persist board sync error");
                }
            }
            self.changed(id, BoardChangeReason::SyncFailed);
        }
        result
    }

    async fn sync_document(&self, id: &BoardId, context: &JobCtx) -> DaemonResult<()> {
        let mut doc = self.load(id)?;
        let backend = self.backends.get(&doc.board.backend.kind)?;
        let caps = backend.capabilities();
        if !caps.pull {
            return Err(BoardError::Unsupported("pull").into());
        }
        context.progress("describing board backend")?;
        let schema = backend.describe(&doc.board).await?;
        context.progress("adopting backend schema")?;
        let now = self.now();
        if doc.board.sync.last_synced_at.is_none() {
            doc.board.sync.last_synced_at = doc
                .cards
                .iter()
                .filter_map(|card| card.remote.as_ref())
                .map(|link| link.synced_at.clone())
                .max();
        }
        let old_properties = doc.board.properties.clone();
        let old_statuses = doc.board.statuses.clone();
        let mut unmapped = adopt_schema(&mut doc.board, &schema, &now);
        for card in &mut doc.cards {
            card.properties.retain(|key, value| {
                if !old_properties
                    .iter()
                    .any(|schema| schema.key == *key && schema.source == PropertySource::Backend)
                {
                    return true;
                }
                doc.board.properties.iter().any(|schema| {
                    // A schema with no options at all states nothing about the values it
                    // allows: Jira's issue-type options come from a `project view` whose
                    // failure is a warning, and dropping every value against an empty list
                    // would delete the property from every card over one failed metadata call.
                    schema.key == *key
                        && value.matches_kind(schema.kind)
                        && (schema.options.is_empty()
                            || match value {
                                PropertyValue::Select(value) => {
                                    schema.options.iter().any(|option| option.value == *value)
                                }
                                PropertyValue::MultiSelect(values) => values.iter().all(|value| {
                                    schema.options.iter().any(|option| option.value == *value)
                                }),
                                _ => true,
                            })
                })
            });
            if !doc
                .board
                .statuses
                .iter()
                .any(|status| status.id == card.status_id)
            {
                let old = old_statuses
                    .iter()
                    .find(|status| status.id == card.status_id);
                let status = old
                    .and_then(|old| {
                        doc.board
                            .statuses
                            .iter()
                            .find(|status| status.name.eq_ignore_ascii_case(&old.name))
                    })
                    .or_else(|| old.and_then(|old| first_status_in(&doc.board, old.category)))
                    .or_else(|| doc.board.statuses.first())
                    .ok_or_else(|| BoardError::Invalid {
                        field: "statuses".into(),
                        reason: "backend provided no usable status".into(),
                    })?;
                card.status_id = status.id.clone();
                card.updated_at = now.clone();
            }
        }
        context.progress("pulling remote cards")?;
        let cursor = if caps.incremental {
            doc.board.sync.cursor.as_deref()
        } else {
            None
        };
        let mut pull = backend.pull(&doc.board, cursor).await?;
        if !pull.full
            && doc.cards.iter().any(|card| {
                (card.dirty || awaiting_push_baseline(card))
                    && card.remote.as_ref().is_some_and(|link| {
                        !pull.cards.iter().any(|remote| remote.key == link.key)
                            && !pull.deleted_keys.contains(&link.key)
                    })
            })
        {
            // A cursor omission cannot tell us which fields (especially status) still need pushing.
            pull = backend.pull(&doc.board, None).await?;
            if doc.cards.iter().any(|card| {
                (card.dirty || awaiting_push_baseline(card))
                    && card.remote.as_ref().is_some_and(|link| {
                        !pull.full
                            && !pull.cards.iter().any(|remote| remote.key == link.key)
                            && !pull.deleted_keys.contains(&link.key)
                    })
            }) {
                return Err(
                    BoardError::Backend("backend omitted a dirty card's baseline".into()).into(),
                );
            }
        }
        for card in doc
            .cards
            .iter_mut()
            .filter(|card| awaiting_push_baseline(card))
        {
            let key = &card.remote.as_ref().expect("recovery requires a link").key;
            if pull.deleted_keys.contains(key)
                || (pull.full && !pull.cards.iter().any(|remote| remote.key == *key))
            {
                card.archived = true;
                card.dirty = false;
                card.conflict = None;
                push_activity(
                    card,
                    ActivityKind::Synced,
                    None,
                    fleet_core::board::ARCHIVED_BY_SYNC,
                    &self.now(),
                );
                continue;
            }
            let remote = pull
                .cards
                .iter()
                .find(|remote| {
                    card.remote
                        .as_ref()
                        .is_some_and(|link| link.key == remote.key)
                })
                .ok_or_else(|| BoardError::Backend("missing post-push baseline".into()))?;
            if let Some(link) = &mut card.remote {
                link.version = remote.version.clone();
                link.remote_updated_at = remote.updated_at.clone();
            }
            push_activity(
                card,
                ActivityKind::Synced,
                None,
                "Refreshed post-push baseline",
                &self.now(),
            );
        }
        // A sampled `describe` names only the statuses its sample happened to use, and a
        // project gains statuses between two syncs. Adopting the ones this pull brought back —
        // before anything reconciles against them — is what turns them into columns instead of
        // "unmapped status" noise on the activity of every card that holds one.
        if let Some(readopted) =
            readopt_pulled_statuses(&mut doc.board, &schema, &pull, &self.now())
        {
            context.progress("adopting newly seen remote statuses")?;
            // The re-adoption covers everything `describe` named plus what the pull added, so
            // its answer replaces the first one rather than joining it.
            unmapped = readopted;
        }
        context.progress("reconciling remote cards")?;
        let now = self.now();
        let mut reconciled = reconcile(&doc.board, &doc.cards, &pull, caps, &now);
        // The pure engine allocates deterministic placeholders; persistence owns UUIDs.
        let mut ids = HashMap::new();
        for card in &mut reconciled.cards {
            if !doc.cards.iter().any(|existing| existing.id == card.id) {
                let id = new_card_id()?;
                ids.insert(card.id.clone(), id.clone());
                card.id = id;
            }
        }
        for card in &mut reconciled.cards {
            if let Some(id) = card.parent_id.as_ref().and_then(|id| ids.get(id)) {
                card.parent_id = Some(id.clone());
            }
        }
        for op in &mut reconciled.to_push {
            let card_id = match op {
                PushOp::Create { card_id }
                | PushOp::Update { card_id, .. }
                | PushOp::Transition { card_id, .. }
                | PushOp::AddComment { card_id, .. } => card_id,
            };
            if let Some(id) = ids.get(card_id) {
                *card_id = id.clone();
            }
        }
        reconciled.summary.unmapped_statuses.extend(unmapped);
        reconciled.summary.unmapped_statuses.sort();
        reconciled.summary.unmapped_statuses.dedup();
        let checkpoint_cursor = doc.board.sync.cursor.clone();
        doc.board = reconciled.board;
        doc.cards = reconciled.cards;
        if let Some(error) = &doc.board.sync.last_error {
            return Err(BoardError::Backend(error.clone()).into());
        }
        let unimportable = std::mem::take(&mut reconciled.summary.skipped);
        let mut skipped = unimportable.clone();
        // A key the backend listed but could not read is reported exactly like one this board
        // could not import: named, without failing the cards that did reconcile.
        skipped.extend(pull.failed_keys.iter().cloned());
        let skipped_warning = (!skipped.is_empty()).then(|| {
            format!(
                "skipped {} remote cards: {}",
                skipped.len(),
                skipped.join("; ")
            )
        });
        if !unimportable.is_empty() {
            // One unimportable remote issue must not throw away the cards that did reconcile,
            // and must not advance the cursor past itself: the next pull offers it again.
            //
            // Only *this* board's failure to import rewinds the cursor. A key the backend
            // could not read is the backend's own business and it has already answered with
            // the cursor it wants kept — holding its watermark while still advancing whatever
            // counts pulls towards the next full one. Rewinding the whole opaque string on top
            // of that undid the second half: one permanently unreadable key froze the counter,
            // no full pull ever came due again, and remote deletions stopped being noticed
            // while the incremental window grew without bound.
            doc.board.sync.cursor = checkpoint_cursor;
        }
        // A remote issue this board cannot import is a warning about that issue, never a
        // reason to strand every local edit: the push below is the only way a dirty card ever
        // reaches the backend, and failing here left it queued forever behind one bad key.
        if let Some(warning) = &skipped_warning
            && reconciled.summary.created == 0
            && reconciled.summary.updated == 0
            && reconciled.summary.deleted == 0
            && reconciled.to_push.is_empty()
        {
            self.persist(&doc).await?;
            return Err(BoardError::Backend(warning.clone()).into());
        }
        self.persist(&doc).await?;
        if !reconciled.to_push.is_empty() {
            context.progress(format!("pushing {} operations", reconciled.to_push.len()))?;
            let result = backend
                .push(&doc.board, &doc.cards, &reconciled.to_push)
                .await?;
            context.progress("applying push acknowledgements")?;
            apply_push_result(
                &mut doc.cards,
                &result,
                &doc.board.backend.kind,
                &reconciled.unpushed,
                &self.now(),
            );
            reconciled.summary.pushed = result
                .acks
                .iter()
                .filter(|ack| {
                    doc.cards.iter().any(|card| card.id == ack.card_id)
                        && !result
                            .failures
                            .iter()
                            .any(|failure| failure.card_id == ack.card_id)
                })
                .count();
            self.persist(&doc).await?;
            if result.acks.iter().any(|ack| ack.version.is_none()) {
                context.progress("refreshing post-push baselines")?;
                let baseline = backend.pull(&doc.board, None).await?;
                for ack in result.acks.iter().filter(|ack| ack.version.is_none()) {
                    let remote = baseline
                        .cards
                        .iter()
                        .find(|remote| remote.key == ack.key)
                        .ok_or_else(|| {
                            BoardError::Backend(format!(
                                "missing post-push baseline for {}",
                                ack.key
                            ))
                        })?;
                    if let Some(card) = doc.cards.iter_mut().find(|card| card.id == ack.card_id) {
                        if let Some(link) = &mut card.remote {
                            link.version = remote.version.clone();
                            link.remote_updated_at = remote.updated_at.clone();
                        }
                        push_activity(
                            card,
                            ActivityKind::Synced,
                            None,
                            "Refreshed post-push baseline",
                            &self.now(),
                        );
                    }
                }
                self.persist(&doc).await?;
            }
            if !result.failures.is_empty() {
                // The card's display key, never its UUID: this sentence is what `board sync`
                // and the board list print, and a user cannot look a UUID up anywhere.
                return Err(BoardError::Backend(
                    result
                        .failures
                        .iter()
                        .map(|failure| {
                            let name = doc
                                .cards
                                .iter()
                                .find(|card| card.id == failure.card_id)
                                .map_or_else(
                                    || failure.card_id.to_string(),
                                    |card| card.display_key(&doc.board),
                                );
                            format!("{name}: {}", failure.error)
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                )
                .into());
            }
        }
        context.progress("saving board sync result")?;
        doc.board.sync.last_synced_at = Some(self.now());
        // The skipped keys stay visible where every surface already looks for a board's
        // trouble, instead of being lost with the job that reported them. `reconcile` clears
        // it on the next sync, so the warning lasts exactly as long as its cause.
        doc.board.sync.last_error = skipped_warning.clone();
        doc.board.updated_at = self.now();
        self.save(&doc, BoardChangeReason::Synced).await?;
        // `deleted` is counted here and nowhere else: a card this sync archived because its
        // issue is gone drops out of every column, out of `summarize` and out of `board show`,
        // and without this number nothing at all says it happened.
        // Both clauses are conditional: an unconditional `unmapped statuses: ` left every
        // successful sync ending in a colon with nothing after it.
        let unmapped_clause = if reconciled.summary.unmapped_statuses.is_empty() {
            String::new()
        } else {
            format!(
                "; unmapped statuses: {}",
                reconciled.summary.unmapped_statuses.join(", ")
            )
        };
        // `pushNewCards` defaults to off, so a card made on a linked board files no issue and
        // stays dirty forever. The sync used to report `0 pushed` and leave the user to guess;
        // this names the setting that is holding them, once, with the count.
        let kept_local_clause = if reconciled.summary.kept_local == 0 {
            String::new()
        } else {
            format!(
                "; {} kept local (pushNewCards is off)",
                reconciled.summary.kept_local
            )
        };
        context.progress(format!("synced: {} pulled, {} created, {} updated, {} archived, {} conflicts, {} pushed{unmapped_clause}{kept_local_clause}{}",
            reconciled.summary.pulled, reconciled.summary.created, reconciled.summary.updated,
            reconciled.summary.deleted, reconciled.summary.conflicts, reconciled.summary.pushed,
            skipped_warning.map(|warning| format!("; {warning}")).unwrap_or_default()))?;
        Ok(())
    }

    /// Resolves a conflict, materializing remote labels and resolving remote parent keys.
    pub async fn resolve_conflict(
        &self,
        card: &CardId,
        resolution: ConflictResolution,
    ) -> DaemonResult<Card> {
        let (_guard, mut doc, index) = self.card_document(card).await?;
        let now = self.now();
        let mut parent = None;
        if resolution == ConflictResolution::TakeRemote
            && let Some(conflict) = doc.cards[index].conflict.clone()
        {
            let mut scratch = doc.cards[index].clone();
            apply_remote(&mut doc.board, &mut scratch, &conflict.remote, &now);
            parent = conflict
                .remote
                .parent_key
                .as_ref()
                .and_then(|key| {
                    doc.cards.iter().find(|candidate| {
                        candidate.id != *card
                            && candidate
                                .remote
                                .as_ref()
                                .is_some_and(|link| link.key == *key)
                    })
                })
                .map(|candidate| candidate.id.clone());
            // A remote parent that would close a cycle is dropped exactly like a key that
            // names no local card: `UpdateCard` refuses one, and the contract forbids it.
            if validate_parent(&doc.cards, Some(card), parent.as_ref()).is_err() {
                parent = None;
            }
        }
        sync::resolve_conflict(&doc.board, &mut doc.cards[index], resolution, &now)?;
        if resolution == ConflictResolution::TakeRemote {
            doc.cards[index].parent_id = parent;
        }
        self.save_card(doc, index, &now).await
    }

    /// Fetches backend metadata without mutating the persisted board.
    pub async fn describe_backend(&self, id: &BoardId) -> DaemonResult<BackendSchema> {
        let doc = self.load(id)?;
        Ok(self
            .backends
            .get(&doc.board.backend.kind)?
            .describe(&doc.board)
            .await?)
    }

    /// Describes every backend kind this build registers.
    pub fn list_backends(&self) -> Vec<BackendDescriptor> {
        self.backends.descriptors()
    }
}

/// Adopts the statuses a pull brought back that the board's status map does not name yet.
///
/// Returns the unmapped statuses of the second adoption, or `None` when there was nothing to
/// adopt. The scratch schema is the described one with those statuses added: `adopt_schema`
/// rebuilds the whole status map, and replaces the board's backend properties, labels and
/// read-only fields, from the schema it is handed — a schema carrying only the new statuses
/// would erase the mapping of every status `describe` did name.
fn readopt_pulled_statuses(
    board: &mut Board,
    schema: &BackendSchema,
    pull: &PullResult,
    now: &str,
) -> Option<Vec<String>> {
    let unseen = unseen_statuses(board, pull);
    if unseen.is_empty() {
        return None;
    }
    let mut used: std::collections::BTreeSet<String> = board
        .statuses
        .iter()
        .map(|status| status.id.to_string())
        .collect();
    for remote in &unseen {
        let name = if remote.name.trim().is_empty() {
            remote_status_key(remote)
        } else {
            remote.name.clone()
        };
        // A column already carrying this name is the column this status belongs to:
        // `adopt_schema` maps the two together, and a second one would split the work in half.
        if board
            .statuses
            .iter()
            .any(|status| status.name.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        let Ok(id) = StatusId::try_from(unique_status_slug(&name, &mut used)) else {
            continue;
        };
        let declared = remote_category(remote);
        let status = Status {
            id,
            name,
            category: declared.unwrap_or(StatusCategory::Unstarted),
            color: None,
        };
        // A board reads left to right in workflow order, and appending put a *started* column
        // to the right of the completed one the moment a project gained a status between two
        // syncs. A status whose category the remote declared goes after the last column of
        // that category or earlier — where `describe` would have placed it had its sample
        // named it the first time. One the remote left unnamed is a guess either way, and the
        // end is the least surprising place for a column nobody can order.
        match declared {
            Some(category) => {
                let at = board
                    .statuses
                    .iter()
                    .rposition(|existing| existing.category <= category)
                    .map_or(0, |index| index + 1);
                board.statuses.insert(at, status);
            }
            None => board.statuses.push(status),
        }
    }
    // `adopt_schema` adopts a schema wholesale on a board that still carries the default
    // columns with nothing mapped to them. Reaching here with those columns untouched means
    // every pulled status matched one by name, so a second adoption would trade the whole
    // default set for the handful this pull happened to mention — including the columns the
    // cards this sync just repaired now sit in.
    if board.statuses == default_statuses()
        && board.sync.status_map.remote_to_local.is_empty()
        && board.sync.last_synced_at.is_none()
    {
        return None;
    }
    let keys: std::collections::BTreeSet<String> = unseen.iter().map(remote_status_key).collect();
    let mut scratch = schema.clone();
    // A status `describe` named but never mapped can appear in both lists; it is the pull's
    // copy that carries the category the issues actually use.
    scratch
        .statuses
        .retain(|status| !keys.contains(&remote_status_key(status)));
    scratch.statuses.extend(unseen);
    Some(adopt_schema(board, &scratch, now))
}

/// The distinct statuses of pulled cards that no live column is mapped to.
fn unseen_statuses(board: &Board, pull: &PullResult) -> Vec<RemoteStatus> {
    let mut seen = std::collections::BTreeSet::new();
    let mut unseen = Vec::new();
    for status in pull.cards.iter().map(|card| &card.status) {
        let key = remote_status_key(status);
        // A status with neither an id nor a name is no status at all: adopting it would add a
        // nameless column, which `validate_board` refuses.
        if key.is_empty() {
            continue;
        }
        let mapped = [&status.id, &status.name]
            .into_iter()
            .filter(|key| !key.is_empty())
            .any(|key| {
                board
                    .sync
                    .status_map
                    .remote_to_local
                    .get(key)
                    // A map entry pointing at a column that no longer exists names nothing.
                    .is_some_and(|id| board.statuses.iter().any(|status| status.id == *id))
            });
        if mapped || !seen.insert(key) {
            continue;
        }
        unseen.push(status.clone());
    }
    unseen
}

/// A status id no column on this board already holds.
fn unique_status_slug(name: &str, used: &mut std::collections::BTreeSet<String>) -> String {
    let mut base = fleet_core::slug::normalize_context_id(name);
    if base.is_empty() {
        base = "status".into();
    }
    let mut slug = base.clone();
    let mut suffix = 2;
    while !used.insert(slug.clone()) {
        slug = format!("{base}-{suffix}");
        suffix += 1;
    }
    slug
}
