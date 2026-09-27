//! Thread attachment leaves and inline-byte materialisation.

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use fleet_core::agents::{Attachment, ThreadId};
use fleet_proto::media::{CHUNK_BYTES, decode};
use uuid::Uuid;

use super::{Media, sanitize_top_name};
use crate::{
    DaemonError, DaemonResult,
    adapters::files::{FileKind, FileMetadata},
};

const ENCODED_CHUNK_CHARS: usize = (CHUNK_BYTES / 3) * 4;
pub(crate) const ORPHAN_ATTACHMENT_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

impl Media {
    /// The one normalized attachment-store root shared by staging, providers, SQL, and sweeping.
    pub(crate) fn attachments_root(&self) -> &std::path::Path {
        &self.attachments
    }

    /// Clones the shared filesystem port used by every attachment consumer.
    pub(crate) fn files(&self) -> Arc<dyn crate::adapters::files::Files> {
        Arc::clone(&self.files)
    }

    /// Returns the direct attachment leaf owned by `thread`.
    pub(crate) fn thread_leaf(&self, thread: ThreadId) -> PathBuf {
        self.attachments.join(thread.to_string())
    }

    /// Creates a thread's private leaf without following directory symlinks.
    pub(crate) async fn ensure_thread_leaf(&self, thread: ThreadId) -> DaemonResult<PathBuf> {
        let leaf = self.thread_leaf(thread);
        let files = Arc::clone(&self.files);
        let create = leaf.clone();
        tokio::task::spawn_blocking(move || files.create_private_dir_all(&create))
            .await
            .map_err(|error| {
                DaemonError::Join(format!("create attachment directory: {error}"))
            })??;
        Ok(leaf)
    }

    /// Decodes an inline attachment into the thread leaf and returns its durable path.
    pub(crate) async fn materialize_base64(
        &self,
        thread: ThreadId,
        attachment: &Attachment,
        data: String,
    ) -> DaemonResult<PathBuf> {
        let leaf = self.ensure_thread_leaf(thread).await?;
        let files = Arc::clone(&self.files);
        let allocations = Arc::clone(&self.allocations);
        let name = materialized_name(attachment);
        tokio::task::spawn_blocking(move || {
            let _allocation_guard = allocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            materialize(files.as_ref(), &leaf, &name, &data)
        })
        .await
        .map_err(|error| DaemonError::Join(format!("materialize attachment: {error}")))?
    }

    /// Stats one attachment path through the daemon filesystem boundary.
    pub(crate) async fn attachment_metadata(&self, path: PathBuf) -> DaemonResult<FileMetadata> {
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || files.metadata(&path))
            .await
            .map_err(|error| DaemonError::Join(format!("stat attachment: {error}")))?
    }

    /// Removes old files that no committed transcript row references.
    ///
    /// The invariant is deliberately narrow: only a regular file directly inside a direct
    /// thread leaf may be removed, only after the 24-hour grace, and only when the SQL path index
    /// does not reference it. Directories, symlinks, special entries, and deeper paths survive.
    pub(crate) async fn sweep_orphan_attachments(
        &self,
        referenced: HashSet<PathBuf>,
    ) -> DaemonResult<usize> {
        let files = Arc::clone(&self.files);
        let root = self.attachments.clone();
        let cutoff = self
            .clock
            .epoch_millis()
            .saturating_sub(i64::try_from(ORPHAN_ATTACHMENT_GRACE.as_millis()).unwrap_or(i64::MAX));
        tokio::task::spawn_blocking(move || {
            sweep_orphans(files.as_ref(), &root, &referenced, cutoff)
        })
        .await
        .map_err(|error| DaemonError::Join(format!("sweep attachment orphans: {error}")))?
    }
}

fn materialized_name(attachment: &Attachment) -> String {
    let fallback = match attachment.media_type.as_str() {
        "image/gif" => "attachment.gif",
        "image/jpeg" => "attachment.jpg",
        "image/png" => "attachment.png",
        "image/webp" => "attachment.webp",
        _ => "attachment.bin",
    };
    let name = sanitize_top_name(attachment.name.as_deref().unwrap_or(fallback));
    format!("{}-{name}", Uuid::new_v4())
}

fn materialize(
    files: &dyn crate::adapters::files::Files,
    leaf: &std::path::Path,
    name: &str,
    data: &str,
) -> DaemonResult<PathBuf> {
    let size = decoded_size(data)?;
    let final_path = leaf.join(name);
    let part_path = leaf.join(format!(".{name}.part"));
    files.create_part_file(&part_path, size)?;
    let result = (|| {
        let mut offset = 0_u64;
        for encoded in data.as_bytes().chunks(ENCODED_CHUNK_CHARS) {
            let encoded = std::str::from_utf8(encoded).map_err(|_| {
                DaemonError::Validation("attachment base64 must be ASCII".to_owned())
            })?;
            let bytes = decode(encoded).map_err(|error| {
                DaemonError::Validation(format!("invalid attachment base64: {error}"))
            })?;
            files.write_part(&part_path, offset, &bytes)?;
            offset = offset.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        }
        if offset != size {
            return Err(DaemonError::Validation(format!(
                "attachment base64 decoded to {offset} bytes; expected {size}"
            )));
        }
        files.rename_part(leaf, &part_path, &final_path)?;
        Ok(final_path.clone())
    })();
    if result.is_err()
        && let Err(error) = files.remove_part_tree(leaf, &part_path)
    {
        tracing::warn!(
            path = %part_path.display(),
            %error,
            "could not clean up a rejected inline attachment"
        );
    }
    result
}

