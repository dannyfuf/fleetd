//! The main panel: diffs, staging mode, conflicts, the status summary and the command log.

use std::sync::Arc;

use fleet_git::{Commit, CommitFile, ConflictFile, Diff, DiffSide, ObjectId};
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::{
    Divider, EmptyState, KeyHintRow, ListView, Pane, PaneBorder, PaneHeader, SectionHeader,
};
use gpui::{AnyElement, App, Context, Pixels, ScrollWheelEvent, div, px};

use crate::root::{Lazygit, SLOT_MAIN, SLOT_PATCH, SLOT_SECONDARY};
use crate::state::{MainContent, PanelId};
use crate::views::diff::{ViewState, diff_list};
use crate::views::diff_model::DiffViewMode;
use crate::views::rows;

/// How many command-log rows the band shows (lazygit's `commandLogSize`).
const LOG_ROWS: usize = 8;

/// The height of the command-log band: [`LOG_ROWS`] `data_small` rows under one pane header.
///
/// Read from the theme rather than frozen as a constant, because `measure`'s row budget and the
/// band this function sizes have to stay the same number under any theme.
#[must_use]
pub(crate) fn log_band_h(cx: &App) -> Pixels {
    let theme = cx.theme();
    theme.metrics.pane_header_h + theme.text.data_small.line_height * LOG_ROWS as f32
}

