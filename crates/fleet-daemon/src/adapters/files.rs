//! Filesystem copying, atomic replacement, and trash operations.

use std::{
    ffi::{CStr, CString, OsStr},
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use uuid::Uuid;

use crate::{DaemonError, DaemonResult};

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
    identity: FileIdentity,
}

impl FileMetadata {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn fake(kind: FileKind, identity: u64) -> Self {
        Self {
            kind,
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
    /// Creates a directory and all missing ancestors.
    fn create_dir_all(&self, path: &Path) -> DaemonResult<()>;
    /// Copies a directory using the platform's copy-on-write strategy when available.
    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()>;
    /// Atomically replaces text using an exclusive same-directory temporary file and rename.
    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()>;
    /// Renames a path without crossing filesystems.
    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()>;
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

    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        fs::rename(source, destination).map_err(|error| DaemonError::fs(source, error))
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

fn path_component(component: &OsStr, path: &Path) -> DaemonResult<CString> {
    CString::new(component.as_bytes())
        .map_err(|_| DaemonError::Validation(format!("path contains NUL: {}", path.display())))
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
        identity: FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    }
}

fn metadata_at(parent: &File, name: &CStr) -> std::io::Result<FileMetadata> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `parent` and the NUL-terminated name remain valid, and `stat` is writable.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful `fstatat` initialized the complete structure.
    let stat = unsafe { stat.assume_init() };
    let kind = match stat.st_mode & libc::S_IFMT {
        libc::S_IFREG => FileKind::File,
        libc::S_IFDIR => FileKind::Directory,
        _ => FileKind::Other,
    };
    Ok(FileMetadata {
        kind,
        identity: FileIdentity {
            // `st_dev` is `u64` on Linux and `i32` on macOS; the cast is load-bearing there.
            #[allow(clippy::unnecessary_cast)]
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        },
    })
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

fn unlink_file_at(parent: &File, name: &CStr, path: &Path) -> DaemonResult<()> {
    // SAFETY: `parent` and the NUL-terminated name remain valid for this call.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(DaemonError::fs(path, std::io::Error::last_os_error()))
    }
}

fn open_directory(path: &Path) -> DaemonResult<File> {
    let path = absolute_lexical(path);
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .map_err(|error| DaemonError::fs(&path, error))?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = open_directory_at(&directory, name, &path)?;
            }
            _ => return Err(unsafe_removal_error(&path)),
        }
    }
    Ok(directory)
}

fn open_directory_at(parent: &File, name: &OsStr, path: &Path) -> DaemonResult<File> {
    let name = path_component(name, path)?;
    // SAFETY: `parent` is open for the duration of the call and `name` is NUL-terminated.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(DaemonError::fs(path, std::io::Error::last_os_error()));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn rename_at(
    source_parent: &File,
    source_name: &CStr,
    destination_parent: &File,
    destination_name: &CStr,
    path: &Path,
) -> DaemonResult<()> {
    // SAFETY: both directory descriptors and both NUL-terminated names remain valid for the call.
    let result = unsafe {
        libc::renameat(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(DaemonError::fs(path, std::io::Error::last_os_error()))
    }
}

fn remove_entry_at(parent: &File, name: &CStr, path: &Path) -> DaemonResult<()> {
    // SAFETY: `parent` and the NUL-terminated entry name remain valid for the call.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Ok(());
    }
    if !matches!(error.raw_os_error(), Some(libc::EISDIR) | Some(libc::EPERM)) {
        return Err(DaemonError::fs(path, error));
    }

    let directory = open_directory_at(parent, OsStr::from_bytes(name.to_bytes()), path)?;
    for child_name in directory_entry_names(&directory, path)? {
        let child_name = OsStr::from_bytes(child_name.to_bytes());
        let child_path = path.join(child_name);
        let child_name = path_component(child_name, &child_path)?;
        remove_entry_at(&directory, &child_name, &child_path)?;
    }

    // SAFETY: `parent` and the NUL-terminated entry name remain valid for the call.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } == 0 {
        Ok(())
    } else {
        Err(DaemonError::fs(path, std::io::Error::last_os_error()))
    }
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: `self.0` is owned by this guard and remains valid until this call.
        unsafe { libc::closedir(self.0) };
    }
}

