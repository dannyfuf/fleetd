//! In-memory filesystem with atomic tree operations and injectable failures.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::lock;
use crate::{DaemonError, DaemonResult, adapters::files::Files};

/// A captured fake filesystem operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeFilesCall {
    /// Read of one text file.
    Read(PathBuf),
    /// Atomic write of one text file and its complete contents.
    Write(PathBuf, String),
    /// Recursive directory creation.
    CreateDir(PathBuf),
    /// Recursive copy from a source tree to a destination.
    Clone(PathBuf, PathBuf),
    /// Rename from a source path to a destination.
    Rename(PathBuf, PathBuf),
    /// Removal of one file or tree.
    Remove(PathBuf),
}

impl FakeFilesCall {
    fn path(&self) -> &Path {
        match self {
            Self::Read(path)
            | Self::Write(path, _)
            | Self::CreateDir(path)
            | Self::Remove(path)
            | Self::Clone(path, _)
            | Self::Rename(path, _) => path,
        }
    }
}

#[derive(Default)]
struct Tree {
    files: BTreeMap<PathBuf, String>,
    directories: BTreeSet<PathBuf>,
    calls: Vec<FakeFilesCall>,
    failures: VecDeque<(FakeFilesCall, io::ErrorKind)>,
}

impl Tree {
    fn record(&mut self, call: FakeFilesCall) -> DaemonResult<()> {
        let error = self
            .failures
            .iter()
            .position(|(expected, _)| expected == &call)
            .and_then(|index| self.failures.remove(index))
            .map(|(_, kind)| failure(call.path(), kind));
        self.calls.push(call);
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.directories.contains(path)
    }

    fn create_dirs(&mut self, path: &Path) -> DaemonResult<()> {
        if let Some(file) = path
            .ancestors()
            .find(|ancestor| self.files.contains_key(*ancestor))
        {
            return Err(failure(file, io::ErrorKind::NotADirectory));
        }
        self.directories
            .extend(path.ancestors().map(Path::to_path_buf));
        Ok(())
    }

    fn require_parent(&self, path: &Path) -> DaemonResult<()> {
        if let Some(parent) = path.parent() {
            if self.files.contains_key(parent) {
                return Err(failure(parent, io::ErrorKind::NotADirectory));
            }
            if !self.directories.contains(parent) {
                return Err(failure(parent, io::ErrorKind::NotFound));
            }
        }
        Ok(())
    }

    fn copy(&mut self, source: &Path, destination: &Path) {
        let files = self
            .files
            .iter()
            .filter_map(|(path, text)| {
                path.strip_prefix(source)
                    .ok()
                    .map(|suffix| (destination.join(suffix), text.clone()))
            })
            .collect::<Vec<_>>();
        let directories = self
            .directories
            .iter()
            .filter_map(|path| {
                path.strip_prefix(source)
                    .ok()
                    .map(|suffix| destination.join(suffix))
            })
            .collect::<Vec<_>>();
        self.files.extend(files);
        self.directories.extend(directories);
    }

    fn remove_tree(&mut self, path: &Path) {
        self.files
            .retain(|candidate, _| !candidate.starts_with(path));
        self.directories
            .retain(|candidate| !candidate.starts_with(path));
    }
}

/// Absolute-path filesystem fake. Failures occur before an operation mutates its tree.
#[derive(Default)]
pub struct FakeFiles {
    tree: Mutex<Tree>,
    trash_root: PathBuf,
    removable_roots: Vec<PathBuf>,
}

impl FakeFiles {
    /// Creates an empty tree whose removals are confined to the supplied roots.
    #[must_use]
    pub fn new(trash_root: PathBuf, removable_roots: Vec<PathBuf>) -> Self {
        Self {
            trash_root,
            removable_roots,
            ..Self::default()
        }
    }

