//! No-follow, fd-relative filesystem primitives used by RealFiles.

use std::{
    ffi::{CStr, CString, OsStr},
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
    path::{Component, Path},
};

use super::{FileIdentity, FileKind, FileMetadata, absolute_lexical, unsafe_removal_error};
use crate::{DaemonError, DaemonResult};

pub(super) fn path_component(component: &OsStr, path: &Path) -> DaemonResult<CString> {
    CString::new(component.as_bytes())
        .map_err(|_| DaemonError::Validation(format!("path contains NUL: {}", path.display())))
}

pub(super) fn metadata_at(parent: &File, name: &CStr) -> std::io::Result<FileMetadata> {
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
        len: u64::try_from(stat.st_size).unwrap_or_default(),
        modified_millis: modified_millis(&stat),
        identity: FileIdentity {
            // `st_dev` is `u64` on Linux and `i32` on macOS; the cast is load-bearing there.
            #[allow(clippy::unnecessary_cast)]
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        },
    })
}

/// The `libc` crate flattens `st_mtimespec` into `st_mtime` / `st_mtime_nsec` on every
/// platform we build for, including macOS.
fn modified_millis(stat: &libc::stat) -> i64 {
    stat.st_mtime
        .saturating_mul(1_000)
        .saturating_add(stat.st_mtime_nsec / 1_000_000)
}

pub(super) fn unlink_file_at(parent: &File, name: &CStr, path: &Path) -> DaemonResult<()> {
    // SAFETY: `parent` and the NUL-terminated name remain valid for this call.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(DaemonError::fs(path, std::io::Error::last_os_error()))
    }
}

pub(super) fn create_private_directories(path: &Path, exclusive_leaf: bool) -> DaemonResult<()> {
    let path = absolute_lexical(path);
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::RootDir => None,
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        return Err(DaemonError::Validation(format!(
            "refusing to create private root directory {}",
            path.display()
        )));
    }

    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .map_err(|error| DaemonError::fs(&path, error))?;
    for (index, name) in components.iter().enumerate() {
        let leaf = index + 1 == components.len();
        match open_directory_at(&directory, name, &path) {
            Ok(_next) if exclusive_leaf && leaf => {
                return Err(DaemonError::fs(
                    &path,
                    std::io::Error::from(std::io::ErrorKind::AlreadyExists),
                ));
            }
            Ok(next) => directory = next,
            Err(DaemonError::Filesystem { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound && (!exclusive_leaf || leaf) =>
            {
                mkdir_at(&directory, name, &path)?;
                directory = open_directory_at(&directory, name, &path)?;
            }
            Err(error) => return Err(error),
        }
    }
    // SAFETY: `directory` is the no-follow-opened leaf descriptor.
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
        return Err(DaemonError::fs(&path, std::io::Error::last_os_error()));
    }
    Ok(())
}

pub(super) fn mkdir_at(parent: &File, name: &OsStr, path: &Path) -> DaemonResult<()> {
    let name = path_component(name, path)?;
    // SAFETY: `parent` remains open and `name` is NUL-terminated for this call.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } == 0 {
        Ok(())
    } else {
        Err(DaemonError::fs(path, std::io::Error::last_os_error()))
    }
}

pub(super) fn open_path_parent(path: &Path) -> DaemonResult<(File, CString)> {
    let path = absolute_lexical(path);
    let parent = path.parent().ok_or_else(|| {
        DaemonError::Validation(format!("path has no parent: {}", path.display()))
    })?;
    let name = path.file_name().ok_or_else(|| {
        DaemonError::Validation(format!("path has no file name: {}", path.display()))
    })?;
    Ok((open_directory(parent)?, path_component(name, &path)?))
}

pub(super) fn open_file_at(
    parent: &File,
    name: &CStr,
    flags: libc::c_int,
    mode: libc::mode_t,
    path: &Path,
) -> DaemonResult<File> {
    // `mode_t` is `u16` on macOS and cannot be passed to a variadic call as is.
    #[allow(clippy::unnecessary_cast)]
    let mode = mode as libc::c_uint;
    // SAFETY: `parent` remains open and `name` is NUL-terminated for this call.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(DaemonError::fs(path, std::io::Error::last_os_error()));
    }
    // SAFETY: successful `openat` returned a newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn open_directory(path: &Path) -> DaemonResult<File> {
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

pub(super) fn open_directory_at(parent: &File, name: &OsStr, path: &Path) -> DaemonResult<File> {
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

pub(super) fn rename_at(
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

pub(super) fn remove_entry_at(parent: &File, name: &CStr, path: &Path) -> DaemonResult<()> {
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

pub(super) fn sync_directory(path: &Path) -> DaemonResult<()> {
    let directory = File::open(path).map_err(|error| DaemonError::fs(path, error))?;
    directory
        .sync_all()
        .map_err(|error| DaemonError::fs(path, error))?;
    #[cfg(test)]
    super::PARENT_SYNCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}
