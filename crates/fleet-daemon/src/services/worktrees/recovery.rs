use super::*;

const LEGACY_REMOTE_ARCHIVE: &str = "cache/legacy-remote.json";

impl Worktrees {
    /// Reconciles publish intents and removes abandoned private attempts.
    pub async fn recover_startup(&self) -> DaemonResult<()> {
        self.migrate_legacy_remote_records().await?;
        let config = self.config.load().await?;
        self.reconcile_trash_expiries(&config)?;
        let state = self.state.load().await?;
        for repo in state.repos {
            let root = repo_worktrees_dir(&config, &repo.id);
            if !self.files.exists(&root) {
                continue;
            }
            for child in self.files.list(&root)? {
                let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if name.starts_with(".hot") || name.starts_with(".discard-") {
                    continue;
                }
                if let Some((slug, _attempt)) = name.rsplit_once(".creating-") {
                    self.recover_attempt(&config, &repo, slug, &child).await?;
                } else {
                    self.recover_published(&repo, name, &child).await?;
                }
            }
        }
        self.recover_post_create_intents().await?;
        Ok(())
    }

    /// Returns pre-federation remote records retained for doctor diagnostics.
    pub async fn legacy_remote_records(
        &self,
    ) -> DaemonResult<fleet_core::state::LegacyRemoteRecords> {
        let path = self.legacy_remote_archive_path()?;
        if !self.files.exists(&path) {
            return Ok(fleet_core::state::LegacyRemoteRecords::empty());
        }
        let records = serde_json::from_str(&self.files.read_text(&path)?)?;
        Ok(records)
    }

    async fn migrate_legacy_remote_records(&self) -> DaemonResult<()> {
        let remote = self
            .state
            .load()
            .await?
            .worktrees
            .into_iter()
            .filter(|worktree| worktree.host.is_some())
            .collect::<Vec<_>>();
        if remote.is_empty() {
            return Ok(());
        }

        let path = self.legacy_remote_archive_path()?;
        let mut archive = if self.files.exists(&path) {
            serde_json::from_str(&self.files.read_text(&path)?)?
        } else {
            fleet_core::state::LegacyRemoteRecords::empty()
        };
        archive.merge(remote.iter().cloned());
        let mut text = serde_json::to_string_pretty(&archive)?;
        text.push('\n');
        self.files.atomic_write_text(&path, &text)?;

        let migrated = remote
            .iter()
            .map(|worktree| (worktree.host.clone(), worktree.id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        self.state
            .transaction(move |state| {
                state.worktrees.retain(|worktree| {
                    !migrated.contains(&(worktree.host.clone(), worktree.id.clone()))
                });
                Ok(())
            })
            .await?;
        tracing::info!(
            count = remote.len(),
            path = %path.display(),
            "migrated persisted remote worktrees into legacy archive"
        );
        Ok(())
    }

    fn legacy_remote_archive_path(&self) -> DaemonResult<PathBuf> {
        let home = self.state.path().parent().ok_or_else(|| {
            DaemonError::Validation(format!(
                "state path has no parent: {}",
                self.state.path().display()
            ))
        })?;
        Ok(home.join(LEGACY_REMOTE_ARCHIVE))
    }

    pub(super) fn schedule_startup_recovery(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.startup_ready.store(true, Ordering::Release);
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            tracing::info!("publish-intent recovery scan started");
            match service.recover_startup().await {
                Ok(()) => tracing::info!("publish-intent recovery scan completed"),
                Err(error) => tracing::warn!(%error, "publish-intent recovery scan failed"),
            }
            service.startup_ready.store(true, Ordering::Release);
            service.startup_notify.notify_waiters();
        });
    }

