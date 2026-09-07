use super::*;

impl Lazygit {
    pub(super) fn selected_oid(&self) -> Option<fleet_git::ObjectId> {
        // While the sub-commits view owns the keyboard, `c` / `v` / `g` act on *its* cursor.
        if self.state.focused == PanelId::Main
            && let MainContent::SubCommits { commits, .. } = &self.state.main
        {
            return commits
                .get(self.state.cursors.main.index())
                .map(|commit| commit.oid.clone());
        }
        match self.state.commit_tab {
            CommitTab::Commits => self.state.selected_commit().map(|c| c.oid.clone()),
            CommitTab::Reflog => self.state.selected_reflog().map(|e| e.oid.clone()),
        }
    }

    pub(super) fn enter_commit(
        &mut self,
        _: &commits::Enter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        let subject = match self.state.commit_tab {
            CommitTab::Commits => self
                .state
                .selected_commit()
                .map(|commit| commit.subject.clone()),
            CommitTab::Reflog => self
                .state
                .selected_reflog()
                .map(|entry| entry.subject.clone()),
        }
        .unwrap_or_default();
        let from = if self.state.focused == PanelId::Main {
            self.state.previous_panel
        } else {
            self.state.focused
        };
        self.open_commit_files(oid, subject, from, cx);
    }

    /// Opens lazygit's commit-files view: a header row plus the commit's changed files, over the
    /// patch of whichever row is selected. The header row keeps the whole-commit patch reachable.
    pub(super) fn open_commit_files(
        &mut self,
        oid: fleet_git::ObjectId,
        subject: String,
        from: PanelId,
        cx: &mut Context<Self>,
    ) {
        // The commit's diff and file list are usually already in hand: the side panel's selection
        // read them when the cursor landed on the commit.
        let (files, whole) = match &self.state.main {
            MainContent::CommitDiff {
                oid: current,
                diff,
                files,
                ..
            } if *current == oid => (files.clone(), diff.clone()),
            _ => (Arc::default(), None),
        };
        let need_read = whole.is_none() || files.is_empty();
        self.state.previous_panel = from;
        self.state.focused = PanelId::Main;
        self.state.staging = None;
        self.state.main = MainContent::CommitFiles {
            oid: oid.clone(),
            subject,
            files,
            whole: whole.clone(),
            shown: None,
            diff: whole,
            whole_error: None,
            diff_error: None,
        };
        self.state.main_h_scroll = 0.0;
        self.sync_main_len();
        self.state.cursors.main.set(0);
        if need_read {
            self.send(GitRequest::CommitDiff(oid));
        }
        cx.notify();
    }

    /// `enter` / `space` in the commit-files view: re-read the selected row's patch.
    ///
    /// The lower half already follows the cursor, so this only has to force the read the cursor
    /// move skipped — which is what makes `enter` on the header row a way back to the whole patch.
    pub(super) fn commit_file_show_patch(
        &mut self,
        _: &commitfiles::ShowPatch,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.state.cursors.main.index();
        let request = match &mut self.state.main {
            MainContent::CommitFiles {
                oid,
                files,
                shown,
                whole_error,
                diff_error,
                ..
            } => match index.checked_sub(1).and_then(|row| files.get(row)) {
                Some(file) => {
                    *shown = Some(file.path.clone());
                    *diff_error = None;
                    Some(GitRequest::CommitFileDiff {
                        oid: oid.clone(),
                        path: file.path.clone(),
                    })
                }
                None => {
                    *shown = None;
                    *whole_error = None;
                    Some(GitRequest::CommitDiff(oid.clone()))
                }
            },
            _ => None,
        };
        if let Some(request) = request {
            self.send(request);
        }
        cx.notify();
    }

