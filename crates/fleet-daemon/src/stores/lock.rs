//! Cross-process coordination for state persistence.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{DaemonError, DaemonResult, adapters::process::pid_is_alive};

/// Cross-process `state.json.lock` guard using swarm's PID-file semantics.
#[derive(Debug)]
pub struct StateLock {
    path: PathBuf,
    owner: u32,
}

impl StateLock {
    /// Acquires a PID lock, retrying every 25 ms for at most three seconds.
    pub fn acquire(path: impl Into<PathBuf>) -> DaemonResult<Self> {
        Self::acquire_with_timeout(path, Duration::from_secs(3))
    }

    fn acquire_with_timeout(path: impl Into<PathBuf>, timeout: Duration) -> DaemonResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| DaemonError::fs(parent, error))?;
        }
        let started = Instant::now();
        let owner = std::process::id();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(format!("{owner}\n").as_bytes())
                        .and_then(|()| file.sync_all())
                        .map_err(|error| DaemonError::fs(&path, error))?;
                    return Ok(Self { path, owner });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path)? {
                        match fs::remove_file(&path) {
                            Ok(()) => continue,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(error) => return Err(DaemonError::fs(&path, error)),
                        }
                    }
                    if started.elapsed() >= timeout {
                        return Err(DaemonError::Timeout(format!(
                            "waiting for {}",
                            path.display()
                        )));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(DaemonError::fs(&path, error)),
            }
        }
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let still_owned = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            == Some(self.owner);
        if still_owned {
            let _ignored = fs::remove_file(&self.path);
        }
    }
}

fn lock_is_stale(path: &Path) -> DaemonResult<bool> {
    let contents = fs::read_to_string(path).map_err(|error| DaemonError::fs(path, error))?;
    if let Ok(pid) = contents.trim().parse::<u32>() {
        return Ok(!pid_is_alive(pid));
    }
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| DaemonError::fs(path, error))?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    Ok(age >= Duration::from_secs(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_owner_causes_contention_timeout() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = temp.path().join("state.json.lock");
        fs::write(&path, format!("{}\n", std::process::id()))
            .unwrap_or_else(|error| panic!("{error}"));
        let error = StateLock::acquire_with_timeout(&path, Duration::from_millis(60))
            .err()
            .unwrap_or_else(|| panic!("lock unexpectedly acquired"));
        assert!(matches!(error, DaemonError::Timeout(_)));
    }

    #[test]
    fn stale_owner_is_reclaimed() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = temp.path().join("state.json.lock");
        fs::write(&path, "4294967295\n").unwrap_or_else(|error| panic!("{error}"));
        let guard = StateLock::acquire(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(guard.path, path);
    }
}
