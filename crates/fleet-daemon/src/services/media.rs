//! Safe, resumable assembly of staged media uploads.

use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use fleet_core::{agents::ThreadId, paths::FleetHome};
use fleet_proto::{
    media::{
        CHUNK_BYTES, MAX_ACTIVE_UPLOADS_PER_OWNER, MAX_UPLOAD_BYTES, MAX_UPLOAD_FILES,
        UPLOAD_IDLE_EXPIRY, decode,
    },
    request::{MediaAnchor, StageEntry, StageOp, StagedFile, UploadId},
    response::ResponseBody,
};
use tokio::{sync::Mutex, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    adapters::{clock::Clock, files::Files},
    error::remote_unsupported,
};

const MAX_COMPONENT_CHARS: usize = 64;
const EXPIRY_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// In-memory upload registry and its two daemon-owned staging roots.
#[derive(Clone)]
pub(crate) struct Media {
    files: Arc<dyn Files>,
    clock: Arc<dyn Clock>,
    downloads: PathBuf,
    attachments: PathBuf,
    uploads: Arc<Mutex<HashMap<UploadId, Arc<UploadSlot>>>>,
    allocations: Arc<StdMutex<()>>,
}

struct UploadSlot {
    owner: u64,
    active: AtomicBool,
    upload: Mutex<Upload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Downloads,
    Thread(ThreadId),
}

struct Upload {
    target: Target,
    entry: StageEntry,
    last_activity: Instant,
    state: UploadState,
}

enum UploadState {
    Preparing,
    Active(PreparedUpload),
    Completed { path: PathBuf, sha256: Vec<String> },
    Closed,
}

struct PreparedUpload {
    root: PathBuf,
    final_path: PathBuf,
    part_path: PathBuf,
    files: Vec<UploadFile>,
}

struct UploadFile {
    label: String,
    path: PathBuf,
    size: u64,
    received: Vec<bool>,
}

struct ValidatedManifest {
    top_name: String,
    total_size: u64,
    kind: ManifestKind,
}

enum ManifestKind {
    File {
        size: u64,
    },
    Directory {
        files: Vec<ManifestFile>,
        dirs: Vec<PathBuf>,
    },
}

struct ManifestFile {
    relative: PathBuf,
    label: String,
    size: u64,
}

impl Media {
    /// Resolves the daemon's download target once for its lifetime.
    #[must_use]
    pub(crate) fn new(home: &FleetHome, files: Arc<dyn Files>, clock: Arc<dyn Clock>) -> Self {
        let downloads = selected_downloads_root(home);
        let attachments = home.agents_attachments_path();
        tracing::info!(
            path = %downloads.display(),
            "selected the media staging directory"
        );
        Self::with_roots(files, clock, downloads, attachments)
    }

    fn with_roots(
        files: Arc<dyn Files>,
        clock: Arc<dyn Clock>,
        downloads: PathBuf,
        attachments: PathBuf,
    ) -> Self {
        Self {
            files,
            clock,
            downloads: crate::adapters::files::absolute_lexical(&downloads),
            attachments: crate::adapters::files::absolute_lexical(&attachments),
            uploads: Arc::new(Mutex::new(HashMap::new())),
            allocations: Arc::new(StdMutex::new(())),
        }
    }

    /// Applies one retry-safe operation to a locally owned test upload.
    #[cfg(test)]
    pub(crate) async fn stage(
        &self,
        anchor: MediaAnchor,
        upload: UploadId,
        op: StageOp,
    ) -> DaemonResult<ResponseBody> {
        self.stage_owned(0, anchor, upload, op).await
    }

    /// Applies one operation while enforcing the live-upload budget of its connection owner.
    pub(crate) async fn stage_owned(
        &self,
        owner: u64,
        anchor: MediaAnchor,
        upload: UploadId,
        op: StageOp,
    ) -> DaemonResult<ResponseBody> {
        let target = self.target(&anchor)?;
        match op {
            StageOp::Begin { entry } => self.begin(owner, upload, target, entry).await,
            StageOp::Chunk { file, offset, data } => {
                self.chunk(upload, &target, file, offset, data).await
            }
            StageOp::Finish { sha256 } => self.finish(upload, &target, sha256).await,
            StageOp::Cancel => self.cancel(upload).await,
        }
    }