    pub(super) async fn await_startup_recovery(&self) {
        while !self.startup_ready.load(Ordering::Acquire) {
            let notified = self.startup_notify.notified();
            if self.startup_ready.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }

    pub(super) async fn recover_attempt(
        &self,
        config: &Config,
        repo: &Repo,
        slug: &str,
        attempt: &Path,
    ) -> DaemonResult<()> {
        let id = worktree_id(&repo.id, slug)?;
        let _lifecycle = self.sessions.claim_worktree_lifecycle(id).await;
        let marker = match self.read_creating_marker(attempt) {
            Ok(marker) => marker,
            Err(error) => {
                self.quarantine_attempt(config, repo, slug, attempt, None)?;
                tracing::warn!(%error, path = %attempt.display(), "quarantined worktree attempt with unavailable marker evidence");
                return Ok(());
            }
        };
        match self
            .recovery_marker_validity(repo, slug, attempt, &marker)
            .await
        {
            RecoveryValidity::Valid => {}
            RecoveryValidity::Invalid => return self.trash_attempt(attempt),
            RecoveryValidity::Unavailable(error) => {
                self.quarantine_attempt(config, repo, slug, attempt, Some(&marker))?;
                tracing::warn!(%error, path = %attempt.display(), "quarantined worktree attempt after observation failure");
                return Ok(());
            }
        }
        let canonical = repo_worktrees_dir(config, &repo.id).join(slug);
        if self.files.exists(&canonical) {
            return self.trash_attempt(attempt);
        }
        self.files.rename(attempt, &canonical)?;
        if let Err(error) = self
            .register_recovered(repo, slug, &canonical, marker)
            .await
        {
            let trashed = self.files.trash(&canonical)?;
            self.files.remove_detached(&trashed)?;
            return Err(error);
        }
        Ok(())
    }

    async fn recover_published(&self, repo: &Repo, slug: &str, path: &Path) -> DaemonResult<()> {
        let expected_id = worktree_id(&repo.id, slug)?;
        let _lifecycle = self
            .sessions
            .claim_worktree_lifecycle(expected_id.clone())
            .await;
        let marker = match self.read_creating_marker(path) {
            Ok(marker) => marker,
            Err(_) => return Ok(()),
        };
        let state = self.state.load().await?;
        if state
            .worktrees
            .iter()
            .any(|item| item.path == path.to_string_lossy())
        {
            self.files.remove_file(&creating_marker_path(path))?;
            return Ok(());
        }
        if state.worktrees.iter().any(|item| item.id == expected_id) {
            return self.trash_attempt(path);
        }
        match self
            .recovery_marker_validity(repo, slug, path, &marker)
            .await
        {
            RecoveryValidity::Valid => {}
            RecoveryValidity::Invalid => return self.trash_attempt(path),
            RecoveryValidity::Unavailable(error) => return Err(error),
        }
        self.register_recovered(repo, slug, path, marker).await
    }

    async fn recovery_marker_validity(
        &self,
        repo: &Repo,
        slug: &str,
        path: &Path,
        marker: &CreatingMarker,
    ) -> RecoveryValidity {
        let expected_id = worktree_id(&repo.id, slug).ok();
        if marker.repo_id != repo.id
            || expected_id
                .as_ref()
                .is_none_or(|id| marker.id != id.to_string())
            || marker.base_ref.is_empty()
            || marker.branch.is_empty()
        {
            return RecoveryValidity::Invalid;
        }
        if marker.worktree_id(slug).is_err() {
            return RecoveryValidity::Invalid;
        }
        match self.git.current_branch(path).await {
            Ok(branch) if branch == marker.branch => RecoveryValidity::Valid,
            Ok(_) => RecoveryValidity::Invalid,
            Err(error) => RecoveryValidity::Unavailable(error),
        }
    }

    async fn register_recovered(
        &self,
        repo: &Repo,
        slug: &str,
        path: &Path,
        marker: CreatingMarker,
    ) -> DaemonResult<()> {
        let worktree = Worktree {
            id: marker
                .worktree_id(slug)
                .map_err(|error| DaemonError::Validation(error.to_string()))?,
            repo_id: repo.id.clone(),
            slug: slug.to_owned(),
            branch: marker.branch,
            base_ref: marker.base_ref,
            path: path.to_string_lossy().into_owned(),
            session: SessionId::local(&repo.name, slug)
                .map_err(|error| DaemonError::Validation(error.to_string()))?
                .to_string(),
            host: None,
            created_at: marker.created_at,
            last_opened_at: None,
            degraded: None,
        };
        let id = worktree.id.clone();
        self.state
            .transaction(move |state| {
                if state.worktrees.iter().any(|item| {
                    item.id == id || item.path == worktree.path || item.session == worktree.session
                }) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {id} conflicts with registered state"
                    )));
                }
                state.worktrees.push(worktree);
                Ok(())
            })
            .await?;
        self.files.remove_file(&creating_marker_path(path))
    }

    fn read_creating_marker(&self, path: &Path) -> DaemonResult<CreatingMarker> {
        Ok(serde_json::from_str(
            &self.files.read_text(&creating_marker_path(path))?,
        )?)
    }

    fn quarantine_attempt(
        &self,
        config: &Config,
        repo: &Repo,
        slug: &str,
        path: &Path,
        creating: Option<&CreatingMarker>,
    ) -> DaemonResult<()> {
        if !self.files.exists(path) {
            return Ok(());
        }

        let worktree = Worktree {
            id: worktree_id(&repo.id, slug)?,
            repo_id: repo.id.clone(),
            slug: slug.to_owned(),
            branch: creating.map_or_else(|| slug.to_owned(), |marker| marker.branch.clone()),
            base_ref: creating.map_or_else(
                || format!("origin/{}", repo.default_branch),
                |marker| marker.base_ref.clone(),
            ),
            path: path.to_string_lossy().into_owned(),
            session: SessionId::local(&repo.name, slug)
                .map_err(|error| DaemonError::Validation(error.to_string()))?
                .to_string(),
            host: None,
            created_at: creating.map_or_else(
                || Utc::now().to_rfc3339(),
                |marker| marker.created_at.clone(),
            ),
            last_opened_at: None,
            degraded: None,
        };
        let worktree_id = worktree.id.clone();
        let expires_at = super::trash::trash_expiry(config.trash.retention_ms);
        let marker = TrashMarker {
            original_path: worktree.path.clone(),
            worktree,
            expires_at: Some(expires_at.to_rfc3339()),
        };
        let mut text = serde_json::to_string_pretty(&marker)?;
        text.push('\n');
        self.files
            .atomic_write_text(&trash_marker_path(path), &text)?;
        let trash = self.files.trash(path)?;
        self.schedule_trash_cleanup(trash, worktree_id, expires_at);
        Ok(())
    }
}

enum RecoveryValidity {
    Valid,
    Invalid,
    Unavailable(DaemonError),
}
