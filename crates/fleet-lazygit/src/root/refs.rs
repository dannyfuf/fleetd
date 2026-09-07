use super::*;

impl Lazygit {
    pub(super) fn checkout_branch(
        &mut self,
        _: &branches::Checkout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        self.mutate("checkout", Mutation::Checkout(Ref::from(branch.name)));
        cx.notify();
    }

    pub(super) fn new_branch(
        &mut self,
        _: &branches::New,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_prompt(
            Prompt {
                title: "New branch".to_owned(),
                subtitle: Some("Branch name".to_owned()),
                buffer: crate::state::Buffer::single_line(),
                kind: PromptKind::NewBranch { start_point: None },
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn delete_branch(
        &mut self,
        _: &branches::Delete,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let name = branch.name.clone();
        let forced = Confirm {
            title: "Force delete branch".to_owned(),
            target: name.clone(),
            facts: vec!["The branch is not merged. Its commits become unreachable.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "delete branch".to_owned(),
                mutation: Box::new(Mutation::DeleteBranch {
                    name: name.clone(),
                    force: true,
                }),
            })),
        };
        self.open_confirm(Confirm {
            title: "Delete branch".to_owned(),
            target: name.clone(),
            facts: vec!["A branch that is not merged asks again before it is forced.".to_owned()],
            danger: false,
            // The safe `git branch -d` runs first; the force dialog appears only if Git refuses.
            outcome: ConfirmOutcome::RequestOrEscalate {
                request: Box::new(GitRequest::Mutate {
                    label: "delete branch".to_owned(),
                    mutation: Box::new(Mutation::DeleteBranch {
                        name: name.clone(),
                        force: false,
                    }),
                }),
                escalate: Box::new(forced),
            },
        });
        cx.notify();
    }

    pub(super) fn rename_branch(
        &mut self,
        _: &branches::Rename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        self.open_prompt(
            Prompt {
                title: "Rename branch".to_owned(),
                subtitle: Some(format!("Renaming `{}`", branch.name)),
                buffer: crate::state::Buffer::single_line().with_text(branch.name.clone()),
                kind: PromptKind::RenameBranch(branch.name),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn merge_branch(
        &mut self,
        _: &branches::Merge,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let head = self.state.head_branch().unwrap_or("HEAD").to_owned();
        self.open_confirm(Confirm {
            title: "Merge".to_owned(),
            target: format!("{} → {head}", branch.name),
            facts: vec!["A conflict pauses the merge; `m` then offers continue/abort.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "merge".to_owned(),
                mutation: Box::new(Mutation::Merge(Ref::from(branch.name))),
            })),
        });
        cx.notify();
    }

    pub(super) fn rebase_branch(
        &mut self,
        _: &branches::Rebase,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let head = self.state.head_branch().unwrap_or("HEAD").to_owned();
        self.open_confirm(Confirm {
            title: "Rebase".to_owned(),
            target: format!("{head} onto {}", branch.name),
            facts: vec!["Rewrites the checked-out branch's commits.".to_owned()],
            danger: false,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "rebase".to_owned(),
                mutation: Box::new(Mutation::RebaseOnto(Ref::from(branch.name))),
            })),
        });
        cx.notify();
    }

    pub(super) fn upstream_menu(
        &mut self,
        _: &branches::UpstreamMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let name = branch.name.clone();
        let items = vec![
            MenuItem {
                key: "s".to_owned(),
                label: "set upstream".to_owned(),
                action: MenuAction::Prompt(Box::new(Prompt {
                    title: "Set upstream".to_owned(),
                    subtitle: Some("remote/branch".to_owned()),
                    buffer: crate::state::Buffer::single_line().with_text(format!("origin/{name}")),
                    kind: PromptKind::SetUpstream(name.clone()),
                })),
            },
            MenuItem {
                key: "u".to_owned(),
                label: "unset upstream".to_owned(),
                action: MenuAction::Request(Box::new(GitRequest::Mutate {
                    label: "unset upstream".to_owned(),
                    mutation: Box::new(Mutation::UnsetUpstream(name.clone())),
                })),
            },
        ];
        self.open_menu(Menu::new("Upstream options", items));
        cx.notify();
    }

    pub(super) fn tag_branch(
        &mut self,
        _: &branches::Tag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .state
            .selected_branch()
            .map(|branch| branch.name.clone())
            .unwrap_or_else(|| "HEAD".to_owned());
        self.open_prompt(
            Prompt {
                title: "New tag".to_owned(),
                subtitle: Some(format!("Tag name for `{target}`")),
                buffer: crate::state::Buffer::single_line(),
                kind: PromptKind::NewTag(target),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn enter_branch(
        &mut self,
        _: &branches::Enter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = self.state.selected_branch().cloned() else {
            return;
        };
        let upstream = branch
            .upstream
            .as_ref()
            .map(|upstream| upstream.name.clone());
        self.open_sub_commits(branch.name, upstream, PanelId::Branches, cx);
    }

    /// Opens lazygit's sub-commits view on a ref: its log in the main panel, the selected
    /// commit's patch below it.
    pub(super) fn open_sub_commits(
        &mut self,
        reference: String,
        upstream: Option<String>,
        from: PanelId,
        cx: &mut Context<Self>,
    ) {
        self.state.previous_panel = from;
        self.state.focused = PanelId::Main;
        self.state.staging = None;
        self.state.main = MainContent::SubCommits {
            reference: reference.clone(),
            commits: Arc::default(),
            shown: None,
            diff: None,
            commits_error: None,
            diff_error: None,
        };
        self.state.cursors.main.set_len(0);
        self.state.main_h_scroll = 0.0;
        self.send(GitRequest::RefCommits {
            reference,
            upstream,
            limit: SUB_COMMIT_LIMIT,
        });
        cx.notify();
    }

    /// `enter` / `space` in the sub-commits view: read the selected commit's patch.
    pub(super) fn sub_commit_show_diff(
        &mut self,
        _: &subcommits::ShowDiff,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.state.cursors.main.index();
        let Some(oid) = (match &self.state.main {
            MainContent::SubCommits { commits, .. } => {
                commits.get(index).map(|commit| commit.oid.clone())
            }
            _ => None,
        }) else {
            return;
        };
        if let MainContent::SubCommits {
            shown,
            diff,
            diff_error,
            ..
        } = &mut self.state.main
        {
            *shown = Some(oid.clone());
            *diff = None;
            *diff_error = None;
        }
        self.send(GitRequest::CommitDiff(oid));
        cx.notify();
    }

    pub(super) fn enter_remote(
        &mut self,
        _: &remotes::Enter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.remote_drill.is_some() {
            let Some(branch) = self.state.selected_remote_branch().cloned() else {
                return;
            };
            // A remote branch is its own upstream, so every commit on it counts as pushed.
            let upstream = Some(branch.name.clone());
            self.open_sub_commits(branch.name, upstream, PanelId::Branches, cx);
            return;
        }
        if let Some(remote) = self.state.selected_remote().cloned() {
            self.state.remote_drill = Some(remote.name);
            self.state
                .cursors
                .remote_branches
                .set_len(self.state.remote_branches().len());
            self.state.cursors.remote_branches.set(0);
            self.refresh_main();
        }
        cx.notify();
    }

    pub(super) fn checkout_remote(
        &mut self,
        _: &remotes::Checkout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(remote) = self.state.remote_drill.clone() else {
            self.toast("Enter a remote first.", Icon::CircleDot);
            cx.notify();
            return;
        };
        let Some(branch) = self.state.selected_remote_branch().cloned() else {
            return;
        };
        self.mutate(
            "checkout remote branch",
            Mutation::CheckoutRemoteBranch {
                remote,
                name: branch.branch,
            },
        );
        cx.notify();
    }

    pub(super) fn new_tag(&mut self, _: &tags::New, window: &mut Window, cx: &mut Context<Self>) {
        self.open_prompt(
            Prompt {
                title: "New tag".to_owned(),
                subtitle: Some("Tag name for HEAD".to_owned()),
                buffer: crate::state::Buffer::single_line(),
                kind: PromptKind::NewTag("HEAD".to_owned()),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn delete_tag(&mut self, _: &tags::Delete, _: &mut Window, cx: &mut Context<Self>) {
        let Some(tag) = self.state.selected_tag().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Delete tag".to_owned(),
            target: tag.name.clone(),
            facts: vec!["The tag is removed locally only.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "delete tag".to_owned(),
                mutation: Box::new(Mutation::DeleteTag(tag.name)),
            })),
        });
        cx.notify();
    }

    pub(super) fn checkout_tag(
        &mut self,
        _: &tags::Checkout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tag) = self.state.selected_tag().cloned() else {
            return;
        };
        self.mutate("checkout tag", Mutation::Checkout(Ref::from(tag.name)));
        cx.notify();
    }

    pub(super) fn stash_apply(&mut self, _: &stash::Apply, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.mutate("stash apply", Mutation::StashApply(entry.index));
        cx.notify();
    }

    pub(super) fn stash_pop(&mut self, _: &stash::Pop, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.mutate("stash pop", Mutation::StashPop(entry.index));
        cx.notify();
    }

    pub(super) fn stash_drop(&mut self, _: &stash::Drop, _: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.open_confirm(Confirm {
            title: "Drop stash entry".to_owned(),
            target: format!("stash@{{{}}}: {}", entry.index, entry.subject),
            facts: vec!["The entry cannot be recovered.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "stash drop".to_owned(),
                mutation: Box::new(Mutation::StashDrop {
                    index: entry.index,
                    oid: entry.oid,
                }),
            })),
        });
        cx.notify();
    }

    pub(super) fn stash_branch(
        &mut self,
        _: &stash::NewBranch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.state.selected_stash().cloned() else {
            return;
        };
        self.open_prompt(
            Prompt {
                title: "Branch from stash".to_owned(),
                subtitle: Some(format!("Branch name for stash@{{{}}}", entry.index)),
                buffer: crate::state::Buffer::single_line(),
                kind: PromptKind::BranchFromStash(entry.index),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn enter_stash(&mut self, _: &stash::Enter, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_panel(PanelId::Main, cx);
    }
}
