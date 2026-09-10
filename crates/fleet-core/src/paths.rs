//! Pure construction of Fleet's on-disk layout and compatibility marker records.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    github::PrTab,
    ids::{BoardId, RepoId, WorktreeId},
};

/// Prepared-copy freshness marker file name retained for swarm compatibility.
const HOT_MARKER_FILE: &str = "swarm-hot.json";
/// Worktree publish-intent marker file name retained for swarm compatibility.
const CREATING_MARKER_FILE: &str = "swarm-creating.json";
/// Repository clone publish-intent marker file name.
pub const CLONE_PUBLISH_MARKER_FILE: &str = ".fleet-clone-publish.json";

/// Root of Fleet's persistent filesystem layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetHome {
    root: PathBuf,
}

impl FleetHome {
    /// Creates layout helpers rooted at `home`.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { root: home.into() }
    }

    /// Returns the Fleet home directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Returns the per-board document directory.
    pub fn boards_dir(&self) -> PathBuf {
        self.root.join("boards")
    }
    /// Returns one board's JSON document path.
    pub fn board_path(&self, id: &BoardId) -> PathBuf {
        self.boards_dir().join(format!("{id}.json"))
    }
    /// Returns `config.json`.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }
    /// Returns `state.json`.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
    /// Returns the cross-process state lock path.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join("state.json.lock")
    }
    /// Returns the default pristine repository directory.
    #[must_use]
    pub fn repos_dir(&self) -> PathBuf {
        self.root.join("repos")
    }
    /// Returns the default worktree directory.
    #[must_use]
    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }
    /// Returns the cache root.
    fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// Returns the GitHub cache root.
    #[must_use]
    pub fn github_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("github")
    }
    /// Returns a GitHub owner repository cache path.
    #[must_use]
    pub fn github_owner_cache_path(&self, owner: &str) -> PathBuf {
        self.github_cache_dir().join(format!("{owner}.json"))
    }
    /// Returns a repository pull-request tab cache path.
    #[must_use]
    pub fn pr_cache_path(&self, repo: &RepoId, tab: PrTab) -> PathBuf {
        self.github_cache_dir()
            .join("prs")
            .join(repo.owner())
            .join(repo.name())
            .join(format!("{tab}.json"))
    }
    /// Returns the OpenSSH control socket directory.
    #[must_use]
    pub fn ssh_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("ssh")
    }
    /// Returns the logs directory.
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    /// Returns the background jobs log directory.
    #[must_use]
    pub fn jobs_log_dir(&self) -> PathBuf {
        self.logs_dir().join("jobs")
    }
    /// Returns the recoverable deletion directory.
    #[must_use]
    pub fn trash_dir(&self) -> PathBuf {
        self.root.join("trash")
    }
    /// Returns the daemon Unix socket path.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.root.join("fleetd.sock")
    }
    /// Returns the daemon PID path.
    #[must_use]
    pub fn pid_path(&self) -> PathBuf {
        self.root.join("fleetd.pid")
    }
}

/// Reason a Fleet home could not be resolved from its argument and the environment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HomeError {
    /// A leading `~` was given but `$HOME` is unset.
    #[error("HOME is not set; cannot expand {}", .path.display())]
    UnexpandableTilde {
        /// The unexpandable path as given.
        path: PathBuf,
    },
    /// No home was given and `$HOME` is unset.
    #[error("HOME is not set; pass --home or set FLEET_HOME")]
    Missing,
}

/// Resolves the selected Fleet home, including a shell-quoted leading `~`.
///
/// Defaults to `$HOME/.fleet` when no home is given. Every Fleet binary resolves
/// `--home`/`FLEET_HOME` through this function so they agree on one directory.
pub fn resolve_home(home: Option<PathBuf>) -> Result<PathBuf, HomeError> {
    resolve_home_with(home, std::env::var_os("HOME").map(PathBuf::from))
}

/// Resolves a Fleet home against an explicit environment home, for callers that discover
/// `$HOME` themselves.
pub fn resolve_home_with(
    home: Option<PathBuf>,
    environment_home: Option<PathBuf>,
) -> Result<PathBuf, HomeError> {
    match home {
        Some(path) => match path.strip_prefix("~") {
            Ok(suffix) => environment_home
                .map(|home| home.join(suffix))
                .ok_or(HomeError::UnexpandableTilde { path }),
            Err(_) => Ok(path),
        },
        None => environment_home
            .map(|home| home.join(".fleet"))
            .ok_or(HomeError::Missing),
    }
}

/// Returns prepared-copy slot 0 (`.hot`) or slot n (`.hot.<n>`).
#[must_use]
pub fn slot_path(repo_worktrees_dir: impl AsRef<Path>, slot: usize) -> PathBuf {
    repo_worktrees_dir.as_ref().join(slot_name(slot))
}

/// Returns a prepared-copy worker staging path.
#[must_use]
pub fn slot_staging_path(repo_worktrees_dir: impl AsRef<Path>, slot: usize) -> PathBuf {
    repo_worktrees_dir
        .as_ref()
        .join(format!("{}.staging", slot_name(slot)))
}

