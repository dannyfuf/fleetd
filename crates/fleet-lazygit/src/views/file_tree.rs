//! lazygit's file tree: the Files pane as a directory tree instead of a flat path list.
//!
//! The model is a straight port of `pkg/gui/filetree` — `BuildTreeFromFiles` → `Sort` →
//! `Compress` — flattened once into the row list the pane renders:
//!
//! * **Directories first, then files**, both alphabetical.
//! * **Single-child directory chains are compressed** into one row (`src/app/deep`), which is
//!   what lazygit does instead of drawing branch glyphs; the row's *visual* depth still advances
//!   by one, so a compressed chain costs one indent level, not three.
//! * **Indentation is two spaces per visual depth** and nothing else. lazygit draws no `├─`,
//!   `└─` or `│` anywhere.
//! * A directory row carries the aggregate of its descendants — `staged`, `unstaged`,
//!   `conflict` — which is what colours its name (green fully staged, yellow mixed, default
//!   otherwise), exactly as a file's does.
//!
//! Collapsed directories are remembered **by path**, so a refresh, a stage or a branch switch
//! never re-opens a directory the user closed, and the flat/tree switch (`` ` `` / `~`) is a flag
//! on the same model rather than a second code path.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fleet_git::FileStatus;
use gpui::SharedString;

use crate::state::{has_staged, has_unstaged, short_status};

/// One rendered row of the Files pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRow {
    /// The full path from the worktree root: the file, or the directory the row stands for.
    pub(crate) path: PathBuf,
    /// What the row prints after the glyph. A compressed chain keeps its joined segments
    /// (`src/app/deep`); every other row is one path component; a flat-mode row is the full path.
    pub(crate) name: SharedString,
    /// Visual depth — two spaces of indent each. A compressed chain counts as one level.
    pub(crate) depth: usize,
    /// Whether this is a directory row.
    pub(crate) is_dir: bool,
    /// Whether a directory row is collapsed. Always `false` on a file row.
    pub(crate) collapsed: bool,
    /// Index into `RepoSnapshot::files`, on a file row.
    pub(crate) file: Option<usize>,
    /// The path a renamed file came from, so the row can print `old → new`.
    pub(crate) previous: Option<PathBuf>,
    /// Every file path at or under this row, in row order. A file row holds just its own.
    pub(crate) children: Vec<PathBuf>,
    /// Anything staged at or under this row (lazygit's `GetHasStagedChanges`).
    pub(crate) staged: bool,
    /// Anything unstaged at or under this row (lazygit's `GetHasUnstagedChanges`).
    pub(crate) unstaged: bool,
    /// Anything conflicted at or under this row.
    pub(crate) conflict: bool,
    /// The two status characters. A file row carries git's own; a directory row carries the
    /// aggregate of its descendants, which lazygit computes but does **not** print — on a
    /// directory it draws the collapse arrow instead and lets the name colour carry the state.
    pub(crate) status: [char; 2],
}

impl FileRow {
    /// The `▼` / `▶` a directory row prints in the glyph column.
    #[must_use]
    pub(crate) fn arrow(&self) -> &'static str {
        if self.collapsed {
            COLLAPSED_ARROW
        } else {
            EXPANDED_ARROW
        }
    }
}

/// `pkg/gui/presentation/files.go:17`.
pub(crate) const EXPANDED_ARROW: &str = "▼";
/// `pkg/gui/presentation/files.go:18`.
pub(crate) const COLLAPSED_ARROW: &str = "▶";

/// The Files pane's row model: tree or flat, plus the set of collapsed directories.
#[derive(Clone, Debug)]
pub(crate) struct FileTree {
    /// `gui.showFileTree`, lazygit's default is `true`.
    tree_mode: bool,
    /// Collapsed directory paths, kept across refreshes so a snapshot never re-opens one.
    collapsed: HashSet<PathBuf>,
    /// The flattened rows the pane renders.
    rows: Arc<[FileRow]>,
}

impl Default for FileTree {
    fn default() -> Self {
        Self {
            tree_mode: true,
            collapsed: HashSet::new(),
            rows: Arc::default(),
        }
    }
}

impl FileTree {
    /// One row.
    #[must_use]
    pub(crate) fn row(&self, index: usize) -> Option<&FileRow> {
        self.rows.get(index)
    }

    /// How many rows there are.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Flips tree and flat layout and rebuilds. Not persisted, as in lazygit.
    pub(crate) fn toggle_mode(&mut self, files: &[FileStatus]) {
        self.tree_mode = !self.tree_mode;
        self.rebuild(files);
    }

