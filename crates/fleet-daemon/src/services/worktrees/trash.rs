use super::*;

impl Worktrees {
    /// Deletes worktrees independently using transactional trash rollback.
    pub async fn delete(&self, ids: Vec<WorktreeId>) -> DaemonResult<Vec<WorktreeDeleteResult>> {
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            let result = self.delete_one_job(id.clone()).await;
            results.push(WorktreeDeleteResult {
                worktree_id: id,
                ok: result.is_ok(),
                trash_entry: result.as_ref().ok().cloned(),
                reason: result.err().map(|error| error.to_string()),
            });
        }
        Ok(results)
    }

    /// Restores one validated worktree entry while its trash directory still exists.
    pub async fn restore_trash(&self, entry: String) -> DaemonResult<()> {
        validate_trash_entry(&entry)?;
        let config = self.config.load().await?;
        let trash = self.trash_dir().join(&entry);
        if !self.files.exists(&trash) {
            return Err(DaemonError::NotFound(format!("trash entry {entry}")));
        }
        let marker_path = trash_marker_path(&trash);
        let marker: TrashMarker = serde_json::from_str(&self.files.read_text(&marker_path)?)?;
        validate_restore_marker(&marker, &config)?;
        let destination = PathBuf::from(&marker.original_path);
        if self.files.exists(&destination) {
            return Err(DaemonError::Conflict(format!(
                "restore destination already exists: {}",
                destination.display()
            )));
        }

        let moved = Arc::new(Mutex::new(false));
        let moved_in_transaction = Arc::clone(&moved);
        let files = Arc::clone(&self.files);
        let trash_in_transaction = trash.clone();
        let destination_in_transaction = destination.clone();
        let worktree = marker.worktree.clone();
        let transaction = self
            .state
            .transaction(move |state| {
                if state.worktrees.iter().any(|item| item.id == worktree.id) {
                    return Err(DaemonError::Conflict(format!(
                        "worktree {} is already registered",
                        worktree.id
                    )));
                }
                files.rename(&trash_in_transaction, &destination_in_transaction)?;
                *moved_in_transaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                state.worktrees.push(worktree);
                Ok(())
            })
            .await;
        if let Err(error) = transaction {
            if *moved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                && self.files.exists(&destination)
            {
                let _ignored = self.files.rename(&destination, &trash);
            }
            return Err(error);
        }

        self.files.remove_file(&trash_marker_path(&destination))?;
        if let Some(job) = self
            .trash_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&entry)
        {
            let _ignored = self.jobs.cancel(&job);
        }
        Ok(())
    }

    async fn delete_one_job(&self, id: WorktreeId) -> DaemonResult<String> {
        let service = self.clone();
        let target = format!("{}:{}", id, Uuid::new_v4());
        let title = format!("Delete {id}");
        let (delivery, awaited) = JobDelivery::caller_gets_copy(copy_error);
        self.jobs.submit(
            JobKind::DeleteWorktree,
            target,
            title,
            false,
            false,
            move |context| async move { delivery.finish(service.delete_one(id, &context).await) },
        );
        awaited.wait().await
    }

    async fn delete_one(&self, id: WorktreeId, context: &JobCtx) -> DaemonResult<String> {
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        self.kill_session(&worktree.session).await?;
        let canonical = repo_worktrees_dir(&config, &worktree.repo_id).join(&worktree.slug);
        if Path::new(&worktree.path) != canonical {
            return Err(DaemonError::Validation(format!(
                "worktree {id} path is not canonical"
            )));
        }
        if !self.files.exists(&canonical) {
            return Err(DaemonError::NotFound(format!(
                "worktree path {}",
                canonical.display()
            )));
        }
        context.progress("moving worktree to trash")?;
        let marker = TrashMarker {
            original_path: worktree.path.clone(),
            worktree: worktree.clone(),
        };
        let mut marker_text = serde_json::to_string_pretty(&marker)?;
        marker_text.push('\n');
        let moved = Arc::new(Mutex::new(None::<PathBuf>));
        let moved_in_transaction = Arc::clone(&moved);
        let files = Arc::clone(&self.files);
        let canonical_in_transaction = canonical.clone();
        let id_in_transaction = id.clone();
        let transaction = self
            .state
            .transaction(move |state| {
                if !state
                    .worktrees
                    .iter()
                    .any(|item| item.id == id_in_transaction)
                {
                    return Err(DaemonError::NotFound(format!(
                        "worktree {id_in_transaction}"
                    )));
                }
                files.atomic_write_text(
                    &trash_marker_path(&canonical_in_transaction),
                    &marker_text,
                )?;
                let trash = files.trash(&canonical_in_transaction)?;
                *moved_in_transaction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(trash);
                state.worktrees.retain(|item| item.id != id_in_transaction);
                Ok(())
            })
            .await;
        if let Err(error) = transaction {
            if let Some(trash) = moved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                && self.files.exists(&trash)
            {
                let _ignored = self.files.rename(&trash, &canonical);
                let _ignored = self.files.remove_file(&trash_marker_path(&canonical));
            }
            return Err(error);
        }
        let trash = moved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| DaemonError::Filesystem {
                path: canonical,
                source: std::io::Error::other("trash rename did not return a destination"),
            })?;
        let entry = trash
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DaemonError::Validation("trash entry is not valid UTF-8".to_owned()))?
            .to_owned();
        self.schedule_trash_cleanup(trash, config.trash.retention_ms);
        Ok(entry)
    }

    fn schedule_trash_cleanup(&self, trash: PathBuf, retention_ms: u64) {
        let Some(entry) = trash
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            return;
        };
        let files = Arc::clone(&self.files);
        let jobs = Arc::clone(&self.trash_jobs);
        let entry_for_job = entry.clone();
        let id = self.jobs.submit(
            JobKind::Custom("trash_cleanup".to_owned()),
            entry.clone(),
            format!("Remove trash entry {entry}"),
            true,
            false,
            move |_context| async move {
                tokio::time::sleep(Duration::from_millis(retention_ms)).await;
                if files.exists(&trash) {
                    files.remove_detached(&trash)?;
                }
                jobs.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&entry_for_job);
                Ok(())
            },
        );
        self.trash_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(entry, id);
    }

    fn trash_dir(&self) -> PathBuf {
        self.state
            .path()
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("trash")
    }

    pub(super) fn trash_attempt(&self, path: &Path) -> DaemonResult<()> {
        if !self.files.exists(path) {
            return Ok(());
        }
        let trashed = self.files.trash(path)?;
        self.files.remove_detached(&trashed)
    }
}