fn directory_entry_names(directory: &File, path: &Path) -> DaemonResult<Vec<CString>> {
    // SAFETY: `directory` owns a valid descriptor. `dup` creates an independently owned copy.
    let descriptor = unsafe { libc::dup(directory.as_raw_fd()) };
    if descriptor < 0 {
        return Err(DaemonError::fs(path, std::io::Error::last_os_error()));
    }
    // SAFETY: `descriptor` is an owned directory descriptor transferred to `fdopendir`.
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: ownership was not transferred when `fdopendir` failed.
        unsafe { libc::close(descriptor) };
        return Err(DaemonError::fs(path, error));
    }
    let stream = DirectoryStream(stream);
    let mut names = Vec::new();
    loop {
        set_errno(0);
        // SAFETY: the stream is valid and exclusively consumed by this loop.
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = errno();
            if error == 0 {
                break;
            }
            return Err(DaemonError::fs(
                path,
                std::io::Error::from_raw_os_error(error),
            ));
        }
        // SAFETY: `readdir` returned a valid entry whose `d_name` is NUL-terminated.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(name.to_owned());
        }
    }
    names.sort_by(|left, right| left.to_bytes().cmp(right.to_bytes()));
    Ok(names)
}

#[cfg(target_os = "macos")]
fn errno_pointer() -> *mut libc::c_int {
    // SAFETY: Darwin exposes the calling thread's errno storage through `__error`.
    unsafe { libc::__error() }
}

#[cfg(target_os = "linux")]
fn errno_pointer() -> *mut libc::c_int {
    // SAFETY: glibc exposes the calling thread's errno storage through `__errno_location`.
    unsafe { libc::__errno_location() }
}

fn errno() -> libc::c_int {
    // SAFETY: `errno_pointer` returns valid thread-local storage.
    unsafe { *errno_pointer() }
}

