//! Buffered progress logs and bounded tail reads.

use super::*;

impl JobManager {
    /// Reads at most the last `lines` lines from a job's persistent log.
    pub async fn tail(&self, id: &JobId, lines: usize) -> DaemonResult<Vec<String>> {
        let path = PathBuf::from(&lock(&self.inner.state).job(id)?.record.log_path);
        self.flush_log(id)?;
        tokio::task::spawn_blocking(move || {
            crate::adapters::logs::tail(&path, lines).map_err(|error| DaemonError::fs(&path, error))
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Returns the persistent log path for a job identifier.
    #[must_use]
    pub fn log_path(&self, id: &JobId) -> PathBuf {
        self.inner.logs_dir.join(format!("{id}.log"))
    }

    pub(crate) fn record_progress(&self, id: &JobId, line: String) -> DaemonResult<()> {
        let (log, path) = self.log_target(id)?;
        self.write_log(&log, &path, Some(&line))?;
        let mut state = lock(&self.inner.state);
        let job = state.job_mut(id)?;
        if job.record.progress.as_ref() != Some(&line) {
            job.record.progress = Some(line);
            let _receivers = self.inner.updates.send(job.record.clone());
        }
        Ok(())
    }

    /// Returns a job's log writer and destination, holding the registry lock only to read them.
    fn log_target(&self, id: &JobId) -> DaemonResult<(JobLog, PathBuf)> {
        let state = lock(&self.inner.state);
        let job = state.job(id)?;
        Ok((Arc::clone(&job.log), PathBuf::from(&job.record.log_path)))
    }

    fn write_log(&self, log: &JobLog, path: &Path, line: Option<&str>) -> DaemonResult<()> {
        let mut log = lock(log);
        let file = match &mut *log {
            Some(file) => file,
            slot @ None => {
                std::fs::create_dir_all(&self.inner.logs_dir)
                    .map_err(|error| DaemonError::fs(&self.inner.logs_dir, error))?;
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|error| DaemonError::fs(path, error))?;
                slot.insert(BufWriter::new(file))
            }
        };
        if let Some(line) = line {
            writeln!(file, "{line}").map_err(|error| DaemonError::fs(path, error))?;
        }
        Ok(())
    }

    pub(super) fn append_log_line(&self, id: &JobId, line: Option<&str>) -> DaemonResult<()> {
        let (log, path) = self.log_target(id)?;
        self.write_log(&log, &path, line)
    }

    pub(crate) fn flush_log(&self, id: &JobId) -> DaemonResult<()> {
        let (log, path) = self.log_target(id)?;
        if let Some(log) = &mut *lock(&log) {
            log.flush().map_err(|error| DaemonError::fs(&path, error))?;
        }
        Ok(())
    }
}