pub(crate) fn decoded_size(data: &str) -> DaemonResult<u64> {
    if !data.is_ascii() || !data.len().is_multiple_of(4) {
        return Err(DaemonError::Validation(
            "attachment base64 has invalid length or characters".to_owned(),
        ));
    }
    let padding = data
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count();
    if padding > 2 {
        return Err(DaemonError::Validation(
            "attachment base64 has invalid padding".to_owned(),
        ));
    }
    let groups = data.len() / 4;
    let decoded = groups
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_sub(padding))
        .ok_or_else(|| DaemonError::Validation("attachment base64 size overflowed".to_owned()))?;
    u64::try_from(decoded)
        .map_err(|_| DaemonError::Validation("attachment base64 size overflowed".to_owned()))
}

fn sweep_orphans(
    files: &dyn crate::adapters::files::Files,
    root: &std::path::Path,
    referenced: &HashSet<PathBuf>,
    cutoff: i64,
) -> DaemonResult<usize> {
    if !files.exists(root) {
        return Ok(0);
    }
    let root = crate::adapters::files::absolute_lexical(root);
    let mut removed = 0;
    for leaf in files.list(&root)? {
        let leaf = crate::adapters::files::absolute_lexical(&leaf);
        if leaf.parent() != Some(root.as_path()) {
            return Err(DaemonError::Validation(format!(
                "attachment sweep listed a non-leaf path: {}",
                leaf.display()
            )));
        }
        let Ok(leaf_metadata) = files.metadata(&leaf) else {
            continue;
        };
        if leaf_metadata.kind != FileKind::Directory {
            continue;
        }
        for candidate in files.list(&leaf)? {
            let candidate = crate::adapters::files::absolute_lexical(&candidate);
            if candidate.parent() != Some(leaf.as_path()) || referenced.contains(&candidate) {
                continue;
            }
            let Ok(metadata) = files.metadata(&candidate) else {
                continue;
            };
            if metadata.kind != FileKind::File || metadata.modified_millis >= cutoff {
                continue;
            }
            if files.remove_file_if_unchanged(&candidate, metadata)? {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapters::files::Files as _,
        testing::fakes::{FakeFiles, FixedClock},
    };

    #[test]
    fn decoded_size_accounts_for_padding() {
        assert_eq!(decoded_size("").expect("empty"), 0);
        assert_eq!(decoded_size("YQ==").expect("one byte"), 1);
        assert_eq!(decoded_size("YWI=").expect("two bytes"), 2);
        assert_eq!(decoded_size("YWJj").expect("three bytes"), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn orphan_sweep_removes_only_old_unreferenced_regular_leaf_files() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-26T12:00:00Z")
            .expect("time")
            .with_timezone(&chrono::Utc);
        let root = PathBuf::from("/fleet/agents/attachments");
        let leaf = root.join(ThreadId::new().to_string());
        let files = Arc::new(FakeFiles::new(PathBuf::from("/trash"), vec![root.clone()]));
        let old = now.timestamp_millis() - 25 * 60 * 60 * 1_000;
        let fresh = now.timestamp_millis() - 60 * 60 * 1_000;
        let orphan = leaf.join("orphan.png");
        let referenced = leaf.join("referenced.png");
        let fresh_file = leaf.join("fresh.png");
        let directory = leaf.join("directory");
        let symlink = leaf.join("linked.png");
        let root_file = root.join("not-in-a-leaf.png");
        files.insert_text_at(&orphan, "old orphan", old);
        files.insert_text_at(&referenced, "old referenced", old);
        files.insert_text_at(&fresh_file, "fresh", fresh);
        files
            .create_dir_all(&directory)
            .expect("directory candidate");
        files.insert_text_at(directory.join("nested.png"), "nested", old);
        // `Other` is the no-follow metadata kind the fake uses for symlinks and special entries.
        files.insert_other(&symlink);
        files.insert_text_at(&root_file, "root file", old);
        let media = Media::with_roots(
            Arc::<FakeFiles>::clone(&files),
            Arc::new(FixedClock::new(now)),
            PathBuf::from("/downloads"),
            root,
        );

        let removed = media
            .sweep_orphan_attachments(HashSet::from([referenced.clone()]))
            .await
            .expect("sweep");

        assert_eq!(removed, 1);
        assert!(!files.exists(&orphan));
        for kept in [referenced, fresh_file, directory, symlink, root_file] {
            assert!(files.exists(&kept), "kept {}", kept.display());
        }
    }
}