/// Returns a prepared-copy worker PID path.
#[must_use]
pub fn slot_pid_path(repo_worktrees_dir: impl AsRef<Path>, slot: usize) -> PathBuf {
    repo_worktrees_dir
        .as_ref()
        .join(format!("{}.staging.pid", slot_name(slot)))
}

/// Returns the private path a worktree creation attempt builds into before publishing.
#[must_use]
pub fn uuid_attempt_path(
    repo_worktrees_dir: impl AsRef<Path>,
    slug: &str,
    attempt: Uuid,
) -> PathBuf {
    repo_worktrees_dir
        .as_ref()
        .join(format!("{slug}.creating-{attempt}"))
}

/// Returns the prepared-copy marker path inside a copy's `.git` directory.
#[must_use]
pub fn hot_marker_path(copy: impl AsRef<Path>) -> PathBuf {
    copy.as_ref().join(".git").join(HOT_MARKER_FILE)
}

/// Returns the publish-intent marker path inside a worktree's `.git` directory.
#[must_use]
pub fn creating_marker_path(worktree: impl AsRef<Path>) -> PathBuf {
    worktree.as_ref().join(".git").join(CREATING_MARKER_FILE)
}

/// Returns the publish-intent marker path inside a cloned repository's `.git` directory.
#[must_use]
pub fn clone_publish_marker_path(repository: impl AsRef<Path>) -> PathBuf {
    repository
        .as_ref()
        .join(".git")
        .join(CLONE_PUBLISH_MARKER_FILE)
}

fn slot_name(slot: usize) -> String {
    if slot == 0 {
        ".hot".to_owned()
    } else {
        format!(".hot.{slot}")
    }
}

/// Contents of `.git/swarm-hot.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotMarker {
    /// ISO-8601 fetch time.
    pub fetched_at: String,
    /// Default branch at preparation time.
    pub default_branch: String,
    /// Prepared remote-tracking commit, 40–64 hexadecimal characters.
    pub sha: String,
    /// SHA-256 fingerprint of ordered prepare commands.
    pub prepare_fingerprint: String,
}

/// Contents of `.git/swarm-creating.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatingMarker {
    /// Unique create-attempt identifier.
    pub id: String,
    /// Parent repository.
    pub repo_id: RepoId,
    /// Checked-out branch.
    pub branch: String,
    /// Ref used to create the branch.
    pub base_ref: String,
    /// ISO-8601 marker creation time.
    pub created_at: String,
}

impl CreatingMarker {
    /// Returns the worktree identifier implied by this marker and slug.
    pub fn worktree_id(&self, slug: &str) -> Result<WorktreeId, crate::ids::IdError> {
        WorktreeId::try_from(format!("{}#{slug}", self.repo_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_layout_paths() {
        let home = FleetHome::new("/tmp/.fleet");
        assert_eq!(home.config_path(), PathBuf::from("/tmp/.fleet/config.json"));
        assert_eq!(home.socket_path(), PathBuf::from("/tmp/.fleet/fleetd.sock"));
        let repo = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            home.pr_cache_path(&repo, PrTab::Review),
            PathBuf::from("/tmp/.fleet/cache/github/prs/acme/api/review.json")
        );
        assert_eq!(
            clone_publish_marker_path("/tmp/repo"),
            PathBuf::from("/tmp/repo/.git/.fleet-clone-publish.json")
        );
    }

    #[test]
    fn resolves_default_and_shell_quoted_tilde_homes() {
        let environment_home = PathBuf::from("/home/df");

        assert_eq!(
            resolve_home_with(None, Some(environment_home.clone())).unwrap(),
            environment_home.join(".fleet")
        );
        assert_eq!(
            resolve_home_with(Some(PathBuf::from("~/.fleet")), Some(environment_home)).unwrap(),
            PathBuf::from("/home/df/.fleet")
        );
    }

    #[test]
    fn preserves_explicit_paths_and_requires_home_only_for_tilde() {
        assert_eq!(
            resolve_home_with(Some(PathBuf::from("/srv/fleet")), None).unwrap(),
            PathBuf::from("/srv/fleet")
        );
        assert_eq!(
            resolve_home_with(Some(PathBuf::from("~df/.fleet")), None).unwrap(),
            PathBuf::from("~df/.fleet")
        );
        assert!(resolve_home_with(Some(PathBuf::from("~/.fleet")), None).is_err());
    }

    #[test]
    fn constructs_slot_and_attempt_paths() {
        assert_eq!(
            slot_path("/w/acme/api", 0),
            PathBuf::from("/w/acme/api/.hot")
        );
        assert_eq!(
            slot_staging_path("/w/acme/api", 2),
            PathBuf::from("/w/acme/api/.hot.2.staging")
        );
        assert_eq!(
            slot_pid_path("/w/acme/api", 0),
            PathBuf::from("/w/acme/api/.hot.staging.pid")
        );
        assert_eq!(
            uuid_attempt_path("/w/acme/api", "feat", Uuid::nil()),
            PathBuf::from("/w/acme/api/feat.creating-00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            hot_marker_path("/w/acme/api/.hot"),
            PathBuf::from("/w/acme/api/.hot/.git/swarm-hot.json")
        );
    }
}
