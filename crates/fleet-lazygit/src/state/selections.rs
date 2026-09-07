use super::*;

/// Which panel owns the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum PanelId {
    /// Panel 1: repository status.
    #[default]
    Status,
    /// Panel 2: working-tree files.
    Files,
    /// Panel 3: branches, remotes and tags.
    Branches,
    /// Panel 4: commits and the reflog.
    Commits,
    /// Panel 5: stash entries.
    Stash,
    /// The main panel on the right.
    Main,
}

impl PanelId {
    /// The five side panels, in the order `<tab>` cycles them.
    pub(crate) const SIDE: [PanelId; 5] = [
        PanelId::Status,
        PanelId::Files,
        PanelId::Branches,
        PanelId::Commits,
        PanelId::Stash,
    ];

    /// The `[n]` jump label lazygit prints in the pane title.
    #[must_use]
    pub(crate) fn jump_label(self) -> &'static str {
        match self {
            PanelId::Status => "1",
            PanelId::Files => "2",
            PanelId::Branches => "3",
            PanelId::Commits => "4",
            PanelId::Stash => "5",
            PanelId::Main => "0",
        }
    }

    /// The next side panel, wrapping.
    #[must_use]
    pub(crate) fn next_side(self) -> PanelId {
        let index = Self::SIDE.iter().position(|panel| *panel == self);
        match index {
            Some(index) => Self::SIDE[(index + 1) % Self::SIDE.len()],
            None => PanelId::Files,
        }
    }

    /// The previous side panel, wrapping.
    #[must_use]
    pub(crate) fn prev_side(self) -> PanelId {
        let index = Self::SIDE.iter().position(|panel| *panel == self);
        match index {
            Some(index) => Self::SIDE[(index + Self::SIDE.len() - 1) % Self::SIDE.len()],
            None => PanelId::Files,
        }
    }
}

/// lazygit's `+` / `_` screen modes. The cycle wraps, matching `screen_mode_actions.go`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum ScreenMode {
    /// Every panel visible.
    #[default]
    Normal,
    /// The focused side panel and the main panel share the window.
    Half,
    /// Only the focused panel.
    Full,
}

impl ScreenMode {
    /// The next mode, wrapping.
    #[must_use]
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Normal => Self::Half,
            Self::Half => Self::Full,
            Self::Full => Self::Normal,
        }
    }

    /// The previous mode, wrapping.
    #[must_use]
    pub(crate) fn prev(self) -> Self {
        match self {
            Self::Normal => Self::Full,
            Self::Half => Self::Normal,
            Self::Full => Self::Half,
        }
    }
}

/// The tab strip of panel 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum BranchTab {
    /// Local branches.
    #[default]
    Local,
    /// Remotes, and their branches once entered.
    Remotes,
    /// Tags.
    Tags,
}

/// The tab strip of panel 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum CommitTab {
    /// The commit log.
    #[default]
    Commits,
    /// The reflog.
    Reflog,
}

/// Every list cursor, one per list the UI renders.
#[derive(Clone, Debug)]
pub(crate) struct Cursors {
    /// Panel 2.
    pub(crate) files: ListCursor,
    /// Panel 3, Local tab.
    pub(crate) branches: ListCursor,
    /// Panel 3, Remotes tab, remote list.
    pub(crate) remotes: ListCursor,
    /// Panel 3, Remotes tab, branches of the entered remote.
    pub(crate) remote_branches: ListCursor,
    /// Panel 3, Tags tab.
    pub(crate) tags: ListCursor,
    /// Panel 4, Commits tab.
    pub(crate) commits: ListCursor,
    /// Panel 4, Reflog tab.
    pub(crate) reflog: ListCursor,
    /// Panel 5.
    pub(crate) stashes: ListCursor,
    /// The main panel's diff rows.
    pub(crate) main: ListCursor,
}

impl Default for Cursors {
    fn default() -> Self {
        Self {
            files: ListCursor::new(0),
            branches: ListCursor::new(0),
            remotes: ListCursor::new(0),
            remote_branches: ListCursor::new(0),
            tags: ListCursor::new(0),
            commits: ListCursor::new(0),
            reflog: ListCursor::new(0),
            stashes: ListCursor::new(0),
            main: ListCursor::new(0),
        }
    }
}

