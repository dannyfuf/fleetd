use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MainIdentity {
    Summary,
    File(PathBuf),
    Commit(fleet_git::ObjectId),
    Branch(String),
    Stash(usize),
    SubCommits(String),
    CommitFiles(fleet_git::ObjectId),
    Remote(String),
    Tag(String),
    Conflict(PathBuf),
    Empty,
}

fn main_identity(content: &MainContent) -> MainIdentity {
    match content {
        MainContent::Summary => MainIdentity::Summary,
        MainContent::FileDiff { path, .. } => MainIdentity::File(path.clone()),
        MainContent::CommitDiff { oid, .. } => MainIdentity::Commit(oid.clone()),
        MainContent::BranchDiff { name, .. } => MainIdentity::Branch(name.clone()),
        MainContent::StashDiff { index, .. } => MainIdentity::Stash(*index),
        MainContent::SubCommits { reference, .. } => MainIdentity::SubCommits(reference.clone()),
        MainContent::CommitFiles { oid, .. } => MainIdentity::CommitFiles(oid.clone()),
        MainContent::RemoteInfo { name } => MainIdentity::Remote(name.clone()),
        MainContent::TagInfo { name } => MainIdentity::Tag(name.clone()),
        MainContent::Conflict { path, .. } => MainIdentity::Conflict(path.clone()),
        MainContent::Empty(_) => MainIdentity::Empty,
    }
}

impl Lazygit {
    pub(super) fn current_main_identity(&self) -> MainIdentity {
        main_identity(&self.state.main)
    }

    pub(super) fn reset_main_scroll_if_changed(&self, previous: &MainIdentity) {
        if &self.current_main_identity() != previous {
            self.scroll_main
                .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
            self.scroll_secondary
                .scroll_to_item_strict(0, gpui::ScrollStrategy::Top);
        }
    }

    pub(super) fn refresh_main(&mut self) {
        let previous = self.current_main_identity();
        for request in self.state.refresh_main() {
            self.send(request);
        }
        self.state.cursors.main.set(0);
        self.state.main_h_scroll = 0.0;
        self.sync_main_len();
        self.reset_main_scroll_if_changed(&previous);
    }

    pub(super) fn scroll_handle(&self) -> &UniformListScrollHandle {
        match self.state.focused {
            PanelId::Status => &self.scroll_main,
            PanelId::Files => &self.scroll_files,
            PanelId::Branches => match self.state.branch_tab {
                BranchTab::Local => &self.scroll_branches,
                BranchTab::Remotes if self.state.remote_drill.is_some() => {
                    &self.scroll_remote_branches
                }
                BranchTab::Remotes => &self.scroll_remotes,
                BranchTab::Tags => &self.scroll_tags,
            },
            PanelId::Commits => match self.state.commit_tab {
                CommitTab::Commits => &self.scroll_commits,
                CommitTab::Reflog => &self.scroll_reflog,
            },
            PanelId::Stash => &self.scroll_stashes,
            PanelId::Main => &self.scroll_main,
        }
    }

    pub(super) fn next_screen_mode(
        &mut self,
        _: &global::NextScreenMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.screen_mode = self.state.screen_mode.next();
        cx.notify();
    }

    pub(super) fn prev_screen_mode(
        &mut self,
        _: &global::PrevScreenMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.screen_mode = self.state.screen_mode.prev();
        cx.notify();
    }

    pub(super) fn toggle_command_log(
        &mut self,
        _: &global::ToggleCommandLog,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.show_command_log = !self.state.show_command_log;
        cx.notify();
    }

    pub(super) fn cancel(&mut self, _: &global::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.pop_overlay() {
            cx.notify();
            return;
        }
        if self.state.staging.is_some() {
            self.state.staging = None;
            self.state.focused = self.state.previous_panel;
            self.refresh_main();
            cx.notify();
            return;
        }
        if self.state.focused == PanelId::Main {
            self.state.focused = self.state.previous_panel;
            // A drill-down is not re-derivable from the side panel, so leaving it has to rebuild
            // the main panel; an ordinary diff keeps its scroll, as lazygit's `esc` does.
            if matches!(
                self.state.main,
                MainContent::SubCommits { .. } | MainContent::CommitFiles { .. }
            ) {
                self.refresh_main();
            }
            cx.notify();
            return;
        }
        if self.state.remote_drill.is_some() && self.state.branch_tab == BranchTab::Remotes {
            self.state.remote_drill = None;
            self.refresh_main();
            cx.notify();
            return;
        }
        if self.state.last_error.take().is_some() {
            self.last_error_label = None;
            cx.notify();
        }
    }

