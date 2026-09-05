//! The frame: the five side panels, the main area, the banner and the bottom bar.

mod main_panel;

pub(crate) use main_panel::LOG_H;

use fleet_git::{Head, OperationState};
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::{
    Divider, EmptyState, KeyHintRow, ListView, ModeWord, Pane, PaneBorder, PaneHeader,
};
use gpui::{AnyElement, App, Context, Window, div, px};

use crate::keymap;
use crate::root::Lazygit;
use crate::state::{BranchTab, CommitTab, PanelId, ScreenMode, mode_word};
use crate::views::rows;

/// The height of the Status pane: one row under its header.
pub(crate) const STATUS_H: f32 = 62.0;
/// The height of the Stash pane when it is not focused.
pub(crate) const STASH_H: f32 = 92.0;
/// The fraction of the window the side column takes in normal mode (lazygit's `sidePanelWidth`).
pub(crate) const SIDE_RATIO: f32 = 0.3333;

/// The share of the window width the side column takes, given the screen mode and what has
/// focus. `0.0` hides it, `1.0` hides the main panel.
#[must_use]
pub(crate) fn side_ratio(mode: ScreenMode, focused: PanelId) -> f32 {
    match (mode, focused != PanelId::Main) {
        (ScreenMode::Normal, _) => SIDE_RATIO,
        (ScreenMode::Half, true) => 0.5,
        (ScreenMode::Full, true) => 1.0,
        (ScreenMode::Half | ScreenMode::Full, false) => 0.0,
    }
}

