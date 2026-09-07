use super::*;
use fleet_core::{config::Config, sessions::WorktreeStatus, state::State};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct InventoryCache {
    persisted: Option<(FileRevision, FileRevision, Arc<State>, Arc<Config>)>,
    pools: HashMap<PathBuf, (FileRevision, u64, u32)>,
}

/// Atomic store replacement changes file identity; directory changes invalidate slot counts.
#[derive(Clone, PartialEq, Eq)]
struct FileRevision {
    modified: std::time::SystemTime,
    len: u64,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileRevision {
    /// Reads inode identity through `std::fs` rather than the `Files` adapter on purpose:
    /// the cache key is the on-disk identity of an atomically replaced store, which the
    /// `Files` boundary does not model. Under a non-real `Files` implementation every
    /// revision is `None`, which disables the cache without changing any answer.
    fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        })
    }
}

impl Services {
    /// Assembles an authoritative snapshot from real persisted state and jobs with runtime sessions.
    pub async fn snapshot(&self) -> DaemonResult<Snapshot> {
        let (state, config, pools) = self.inventory().await?;
        let (sessions, runtime_statuses) = self.sessions.snapshot_with_statuses(&state);
        let generated_at = chrono::Utc::now().to_rfc3339();
        let statuses = merge_statuses(&state.worktrees, Some(&runtime_statuses));
        let hosts = self.hosts.snapshot(&config, &generated_at).await;
        Ok(Snapshot {
            boards: self.boards.summaries().await,
            generated_at,
            contexts: state.contexts.clone(),
            repos: state.repos.clone(),
            clones: state.clones.clone(),
            worktrees: state.worktrees.clone(),
            active_context: state.active_context_id.clone(),
            sessions,
            statuses,
            pools,
            hosts,
            jobs: self.jobs.list(),
            daemon: DaemonInfo {
                version: Self::version(),
                pid: std::process::id(),
                started_at: self.started_at.clone(),
                home: self.home.to_string_lossy().into_owned(),
            },
        })
    }

