//! Filesystem copying, atomic replacement, and trash operations.

use std::{
    ffi::{CStr, CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::ffi::OsStrExt,
        unix::fs::{FileExt, MetadataExt},
    },
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{DaemonError, DaemonResult};

use self::fd::{
    create_private_directories, metadata_at, open_directory, open_directory_at, open_file_at,
    open_path_parent, path_component, remove_entry_at, rename_at, sync_directory, unlink_file_at,
};

mod fd;

/// Filesystem entry type observed together with its stable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symlink, socket, or another non-cache entry.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

/// Entry metadata used to condition a later removal on the same file still being present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMetadata {
    /// The observed entry type without following symlinks.
    pub kind: FileKind,
    /// Length in bytes for a regular file, zero for other entry kinds.
    pub len: u64,
    /// Last modification time as Unix epoch milliseconds.
    pub modified_millis: i64,
    identity: FileIdentity,
}

impl FileMetadata {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn fake(kind: FileKind, identity: u64, len: u64, modified_millis: i64) -> Self {
        Self {
            kind,
            len,
            modified_millis,
            identity: FileIdentity {
                device: 0,
                inode: identity,
            },
        }
    }
}

/// Observable revision of a file: it changes when the bytes may have changed, whether the file
/// was replaced by a rename or rewritten in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileRevision {
    identity: FileIdentity,
    len: u64,
    modified: (i64, i64),
}

impl FileRevision {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn fake(identity: u64, len: u64) -> Self {
        Self {
            identity: FileIdentity {
                device: 0,
                inode: identity,
            },
            len,
            modified: (0, 0),
        }
    }
}

/// Filesystem operations used by stores and domain services.
pub trait Files: Send + Sync {
    /// Reads an entire UTF-8 text file.
    fn read_text(&self, path: &Path) -> DaemonResult<String>;
    /// Resolves every symlink in a path.
    fn canonicalize(&self, path: &Path) -> DaemonResult<PathBuf> {
        Err(DaemonError::Validation(format!(
            "path canonicalization is unsupported for {}",
            path.display()
        )))
    }
    /// Reads one regular file without following its final component, capped before allocation.
    fn read_bytes_bounded(&self, path: &Path, maximum: u64) -> DaemonResult<Vec<u8>> {
        Err(DaemonError::Validation(format!(
            "bounded file reads are unsupported for {} (maximum {maximum} bytes)",
            path.display()
        )))
    }
    /// Creates a directory and all missing ancestors.
    fn create_dir_all(&self, path: &Path) -> DaemonResult<()>;
    /// Copies a directory using the platform's copy-on-write strategy when available.
    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()>;
    /// Atomically replaces text using an exclusive same-directory temporary file and rename.
    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()>;
    /// Creates a private directory and all missing ancestors, forcing the leaf to mode `0700`.
    fn create_private_dir_all(&self, path: &Path) -> DaemonResult<()> {
        Err(DaemonError::Validation(format!(
            "private directories are unsupported for {}",
            path.display()
        )))
    }
    /// Creates one private leaf directory exclusively below an existing parent.
    fn create_private_dir(&self, path: &Path) -> DaemonResult<()> {
        Err(DaemonError::Validation(format!(
            "private directory allocation is unsupported for {}",
            path.display()
        )))
    }
    /// Creates an exclusive sparse upload part file with its final length and mode `0600`.
    fn create_part_file(&self, path: &Path, size: u64) -> DaemonResult<()> {
        Err(DaemonError::Validation(format!(
            "part files are unsupported for {} ({size} bytes)",
            path.display()
        )))
    }
    /// Writes one upload chunk at its exact file offset.
    fn write_part(&self, path: &Path, offset: u64, bytes: &[u8]) -> DaemonResult<()> {
        Err(DaemonError::Validation(format!(
            "part writes are unsupported for {} at offset {offset} ({} bytes)",
            path.display(),
            bytes.len()
        )))
    }
    /// Computes the lowercase SHA-256 digest of one file without loading it all into memory.
    fn sha256(&self, path: &Path) -> DaemonResult<String> {
        Err(DaemonError::Validation(format!(
            "file hashing is unsupported for {}",
            path.display()
        )))
    }
    /// Removes one direct child part file or tree without following symlinks.
    fn remove_part_tree(&self, root: &Path, path: &Path) -> DaemonResult<()> {
        Err(DaemonError::Validation(format!(
            "part removal is unsupported for {} below {}",
            path.display(),
            root.display()
        )))
    }
    /// Returns currently available bytes for a directory when the adapter can query them cheaply.
    fn available_bytes(&self, _path: &Path) -> DaemonResult<Option<u64>> {
        Ok(None)
    }
    /// Renames a path without crossing filesystems.
    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()>;
    /// Publishes one direct child part below `root` without following directory symlinks.
    fn rename_part(&self, _root: &Path, source: &Path, destination: &Path) -> DaemonResult<()> {
        self.rename(source, destination)
    }
    /// Moves a path below Fleet's trash directory and returns the new path.
    fn trash(&self, path: &Path) -> DaemonResult<PathBuf>;
    /// Removes a validated descendant on a detached thread.
    fn remove_detached(&self, path: &Path) -> DaemonResult<()>;
    /// Removes one file when it exists.
    fn remove_file(&self, path: &Path) -> DaemonResult<()>;
    /// Returns a value that changes whenever a file's content may have changed.
    ///
    /// Callers cache a parse against it; an adapter that cannot observe revisions returns an
    /// error and its callers simply reload every time.
    fn revision(&self, path: &Path) -> DaemonResult<FileRevision> {
        Err(DaemonError::Validation(format!(
            "file revisions are unsupported for {}",
            path.display()
        )))
    }
    /// Inspects one entry without following symlinks.
    fn metadata(&self, path: &Path) -> DaemonResult<FileMetadata> {
        Err(DaemonError::Validation(format!(
            "file metadata is unsupported for {}",
            path.display()
        )))
    }
    /// Removes a regular file only if it still has the supplied identity.
    fn remove_file_if_unchanged(&self, path: &Path, _expected: FileMetadata) -> DaemonResult<bool> {
        Err(DaemonError::Validation(format!(
            "conditional file removal is unsupported for {}",
            path.display()
        )))
    }
    /// Returns whether a path exists.
    fn exists(&self, path: &Path) -> bool;
    /// Lists direct directory children in deterministic path order.
    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>>;
    /// Rejects roots, siblings, and paths escaping every configured safe root.
    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()>;
    /// Replaces the configured deletion roots after a successful configuration update.
    fn set_removable_roots(&self, roots: Vec<PathBuf>);
}

