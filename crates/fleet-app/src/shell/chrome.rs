//! The persistent chrome of §2.2: the context bar and the status bar.
//!
//! Both are pure functions of [`AppState`]. The parts that make a decision — which job the
//! ticker shows, what the breadcrumb reads — are separate, testable functions; the rest is
//! kit composition with no styling of its own.

use fleet_proto::job::{JobKind, JobRecord};
use fleet_ui_kit::{
    Chip, ContextBar, ContextTab, Icon, JobTicker, StatusBar, StickyErrorSlot, Tone,
};
use gpui::{AnyElement, App, IntoElement, SharedString, px};

use crate::{
    shell::daemon::{dot_label, dot_state},
    state::{AppState, RepoScope, breadcrumb, chip_counts, parse_percent, running_jobs},
};

/// The left inset that clears the macOS traffic lights (§2.2).
#[cfg(target_os = "macos")]
const LEADING_INSET: f32 = 84.0;
/// Elsewhere the bar starts at the normal 12 px gutter.
#[cfg(not(target_os = "macos"))]
const LEADING_INSET: f32 = 12.0;

/// The short word the status-bar ticker uses for a job kind.
#[must_use]
pub fn job_kind_label(kind: &JobKind) -> &str {
    match kind {
        JobKind::Clone => "clone",
        JobKind::PoolBuild => "pool",
        JobKind::PoolRefresh => "refresh",
        JobKind::CreateWorktree => "create",
        JobKind::DeleteWorktree => "delete",
        JobKind::Prune => "prune",
        JobKind::Inspect => "inspect",
        JobKind::PostCreateHooks => "hooks",
        JobKind::PrFetch => "prs",
        JobKind::RepoFetch => "fetch",
        JobKind::RepoDiscovery => "discover",
        JobKind::Update => "update",
        JobKind::Import => "import",
        JobKind::Custom(name) => name,
    }
}

/// What the job ticker shows: the newest running job, its percent and how many others run.
#[must_use]
pub fn ticker_parts(jobs: &[JobRecord]) -> Option<(String, String, Option<u8>, usize)> {
    let running = running_jobs(jobs);
    let newest = running
        .iter()
        .max_by(|left, right| left.started_at.cmp(&right.started_at))?;
    Some((
        job_kind_label(&newest.kind).to_owned(),
        newest.target.clone(),
        newest.progress.as_deref().and_then(parse_percent),
        running.len() - 1,
    ))
}

/// The 36 px context bar (§2.1, §2.3, §3.1).
#[must_use]
pub fn context_bar(state: &AppState, _cx: &App) -> AnyElement {
    let contexts = state
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.contexts.as_slice())
        .unwrap_or_default();
    let active = state
        .active_context()
        .and_then(|active| contexts.iter().position(|context| &context.id == active))
        .unwrap_or(0);
    let tabs = contexts
        .iter()
        .take(9)
        .enumerate()
        .map(|(index, context)| ContextTab::new(context.name.clone(), index + 1));
    let overflow = contexts.len().saturating_sub(9);

    let sessions: Vec<_> = state
        .snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .statuses
                .iter()
                .map(|status| status.session)
                .collect()
        })
        .unwrap_or_default();
    let hosts = state
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.hosts.as_slice())
        .unwrap_or_default();
    let jobs = state
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.jobs.as_slice())
        .unwrap_or_default();
    let counts = chip_counts(jobs, &sessions, hosts, state.review_pr_count);

    let mut bar = ContextBar::new(tabs)
        .active(active)
        .overflow(overflow)
        .leading_inset(px(LEADING_INSET))
        .daemon(dot_state(&state.daemon));
    if let Some(label) = dot_label(&state.daemon) {
        bar = bar.daemon_label(label);
    }
    if contexts.is_empty() {
        bar = bar.empty("No contexts yet.", "N  create your first context");
    }

    // §2.3: the failed count replaces the jobs chip's color, it is never a second chip.
    let jobs_chip = if counts.failed > 0 {
        Chip::counter(Icon::TriangleAlert, counts.failed).tone(Tone::Danger)
    } else {
        Chip::counter(Icon::LoaderCircle, counts.running)
            .tone(Tone::Warning)
            .spinning(true)
            .id("context-bar-jobs")
    };

    bar = bar
        .chip(jobs_chip)
        .chip(Chip::counter(Icon::CircleDot, counts.live).tone(Tone::Success))
        .chip(Chip::counter(Icon::Moon, counts.sleeping))
        .chip(Chip::counter(Icon::CircleQuestionMark, counts.unknown).tone(Tone::Warning))
        .chip(Chip::counter(Icon::Flag, counts.review));
    if let Some(version) = &state.update_version {
        bar = bar.chip(Chip::labeled(
            Icon::CircleArrowUp,
            format!("\u{2191}{version}"),
        ));
    }
    bar.into_any_element()
}