impl GitUiState {
    /// The working-tree files.
    #[must_use]
    pub(crate) fn files(&self) -> &[FileStatus] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.files)
    }

    /// The local branches.
    #[must_use]
    pub(crate) fn branches(&self) -> &[Branch] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.local_branches)
    }

    /// The configured remotes.
    #[must_use]
    pub(crate) fn remotes(&self) -> &[Remote] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.remotes)
    }

    /// The branches of the entered remote.
    #[must_use]
    pub(crate) fn remote_branches(&self) -> &[RemoteBranch] {
        let Some(remote) = self.remote_drill.as_deref() else {
            return &[];
        };
        self.snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .remote_branches
                    .iter()
                    .find(|group| group.remote == remote)
            })
            .map_or(&[][..], |group| &group.branches)
    }

    /// The tags.
    #[must_use]
    pub(crate) fn tags(&self) -> &[Tag] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.tags)
    }

    /// The commit log.
    #[must_use]
    pub(crate) fn commits(&self) -> &[Commit] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.commits)
    }

    /// The reflog.
    #[must_use]
    pub(crate) fn reflog(&self) -> &[ReflogEntry] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.reflog)
    }

    /// The stash entries.
    #[must_use]
    pub(crate) fn stashes(&self) -> &[StashEntry] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.stashes)
    }

    /// The current HEAD.
    #[must_use]
    pub(crate) fn head(&self) -> Option<&Head> {
        self.snapshot.as_ref().map(|snapshot| &snapshot.head)
    }

    /// The in-progress operation, if any.
    #[must_use]
    pub(crate) fn operation(&self) -> OperationState {
        self.snapshot
            .as_ref()
            .map_or(OperationState::None, |snapshot| snapshot.operation.clone())
    }

    /// The branch HEAD points at, when it is a branch.
    #[must_use]
    pub(crate) fn head_branch(&self) -> Option<&str> {
        match self.head() {
            Some(Head::Branch { name, .. }) | Some(Head::Unborn { name }) => Some(name.as_str()),
            _ => None,
        }
    }

    /// The selected Files row: a file, or a directory in tree mode.
    #[must_use]
    pub(crate) fn file_row(&self) -> Option<&FileRow> {
        self.file_tree.row(self.cursors.files.index())
    }

    /// The selected working-tree file, or `None` when the cursor is on a directory row.
    #[must_use]
    pub(crate) fn selected_file(&self) -> Option<&FileStatus> {
        let index = self.file_row()?.file?;
        self.files().get(index)
    }

    /// The selected local branch.
    #[must_use]
    pub(crate) fn selected_branch(&self) -> Option<&Branch> {
        self.branches().get(self.cursors.branches.index())
    }

    /// The selected remote.
    #[must_use]
    pub(crate) fn selected_remote(&self) -> Option<&Remote> {
        self.remotes().get(self.cursors.remotes.index())
    }

    /// The selected remote branch.
    #[must_use]
    pub(crate) fn selected_remote_branch(&self) -> Option<&RemoteBranch> {
        self.remote_branches()
            .get(self.cursors.remote_branches.index())
    }

    /// The selected tag.
    #[must_use]
    pub(crate) fn selected_tag(&self) -> Option<&Tag> {
        self.tags().get(self.cursors.tags.index())
    }

    /// The selected commit.
    #[must_use]
    pub(crate) fn selected_commit(&self) -> Option<&Commit> {
        self.commits().get(self.cursors.commits.index())
    }

    /// The selected reflog entry.
    #[must_use]
    pub(crate) fn selected_reflog(&self) -> Option<&ReflogEntry> {
        self.reflog().get(self.cursors.reflog.index())
    }

    /// The selected stash entry.
    #[must_use]
    pub(crate) fn selected_stash(&self) -> Option<&StashEntry> {
        self.stashes().get(self.cursors.stashes.index())
    }

    /// Points the Files cursor back at [`GitUiState::sel_file`] after the row list changed.
    ///
    /// A collapse deletes the row the cursor was on, so the lookup falls back to the deepest
    /// visible ancestor: the directory that swallowed the selection, which is where lazygit
    /// leaves the cursor.
    pub(crate) fn resync_file_rows(&mut self) {
        let tree = &self.file_tree;
        let wanted = self.sel_file.clone();
        self.cursors.files.retain(tree.len(), |_| {
            wanted.as_ref().and_then(|path| tree.index_of(path))
        });
        self.sel_file = self
            .file_tree
            .row(self.cursors.files.index())
            .map(|row| row.path.clone());
    }

    /// `` ` `` / `~` — switch the Files pane between the tree and the flat layout.
    pub(crate) fn toggle_file_tree_mode(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.toggle_mode(&snapshot.files);
        self.resync_file_rows();
    }

    /// `enter` on a directory row — collapse it, or expand it again.
    pub(crate) fn toggle_file_collapsed(&mut self, path: &Path) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.toggle_collapsed(path, &snapshot.files);
        self.resync_file_rows();
    }

    /// `-` — collapse every directory.
    pub(crate) fn collapse_all_files(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.collapse_all(&snapshot.files);
        self.resync_file_rows();
    }

    /// `=` — expand every directory.
    pub(crate) fn expand_all_files(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.expand_all(&snapshot.files);
        self.resync_file_rows();
    }

    /// Records the identity of whatever the focused panel now points at.
    pub(crate) fn remember_selection(&mut self) {
        match self.focused {
            PanelId::Files => {
                self.sel_file = self.file_row().map(|row| row.path.clone());
            }
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => {
                    self.sel_branch = self.selected_branch().map(|branch| branch.name.clone());
                }
                BranchTab::Remotes => {
                    if self.remote_drill.is_some() {
                        self.sel_remote_branch = self
                            .selected_remote_branch()
                            .map(|branch| branch.name.clone());
                    } else {
                        self.sel_remote = self.selected_remote().map(|remote| remote.name.clone());
                    }
                }
                BranchTab::Tags => {
                    self.sel_tag = self.selected_tag().map(|tag| tag.name.clone());
                }
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => {
                    self.sel_commit = self.selected_commit().map(|commit| commit.oid.clone());
                }
                CommitTab::Reflog => {
                    self.sel_reflog = self.selected_reflog().map(ReflogIdentity::from);
                }
            },
            PanelId::Stash => {
                self.sel_stash = self.selected_stash().map(|entry| entry.oid.clone());
            }
            PanelId::Status | PanelId::Main => {}
        }
    }

    /// The cursor of the list the focused panel renders.
    #[must_use]
    pub(crate) fn focused_cursor(&self) -> &ListCursor {
        match self.focused {
            PanelId::Status => &self.cursors.main,
            PanelId::Files => &self.cursors.files,
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => &self.cursors.branches,
                BranchTab::Remotes if self.remote_drill.is_some() => &self.cursors.remote_branches,
                BranchTab::Remotes => &self.cursors.remotes,
                BranchTab::Tags => &self.cursors.tags,
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => &self.cursors.commits,
                CommitTab::Reflog => &self.cursors.reflog,
            },
            PanelId::Stash => &self.cursors.stashes,
            PanelId::Main => &self.cursors.main,
        }
    }

    /// The cursor of the list the focused panel renders, mutably.
    pub(crate) fn focused_cursor_mut(&mut self) -> &mut ListCursor {
        match self.focused {
            PanelId::Status => &mut self.cursors.main,
            PanelId::Files => &mut self.cursors.files,
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => &mut self.cursors.branches,
                BranchTab::Remotes if self.remote_drill.is_some() => {
                    &mut self.cursors.remote_branches
                }
                BranchTab::Remotes => &mut self.cursors.remotes,
                BranchTab::Tags => &mut self.cursors.tags,
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => &mut self.cursors.commits,
                CommitTab::Reflog => &mut self.cursors.reflog,
            },
            PanelId::Stash => &mut self.cursors.stashes,
            PanelId::Main => &mut self.cursors.main,
        }
    }

    /// Appends a command-log line, capped at [`COMMAND_LOG_CAP`].
    pub(crate) fn log_command(&mut self, line: impl Into<gpui::SharedString>) {
        if self.command_log.len() >= COMMAND_LOG_CAP {
            self.command_log.pop_front();
        }
        self.command_log.push_back(line.into());
    }
}
