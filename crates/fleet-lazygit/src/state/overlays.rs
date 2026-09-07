use super::*;
use unicode_segmentation::UnicodeSegmentation;

/// A single-line or multi-line text buffer with a grapheme-aware caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Buffer {
    value: String,
    /// Byte offset kept on an extended grapheme-cluster boundary.
    caret: usize,
    multiline: bool,
    lines: Arc<[gpui::SharedString]>,
    line_starts: Vec<usize>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            value: String::new(),
            caret: 0,
            multiline: false,
            lines: Arc::from([gpui::SharedString::default()]),
            line_starts: vec![0],
        }
    }
}

impl Buffer {
    /// An empty single-line buffer.
    #[must_use]
    pub(crate) fn single_line() -> Self {
        Self::default()
    }

    /// An empty buffer that accepts newlines.
    #[must_use]
    pub(crate) fn multi_line() -> Self {
        Self {
            multiline: true,
            ..Self::default()
        }
    }

    /// Pre-fills the buffer, caret at the end.
    #[must_use]
    pub(crate) fn with_text(mut self, text: impl Into<String>) -> Self {
        self.value = text.into();
        self.caret = self.value.len();
        self.rebuild_lines();
        self
    }

    /// The buffer's contents.
    #[must_use]
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    /// The caret, as a character index for the text-field renderer.
    #[must_use]
    pub(crate) fn caret(&self) -> usize {
        self.value[..self.caret].chars().count()
    }

    /// Whether the buffer accepts newlines.
    #[must_use]
    pub(crate) fn is_multiline(&self) -> bool {
        self.multiline
    }

    fn previous_boundary(&self) -> usize {
        self.value[..self.caret]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self) -> usize {
        self.value[self.caret..]
            .graphemes(true)
            .next()
            .map_or(self.caret, |grapheme| self.caret + grapheme.len())
    }

    /// Inserts text at the caret, dropping newlines in a single-line buffer.
    pub(crate) fn insert(&mut self, text: &str) {
        let filtered: String = text
            .chars()
            .filter(|character| {
                if *character == '\n' {
                    self.multiline
                } else {
                    !character.is_control()
                }
            })
            .collect();
        if filtered.is_empty() {
            return;
        }
        self.value.insert_str(self.caret, &filtered);
        self.caret += filtered.len();
        self.rebuild_lines();
    }

    /// Deletes the character before the caret. Returns whether anything changed.
    pub(crate) fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let start = self.previous_boundary();
        self.value.replace_range(start..self.caret, "");
        self.caret = start;
        self.rebuild_lines();
        true
    }

    /// Deletes the word before the caret (`ctrl-w`).
    pub(crate) fn delete_word(&mut self) -> bool {
        let end = self.caret;
        let prefix = &self.value[..end];
        let trimmed = prefix.trim_end_matches(char::is_whitespace);
        let start = trimmed.rfind(char::is_whitespace).map_or(0, |index| {
            index + trimmed[index..].chars().next().map_or(0, char::len_utf8)
        });
        self.delete_before(start, end)
    }

    /// Deletes from the start of the line to the caret (`ctrl-u`).
    pub(crate) fn delete_to_line_start(&mut self) -> bool {
        let end = self.caret;
        let start = self.value[..end].rfind('\n').map_or(0, |index| index + 1);
        self.delete_before(start, end)
    }

    fn delete_before(&mut self, start: usize, end: usize) -> bool {
        if start == end {
            return false;
        }
        self.value.replace_range(start..end, "");
        self.caret = start;
        self.rebuild_lines();
        true
    }

    /// Moves the caret one grapheme left.
    pub(crate) fn left(&mut self) {
        self.caret = self.previous_boundary();
    }

    /// Moves the caret one grapheme right.
    pub(crate) fn right(&mut self) {
        self.caret = self.next_boundary();
    }

    /// Moves the caret to the start of the buffer.
    pub(crate) fn home(&mut self) {
        self.caret = 0;
    }

    /// Moves the caret to the end of the buffer.
    pub(crate) fn end(&mut self) {
        self.caret = self.value.len();
    }

    /// The lines of the buffer plus the caret's (line, column), for rendering.
    #[must_use]
    pub(crate) fn lines_with_caret(&self) -> (Arc<[gpui::SharedString]>, usize, usize) {
        let line = self
            .line_starts
            .partition_point(|start| *start <= self.caret)
            .saturating_sub(1);
        (
            self.lines.clone(),
            line,
            self.value[self.line_starts[line]..self.caret]
                .chars()
                .count(),
        )
    }

    fn rebuild_lines(&mut self) {
        self.lines = self
            .value
            .split('\n')
            .map(gpui::SharedString::new)
            .collect();
        self.line_starts.clear();
        self.line_starts.push(0);
        for (index, character) in self.value.char_indices() {
            if character == '\n' {
                self.line_starts.push(index + 1);
            }
        }
    }
}

