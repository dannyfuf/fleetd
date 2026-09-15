//! Run artifact directory layout.
//!
//! Every run owns one directory. It is created before anything else starts, printed on the
//! first line of output so a failed run can be found, and it survives the run: it is the
//! evidence. Only the hermetic `home/` is reclaimed, and only after a clean run without
//! `--keep`.

use crate::env::{APP_SOCKET_NAME, DAEMON_SOCKET_RELATIVE};
use anyhow::Context as _;
use fleet_drive::protocol::{Request, Response};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncWriteExt as _;

/// Where run directories are created when `--run-dir` is not given.
const DEFAULT_ROOT: &str = "/tmp/fleet-harness";

/// The longest a Unix-socket path may be, in bytes.
///
/// `sockaddr_un::sun_path` is 108 bytes on Linux including its terminator, and `fleetd` binds
/// `<home>/fleetd.sock` inside this directory. A run directory that pushes past it fails with
/// "path must be shorter than SUN_LEN" buried in `fixture/seed-fleetd.log` while stdout says
/// only that the daemon never became ready, so the length is checked here instead — before
/// anything is created, naming the path and the limit.
const MAX_SOCKET_PATH: usize = 107;

/// The longest socket path relative to a run directory.
///
/// Keep the choice derived from the environment's socket names so renaming either socket cannot
/// silently weaken the depth guard.
const LONGEST_SOCKET_NAME: &str = if APP_SOCKET_NAME.len() >= DAEMON_SOCKET_RELATIVE.len() {
    APP_SOCKET_NAME
} else {
    DAEMON_SOCKET_RELATIVE
};

/// The directory a single scenario run writes all of its artifacts into.
#[derive(Debug, Clone)]
pub struct RunDirectory {
    /// The directory itself.
    pub root: PathBuf,
    /// Timestamped request/response journal, one JSON object per line.
    pub journal: PathBuf,
    /// Verbatim copy of the scenario source that ran.
    pub scenario: PathBuf,
    /// Fleet's stdout and stderr.
    pub app_log: PathBuf,
    /// The private daemon's stdout and stderr.
    pub daemon_log: PathBuf,
    /// `NNN-<name>.png` screenshots.
    pub shots: PathBuf,
    /// `NNN-<name>.json` snapshot dumps.
    pub dumps: PathBuf,
    /// The private `FLEET_HOME` this run gave `fleetd` and Fleet.
    pub home: PathBuf,
}

impl RunDirectory {
    /// Creates the directory and its subdirectories, defaulting to
    /// `/tmp/fleet-harness/<UTC timestamp>-<scenario stem>[-N]/`.
    pub fn create(requested: Option<&Path>, scenario: &Path) -> anyhow::Result<Self> {
        Self::create_at(
            requested,
            scenario,
            SystemTime::now(),
            Path::new(DEFAULT_ROOT),
        )
    }

    /// Atomically claims the root of a directory run without populating single-run artifacts.
    ///
    /// An explicit root must not already exist. A default root follows the same timestamp and
    /// collision-suffix policy as a single run.
    pub fn create_suite(requested: Option<&Path>, directory: &Path) -> anyhow::Result<PathBuf> {
        Self::create_suite_at(
            requested,
            directory,
            SystemTime::now(),
            Path::new(DEFAULT_ROOT),
        )
    }

    fn create_suite_at(
        requested: Option<&Path>,
        directory: &Path,
        now: SystemTime,
        default_root: &Path,
    ) -> anyhow::Result<PathBuf> {
        claim_root(requested, directory, now, default_root)
    }

