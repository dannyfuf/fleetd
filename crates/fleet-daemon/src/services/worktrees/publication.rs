use super::*;

impl Worktrees {
    pub(super) async fn publish(
        &self,
        repo: &Repo,
        slug: &str,
        branch: &str,
        base_ref: &str,
        hooks: &RepoHooks,
        attempt: &Path,
    ) -> DaemonResult<Worktree> {
        let id = worktree_id(&repo.id, slug)?;
        let session = SessionId::local(&repo.name, slug)
            .map_err(|error| DaemonError::Validation(error.to_string()))?
            .to_string();
        let canonical = attempt
            .parent()
            .ok_or_else(|| DaemonError::Validation("attempt has no parent".to_owned()))?
            .join(slug);
        let created_at = Utc::now().to_rfc3339();
        let worktree = Worktree {
            id: id.clone(),
            repo_id: repo.id.clone(),
            slug: slug.to_owned(),
            branch: branch.to_owned(),
            base_ref: base_ref.to_owned(),
            path: canonical.to_string_lossy().into_owned(),
            session,
            host: None,
            created_at: created_at.clone(),
            last_opened_at: None,
            degraded: None,
        };
        let intent = CreatingMarker {
            id: id.to_string(),
            repo_id: repo.id.clone(),
            branch: branch.to_owned(),
            base_ref: base_ref.to_owned(),
            created_at,
        };
        let mut intent_text = serde_json::to_string_pretty(&intent)?;
        intent_text.push('\n');

        let published = Arc::new(Mutex::new(false));
        let published_in_transaction = Arc::clone(&published);
        let files = Arc::clone(&self.files);
        let attempt_in_transaction = attempt.to_path_buf();
        let canonical_in_transaction = canonical.clone();
        let marker_in_transaction = creating_marker_path(attempt);
        let id_in_transaction = id.clone();
        let hooks = hooks.clone();
        let worktree_in_transaction = worktree.clone();
        let repo_id = repo.id.clone();
        let transaction = self
            .state
            .transaction(move |state| {
                if state
                    .worktrees
                    .iter()
                    .any(|item| item.id == id_in_transaction)
                {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {id_in_transaction} already exists"
                    )));
                }
                if state.worktrees.iter().any(|item| {
                    item.session == worktree_in_transaction.session
                        || item.path == worktree_in_transaction.path
                }) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree path or session conflicts for {id_in_transaction}"
                    )));
                }
                if files.exists(&canonical_in_transaction) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree destination already exists: {}",
                        canonical_in_transaction.display()
                    )));
                }
                files.atomic_write_text(&marker_in_transaction, &intent_text)?;
                files.rename(&attempt_in_transaction, &canonical_in_transaction)?;
                *published_in_transaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                let registered_repo = state
                    .repos
                    .iter_mut()
                    .find(|item| item.id == repo_id)
                    .ok_or_else(|| DaemonError::NotFound(format!("repository {repo_id}")))?;
                registered_repo.hooks = hooks;
                state.worktrees.push(worktree_in_transaction.clone());
                Ok(worktree_in_transaction)
            })
            .await;
        match transaction {
            Ok(worktree) => {
                self.files.remove_file(&creating_marker_path(&canonical))?;
                Ok(worktree)
            }
            Err(error) => {
                if *published
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    && self.files.exists(&canonical)
                    && let Ok(trashed) = self.files.trash(&canonical)
                {
                    let _ignored = self.files.remove_detached(&trashed);
                }
                Err(error)
            }
        }
    }

    pub(super) async fn reclaim_publish_intent(
        &self,
        expected_id: &WorktreeId,
        canonical: &Path,
    ) -> DaemonResult<()> {
        if !self.files.exists(canonical) {
            return Ok(());
        }
        let marker_path = creating_marker_path(canonical);
        let marker_text = self.files.read_text(&marker_path).map_err(|_| {
            DaemonError::Conflict(format!(
                "worktree destination exists without publish intent: {}",
                canonical.display()
            ))
        })?;
        let marker: CreatingMarker = serde_json::from_str(&marker_text).map_err(|_| {
            DaemonError::Conflict(format!(
                "worktree destination has invalid publish intent: {}",
                canonical.display()
            ))
        })?;
        if marker.id != expected_id.to_string()
            || marker.worktree_id(expected_id.slug()).ok().as_ref() != Some(expected_id)
        {
            return Err(DaemonError::Conflict(format!(
                "mismatched publish intent at {}",
                canonical.display()
            )));
        }
        let trashed = self.files.trash(canonical)?;
        self.files.remove_detached(&trashed)
    }
}