/// What submitting a prompt does.
#[derive(Clone, Debug)]
pub(crate) enum PromptKind {
    /// Commit the index with the typed message.
    Commit {
        /// Amend the previous commit instead of creating one.
        amend: bool,
    },
    /// Create and check out a branch with the typed name.
    NewBranch {
        /// Start point, when creating from a commit.
        start_point: Option<String>,
    },
    /// Rename the named branch.
    RenameBranch(String),
    /// Create a tag at the given target.
    NewTag(String),
    /// Reword a commit.
    Reword(ObjectId),
    /// Set the named branch's upstream from a `remote/branch` string.
    SetUpstream(String),
    /// Stash with the typed message.
    Stash(StashOptions),
    /// Create a branch from a stash entry.
    BranchFromStash(usize),
    /// Push with `--set-upstream` to the typed remote.
    PushSetUpstream,
}

/// A prompt overlay.
#[derive(Clone, Debug)]
pub(crate) struct Prompt {
    /// The dialog title.
    pub(crate) title: String,
    /// A one-line explanation under the title.
    pub(crate) subtitle: Option<String>,
    /// The editable buffer.
    pub(crate) buffer: Buffer,
    /// What confirming does.
    pub(crate) kind: PromptKind,
}

/// What confirming a confirmation does.
#[derive(Clone, Debug)]
pub(crate) enum ConfirmOutcome {
    /// Send this request.
    Request(Box<GitRequest>),
    /// Send this request, and open the stronger dialog only if it fails.
    ///
    /// This is lazygit's delete escalation: `git branch -d` runs first and the "force?" dialog
    /// appears only when Git refuses because the branch is not merged.
    RequestOrEscalate {
        /// The safe attempt. Must be a [`GitRequest::Mutate`], whose label keys the follow-up.
        request: Box<GitRequest>,
        /// The dialog that opens when the safe attempt fails.
        escalate: Box<Confirm>,
    },
}

/// A confirmation overlay.
#[derive(Clone, Debug)]
pub(crate) struct Confirm {
    /// The dialog title.
    pub(crate) title: String,
    /// The thing being acted on, shown on its own line.
    pub(crate) target: String,
    /// One line per consequence.
    pub(crate) facts: Vec<String>,
    /// Whether this is the destructive escalation.
    pub(crate) danger: bool,
    /// What confirming does.
    pub(crate) outcome: ConfirmOutcome,
}

/// What choosing a menu item does.
#[derive(Clone, Debug)]
pub(crate) enum MenuAction {
    /// Send this request.
    Request(Box<GitRequest>),
    /// Open a confirmation.
    Confirm(Box<Confirm>),
    /// Open a prompt.
    Prompt(Box<Prompt>),
}

/// One row of a menu.
#[derive(Clone, Debug)]
pub(crate) struct MenuItem {
    /// The shortcut shown on the right.
    pub(crate) key: String,
    /// The row label.
    pub(crate) label: String,
    /// What choosing it does.
    pub(crate) action: MenuAction,
}

/// A menu overlay: lazygit's option menus.
#[derive(Clone, Debug)]
pub(crate) struct Menu {
    /// The menu title.
    pub(crate) title: String,
    /// Every row, unfiltered.
    pub(crate) items: Vec<MenuItem>,
    /// The cursor into the *filtered* rows.
    pub(crate) cursor: usize,
    /// The filter buffer, present while `/` filtering is active.
    pub(crate) filter: Option<Buffer>,
}

