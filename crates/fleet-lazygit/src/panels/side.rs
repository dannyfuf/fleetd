use super::*;

impl Lazygit {
    pub(super) fn status_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let focused = self.state.focused == PanelId::Status;
        let repo = self
            .state
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.state.root.display().to_string());
        let branch = match self.state.head() {
            Some(Head::Branch { name, .. }) => name.clone(),
            Some(Head::Detached { oid, .. }) => {
                format!("detached at {}", crate::state::short_oid(oid))
            }
            Some(Head::Unborn { name }) => format!("{name} (unborn)"),
            None => "…".to_owned(),
        };
        let head_branch = self
            .state
            .branches()
            .iter()
            .find(|candidate| candidate.is_head);
        let upstream = head_branch
            .and_then(crate::state::upstream_status)
            .unwrap_or_default();
        let operation = self.state.operation();
        let mode = crate::state::lower_mode_word(&operation);

        let mut line = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.space.sm)
            .px(theme.space.md)
            .h(theme.metrics.row_h);
        if !upstream.is_empty() {
            line = line.child(
                Text::data(upstream.clone())
                    .color(crate::views::Ansi::Yellow.color(theme))
                    .flex_none(),
            );
        }
        if let Some(mode) = mode {
            line = line.child(
                Text::data(format!("({mode})"))
                    .color(crate::views::Ansi::Yellow.color(theme))
                    .flex_none(),
            );
        }
        line = line
            .child(Text::data(repo).flex_none())
            .child(Text::data("→").faint().flex_none())
            .child(Text::data(branch).ellipsize());

        Pane::new()
            .focused(focused)
            .border(PaneBorder::None)
            .header(self.header(PanelId::Status, "Status", 0))
            .body(div().size_full().child(line))
            .into_any_element()
    }

    pub(super) fn files_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Files;
        // The header keeps counting **files**: a directory row is a fold of the list, not an
        // entry in it, so the count must not move when a directory is expanded or collapsed.
        let count = self.state.files().len();
        let tree_rows = self.state.file_tree.shared_rows();
        let cursor = self.state.cursors.files.index();
        let budget = self.budget(2);
        let list = ListView::new(
            "lazygit-files",
            tree_rows.len(),
            move |index, is_cursor, _window, cx| {
                let Some(row) = tree_rows.get(index) else {
                    return div().into_any_element();
                };
                rows::file_tree_row(row, budget, is_cursor, focused, cx)
            },
        )
        .cursor(cursor)
        .track_scroll(&self.scroll_files)
        .loading(self.state.snapshot.is_none())
        .empty(EmptyState::new("No changed files.").action("c  commit"));

        let body = self.clipped(self.rows_side, list.into_any_element(), cx);
        Pane::new()
            .focused(focused)
            .border(PaneBorder::None)
            .header(self.header(PanelId::Files, "Files", count))
            .body(body)
            .into_any_element()
    }

    pub(super) fn branches_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Branches;
        let (title, count, body) = match self.state.branch_tab {
            BranchTab::Local => self.local_branches_tab(focused, cx),
            BranchTab::Remotes => match self.state.remote_drill.clone() {
                Some(remote) => self.remote_branches_tab(remote, focused, cx),
                None => self.remotes_tab(focused, cx),
            },
            BranchTab::Tags => self.tags_tab(focused, cx),
        };
        let active = match self.state.branch_tab {
            BranchTab::Local => 0,
            BranchTab::Remotes => 1,
            BranchTab::Tags => 2,
        };
        let strip = self.tab_strip(&["Local", "Remotes", "Tags"], active, cx);
        Pane::new()
            .focused(focused)
            .border(PaneBorder::None)
            .header(self.header(PanelId::Branches, title, count).trailing(strip))
            .body(body)
            .into_any_element()
    }

    fn local_branches_tab(
        &self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> (&'static str, usize, AnyElement) {
        let now = now_seconds();
        let snapshot = self.state.snapshot.clone();
        let count = self.state.branches().len();
        let budget = self.budget(3 + 9);
        let list = ListView::new(
            "lazygit-branches",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(branch) = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.local_branches.get(index))
                else {
                    return div().into_any_element();
                };
                rows::branch_row(branch, now, budget, is_cursor, focused, cx)
            },
        );
        let body = self.side_list(
            self.rows_side,
            list,
            self.state.cursors.branches.index(),
            &self.scroll_branches,
            EmptyState::new("No branches.").action("n  new branch"),
            cx,
        );
        ("Local branches", count, body)
    }

    fn remote_branches_tab(
        &self,
        remote: String,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> (&'static str, usize, AnyElement) {
        let snapshot = self.state.snapshot.clone();
        let count = self.state.remote_branches().len();
        let budget = self.budget(0);
        let list = ListView::new(
            "lazygit-remote-branches",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(branch) = snapshot.as_ref().and_then(|snapshot| {
                    snapshot
                        .remote_branches
                        .iter()
                        .find(|group| group.remote == remote)
                        .and_then(|group| group.branches.get(index))
                }) else {
                    return div().into_any_element();
                };
                rows::remote_branch_row(branch, budget, is_cursor, focused, cx)
            },
        );
        let body = self.side_list(
            self.rows_side,
            list,
            self.state.cursors.remote_branches.index(),
            &self.scroll_remote_branches,
            EmptyState::new("No branches on this remote.").action("f  fetch"),
            cx,
        );
        ("Remotes", count, body)
    }

    fn remotes_tab(
        &self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> (&'static str, usize, AnyElement) {
        let snapshot = self.state.snapshot.clone();
        let count = self.state.remotes().len();
        let list = ListView::new(
            "lazygit-remotes",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(snapshot) = snapshot.as_ref() else {
                    return div().into_any_element();
                };
                let Some(remote) = snapshot.remotes.get(index) else {
                    return div().into_any_element();
                };
                let branches = snapshot
                    .remote_branches
                    .iter()
                    .find(|group| group.remote == remote.name)
                    .map_or(0, |group| group.branches.len());
                rows::remote_row(remote, branches, is_cursor, focused, cx)
            },
        );
        let body = self.side_list(
            self.rows_side,
            list,
            self.state.cursors.remotes.index(),
            &self.scroll_remotes,
            EmptyState::new("No remotes.").action(""),
            cx,
        );
        ("Remotes", count, body)
    }

    fn tags_tab(&self, focused: bool, cx: &mut Context<Self>) -> (&'static str, usize, AnyElement) {
        let snapshot = self.state.snapshot.clone();
        let count = self.state.tags().len();
        let list = ListView::new(
            "lazygit-tags",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(tag) = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.tags.get(index))
                else {
                    return div().into_any_element();
                };
                rows::tag_row(tag, is_cursor, focused, cx)
            },
        );
        let body = self.side_list(
            self.rows_side,
            list,
            self.state.cursors.tags.index(),
            &self.scroll_tags,
            EmptyState::new("No tags.").action("n  new tag"),
            cx,
        );
        ("Tags", count, body)
    }

    pub(super) fn commits_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Commits;
        let (title, count, body) = match self.state.commit_tab {
            CommitTab::Commits => self.commits_tab(focused, cx),
            CommitTab::Reflog => self.reflog_tab(focused, cx),
        };
        let active = usize::from(self.state.commit_tab == CommitTab::Reflog);
        let strip = self.tab_strip(&["Commits", "Reflog"], active, cx);
        Pane::new()
            .focused(focused)
            .border(PaneBorder::None)
            .header(self.header(PanelId::Commits, title, count).trailing(strip))
            .body(body)
            .into_any_element()
    }

    fn commits_tab(
        &self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> (&'static str, usize, AnyElement) {
        let now = now_seconds();
        let snapshot = self.state.snapshot.clone();
        let copied = self.state.copied.clone();
        let merged_from = merged_from(self.state.commits());
        let count = self.state.commits().len();
        let subject_budget = self.budget(2 + 9 + 3 + 4);
        let columns = rows::CommitColumns::resolve(self.side_ch as f32);
        let list = ListView::new(
            "lazygit-commits",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(commit) = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.commits.get(index))
                else {
                    return div().into_any_element();
                };
                // Green means "merged into a main branch", which requires the commit to be on
                // the upstream at all; without an upstream every commit is red.
                let merged = commit.pushed && merged_from.is_some_and(|first| index >= first);
                rows::commit_row(
                    commit,
                    now,
                    &columns,
                    rows::CommitStyle {
                        budget: subject_budget,
                        merged,
                        copied: copied.contains(&commit.oid),
                    },
                    is_cursor,
                    focused,
                    cx,
                )
            },
        );
        let body = self.side_list(
            self.rows_side,
            list,
            self.state.cursors.commits.index(),
            &self.scroll_commits,
            EmptyState::new("No commits on this branch.").action("c  commit"),
            cx,
        );
        ("Commits", count, body)
    }

    fn reflog_tab(
        &self,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> (&'static str, usize, AnyElement) {
        let snapshot = self.state.snapshot.clone();
        let count = self.state.reflog().len();
        let list = ListView::new(
            "lazygit-reflog",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(entry) = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.reflog.get(index))
                else {
                    return div().into_any_element();
                };
                rows::reflog_row(entry, is_cursor, focused, cx)
            },
        );
        let body = self.side_list(
            self.rows_side,
            list,
            self.state.cursors.reflog.index(),
            &self.scroll_reflog,
            EmptyState::new("No reflog history.").action(""),
            cx,
        );
        ("Reflog", count, body)
    }

    pub(super) fn stash_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Stash;
        let snapshot = self.state.snapshot.clone();
        let count = self.state.stashes().len();
        let cursor = self.state.cursors.stashes.index();
        let list = ListView::new(
            "lazygit-stashes",
            count,
            move |index, is_cursor, _window, cx| {
                let Some(entry) = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.stashes.get(index))
                else {
                    return div().into_any_element();
                };
                rows::stash_row(entry, is_cursor, focused, cx)
            },
        );

        let body = self.side_list(
            self.rows_stash,
            list,
            cursor,
            &self.scroll_stashes,
            EmptyState::new("No stash entries.").action("S  stash options"),
            cx,
        );
        Pane::new()
            .focused(focused)
            .border(PaneBorder::None)
            .header(self.header(PanelId::Stash, "Stash", count))
            .body(body)
            .into_any_element()
    }
}