    /// Collapses or expands one directory. A file path is ignored.
    pub(crate) fn toggle_collapsed(&mut self, path: &Path, files: &[FileStatus]) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_path_buf());
        }
        self.rebuild(files);
    }

    /// Collapses every directory (`-`).
    pub(crate) fn collapse_all(&mut self, files: &[FileStatus]) {
        for path in all_directories(files) {
            self.collapsed.insert(path);
        }
        self.rebuild(files);
    }

    /// Expands every directory (`=`).
    pub(crate) fn expand_all(&mut self, files: &[FileStatus]) {
        self.collapsed.clear();
        self.rebuild(files);
    }

    /// The row index for a path: the row itself, or the deepest visible ancestor of it.
    ///
    /// Collapsing a directory hides the row the cursor was on, and lazygit then leaves the
    /// cursor on the directory that swallowed it rather than jumping to the top.
    #[must_use]
    pub(crate) fn index_of(&self, path: &Path) -> Option<usize> {
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            return Some(index);
        }
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_dir && path.starts_with(&row.path))
            .max_by_key(|(_, row)| row.path.as_os_str().len())
            .map(|(index, _)| index)
    }

    /// Rebuilds the rows from a snapshot's files, keeping the collapse set.
    pub(crate) fn rebuild(&mut self, files: &[FileStatus]) {
        self.rows = if self.tree_mode {
            tree_rows(files, &self.collapsed)
        } else {
            flat_rows(files)
        }
        .into();
    }

    pub(crate) fn shared_rows(&self) -> Arc<[FileRow]> {
        self.rows.clone()
    }
}

/// A node of the intermediate tree, before flattening.
#[derive(Debug, Default)]
struct Node {
    /// Child directories, keyed by their own name — a `BTreeMap` is the alphabetical sort.
    dirs: BTreeMap<String, Node>,
    /// Files directly in this directory: `(name, index into files)`, alphabetical.
    files: BTreeMap<String, usize>,
    aggregate: Aggregate,
}

fn insert(node: &mut Node, components: &[String], index: usize) {
    match components {
        [] => {}
        [name] => {
            node.files.insert(name.clone(), index);
        }
        [head, tail @ ..] => insert(node.dirs.entry(head.clone()).or_default(), tail, index),
    }
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

/// Every directory path the files imply, so collapse-all can name them all.
fn all_directories(files: &[FileStatus]) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    for file in files {
        let mut prefix = PathBuf::new();
        let parts = components(&file.path);
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            prefix.push(part);
            out.insert(prefix.clone());
        }
    }
    out
}

/// lazygit's `Compress`: while a directory's only child is a directory, the child replaces it and
/// the display name grows by one segment. The chain still costs exactly one indent level.
fn compress<'a>(name: &str, node: &'a Node, path: &Path) -> (String, PathBuf, &'a Node) {
    let mut name = name.to_owned();
    let mut path = path.to_path_buf();
    let mut node = node;
    while node.files.is_empty() && node.dirs.len() == 1 {
        let Some((child_name, child)) = node.dirs.iter().next() else {
            break;
        };
        name = format!("{name}/{child_name}");
        path = path.join(child_name);
        node = child;
    }
    (name, path, node)
}

/// Aggregate before emission, so collapsed subtrees create no discarded rows.
fn aggregate(node: &mut Node, files: &[FileStatus]) {
    let mut total = Aggregate::default();
    for child in node.dirs.values_mut() {
        aggregate(child, files);
        total.merge(&child.aggregate);
    }
    for index in node.files.values() {
        let file = &files[*index];
        total.paths.push(file.path.clone());
        total.staged |= has_staged(file);
        total.unstaged |= has_unstaged(file);
        total.conflict |= file.conflict.is_some();
    }
    node.aggregate = total;
}

fn file_row(name: String, index: usize, depth: usize, file: &FileStatus) -> FileRow {
    FileRow {
        path: file.path.clone(),
        name: name.into(),
        depth,
        is_dir: false,
        collapsed: false,
        file: Some(index),
        previous: file.previous_path.clone(),
        children: vec![file.path.clone()],
        staged: has_staged(file),
        unstaged: has_unstaged(file),
        conflict: file.conflict.is_some(),
        status: short_status(file),
    }
}