    /// Seeds a file and its ancestor directories without recording a call.
    pub fn insert_text(&self, path: impl Into<PathBuf>, text: impl Into<String>) {
        let path = path.into();
        let mut tree = lock(&self.tree);
        if let Some(parent) = path.parent() {
            tree.directories
                .extend(parent.ancestors().map(Path::to_path_buf));
        }
        tree.files.insert(path, text.into());
    }

    /// Fails the next matching operation once, before it mutates the tree.
    pub fn fail_next(&self, call: FakeFilesCall, kind: io::ErrorKind) {
        lock(&self.tree).failures.push_back((call, kind));
    }

    /// Returns the ordered captured operations.
    #[must_use]
    pub fn calls(&self) -> Vec<FakeFilesCall> {
        lock(&self.tree).calls.clone()
    }

    /// Returns the current contents of one file.
    #[must_use]
    pub fn text(&self, path: &Path) -> Option<String> {
        lock(&self.tree).files.get(path).cloned()
    }
}

impl Files for FakeFiles {
    fn read_text(&self, path: &Path) -> DaemonResult<String> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Read(path.to_path_buf()))?;
        if tree.directories.contains(path) {
            return Err(failure(path, io::ErrorKind::IsADirectory));
        }
        tree.files
            .get(path)
            .cloned()
            .ok_or_else(|| failure(path, io::ErrorKind::NotFound))
    }

    fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::CreateDir(path.to_path_buf()))?;
        tree.create_dirs(path)
    }

    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Clone(
            source.to_path_buf(),
            destination.to_path_buf(),
        ))?;
        if tree.exists(destination) {
            return Err(DaemonError::Conflict(format!(
                "copy destination already exists: {}",
                destination.display()
            )));
        }
        if !tree.exists(source) {
            return Err(failure(source, io::ErrorKind::NotFound));
        }
        if !tree.directories.contains(source) {
            return Err(failure(source, io::ErrorKind::NotADirectory));
        }
        tree.require_parent(destination)?;
        tree.copy(source, destination);
        Ok(())
    }

    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Write(path.to_path_buf(), text.to_owned()))?;
        if tree.directories.contains(path) {
            return Err(failure(path, io::ErrorKind::IsADirectory));
        }
        if let Some(parent) = path.parent() {
            tree.create_dirs(parent)?;
        }
        tree.files.insert(path.to_path_buf(), text.to_owned());
        Ok(())
    }

    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Rename(
            source.to_path_buf(),
            destination.to_path_buf(),
        ))?;
        if !tree.exists(source) {
            return Err(failure(source, io::ErrorKind::NotFound));
        }
        if source == destination {
            return Ok(());
        }
        tree.require_parent(destination)?;
        if tree.directories.contains(source) {
            if destination.starts_with(source) {
                return Err(failure(destination, io::ErrorKind::InvalidInput));
            }
            if tree.files.contains_key(destination) {
                return Err(failure(destination, io::ErrorKind::NotADirectory));
            }
            if tree
                .files
                .keys()
                .chain(tree.directories.iter())
                .any(|path| path != destination && path.starts_with(destination))
            {
                return Err(failure(destination, io::ErrorKind::DirectoryNotEmpty));
            }
        } else if tree.directories.contains(destination) {
            return Err(failure(destination, io::ErrorKind::IsADirectory));
        }
        tree.copy(source, destination);
        tree.remove_tree(source);
        Ok(())
    }

    fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
        self.create_dir_all(&self.trash_root)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("item");
        let destination = self.trash_root.join(format!("0-{name}"));
        self.rename(path, &destination)?;
        Ok(destination)
    }

    fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
        self.guard_strict_descendant(path)?;
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Remove(path.to_path_buf()))?;
        tree.remove_tree(path);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> DaemonResult<()> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Remove(path.to_path_buf()))?;
        if tree.directories.contains(path) {
            return Err(failure(path, io::ErrorKind::IsADirectory));
        }
        tree.files.remove(path);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        lock(&self.tree).exists(path)
    }

    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
        let tree = lock(&self.tree);
        if tree.files.contains_key(path) {
            return Err(failure(path, io::ErrorKind::NotADirectory));
        }
        if !tree.directories.contains(path) {
            return Err(failure(path, io::ErrorKind::NotFound));
        }
        Ok(tree
            .files
            .keys()
            .chain(tree.directories.iter())
            .filter(|candidate| candidate.parent() == Some(path))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
        let path = crate::adapters::files::absolute_lexical(path);
        if self
            .removable_roots
            .iter()
            .chain(std::iter::once(&self.trash_root))
            .map(|root| crate::adapters::files::absolute_lexical(root))
            .any(|root| path != root && path.starts_with(root))
        {
            Ok(())
        } else {
            Err(DaemonError::Validation(format!(
                "unsafe removal: {}",
                path.display()
            )))
        }
    }
}