/// Real filesystem adapter with explicit deletion roots.
#[derive(Debug, Clone)]
pub struct RealFiles {
    trash_dir: PathBuf,
    removable_roots: Arc<RwLock<Vec<RemovableRoot>>>,
    #[cfg(test)]
    conditional_removal_hook: Arc<RwLock<Option<ConditionalRemovalHook>>>,
}

#[cfg(test)]
type ConditionalRemovalCallback = dyn Fn(ConditionalRemovalStage, &Path) + Send + Sync + 'static;

#[cfg(test)]
#[derive(Clone)]
struct ConditionalRemovalHook(Arc<ConditionalRemovalCallback>);

#[cfg(test)]
impl std::fmt::Debug for ConditionalRemovalHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConditionalRemovalHook(..)")
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConditionalRemovalStage {
    BeforeRename,
    AfterRename,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemovableRoot {
    requested: PathBuf,
    resolved: PathBuf,
}

impl RemovableRoot {
    fn new(path: PathBuf) -> Self {
        let requested = absolute_lexical(&path);
        let resolved = resolve_root(&requested);
        Self {
            requested,
            resolved,
        }
    }

    fn relative_to(&self, path: &Path) -> Option<PathBuf> {
        path.strip_prefix(&self.requested)
            .or_else(|_| path.strip_prefix(&self.resolved))
            .ok()
            .map(Path::to_path_buf)
    }
}

struct QuarantinedPath {
    path: PathBuf,
    parent: File,
    name: CString,
}

impl RealFiles {
    /// Creates a filesystem adapter allowing removal only below the supplied roots.
    #[must_use]
    pub fn new(
        trash_dir: impl Into<PathBuf>,
        removable_roots: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        let trash_root = RemovableRoot::new(trash_dir.into());
        let trash_dir = trash_root.resolved.clone();
        let mut roots = removable_roots
            .into_iter()
            .map(RemovableRoot::new)
            .collect::<Vec<_>>();
        if !roots.contains(&trash_root) {
            roots.push(trash_root);
        }
        Self {
            trash_dir,
            removable_roots: Arc::new(RwLock::new(roots)),
            #[cfg(test)]
            conditional_removal_hook: Arc::new(RwLock::new(None)),
        }
    }

    #[cfg(test)]
    fn set_conditional_removal_hook(
        &self,
        hook: impl Fn(ConditionalRemovalStage, &Path) + Send + Sync + 'static,
    ) {
        *self
            .conditional_removal_hook
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(ConditionalRemovalHook(Arc::new(hook)));
    }

    #[cfg(test)]
    fn run_conditional_removal_hook(&self, stage: ConditionalRemovalStage, path: &Path) {
        let hook = self
            .conditional_removal_hook
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            (hook.0)(stage, path);
        }
    }

    fn confined_parent(&self, path: &Path) -> DaemonResult<(File, CString)> {
        let path = absolute_lexical(path);
        let (root, relative) = self
            .removable_roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|root| {
                root.relative_to(&path)
                    .map(|relative| (root.resolved.clone(), relative))
            })
            .filter(|(_, relative)| relative.components().count() > 0)
            .max_by_key(|(root, _)| root.components().count())
            .ok_or_else(|| unsafe_removal_error(&path))?;
        let mut components = relative.components().peekable();
        let mut directory = open_directory(&root)?;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(unsafe_removal_error(&path));
            };
            if components.peek().is_none() {
                return Ok((directory, path_component(name, &path)?));
            }
            directory = open_directory_at(&directory, name, &path)?;
        }
        Err(unsafe_removal_error(&path))
    }

    fn quarantine(&self, path: &Path) -> DaemonResult<QuarantinedPath> {
        let path = absolute_lexical(path);
        let (source_parent, source_name) = self.confined_parent(&path)?;
        fs::create_dir_all(&self.trash_dir)
            .map_err(|error| DaemonError::fs(&self.trash_dir, error))?;
        let trash = open_directory(&self.trash_dir)?;
        let destination_name = trash_name(&path);
        rename_at(
            &source_parent,
            &source_name,
            &trash,
            destination_name.as_c_str(),
            &path,
        )?;
        let path = self
            .trash_dir
            .join(OsStr::from_bytes(destination_name.as_bytes()));
        Ok(QuarantinedPath {
            path,
            parent: trash,
            name: destination_name,
        })
    }
}

