//! The frame: the five side panels, the main area, the banner and the bottom bar.

mod bands;
mod main_panel;
mod side;

pub(crate) use main_panel::log_band_h;

use fleet_git::{Head, OperationState};
use fleet_ui_kit::prelude::*;
use fleet_ui_kit::{
    Divider, EmptyState, KeyHintRow, ListView, ModeWord, Pane, PaneBorder, PaneHeader,
};
use gpui::{AnyElement, App, Context, Window, div};

use crate::keymap;
use crate::root::Lazygit;
use crate::state::{BranchTab, CommitTab, PanelId, ScreenMode, mode_word};
use crate::views::rows;

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
            let metrics = &cx.theme().metrics;
            let fixed = if only_focused {
                None
            } else {
                match panel {
                    PanelId::Status => Some(metrics.status_pane_h),
                    PanelId::Stash if !focused => Some(metrics.stash_pane_h),
                    _ => None,
                }
            };
            panels.push(match fixed {
                Some(height) => div()
                    .h(height)
                    .flex_none()
                    .min_h_0()
                    .child(element)
                    .into_any_element(),
                None => div().flex_1().min_h_0().child(element).into_any_element(),
            });
        }
        panels
    }

    /// The tail every scrolling side list shares: the cursor, the retained scroll position, the
    /// empty state, and clipping to `rows` whole rows.
    fn side_list(
        &self,
        rows: usize,
        list: ListView,
        cursor: usize,
        scroll: &gpui::UniformListScrollHandle,
        empty: EmptyState,
        cx: &App,
    ) -> AnyElement {
        let list = list.cursor(cursor).track_scroll(scroll).empty(empty);
        self.clipped(rows, list.into_any_element(), cx)
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
}

/// The index of the first commit that is reachable from a main branch, which is where lazygit's
/// green "merged" colouring starts. `None` when no main branch is decorated in the log.
#[must_use]
pub(crate) fn merged_from(commits: &[fleet_git::Commit]) -> Option<usize> {
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
pub(crate) fn now_seconds() -> i64 {
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
