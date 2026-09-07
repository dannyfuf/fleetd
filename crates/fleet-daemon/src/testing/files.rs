//! In-memory filesystem with atomic tree operations and injectable failures.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::lock;
use crate::{
    DaemonError, DaemonResult,
    adapters::files::{FileKind, FileMetadata, Files},
};

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
    /// Metadata inspection of one path.
    Metadata(PathBuf),
    /// Listing of one directory.
    List(PathBuf),
}

impl FakeFilesCall {
    fn path(&self) -> &Path {
        match self {
            Self::Read(path)
            | Self::Write(path, _)
            | Self::CreateDir(path)
            | Self::Remove(path)
            | Self::Metadata(path)
            | Self::List(path)
            | Self::Clone(path, _)
            | Self::Rename(path, _) => path,
        }
    }
}

#[derive(Default)]
struct Tree {
    files: BTreeMap<PathBuf, String>,
    file_identities: BTreeMap<PathBuf, u64>,
    directories: BTreeSet<PathBuf>,
    calls: Vec<FakeFilesCall>,
    failures: VecDeque<(FakeFilesCall, io::ErrorKind)>,
    replacements_before_conditional_remove: BTreeMap<PathBuf, String>,
    next_identity: u64,
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

    fn metadata(&self, path: &Path) -> DaemonResult<FileMetadata> {
        if let Some(identity) = self.file_identities.get(path) {
            Ok(FileMetadata::fake(FileKind::File, *identity))
        } else if self.directories.contains(path) {
            Ok(FileMetadata::fake(FileKind::Directory, 0))
        } else {
            Err(failure(path, io::ErrorKind::NotFound))
        }
    }