    fn create_at(
        requested: Option<&Path>,
        scenario: &Path,
        now: SystemTime,
        default_root: &Path,
    ) -> anyhow::Result<Self> {
        let root = claim_root(requested, scenario, now, default_root)?;
        let shots = root.join("shots");
        let dumps = root.join("dumps");
        let home = root.join("home");
        for directory in [&shots, &dumps, &home] {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("create run directory {}", directory.display()))?;
        }
        Ok(Self {
            journal: root.join("run.jsonl"),
            scenario: root.join("scenario.txt"),
            app_log: root.join("app.log"),
            daemon_log: root.join("fleetd.log"),
            shots,
            dumps,
            home,
            root,
        })
    }

    /// A short identifier unique to this run, safe as a compositor output name and a window
    /// title fragment.
    ///
    /// The process id is part of it because the directory name alone is not unique: a suite
    /// numbers its scenarios `001-<stem>`, so `002-help` is the run id of the second scenario
    /// of *every* suite run. Two runs on one box would then ask the compositor for the same
    /// output name and paint windows with the same title, and each would move or remove the
    /// other's. Runs inside one process are sequential, so the pid is enough to tell them apart.
    pub fn run_id(&self) -> String {
        let stem = sanitize(&self.root.file_name().unwrap_or_default().to_string_lossy());
        format!("{stem}-{}", std::process::id())
    }

    /// Stores the exact scenario source that ran beside its artifacts.
    pub async fn write_scenario(&self, source: &str) -> anyhow::Result<()> {
        tokio::fs::write(&self.scenario, source)
            .await
            .with_context(|| format!("write {}", self.scenario.display()))
    }

    /// Appends one correlated command exchange to the journal.
    pub async fn record(
        &self,
        line: usize,
        request: &Request,
        response: &Response,
    ) -> anyhow::Result<()> {
        self.append(serde_json::json!({
            "at": rfc3339(utc(SystemTime::now())),
            "kind": "command",
            "request": request,
            "response": response,
            "data": { "line": line },
        }))
        .await
    }

    /// Appends one runner-side journal entry: a fixture, a fault, a lane choice, a failure.
    pub async fn record_event(&self, kind: &str, data: serde_json::Value) -> anyhow::Result<()> {
        self.append(serde_json::json!({
            "at": rfc3339(utc(SystemTime::now())),
            "kind": kind,
            "data": data,
        }))
        .await
    }

    /// Writes `dumps/NNN-<name>.json`, numbered by the scenario line that asked for it.
    pub async fn write_dump(
        &self,
        sequence: usize,
        name: &str,
        value: &serde_json::Value,
    ) -> anyhow::Result<PathBuf> {
        self.write_json(
            self.dumps
                .join(format!("{sequence:03}-{}.json", sanitize(name))),
            value,
        )
        .await
    }

    /// Writes `dumps/failure-NNN.json` for the line that failed the scenario.
    pub async fn write_failure_dump(
        &self,
        sequence: usize,
        value: &serde_json::Value,
    ) -> anyhow::Result<PathBuf> {
        self.write_json(
            self.dumps.join(format!("failure-{sequence:03}.json")),
            value,
        )
        .await
    }

    /// Reclaims the private `FLEET_HOME`, which is the only part of a run directory that is
    /// ever removed. Missing is success: teardown may run twice.
    pub fn remove_home(&self) -> anyhow::Result<()> {
        match std::fs::remove_dir_all(&self.home) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(anyhow::Error::new(error).context(format!("remove {}", self.home.display())))
            }
        }
    }

    async fn write_json(
        &self,
        path: PathBuf,
        value: &serde_json::Value,
    ) -> anyhow::Result<PathBuf> {
        let body = serde_json::to_vec_pretty(value).context("encode snapshot dump")?;
        tokio::fs::write(&path, body)
            .await
            .with_context(|| format!("write {}", path.display()))?;
        Ok(path)
    }

    async fn append(&self, entry: serde_json::Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(&entry).context("encode journal entry")?;
        line.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.journal)
            .await
            .with_context(|| format!("open {}", self.journal.display()))?;
        file.write_all(&line)
            .await
            .with_context(|| format!("append to {}", self.journal.display()))?;
        // `tokio::fs::File` buffers, and a run that dies mid-scenario must still leave the
        // journal entries it already recorded behind.
        file.flush()
            .await
            .with_context(|| format!("flush {}", self.journal.display()))
    }
}

/// Claims an explicit root or the first free timestamped default candidate.
fn claim_root(
    requested: Option<&Path>,
    subject: &Path,
    now: SystemTime,
    default_root: &Path,
) -> anyhow::Result<PathBuf> {
    match requested {
        Some(path) => {
            ensure_socket_path_fits(path)?;
            anyhow::ensure!(
                claim(path)?,
                "the run directory {} already exists; refusing to use a directory this run did not create",
                path.display()
            );
            Ok(path.to_owned())
        }
        None => {
            std::fs::create_dir_all(default_root)
                .with_context(|| format!("create default run root {}", default_root.display()))?;
            let stem = format!(
                "{}-{}",
                stamp(utc(now)),
                sanitize(&subject.file_stem().unwrap_or_default().to_string_lossy())
            );
            let mut suffix = 1_u64;
            loop {
                let name = if suffix == 1 {
                    stem.clone()
                } else {
                    format!("{stem}-{suffix}")
                };
                let candidate = default_root.join(name);
                ensure_socket_path_fits(&candidate)?;
                if claim(&candidate)? {
                    return Ok(candidate);
                }
                suffix = suffix
                    .checked_add(1)
                    .context("exhausted run-directory collision suffixes")?;
            }
        }
    }
}

/// Atomically claims `path`, returning false when another run already owns it.
fn claim(path: &Path) -> anyhow::Result<bool> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(error) => {
            Err(anyhow::Error::new(error)
                .context(format!("claim run directory {}", path.display())))
        }
    }
}

