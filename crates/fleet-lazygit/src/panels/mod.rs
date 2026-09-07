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
use gpui::{AnyElement, App, Context, Window, div, relative};

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
    pub(crate) fn body(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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

        let mut body = div().flex().flex_row().size_full().min_h_0();
        if ratio > 0.0 {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .min_h_0()
                    .w(relative(ratio))
                    .flex_none()
                    .debug_selector(|| "lazygit-side-column".to_owned())
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

/// Commit identities reachable from decorated main-branch tips in the bounded snapshot graph.
#[must_use]
pub(crate) fn merged_from(
    commits: &[fleet_git::Commit],
) -> std::collections::HashSet<fleet_git::ObjectId> {
    const MAIN: [&str; 4] = ["main", "master", "develop", "trunk"];
    let by_oid = commits
        .iter()
        .map(|commit| (&commit.oid, commit))
        .collect::<std::collections::HashMap<_, _>>();
    let mut pending = commits
        .iter()
        .filter(|commit| {
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
        .map(|commit| commit.oid.clone())
        .collect::<Vec<_>>();
    let mut reachable = std::collections::HashSet::new();
    while let Some(oid) = pending.pop() {
        if !reachable.insert(oid.clone()) {
            continue;
        }
        if let Some(commit) = by_oid.get(&oid) {
            pending.extend(commit.parents.iter().cloned());
        }
    }
    reachable
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
    use gpui::{Entity, IntoElement, Render, TestAppContext, VisualTestContext, px, size};

    const EMBEDDED_WIDTH: f32 = 600.0;

    struct EmbeddedHarness {
        pane: Entity<Lazygit>,
    }

    impl Render for EmbeddedHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(EMBEDDED_WIDTH))
                .h_full()
                .child(self.pane.clone())
        }
    }

    fn commit(oid: &str, parents: &[&str], decorations: &[&str]) -> Commit {
        Commit {
            oid: ObjectId::from(oid),
            parents: parents
                .iter()
                .map(|parent| ObjectId::from(*parent))
                .collect(),
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
    fn merged_status_uses_reachability() {
        let commits = vec![
            commit("main-tip", &["base"], &["origin/main", "main"]),
            commit("old-unmerged", &["base"], &["HEAD -> feature"]),
            commit("base", &[], &[]),
        ];
        let merged = merged_from(&commits);
        assert!(merged.contains(&ObjectId::from("main-tip")));
        assert!(merged.contains(&ObjectId::from("base")));
        assert!(!merged.contains(&ObjectId::from("old-unmerged")));
        assert!(merged_from(&[commit("feature", &[], &[])]).is_empty());
    }

    #[gpui::test]
    fn embedded_body_uses_pane_width(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
        let pane = cx.new(|cx| {
            Lazygit::embedded("/fleet-lazygit-embedded-width-test-nonexistent".into(), cx)
        });
        let window = cx.add_window(|_, _| EmbeddedHarness { pane: pane.clone() });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.simulate_resize(size(px(1_200.0), px(800.0)));
        pane.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();

        let side = visual
            .debug_bounds("lazygit-side-column")
            .expect("side column was rendered");
        assert!((f32::from(side.size.width) - EMBEDDED_WIDTH * SIDE_RATIO).abs() < 0.1);

        pane.read_with(&visual, |pane, _| {
            let one_ch = f32::from(fleet_ui_kit::theme::ch(1.0)).max(1.0);
            let measured_side = pane.side_ch as f32 * one_ch;
            assert!(measured_side <= f32::from(side.size.width));
            assert!(f32::from(side.size.width) - measured_side < one_ch);

            let gutter =
                crate::views::diff::gutter_width(&crate::views::diff_model::DiffModel::empty(
                    crate::views::diff_model::DiffViewMode::Unified,
                ));
            let expected_main = (EMBEDDED_WIDTH - f32::from(side.size.width) - gutter).max(80.0);
            assert!((pane.main_px_w - expected_main).abs() < 0.1);
        });
    }
}
