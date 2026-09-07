use super::*;

/// What the main panel is showing.
#[derive(Clone, Debug, Default)]
pub(crate) enum MainContent {
    /// The Status panel is focused: repository summary.
    #[default]
    Summary,
    /// A working-tree file, both sides.
    FileDiff {
        /// The file.
        path: PathBuf,
        /// Worktree vs index.
        unstaged: Option<Arc<Diff>>,
        /// Index vs HEAD.
        staged: Option<Arc<Diff>>,
        /// Failure from the read for this path.
        error: Option<String>,
    },
    /// A commit's diff and file list.
    CommitDiff {
        /// The commit.
        oid: ObjectId,
        /// Its diff.
        diff: Option<Arc<Diff>>,
        /// Its changed files.
        files: Arc<[CommitFile]>,
        /// Failure from the commit read.
        error: Option<String>,
    },
    /// A branch's diff against its merge base with HEAD.
    BranchDiff {
        /// The branch.
        name: String,
        /// Its diff.
        diff: Option<Arc<Diff>>,
        /// Failure from the branch read.
        error: Option<String>,
    },
    /// A stash entry's diff.
    StashDiff {
        /// Stash index.
        index: usize,
        /// Its diff.
        diff: Option<Arc<Diff>>,
        /// Failure from the stash read.
        error: Option<String>,
    },
    /// A ref's commits — lazygit's sub-commits view, drilled into from a branch.
    SubCommits {
        /// The ref whose log this is.
        reference: String,
        /// Its commits, newest first.
        commits: Arc<[Commit]>,
        /// The sub-commit whose patch the lower half shows.
        shown: Option<ObjectId>,
        /// That patch, once read.
        diff: Option<Arc<Diff>>,
        /// Failure from loading the ref's commit list.
        commits_error: Option<String>,
        /// Failure from loading the shown commit's patch.
        diff_error: Option<String>,
    },
    /// A commit's changed files — lazygit's commit-files view, drilled into from a commit.
    CommitFiles {
        /// The commit.
        oid: ObjectId,
        /// Its subject, for the header row.
        subject: String,
        /// Its changed files.
        files: Arc<[CommitFile]>,
        /// The whole-commit patch, shown while the header row is selected.
        whole: Option<Arc<Diff>>,
        /// The file whose patch the lower half shows; `None` is the header row.
        shown: Option<PathBuf>,
        /// That patch, once read.
        diff: Option<Arc<Diff>>,
        /// Failure from loading the whole commit and its file list.
        whole_error: Option<String>,
        /// Failure from loading the selected file's patch.
        diff_error: Option<String>,
    },
    /// A remote's URLs.
    RemoteInfo {
        /// Remote name.
        name: String,
    },
    /// A tag's target and subject.
    TagInfo {
        /// Tag name.
        name: String,
    },
    /// Conflict resolution for one path.
    Conflict {
        /// The conflicted path.
        path: PathBuf,
        /// The parsed file, once read.
        file: Option<Arc<ConflictFile>>,
        /// Which conflict section is selected.
        section: usize,
        /// Failure from reading the conflicted file.
        error: Option<String>,
    },
    /// Nothing to show; the string is the reason.
    Empty(String),
}