fn ensure_socket_path_fits(root: &Path) -> anyhow::Result<()> {
    let socket = root.join(LONGEST_SOCKET_NAME);
    let length = socket.as_os_str().as_encoded_bytes().len();
    anyhow::ensure!(
        length <= MAX_SOCKET_PATH,
        "the run directory {} is too deep: its longest socket {} would be {length} bytes, \
         and a Unix socket path may be at most {MAX_SOCKET_PATH}. Pass a shorter --run-dir.",
        root.display(),
        socket.display()
    );
    Ok(())
}

/// Keeps a name usable as a path component, a compositor output name and a window title.
fn sanitize(value: &str) -> String {
    let kept: String = value
        .chars()
        .map(|character| {
            // `.` is deliberately not kept: `lane::validate_run_id` rejects it, and a run id is
            // minted from this, so keeping it would let `--run-dir /tmp/my.run` create the
            // directory, seed the fixture and only then fail when the lane reads the name.
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if kept.is_empty() {
        "scenario".to_owned()
    } else {
        kept
    }
}

/// A wall-clock instant split into UTC calendar fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Utc {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    millis: u32,
}

/// `YYYYmmdd-HHMMSS`, for a run directory name.
fn stamp(at: Utc) -> String {
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    )
}

/// RFC 3339 in UTC with milliseconds, for a journal entry.
fn rfc3339(at: Utc) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        at.year, at.month, at.day, at.hour, at.minute, at.second, at.millis
    )
}

/// Splits an instant into UTC fields.
///
/// `fleet-harness` does not take a calendar dependency for two fixed formats, so the
/// days-to-civil-date conversion (Howard Hinnant's algorithm) is inlined and pinned by tests.
/// Instants before the Unix epoch are clamped to it; the harness only ever formats "now".
fn utc(now: SystemTime) -> Utc {
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(i64::try_from(seconds / 86_400).unwrap_or(0));
    Utc {
        year,
        month,
        day,
        hour: u32::try_from(day_seconds / 3_600).unwrap_or(0),
        minute: u32::try_from((day_seconds % 3_600) / 60).unwrap_or(0),
        second: u32::try_from(day_seconds % 60).unwrap_or(0),
        millis: since_epoch.subsec_millis(),
    }
}

