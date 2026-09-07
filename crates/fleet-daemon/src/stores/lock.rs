//! Cross-process coordination for state persistence.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{DaemonError, DaemonResult, adapters::process::pid_is_alive};

static PROCESS_RECLAMATION: Mutex<()> = Mutex::new(());

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
        Self::acquire_with_timeout_after_stale_observation(path, timeout, || {})
    }

    fn acquire_with_timeout_after_stale_observation<F>(
        path: impl Into<PathBuf>,
        timeout: Duration,
        mut after_stale_observation: F,
    ) -> DaemonResult<Self>
    where
        F: FnMut(),
    {
        let path = path.into();
        let parent = path.parent().ok_or_else(|| {
            DaemonError::Validation(format!("lock path has no parent: {}", path.display()))
        })?;
        fs::create_dir_all(parent).map_err(|error| DaemonError::fs(parent, error))?;
        let started = Instant::now();
        let owner = std::process::id();
        loop {
            let reclamation = ReclamationLock::acquire(parent)?;
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(format!("{owner}\n").as_bytes())
                        .and_then(|()| file.sync_all())
                        .map_err(|error| DaemonError::fs(&path, error))?;
                    return Ok(Self { path, owner });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path)? {
                        after_stale_observation();
                        match fs::remove_file(&path) {
                            Ok(()) => continue,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(error) => return Err(DaemonError::fs(&path, error)),
                        }
                    }
                    drop(reclamation);
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

struct ReclamationLock {
    _process: MutexGuard<'static, ()>,
    directory: fs::File,
    path: PathBuf,
}

impl ReclamationLock {
    fn acquire(path: &Path) -> DaemonResult<Self> {
        let process = PROCESS_RECLAMATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = fs::File::open(path).map_err(|error| DaemonError::fs(path, error))?;
        // SAFETY: `directory` owns a valid descriptor for the duration of the call.
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(DaemonError::fs(path, std::io::Error::last_os_error()));
        }
        Ok(Self {
            _process: process,
            directory,
            path: path.to_path_buf(),
        })
    }
}

impl Drop for ReclamationLock {
    fn drop(&mut self) {
        // SAFETY: `directory` remains open until after this drop implementation returns.
        if unsafe { libc::flock(self.directory.as_raw_fd(), libc::LOCK_UN) } != 0 {
            tracing::warn!(path = %self.path.display(), error = %std::io::Error::last_os_error(), "failed to release lock reclamation guard");
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
    use std::sync::{Barrier, mpsc};

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

    #[test]
    fn competing_reclaimers_preserve_new_owner() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = temp.path().join("state.json.lock");
        fs::write(&path, "4294967295\n").unwrap_or_else(|error| panic!("{error}"));
        let stale_observed = std::sync::Arc::new(Barrier::new(2));
        let release_reclaimer = std::sync::Arc::new(Barrier::new(2));
        let (sender, receiver) = mpsc::channel();
        let contender_path = path.clone();
        let contender_observed = std::sync::Arc::clone(&stale_observed);
        let contender_release = std::sync::Arc::clone(&release_reclaimer);
        let contender = thread::spawn(move || {
            let result = StateLock::acquire_with_timeout_after_stale_observation(
                &contender_path,
                Duration::from_millis(100),
                || {
                    contender_observed.wait();
                    contender_release.wait();
                },
            );
            sender
                .send(result)
                .unwrap_or_else(|error| panic!("{error}"));
        });

        stale_observed.wait();
        let competing_reclaimer_could_enter = PROCESS_RECLAMATION.try_lock().is_ok();
        release_reclaimer.wait();
        let guard = receiver
            .recv()
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|error| panic!("first reclaimer failed: {error}"));
        contender
            .join()
            .unwrap_or_else(|error| std::panic::resume_unwind(error));
        assert!(!competing_reclaimer_could_enter);

        let second = StateLock::acquire_with_timeout(&path, Duration::from_millis(60));
        assert!(matches!(second, Err(DaemonError::Timeout(_))));
        assert_eq!(
            fs::read_to_string(&path)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok()),
            Some(guard.owner)
        );
        drop(guard);
    }
}
