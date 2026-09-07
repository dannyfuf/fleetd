use super::*;

impl Worktrees {
    /// Reconciles publish intents and removes abandoned private attempts.
    pub async fn recover_startup(&self) -> DaemonResult<()> {
        let config = self.config.load().await?;
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

    async fn recover_attempt(
        &self,
        config: &Config,
        repo: &Repo,
        slug: &str,
        attempt: &Path,
    ) -> DaemonResult<()> {
        let marker = self.read_creating_marker(attempt);
        let Ok(marker) = marker else {
            return self.trash_attempt(attempt);
        };
        if !self
            .valid_recovery_marker(repo, slug, attempt, &marker)
            .await
        {
            return self.trash_attempt(attempt);
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
        let expected_id = worktree_id(&repo.id, slug)?;
        if state.worktrees.iter().any(|item| item.id == expected_id) {
            return self.trash_attempt(path);
        }
        if !self.valid_recovery_marker(repo, slug, path, &marker).await {
            return self.trash_attempt(path);
        }
        self.register_recovered(repo, slug, path, marker).await
    }

    async fn valid_recovery_marker(
        &self,
        repo: &Repo,
        slug: &str,
        path: &Path,
        marker: &CreatingMarker,
    ) -> bool {
        let expected_id = worktree_id(&repo.id, slug).ok();
        if marker.repo_id != repo.id
            || expected_id
                .as_ref()
                .is_none_or(|id| marker.id != id.to_string())
            || marker.base_ref.is_empty()
            || marker.branch.is_empty()
        {
            return false;
        }
        if marker.worktree_id(slug).is_err() {
            return false;
        }
        self.git
            .current_branch(path)
            .await
            .is_ok_and(|branch| branch == marker.branch)
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
}