fn failure(path: &Path, kind: io::ErrorKind) -> DaemonError {
    DaemonError::fs(path, io::Error::from(kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::files::RealFiles;

    fn directory_round_trip(files: &dyn Files, root: &Path) {
        let source = root.join("source");
        let copy = root.join("copy");
        let renamed = root.join("renamed");
        files
            .create_dir_all(&source.join("empty/nested"))
            .expect("create tree");
        files
            .atomic_write_text(&source.join("text/file"), "contents")
            .expect("write file");
        files.clone_dir(&source, &copy).expect("clone");
        assert_eq!(
            files.list(&copy).expect("list"),
            [copy.join("empty"), copy.join("text")]
        );
        assert!(files.exists(&copy.join("empty/nested")));
        files.rename(&copy, &renamed).expect("rename");
        assert!(!files.exists(&copy));
        assert!(!files.exists(&copy.join("empty/nested")));
        assert!(files.exists(&renamed.join("empty/nested")));
        assert_eq!(
            files
                .read_text(&renamed.join("text/file"))
                .expect("read moved file"),
            "contents"
        );
        files.remove_detached(&renamed).expect("remove tree");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while files.exists(&renamed) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!files.exists(&renamed));
        assert!(!files.exists(&renamed.join("empty/nested")));
        assert!(files.exists(&source));
    }

    #[test]
    fn fake_and_real_files_preserve_directory_trees() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path();
        directory_round_trip(
            &FakeFiles::new(root.join("trash"), vec![root.to_path_buf()]),
            root,
        );
        directory_round_trip(
            &RealFiles::new(root.join("trash"), [root.to_path_buf()]),
            root,
        );
    }

    #[test]
    fn missing_sources_and_injected_failures_do_not_mutate_the_tree() {
        let files = FakeFiles::new("/trash".into(), vec!["/data".into()]);
        files.create_dir_all(Path::new("/data")).expect("root");
        let error = files
            .clone_dir(Path::new("/missing"), Path::new("/data/copy"))
            .expect_err("missing source");
        assert!(
            matches!(error, DaemonError::Filesystem { source, .. } if source.kind() == io::ErrorKind::NotFound)
        );
        assert!(!files.exists(Path::new("/data/copy")));
        files.insert_text("/data/source/file", "kept");
        files.fail_next(
            FakeFilesCall::Rename("/data/source".into(), "/data/moved".into()),
            io::ErrorKind::PermissionDenied,
        );
        let error = files
            .rename(Path::new("/data/source"), Path::new("/data/moved"))
            .expect_err("injected failure");
        assert!(
            matches!(error, DaemonError::Filesystem { source, .. } if source.kind() == io::ErrorKind::PermissionDenied)
        );
        assert_eq!(
            files.text(Path::new("/data/source/file")).as_deref(),
            Some("kept")
        );
        assert!(!files.exists(Path::new("/data/moved")));
        files
            .rename(Path::new("/data/source"), Path::new("/data/moved"))
            .expect("one-shot failure consumed");
        assert!(!files.exists(Path::new("/data/source")));
    }
}
