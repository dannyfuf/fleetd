//! Retention and safety policy for staged downloads.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, NaiveDateTime, Utc};
use tokio_util::sync::CancellationToken;

use super::{Media, UploadState};
use crate::{
    DaemonError, DaemonResult,
    adapters::files::{FileKind, Files},
};

const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Completed downloads remain available for seven days before Fleet removes them.
pub(super) const DOWNLOAD_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const ORPHAN_PART_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

impl Media {
    /// Runs cancellation-aware cleanup of Fleet-stamped staged downloads.
    pub(crate) async fn run_sweep(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = self.sweep(DOWNLOAD_RETENTION).await {
                        tracing::warn!(%error, "failed to sweep staged media downloads");
                    }
                }
            }
        }
    }

    /// Removes expired entries that Fleet can prove it owns from the downloads staging root.
    ///
    /// An entry is eligible only when it is a direct child of that root, its name begins with
    /// Fleet's `YYYYMMDD-HHMMSS-` stamp, and it is either a regular file or a directory whose
    /// entire tree contains only regular files and directories. A symlink or special node at any
    /// depth preserves the whole entry, and deletion never follows symlinks. Stamped `.part`
    /// entries not present in the live upload table use a fixed one-day retention; every other
    /// stamped entry uses `older_than`. Thread attachment leaves are outside this root and are
    /// never listed or removed by this sweep.
    pub(crate) async fn sweep(&self, older_than: Duration) -> DaemonResult<usize> {
        let now = self.clock.now();
        let completed_cutoff = retention_cutoff(now, older_than)?;
        let orphan_cutoff = retention_cutoff(now, ORPHAN_PART_RETENTION)?;
        let live_parts = self.live_part_paths().await;
        let files = Arc::clone(&self.files);
        let downloads = self.downloads.clone();
        tokio::task::spawn_blocking(move || {
            sweep_downloads(
                files.as_ref(),
                &downloads,
                &live_parts,
                completed_cutoff,
                orphan_cutoff,
            )
        })
        .await
        .map_err(|error| DaemonError::Join(format!("sweep staged media downloads: {error}")))?
    }

    async fn live_part_paths(&self) -> HashSet<PathBuf> {
        let slots = self
            .uploads
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut paths = HashSet::with_capacity(slots.len());
        for slot in slots {
            let upload = slot.upload.lock().await;
            if let UploadState::Active(prepared) = &upload.state {
                paths.insert(prepared.part_path.clone());
            }
        }
        paths
    }
}

fn retention_cutoff(now: DateTime<Utc>, retention: Duration) -> DaemonResult<DateTime<Utc>> {
    let retention = chrono::Duration::from_std(retention)
        .map_err(|error| DaemonError::Validation(format!("invalid media retention: {error}")))?;
    now.checked_sub_signed(retention)
        .ok_or_else(|| DaemonError::Validation("media retention cutoff is out of range".to_owned()))
}

fn sweep_downloads(
    files: &dyn Files,
    downloads: &Path,
    live_parts: &HashSet<PathBuf>,
    completed_cutoff: DateTime<Utc>,
    orphan_cutoff: DateTime<Utc>,
) -> DaemonResult<usize> {
    if !files.exists(downloads) {
        return Ok(0);
    }
    let downloads = crate::adapters::files::absolute_lexical(downloads);
    let mut removed = 0;
    for candidate in files.list(&downloads)? {
        let candidate = crate::adapters::files::absolute_lexical(&candidate);
        if candidate.parent() != Some(downloads.as_path()) {
            return Err(DaemonError::Validation(format!(
                "media sweep listed a non-child path: {}",
                candidate.display()
            )));
        }
        let Some((stamped_at, is_part)) = stamped_entry(&candidate) else {
            continue;
        };
        let cutoff = if is_part {
            if live_parts.contains(&candidate) {
                continue;
            }
            orphan_cutoff
        } else {
            completed_cutoff
        };
        if stamped_at > cutoff {
            continue;
        }
        let safe = match tree_contains_only_regular_entries(files, &candidate) {
            Ok(safe) => safe,
            Err(error) => {
                tracing::warn!(
                    path = %candidate.display(),
                    %error,
                    "could not inspect a staged media candidate during sweep"
                );
                continue;
            }
        };
        if !safe {
            continue;
        }
        if let Err(error) = files.remove_part_tree(&downloads, &candidate) {
            tracing::warn!(
                path = %candidate.display(),
                %error,
                "could not remove a staged media candidate during sweep"
            );
            continue;
        }
        removed += 1;
    }
    Ok(removed)
}

fn stamped_entry(path: &Path) -> Option<(DateTime<Utc>, bool)> {
    let name = path.file_name()?.to_str()?;
    let bytes = name.as_bytes();
    if bytes.len() < 16
        || !bytes[..8].iter().all(u8::is_ascii_digit)
        || bytes[8] != b'-'
        || !bytes[9..15].iter().all(u8::is_ascii_digit)
        || bytes[15] != b'-'
    {
        return None;
    }
    let stamped_at = NaiveDateTime::parse_from_str(&name[..15], "%Y%m%d-%H%M%S")
        .ok()?
        .and_utc();
    Some((stamped_at, name.ends_with(".part")))
}

fn tree_contains_only_regular_entries(files: &dyn Files, root: &Path) -> DaemonResult<bool> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        match files.metadata(&path)?.kind {
            FileKind::File => {}
            FileKind::Directory => pending.extend(files.list(&path)?),
            FileKind::Other => return Ok(false),
        }
    }
    Ok(true)
}