    /// Loads whatever the main panel's cursor now points at, for the views whose lower half
    /// follows the selection. A no-op everywhere else.
    pub(super) fn request_main_selection(&mut self) {
        let index = self.state.cursors.main.index();
        let request = match &mut self.state.main {
            MainContent::CommitFiles {
                oid,
                files,
                whole,
                shown,
                diff,
                diff_error,
                ..
            } => {
                // Row 0 is the commit header: it shows the whole patch, every later row one file.
                match index.checked_sub(1).and_then(|row| files.get(row)) {
                    Some(file) => {
                        let path = file.path.clone();
                        if shown.as_deref() == Some(path.as_path()) {
                            None
                        } else {
                            *shown = Some(path.clone());
                            *diff = None;
                            *diff_error = None;
                            Some(GitRequest::CommitFileDiff {
                                oid: oid.clone(),
                                path,
                            })
                        }
                    }
                    None => {
                        *shown = None;
                        *diff = whole.clone();
                        *diff_error = None;
                        None
                    }
                }
            }
            _ => None,
        };
        if let Some(request) = request {
            self.send(request);
        }
    }

    pub(super) fn checkout_commit(
        &mut self,
        _: &commits::Checkout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Checkout commit".to_owned(),
            target: crate::state::short_oid(&oid),
            facts: vec!["HEAD becomes detached.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "checkout".to_owned(),
                mutation: Box::new(Mutation::Checkout(Ref::from(oid.0))),
            })),
        });
        cx.notify();
    }

    pub(super) fn reword_commit(
        &mut self,
        _: &commits::Reword,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_prompt(
            Prompt {
                title: "Reword commit".to_owned(),
                subtitle: Some("⌘⏎ rewords, ⏎ starts a new line".to_owned()),
                buffer: crate::state::Buffer::multi_line().with_text(commit_message(&commit)),
                kind: PromptKind::Reword(commit.oid),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn squash_commit(
        &mut self,
        _: &commits::Squash,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Squash".to_owned(),
            target: format!(
                "{} {}",
                crate::state::short_oid(&commit.oid),
                commit.subject
            ),
            facts: vec!["Folds the commit into the one below it via a rebase.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "squash".to_owned(),
                mutation: Box::new(Mutation::Squash(commit.oid)),
            })),
        });
        cx.notify();
    }

    pub(super) fn fixup_commit(
        &mut self,
        _: &commits::Fixup,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Fixup".to_owned(),
            target: format!(
                "{} {}",
                crate::state::short_oid(&commit.oid),
                commit.subject
            ),
            facts: vec!["Melds the commit into the one below it and drops its message.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "fixup".to_owned(),
                mutation: Box::new(Mutation::Fixup(commit.oid)),
            })),
        });
        cx.notify();
    }

    pub(super) fn drop_commit(
        &mut self,
        _: &commits::Drop,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Drop commit".to_owned(),
            target: format!(
                "{} {}",
                crate::state::short_oid(&commit.oid),
                commit.subject
            ),
            facts: vec!["The commit is removed from the branch via a rebase.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "drop".to_owned(),
                mutation: Box::new(Mutation::DropCommit(commit.oid)),
            })),
        });
        cx.notify();
    }

    pub(super) fn edit_commit(
        &mut self,
        _: &commits::Edit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.mutate("edit commit", Mutation::EditCommit(commit.oid));
        cx.notify();
    }

    pub(super) fn move_commit_down(
        &mut self,
        _: &commits::MoveDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.mutate(
            "move commit down",
            Mutation::MoveCommit {
                oid: commit.oid,
                direction: MoveDirection::Down,
            },
        );
        cx.notify();
    }

    pub(super) fn move_commit_up(
        &mut self,
        _: &commits::MoveUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(commit) = self.state.selected_commit().cloned() else {
            return;
        };
        self.mutate(
            "move commit up",
            Mutation::MoveCommit {
                oid: commit.oid,
                direction: MoveDirection::Up,
            },
        );
        cx.notify();
    }

    pub(super) fn reset_menu(
        &mut self,
        _: &commits::ResetMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        let short = crate::state::short_oid(&oid);
        let item = |key: &str, label: &str, mode: ResetMode, danger: bool| MenuItem {
            key: key.to_owned(),
            label: label.to_owned(),
            action: MenuAction::Confirm(Box::new(Confirm {
                title: format!("Reset {label}"),
                target: short.clone(),
                facts: vec![
                    match mode {
                        ResetMode::Soft => "Moves HEAD; index and worktree keep their content.",
                        ResetMode::Mixed => {
                            "Moves HEAD and resets the index; the worktree is kept."
                        }
                        ResetMode::Hard => "Moves HEAD and discards the index and the worktree.",
                    }
                    .to_owned(),
                ],
                danger,
                outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                    label: "reset".to_owned(),
                    mutation: Box::new(Mutation::Reset {
                        to: Ref::from(oid.0.clone()),
                        mode,
                    }),
                })),
            })),
        };
        self.open_menu(Menu::new(
            "Reset options",
            vec![
                item("s", "soft", ResetMode::Soft, false),
                item("m", "mixed", ResetMode::Mixed, false),
                item("h", "hard", ResetMode::Hard, true),
            ],
        ));
        cx.notify();
    }

    pub(super) fn copy_commit(
        &mut self,
        _: &commits::Copy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        if let Some(index) = self.state.copied.iter().position(|copied| *copied == oid) {
            Arc::make_mut(&mut self.state.copied).remove(index);
        } else {
            Arc::make_mut(&mut self.state.copied).push(oid);
        }
        let count = self.state.copied.len();
        let noun = if count == 1 { "commit" } else { "commits" };
        self.toast(format!("{count} {noun} copied"), Icon::Check);
        cx.notify();
    }

    pub(super) fn paste_commits(
        &mut self,
        _: &commits::Paste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.copied.is_empty() {
            self.toast(
                "Nothing copied. Press `c` on a commit first.",
                Icon::CircleDot,
            );
            cx.notify();
            return;
        }
        let copied = self.state.copied.clone();
        self.open_confirm(Confirm {
            title: "Paste commits".to_owned(),
            target: copied
                .iter()
                .map(crate::state::short_oid)
                .collect::<Vec<_>>()
                .join(" "),
            facts: vec!["Cherry-picks the copied commits onto the current branch.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "cherry-pick".to_owned(),
                mutation: Box::new(Mutation::CherryPick(copied.as_ref().clone())),
            })),
        });
        cx.notify();
    }

    pub(super) fn revert_commit(
        &mut self,
        _: &commits::Revert,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Revert commit".to_owned(),
            target: crate::state::short_oid(&oid),
            facts: vec!["Creates a commit that applies the changes in reverse.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "revert".to_owned(),
                mutation: Box::new(Mutation::Revert(oid)),
            })),
        });
        cx.notify();
    }

    pub(super) fn tag_commit(
        &mut self,
        _: &commits::Tag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_prompt(
            Prompt {
                title: "Tag commit".to_owned(),
                subtitle: Some(format!("Tag name for {}", crate::state::short_oid(&oid))),
                buffer: crate::state::Buffer::single_line(),
                kind: PromptKind::NewTag(oid.0),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn amend_commit(
        &mut self,
        _: &commits::Amend,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_amend_confirm();
        cx.notify();
    }

    pub(super) fn branch_from_commit(
        &mut self,
        _: &commits::NewBranch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(oid) = self.selected_oid() else {
            return;
        };
        self.open_prompt(
            Prompt {
                title: "New branch".to_owned(),
                subtitle: Some(format!("Starting at {}", crate::state::short_oid(&oid))),
                buffer: crate::state::Buffer::single_line(),
                kind: PromptKind::NewBranch {
                    start_point: Some(oid.0),
                },
            },
            window,
            cx,
        );
        cx.notify();
    }
}

pub(super) fn commit_message(commit: &fleet_git::Commit) -> String {
    let body = commit.body.trim_end_matches('\n');
    if body.is_empty() {
        commit.subject.clone()
    } else {
        format!("{}\n\n{body}", commit.subject)
    }
}
