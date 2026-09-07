//! Scrollable Hub detail surfaces and shared anatomy.

use fleet_core::{
    github::{PrChecks, PrReviewDecision, PullRequest, derive_pr_state, local_branch_for_pr},
    inspection::WorktreeInspection,
    model::{CloneJob, CloneStatus, Repo, Worktree},
    sessions::{AgentActivity, SessionState, WorktreeStatus},
};
use fleet_proto::snapshot::PoolStatus;
use fleet_ui_kit::{
    ActiveTheme, FactRow, FactValue, FreshnessStamp, Icon, IconSize, KeyValueList, Pane,
    PaneBorder, PrBadge, SectionHeader, Sheet, StatusGlyph, StatusKind, Text, Tone, Truncate,
    format_age, truncate,
};
use gpui::{AnyElement, App, IntoElement, SharedString, div, prelude::*};

use crate::presentation::{age_secs, pr_badge_state, row_glyph};

mod inspection;
mod pull_request;
mod repo;
mod worktree;

pub use inspection::Inspected;
pub use pull_request::{PrProps, pull_request, pull_request_with_status};
pub use repo::{RepoProps, clone_job, repo};
pub use worktree::{WorktreeProps, worktree, worktree_with_status};

/// Resolves every worktree surface through the same health-first status precedence.
pub(crate) fn resolved_worktree_status(
    status: Option<&WorktreeStatus>,
    slept: bool,
    degraded: bool,
    host_unreachable: bool,
    job_running: bool,
) -> StatusKind {
    row_glyph(
        status.map_or(SessionState::Unknown, |status| status.session),
        slept,
        status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
        degraded,
        host_unreachable,
        job_running,
    )
}

fn path_budget(cx: &App) -> usize {
    let theme = cx.theme();
    ((f32::from(
        theme.metrics.detail_overlay_w
            - theme.space.md * 2.0
            - theme.metrics.fact_label_w
            - theme.space.sm,
    )) / fleet_ui_kit::theme::CH) as usize
}

fn line_budget(cx: &App) -> usize {
    let theme = cx.theme();
    (f32::from(theme.metrics.detail_overlay_w - theme.space.md * 2.0) / fleet_ui_kit::theme::CH)
        as usize
}

/// A `$HOME`-collapsed path, fitted to the panel's value column. `~` is what the shell prints,
/// and the middle is what a reader can afford to lose.
fn path_value(path: &str, home: &str, cx: &App) -> FactValue {
    let tilde = crate::presentation::tilde(path, Some(std::path::Path::new(home)));
    FactValue::known(truncate(&tilde, path_budget(cx), Truncate::Middle))
}

/// The vertical stack every §3.4 variant is: blocks, top to bottom, never scrolling itself.
fn variant(children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_none()
        .children(children)
        .into_any_element()
}

/// The panel's own surface: a docked sheet over a narrow window, an inset pane otherwise.
#[derive(gpui::IntoElement)]
struct DetailSurface {
    body: AnyElement,
    as_overlay: bool,
    scroll: gpui::ScrollHandle,
}

impl gpui::RenderOnce for DetailSurface {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let metrics = cx.theme().metrics;
        let body = div()
            .id("hub-detail-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(self.body);
        if self.as_overlay {
            Sheet::new(true)
                .width(metrics.detail_overlay_w)
                .body(body)
                .into_any_element()
        } else {
            Pane::fixed(metrics.detail_w)
                .border(PaneBorder::Left)
                .raised(true)
                .body(body)
                .into_any_element()
        }
    }
}

pub(crate) fn panel(body: AnyElement, as_overlay: bool, scroll: &gpui::ScrollHandle) -> AnyElement {
    DetailSurface {
        body,
        as_overlay,
        scroll: scroll.clone(),
    }
    .into_any_element()
}

/// The panel's two-line head: `⑂ branch` over a `fg.muted` provenance line.
fn head(icon: Icon, title: SharedString, subtitle: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .px(theme.space.md)
        .pt(theme.space.md)
        .gap(theme.space.xxs)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(icon.el().size(IconSize::Large))
                .child(Text::title(title).ellipsize()),
        )
        .child(Text::ui(subtitle).muted().ellipsize())
        .into_any_element()
}

/// A vertical stack with the panel's 12 px padding.
fn block(children: Vec<AnyElement>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .w_full()
        .px(theme.space.md)
        .pt(theme.space.md)
        .gap(theme.space.xs)
        .children(children)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        ids::WorktreeId,
        sessions::{AgentActivity, SessionState, WorktreeStatus},
    };

    use super::*;

    fn status(session: SessionState, activity: AgentActivity) -> WorktreeStatus {
        WorktreeStatus {
            worktree_id: WorktreeId::try_from("acme/api#feature")
                .unwrap_or_else(|error| panic!("{error}")),
            session,
            windows: Vec::new(),
            running: Vec::new(),
            agent_activity: activity,
            agent_activity_changed_at: None,
        }
    }

    #[test]
    fn detail_and_list_resolve_identical_status() {
        let attached = status(SessionState::Attached, AgentActivity::Working);
        assert_eq!(
            resolved_worktree_status(Some(&attached), false, true, true, true),
            StatusKind::HostUnreachable
        );
        assert_eq!(
            resolved_worktree_status(Some(&attached), false, true, false, true),
            StatusKind::Degraded
        );
        assert_eq!(
            resolved_worktree_status(Some(&attached), false, false, false, true),
            StatusKind::JobRunning
        );
        assert_eq!(
            resolved_worktree_status(Some(&attached), false, false, false, false),
            StatusKind::AgentWorking
        );
        assert_eq!(
            resolved_worktree_status(None, false, false, false, false),
            StatusKind::Unknown
        );
    }
}