    pub(super) fn focus_panel(&mut self, panel: PanelId, cx: &mut Context<Self>) {
        if self.state.focused == panel {
            return;
        }
        if panel == PanelId::Main {
            self.state.previous_panel = if self.state.focused == PanelId::Main {
                self.state.previous_panel
            } else {
                self.state.focused
            };
        } else {
            self.state.previous_panel = panel;
        }
        self.state.focused = panel;
        if panel != PanelId::Main {
            self.state.staging = None;
            self.refresh_main();
        }
        cx.notify();
    }

    pub(super) fn focus_status(
        &mut self,
        _: &global::FocusStatus,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Status, cx);
    }

    pub(super) fn focus_files(
        &mut self,
        _: &global::FocusFiles,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Files, cx);
    }

    pub(super) fn focus_branches(
        &mut self,
        _: &global::FocusBranches,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Branches, cx);
    }

    pub(super) fn focus_commits(
        &mut self,
        _: &global::FocusCommits,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Commits, cx);
    }

    pub(super) fn focus_stash(
        &mut self,
        _: &global::FocusStash,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Stash, cx);
    }

    pub(super) fn focus_main(
        &mut self,
        _: &global::FocusMain,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_panel(PanelId::Main, cx);
    }

    pub(super) fn next_panel(
        &mut self,
        _: &global::NextPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = if self.state.focused == PanelId::Main {
            self.state.previous_panel
        } else {
            self.state.focused.next_side()
        };
        self.focus_panel(panel, cx);
    }

    pub(super) fn prev_panel(
        &mut self,
        _: &global::PrevPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = if self.state.focused == PanelId::Main {
            self.state.previous_panel
        } else {
            self.state.focused.prev_side()
        };
        self.focus_panel(panel, cx);
    }

    pub(super) fn cycle_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        match self.state.focused {
            PanelId::Branches => {
                self.state.branch_tab = match (self.state.branch_tab, forward) {
                    (BranchTab::Local, true) => BranchTab::Remotes,
                    (BranchTab::Remotes, true) => BranchTab::Tags,
                    (BranchTab::Tags, true) => BranchTab::Local,
                    (BranchTab::Local, false) => BranchTab::Tags,
                    (BranchTab::Remotes, false) => BranchTab::Local,
                    (BranchTab::Tags, false) => BranchTab::Remotes,
                };
                self.state.remote_drill = None;
                self.refresh_main();
            }
            PanelId::Commits => {
                self.state.commit_tab = match self.state.commit_tab {
                    CommitTab::Commits => CommitTab::Reflog,
                    CommitTab::Reflog => CommitTab::Commits,
                };
                self.refresh_main();
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn next_tab(&mut self, _: &global::NextTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(true, cx);
    }

    pub(super) fn prev_tab(&mut self, _: &global::PrevTab, _: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(false, cx);
    }

    pub(super) fn motion(&mut self, motion: ListMotion, cx: &mut Context<Self>) {
        if self.state.focused == PanelId::Main {
            let rows = self.main_len();
            // `ctrl-d` / `ctrl-u` move half a *measured* viewport, never a hardcoded ten rows.
            let page = self.state.cursors.main.page_rows();
            if let Some(staging) = &mut self.state.staging {
                let cursor = staging.cursor;
                let next = match motion {
                    ListMotion::Down => (cursor + 1).min(rows.saturating_sub(1)),
                    ListMotion::Up => cursor.saturating_sub(1),
                    ListMotion::First => 0,
                    ListMotion::Last => rows.saturating_sub(1),
                    ListMotion::PageDown => (cursor + page).min(rows.saturating_sub(1)),
                    ListMotion::PageUp => cursor.saturating_sub(page),
                };
                staging.cursor = next;
                self.state.cursors.main.set_len(rows);
                self.state.cursors.main.set(next);
            } else {
                self.state.cursors.main.set_len(rows);
                self.state.cursors.main.motion(motion);
            }
            let moving_down = motion.moves_down();
            ListView::reveal(&self.scroll_main, &self.state.cursors.main, moving_down);
            self.request_main_selection();
            cx.notify();
            return;
        }
        let previous = self.state.focused_cursor().index();
        let moving_down = self.state.focused_cursor_mut().motion(motion);
        if self.state.focused_cursor().index() == previous {
            return;
        }
        let cursor = *self.state.focused_cursor();
        ListView::reveal(self.scroll_handle(), &cursor, moving_down);
        self.state.remember_selection();
        self.refresh_main();
        cx.notify();
    }

    pub(super) fn move_down(&mut self, _: &list::MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::Down, cx);
    }

    pub(super) fn move_up(&mut self, _: &list::MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::Up, cx);
    }

    pub(super) fn go_top(&mut self, _: &list::GoTop, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::First, cx);
    }

    pub(super) fn go_bottom(&mut self, _: &list::GoBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::Last, cx);
    }

    pub(super) fn page_down(&mut self, _: &list::PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::PageDown, cx);
    }

    pub(super) fn page_up(&mut self, _: &list::PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(ListMotion::PageUp, cx);
    }

    pub(super) fn scroll_left(
        &mut self,
        _: &list::ScrollLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slot = self.focused_pan_slot();
        self.pan_diff_slot(slot, f32::from(cx.theme().metrics.diff_horizontal_step), cx);
    }

    fn focused_pan_slot(&self) -> &'static str {
        if matches!(
            self.state.main,
            MainContent::SubCommits { .. } | MainContent::CommitFiles { .. }
        ) {
            SLOT_PATCH
        } else {
            SLOT_MAIN
        }
    }

    /// `L`: pan right, clamped to the widest line, so panning never reaches blank rows.
    pub(super) fn scroll_right(
        &mut self,
        _: &list::ScrollRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slot = self.focused_pan_slot();
        self.pan_diff_slot(
            slot,
            -f32::from(cx.theme().metrics.diff_horizontal_step),
            cx,
        );
    }

    /// Pans the diff payload sideways by a wheel delta, clamped to the longest line.
    ///
    /// `uniform_list` only ever declares `overflow.y = Scroll`, so a horizontal delta — a
    /// two-finger sideways swipe, or `shift` plus a wheel on a platform that swaps the axis —
    /// reaches the list and is dropped. Handling it here is purely additive: nothing else in the
    /// chain consumes `delta.x`, so there is no double-scroll to guard against.
    pub(crate) fn pan_diff_slot(
        &mut self,
        slot: &'static str,
        delta_x: f32,
        cx: &mut Context<Self>,
    ) {
        if delta_x == 0.0 {
            return;
        }
        let limit = crate::views::diff::max_h_scroll(&self.pan_model(slot), self.main_px_w);
        let next = (self.state.main_h_scroll - delta_x).clamp(0.0, limit);
        if next != self.state.main_h_scroll {
            self.state.main_h_scroll = next;
            cx.notify();
        }
    }

    /// `|`: switch the diff between unified and side by side.
    ///
    /// Staging mode stays unified whatever this says (see [`Self::main_mode`]); the toggle still
    /// takes effect, it just does not apply until staging is left.
    pub(super) fn toggle_split(
        &mut self,
        _: &diff_actions::ToggleSplit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.diff_mode = self.state.diff_mode.toggled();
        self.state.main_h_scroll = 0.0;
        self.sync_main_len();
        cx.notify();
    }

    /// `{` / `}`: narrow or widen the context git prints around every hunk.
    ///
    /// The width lives on the repository, so the patch a staging selection builds is read back
    /// with the same `-U` it was displayed with.
    pub(super) fn set_context(&mut self, context: u32, cx: &mut Context<Self>) {
        let context = context.min(fleet_git::MAX_DIFF_CONTEXT);
        if context == self.state.diff_context {
            return;
        }
        self.state.diff_context = context;
        let drill_down = match &mut self.state.main {
            MainContent::CommitDiff { diff, .. } => {
                *diff = None;
                None
            }
            MainContent::SubCommits {
                shown: Some(oid),
                diff,
                ..
            } => {
                *diff = None;
                Some(vec![GitRequest::CommitDiff(oid.clone())])
            }
            MainContent::CommitFiles {
                oid,
                shown,
                whole,
                diff,
                ..
            } => {
                *whole = None;
                *diff = None;
                let mut requests = vec![GitRequest::CommitDiff(oid.clone())];
                if let Some(path) = shown {
                    requests.push(GitRequest::CommitFileDiff {
                        oid: oid.clone(),
                        path: path.clone(),
                    });
                }
                Some(requests)
            }
            _ => None,
        };
        self.send(GitRequest::SetDiffContext(context));
        if let Some(requests) = drill_down {
            for request in requests {
                self.send(request);
            }
        } else {
            self.refresh_main();
        }
        self.toast(format!("Diff context: {context}"), Icon::FileDiff);
        cx.notify();
    }

    pub(super) fn less_context(
        &mut self,
        _: &diff_actions::LessContext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.state.diff_context.saturating_sub(1);
        self.set_context(context, cx);
    }

    pub(super) fn more_context(
        &mut self,
        _: &diff_actions::MoreContext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.state.diff_context.saturating_add(1);
        self.set_context(context, cx);
    }
}