/// Converts days since the Unix epoch into a proleptic Gregorian year, month and day.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64, millis: u32) -> Utc {
        utc(UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_millis(u64::from(millis)))
    }

    #[test]
    fn utc_conversion_matches_known_instants() {
        assert_eq!(rfc3339(at(0, 0)), "1970-01-01T00:00:00.000Z");
        // A leap day, the case the civil-from-days algorithm exists for.
        assert_eq!(rfc3339(at(951_782_400, 0)), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339(at(1_000_000_000, 250)), "2001-09-09T01:46:40.250Z");
        assert_eq!(rfc3339(at(1_700_000_000, 1)), "2023-11-14T22:13:20.001Z");
        assert_eq!(stamp(at(1_700_000_000, 0)), "20231114-221320");
    }

    #[test]
    fn run_directory_is_populated_and_named_from_the_scenario() {
        let root = std::env::temp_dir().join(format!(
            "fleet-harness-rundir-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let run = RunDirectory::create(Some(&root), Path::new("/tmp/help me.scenario"))
            .expect("create run directory");
        assert!(run.shots.is_dir() && run.dumps.is_dir() && run.home.is_dir());
        let stem = sanitize(&root.file_name().unwrap_or_default().to_string_lossy());
        let run_id = run.run_id();
        assert!(
            run_id.starts_with(&stem) && run_id.ends_with(&std::process::id().to_string()),
            "the run id names the directory and this process: {run_id}"
        );
        crate::lane::validate_run_id_for_test(&run_id)
            .unwrap_or_else(|error| panic!("a minted run id must satisfy the lane: {error}"));
        run.remove_home().expect("remove home");
        assert!(!run.home.exists());
        run.remove_home()
            .expect("removing a missing home is success");
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn socket_guard_uses_the_longer_environment_socket_name() {
        assert_eq!(
            LONGEST_SOCKET_NAME.len(),
            APP_SOCKET_NAME.len().max(DAEMON_SOCKET_RELATIVE.len())
        );
        assert!(
            LONGEST_SOCKET_NAME == APP_SOCKET_NAME || LONGEST_SOCKET_NAME == DAEMON_SOCKET_RELATIVE
        );
    }

    /// A run directory deep enough to overrun `sun_path` has to be refused where the reader
    /// can see it, not by `fleetd` failing to bind inside its own log.
    #[test]
    fn roots_of_length_89_and_90_are_refused_up_front() {
        const PREFIX: &str = "/tmp/";
        for length in [89, 90] {
            let root = PathBuf::from(format!("{PREFIX}{}", "x".repeat(length - PREFIX.len())));
            assert_eq!(root.as_os_str().as_encoded_bytes().len(), length);
            let error = RunDirectory::create(Some(&root), Path::new("help.scenario"))
                .expect_err("this root cannot hold the app socket");
            let message = format!("{error:#}");
            assert!(
                message.contains("too deep") && message.contains("--run-dir"),
                "the failure names the cause and the fix: {message}"
            );
            assert!(!root.exists(), "nothing is created before the check");
        }
    }

    #[test]
    fn two_default_creates_in_the_same_second_claim_distinct_directories() {
        let temporary = tempfile::tempdir().expect("temporary default root");
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let scenario = Path::new("help.scenario");
        let first = RunDirectory::create_at(None, scenario, now, temporary.path())
            .expect("claim the unsuffixed directory");
        let second = RunDirectory::create_at(None, scenario, now, temporary.path())
            .expect("claim a suffixed directory");

        assert_ne!(first.root, second.root);
        assert_eq!(
            second.root.file_name().and_then(|name| name.to_str()),
            Some("20231114-221320-help-2")
        );
        assert!(first.root.is_dir() && second.root.is_dir());
    }

    #[test]
    fn an_explicit_suite_root_is_claimed_without_single_run_artifacts() {
        let temporary = tempfile::tempdir().expect("temporary parent");
        let root = temporary.path().join("suite");

        let suite = RunDirectory::create_suite(Some(&root), Path::new("scenarios"))
            .expect("claim the suite root");

        assert_eq!(suite, root);
        let entries = std::fs::read_dir(&suite)
            .expect("read suite root")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect suite entries");
        assert!(entries.is_empty(), "a new suite root starts empty");
    }

    #[test]
    fn an_existing_explicit_suite_root_is_left_to_its_owner() {
        let temporary = tempfile::tempdir().expect("temporary parent");
        let root = temporary.path().join("suite");
        std::fs::create_dir(&root).expect("create an existing suite root");
        let marker = root.join("report.md");
        std::fs::write(&marker, b"another suite").expect("write ownership marker");

        RunDirectory::create_suite(Some(&root), Path::new("scenarios"))
            .expect_err("an existing suite root belongs to another run");

        assert_eq!(
            std::fs::read(&marker).expect("read ownership marker"),
            b"another suite"
        );
        assert_eq!(
            std::fs::read_dir(&root)
                .expect("read existing suite root")
                .count(),
            1
        );
    }

    #[test]
    fn default_suite_roots_use_collision_suffixes_and_stay_empty() {
        let temporary = tempfile::tempdir().expect("temporary default root");
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let directory = Path::new("scenarios");
        let first = RunDirectory::create_suite_at(None, directory, now, temporary.path())
            .expect("claim the unsuffixed suite root");
        let second = RunDirectory::create_suite_at(None, directory, now, temporary.path())
            .expect("claim a suffixed suite root");

        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some("20231114-221320-scenarios")
        );
        assert_eq!(
            second.file_name().and_then(|name| name.to_str()),
            Some("20231114-221320-scenarios-2")
        );
        for suite in [first, second] {
            assert_eq!(
                std::fs::read_dir(&suite).expect("read suite root").count(),
                0,
                "a suite root holds no single-run artifacts"
            );
        }
    }

    #[test]
    fn a_failed_claim_leaves_the_existing_directory_untouched() {
        let temporary = tempfile::tempdir().expect("temporary parent");
        let root = temporary.path().join("already-owned");
        let daemon_socket = root.join(DAEMON_SOCKET_RELATIVE);
        let app_socket = root.join(APP_SOCKET_NAME);
        std::fs::create_dir(&root).expect("create the existing directory");
        std::fs::create_dir(root.join("home")).expect("create its existing home");
        std::fs::write(&daemon_socket, b"daemon-owner").expect("seed the daemon socket marker");
        std::fs::write(&app_socket, b"app-owner").expect("seed the app socket marker");

        RunDirectory::create(Some(&root), Path::new("help.scenario"))
            .expect_err("an existing directory belongs to another run");

        assert_eq!(
            std::fs::read(&daemon_socket).expect("read the daemon socket marker"),
            b"daemon-owner"
        );
        assert_eq!(
            std::fs::read(&app_socket).expect("read the app socket marker"),
            b"app-owner"
        );
        assert!(!root.join("shots").exists());
        assert!(!root.join("dumps").exists());
    }

    #[test]
    fn names_with_separators_never_escape_their_directory() {
        assert_eq!(sanitize("hub/open palette"), "hub-open-palette");
        assert_eq!(sanitize(".."), "--");
        assert_eq!(sanitize("my.run"), "my-run");
        assert_eq!(sanitize(""), "scenario");
    }
}