    async fn inventory(&self) -> DaemonResult<(Arc<State>, Arc<Config>, Vec<PoolStatus>)> {
        let mut cache = self.inventory.lock().await;
        let state_path = self.state.path().to_path_buf();
        let config_path = self.config.path().to_path_buf();
        let revisions = tokio::task::spawn_blocking(move || {
            (
                FileRevision::read(&state_path),
                FileRevision::read(&config_path),
            )
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?;
        let cached = match (&cache.persisted, &revisions) {
            (
                Some((state_revision, config_revision, state, config)),
                (Some(current_state), Some(current_config)),
            ) if state_revision == current_state && config_revision == current_config => {
                Some((Arc::clone(state), Arc::clone(config)))
            }
            _ => None,
        };
        let (state, config) = if let Some(cached) = cached {
            cached
        } else {
            let (state, config) = tokio::try_join!(self.state.load(), self.config.load())?;
            let state = Arc::new(state);
            let config = Arc::new(config);
            cache.persisted = match revisions {
                (Some(state_revision), Some(config_revision)) => Some((
                    state_revision,
                    config_revision,
                    Arc::clone(&state),
                    Arc::clone(&config),
                )),
                _ => None,
            };
            (state, config)
        };
        let previous = std::mem::take(&mut cache.pools);
        let state_for_scan = Arc::clone(&state);
        let config_for_scan = Arc::clone(&config);
        let files = Arc::clone(&self.adapters.files);
        let (counts, entries) = tokio::task::spawn_blocking(move || {
            let mut entries = HashMap::new();
            let counts = state_for_scan
                .repos
                .iter()
                .map(|repo| {
                    let root = Path::new(&config_for_scan.worktrees_dir)
                        .join(&repo.owner)
                        .join(&repo.name);
                    let revision = FileRevision::read(&root);
                    let size = config_for_scan.hot_pool_size;
                    let ready = previous
                        .get(&root)
                        .filter(|(cached_revision, cached_size, _)| {
                            Some(cached_revision) == revision.as_ref() && *cached_size == size
                        })
                        .map(|(_, _, ready)| *ready)
                        .unwrap_or_else(|| {
                            let ready = (0..size)
                                .filter(|slot| {
                                    usize::try_from(*slot)
                                        .ok()
                                        .is_some_and(|slot| files.exists(&slot_path(&root, slot)))
                                })
                                .count();
                            u32::try_from(ready).unwrap_or(u32::MAX)
                        });
                    if let Some(revision) = revision {
                        entries.insert(root, (revision, size, ready));
                    }
                    (repo.id.clone(), ready)
                })
                .collect::<Vec<_>>();
            (counts, entries)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?;
        cache.pools = entries;
        drop(cache);
        let refreshed = self.pool_refreshed_at.read().await;
        let pools = counts
            .into_iter()
            .map(|(repo, ready)| PoolStatus {
                refreshed_at: refreshed.get(&repo).cloned(),
                repo,
                ready,
                size: u32::try_from(config.hot_pool_size).unwrap_or(u32::MAX),
            })
            .collect();
        Ok((state, config, pools))
    }
}

pub(super) fn merge_statuses(
    worktrees: &[fleet_core::model::Worktree],
    observed: Option<&[WorktreeStatus]>,
) -> Vec<WorktreeStatus> {
    let observed = observed
        .unwrap_or_default()
        .iter()
        .map(|status| (&status.worktree_id, status))
        .collect::<BTreeMap<_, _>>();
    worktrees
        .iter()
        .map(|worktree| {
            observed.get(&worktree.id).map_or_else(
                || Inspect::unknown_status(worktree),
                |status| (*status).clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{
        clock::SystemClock,
        files::{Files, RealFiles},
    };
    use fleet_core::{
        config::{NATIVE_LAZYGIT, WindowConfig},
        ids::WorktreeId,
        model::{Context, Repo, RepoHooks, Worktree},
        sessions::SessionState,
        state::default_state,
    };

    #[tokio::test]
    async fn inventory_reuses_unchanged_data_and_invalidates_atomic_writes_and_slots() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        config.load().await.unwrap();
        let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
        let mut initial = default_state();
        initial.contexts.push(Context {
            id: "team".parse().unwrap(),
            name: "Team".into(),
            owners: vec!["owner".into()],
            created_at: "2026-09-06T00:00:00Z".into(),
        });
        initial.repos.push(Repo {
            id: "owner/repo".parse().unwrap(),
            owner: "owner".into(),
            name: "repo".into(),
            url: "https://example.invalid/owner/repo".into(),
            context_id: "team".parse().unwrap(),
            default_branch: "main".into(),
            path: home.join("repos/owner/repo").display().to_string(),
            cloned_at: "2026-09-06T00:00:00Z".into(),
            hooks: RepoHooks::default(),
        });
        state.save(initial).await.unwrap();
        let root = home.join("worktrees/owner/repo");
        files.create_dir_all(&root).unwrap();
        let services = Services::new(
            home,
            config.clone(),
            state.clone(),
            Arc::new(JobManager::new(home)),
            Adapters::system(files.clone()),
        );
        let (first_state, first_config, first_pools) = services.inventory().await.unwrap();
        assert_eq!(first_pools[0].ready, 0);
        let (same_state, same_config, _) = services.inventory().await.unwrap();
        assert!(Arc::ptr_eq(&first_state, &same_state));
        assert!(Arc::ptr_eq(&first_config, &same_config));
        files.create_dir_all(&root.join(".hot")).unwrap();
        let (_, _, pools) = services.inventory().await.unwrap();
        assert_eq!(pools[0].ready, 1);
        config
            .update(serde_json::json!({"hotPoolSize": 2}))
            .await
            .unwrap();
        let (_, updated_config, pools) = services.inventory().await.unwrap();
        assert!(!Arc::ptr_eq(&first_config, &updated_config));
        assert_eq!(pools[0].size, 2);
        state
            .transaction(|state| {
                state.contexts[0].name = "Renamed".into();
                Ok(())
            })
            .await
            .unwrap();
        let (updated_state, _, _) = services.inventory().await.unwrap();
        assert!(!Arc::ptr_eq(&first_state, &updated_state));
        assert_eq!(updated_state.contexts[0].name, "Renamed");
        files
            .rename(&root.join(".hot"), &root.join("claimed"))
            .unwrap();
        assert_eq!(services.inventory().await.unwrap().2[0].ready, 0);
    }

    #[tokio::test]
    async fn snapshot_never_mixes_session_generations() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let mut effective = config.load().await.unwrap();
        effective.windows = vec![WindowConfig {
            name: "git".into(),
            command: NATIVE_LAZYGIT.into(),
        }];
        config.save(effective).await.unwrap();
        let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
        let context: fleet_core::ids::ContextId = "team".parse().unwrap();
        let repo: RepoId = "owner/repo".parse().unwrap();
        let worktree = WorktreeId::try_from("owner/repo#main").unwrap();
        let mut initial = default_state();
        initial.contexts.push(Context {
            id: context.clone(),
            name: "Team".into(),
            owners: vec!["owner".into()],
            created_at: "2026-09-06T00:00:00Z".into(),
        });
        initial.repos.push(Repo {
            id: repo.clone(),
            owner: "owner".into(),
            name: "repo".into(),
            url: "https://example.invalid/owner/repo".into(),
            context_id: context,
            default_branch: "main".into(),
            path: home.join("repos/owner/repo").display().to_string(),
            cloned_at: "2026-09-06T00:00:00Z".into(),
            hooks: RepoHooks::default(),
        });
        initial.worktrees.push(Worktree {
            id: worktree.clone(),
            repo_id: repo,
            slug: "main".into(),
            branch: "main".into(),
            base_ref: "origin/main".into(),
            path: home.join("worktrees/owner/repo/main").display().to_string(),
            session: "repo/main".into(),
            host: None,
            created_at: "2026-09-06T00:00:00Z".into(),
            last_opened_at: None,
            degraded: None,
        });
        state.save(initial).await.unwrap();
        let services = Services::new(
            home,
            config,
            state,
            Arc::new(JobManager::new(home)),
            Adapters::system(files),
        );
        services
            .sessions
            .ensure(Some(worktree.clone()), None, false)
            .await
            .unwrap();
        let snapshot = services.snapshot().await.unwrap();

        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.statuses[0].session, SessionState::Detached);
        assert_eq!(snapshot.statuses[0].windows.len(), 1);
    }
}