impl Menu {
    /// A menu with a title and rows.
    #[must_use]
    pub(crate) fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            title: title.into(),
            items,
            cursor: 0,
            filter: None,
        }
    }

    /// The rows the filter admits, with their index into [`Menu::items`].
    #[must_use]
    pub(crate) fn visible(&self) -> Vec<(usize, &MenuItem)> {
        let query = self
            .filter
            .as_ref()
            .map(|buffer| buffer.value().to_lowercase())
            .unwrap_or_default();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                query.is_empty() || item.label.to_lowercase().contains(query.as_str())
            })
            .collect()
    }

    /// The selected row, if any.
    #[must_use]
    pub(crate) fn selected(&self) -> Option<&MenuItem> {
        let visible = self.visible();
        visible.get(self.cursor).map(|(_, item)| *item)
    }

    /// The row whose shortcut letter is `key`, as lazygit's menus dispatch them.
    ///
    /// The whole menu is searched, not just the filtered rows: the letter *is* the row's
    /// identity. Callers must only consult this while no filter prompt is open, because a
    /// printable key then belongs to the filter.
    #[must_use]
    pub(crate) fn item_for_key(&self, key: &str) -> Option<&MenuItem> {
        self.items.iter().find(|item| item.key == key)
    }
}

/// One entry of the overlay stack. The stack is LIFO; `<esc>` pops one layer.
#[derive(Clone, Debug)]
pub(crate) enum Overlay {
    /// A destructive confirmation.
    Confirm(Confirm),
    /// A text prompt.
    Prompt(Prompt),
    /// An option menu.
    Menu(Menu),
    /// The generated keybinding help.
    Help {
        /// Scroll offset in rows.
        top: usize,
    },
}

impl GitUiState {
    /// The top overlay, if any.
    #[must_use]
    pub(crate) fn overlay(&self) -> Option<&Overlay> {
        self.overlays.last()
    }

    /// The top overlay, mutably.
    pub(crate) fn overlay_mut(&mut self) -> Option<&mut Overlay> {
        self.overlays.last_mut()
    }

    /// The nested gpui key contexts, outermost first, **below** the root context.
    ///
    /// An overlay replaces the whole chain rather than adding to it, which is how lazygit's
    /// "a prompt swallows the keymap" rule is reproduced.
    #[must_use]
    pub(crate) fn context_chain(&self) -> Vec<&'static str> {
        if let Some(overlay) = self.overlay() {
            return match overlay {
                Overlay::Confirm(_) => vec!["LgDialog", "LgConfirm"],
                Overlay::Prompt(_) => vec!["LgDialog", "Prompt"],
                Overlay::Menu(menu) if menu.filter.is_some() => vec!["LgDialog", "MenuFilter"],
                Overlay::Menu(_) => vec!["LgDialog", "Menu"],
                Overlay::Help { .. } => vec!["LgDialog", "LgHelp"],
            };
        }
        match self.focused {
            PanelId::Status => vec!["Panels", "Status"],
            PanelId::Files => vec!["Panels", "Files"],
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => vec!["Panels", "Branches"],
                BranchTab::Remotes => vec!["Panels", "Remotes"],
                BranchTab::Tags => vec!["Panels", "Tags"],
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => vec!["Panels", "Commits"],
                CommitTab::Reflog => vec!["Panels", "Reflog"],
            },
            PanelId::Stash => vec!["Panels", "Stash"],
            PanelId::Main => {
                if self.staging.is_some() {
                    vec!["Panels", "Main", "Staging"]
                } else {
                    match self.main {
                        MainContent::Conflict { .. } => vec!["Panels", "Main", "Conflict"],
                        MainContent::SubCommits { .. } => vec!["Panels", "Main", "SubCommits"],
                        MainContent::CommitFiles { .. } => vec!["Panels", "Main", "CommitFiles"],
                        _ => vec!["Panels", "Main"],
                    }
                }
            }
        }
    }

    /// Pushes an overlay.
    pub(crate) fn push_overlay(&mut self, overlay: Overlay) {
        self.overlays.push(overlay);
    }

    /// Pops the top overlay. Returns whether one was open.
    pub(crate) fn pop_overlay(&mut self) -> bool {
        self.overlays.pop().is_some()
    }

    /// Raises a toast that expires after `dwell`.
    pub(crate) fn toast(&mut self, toast: Toast, now: Instant, dwell: std::time::Duration) {
        if self.toasts.len() >= 3 {
            self.toasts.remove(0);
        }
        self.toasts.push((toast, now + dwell));
    }

    pub(crate) fn visible_toasts(&self) -> Vec<Toast> {
        self.toasts.iter().map(|(toast, _)| toast.clone()).collect()
    }

    /// Expires toasts in place.
    pub(crate) fn expire_toasts(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|(_, deadline)| *deadline > now);
        before != self.toasts.len()
    }
}
