//! Filesystem copying, atomic replacement, and trash operations.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use uuid::Uuid;

use crate::{DaemonError, DaemonResult};

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
    /// Returns whether a path exists.
    fn exists(&self, path: &Path) -> bool;
    /// Lists direct directory children in deterministic path order.
    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>>;
    /// Rejects roots, siblings, and paths escaping every configured safe root.
    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()>;
}

/// Real filesystem adapter with explicit deletion roots.
#[derive(Debug, Clone)]
pub struct RealFiles {
    trash_dir: PathBuf,
    removable_roots: Vec<PathBuf>,
}

impl RealFiles {
    /// Creates a filesystem adapter allowing removal only below the supplied roots.
    #[must_use]
    pub fn new(
        trash_dir: impl Into<PathBuf>,
        removable_roots: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        let trash_dir = absolute_lexical(&trash_dir.into());
        let mut roots = removable_roots
            .into_iter()
            .map(|root| absolute_lexical(&root))
            .collect::<Vec<_>>();
        if !roots.contains(&trash_dir) {
            roots.push(trash_dir.clone());
        }
        Self {
            trash_dir,
            removable_roots: roots,
        }
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
            fs::rename(&temporary, path).map_err(|error| DaemonError::fs(path, error))
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
        fs::create_dir_all(&self.trash_dir)
            .map_err(|error| DaemonError::fs(&self.trash_dir, error))?;
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("item");
        let destination = self
            .trash_dir
            .join(format!("{epoch}-{name}-{}", Uuid::new_v4()));
        fs::rename(path, &destination).map_err(|error| DaemonError::fs(path, error))?;
        Ok(destination)
    }

    fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
        self.guard_strict_descendant(path)?;
        let path = path.to_path_buf();
        let (started, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("fleet-remove".to_owned())
            .spawn(move || {
                match Command::new("rm").arg("-rf").arg(&path).spawn() {
                    Ok(mut child) => {
                        let _ = started.send(Ok(()));
                        match child.wait() {
                            Ok(status) if status.success() => {}
                            Ok(status) => tracing::warn!(%status, path = %path.display(), "detached removal failed"),
                            Err(error) => tracing::warn!(%error, path = %path.display(), "failed to reap detached removal"),
                        }
                    }
                    Err(error) => { let _ = started.send(Err(DaemonError::fs(path, error))); }
                }
            })
            .map_err(|error| DaemonError::Join(format!("removal thread: {error}")))?;
        receiver
            .recv()
            .map_err(|error| DaemonError::Join(format!("removal startup: {error}")))?
    }

    fn remove_file(&self, path: &Path) -> DaemonResult<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DaemonError::fs(path, error)),
        }
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
        let path = absolute_lexical(path);
        if self
            .removable_roots
            .iter()
            .any(|root| path != *root && path.starts_with(root))
        {
            Ok(())
        } else {
            Err(DaemonError::Validation(format!(
                "refusing to remove non-descendant path {}",
                path.display()
            )))
        }
    }
}

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
    normalized
}

#[cfg(test)]
mod tests {
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
    fn descendant_guard_rejects_roots_and_escapes() {
        let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temp.path().join("repos");
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
}