impl Files for RealFiles {
    fn read_text(&self, path: &Path) -> DaemonResult<String> {
        fs::read_to_string(path).map_err(|error| DaemonError::fs(path, error))
    }

    fn canonicalize(&self, path: &Path) -> DaemonResult<PathBuf> {
        fs::canonicalize(path)
            .map(|path| absolute_lexical(&path))
            .map_err(|error| DaemonError::fs(path, error))
    }

    fn read_bytes_bounded(&self, path: &Path, maximum: u64) -> DaemonResult<Vec<u8>> {
        let (parent, name) = open_path_parent(path)?;
        let mut file = open_file_at(
            &parent,
            &name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
            path,
        )?;
        let capacity = usize::try_from(maximum.min(64 * 1024)).unwrap_or(64 * 1024);
        let mut bytes = Vec::with_capacity(capacity);
        std::io::Read::by_ref(&mut file)
            .take(maximum.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| DaemonError::fs(path, error))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
            return Err(DaemonError::Validation(format!(
                "file is larger than {maximum} bytes: {}",
                path.display()
            )));
        }
        Ok(bytes)
    }

    fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
        fs::create_dir_all(path).map_err(|error| DaemonError::fs(path, error))
    }

    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        if destination.exists() {
            return Err(DaemonError::Conflict(format!(
                "copy destination already exists: {}",
                destination.display()
            )));
        }
        #[cfg(target_os = "macos")]
        {
            use std::{ffi::CString, os::unix::ffi::OsStrExt};

            let source_c = CString::new(source.as_os_str().as_bytes())
                .map_err(|_| DaemonError::Validation("copy source contains NUL".to_owned()))?;
            let destination_c = CString::new(destination.as_os_str().as_bytes())
                .map_err(|_| DaemonError::Validation("copy destination contains NUL".to_owned()))?;
            unsafe extern "C" {
                fn clonefile(
                    source: *const libc::c_char,
                    destination: *const libc::c_char,
                    flags: libc::c_int,
                ) -> libc::c_int;
            }
            // SAFETY: both pointers are valid NUL-terminated path strings for this call.
            if unsafe { clonefile(source_c.as_ptr(), destination_c.as_ptr(), 0) } == 0 {
                return Ok(());
            }
            if destination.exists() {
                fs::remove_dir_all(destination)
                    .map_err(|error| DaemonError::fs(destination, error))?;
            }
        }

        let mut command = Command::new("cp");
        #[cfg(target_os = "macos")]
        command.arg("-Rc");
        #[cfg(target_os = "linux")]
        command.args(["-R", "--reflink=auto"]);
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        command.arg("-R");
        let output = command
            .arg(source)
            .arg(destination)
            .output()
            .map_err(|error| DaemonError::fs(destination, error))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(DaemonError::Filesystem {
                path: destination.to_path_buf(),
                source: std::io::Error::other(
                    String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                ),
            })
        }
    }

    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
        let parent = path.parent().ok_or_else(|| {
            DaemonError::Validation(format!("path has no parent: {}", path.display()))
        })?;
        fs::create_dir_all(parent).map_err(|error| DaemonError::fs(parent, error))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file");
        let temporary = parent.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| DaemonError::fs(&temporary, error))?;
            file.write_all(text.as_bytes())
                .map_err(|error| DaemonError::fs(&temporary, error))?;
            file.sync_all()
                .map_err(|error| DaemonError::fs(&temporary, error))?;
            fs::rename(&temporary, path).map_err(|error| DaemonError::fs(path, error))?;
            sync_directory(parent)
        })();
        if result.is_err() {
            let _ignored = fs::remove_file(&temporary);
        }
        result
    }

    fn create_private_dir_all(&self, path: &Path) -> DaemonResult<()> {
        create_private_directories(path, false)
    }

    fn create_private_dir(&self, path: &Path) -> DaemonResult<()> {
        create_private_directories(path, true)
    }

    fn create_part_file(&self, path: &Path, size: u64) -> DaemonResult<()> {
        let (parent, name) = open_path_parent(path)?;
        let file = open_file_at(
            &parent,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
            path,
        )?;
        if let Err(error) = file.set_len(size) {
            match unlink_file_at(&parent, &name, path) {
                Ok(()) => {}
                Err(DaemonError::Filesystem { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(cleanup) => tracing::warn!(
                    path = %path.display(),
                    %cleanup,
                    "could not remove a part file after allocation failed"
                ),
            }
            return Err(DaemonError::fs(path, error));
        }
        Ok(())
    }

    fn write_part(&self, path: &Path, offset: u64, bytes: &[u8]) -> DaemonResult<()> {
        let (parent, name) = open_path_parent(path)?;
        let file = open_file_at(
            &parent,
            &name,
            libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
            path,
        )?;
        file.write_all_at(bytes, offset)
            .map_err(|error| DaemonError::fs(path, error))
    }

    fn sha256(&self, path: &Path) -> DaemonResult<String> {
        let (parent, name) = open_path_parent(path)?;
        let mut file = open_file_at(
            &parent,
            &name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
            path,
        )?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| DaemonError::fs(path, error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hex_lower(&hasher.finalize()))
    }

    fn remove_part_tree(&self, root: &Path, path: &Path) -> DaemonResult<()> {
        let root = absolute_lexical(root);
        let path = absolute_lexical(path);
        if path.parent() != Some(root.as_path()) {
            return Err(DaemonError::Validation(format!(
                "refusing to remove non-child part path {} below {}",
                path.display(),
                root.display()
            )));
        }
        if !path.exists() {
            return Ok(());
        }
        let parent = open_directory(&root)?;
        let name = path.file_name().ok_or_else(|| {
            DaemonError::Validation(format!("part path has no file name: {}", path.display()))
        })?;
        let name = path_component(name, &path)?;
        remove_entry_at(&parent, &name, &path)
    }

    fn available_bytes(&self, path: &Path) -> DaemonResult<Option<u64>> {
        let directory = open_directory(path)?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `directory` is open for the call and `stats` points to writable storage.
        if unsafe { libc::fstatvfs(directory.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
            return Err(DaemonError::fs(path, std::io::Error::last_os_error()));
        }
        // SAFETY: successful `statvfs` initialized the complete structure.
        let stats = unsafe { stats.assume_init() };
        Ok(Some(stats.f_bavail.saturating_mul(stats.f_frsize)))
    }

    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        fs::rename(source, destination).map_err(|error| DaemonError::fs(source, error))
    }

    fn rename_part(&self, root: &Path, source: &Path, destination: &Path) -> DaemonResult<()> {
        let root = absolute_lexical(root);
        let source = absolute_lexical(source);
        let destination = absolute_lexical(destination);
        if source.parent() != Some(root.as_path()) || destination.parent() != Some(root.as_path()) {
            return Err(DaemonError::Validation(format!(
                "refusing to publish media part {} as {} outside {}",
                source.display(),
                destination.display(),
                root.display()
            )));
        }
        let parent = open_directory(&root)?;
        let source_name = source.file_name().ok_or_else(|| {
            DaemonError::Validation(format!("media part has no file name: {}", source.display()))
        })?;
        let destination_name = destination.file_name().ok_or_else(|| {
            DaemonError::Validation(format!(
                "media destination has no file name: {}",
                destination.display()
            ))
        })?;
        let source_name = path_component(source_name, &source)?;
        let destination_name = path_component(destination_name, &destination)?;
        rename_at(&parent, &source_name, &parent, &destination_name, &source)
    }

    fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
        self.quarantine(path).map(|quarantined| quarantined.path)
    }

    fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
        let quarantined = self.quarantine(path)?;
        let (completed, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("fleet-remove".to_owned())
            .spawn(move || {
                let result =
                    remove_entry_at(&quarantined.parent, &quarantined.name, &quarantined.path);
                let _ignored = completed.send(result);
            })
            .map_err(|error| DaemonError::Join(format!("removal thread: {error}")))?;
        receiver
            .recv()
            .map_err(|error| DaemonError::Join(format!("removal completion: {error}")))?
    }

    fn remove_file(&self, path: &Path) -> DaemonResult<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DaemonError::fs(path, error)),
        }
    }

    fn revision(&self, path: &Path) -> DaemonResult<FileRevision> {
        let metadata = fs::metadata(path).map_err(|error| DaemonError::fs(path, error))?;
        Ok(FileRevision {
            identity: FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            len: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
        })
    }

    fn metadata(&self, path: &Path) -> DaemonResult<FileMetadata> {
        fs::symlink_metadata(path)
            .map(|metadata| file_metadata(&metadata))
            .map_err(|error| DaemonError::fs(path, error))
    }

    fn remove_file_if_unchanged(&self, path: &Path, expected: FileMetadata) -> DaemonResult<bool> {
        if expected.kind != FileKind::File {
            return Ok(false);
        }
        let path = absolute_lexical(path);
        let parent_path = path.parent().ok_or_else(|| {
            DaemonError::Validation(format!("path has no parent: {}", path.display()))
        })?;
        let name = path.file_name().ok_or_else(|| {
            DaemonError::Validation(format!("path has no file name: {}", path.display()))
        })?;
        let name = path_component(name, &path)?;
        let parent = open_directory(parent_path)?;
        match metadata_at(&parent, &name) {
            Ok(current) if current == expected => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(DaemonError::fs(&path, error)),
        }

        let quarantine_name = conditional_removal_name(&path);
        #[cfg(test)]
        self.run_conditional_removal_hook(ConditionalRemovalStage::BeforeRename, &path);
        if let Err(error) = rename_at(&parent, &name, &parent, &quarantine_name, &path) {
            if matches!(
                &error,
                DaemonError::Filesystem { source, .. }
                    if source.kind() == std::io::ErrorKind::NotFound
            ) {
                return Ok(false);
            }
            return Err(error);
        }
        #[cfg(test)]
        self.run_conditional_removal_hook(ConditionalRemovalStage::AfterRename, &path);

        let quarantine_path = parent_path.join(OsStr::from_bytes(quarantine_name.as_bytes()));
        let observed = metadata_at(&parent, &quarantine_name)
            .map_err(|error| DaemonError::fs(&quarantine_path, error))?;
        if observed != expected {
            restore_quarantined_file(&parent, &quarantine_name, &name, &path, &quarantine_path)?;
            return Ok(false);
        }
        unlink_file_at(&parent, &quarantine_name, &quarantine_path)?;
        Ok(true)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
        let mut entries = fs::read_dir(path)
            .map_err(|error| DaemonError::fs(path, error))?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|error| DaemonError::fs(path, error))
            })
            .collect::<DaemonResult<Vec<_>>>()?;
        entries.sort();
        Ok(entries)
    }

    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
        self.confined_parent(path).map(|_| ())
    }

    fn set_removable_roots(&self, roots: Vec<PathBuf>) {
        let mut roots = roots
            .into_iter()
            .map(RemovableRoot::new)
            .collect::<Vec<_>>();
        let trash_root = RemovableRoot::new(self.trash_dir.clone());
        if !roots.contains(&trash_root) {
            roots.push(trash_root);
        }
        *self
            .removable_roots
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = roots;
    }
}