impl Lazygit {
    /// The right column: the main panel over the command log.
    pub(crate) fn main_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut column = div()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .child(div().flex_1().min_h_0().child(self.main_pane(cx)));
        if self.state.show_command_log {
            column = column.child(
                div()
                    .h(log_band_h(cx))
                    .flex_none()
                    .min_h_0()
                    .child(self.command_log_pane(cx)),
            );
        }
        column.into_any_element()
    }

    fn main_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Main;
        let title = self.main_title();
        let body = self.main_body(cx);
        Pane::new()
            .focused(focused)
            .border(PaneBorder::Left)
            .header(PaneHeader::new(format!("[0] {title}")))
            .body(body)
            .into_any_element()
    }

    fn main_title(&self) -> String {
        match (&self.state.main, self.state.staging.as_ref()) {
            (_, Some(staging)) => match staging.side {
                DiffSide::Unstaged => "Unstaged changes".to_owned(),
                DiffSide::Staged => "Staged changes".to_owned(),
            },
            (MainContent::Summary, _) => "Status".to_owned(),
            (MainContent::FileDiff { unstaged, .. }, _) => {
                // With nothing left in the worktree the panel shows the staged half instead.
                let has_unstaged = unstaged.as_ref().is_some_and(|diff| !diff.files.is_empty());
                if has_unstaged {
                    "Unstaged changes".to_owned()
                } else {
                    "Staged changes".to_owned()
                }
            }
            (MainContent::CommitDiff { .. }, _) => "Patch".to_owned(),
            (MainContent::SubCommits { reference, .. }, _) => format!("Commits · {reference}"),
            (MainContent::CommitFiles { oid, .. }, _) => {
                format!("Commit files · {}", crate::state::short_oid(oid))
            }
            // `diff_branch` diffs the merge base with the branch tip, so the panel holds a diff
            // rather than a commit list.
            (MainContent::BranchDiff { name, .. }, _) => format!("Diff · {name}"),
            (MainContent::StashDiff { index, .. }, _) => format!("Stash · stash@{{{index}}}"),
            (MainContent::RemoteInfo { name }, _) => format!("Remote · {name}"),
            (MainContent::TagInfo { name }, _) => format!("Tag · {name}"),
            (MainContent::Conflict { .. }, _) => "Conflicts".to_owned(),
            (MainContent::Empty(_), _) => "Diff".to_owned(),
        }
    }

    fn main_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.state.main {
            MainContent::Summary => self.summary_body(cx),
            MainContent::Empty(reason) => EmptyState::new(reason.clone())
                .action("? for the keybindings")
                .into_any_element(),
            MainContent::RemoteInfo { name } => self.remote_body(name, cx),
            MainContent::TagInfo { name } => self.tag_body(name, cx),
            MainContent::Conflict { file, section, .. } => {
                self.conflict_body(file.clone(), *section, cx)
            }
            MainContent::FileDiff {
                unstaged, staged, ..
            } => self.file_diff_body(unstaged.clone(), staged.clone(), cx),
            MainContent::CommitDiff { diff, .. } => self.main_diff_list(diff.clone(), None, cx),
            MainContent::BranchDiff { diff, .. } | MainContent::StashDiff { diff, .. } => {
                self.main_diff_list(diff.clone(), None, cx)
            }
            MainContent::SubCommits {
                commits,
                shown,
                diff,
                ..
            } => self.sub_commits_body(commits, shown.as_ref(), diff.clone(), cx),
            MainContent::CommitFiles {
                oid,
                subject,
                files,
                shown,
                diff,
                ..
            } => self.commit_files_body(oid, subject, files, shown.is_none(), diff.clone(), cx),
        }
    }

    /// lazygit's sub-commits view: a ref's log over the selected commit's patch.
    fn sub_commits_body(
        &self,
        commits: &Arc<[Commit]>,
        shown: Option<&ObjectId>,
        diff: Option<Arc<Diff>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.state.focused == PanelId::Main;
        let cursor = self.state.cursors.main.index();
        let now = super::now_seconds();
        let copied = self.state.copied.clone();
        let budget = self.budget(2 + 9 + 3 + 4);
        let columns = rows::CommitColumns::resolve(self.side_ch as f32);
        let painted = commits.clone();
        let list = ListView::new(
            "lazygit-sub-commits",
            painted.len(),
            move |index, is_cursor, _window, cx| {
                let Some(commit) = painted.get(index) else {
                    return div().into_any_element();
                };
                let is_copied = copied.contains(&commit.oid);
                rows::commit_row(
                    commit,
                    now,
                    &columns,
                    rows::CommitStyle {
                        budget,
                        merged: false,
                        copied: is_copied,
                    },
                    is_cursor,
                    focused,
                    cx,
                )
            },
        )
        .cursor(cursor)
        .track_scroll(&self.scroll_main)
        .empty(EmptyState::new("No commits on this ref.").action(""));

        let patch = match (shown, diff) {
            (Some(_), Some(diff)) => self.patch_list("lazygit-sub-patch", Some(diff), cx),
            (Some(_), None) => EmptyState::new("Reading the patch…")
                .action("")
                .into_any_element(),
            (None, _) => EmptyState::new("⏎ shows the selected commit's patch")
                .action("esc  back")
                .into_any_element(),
        };
        self.split(list.into_any_element(), patch)
    }

    /// lazygit's commit-files view: a header row plus the commit's files, over the selected row's
    /// patch. The header row is how the whole-commit patch stays reachable.
    fn commit_files_body(
        &self,
        oid: &ObjectId,
        subject: &str,
        files: &Arc<[CommitFile]>,
        on_header: bool,
        diff: Option<Arc<Diff>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.state.focused == PanelId::Main;
        let cursor = self.state.cursors.main.index();
        let budget = self.budget(3);
        let header = format!("{}  {subject}", crate::state::short_oid(oid));
        let painted = files.clone();
        let list = ListView::new(
            "lazygit-commit-files",
            painted.len() + 1,
            move |index, is_cursor, _window, cx| match index.checked_sub(1) {
                None => rows::commit_files_header_row(&header, is_cursor, focused, cx),
                Some(row) => match painted.get(row) {
                    Some(file) => rows::commit_file_row(file, budget, is_cursor, focused, cx),
                    None => div().into_any_element(),
                },
            },
        )
        .cursor(cursor)
        .track_scroll(&self.scroll_main)
        .empty(EmptyState::new("This commit changed nothing.").action(""));

        let patch = match diff {
            Some(diff) => self.patch_list("lazygit-commit-patch", Some(diff), cx),
            None if on_header => EmptyState::new("Reading the patch…")
                .action("")
                .into_any_element(),
            None => EmptyState::new("Reading the file's patch…")
                .action("")
                .into_any_element(),
        };
        self.split(list.into_any_element(), patch)
    }

    /// A drill-down list over the patch it selects: two fifths list, three fifths patch.
    fn split(&self, list: AnyElement, patch: AnyElement) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .flex_grow(2.0)
                    .flex_basis(px(0.0))
                    .min_h_0()
                    .overflow_hidden()
                    .child(list),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .flex_grow(3.0)
                    .flex_basis(px(0.0))
                    .min_h_0()
                    .overflow_hidden()
                    .child(patch),
            )
            .into_any_element()
    }

    /// Wraps a diff list so a sideways wheel delta pans its payload.
    fn with_pan(&self, list: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .min_h_0()
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                let delta = event.delta.pixel_delta(cx.theme().metrics.diff_row_h);
                // Only the dominant axis acts, which is the axis lock trackpads need: without
                // it diagonal drift makes sideways panning unusable.
                if delta.x.abs() > delta.y.abs() {
                    this.pan_diff(f32::from(delta.x), cx);
                }
            }))
            .child(list)
            .into_any_element()
    }

    /// A read-only, cursorless patch list — the lower half of a drill-down.
    fn patch_list(
        &self,
        id: &'static str,
        diff: Option<Arc<Diff>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.slot_model(SLOT_PATCH, diff.as_ref(), self.state.diff_mode);
        if model.is_empty() {
            return EmptyState::new("No changes to show.")
                .action("")
                .into_any_element();
        }
        let list = diff_list(
            id,
            model,
            ViewState {
                cursor: None,
                range: None,
                focused: false,
                h_scroll: self.state.main_h_scroll,
                scroll: self.scroll_secondary.clone(),
                scrollbar: true,
            },
            cx,
        );
        self.with_pan(list, cx)
    }

    fn file_diff_body(
        &self,
        unstaged: Option<Arc<Diff>>,
        staged: Option<Arc<Diff>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_staged = staged.as_ref().is_some_and(|diff| !diff.files.is_empty());
        match self.state.staging.as_ref() {
            Some(staging) => {
                let (primary, secondary) = match staging.side {
                    DiffSide::Unstaged => (unstaged, staged),
                    DiffSide::Staged => (staged, unstaged),
                };
                let range = self.staging_range();
                let main = self.main_diff_list(primary, range, cx);
                if secondary
                    .as_ref()
                    .map(|diff| diff.files.is_empty())
                    .unwrap_or(true)
                {
                    return main;
                }
                let label = match staging.side {
                    DiffSide::Unstaged => "Staged changes",
                    DiffSide::Staged => "Unstaged changes",
                };
                self.side_by_side(main, self.secondary_list(secondary, label, cx))
            }
            None => {
                let has_unstaged = unstaged.as_ref().is_some_and(|diff| !diff.files.is_empty());
                if !has_unstaged && has_staged {
                    // Nothing left in the worktree: the panel shows the staged half instead.
                    return self.main_diff_list(staged, None, cx);
                }
                let main = self.main_diff_list(unstaged, None, cx);
                if !has_staged {
                    return main;
                }
                self.side_by_side(main, self.secondary_list(staged, "Staged changes", cx))
            }
        }
    }

    /// The two halves of a working-tree file, side by side.
    fn side_by_side(&self, main: AnyElement, secondary: AnyElement) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .size_full()
            .min_h_0()
            .child(div().flex_1().min_w_0().min_h_0().child(main))
            .child(div().flex_1().min_w_0().min_h_0().child(secondary))
            .into_any_element()
    }

    /// The main panel's primary diff: one virtualised, non-wrapping row per line.
    ///
    /// `range` is the staging selection, when there is one; otherwise the cursor row is drawn on
    /// its own. Wheel scrolling works here whether or not the panel is focused, because the
    /// offset lives in the `UniformListScrollHandle` this view owns.
    fn main_diff_list(
        &self,
        diff: Option<Arc<Diff>>,
        range: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.slot_model(SLOT_MAIN, diff.as_ref(), self.main_mode());
        if model.is_empty() {
            return EmptyState::new("No changes to show.")
                .action("")
                .into_any_element();
        }
        let focused = self.state.focused == PanelId::Main;
        let list = diff_list(
            "lazygit-main",
            model,
            ViewState {
                cursor: Some(self.state.cursors.main.index()),
                range,
                focused,
                h_scroll: self.state.main_h_scroll,
                scroll: self.scroll_main.clone(),
                scrollbar: true,
            },
            cx,
        );
        self.with_pan(list, cx)
    }

    /// The read-only half of a split diff, with its own label so the two sides never blur.
    fn secondary_list(
        &self,
        diff: Option<Arc<Diff>>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.slot_model(SLOT_SECONDARY, diff.as_ref(), DiffViewMode::Unified);
        if model.is_empty() {
            return div().into_any_element();
        }
        let list = diff_list(
            "lazygit-secondary",
            model,
            ViewState {
                cursor: None,
                range: None,
                focused: false,
                h_scroll: self.state.main_h_scroll,
                scroll: self.scroll_secondary.clone(),
                scrollbar: true,
            },
            cx,
        );
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .px(cx.theme().space.sm)
                    .flex_none()
                    .child(SectionHeader::new(label)),
            )
            .child(div().flex_1().min_h_0().child(self.with_pan(list, cx)))
            .into_any_element()
    }

    fn summary_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let files = self.state.files().len();
        let branches = self.state.branches().len();
        let stashes = self.state.stashes().len();
        let commits = self.state.commits().len();
        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .p(theme.space.lg)
            .gap(theme.space.sm)
            .child(Text::title("fleet-lazygit"))
            .child(Text::data(self.state.root.display().to_string()).muted());
        for (label, value) in [
            ("changed files", files),
            ("local branches", branches),
            ("commits loaded", commits),
            ("stash entries", stashes),
        ] {
            column = column.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(theme.space.sm)
                    .child(Text::data(format!("{value:>4}")).flex_none())
                    .child(Text::ui(label).muted()),
            );
        }
        column
            .child(div().h(theme.space.md))
            .child(
                KeyHintRow::new()
                    .key("1-5", "panels")
                    .key("?", "keybindings")
                    .key("R", "refresh")
                    .key("q", "quit"),
            )
            .into_any_element()
    }

    fn remote_body(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let remote = self
            .state
            .remotes()
            .iter()
            .find(|remote| remote.name == name);
        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .p(theme.space.lg)
            .gap(theme.space.xs)
            .child(Text::data(name.to_owned()).color(crate::views::Ansi::Green.color(theme)));
        if let Some(remote) = remote {
            if let Some(url) = &remote.fetch_url {
                column = column.child(Text::data(format!("fetch  {url}")).muted());
            }
            if let Some(url) = &remote.push_url {
                column = column.child(Text::data(format!("push   {url}")).muted());
            }
        }
        column.into_any_element()
    }

    fn tag_body(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let tag = self.state.tags().iter().find(|tag| tag.name == name);
        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .p(theme.space.lg)
            .gap(theme.space.xs)
            .child(Text::data(name.to_owned()));
        if let Some(tag) = tag {
            column = column
                .child(Text::data(crate::state::short_oid(&tag.oid)).muted())
                .child(
                    Text::data(tag.subject.clone()).color(crate::views::Ansi::Yellow.color(theme)),
                );
        }
        column.into_any_element()
    }

    fn conflict_body(
        &self,
        file: Option<Arc<ConflictFile>>,
        section: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let Some(file) = file else {
            return EmptyState::new("Reading the conflict…")
                .action("")
                .into_any_element();
        };
        if file.conflicts.is_empty() {
            return EmptyState::new("No conflict markers left in this file.")
                .action("space  stage the file")
                .into_any_element();
        }
        let index = section.min(file.conflicts.len() - 1);
        let Some((ours, theirs)) = self.conflict_lines(&file, index) else {
            return EmptyState::new("Reading the conflict…")
                .action("")
                .into_any_element();
        };
        let side = |label: &'static str,
                    lines: Arc<[gpui::SharedString]>,
                    color,
                    scroll: &gpui::UniformListScrollHandle| {
            let row_h = theme.text.data.line_height + theme.space.xxs;
            let list = ListView::new(label, lines.len(), move |index, _, _, _| {
                div()
                    .h(row_h)
                    .child(Text::data(lines[index].clone()).color(color))
                    .into_any_element()
            })
            .row_height(row_h)
            .track_scroll(scroll);
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap(theme.space.xxs)
                .child(Text::label(label))
                .child(div().flex_1().min_h_0().child(list))
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .p(theme.space.md)
            .gap(theme.space.sm)
            .child(
                Text::ui(format!(
                    "conflict {} of {}",
                    index + 1,
                    file.conflicts.len()
                ))
                .muted(),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(theme.space.lg)
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(side(
                        "ours",
                        ours,
                        crate::views::Ansi::Green.color(theme),
                        &self.scroll_conflict_ours,
                    ))
                    .child(side(
                        "theirs",
                        theirs,
                        crate::views::Ansi::Cyan.color(theme),
                        &self.scroll_conflict_theirs,
                    )),
            )
            .child(
                KeyHintRow::new()
                    .key("o", "ours")
                    .key("t", "theirs")
                    .key("b", "both")
                    .key("h/l", "section")
                    .key("esc", "back"),
            )
            .into_any_element()
    }

    /// The command-log band.
    ///
    /// A local row list rather than the kit's `LogView`: a `git` argv is long enough to wrap,
    /// and a wrapped row breaks `uniform_list`'s uniform-height assumption, which paints two
    /// lines on top of each other. These rows never wrap and there are only [`LOG_ROWS`] of them.
    fn command_log_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let total = self.state.command_log.len();
        let mut column = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .px(theme.space.md)
            .overflow_hidden();
        if total == 0 {
            column = column.child(Text::data_small("No git commands yet.").faint());
        }
        for line in self.state.command_log.iter().rev().take(LOG_ROWS).rev() {
            let tone = if line.starts_with('\u{2717}') {
                Tone::Danger
            } else {
                Tone::Secondary
            };
            column = column.child(
                div()
                    .h(theme.text.data_small.line_height)
                    .w_full()
                    .flex_none()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(Text::data_small(line.clone()).tone(tone)),
            );
        }
        Pane::new()
            .border(PaneBorder::Left)
            .header(PaneHeader::new("Command log").total(total))
            .body(column)
            .into_any_element()
    }
}
