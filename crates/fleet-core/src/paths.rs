//! Pure construction of Fleet's on-disk layout and compatibility marker records.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    github::PrTab,
    ids::{BoardId, RepoId, TerminalId, WorktreeId},
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
    /// Returns the agents directory.
    #[must_use]
    pub fn agents_path(&self) -> PathBuf {
        self.root.join("agents")
    }
    /// Returns the agents database path.
    #[must_use]
    pub fn agents_db_path(&self) -> PathBuf {
        self.agents_path().join("state.sqlite")
    }
    /// Returns the agents attachments directory.
    #[must_use]
    pub fn agents_attachments_path(&self) -> PathBuf {
        self.agents_path().join("attachments")
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
    /// Returns the directory holding one socket and one sidecar per detached PTY holder.
    #[must_use]
    pub fn pty_dir(&self) -> PathBuf {
        self.root.join("pty")
    }
    /// Returns a holder's sidecar path, the record a restarted daemon reattaches from.
    #[must_use]
    pub fn pty_sidecar_path(&self, terminal: TerminalId) -> PathBuf {
        self.pty_dir().join(format!("{terminal}.json"))
    }
    /// Returns the log every detached PTY holder appends its own diagnostics to.
    #[must_use]
    pub fn pty_log_path(&self) -> PathBuf {
        self.logs_dir().join("pty-hold.log")
    }
}

/// Longest `sun_path` a Unix socket address can carry.
///
/// macOS caps `sockaddr_un.sun_path` at 104 bytes including the terminator and Linux at 108, so
/// the smaller of the two is the portable budget and 100 leaves room for the terminator.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Longest socket file name [`pty_socket_path`] produces: `<terminal>-<nonce>.sock`.
///
/// A `TerminalId` is a `u64`, so at most 20 digits; the nonce is 16 hexadecimal characters.
const MAX_SOCKET_NAME_BYTES: usize = 20 + 1 + 16 + 5;

/// Returns the directory a Fleet home's PTY holders bind their sockets in.
///
/// `<home>/pty` keeps every runtime file for a home in one place. When a name under it could
/// exceed the platform's `sun_path` budget — a deep `TMPDIR` under test, or a Fleet home nested
/// far down — sockets move to a private directory in the system temporary directory instead. The
/// holder refuses to bind unless that directory is owned by this user with mode 0700, and the
/// sidecar records the path that was chosen, so nothing downstream has to repeat this decision.
#[must_use]
pub fn pty_socket_dir(home: &FleetHome) -> PathBuf {
    let preferred = home.pty_dir();
    if preferred.as_os_str().len() + 1 + MAX_SOCKET_NAME_BYTES <= MAX_SOCKET_PATH_BYTES {
        return preferred;
    }
    socket_fallback_dir(home)
}

/// Returns the private temporary directory a home's sockets fall back to.
///
/// `$TMPDIR` first, because that is where a user's runtime files belong; `/tmp` when even that is
/// too deep for `sun_path`, which macOS's per-user `$TMPDIR` can be. Either way the holder binds
/// only after checking the directory is owned by this user with mode 0700.
fn socket_fallback_dir(home: &FleetHome) -> PathBuf {
    let name = format!("fleet-pty-{:016x}", home_fingerprint(home.root()));
    let preferred = std::env::temp_dir().join(&name);
    if preferred.as_os_str().len() + 1 + MAX_SOCKET_NAME_BYTES <= MAX_SOCKET_PATH_BYTES {
        return preferred;
    }
    PathBuf::from("/tmp").join(name)
}

/// Returns the socket path for one spawn of a terminal.
///
/// The nonce makes the name unique per spawn, not per terminal. Terminal identifiers are reused —
/// `restart_terminal` keeps the id — and two holders must never share a pathname: a lingering
/// predecessor would otherwise unlink its successor's socket on the way out, and a local attacker
/// could pre-create a name it can predict.
#[must_use]
pub fn pty_socket_path(home: &FleetHome, terminal: TerminalId, nonce: &str) -> PathBuf {
    pty_socket_dir(home).join(format!("{terminal}-{nonce}.sock"))
}

/// Returns the file-name prefix every socket of `terminal` shares.
///
/// The only way back to a holder whose sidecar is unreadable: its socket still carries its
/// terminal identifier.
#[must_use]
pub fn pty_socket_prefix(terminal: TerminalId) -> String {
    format!("{terminal}-")
}