impl MainContent {
    fn read_error_slot(
        &mut self,
        identity: &crate::bridge::ReadIdentity,
    ) -> Option<&mut Option<String>> {
        match (self, identity) {
            (Self::FileDiff { path, error, .. }, crate::bridge::ReadIdentity::FileDiff(read))
                if path == read =>
            {
                Some(error)
            }
            (
                Self::CommitDiff { oid, error, .. },
                crate::bridge::ReadIdentity::CommitDiff(read),
            ) if oid == read => Some(error),
            (
                Self::BranchDiff { name, error, .. },
                crate::bridge::ReadIdentity::BranchDiff(read),
            ) if name == read => Some(error),
            (
                Self::StashDiff { index, error, .. },
                crate::bridge::ReadIdentity::StashDiff(read),
            ) if index == read => Some(error),
            (
                Self::SubCommits {
                    reference,
                    commits_error,
                    ..
                },
                crate::bridge::ReadIdentity::RefCommits(read),
            ) if reference == read => Some(commits_error),
            (
                Self::SubCommits {
                    shown: Some(shown),
                    diff_error,
                    ..
                },
                crate::bridge::ReadIdentity::CommitDiff(read),
            ) if shown == read => Some(diff_error),
            (
                Self::CommitFiles {
                    oid, whole_error, ..
                },
                crate::bridge::ReadIdentity::CommitDiff(read),
            ) if oid == read => Some(whole_error),
            (
                Self::CommitFiles {
                    oid,
                    shown: Some(shown),
                    diff_error,
                    ..
                },
                crate::bridge::ReadIdentity::CommitFileDiff {
                    oid: read_oid,
                    path,
                },
            ) if oid == read_oid && shown == path => Some(diff_error),
            (Self::Conflict { path, error, .. }, crate::bridge::ReadIdentity::Conflict(read))
                if path == read =>
            {
                Some(error)
            }
            _ => None,
        }
    }

    pub(crate) fn record_read_error(
        &mut self,
        identity: &crate::bridge::ReadIdentity,
        message: String,
    ) -> bool {
        let Some(slot) = self.read_error_slot(identity) else {
            return false;
        };
        *slot = Some(message);
        true
    }

    pub(crate) fn clear_read_error(&mut self, identity: &crate::bridge::ReadIdentity) {
        if let Some(slot) = self.read_error_slot(identity) {
            *slot = None;
        }
    }
}

/// Replaces a cached patch only when its content really changed.
///
/// The periodic refresh re-reads the same diff every couple of seconds. Keeping the existing
/// `Arc` when the bytes are identical is what lets the pointer-keyed `DiffModel` cache survive a
/// refresh — and with it the syntax pass, the scroll offset and the cursor. Comparing two
/// parsed diffs is a walk over the line vectors; rebuilding the model and re-highlighting it is
/// far more expensive.
pub(crate) fn keep_or_replace(slot: &mut Option<Arc<Diff>>, fresh: Arc<Diff>) {
    match slot {
        Some(existing) if **existing == *fresh => {}
        _ => *slot = Some(fresh),
    }
}
/// Keeps one list cursor on the item it was on across a snapshot swap.
///
/// The match is by identity (`key`), never by index: a refresh that inserts or removes rows above
/// the cursor must not move the selection. `selected` is rewritten from wherever the cursor
/// actually landed, so it always names the row the panel highlights.
fn retain_selection<T, K: PartialEq>(
    cursor: &mut ListCursor,
    items: &[T],
    selected: &mut Option<K>,
    key: impl Fn(&T) -> K,
) {
    let wanted = selected.take();
    cursor.retain(items.len(), |_| {
        let wanted = wanted.as_ref()?;
        items.iter().position(|item| key(item) == *wanted)
    });
    *selected = items.get(cursor.index()).map(key);
}