impl Lazygit {
    /// The body band: the side column beside the main area.
    pub(crate) fn body(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(fatal) = &self.state.fatal {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(EmptyState::new(fatal.clone()).action("q  quit"))
                .into_any_element();
        }

        let ratio = side_ratio(self.state.screen_mode, self.state.focused);
        let width = window.viewport_size().width * ratio;

        let mut body = div().flex().flex_row().size_full().min_h_0();
        if ratio > 0.0 {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .min_h_0()
                    .w(width)
                    .flex_none()
                    .children(self.side_panels(cx)),
            );
        }
        if ratio < 1.0 {
            body = body.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .child(self.main_area(cx)),
            );
        }
        body.into_any_element()
    }

    fn side_panels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let only_focused = self.state.screen_mode != ScreenMode::Normal;
        let mut panels: Vec<AnyElement> = Vec::new();
        for panel in PanelId::SIDE {
            if only_focused && self.state.focused != panel {
                continue;
            }
            if !panels.is_empty() {
                panels.push(Divider::horizontal().into_any_element());
            }
            let focused = self.state.focused == panel;
            let element = match panel {
                PanelId::Status => self.status_panel(cx),
                PanelId::Files => self.files_panel(cx),
                PanelId::Branches => self.branches_panel(cx),
                PanelId::Commits => self.commits_panel(cx),
                PanelId::Stash => self.stash_panel(cx),
                PanelId::Main => continue,
            };
            let fixed = if only_focused {
                None
            } else {
                match panel {
                    PanelId::Status => Some(STATUS_H),
                    PanelId::Stash if !focused => Some(STASH_H),
                    _ => None,
                }
            };
            panels.push(match fixed {
                Some(height) => div()
                    .h(px(height))
                    .flex_none()
                    .min_h_0()
                    .child(element)
                    .into_any_element(),
                None => div().flex_1().min_h_0().child(element).into_any_element(),
            });
        }
        panels
    }

    /// Clips a side list to a whole number of rows.
    ///
    /// `uniform_list` fills the height it is given, so a pane whose body is not an exact multiple
    /// of the row height paints a sliced row at the bottom. Sizing the list to
    /// `rows × row_h` and letting the pane keep the remainder as background is lazygit's
    /// behaviour: a list shows whole rows or nothing.
    fn clipped(&self, rows: usize, list: AnyElement, cx: &App) -> AnyElement {
        let height = cx.theme().metrics.row_h * rows.max(1) as f32;
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(div().h(height).w_full().flex_none().child(list))
            .into_any_element()
    }

    /// The characters a side-pane row can spend on its flexible column, after `fixed` characters
    /// of gutters and the row's own padding.
    fn budget(&self, fixed: usize) -> usize {
        // Two `space.md` paddings plus one gap per column, in round characters.
        const CHROME: usize = 6;
        self.side_ch.saturating_sub(fixed + CHROME).max(8)
    }

    fn header(&self, panel: PanelId, label: &str, total: usize) -> PaneHeader {
        PaneHeader::new(format!("[{}] {label}", panel.jump_label())).total(total)
    }

    /// A tab strip for the panels that have one, active tab first in strength.
    fn tab_strip(&self, titles: &[&str], active: usize, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.space.sm)
            .flex_none();
        for (index, title) in titles.iter().enumerate() {
            let text = if index == active {
                Text::label(*title).tone(Tone::Default)
            } else {
                Text::label(*title).faint()
            };
            strip = strip.child(text);
        }
        strip.into_any_element()
    }

    fn status_panel(&self, cx: &mut Context<Self>) -> AnyElement {
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
            .find(|candidate| candidate.is_head)
            .cloned();
        let upstream = head_branch
            .as_ref()
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
            .h(px(30.0));
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

    fn files_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Files;
        // The header keeps counting **files**: a directory row is a fold of the list, not an
        // entry in it, so the count must not move when a directory is expanded or collapsed.
        let count = self.state.files().len();
        let tree_rows = self.state.file_tree.rows().to_vec();
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

    fn branches_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Branches;
        let now = now_seconds();
        let snapshot = self.state.snapshot.clone();
        let (title, count, body): (&str, usize, AnyElement) = match self.state.branch_tab {
            BranchTab::Local => {
                let count = self.state.branches().len();
                let cursor = self.state.cursors.branches.index();
                let name_budget = self.budget(3 + 9);
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
                        rows::branch_row(branch, now, name_budget, is_cursor, focused, cx)
                    },
                )
                .cursor(cursor)
                .track_scroll(&self.scroll_branches)
                .empty(EmptyState::new("No branches.").action("n  new branch"));
                (
                    "Local branches",
                    count,
                    self.clipped(self.rows_side, list.into_any_element(), cx),
                )
            }
            BranchTab::Remotes => match self.state.remote_drill.clone() {
                Some(remote) => {
                    let count = self.state.remote_branches().len();
                    let cursor = self.state.cursors.remote_branches.index();
                    let remote_budget = self.budget(0);
                    let wanted = remote.clone();
                    let list = ListView::new(
                        "lazygit-remote-branches",
                        count,
                        move |index, is_cursor, _window, cx| {
                            let Some(branch) = snapshot.as_ref().and_then(|snapshot| {
                                snapshot
                                    .remote_branches
                                    .iter()
                                    .find(|group| group.remote == wanted)
                                    .and_then(|group| group.branches.get(index))
                            }) else {
                                return div().into_any_element();
                            };
                            rows::remote_branch_row(branch, remote_budget, is_cursor, focused, cx)
                        },
                    )
                    .cursor(cursor)
                    .track_scroll(&self.scroll_remote_branches)
                    .empty(EmptyState::new("No branches on this remote.").action("f  fetch"));
                    (
                        "Remotes",
                        count,
                        self.clipped(self.rows_side, list.into_any_element(), cx),
                    )
                }
                None => {
                    let count = self.state.remotes().len();
                    let cursor = self.state.cursors.remotes.index();
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
                    )
                    .cursor(cursor)
                    .track_scroll(&self.scroll_remotes)
                    .empty(EmptyState::new("No remotes.").action(""));
                    (
                        "Remotes",
                        count,
                        self.clipped(self.rows_side, list.into_any_element(), cx),
                    )
                }
            },
            BranchTab::Tags => {
                let count = self.state.tags().len();
                let cursor = self.state.cursors.tags.index();
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
                )
                .cursor(cursor)
                .track_scroll(&self.scroll_tags)
                .empty(EmptyState::new("No tags.").action("n  new tag"));
                (
                    "Tags",
                    count,
                    self.clipped(self.rows_side, list.into_any_element(), cx),
                )
            }
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

    fn commits_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.state.focused == PanelId::Commits;
        let now = now_seconds();
        let snapshot = self.state.snapshot.clone();
        let copied = self.state.copied.clone();
        let merged_from = merged_from(self.state.commits());
        let (title, count, body): (&str, usize, AnyElement) = match self.state.commit_tab {
            CommitTab::Commits => {
                let count = self.state.commits().len();
                let cursor = self.state.cursors.commits.index();
                let subject_budget = self.budget(2 + 9 + 3 + 4);
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
                        // Green means "merged into a main branch", which requires the commit to
                        // be on the upstream at all; without an upstream every commit is red.
                        let merged =
                            commit.pushed && merged_from.is_some_and(|first| index >= first);
                        let is_copied = copied.contains(&commit.oid);
                        rows::commit_row(
                            commit,
                            now,
                            rows::CommitStyle {
                                budget: subject_budget,
                                merged,
                                copied: is_copied,
                            },
                            is_cursor,
                            focused,
                            cx,
                        )
                    },
                )
                .cursor(cursor)
                .track_scroll(&self.scroll_commits)
                .empty(EmptyState::new("No commits on this branch.").action("c  commit"));
                (
                    "Commits",
                    count,
                    self.clipped(self.rows_side, list.into_any_element(), cx),
                )
            }
            CommitTab::Reflog => {
                let count = self.state.reflog().len();
                let cursor = self.state.cursors.reflog.index();
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
                )
                .cursor(cursor)
                .track_scroll(&self.scroll_reflog)
                .empty(EmptyState::new("No reflog history.").action(""));
                (
                    "Reflog",
                    count,
                    self.clipped(self.rows_side, list.into_any_element(), cx),
                )
            }
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

    fn stash_panel(&self, cx: &mut Context<Self>) -> AnyElement {
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
        )
        .cursor(cursor)
        .track_scroll(&self.scroll_stashes)
        .empty(EmptyState::new("No stash entries.").action("S  stash options"));

        let body = self.clipped(self.rows_stash, list.into_any_element(), cx);
        Pane::new()
            .focused(focused)
            .border(PaneBorder::None)
            .header(self.header(PanelId::Stash, "Stash", count))
            .body(body)
            .into_any_element()
    }

    /// The "rebasing / merging" strip above the body.
    pub(crate) fn banner(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        let operation = self.state.operation();
        let word = mode_word(&operation)?;
        let mut banner = fleet_ui_kit::Banner::warning(format!("{word} in progress"))
            .icon(Icon::TriangleAlert)
            .hints(
                KeyHintRow::new()
                    .key("m", "continue / abort")
                    .key("R", "refresh"),
            );
        if let OperationState::Rebasing {
            done: Some(done),
            total: Some(total),
            ..
        } = operation
        {
            banner = banner.countdown(format!("{done}/{total}"));
        }
        Some(banner.into_any_element())
    }

    /// The one-row bottom bar: key hints on the left, mode and version on the right.
    pub(crate) fn status_bar(&self, chain: &[&'static str], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mut hints = KeyHintRow::new();
        for (keys, label) in keymap::hints_for_chain(chain, 8) {
            hints = hints.key(keys, label);
        }
        let operation = self.state.operation();
        let mode = mode_word(&operation);

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .size_full()
            .px(theme.space.md)
            .gap(theme.space.md)
            .bg(theme.colors.bg)
            .border_t(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(div().flex_1().min_w_0().overflow_hidden().child(hints));
        if let Some(error) = &self.state.last_error {
            bar = bar.child(
                Text::ui(error.clone())
                    .color(theme.colors.danger)
                    .truncate_at(60, Truncate::Tail)
                    .flex_none(),
            );
        }
        if self.state.refreshing {
            bar = bar.child(Text::hint("…").flex_none());
        }
        match mode {
            Some(word) => {
                bar = bar.child(ModeWord::word(word).tone(Tone::Warning));
            }
            None => {
                bar = bar.child(ModeWord::word("NORMAL").tone(Tone::Secondary));
            }
        }
        bar.child(Text::hint(format!("fleet-lazygit {}", env!("CARGO_PKG_VERSION"))).flex_none())
            .into_any_element()
    }
}

/// The index of the first commit that is reachable from a main branch, which is where lazygit's
/// green "merged" colouring starts. `None` when no main branch is decorated in the log.
#[must_use]
pub fn merged_from(commits: &[fleet_git::Commit]) -> Option<usize> {
    const MAIN: [&str; 4] = ["main", "master", "develop", "trunk"];
    commits.iter().position(|commit| {
        commit.decorations.iter().any(|decoration| {
            decoration
                .split(&[' ', ',', '>'][..])
                .filter(|part| !part.is_empty())
                .any(|part| {
                    let part = part.trim_start_matches("origin/");
                    MAIN.contains(&part)
                })
        })
    })
}

/// Wall-clock seconds, for the age columns.
#[must_use]
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_git::{Commit, ObjectId};

    fn commit(decorations: &[&str]) -> Commit {
        Commit {
            oid: ObjectId::from("a"),
            parents: Vec::new(),
            author_name: String::new(),
            author_email: String::new(),
            authored_at: 0,
            committed_at: 0,
            subject: String::new(),
            body: String::new(),
            decorations: decorations.iter().map(|text| (*text).to_owned()).collect(),
            pushed: false,
        }
    }

    #[test]
    fn merged_starts_at_the_first_main_branch_decoration() {
        let commits = vec![
            commit(&[]),
            commit(&["HEAD -> feature"]),
            commit(&["origin/main", "main"]),
            commit(&[]),
        ];
        assert_eq!(merged_from(&commits), Some(2));
        assert_eq!(merged_from(&[commit(&[])]), None);
    }
}