fn walk(
    node: &Node,
    path: &Path,
    depth: usize,
    files: &[FileStatus],
    collapsed: &HashSet<PathBuf>,
    out: &mut Vec<FileRow>,
) {
    for (name, child) in &node.dirs {
        let (name, child_path, child) = compress(name, child, &path.join(name));
        let is_collapsed = collapsed.contains(&child_path);
        let aggregate = &child.aggregate;
        out.push(FileRow {
            path: child_path.clone(),
            name: name.into(),
            depth,
            is_dir: true,
            collapsed: is_collapsed,
            file: None,
            previous: None,
            children: aggregate.paths.clone(),
            staged: aggregate.staged,
            unstaged: aggregate.unstaged,
            conflict: aggregate.conflict,
            status: [' ', ' '],
        });
        if !is_collapsed {
            walk(child, &child_path, depth + 1, files, collapsed, out);
        }
    }
    for (name, index) in &node.files {
        out.push(file_row(name.clone(), *index, depth, &files[*index]));
    }
}

/// What a subtree contributes to the directory above it.
#[derive(Debug, Default)]
struct Aggregate {
    paths: Vec<PathBuf>,
    staged: bool,
    unstaged: bool,
    conflict: bool,
}

impl Aggregate {
    fn merge(&mut self, other: &Aggregate) {
        self.paths.extend(other.paths.iter().cloned());
        self.staged |= other.staged;
        self.unstaged |= other.unstaged;
        self.conflict |= other.conflict;
    }
}

/// lazygit's `tracked`: `!lo.Contains([]string{"??", "A ", "AM"}, shortStatus)`.
fn is_tracked(file: &FileStatus) -> bool {
    !matches!(short_status(file), ['?', '?'] | ['A', ' '] | ['A', 'M'])
}

fn tree_rows(files: &[FileStatus], collapsed: &HashSet<PathBuf>) -> Vec<FileRow> {
    let mut root = Node::default();
    for (index, file) in files.iter().enumerate() {
        insert(&mut root, &components(&file.path), index);
    }
    aggregate(&mut root, files);
    let mut rows = Vec::new();
    walk(&root, Path::new(""), 0, files, collapsed, &mut rows);
    rows
}

