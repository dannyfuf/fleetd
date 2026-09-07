use super::*;
use gpui::EntityInputHandler;

enum PromptEdit<'a> {
    Backspace,
    DeleteWord,
    DeleteToStart,
    Left,
    Right,
    Home,
    End,
    Insert(&'a str),
}

impl Lazygit {
    pub(super) fn toast(&mut self, text: impl Into<gpui::SharedString>, icon: Icon) {
        self.state
            .toast(Toast::new(text).icon(icon), Instant::now(), TOAST_DWELL);
    }

    pub(super) fn open_confirm(&mut self, confirm: Confirm) {
        self.state.push_overlay(Overlay::Confirm(confirm));
    }

    pub(super) fn open_prompt(
        &mut self,
        prompt: Prompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_prompt = None;
        self.prompt_input = (!prompt.buffer.is_multiline()).then(|| {
            cx.new(|cx| {
                TextInput::new(cx)
                    .with_mono(true)
                    .with_text(prompt.buffer.value())
            })
        });
        self.state.push_overlay(Overlay::Prompt(prompt));
        window.focus(&self.wanted_focus(cx), cx);
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
        self.help_context = Some(self.state.context_chain());
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
                let mut request = *request;
                if let GitRequest::Mutate { label, .. } = &mut request {
                    let tracked = self.tracked_label(label);
                    *label = tracked.clone();
                    self.state.escalation = Some((tracked, escalate));
                }
                self.send(request);
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
        window: &mut Window,
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
        self.submit_prompt(window, cx);
    }

    pub(super) fn prompt_submit(
        &mut self,
        _: &prompt::Submit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_prompt(window, cx);
    }

    pub(super) fn submit_prompt(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Prompt(prompt)) = self.state.overlay().cloned() else {
            return;
        };
        if self.pending_prompt.is_some() {
            return;
        }
        let value = if let Some(input) = &self.prompt_input {
            input.read(cx).text().trim().to_owned()
        } else {
            prompt.buffer.value().trim().to_owned()
        };
        match prompt.kind {
            PromptKind::Commit { amend } => {
                if value.is_empty() {
                    self.toast("A commit needs a message.", Icon::TriangleAlert);
                } else {
                    self.submit_prompt_mutation(
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
                    self.submit_prompt_mutation(
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
                    self.submit_prompt_mutation(
                        "rename branch",
                        Mutation::RenameBranch { old, new: value },
                    );
                }
            }
            PromptKind::NewTag(target) => {
                if !value.is_empty() {
                    self.submit_prompt_mutation(
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
                    self.submit_prompt_mutation(
                        "reword",
                        Mutation::Reword {
                            oid,
                            message: value,
                        },
                    );
                }
            }
            PromptKind::SetUpstream(branch) => match value.split_once('/') {
                Some((remote, remote_branch)) => self.submit_prompt_mutation(
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
                self.submit_prompt_mutation(
                    "stash",
                    Mutation::StashPush(StashOptions { message, ..options }),
                );
            }
            PromptKind::BranchFromStash(index) => {
                if !value.is_empty() {
                    self.submit_prompt_mutation(
                        "stash branch",
                        Mutation::StashBranch { name: value, index },
                    );
                }
            }
            PromptKind::PushSetUpstream => {
                let branch = self.state.head_branch().map(str::to_owned);
                if let Some(branch) = branch {
                    self.submit_prompt_mutation(
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

    fn submit_prompt_mutation(&mut self, label: &str, mutation: Mutation) {
        let tracked = self.tracked_label(label);
        self.pending_prompt = Some(tracked.clone());
        self.send(GitRequest::Mutate {
            label: tracked,
            mutation: Box::new(mutation),
        });
    }

    pub(super) fn prompt_cancel(
        &mut self,
        _: &prompt::Cancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.pop_overlay();
        self.pending_prompt = None;
        self.prompt_input = None;
        if self.active {
            window.focus(&self.focus, cx);
        }
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

    fn edit_prompt(&mut self, edit: PromptEdit<'_>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.prompt_input.clone() {
            input.update(cx, |input, cx| {
                let mut state = input.state().clone();
                match edit {
                    PromptEdit::Backspace => {
                        state.backspace();
                    }
                    PromptEdit::DeleteWord => {
                        state.delete_word_before();
                    }
                    PromptEdit::DeleteToStart => {
                        state.delete_to_start();
                    }
                    PromptEdit::Left => {
                        state.move_left();
                    }
                    PromptEdit::Right => {
                        state.move_right();
                    }
                    PromptEdit::Home => {
                        state.move_to_start();
                    }
                    PromptEdit::End => {
                        state.move_to_end();
                    }
                    PromptEdit::Insert(text) => state.insert(text),
                }
                let caret = state.offset_to_utf16(state.cursor());
                input.set_text(state.text().to_owned(), cx);
                input.set_selected_text_range(caret..caret, window, cx);
            });
            return;
        }
        self.with_buffer(|buffer| match edit {
            PromptEdit::Backspace => {
                buffer.backspace();
            }
            PromptEdit::DeleteWord => {
                buffer.delete_word();
            }
            PromptEdit::DeleteToStart => {
                buffer.delete_to_line_start();
            }
            PromptEdit::Left => buffer.left(),
            PromptEdit::Right => buffer.right(),
            PromptEdit::Home => buffer.home(),
            PromptEdit::End => buffer.end(),
            PromptEdit::Insert(text) => buffer.insert(text),
        });
    }

    pub(super) fn prompt_backspace(
        &mut self,
        _: &prompt::Backspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::Backspace, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_delete_word(
        &mut self,
        _: &prompt::DeleteWord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::DeleteWord, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_delete_to_start(
        &mut self,
        _: &prompt::DeleteToStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::DeleteToStart, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_left(
        &mut self,
        _: &prompt::Left,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::Left, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_right(
        &mut self,
        _: &prompt::Right,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::Right, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_home(
        &mut self,
        _: &prompt::Home,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::Home, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_end(
        &mut self,
        _: &prompt::End,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_prompt(PromptEdit::End, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_paste(
        &mut self,
        _: &prompt::Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        if !text.is_empty() {
            self.edit_prompt(PromptEdit::Insert(&text), window, cx);
        }
        cx.notify();
    }

    pub(super) fn menu_accept(
        &mut self,
        _: &menu::Accept,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Overlay::Menu(menu)) = self.state.overlay() else {
            return;
        };
        let Some(action) = menu.selected().map(|item| item.action.clone()) else {
            return;
        };
        self.run_menu_action(action, window, cx);
    }

    /// Closes the menu and performs one row's action.
    pub(super) fn run_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.pop_overlay();
        match action {
            MenuAction::Request(request) => self.send(*request),
            MenuAction::Confirm(confirm) => self.open_confirm(*confirm),
            MenuAction::Prompt(prompt) => self.open_prompt(*prompt, window, cx),
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
        self.help_context = None;
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
    pub(super) fn typed(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.run_menu_action(action, window, cx);
            return;
        }
        let typing = matches!(
            self.state.overlay(),
            Some(Overlay::Prompt(prompt)) if prompt.buffer.is_multiline()
        ) || matches!(
            self.state.overlay(),
            Some(Overlay::Menu(Menu {
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
