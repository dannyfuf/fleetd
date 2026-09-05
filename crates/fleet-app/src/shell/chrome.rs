//! The persistent chrome of §2.2: the context bar and the status bar.
//!
//! Both are pure functions of [`AppState`]. The parts that make a decision — which job the
//! ticker shows, what the breadcrumb reads — are separate, testable functions; the rest is
//! kit composition with no styling of its own.

use fleet_proto::job::JobKind;
use fleet_ui_kit::{Chip, ContextBar, ContextTab, Icon, StatusBar, Tone};
use gpui::{AnyElement, App, IntoElement, SharedString, px};

use crate::{
    shell::daemon::{dot_label, dot_state},
    state::{AppState, RepoScope, breadcrumb, chip_counts},
    views::{job_ticker, sticky_error},
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
        JobKind::DeleteRepo => "delete repo",
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

/// The bare version of the daemon's build string.
///
/// fleetd answers `fleetd 0.1.0`, product name included, because the same string is what
/// `fleetd --version` prints on a terminal. Every surface that already says "fleetd" or "Fleet"
/// next to it — §3.13's `◍ fleetd running · 0.1.0 · ~/.fleet`, §3.8.7's `Fleet <version>`,
/// §3.8.6's `Fleet 0.1.0+<sha>` — wants the number alone, not `fleetd fleetd 0.1.0`.
#[must_use]
pub fn bare_version(version: &str) -> &str {
    version
        .strip_prefix("fleetd ")
        .or_else(|| version.strip_prefix("Fleet "))
        .unwrap_or(version)
        .trim()
}

/// How many characters a canonical UUID takes: `8-4-4-4-12`.
const UUID_LEN: usize = 36;

/// The real domain id inside a job target (§3.7 Target).
///
/// fleetd disambiguates concurrent jobs by appending its job id to the target, so the wire
/// carries `acme/widgets#feature-one:762d2efa-4911-…`. §3.7 asks the row for "`RepoId` or
/// `WorktreeId` — the real domain id" and explicitly puts job ids in the log path and on `y`
/// only, because a column that tail-truncates shows nothing *but* the UUID — the exact mistake
/// (`hot-copy:<repo>` matching no row) the spec calls out. Everything that paints a target goes
/// through here.
#[must_use]
pub fn domain_target(target: &str) -> &str {
    let Some(head_end) = target.len().checked_sub(UUID_LEN + 1) else {
        return target;
    };
    if !target.is_char_boundary(head_end) {
        return target;
    }
    if !matches!(target.as_bytes()[head_end], b':' | b'-') {
        return target;
    }
    if !is_uuid(&target[head_end + 1..]) {
        return target;
    }
    let head = &target[..head_end];
    if head.is_empty() { target } else { head }
}

/// Whether `text` is exactly a canonical `8-4-4-4-12` hexadecimal UUID.
///
/// Deliberately strict: a loose "trailing hex run" test would eat a branch called
/// `feat/abc123` out of a perfectly good `WorktreeId`.
fn is_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != UUID_LEN {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
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
    match job_ticker::status_slot(jobs, state.sticky_error.as_ref()) {
        job_ticker::StatusSlot::Error(error) => {
            bar = bar.error(sticky_error::render(&error, &state.screen));
        }
        job_ticker::StatusSlot::Ticker(content) => {
            bar = bar.ticker(job_ticker::render(&content));
        }
        job_ticker::StatusSlot::Idle => {}
    }
    bar.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_product_name_is_said_once() {
        assert_eq!(bare_version("fleetd 0.1.0"), "0.1.0");
        assert_eq!(bare_version("Fleet 0.1.0+abc123"), "0.1.0+abc123");
        assert_eq!(
            bare_version("0.1.0"),
            "0.1.0",
            "a bare semver is left alone"
        );
        assert_eq!(bare_version(""), "");
    }

    #[test]
    fn a_job_target_shows_the_domain_id_and_never_the_job_id() {
        assert_eq!(
            domain_target("acme/widgets#feature-one:762d2efa-4911-4a0e-8b1c-8f3e0d5b2a91"),
            "acme/widgets#feature-one"
        );
        assert_eq!(
            domain_target("acme/widgets:503ad700-7d58-40ff-9294-c2597b1f0a3e"),
            "acme/widgets"
        );
        assert_eq!(
            domain_target("acme/widgets:mine:e5e7b8a8-251b-4b47-9711-2a6f9c0d4e15"),
            "acme/widgets:mine"
        );
        assert_eq!(
            domain_target("inspect-2eea3e43-bbef-4352-aaaa-54a3a1c6f0d2"),
            "inspect"
        );
    }

    #[test]
    fn a_target_that_merely_ends_in_hex_is_left_alone() {
        // A branch name is not a job id: only the canonical 8-4-4-4-12 shape is stripped.
        assert_eq!(
            domain_target("acme/widgets#feat-abc123"),
            "acme/widgets#feat-abc123"
        );
        assert_eq!(domain_target("nixos"), "nixos");
        assert_eq!(domain_target(""), "");
        assert_eq!(
            domain_target("762d2efa-4911-4a0e-8b1c-8f3e0d5b2a91"),
            "762d2efa-4911-4a0e-8b1c-8f3e0d5b2a91",
            "with nothing in front of it, the id is all there is to say"
        );
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
