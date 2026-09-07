use super::*;

impl Lazygit {
    /// The rows the current selection covers, as indexes into the main model's rows.
    pub(crate) fn staging_range(&self) -> Option<(usize, usize)> {
        let staging = self.state.staging.as_ref()?;
        let model = self.main_model();
        let rows = &model.rows;
        rows.get(staging.cursor)?;
        if let Some(anchor) = staging.anchor {
            let (start, end) = if anchor <= staging.cursor {
                (anchor, staging.cursor)
            } else {
                (staging.cursor, anchor)
            };
            return Some((start, end));
        }
        if staging.line_mode {
            return Some((staging.cursor, staging.cursor));
        }
        // Hunk mode: the whole `@@` hunk the cursor sits in, keyed by (file, hunk).
        crate::state::hunk_range(rows, staging.cursor)
    }

    pub(super) fn patch_selection(&self) -> Option<PatchSelection> {
        let staging = self.state.staging.as_ref()?;
        let (start, end) = self.staging_range()?;
        let model = self.main_model();
        let hunks = crate::state::selection_hunks(&model.rows, start, end);
        if hunks.is_empty() {
            return None;
        }
        Some(PatchSelection {
            path: staging.path.clone(),
            side: staging.side,
            hunks,
        })
    }

    pub(super) fn staging_apply(
        &mut self,
        _: &staging::Apply,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.patch_selection() else {
            self.toast("Nothing selected.", Icon::CircleDot);
            cx.notify();
            return;
        };
        let action = match selection.side {
            DiffSide::Unstaged => PatchAction::Stage,
            DiffSide::Staged => PatchAction::Unstage,
        };
        let label = if action == PatchAction::Stage {
            "stage selection"
        } else {
            "unstage selection"
        };
        self.mutate(label, Mutation::Patch { selection, action });
        if let Some(staging) = &mut self.state.staging {
            staging.anchor = None;
        }
        cx.notify();
    }

    pub(super) fn staging_discard(
        &mut self,
        _: &staging::Discard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.patch_selection() else {
            return;
        };
        if selection.side == DiffSide::Staged {
            self.mutate(
                "unstage selection",
                Mutation::Patch {
                    selection,
                    action: PatchAction::Unstage,
                },
            );
            cx.notify();
            return;
        }
        self.open_confirm(Confirm {
            title: "Discard selection".to_owned(),
            target: selection.path.display().to_string(),
            facts: vec!["The selected lines are reverted in the working tree.".to_owned()],
            danger: true,
            outcome: ConfirmOutcome::Request(Box::new(GitRequest::Mutate {
                label: "discard selection".to_owned(),
                mutation: Box::new(Mutation::Patch {
                    selection,
                    action: PatchAction::Discard,
                }),
            })),
        });
        cx.notify();
    }

    pub(super) fn staging_toggle_range(
        &mut self,
        _: &staging::ToggleRange,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(staging) = &mut self.state.staging {
            staging.anchor = match staging.anchor {
                Some(_) => None,
                None => {
                    staging.line_mode = true;
                    Some(staging.cursor)
                }
            };
        }
        cx.notify();
    }

    pub(super) fn staging_toggle_line_mode(
        &mut self,
        _: &staging::ToggleLineMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(staging) = &mut self.state.staging {
            staging.line_mode = !staging.line_mode;
            staging.anchor = None;
            staging.snapped = false;
        }
        self.snap_to_change();
        cx.notify();
    }

    pub(super) fn staging_switch_side(
        &mut self,
        _: &staging::SwitchSide,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(staging) = &mut self.state.staging {
            staging.side = match staging.side {
                DiffSide::Unstaged => DiffSide::Staged,
                DiffSide::Staged => DiffSide::Unstaged,
            };
            staging.cursor = 0;
            staging.anchor = None;
            staging.snapped = false;
        }
        self.sync_main_len();
        self.snap_to_change();
        cx.notify();
    }

    pub(super) fn staging_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        let model = self.main_model();
        let rows = &model.rows;
        let Some(staging) = &mut self.state.staging else {
            return;
        };
        let file = rows.get(staging.cursor).map(|row| row.file);
        let current = rows.get(staging.cursor).and_then(|row| row.hunk);
        let same_file = |row: &crate::views::diff_model::DiffRow| Some(row.file) == file;
        let target = if forward {
            rows.iter()
                .enumerate()
                .find(|(_, row)| row.line.is_some() && same_file(row) && row.hunk > current)
        } else {
            // Both directions land on the *top* of the hunk they move to, as lazygit's `h`/`l` do,
            // so the nearest previous hunk has to be found before its first line.
            let previous = rows
                .iter()
                .filter(|row| {
                    row.line.is_some()
                        && same_file(row)
                        && current.is_some_and(|c| row.hunk < Some(c))
                })
                .filter_map(|row| row.hunk)
                .max();
            previous.and_then(|hunk| {
                rows.iter()
                    .enumerate()
                    .find(|(_, row)| row.line.is_some() && same_file(row) && row.hunk == Some(hunk))
            })
        };
        if let Some((index, _)) = target {
            staging.cursor = index;
            staging.anchor = None;
            self.state.cursors.main.set_len(rows.len());
            self.state.cursors.main.set(index);
            ListView::reveal(&self.scroll_main, &self.state.cursors.main, forward);
        }
        cx.notify();
    }

    pub(super) fn staging_prev_hunk(
        &mut self,
        _: &staging::PrevHunk,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.staging_hunk(false, cx);
    }

    pub(super) fn staging_next_hunk(
        &mut self,
        _: &staging::NextHunk,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.staging_hunk(true, cx);
    }

    pub(super) fn resolve(&mut self, choice: ConflictChoice, label: &str, cx: &mut Context<Self>) {
        let crate::state::MainContent::Conflict { path, .. } = &self.state.main else {
            return;
        };
        let path = path.clone();
        self.mutate(label, Mutation::ResolveConflict { path, choice });
        cx.notify();
    }

    pub(super) fn take_ours(
        &mut self,
        _: &conflict::TakeOurs,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resolve(ConflictChoice::Ours, "resolve ours", cx);
    }

    pub(super) fn take_theirs(
        &mut self,
        _: &conflict::TakeTheirs,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resolve(ConflictChoice::Theirs, "resolve theirs", cx);
    }

    pub(super) fn take_both(
        &mut self,
        _: &conflict::TakeBoth,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resolve(ConflictChoice::Both, "resolve both", cx);
    }

    pub(super) fn conflict_section(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let crate::state::MainContent::Conflict { file, section, .. } = &mut self.state.main {
            let total = file.as_ref().map_or(0, |file| file.conflicts.len());
            if total > 0 {
                *section = if forward {
                    (*section + 1) % total
                } else {
                    (*section + total - 1) % total
                };
            }
        }
        cx.notify();
    }

    pub(super) fn next_section(
        &mut self,
        _: &conflict::NextSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.conflict_section(true, cx);
    }

    pub(super) fn prev_section(
        &mut self,
        _: &conflict::PrevSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.conflict_section(false, cx);
    }
}