/// lazygit's `BuildFlatTreeFromFiles`: the tree's leaves, then stably re-sorted so merge
/// conflicts come first, then tracked files, then untracked ones.
fn flat_rows(files: &[FileStatus]) -> Vec<FileRow> {
    fn collect(node: &Node, files: &[FileStatus], rows: &mut Vec<FileRow>) {
        for child in node.dirs.values() {
            collect(child, files, rows);
        }
        for index in node.files.values() {
            let file = &files[*index];
            rows.push(file_row(
                file.path.to_string_lossy().into_owned(),
                *index,
                0,
                file,
            ));
        }
    }
    let mut root = Node::default();
    for (index, file) in files.iter().enumerate() {
        insert(&mut root, &components(&file.path), index);
    }
    let mut rows = Vec::with_capacity(files.len());
    collect(&root, files, &mut rows);
    rows.sort_by_key(|row| {
        let file = row.file.map(|index| &files[index]);
        if row.conflict {
            0
        } else if file.is_some_and(is_tracked) {
            1
        } else {
            2
        }
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_git::ChangeKind;

    fn file(path: &str, index: ChangeKind, worktree: ChangeKind) -> FileStatus {
        FileStatus {
            path: PathBuf::from(path),
            previous_path: None,
            index,
            worktree,
            conflict: None,
        }
    }

    fn modified(path: &str) -> FileStatus {
        file(path, ChangeKind::Unmodified, ChangeKind::Modified)
    }

    fn staged(path: &str) -> FileStatus {
        file(path, ChangeKind::Modified, ChangeKind::Unmodified)
    }

    fn untracked(path: &str) -> FileStatus {
        file(path, ChangeKind::Untracked, ChangeKind::Untracked)
    }

    fn tree(files: &[FileStatus]) -> FileTree {
        let mut tree = FileTree::default();
        tree.rebuild(files);
        tree
    }

    fn shape(tree: &FileTree) -> Vec<String> {
        tree.shared_rows()
            .iter()
            .map(|row| {
                format!(
                    "{}{}{}",
                    "  ".repeat(row.depth),
                    if row.is_dir { row.arrow() } else { "" },
                    row.name
                )
            })
            .collect()
    }

    #[test]
    fn directories_come_first_and_both_halves_are_alphabetical() {
        let files = [
            modified("zed.txt"),
            modified("alpha.txt"),
            modified("src/b.txt"),
            modified("pkg/a.txt"),
        ];
        assert_eq!(
            shape(&tree(&files)),
            vec!["▼pkg", "  a.txt", "▼src", "  b.txt", "alpha.txt", "zed.txt"]
        );
    }

    #[test]
    fn a_single_child_directory_chain_is_one_row_at_one_indent() {
        let files = [modified("src/app/deep/one.txt"), modified("top.txt")];
        assert_eq!(
            shape(&tree(&files)),
            vec!["▼src/app/deep", "  one.txt", "top.txt"]
        );
        let rows = tree(&files);
        assert_eq!(rows.row(0).unwrap().depth, 0);
        assert_eq!(rows.row(0).unwrap().path, PathBuf::from("src/app/deep"));
        assert_eq!(rows.row(1).unwrap().depth, 1);
    }

    #[test]
    fn a_chain_stops_compressing_where_it_branches() {
        let files = [modified("src/app/one.txt"), modified("src/lib/two.txt")];
        assert_eq!(
            shape(&tree(&files)),
            vec!["▼src", "  ▼app", "    one.txt", "  ▼lib", "    two.txt"]
        );
    }

    #[test]
    fn a_directory_aggregates_its_descendants() {
        let files = [
            staged("src/a.txt"),
            modified("src/deep/b.txt"),
            untracked("src/deep/c.txt"),
        ];
        let tree = tree(&files);
        let root = tree.row(0).unwrap();
        assert_eq!(root.path, PathBuf::from("src"));
        assert!(root.staged && root.unstaged && !root.conflict);
        assert!(root.staged && root.unstaged);
        assert_eq!(
            root.children,
            vec![
                PathBuf::from("src/deep/b.txt"),
                PathBuf::from("src/deep/c.txt"),
                PathBuf::from("src/a.txt"),
            ]
        );
        let deep = tree.row(1).unwrap();
        assert_eq!(deep.path, PathBuf::from("src/deep"));
        assert!(!deep.staged && deep.unstaged);
        assert!(!deep.staged && deep.unstaged);
    }

    #[test]
    fn collapsing_hides_the_subtree_and_survives_a_rebuild() {
        let files = [modified("src/app/one.txt"), modified("top.txt")];
        let mut tree = tree(&files);
        tree.toggle_collapsed(Path::new("src/app"), &files);
        assert_eq!(shape(&tree), vec!["▶src/app", "top.txt"]);
        // A directory the user closed keeps its children for `space` and `d`.
        assert_eq!(
            tree.row(0).unwrap().children,
            vec![PathBuf::from("src/app/one.txt")]
        );
        // A refresh must not re-open it.
        tree.rebuild(&files);
        assert_eq!(shape(&tree), vec!["▶src/app", "top.txt"]);
        tree.toggle_collapsed(Path::new("src/app"), &files);
        assert_eq!(shape(&tree), vec!["▼src/app", "  one.txt", "top.txt"]);
    }

    #[test]
    fn collapse_all_and_expand_all_cover_every_level() {
        let files = [modified("src/app/one.txt"), modified("src/lib/two.txt")];
        let mut tree = tree(&files);
        tree.collapse_all(&files);
        assert_eq!(shape(&tree), vec!["▶src"]);
        tree.expand_all(&files);
        assert_eq!(
            shape(&tree),
            vec!["▼src", "  ▼app", "    one.txt", "  ▼lib", "    two.txt"]
        );
    }

    #[test]
    fn a_hidden_row_resolves_to_the_directory_that_swallowed_it() {
        let files = [modified("src/app/one.txt"), modified("top.txt")];
        let mut tree = tree(&files);
        assert_eq!(tree.index_of(Path::new("src/app/one.txt")), Some(1));
        tree.toggle_collapsed(Path::new("src/app"), &files);
        assert_eq!(tree.index_of(Path::new("src/app/one.txt")), Some(0));
        assert_eq!(tree.index_of(Path::new("nowhere.txt")), None);
    }

    #[test]
    fn flat_mode_lists_full_paths_with_untracked_last() {
        let files = [
            untracked("src/app/new.txt"),
            modified("src/app/one.txt"),
            modified("top.txt"),
        ];
        let mut tree = tree(&files);
        tree.toggle_mode(&files);
        assert!(!tree.tree_mode);
        assert_eq!(
            shape(&tree),
            vec!["src/app/one.txt", "top.txt", "src/app/new.txt"]
        );
        assert!(
            tree.shared_rows()
                .iter()
                .all(|row| !row.is_dir && row.depth == 0)
        );
        tree.toggle_mode(&files);
        assert!(tree.tree_mode);
    }

    #[test]
    fn every_file_row_points_back_at_its_status() {
        let files = [modified("src/app/one.txt"), staged("top.txt")];
        let tree = tree(&files);
        for row in tree.shared_rows().iter().filter(|row| !row.is_dir) {
            let index = row.file.expect("a file row carries its index");
            assert_eq!(files[index].path, row.path);
        }
    }
}