impl GitUiState {
    /// Installs a fresh snapshot, keeping every cursor on the item it was on.
    ///
    /// Returns the follow-up requests the new snapshot implies.
    pub(crate) fn apply_snapshot(&mut self, snapshot: Box<RepoSnapshot>) -> Vec<GitRequest> {
        // Snapshots can complete out of order; a stale one must never overwrite a newer one.
        if self.snapshot.is_some() && snapshot.generation <= self.epoch {
            return Vec::new();
        }
        self.epoch = snapshot.generation;
        self.refreshing = false;
        let snapshot = Arc::new(*snapshot);

        if self
            .snapshot
            .as_ref()
            .is_none_or(|previous| previous.files != snapshot.files)
        {
            self.file_tree.rebuild(&snapshot.files);
            self.resync_file_rows();
        }

        retain_selection(
            &mut self.cursors.branches,
            &snapshot.local_branches,
            &mut self.sel_branch,
            |branch| branch.name.clone(),
        );
        retain_selection(
            &mut self.cursors.remotes,
            &snapshot.remotes,
            &mut self.sel_remote,
            |remote| remote.name.clone(),
        );
        retain_selection(
            &mut self.cursors.tags,
            &snapshot.tags,
            &mut self.sel_tag,
            |tag| tag.name.clone(),
        );
        retain_selection(
            &mut self.cursors.commits,
            &snapshot.commits,
            &mut self.sel_commit,
            |commit| commit.oid.clone(),
        );
        retain_selection(
            &mut self.cursors.reflog,
            &snapshot.reflog,
            &mut self.sel_reflog,
            |entry| ReflogIdentity::from(entry),
        );
        retain_selection(
            &mut self.cursors.stashes,
            &snapshot.stashes,
            &mut self.sel_stash,
            |entry| entry.oid.clone(),
        );

        if let Some(remote) = self.remote_drill.clone() {
            let branches = snapshot
                .remote_branches
                .iter()
                .find(|group| group.remote == remote)
                .map_or(&[][..], |group| group.branches.as_slice());
            retain_selection(
                &mut self.cursors.remote_branches,
                branches,
                &mut self.sel_remote_branch,
                |branch| branch.name.clone(),
            );
        }

        self.snapshot = Some(snapshot);
        self.refresh_main()
    }

    /// Re-derives the main panel from the focused panel's selection, keeping any cached diff.
    ///
    /// Returns the read requests needed to fill it.
    pub(crate) fn refresh_main(&mut self) -> Vec<GitRequest> {
        // Staging keeps the file diff it already has; a snapshot refresh re-reads it.
        if let Some(staging) = &self.staging {
            return vec![GitRequest::FileDiff(staging.path.clone())];
        }
        // A commit-files drill-down is keyed by an immutable OID and survives unchanged. A ref
        // log has mutable membership, so keep the drill-down open but refresh its rows.
        if self.focused == PanelId::Main {
            match &self.main {
                MainContent::CommitFiles { .. } => return Vec::new(),
                MainContent::SubCommits { reference, .. } => {
                    let reference = reference.clone();
                    let upstream = self.snapshot.as_ref().and_then(|snapshot| {
                        snapshot
                            .local_branches
                            .iter()
                            .find(|branch| branch.name == reference)
                            .and_then(|branch| branch.upstream.as_ref())
                            .map(|upstream| upstream.name.clone())
                            .or_else(|| {
                                snapshot
                                    .remote_branches
                                    .iter()
                                    .flat_map(|group| &group.branches)
                                    .any(|branch| branch.name == reference)
                                    .then(|| reference.clone())
                            })
                    });
                    return vec![GitRequest::RefCommits {
                        reference,
                        upstream,
                        limit: 300,
                    }];
                }
                _ => {}
            }
        }
        let panel = if self.focused == PanelId::Main {
            self.previous_panel
        } else {
            self.focused
        };
        match panel {
            PanelId::Status => {
                self.main = MainContent::Summary;
                Vec::new()
            }
            PanelId::Files => self.refresh_file(),
            PanelId::Branches => self.refresh_branches_tab(),
            PanelId::Commits => self.refresh_commit(),
            PanelId::Stash => self.refresh_stash(),
            PanelId::Main => Vec::new(),
        }
    }

    /// Installs a diff that just arrived. Returns whether anything changed.
    /// The Branches panel shows one of three tabs, and its Remotes tab one of two levels.
    fn refresh_branches_tab(&mut self) -> Vec<GitRequest> {
        match self.branch_tab {
            BranchTab::Local => match self.selected_branch() {
                Some(branch) => {
                    let name = branch.name.clone();
                    self.refresh_branch(name)
                }
                None => self.show_empty("No branches in this repository."),
            },
            BranchTab::Remotes if self.remote_drill.is_some() => {
                match self.selected_remote_branch() {
                    Some(branch) => {
                        let name = branch.name.clone();
                        self.refresh_branch(name)
                    }
                    None => self.show_empty("No branches on this remote."),
                }
            }
            BranchTab::Remotes => match self.selected_remote() {
                Some(remote) => {
                    self.main = MainContent::RemoteInfo {
                        name: remote.name.clone(),
                    };
                    Vec::new()
                }
                None => self.show_empty("No remotes."),
            },
            BranchTab::Tags => match self.selected_tag() {
                Some(tag) => {
                    self.main = MainContent::TagInfo {
                        name: tag.name.clone(),
                    };
                    Vec::new()
                }
                None => self.show_empty("No tags."),
            },
        }
    }

