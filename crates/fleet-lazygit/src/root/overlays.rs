use super::*;

impl Lazygit {
    pub(super) fn toast(&mut self, text: impl Into<gpui::SharedString>, icon: Icon) {
        self.state
            .toast(Toast::new(text).icon(icon), Instant::now(), TOAST_DWELL);
    }

    pub(super) fn open_confirm(&mut self, confirm: Confirm) {
        self.state.push_overlay(Overlay::Confirm(confirm));
    }

    pub(super) fn open_prompt(&mut self, prompt: Prompt) {
        self.state.push_overlay(Overlay::Prompt(prompt));
    }

    pub(super) fn open_menu(&mut self, menu: Menu) {
        self.state.push_overlay(Overlay::Menu(menu));
    }

    pub(super) fn open_help(
        &mut self,
        _: &global::OpenHelp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.push_overlay(Overlay::Help { top: 0 });
        cx.notify();
    }

    pub(super) fn confirm_accept(
        &mut self,
        _: &lg_confirm::Accept,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Overlay::Confirm(confirm)) = self.state.overlays.pop() else {
            return;
        };
        match confirm.outcome {
            ConfirmOutcome::Request(request) => match *request {
                GitRequest::Shutdown => {
                    self.bridge.send(GitRequest::Shutdown);
                    cx.emit(LazygitEvent::Quit);
                }
                request => self.send(request),
            },
            ConfirmOutcome::RequestOrEscalate { request, escalate } => {
                if let GitRequest::Mutate { label, .. } = request.as_ref() {
                    self.state.escalation = Some((label.clone(), escalate));
                }
                self.send(*request);
            }
        }
        cx.notify();
    }

    pub(super) fn confirm_cancel(
        &mut self,
        _: &lg_confirm::Cancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.pop_overlay();
        cx.notify();
    }

    pub(super) fn prompt_accept(
        &mut self,
        _: &prompt::Accept,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let multiline = matches!(self.state.overlay(), Some(Overlay::Prompt(prompt)) if prompt.buffer.is_multiline());
        if multiline {
            if let Some(Overlay::Prompt(prompt)) = self.state.overlay_mut() {
                prompt.buffer.insert("\n");
            }
            cx.notify();
            return;
        }
        self.submit_prompt(cx);
    }

    pub(super) fn prompt_submit(
        &mut self,
        _: &prompt::Submit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_prompt(cx);
    }

    pub(super) fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Prompt(prompt)) = self.state.overlays.pop() else {
            return;
        };
        let value = prompt.buffer.value().trim().to_owned();
        match prompt.kind {
            PromptKind::Commit { amend } => {
                if value.is_empty() {
                    self.toast("A commit needs a message.", Icon::TriangleAlert);
                } else {
                    self.mutate(
                        "commit",
                        Mutation::Commit {
                            message: value,
                            options: CommitOptions {
                                amend,
                                ..CommitOptions::default()
                            },
                        },
                    );
                }
            }
            PromptKind::NewBranch { start_point } => {
                if !value.is_empty() {
                    self.mutate(
                        "new branch",
                        Mutation::CheckoutNewBranch {
                            name: value,
                            start_point: start_point.map(Ref::from),
                        },
                    );
                }
            }
            PromptKind::RenameBranch(old) => {
                if !value.is_empty() && value != old {
                    self.mutate("rename branch", Mutation::RenameBranch { old, new: value });
                }
            }
            PromptKind::NewTag(target) => {
                if !value.is_empty() {
                    self.mutate(
                        "new tag",
                        Mutation::CreateTag {
                            name: value,
                            target: Ref::from(target),
                            message: None,
                        },
                    );
                }
            }
            PromptKind::Reword(oid) => {
                if !value.is_empty() {
                    self.mutate(
                        "reword",
                        Mutation::Reword {
                            oid,
                            message: value,
                        },
                    );
                }
            }
            PromptKind::SetUpstream(branch) => match value.split_once('/') {
                Some((remote, remote_branch)) => self.mutate(
                    "set upstream",
                    Mutation::SetUpstream {
                        branch,
                        remote: remote.to_owned(),
                        remote_branch: remote_branch.to_owned(),
                    },
                ),
                None => self.toast("Type `remote/branch`.", Icon::TriangleAlert),
            },
            PromptKind::Stash(options) => {
                let message = (!value.is_empty()).then_some(value);
                self.mutate(
                    "stash",
                    Mutation::StashPush(StashOptions { message, ..options }),
                );
            }
            PromptKind::BranchFromStash(index) => {
                if !value.is_empty() {
                    self.mutate("stash branch", Mutation::StashBranch { name: value, index });
                }
            }
            PromptKind::PushSetUpstream => {
                let branch = self.state.head_branch().map(str::to_owned);
                if let Some(branch) = branch {
                    self.mutate(
                        "push",
                        Mutation::Push(PushRequest {
                            remote: Some(if value.is_empty() {
                                "origin".to_owned()
                            } else {
                                value
                            }),
                            branch: Some(branch),
                            set_upstream: true,
                            ..PushRequest::default()
                        }),
                    );
                }
            }
        }
        cx.notify();
    }

    pub(super) fn prompt_cancel(
        &mut self,
        _: &prompt::Cancel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.pop_overlay();
        cx.notify();
    }

    pub(super) fn with_buffer(&mut self, edit: impl FnOnce(&mut crate::state::Buffer)) {
        match self.state.overlay_mut() {
            Some(Overlay::Prompt(prompt)) => edit(&mut prompt.buffer),
            Some(Overlay::Menu(menu)) => {
                if let Some(filter) = &mut menu.filter {
                    edit(filter);
                }
                menu.cursor = 0;
            }
            _ => {}
        }
    }

    pub(super) fn prompt_backspace(
        &mut self,
        _: &prompt::Backspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_buffer(|buffer| {
            buffer.backspace();
        });
        cx.notify();
    }

    pub(super) fn prompt_delete_word(
        &mut self,
        _: &prompt::DeleteWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_buffer(|buffer| {
            buffer.delete_word();
        });
        cx.notify();
    }

    pub(super) fn prompt_delete_to_start(
        &mut self,
        _: &prompt::DeleteToStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_buffer(|buffer| {
            buffer.delete_to_line_start();
        });
        cx.notify();
    }

    pub(super) fn prompt_left(&mut self, _: &prompt::Left, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::left);
        cx.notify();
    }

    pub(super) fn prompt_right(
        &mut self,
        _: &prompt::Right,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_buffer(crate::state::Buffer::right);
        cx.notify();
    }

    pub(super) fn prompt_home(&mut self, _: &prompt::Home, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::home);
        cx.notify();
    }

    pub(super) fn prompt_end(&mut self, _: &prompt::End, _: &mut Window, cx: &mut Context<Self>) {
        self.with_buffer(crate::state::Buffer::end);
        cx.notify();
    }

    pub(super) fn prompt_paste(
        &mut self,
        _: &prompt::Paste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        if !text.is_empty() {
            self.with_buffer(|buffer| buffer.insert(&text));
        }
        cx.notify();
    }

    pub(super) fn menu_accept(&mut self, _: &menu::Accept, _: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Menu(menu)) = self.state.overlay() else {
            return;
        };
        let Some(action) = menu.selected().map(|item| item.action.clone()) else {
            return;
        };
        self.run_menu_action(action, cx);
    }

    /// Closes the menu and performs one row's action.
    pub(super) fn run_menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        self.state.pop_overlay();
        match action {
            MenuAction::Request(request) => self.send(*request),
            MenuAction::Confirm(confirm) => self.open_confirm(*confirm),
            MenuAction::Prompt(prompt) => self.open_prompt(*prompt),
        }
        cx.notify();
    }

    pub(super) fn menu_cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut()
            && menu.filter.is_some()
        {
            menu.filter = None;
            menu.cursor = 0;
            cx.notify();
            return;
        }
        self.state.pop_overlay();
        cx.notify();
    }

    pub(super) fn menu_down(&mut self, _: &menu::Down, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut() {
            let len = menu.visible().len();
            if len > 0 {
                menu.cursor = (menu.cursor + 1) % len;
            }
        }
        cx.notify();
    }

    pub(super) fn menu_up(&mut self, _: &menu::Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut() {
            let len = menu.visible().len();
            if len > 0 {
                menu.cursor = (menu.cursor + len - 1) % len;
            }
        }
        cx.notify();
    }

    pub(super) fn menu_start_filter(
        &mut self,
        _: &menu::StartFilter,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(Overlay::Menu(menu)) = self.state.overlay_mut() {
            menu.filter = Some(crate::state::Buffer::single_line());
            menu.cursor = 0;
        }
        cx.notify();
    }

    pub(super) fn help_close(
        &mut self,
        _: &lg_help::Close,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.pop_overlay();
        cx.notify();
    }

    pub(super) fn help_down(&mut self, _: &lg_help::Down, _: &mut Window, cx: &mut Context<Self>) {
        // The last row is the last thing the overlay scrolls to; past it there is nothing to see.
        let last = crate::overlays::help_last_top(self);
        if let Some(Overlay::Help { top }) = self.state.overlay_mut() {
            *top = (*top + 1).min(last);
        }
        cx.notify();
    }

    pub(super) fn help_up(&mut self, _: &lg_help::Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Help { top }) = self.state.overlay_mut() {
            *top = top.saturating_sub(1);
        }
        cx.notify();
    }

    /// Types a printable character into the focused buffer.
    ///
    /// gpui dispatches key **bindings** before `on_key_down`, and no text context binds a
    /// single-character key, so exactly the printable set reaches here.
    pub(super) fn typed(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        let Some(text) = event.keystroke.key_char.as_deref() else {
            return;
        };
        if text.is_empty() || text.chars().any(char::is_control) {
            return;
        }
        // lazygit's menus dispatch on the letter printed beside each row. It only applies while
        // no filter prompt is open: a printable key then belongs to the filter.
        if let Some(Overlay::Menu(menu)) = self.state.overlay()
            && menu.filter.is_none()
            && let Some(action) = menu.item_for_key(text).map(|item| item.action.clone())
        {
            self.run_menu_action(action, cx);
            return;
        }
        let typing = matches!(
            self.state.overlay(),
            Some(Overlay::Prompt(_))
                | Some(Overlay::Menu(Menu {
                    filter: Some(_),
                    ..
                }))
        );
        if !typing {
            return;
        }
        let text = text.to_owned();
        self.with_buffer(|buffer| buffer.insert(&text));
        cx.notify();
    }
}