    fn insert_file(&mut self, path: PathBuf, text: String) {
        self.next_identity = self.next_identity.saturating_add(1);
        self.files.insert(path.clone(), text);
        self.file_identities.insert(path, self.next_identity);
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

    fn copy(&mut self, source: &Path, destination: &Path, preserve_identity: bool) {
        let files = self
            .files
            .iter()
            .filter_map(|(path, text)| {
                path.strip_prefix(source).ok().map(|suffix| {
                    (
                        destination.join(suffix),
                        text.clone(),
                        self.file_identities.get(path).copied().unwrap_or_default(),
                    )
                })
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
        for (path, text, identity) in files {
            if preserve_identity {
                self.files.insert(path.clone(), text);
                self.file_identities.insert(path, identity);
            } else {
                self.insert_file(path, text);
            }
        }
        self.directories.extend(directories);
    }

    fn remove_tree(&mut self, path: &Path) {
        self.files
            .retain(|candidate, _| !candidate.starts_with(path));
        self.file_identities
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
    removable_roots: Mutex<Vec<PathBuf>>,
}

impl FakeFiles {
    /// Creates an empty tree whose removals are confined to the supplied roots.
    #[must_use]
    pub fn new(trash_root: PathBuf, removable_roots: Vec<PathBuf>) -> Self {
        Self {
            trash_root,
            removable_roots: Mutex::new(removable_roots),
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
        tree.insert_file(path, text.into());
    }

    /// Replaces a file immediately before its next identity-checked removal.
    pub fn replace_before_conditional_remove(
        &self,
        path: impl Into<PathBuf>,
        text: impl Into<String>,
    ) {
        lock(&self.tree)
            .replacements_before_conditional_remove
            .insert(path.into(), text.into());
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
        tree.copy(source, destination, false);
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
        tree.insert_file(path.to_path_buf(), text.to_owned());
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
        tree.copy(source, destination, true);
        tree.remove_tree(source);
        Ok(())
    }

    fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
        self.guard_strict_descendant(path)?;
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
        tree.file_identities.remove(path);
        Ok(())
    }

    fn metadata(&self, path: &Path) -> DaemonResult<FileMetadata> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::Metadata(path.to_path_buf()))?;
        tree.metadata(path)
    }

    fn remove_file_if_unchanged(&self, path: &Path, expected: FileMetadata) -> DaemonResult<bool> {
        let mut tree = lock(&self.tree);
        if let Some(text) = tree.replacements_before_conditional_remove.remove(path) {
            tree.insert_file(path.to_path_buf(), text);
        }
        if tree.metadata(path).ok() != Some(expected) || expected.kind != FileKind::File {
            return Ok(false);
        }
        tree.record(FakeFilesCall::Remove(path.to_path_buf()))?;
        tree.files.remove(path);
        tree.file_identities.remove(path);
        Ok(true)
    }

    fn exists(&self, path: &Path) -> bool {
        lock(&self.tree).exists(path)
    }

    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
        let mut tree = lock(&self.tree);
        tree.record(FakeFilesCall::List(path.to_path_buf()))?;
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
        let root = self
            .removable_roots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .chain(std::iter::once(&self.trash_root))
            .map(|root| (crate::adapters::files::absolute_lexical(root), root.clone()))
            .filter(|(root, _)| path != *root && path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .ok_or_else(|| {
                DaemonError::Validation(format!("unsafe removal: {}", path.display()))
            })?;
        let tree = lock(&self.tree);
        if !tree.directories.contains(&root.1) {
            return Err(failure(&root.1, io::ErrorKind::NotFound));
        }
        let relative = path
            .strip_prefix(&root.0)
            .map_err(|_| DaemonError::Validation(format!("unsafe removal: {}", path.display())))?;
        let mut parent = root.1;
        if let Some(relative_parent) = relative.parent() {
            for component in relative_parent.components() {
                parent.push(component);
                if tree.files.contains_key(&parent) {
                    return Err(failure(&parent, io::ErrorKind::NotADirectory));
                }
                if !tree.directories.contains(&parent) {
                    return Err(failure(&parent, io::ErrorKind::NotFound));
                }
            }
        }
        Ok(())
    }

    fn set_removable_roots(&self, roots: Vec<PathBuf>) {
        *self
            .removable_roots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = roots;
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

    fn assert_missing_intermediate_is_rejected(files: &dyn Files, root: &Path) {
        files.create_dir_all(root).expect("root");
        let error = files
            .guard_strict_descendant(&root.join("missing/item"))
            .expect_err("missing parent must be rejected");
        assert!(
            matches!(error, DaemonError::Filesystem { source, .. } if source.kind() == io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn fake_and_real_guards_reject_missing_intermediate_directories() {
        let temp = tempfile::tempdir().expect("temp dir");
        let fake_root = temp.path().join("fake");
        assert_missing_intermediate_is_rejected(
            &FakeFiles::new(temp.path().join("fake-trash"), vec![fake_root.clone()]),
            &fake_root,
        );
        let real_root = temp.path().join("real");
        assert_missing_intermediate_is_rejected(
            &RealFiles::new(temp.path().join("real-trash"), [real_root.clone()]),
            &real_root,
        );
    }

    fn assert_trash_is_confined(files: &dyn Files, outside: &Path) {
        files
            .atomic_write_text(outside, "keep")
            .expect("outside fixture");
        assert!(files.trash(outside).is_err());
        assert!(files.exists(outside));
    }

    #[test]
    fn fake_and_real_trash_reject_paths_outside_removable_roots() {
        let temp = tempfile::tempdir().expect("temp dir");
        let fake_outside = temp.path().join("fake-outside/item");
        assert_trash_is_confined(
            &FakeFiles::new(
                temp.path().join("fake-trash"),
                vec![temp.path().join("fake-root")],
            ),
            &fake_outside,
        );
        let real_outside = temp.path().join("real-outside/item");
        assert_trash_is_confined(
            &RealFiles::new(
                temp.path().join("real-trash"),
                [temp.path().join("real-root")],
            ),
            &real_outside,
        );
    }
}