/// The status-bar breadcrumb `context › repo › row` (§2.2).
#[must_use]
pub fn breadcrumb_text(state: &AppState) -> String {
    let context = state
        .snapshot
        .as_ref()
        .and_then(|snapshot| {
            let active = snapshot.active_context.as_ref()?;
            snapshot
                .contexts
                .iter()
                .find(|context| &context.id == active)
                .map(|context| context.name.clone())
        })
        .unwrap_or_default();
    let repo = match &state.scope {
        RepoScope::All => String::new(),
        RepoScope::Repo(repo) => repo.name().to_owned(),
    };
    let row = state.breadcrumb_row.clone().unwrap_or_default();
    breadcrumb(&[&context, &repo, &row])
}

/// The 26 px status bar (§2.2): breadcrumb · mode word · job ticker · sticky error slot.
#[must_use]
pub fn status_bar(state: &AppState, _cx: &App) -> AnyElement {
    let mut bar = StatusBar::new()
        .breadcrumb(SharedString::from(breadcrumb_text(state)))
        .mode(state.mode().word());

    let jobs = state
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.jobs.as_slice())
        .unwrap_or_default();
    if let Some((kind, target, percent, extra)) = ticker_parts(jobs) {
        let mut ticker = JobTicker::new(kind, target).extra(extra);
        if let Some(percent) = percent {
            ticker = ticker.percent(percent);
        }
        bar = bar.ticker(ticker);
    }
    if let Some(error) = &state.sticky_error {
        bar = bar.error(StickyErrorSlot::new(error.text.clone()));
    }
    bar.into_any_element()
}

#[cfg(test)]
mod tests {
    use fleet_proto::job::JobStatus;

    use super::*;

    fn job(
        id: &str,
        kind: JobKind,
        status: JobStatus,
        started: &str,
        progress: Option<&str>,
    ) -> JobRecord {
        JobRecord {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            kind,
            target: "nixos".to_owned(),
            title: "job".to_owned(),
            status,
            progress: progress.map(str::to_owned),
            log_path: "/tmp/j.log".to_owned(),
            started_at: started.to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        }
    }

    #[test]
    fn the_ticker_shows_the_newest_running_job_and_counts_the_rest() {
        let jobs = vec![
            job(
                "j-1",
                JobKind::Clone,
                JobStatus::Running,
                "2026-09-04T12:00:00Z",
                Some("Receiving objects: 40% (81/202)"),
            ),
            job(
                "j-2",
                JobKind::PostCreateHooks,
                JobStatus::Running,
                "2026-09-04T12:01:00Z",
                Some("pnpm install (2/3)"),
            ),
            job(
                "j-3",
                JobKind::Prune,
                JobStatus::Succeeded,
                "2026-09-04T12:02:00Z",
                None,
            ),
        ];
        let (kind, target, percent, extra) =
            ticker_parts(&jobs).unwrap_or_else(|| panic!("expected a ticker"));
        assert_eq!(kind, "hooks");
        assert_eq!(target, "nixos");
        assert_eq!(percent, None);
        assert_eq!(extra, 1);
    }

    #[test]
    fn an_idle_daemon_has_no_ticker() {
        assert_eq!(ticker_parts(&[]), None);
    }

    #[test]
    fn job_kind_labels_are_single_words() {
        for kind in [
            JobKind::Clone,
            JobKind::PoolBuild,
            JobKind::CreateWorktree,
            JobKind::PrFetch,
            JobKind::Custom("thing".to_owned()),
        ] {
            let label = job_kind_label(&kind);
            assert!(!label.is_empty());
            assert!(!label.contains(' '));
        }
    }
}