    /// Runs cancellation-aware idle upload cleanup for the daemon lifetime.
    pub(crate) async fn run_expiry(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(EXPIRY_CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = self.expire_idle().await {
                        tracing::warn!(%error, "failed to expire idle media uploads");
                    }
                }
            }
        }
    }

    /// Deletes uploads with no successful activity inside [`UPLOAD_IDLE_EXPIRY`].
    ///
    /// Removing a table entry happens before disk cleanup, so a racing operation observes
    /// `NotFound` instead of acknowledging bytes that have just been deleted.
    async fn expire_idle(&self) -> DaemonResult<usize> {
        let now = Instant::now();
        let candidates = self
            .uploads
            .lock()
            .await
            .iter()
            .map(|(id, upload)| (*id, Arc::clone(upload)))
            .collect::<Vec<_>>();
        let mut expired = 0;
        for (id, slot) in candidates {
            let mut upload = slot.upload.lock().await;
            if now.duration_since(upload.last_activity) < UPLOAD_IDLE_EXPIRY {
                continue;
            }
            let cleanup = match std::mem::replace(&mut upload.state, UploadState::Closed) {
                UploadState::Active(prepared) => Some((prepared.root, prepared.part_path)),
                UploadState::Preparing | UploadState::Completed { .. } | UploadState::Closed => {
                    None
                }
            };
            slot.active.store(false, Ordering::Relaxed);
            drop(upload);
            self.remove_if_same(id, &slot).await;
            if let Some((root, part)) = cleanup
                && let Err(error) = self.remove_part(root, part.clone()).await
            {
                tracing::warn!(
                    %id,
                    path = %part.display(),
                    %error,
                    "could not clean up an expired media upload"
                );
            }
            expired += 1;
        }
        Ok(expired)
    }

    async fn begin(
        &self,
        owner: u64,
        id: UploadId,
        target: Target,
        entry: StageEntry,
    ) -> DaemonResult<ResponseBody> {
        let pending = Arc::new(UploadSlot {
            owner,
            active: AtomicBool::new(true),
            upload: Mutex::new(Upload {
                target: target.clone(),
                entry: entry.clone(),
                last_activity: Instant::now(),
                state: UploadState::Preparing,
            }),
        });
        let pending_guard = pending.upload.lock().await;
        let existing = {
            let mut uploads = self.uploads.lock().await;
            if let Some(existing) = uploads.get(&id) {
                Some(Arc::clone(existing))
            } else {
                let active = uploads
                    .values()
                    .filter(|upload| upload.owner == owner && upload.active.load(Ordering::Relaxed))
                    .count();
                if active >= MAX_ACTIVE_UPLOADS_PER_OWNER {
                    return Err(DaemonError::Validation(format!(
                        "connection already has {MAX_ACTIVE_UPLOADS_PER_OWNER} active media uploads"
                    )));
                }
                uploads.insert(id, Arc::clone(&pending));
                None
            }
        };

        if let Some(existing) = existing {
            drop(pending_guard);
            let mut upload = existing.upload.lock().await;
            if upload.target == target && upload.entry == entry {
                upload.last_activity = Instant::now();
                return Ok(ResponseBody::Ack);
            }
            return Err(DaemonError::Validation(format!(
                "upload {id} was already begun with a different target or manifest"
            )));
        }

        let mut upload = pending_guard;
        let manifest = match validate_manifest(&entry) {
            Ok(manifest) => manifest,
            Err(error) => {
                upload.state = UploadState::Closed;
                pending.active.store(false, Ordering::Relaxed);
                drop(upload);
                self.remove_if_same(id, &pending).await;
                return Err(error);
            }
        };
        let root = self.root_for(&target);
        let files = Arc::clone(&self.files);
        let allocations = Arc::clone(&self.allocations);
        let stamp = self.clock.now().format("%Y%m%d-%H%M%S").to_string();
        let prepared = match tokio::task::spawn_blocking(move || {
            let _allocation_guard = allocations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            prepare_on_disk(files.as_ref(), root, &stamp, manifest)
        })
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                upload.state = UploadState::Closed;
                pending.active.store(false, Ordering::Relaxed);
                drop(upload);
                self.remove_if_same(id, &pending).await;
                return Err(DaemonError::Join(format!("prepare media upload: {error}")));
            }
        };
        match prepared {
            Ok(prepared) => {
                upload.state = UploadState::Active(prepared);
                upload.last_activity = Instant::now();
                Ok(ResponseBody::Ack)
            }
            Err(error) => {
                upload.state = UploadState::Closed;
                pending.active.store(false, Ordering::Relaxed);
                drop(upload);
                self.remove_if_same(id, &pending).await;
                Err(error)
            }
        }
    }

    async fn chunk(
        &self,
        id: UploadId,
        target: &Target,
        file: u32,
        offset: u64,
        data: String,
    ) -> DaemonResult<ResponseBody> {
        let bytes = decode(&data).map_err(|error| DaemonError::Validation(error.to_string()))?;
        let slot = self.upload(id).await?;
        let mut upload = slot.upload.lock().await;
        ensure_target(id, &upload, target)?;
        let (path, chunk_index, duplicate) = {
            let prepared = active(id, &mut upload)?;
            let index = usize::try_from(file).map_err(|_| {
                DaemonError::Validation(format!("upload {id} file index {file} is invalid"))
            })?;
            let file = prepared.files.get(index).ok_or_else(|| {
                DaemonError::Validation(format!("upload {id} has no file at index {file}"))
            })?;
            let chunk_index = validate_chunk(id, file, offset, bytes.len())?;
            (file.path.clone(), chunk_index, file.received[chunk_index])
        };
        if duplicate {
            upload.last_activity = Instant::now();
            return Ok(ResponseBody::Ack);
        }
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || files.write_part(&path, offset, &bytes))
            .await
            .map_err(|error| DaemonError::Join(format!("write media chunk: {error}")))??;
        let prepared = active(id, &mut upload)?;
        let index = usize::try_from(file).map_err(|_| {
            DaemonError::Validation(format!("upload {id} file index {file} is invalid"))
        })?;
        prepared.files[index].received[chunk_index] = true;
        upload.last_activity = Instant::now();
        Ok(ResponseBody::Ack)
    }

    async fn finish(
        &self,
        id: UploadId,
        target: &Target,
        expected: Vec<String>,
    ) -> DaemonResult<ResponseBody> {
        let slot = self.upload(id).await?;
        let mut upload = slot.upload.lock().await;
        ensure_target(id, &upload, target)?;
        if let UploadState::Completed { path, sha256 } = &upload.state {
            if sha256 != &expected {
                return Err(DaemonError::Validation(format!(
                    "upload {id} was already finished with different SHA-256 digests"
                )));
            }
            let path = path.clone();
            upload.last_activity = Instant::now();
            return Ok(ResponseBody::Path {
                path: path.to_string_lossy().into_owned(),
                host: None,
            });
        }
        let (files_to_hash, root, part_path, final_path) = {
            let prepared = active(id, &mut upload)?;
            for file in &prepared.files {
                if let Some(missing) = file.received.iter().position(|received| !received) {
                    return Err(DaemonError::Validation(format!(
                        "upload {id} is missing chunk {missing} for {}",
                        file.label
                    )));
                }
            }
            (
                prepared
                    .files
                    .iter()
                    .map(|file| (file.label.clone(), file.path.clone()))
                    .collect::<Vec<_>>(),
                prepared.root.clone(),
                prepared.part_path.clone(),
                prepared.final_path.clone(),
            )
        };

        let mismatch = if expected.len() != files_to_hash.len() {
            Some(format!(
                "upload {id} supplied {} SHA-256 digests for {} files",
                expected.len(),
                files_to_hash.len()
            ))
        } else {
            let files = Arc::clone(&self.files);
            let hashed = tokio::task::spawn_blocking(move || {
                files_to_hash
                    .into_iter()
                    .map(|(label, path)| files.sha256(&path).map(|digest| (label, digest)))
                    .collect::<DaemonResult<Vec<_>>>()
            })
            .await
            .map_err(|error| DaemonError::Join(format!("hash media upload: {error}")))??;
            hashed
                .iter()
                .zip(&expected)
                .find_map(|((label, actual), expected)| {
                    (actual != expected).then(|| {
                        format!("SHA-256 mismatch for {label}: expected {expected}, got {actual}")
                    })
                })
        };

        if let Some(message) = mismatch {
            upload.state = UploadState::Closed;
            slot.active.store(false, Ordering::Relaxed);
            drop(upload);
            self.remove_if_same(id, &slot).await;
            if let Err(error) = self.remove_part(root, part_path).await {
                tracing::warn!(%id, %error, "could not clean up a rejected media upload");
            }
            return Err(DaemonError::Validation(message));
        }

        let files = Arc::clone(&self.files);
        let part_for_rename = part_path.clone();
        let final_for_rename = final_path.clone();
        let root_for_rename = root.clone();
        tokio::task::spawn_blocking(move || {
            files.rename_part(&root_for_rename, &part_for_rename, &final_for_rename)
        })
        .await
        .map_err(|error| DaemonError::Join(format!("publish media upload: {error}")))??;
        upload.state = UploadState::Completed {
            path: final_path.clone(),
            sha256: expected,
        };
        upload.last_activity = Instant::now();
        slot.active.store(false, Ordering::Relaxed);
        Ok(ResponseBody::Path {
            path: final_path.to_string_lossy().into_owned(),
            host: None,
        })
    }

    async fn cancel(&self, id: UploadId) -> DaemonResult<ResponseBody> {
        let slot = self.uploads.lock().await.remove(&id);
        let Some(slot) = slot else {
            return Ok(ResponseBody::Ack);
        };
        let mut upload = slot.upload.lock().await;
        let cleanup = match std::mem::replace(&mut upload.state, UploadState::Closed) {
            UploadState::Active(prepared) => Some((prepared.root, prepared.part_path)),
            UploadState::Preparing | UploadState::Completed { .. } | UploadState::Closed => None,
        };
        slot.active.store(false, Ordering::Relaxed);
        drop(upload);
        if let Some((root, part)) = cleanup {
            self.remove_part(root, part).await?;
        }
        Ok(ResponseBody::Ack)
    }

    fn target(&self, anchor: &MediaAnchor) -> DaemonResult<Target> {
        match anchor {
            MediaAnchor::Local | MediaAnchor::Terminal { .. } => Ok(Target::Downloads),
            MediaAnchor::Thread { thread } => Ok(Target::Thread(*thread)),
            MediaAnchor::Host { .. } => Err(remote_unsupported()),
        }
    }

    fn root_for(&self, target: &Target) -> PathBuf {
        match target {
            Target::Downloads => self.downloads.clone(),
            Target::Thread(thread) => self.attachments.join(thread.to_string()),
        }
    }

    async fn upload(&self, id: UploadId) -> DaemonResult<Arc<UploadSlot>> {
        self.uploads
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("media upload {id}")))
    }

    async fn remove_if_same(&self, id: UploadId, expected: &Arc<UploadSlot>) {
        let mut uploads = self.uploads.lock().await;
        if uploads
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, expected))
        {
            uploads.remove(&id);
        }
    }

    async fn remove_part(&self, root: PathBuf, part: PathBuf) -> DaemonResult<()> {
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || files.remove_part_tree(&root, &part))
            .await
            .map_err(|error| DaemonError::Join(format!("remove media upload: {error}")))?
    }
}