fn set_errno(value: libc::c_int) {
    // SAFETY: `errno_pointer` returns valid thread-local storage.
    unsafe { *errno_pointer() = value };
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

fn sync_directory(path: &Path) -> DaemonResult<()> {
    let directory = File::open(path).map_err(|error| DaemonError::fs(path, error))?;
    directory
        .sync_all()
        .map_err(|error| DaemonError::fs(path, error))?;
    #[cfg(test)]
    PARENT_SYNCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(())
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
mod tests {
    use std::sync::atomic::Ordering;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn atomic_write_replaces_complete_text_and_leaves_no_temp() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("data")]);
        let path = temp.path().join("data/config.json");
        files
            .atomic_write_text(&path, "first")
            .unwrap_or_else(|error| panic!("{error}"));
        files
            .atomic_write_text(&path, "second")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(fs::read_to_string(&path).ok().as_deref(), Some("second"));
        let children = files
            .list(path.parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(children, vec![path]);
    }

    #[test]
    fn atomic_replace_syncs_parent_directory() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("data")]);
        let path = temp.path().join("data/config.json");
        let before = PARENT_SYNCS.load(Ordering::Relaxed);
        files
            .atomic_write_text(&path, "durable")
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(PARENT_SYNCS.load(Ordering::Relaxed) > before);
    }

    #[test]
    fn conditional_remove_preserves_replacement() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
        let path = temp.path().join("cache/pr.json");
        files
            .atomic_write_text(&path, "stale")
            .unwrap_or_else(|error| panic!("{error}"));
        let inspected = files
            .metadata(&path)
            .unwrap_or_else(|error| panic!("{error}"));
        files
            .atomic_write_text(&path, "refreshed")
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(
            !files
                .remove_file_if_unchanged(&path, inspected)
                .unwrap_or_else(|error| panic!("{error}"))
        );
        assert_eq!(
            files
                .read_text(&path)
                .unwrap_or_else(|error| panic!("{error}")),
            "refreshed"
        );
    }

    #[test]
    fn conditional_remove_restores_replacement_after_post_check_race() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
        let path = temp.path().join("cache/pr.json");
        files
            .atomic_write_text(&path, "stale")
            .unwrap_or_else(|error| panic!("{error}"));
        let inspected = files
            .metadata(&path)
            .unwrap_or_else(|error| panic!("{error}"));
        let racing_files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
        files.set_conditional_removal_hook(move |stage, path| {
            if stage == ConditionalRemovalStage::BeforeRename {
                racing_files
                    .atomic_write_text(path, "refreshed")
                    .unwrap_or_else(|error| panic!("{error}"));
            }
        });

        assert!(
            !files
                .remove_file_if_unchanged(&path, inspected)
                .unwrap_or_else(|error| panic!("{error}"))
        );
        assert_eq!(
            files
                .read_text(&path)
                .unwrap_or_else(|error| panic!("{error}")),
            "refreshed"
        );
        assert_eq!(
            files
                .list(path.parent().unwrap_or(temp.path()))
                .unwrap_or_else(|error| panic!("{error}")),
            vec![path]
        );
    }

    #[test]
    fn conditional_remove_preserves_recovery_when_restore_destination_exists() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
        let path = temp.path().join("cache/pr.json");
        files
            .atomic_write_text(&path, "stale")
            .unwrap_or_else(|error| panic!("{error}"));
        let inspected = files
            .metadata(&path)
            .unwrap_or_else(|error| panic!("{error}"));
        let racing_files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
        files.set_conditional_removal_hook(move |stage, path| {
            let text = match stage {
                ConditionalRemovalStage::BeforeRename => "refreshed",
                ConditionalRemovalStage::AfterRename => "newest",
            };
            racing_files
                .atomic_write_text(path, text)
                .unwrap_or_else(|error| panic!("{error}"));
        });

        assert!(
            !files
                .remove_file_if_unchanged(&path, inspected)
                .unwrap_or_else(|error| panic!("{error}"))
        );
        assert_eq!(
            files
                .read_text(&path)
                .unwrap_or_else(|error| panic!("{error}")),
            "newest"
        );
        let recovery = files
            .list(path.parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"))
            .into_iter()
            .find(|candidate| {
                candidate.file_name().is_some_and(|name| {
                    name.as_bytes()
                        .windows(14)
                        .any(|part| part == b".fleet-expiry-")
                })
            })
            .unwrap_or_else(|| panic!("recovery file"));
        assert_eq!(
            files
                .read_text(&recovery)
                .unwrap_or_else(|error| panic!("{error}")),
            "refreshed"
        );
    }

    #[test]
    fn descendant_guard_rejects_roots_and_escapes() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path().join("repos");
        fs::create_dir_all(root.join("owner")).unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [root.clone()]);
        assert!(
            files
                .guard_strict_descendant(&root.join("owner/repo"))
                .is_ok()
        );
        assert!(files.guard_strict_descendant(&root).is_err());
        assert!(
            files
                .guard_strict_descendant(&root.join("../outside"))
                .is_err()
        );
    }

    #[test]
    fn symlink_swap_cannot_escape_removable_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path().join("repos");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(&outside).unwrap_or_else(|error| panic!("{error}"));
        fs::write(outside.join("keep"), "untouched").unwrap_or_else(|error| panic!("{error}"));
        symlink(&outside, root.join("swapped")).unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [root.clone()]);

        assert!(files.trash(&root.join("swapped/keep")).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep")).ok().as_deref(),
            Some("untouched")
        );
    }

    #[test]
    fn remove_detached_removes_nested_directory_and_reports_completion() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path().join("repos");
        let trash = temp.path().join("trash");
        let target = root.join("owner/repo");
        fs::create_dir_all(target.join("nested/empty")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(target.join("nested/file"), "contents").unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(&trash, [root]);

        files
            .remove_detached(&target)
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(!target.exists());
        assert!(
            fs::read_dir(&trash)
                .unwrap_or_else(|error| panic!("{error}"))
                .next()
                .is_none()
        );
    }

    #[test]
    fn symlinked_ancestor_above_root_allows_confined_removal() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let real = temp.path().join("real");
        let link = temp.path().join("link");
        let root = link.join("repos");
        let target = root.join("owner/repo");
        fs::create_dir_all(real.join("repos/owner/repo/nested"))
            .unwrap_or_else(|error| panic!("{error}"));
        fs::write(real.join("repos/owner/repo/nested/file"), "contents")
            .unwrap_or_else(|error| panic!("{error}"));
        symlink(&real, &link).unwrap_or_else(|error| panic!("{error}"));
        let trash = temp.path().join("trash");
        let files = RealFiles::new(&trash, [root]);

        files
            .remove_detached(&target)
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(!real.join("repos/owner/repo").exists());
        assert!(
            fs::read_dir(&trash)
                .unwrap_or_else(|error| panic!("{error}"))
                .next()
                .is_none()
        );
    }

    #[test]
    fn removable_roots_can_be_refreshed() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let old_root = temp.path().join("old");
        let new_root = temp.path().join("new");
        fs::create_dir_all(old_root.join("owner")).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(new_root.join("owner")).unwrap_or_else(|error| panic!("{error}"));
        let files = RealFiles::new(temp.path().join("trash"), [old_root.clone()]);

        assert!(
            files
                .guard_strict_descendant(&old_root.join("owner/repo"))
                .is_ok()
        );
        assert!(
            files
                .guard_strict_descendant(&new_root.join("owner/repo"))
                .is_err()
        );

        files.set_removable_roots(vec![new_root.clone()]);

        assert!(
            files
                .guard_strict_descendant(&old_root.join("owner/repo"))
                .is_err()
        );
        assert!(
            files
                .guard_strict_descendant(&new_root.join("owner/repo"))
                .is_ok()
        );
    }
}