    /// A commit patch is immutable, so an already-loaded one is never re-read.
    fn refresh_commit(&mut self) -> Vec<GitRequest> {
        let oid = match self.commit_tab {
            CommitTab::Commits => self.selected_commit().map(|commit| commit.oid.clone()),
            CommitTab::Reflog => self.selected_reflog().map(|entry| entry.oid.clone()),
        };
        let Some(oid) = oid else {
            return self.show_empty("No commits on this branch.");
        };
        match &self.main {
            MainContent::CommitDiff {
                oid: current,
                diff: Some(_),
                ..
            } if *current == oid => return Vec::new(),
            MainContent::CommitDiff { oid: current, .. } if *current == oid => {}
            _ => {
                self.main = MainContent::CommitDiff {
                    oid: oid.clone(),
                    diff: None,
                    files: Arc::default(),
                    error: None,
                };
            }
        }
        vec![GitRequest::CommitDiff(oid)]
    }

    fn refresh_stash(&mut self) -> Vec<GitRequest> {
        let Some(entry) = self.selected_stash() else {
            return self.show_empty("No stash entries.");
        };
        let index = entry.index;
        if !matches!(&self.main, MainContent::StashDiff { index: current, .. } if *current == index)
        {
            self.main = MainContent::StashDiff {
                index,
                diff: None,
                error: None,
            };
        }
        vec![GitRequest::StashDiff(index)]
    }

    /// Shows `reason` in the main panel and asks for nothing.
    fn show_empty(&mut self, reason: &str) -> Vec<GitRequest> {
        self.main = MainContent::Empty(reason.to_owned());
        Vec::new()
    }

    fn refresh_file(&mut self) -> Vec<GitRequest> {
        let Some(row) = self.file_row() else {
            return self.show_empty("No changed files.");
        };
        let path = row.path.clone();
        let request = if row.is_dir {
            GitRequest::PathsDiff {
                key: path.clone(),
                paths: row.children.clone(),
            }
        } else {
            let Some(file) = row.file.and_then(|index| self.files().get(index)) else {
                return self.show_empty("No changed files.");
            };
            if file.conflict.is_some() {
                if !matches!(&self.main, MainContent::Conflict { path: current, .. } if *current == path)
                {
                    self.main = MainContent::Conflict {
                        path: path.clone(),
                        file: None,
                        section: 0,
                        error: None,
                    };
                }
                return vec![GitRequest::Conflict(path)];
            }
            GitRequest::FileDiff(path.clone())
        };
        if !matches!(&self.main, MainContent::FileDiff { path: current, .. } if *current == path) {
            self.main = MainContent::FileDiff {
                path,
                unstaged: None,
                staged: None,
                error: None,
            };
        }
        vec![request]
    }

    fn refresh_branch(&mut self, name: String) -> Vec<GitRequest> {
        if !matches!(&self.main, MainContent::BranchDiff { name: current, .. } if *current == name)
        {
            self.main = MainContent::BranchDiff {
                name: name.clone(),
                diff: None,
                error: None,
            };
        }
        vec![GitRequest::BranchDiff(name)]
    }

    pub(crate) fn apply_file_diff(
        &mut self,
        path: &Path,
        unstaged: Arc<Diff>,
        staged: Arc<Diff>,
    ) -> bool {
        match &mut self.main {
            MainContent::FileDiff {
                path: current,
                unstaged: u,
                staged: s,
                ..
            } if current == path => {
                keep_or_replace(u, unstaged);
                keep_or_replace(s, staged);
                true
            }
            _ => false,
        }
    }
}
