//! Durable attachment references projected alongside user items.

use std::path::Path;

use anyhow::Context;
use fleet_core::agents::{AgentEvent, AttachmentSource, ItemKind, SeqEvent, ThreadId};

use crate::adapters::files::{FileKind, Files};

/// Records attachment references inside the same transaction as their user item.
pub(super) fn record(
    transaction: &rusqlite::Transaction<'_>,
    thread: ThreadId,
    event: &SeqEvent,
    files: &dyn Files,
    attachments_root: &Path,
) -> anyhow::Result<()> {
    let AgentEvent::ItemStarted {
        item,
        kind: ItemKind::UserMessage { attachments, .. },
        ..
    } = &event.event
    else {
        return Ok(());
    };
    transaction
        .execute(
            "DELETE FROM item_attachments WHERE thread_id = ?1 AND item_id = ?2",
            rusqlite::params![thread.to_string(), item.to_string()],
        )
        .with_context(|| format!("replace attachments for item {item} of thread {thread}"))?;
    if !attachments
        .iter()
        .any(|attachment| matches!(&attachment.source, AttachmentSource::Path(_)))
    {
        return Ok(());
    }
    let canonical_root = files
        .canonicalize(attachments_root)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "resolve the attachment root `{}` for item {item}",
                attachments_root.display()
            )
        })?;
    for (index, attachment) in attachments.iter().enumerate() {
        let AttachmentSource::Path(path) = &attachment.source else {
            continue;
        };
        let Ok(canonical_path) = files.canonicalize(path) else {
            continue;
        };
        let Ok(relative) = canonical_path.strip_prefix(&canonical_root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let metadata = files
            .metadata(&canonical_path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("stat attachment `{}` for item {item}", path.display()))?;
        if metadata.kind != FileKind::File {
            continue;
        }
        let relative_path = relative.to_string_lossy().replace('\\', "/");
        let file_name = canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment");
        let attachment_id = format!("{index}-{file_name}");
        transaction
            .execute(
                "INSERT INTO item_attachments \
                 (thread_id, item_id, attachment_id, relative_path, mime, bytes) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    thread.to_string(),
                    item.to_string(),
                    attachment_id,
                    relative_path,
                    attachment.media_type,
                    i64::try_from(metadata.len).unwrap_or(i64::MAX),
                ],
            )
            .with_context(|| format!("record attachment {index} for item {item}"))?;
    }
    Ok(())
}