/// Returns a stable, short digest of a Fleet home, used to keep fallback directories distinct.
fn home_fingerprint(root: &Path) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    hasher.finish()
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

/// Expands a leading `~` against `$HOME`, leaving every other path unchanged.
///
/// Configuration values reach process arguments verbatim — no shell interprets them — so a
/// configured `~/.ssh/id_ed25519` is an unusable relative path unless it is expanded here.
#[must_use]
pub fn expand_tilde(path: impl Into<PathBuf>) -> PathBuf {
    expand_tilde_with(path, std::env::var_os("HOME").map(PathBuf::from))
}

/// Expands a leading `~` against an explicit home, for callers that discover `$HOME`
/// themselves and for tests.
#[must_use]
pub fn expand_tilde_with(path: impl Into<PathBuf>, environment_home: Option<PathBuf>) -> PathBuf {
    let path = path.into();
    let Some(home) = environment_home else {
        return path;
    };
    match path.strip_prefix("~") {
        Ok(suffix) => home.join(suffix),
        Err(_) => path,
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
    fn expands_only_a_leading_tilde_and_only_with_a_known_home() {
        let home = Some(PathBuf::from("/home/df"));
        assert_eq!(
            expand_tilde_with("~/.ssh/id_ed25519", home.clone()),
            PathBuf::from("/home/df/.ssh/id_ed25519")
        );
        assert_eq!(
            expand_tilde_with("~", home.clone()),
            PathBuf::from("/home/df")
        );
        assert_eq!(
            expand_tilde_with("/etc/ssh/key", home),
            PathBuf::from("/etc/ssh/key")
        );
        // A path that cannot be expanded is passed through rather than silently rewritten.
        assert_eq!(
            expand_tilde_with("~/.ssh/id_ed25519", None),
            PathBuf::from("~/.ssh/id_ed25519")
        );
    }

    #[test]
    fn constructs_layout_paths() {
        let home = FleetHome::new("/tmp/.fleet");
        assert_eq!(home.config_path(), PathBuf::from("/tmp/.fleet/config.json"));
        assert_eq!(home.agents_path(), PathBuf::from("/tmp/.fleet/agents"));
        assert_eq!(
            home.agents_db_path(),
            PathBuf::from("/tmp/.fleet/agents/state.sqlite")
        );
        assert_eq!(
            home.agents_attachments_path(),
            PathBuf::from("/tmp/.fleet/agents/attachments")
        );
        assert_eq!(home.socket_path(), PathBuf::from("/tmp/.fleet/fleetd.sock"));
        assert_eq!(home.pty_dir(), PathBuf::from("/tmp/.fleet/pty"));
        assert_eq!(
            home.pty_sidecar_path(TerminalId(7)),
            PathBuf::from("/tmp/.fleet/pty/7.json")
        );
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
    fn short_homes_keep_their_holder_socket_and_long_ones_fall_back() {
        let home = FleetHome::new("/tmp/.fleet");
        assert_eq!(pty_socket_dir(&home), PathBuf::from("/tmp/.fleet/pty"));
        assert_eq!(
            pty_socket_path(&home, TerminalId(7), "0123456789abcdef"),
            PathBuf::from("/tmp/.fleet/pty/7-0123456789abcdef.sock")
        );
        assert!(
            pty_socket_path(&home, TerminalId(7), "0123456789abcdef")
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&pty_socket_prefix(TerminalId(7))))
        );

        let deep = FleetHome::new(format!("/tmp/{}/fleet", "nested/".repeat(16)));
        let fallback = pty_socket_dir(&deep);
        assert!(
            !fallback.starts_with(deep.root()),
            "a home too deep for sun_path must not keep its sockets inside itself"
        );
        assert!(
            fallback
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("fleet-pty-")),
            "{} is not a private holder directory",
            fallback.display()
        );
        assert_ne!(
            fallback,
            pty_socket_dir(&FleetHome::new("/tmp/other/fleet"))
        );
    }

    #[test]
    fn every_holder_socket_fits_the_portable_sun_path_budget() {
        // The worst case a caller can reach: the longest identifier and the deepest home.
        for home in [
            FleetHome::new("/tmp/.fleet"),
            FleetHome::new(format!("/tmp/{}/fleet", "nested/".repeat(16))),
        ] {
            let path = pty_socket_path(&home, TerminalId(u64::MAX), "0123456789abcdef");
            assert!(
                path.as_os_str().len() <= MAX_SOCKET_PATH_BYTES,
                "{} exceeds the sun_path budget",
                path.display()
            );
        }
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