fn active(id: UploadId, upload: &mut Upload) -> DaemonResult<&mut PreparedUpload> {
    match &mut upload.state {
        UploadState::Active(prepared) => Ok(prepared),
        UploadState::Preparing | UploadState::Completed { .. } | UploadState::Closed => {
            Err(DaemonError::NotFound(format!("media upload {id}")))
        }
    }
}

fn ensure_target(id: UploadId, upload: &Upload, target: &Target) -> DaemonResult<()> {
    if &upload.target == target {
        Ok(())
    } else {
        Err(DaemonError::Validation(format!(
            "media upload {id} was addressed through a different anchor"
        )))
    }
}

fn validate_chunk(
    id: UploadId,
    file: &UploadFile,
    offset: u64,
    actual: usize,
) -> DaemonResult<usize> {
    let chunk_bytes = CHUNK_BYTES as u64;
    if !offset.is_multiple_of(chunk_bytes) {
        return Err(DaemonError::Validation(format!(
            "upload {id} offset {offset} is not aligned to {CHUNK_BYTES} bytes"
        )));
    }
    if offset >= file.size {
        return Err(DaemonError::Validation(format!(
            "upload {id} offset {offset} is outside {} ({} bytes)",
            file.label, file.size
        )));
    }
    let expected = usize::try_from((file.size - offset).min(chunk_bytes)).map_err(|_| {
        DaemonError::Validation(format!("upload {id} chunk length cannot be represented"))
    })?;
    if actual != expected {
        return Err(DaemonError::Validation(format!(
            "upload {id} chunk for {} at offset {offset} is {actual} bytes; expected {expected}",
            file.label
        )));
    }
    usize::try_from(offset / chunk_bytes)
        .map_err(|_| DaemonError::Validation(format!("upload {id} chunk index is too large")))
}

