use std::{path::Path, path::PathBuf, sync::Arc};

use fleet_proto::{
    media::{MAX_UPLOAD_BYTES, MAX_UPLOAD_FILES},
    request::{StageEntry, UploadId},
};

use super::{ItemFailure, PreparedBatch};
use crate::media::{
    Attachment, PreparedManifest, drop_entry_limit_message, drop_size_limit_message,
    prepare_manifest,
};

#[derive(Clone)]
pub(super) enum DataSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

pub(super) struct UploadFile {
    pub(super) source: DataSource,
    pub(super) size: u64,
}

pub(super) struct PreparedUpload {
    pub(super) name: String,
    pub(super) entry: StageEntry,
    pub(super) files: Vec<UploadFile>,
    pub(super) total_bytes: u64,
    pub(super) entry_count: usize,
    pub(super) skipped: usize,
}

pub(super) struct PreparedItem {
    pub(super) index: usize,
    pub(super) id: UploadId,
    pub(super) upload: PreparedUpload,
}

pub(super) fn prepare_attachment(
    attachment: Attachment,
    uploads: Vec<UploadId>,
) -> anyhow::Result<PreparedBatch> {
    let mut failures = Vec::new();
    let prepared = match attachment {
        Attachment::Paths(paths) => paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| match prepare_path(path) {
                Ok(prepared) => Some(PreparedItem {
                    index,
                    id: uploads[index],
                    upload: prepared,
                }),
                Err(error) => {
                    failures.push(ItemFailure {
                        index,
                        upload: uploads[index],
                        name: path_label(path),
                        message: error.to_string(),
                    });
                    None
                }
            })
            .collect::<Vec<_>>(),
        Attachment::Blob {
            name,
            format: _,
            bytes,
        } => {
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if size > MAX_UPLOAD_BYTES {
                anyhow::bail!(drop_size_limit_message());
            }
            vec![PreparedItem {
                index: 0,
                id: uploads[0],
                upload: PreparedUpload {
                    name: name.clone(),
                    entry: StageEntry::File { name, size },
                    files: vec![UploadFile {
                        source: DataSource::Bytes(bytes.into()),
                        size,
                    }],
                    total_bytes: size,
                    entry_count: 1,
                    skipped: 0,
                },
            }]
        }
    };

    validate_gesture_limits(&prepared)?;
    Ok(PreparedBatch {
        uploads: prepared,
        failures,
    })
}

pub(super) fn validate_gesture_limits(prepared: &[PreparedItem]) -> anyhow::Result<()> {
    let mut total_bytes = 0_u64;
    let mut total_entries = 0_usize;
    for item in prepared {
        let upload = &item.upload;
        total_bytes = total_bytes
            .checked_add(upload.total_bytes)
            .filter(|total| *total <= MAX_UPLOAD_BYTES)
            .ok_or_else(|| anyhow::anyhow!(drop_size_limit_message()))?;
        total_entries = total_entries
            .checked_add(upload.entry_count.max(1))
            .filter(|total| *total <= MAX_UPLOAD_FILES)
            .ok_or_else(|| anyhow::anyhow!(drop_entry_limit_message()))?;
    }
    Ok(())
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn prepare_path(path: &Path) -> anyhow::Result<PreparedUpload> {
    let PreparedManifest {
        entry,
        files,
        total_bytes,
        entry_count,
        skipped,
    } = prepare_manifest(path)?;
    let sizes = entry_sizes(&entry);
    let files = files
        .into_iter()
        .zip(sizes)
        .map(|(path, size)| UploadFile {
            source: DataSource::Path(path),
            size,
        })
        .collect();
    Ok(PreparedUpload {
        name: entry_name(&entry).to_owned(),
        entry,
        files,
        total_bytes,
        entry_count,
        skipped,
    })
}

pub(super) fn entry_name(entry: &StageEntry) -> &str {
    match entry {
        StageEntry::File { name, .. } | StageEntry::Directory { name, .. } => name,
    }
}

fn entry_sizes(entry: &StageEntry) -> Vec<u64> {
    match entry {
        StageEntry::File { size, .. } => vec![*size],
        StageEntry::Directory { files, .. } => files.iter().map(|file| file.size).collect(),
    }
}
