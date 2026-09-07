use super::*;

impl Lazygit {
    /// `space`: on a file, stage or unstage it; on a directory, every file under it.
    ///
    /// lazygit asks the *node* whether anything below it is unstaged, so a directory with one
    /// unstaged file stages the whole directory, and a fully staged one unstages it.
    pub(super) fn toggle_staged(
        &mut self,
        _: &files::ToggleStaged,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.state.file_row().cloned() else {
            return;
        };
        if row.children.is_empty() {
            return;
        }
        if row.unstaged {
            self.mutate("stage", Mutation::Stage(row.children));
        } else {
            self.mutate("unstage", Mutation::Unstage(row.children));
        }
        cx.notify();
    }

    pub(super) fn toggle_staged_all(
        &mut self,
        _: &files::ToggleStagedAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_unstaged = self.state.files().iter().any(has_unstaged);
        if any_unstaged {
            self.mutate("stage all", Mutation::StageAll);
        } else {
            self.mutate("unstage all", Mutation::UnstageAll);
        }
        cx.notify();
    }

    /// `d`: discard the row's changes — one file, or every file under a directory.
    pub(super) fn discard_file(
        &mut self,
        _: &files::Discard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.state.file_row().cloned() else {
            return;
        };
        if row.children.is_empty() {
            return;
        }
        let mut facts = vec!["The working-tree changes cannot be recovered.".to_owned()];
        if row.is_dir {
            facts.insert(
                0,
                format!(
                    "{} file{} under this directory.",
                    row.children.len(),
                    if row.children.len() == 1 { "" } else { "s" }
                ),
            );
        }
        self.open_confirm(Confirm {
            title: "Discard changes".to_owned(),
            target: row.path.display().to_string(),
            facts,
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "discard".to_owned(),
                mutation: Box::new(Mutation::Discard(row.children)),
            })),
        });
        cx.notify();
    }

    pub(super) fn commit(&mut self, _: &files::Commit, _: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(Prompt {
            title: "Commit".to_owned(),
            subtitle: Some("⌘⏎ commits, ⏎ starts a new line".to_owned()),
            buffer: crate::state::Buffer::multi_line(),
            kind: PromptKind::Commit { amend: false },
        });
        cx.notify();
    }

    pub(super) fn amend(&mut self, _: &files::Amend, _: &mut Window, cx: &mut Context<Self>) {
        self.open_amend_confirm();
        cx.notify();
    }

    pub(super) fn open_amend_confirm(&mut self) {
        self.open_confirm(Confirm {
            title: "Amend last commit".to_owned(),
            target: self
                .state
                .commits()
                .first()
                .map(|commit| commit.subject.clone())
                .unwrap_or_default(),
            facts: vec!["The staged changes are folded into the last commit.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "amend".to_owned(),
                mutation: Box::new(Mutation::Commit {
                    message: String::new(),
                    options: CommitOptions {
                        amend: true,
                        allow_empty: true,
                        ..CommitOptions::default()
                    },
                }),
            })),
        });
    }

    pub(super) fn stash_menu(
        &mut self,
        _: &files::StashMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = vec![
            MenuItem {
                key: "a".to_owned(),
                label: "stash all changes".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Stash all changes".to_owned(),
                    subtitle: Some("Message".to_owned()),
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::Stash(StashOptions::default()),
                })),
            },
            MenuItem {
                key: "s".to_owned(),
                label: "stash staged changes".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Stash staged changes".to_owned(),
                    subtitle: Some("Message".to_owned()),
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::Stash(StashOptions {
                        staged_only: true,
                        ..StashOptions::default()
                    }),
                })),
            },
            MenuItem {
                key: "u".to_owned(),
                label: "stash all, including untracked".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Stash including untracked".to_owned(),
                    subtitle: Some("Message".to_owned()),
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::Stash(StashOptions {
                        include_untracked: true,
                        ..StashOptions::default()
                    }),
                })),
            },
        ];
        self.open_menu(Menu::new("Stash options", items));
        cx.notify();
    }

    /// `` ` `` / `~` — the tree/flat switch.
    pub(super) fn toggle_file_tree(
        &mut self,
        _: &files::ToggleTree,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.toggle_file_tree_mode();
        self.refresh_main();
        cx.notify();
    }

    /// `-` — collapse every directory.
    pub(super) fn collapse_all_files(
        &mut self,
        _: &files::CollapseAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.collapse_all_files();
        self.refresh_main();
        cx.notify();
    }

    /// `=` — expand every directory.
    pub(super) fn expand_all_files(
        &mut self,
        _: &files::ExpandAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.expand_all_files();
        self.refresh_main();
        cx.notify();
    }

    /// `enter`: staging mode on a file, the conflict view on a conflicted one, and — lazygit's
    /// "Stage lines / Collapse directory" — a collapse toggle on a directory row.
    pub(super) fn enter_file(&mut self, _: &files::Enter, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.state.file_row().cloned()
            && row.is_dir
        {
            self.state.toggle_file_collapsed(&row.path);
            self.refresh_main();
            cx.notify();
            return;
        }
        let Some(file) = self.state.selected_file().cloned() else {
            return;
        };
        self.state.previous_panel = PanelId::Files;
        self.state.focused = PanelId::Main;
        if file.conflict.is_some() {
            self.state.staging = None;
            self.refresh_main();
        } else {
            let side = if has_unstaged(&file) {
                DiffSide::Unstaged
            } else {
                DiffSide::Staged
            };
            self.state.staging = Some(Staging {
                path: file.path.clone(),
                side,
                cursor: 0,
                anchor: None,
                line_mode: false,
                snapped: false,
            });
            self.sync_main_len();
            self.snap_to_change();
        }
        cx.notify();
    }

    /// Puts the staging cursor on the first added or removed line, unless it is on one already.
    ///
    /// lazygit snaps to a change line whenever it enters or re-modes the staging view; a context
    /// line is never a useful selection. It does **not** re-snap on a background refresh, which
    /// is what `Staging::snapped` guards.
    pub(super) fn snap_to_change(&mut self) {
        if self
            .state
            .staging
            .as_ref()
            .is_some_and(|staging| staging.snapped)
        {
            return;
        }
        let model = self.main_model();
        let rows = &model.rows;
        let already = self
            .state
            .staging
            .as_ref()
            .and_then(|staging| rows.get(staging.cursor))
            .is_some_and(crate::views::diff_model::DiffRow::is_change);
        if already {
            if let Some(staging) = &mut self.state.staging {
                staging.snapped = true;
            }
            return;
        }
        if let Some(index) = rows
            .iter()
            .position(crate::views::diff_model::DiffRow::is_change)
            && let Some(staging) = &mut self.state.staging
        {
            staging.cursor = index;
            staging.snapped = true;
            self.state.cursors.main.set_len(rows.len());
            self.state.cursors.main.set(index);
            ListView::reveal(&self.scroll_main, &self.state.cursors.main, true);
        }
    }
}