fn validate_manifest(entry: &StageEntry) -> DaemonResult<ValidatedManifest> {
    match entry {
        StageEntry::File { name, size } => {
            validate_total_size(*size)?;
            Ok(ValidatedManifest {
                top_name: sanitize_top_name(name),
                total_size: *size,
                kind: ManifestKind::File { size: *size },
            })
        }
        StageEntry::Directory { name, files, dirs } => {
            let count = files.len().checked_add(dirs.len()).ok_or_else(|| {
                DaemonError::Validation("media manifest entry count overflowed".to_owned())
            })?;
            if count > MAX_UPLOAD_FILES {
                return Err(DaemonError::Validation(format!(
                    "media manifest has {count} entries; maximum is {MAX_UPLOAD_FILES}"
                )));
            }
            let mut total_size = 0_u64;
            let mut sanitized_files = Vec::with_capacity(files.len());
            let mut file_paths = HashSet::with_capacity(files.len());
            for StagedFile { relative, size } in files {
                total_size = total_size.checked_add(*size).ok_or_else(|| {
                    DaemonError::Validation("media upload size overflowed".to_owned())
                })?;
                validate_total_size(total_size)?;
                let path = sanitize_relative(relative)?;
                if !file_paths.insert(path.clone()) {
                    return Err(collision(relative, &path));
                }
                sanitized_files.push(ManifestFile {
                    label: slash_path(&path),
                    relative: path,
                    size: *size,
                });
            }

            let mut explicit_dirs = HashSet::with_capacity(dirs.len());
            let mut sanitized_dirs = Vec::with_capacity(dirs.len());
            for relative in dirs {
                let path = sanitize_relative(relative)?;
                if !explicit_dirs.insert(path.clone()) {
                    return Err(collision(relative, &path));
                }
                sanitized_dirs.push(path);
            }

            let mut all_dirs = explicit_dirs;
            for directory in &sanitized_dirs {
                let mut parent = directory.parent();
                while let Some(path) = parent {
                    if path.as_os_str().is_empty() {
                        break;
                    }
                    all_dirs.insert(path.to_path_buf());
                    parent = path.parent();
                }
            }
            for file in &sanitized_files {
                let mut parent = file.relative.parent();
                while let Some(path) = parent {
                    if path.as_os_str().is_empty() {
                        break;
                    }
                    all_dirs.insert(path.to_path_buf());
                    parent = path.parent();
                }
            }
            if let Some(path) = file_paths.intersection(&all_dirs).next() {
                return Err(DaemonError::Validation(format!(
                    "media manifest path {} is both a file and a directory after sanitising",
                    slash_path(path)
                )));
            }
            Ok(ValidatedManifest {
                top_name: sanitize_top_name(name),
                total_size,
                kind: ManifestKind::Directory {
                    files: sanitized_files,
                    dirs: sanitized_dirs,
                },
            })
        }
    }
}