fn unsafe_removal_error(path: &Path) -> DaemonError {
    DaemonError::Validation(format!(
        "refusing to remove non-descendant path {}",
        path.display()
    ))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn file_metadata(metadata: &fs::Metadata) -> FileMetadata {
    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        FileKind::File
    } else if file_type.is_dir() {
        FileKind::Directory
    } else {
        FileKind::Other
    };
    FileMetadata {
        kind,
        len: metadata.len(),
        modified_millis: metadata
            .mtime()
            .saturating_mul(1_000)
            .saturating_add(metadata.mtime_nsec() / 1_000_000),
        identity: FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    }
}

fn restore_quarantined_file(
    parent: &File,
    quarantine_name: &CStr,
    original_name: &CStr,
    original_path: &Path,
    quarantine_path: &Path,
) -> DaemonResult<()> {
    // SAFETY: both names are NUL-terminated and the directory descriptor remains valid.
    let result = unsafe {
        libc::linkat(
            parent.as_raw_fd(),
            quarantine_name.as_ptr(),
            parent.as_raw_fd(),
            original_name.as_ptr(),
            0,
        )
    };
    if result == 0 {
        return unlink_file_at(parent, quarantine_name, quarantine_path);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        tracing::warn!(
            path = %original_path.display(),
            recovery = %quarantine_path.display(),
            "cache entry changed during expiry; preserved raced entry"
        );
        Ok(())
    } else {
        Err(DaemonError::fs(quarantine_path, error))
    }
}

