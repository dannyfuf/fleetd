//! Prepared worktree creation, recovery, publication, and deletion.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use fleet_core::{
    config::Config,
    ids::{JobId, RepoId, SessionId, WorktreeId},
    model::{Degraded, Repo, RepoHooks, Worktree},
    paths::{CreatingMarker, creating_marker_path, hot_marker_path, uuid_attempt_path},
    slug::slugify,
    validate::{validate_branch, validate_slug},
};
use fleet_proto::{
    job::{JobKind, JobRecord},
    response::WorktreeDeleteResult,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        Adapters,
        files::Files,
        git::Git,
        github::Github,
        process::pid_is_alive,
        shell::{Shell, ShellCommand},
    },
    error::remote_unsupported,
    jobs::{JobCtx, JobManager},
    stores::{config::ConfigStore, state::StateStore},
};

use super::{
    awaited::{JobDelivery, copy_error},
    branch_refspec, check_cancelled, discard_path, hooks,
    pool::{CancellableCopy, Pool},
    repo_worktrees_dir,
    sessions::Sessions,
};

mod creation;
mod post_create;
mod publication;
mod recovery;
mod trash;

const TRASH_MARKER_FILE: &str = "fleet-trash.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostCreateIntent {
    worktree: Worktree,
    hooks: Vec<String>,
    pid: u32,
    status_path: PathBuf,
    log_path: PathBuf,
    intent_path: PathBuf,
}

/// Worktree service owning prepared-copy claim, publication, and deletion orchestration.
#[derive(Clone)]
pub struct Worktrees {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    files: Arc<dyn Files>,
    git: Arc<dyn Git>,
    shell: Arc<dyn Shell>,
    github: Arc<dyn Github>,
    sessions: Sessions,
    pool: Pool,
    trash_jobs: Arc<Mutex<HashMap<String, JobId>>>,
    startup_ready: Arc<AtomicBool>,
    startup_notify: Arc<tokio::sync::Notify>,
}

impl Worktrees {
    /// Creates the worktree service and schedules startup intent recovery. Creation drives
    /// files, Git, the shell, and GitHub, so it takes the shared adapter bundle.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        adapters: &Adapters,
        sessions: Sessions,
    ) -> Self {
        let service = Self {
            pool: Pool::new(
                Arc::clone(&config),
                Arc::clone(&state),
                Arc::clone(&jobs),
                Arc::clone(&adapters.git),
                Arc::clone(&adapters.files),
                Arc::clone(&adapters.shell),
            ),
            config,
            state,
            jobs,
            files: Arc::clone(&adapters.files),
            git: Arc::clone(&adapters.git),
            shell: Arc::clone(&adapters.shell),
            github: Arc::clone(&adapters.github),
            sessions,
            trash_jobs: Arc::new(Mutex::new(HashMap::new())),
            startup_ready: Arc::new(AtomicBool::new(false)),
            startup_notify: Arc::new(tokio::sync::Notify::new()),
        };
        service.schedule_startup_recovery();
        service
    }

    /// Hard-kills the session associated with a worktree.
    pub async fn kill(&self, id: WorktreeId) -> DaemonResult<()> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        self.kill_session(&worktree.session).await
    }

    /// Kills a worktree's session, treating an already-gone session as success.
    async fn kill_session(&self, session: &str) -> DaemonResult<()> {
        let session = SessionId::try_from(session)
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
        match self.sessions.kill(session).await {
            Ok(()) | Err(DaemonError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Updates `lastOpenedAt` transactionally without changing worktree identity.
    pub async fn touch_opened(&self, id: WorktreeId) -> DaemonResult<()> {
        let now = Utc::now().to_rfc3339();
        self.state
            .transaction(move |state| {
                let worktree = state
                    .worktrees
                    .iter_mut()
                    .find(|worktree| worktree.id == id)
                    .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
                worktree.last_opened_at = Some(now);
                Ok(())
            })
            .await
    }

    /// Resolves the absolute path of a local worktree and rejects remote mirrors.
    pub async fn path(&self, id: WorktreeId) -> DaemonResult<String> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .into_iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        let path = PathBuf::from(&worktree.path);
        if !path.is_absolute() {
            return Err(DaemonError::Validation(format!(
                "worktree {id} has a non-absolute path"
            )));
        }
        Ok(path.to_string_lossy().into_owned())
    }

    async fn repo_and_config(&self, id: &RepoId) -> DaemonResult<(Repo, Config)> {
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let repo = state
            .repos
            .into_iter()
            .find(|repo| &repo.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("repository {id}")))?;
        Ok((repo, config))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrashMarker {
    original_path: String,
    worktree: Worktree,
}

struct AttemptGuard {
    files: Arc<dyn Files>,
    path: PathBuf,
    armed: bool,
}

impl AttemptGuard {
    fn new(files: Arc<dyn Files>, path: PathBuf) -> Self {
        Self {
            files,
            path,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ignored = discard_path(self.files.as_ref(), &self.path);
    }
}

fn trash_marker_path(root: &Path) -> PathBuf {
    root.join(".git").join(TRASH_MARKER_FILE)
}

fn validate_restore_marker(marker: &TrashMarker, config: &Config) -> DaemonResult<()> {
    if marker.worktree.host.is_some() {
        return Err(remote_unsupported());
    }
    let expected = repo_worktrees_dir(config, &marker.worktree.repo_id).join(&marker.worktree.slug);
    if Path::new(&marker.original_path) != expected || marker.worktree.path != marker.original_path
    {
        return Err(DaemonError::Validation(
            "trash entry does not contain a canonical worktree path".to_owned(),
        ));
    }
    Ok(())
}

fn validate_trash_entry(entry: &str) -> DaemonResult<()> {
    if entry.is_empty()
        || matches!(entry, "." | "..")
        || Path::new(entry).file_name().and_then(|name| name.to_str()) != Some(entry)
    {
        return Err(DaemonError::Validation(
            "trash entry must be one directory name".to_owned(),
        ));
    }
    Ok(())
}

fn worktree_id(repo: &RepoId, slug: &str) -> DaemonResult<WorktreeId> {
    WorktreeId::try_from(format!("{repo}#{slug}"))
        .map_err(|error| DaemonError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_as_trash_entry_names() {
        assert!(validate_trash_entry("123-feature").is_ok());
        assert!(validate_trash_entry("../123-feature").is_err());
        assert!(validate_trash_entry(".").is_err());
    }
}