fn validate_total_size(size: u64) -> DaemonResult<()> {
    if size > MAX_UPLOAD_BYTES {
        Err(DaemonError::Validation(format!(
            "media upload is {size} bytes; maximum is {MAX_UPLOAD_BYTES}"
        )))
    } else {
        Ok(())
    }
}

fn sanitize_top_name(name: &str) -> String {
    let stripped = strip_component(name);
    if stripped.is_empty() || stripped.chars().all(|character| character == '.') {
        "paste".to_owned()
    } else {
        truncate_component(&stripped)
    }
}

fn sanitize_relative(relative: &str) -> DaemonResult<PathBuf> {
    if relative.is_empty() || relative.starts_with('/') || Path::new(relative).is_absolute() {
        return Err(DaemonError::Validation(format!(
            "media manifest path is not relative: {relative:?}"
        )));
    }
    let mut path = PathBuf::new();
    for component in relative.split('/') {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(DaemonError::Validation(format!(
                "media manifest path has an unsafe component: {relative:?}"
            )));
        }
        let component = strip_component(component);
        if component.is_empty() || component.chars().all(|character| character == '.') {
            return Err(DaemonError::Validation(format!(
                "media manifest path sanitises to an empty component: {relative:?}"
            )));
        }
        path.push(truncate_component(&component));
    }
    Ok(path)
}