fn trash_name(path: &Path) -> CString {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("item");
    CString::new(format!("{epoch}-{name}-{}", Uuid::new_v4()))
        .unwrap_or_else(|_| unreachable!("generated trash names contain no NUL"))
}

fn conditional_removal_name(path: &Path) -> CString {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("cache");
    CString::new(format!(".{name}.fleet-expiry-{}.recovery", Uuid::new_v4()))
        .unwrap_or_else(|_| unreachable!("generated recovery names contain no NUL"))
}

#[cfg(test)]
static PARENT_SYNCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn absolute_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    platform_root_alias(normalized)
}

fn resolve_root(path: &Path) -> PathBuf {
    let mut ancestor = path;
    loop {
        if let Ok(resolved) = fs::canonicalize(ancestor) {
            let remainder = path
                .strip_prefix(ancestor)
                .unwrap_or_else(|_| Path::new(""));
            return absolute_lexical(&resolved.join(remainder));
        }
        let Some(parent) = ancestor.parent() else {
            return path.to_path_buf();
        };
        ancestor = parent;
    }
}

#[cfg(target_os = "macos")]
fn platform_root_alias(path: PathBuf) -> PathBuf {
    let alias = Path::new("/var");
    path.strip_prefix(alias)
        .map(|relative| Path::new("/private/var").join(relative))
        .unwrap_or(path)
}

#[cfg(not(target_os = "macos"))]
fn platform_root_alias(path: PathBuf) -> PathBuf {
    path
}

#[cfg(test)]
mod tests;