fn strip_component(component: &str) -> String {
    component
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .collect::<String>()
        .trim()
        .to_owned()
}

fn truncate_component(component: &str) -> String {
    let characters = component.chars().collect::<Vec<_>>();
    if characters.len() <= MAX_COMPONENT_CHARS {
        return component.to_owned();
    }
    let extension_start = characters
        .iter()
        .rposition(|character| *character == '.')
        .filter(|index| *index > 0);
    let keep_extension = extension_start
        .filter(|index| characters.len() - index < MAX_COMPONENT_CHARS)
        .unwrap_or(characters.len());
    if keep_extension == characters.len() {
        return characters[..MAX_COMPONENT_CHARS].iter().collect();
    }
    let extension_len = characters.len() - keep_extension;
    let stem_len = MAX_COMPONENT_CHARS - extension_len;
    characters[..stem_len]
        .iter()
        .chain(&characters[keep_extension..])
        .collect()
}

fn collision(original: &str, sanitized: &Path) -> DaemonError {
    DaemonError::Validation(format!(
        "media manifest path {original:?} collides at {} after sanitising",
        slash_path(sanitized)
    ))
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn prepare_on_disk(
    files: &dyn Files,
    root: PathBuf,
    stamp: &str,
    manifest: ValidatedManifest,
) -> DaemonResult<PreparedUpload> {
    files.create_private_dir_all(&root)?;
    if let Some(available) = files.available_bytes(&root)?
        && manifest.total_size > available
    {
        return Err(DaemonError::fs(
            &root,
            std::io::Error::other(format!(
                "media upload needs {} bytes but only {available} bytes are available",
                manifest.total_size
            )),
        ));
    }
    let final_path = unique_final_path(files, &root, stamp, &manifest.top_name)?;
    let part_path = part_path(&final_path);
    let prepared = match manifest.kind {
        ManifestKind::File { size } => {
            files.create_part_file(&part_path, size)?;
            PreparedUpload {
                root,
                final_path,
                part_path: part_path.clone(),
                files: vec![upload_file(manifest.top_name, part_path, size)],
            }
        }
        ManifestKind::Directory {
            files: manifest_files,
            dirs,
        } => {
            if let Err(error) = files.create_private_dir(&part_path) {
                cleanup_failed_prepare(files, &root, &part_path);
                return Err(error);
            }
            let result = (|| {
                for directory in dirs {
                    files.create_private_dir_all(&part_path.join(directory))?;
                }
                let mut upload_files = Vec::with_capacity(manifest_files.len());
                for file in manifest_files {
                    let path = part_path.join(&file.relative);
                    if let Some(parent) = path.parent() {
                        files.create_private_dir_all(parent)?;
                    }
                    files.create_part_file(&path, file.size)?;
                    upload_files.push(upload_file(file.label, path, file.size));
                }
                Ok(upload_files)
            })();
            match result {
                Ok(upload_files) => PreparedUpload {
                    root,
                    final_path,
                    part_path: part_path.clone(),
                    files: upload_files,
                },
                Err(error) => {
                    cleanup_failed_prepare(files, &root, &part_path);
                    return Err(error);
                }
            }
        }
    };
    Ok(prepared)
}

fn cleanup_failed_prepare(files: &dyn Files, root: &Path, part: &Path) {
    if let Err(error) = files.remove_part_tree(root, part) {
        tracing::warn!(
            path = %part.display(),
            %error,
            "could not clean up a failed media upload allocation"
        );
    }
}

fn upload_file(label: String, path: PathBuf, size: u64) -> UploadFile {
    let chunks = size.div_ceil(CHUNK_BYTES as u64);
    // Manifest validation caps an upload at 8,192 chunks, which fits Rust's minimum usize width.
    let chunk_count = chunks as usize;
    UploadFile {
        label,
        path,
        size,
        received: vec![false; chunk_count],
    }
}

fn unique_final_path(
    files: &dyn Files,
    root: &Path,
    stamp: &str,
    name: &str,
) -> DaemonResult<PathBuf> {
    const MAX_COLLISION_ATTEMPTS: u64 = 10_000;
    let base = format!("{stamp}-{name}");
    for suffix in 1_u64..=MAX_COLLISION_ATTEMPTS {
        let candidate = if suffix == 1 {
            root.join(&base)
        } else {
            root.join(with_numeric_suffix(&base, suffix))
        };
        if !files.exists(&candidate) && !files.exists(&part_path(&candidate)) {
            return Ok(candidate);
        }
    }
    Err(DaemonError::Validation(format!(
        "could not allocate a unique media name for {name:?} after {MAX_COLLISION_ATTEMPTS} attempts"
    )))
}

fn with_numeric_suffix(name: &str, suffix: u64) -> String {
    let path = Path::new(name);
    match (
        path.file_stem().and_then(|stem| stem.to_str()),
        path.extension().and_then(|extension| extension.to_str()),
    ) {
        (Some(stem), Some(extension)) => format!("{stem}-{suffix}.{extension}"),
        _ => format!("{name}-{suffix}"),
    }
}

fn part_path(final_path: &Path) -> PathBuf {
    let mut name = final_path.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

fn selected_downloads_root(home: &FleetHome) -> PathBuf {
    selected_downloads_root_from(home, std::env::var_os("HOME").map(PathBuf::from))
}

fn selected_downloads_root_from(home: &FleetHome, os_home: Option<PathBuf>) -> PathBuf {
    os_home
        .map(|home| home.join("Downloads"))
        .filter(|downloads| downloads.is_dir() && directory_is_writable(downloads))
        .and_then(|downloads| std::fs::canonicalize(downloads).ok())
        .map(|downloads| downloads.join("fleet"))
        .unwrap_or_else(|| canonical_fleet_root(home).join("media"))
}

fn canonical_fleet_root(home: &FleetHome) -> PathBuf {
    std::fs::canonicalize(home.root()).unwrap_or_else(|_| home.root().to_path_buf())
}

fn directory_is_writable(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated pathname for this read-only access check.
    unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

mod attachments;
pub(crate) use attachments::decoded_size;
mod sweep;
#[cfg(test)]
mod tests;

#[cfg(test)]
fn hex_for_test(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}
