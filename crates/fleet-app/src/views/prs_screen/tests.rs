use fleet_core::{
    github::{PrChecks, PrReviewDecision},
    ids::{HostId, WorktreeId},
    model::Degraded,
};

use super::*;

use crate::presentation::pr_badge_state;
use fleet_core::github::derive_pr_state;
use fleet_ui_kit::PrBadgeState;

use fleet_proto::snapshot::{HostStatus, Snapshot};

fn pr(number: u64, updated: &str, cross: bool, head: &str) -> PullRequest {
    PullRequest {
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        number,
        title: format!("PR {number}"),
        url: format!("https://github.com/buk/payroll/pull/{number}"),
        author: "dannyfuf".to_owned(),
        head_ref_name: head.to_owned(),
        base_ref_name: "main".to_owned(),
        is_draft: false,
        is_cross_repository: cross,
        head_repo: None,
        review_decision: PrReviewDecision::None,
        checks: PrChecks::Pass,
        checks_passed: None,
        checks_total: None,
        additions: 1,
        deletions: 1,
        labels: Vec::new(),
        updated_at: updated.to_owned(),
    }
}

fn worktree(branch: &str, base: &str) -> Worktree {
    Worktree {
        id: WorktreeId::try_from(format!("buk/payroll#{}", branch.replace('/', "-")))
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        slug: "local".to_owned(),
        branch: branch.to_owned(),
        base_ref: base.to_owned(),
        path: "/tmp/wt".to_owned(),
        session: "payroll/local".to_owned(),
        host: None,
        created_at: "2026-09-01T10:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    }
}

fn slice(prs: Vec<PullRequest>, total: usize) -> PrSlice {
    PrSlice {
        tab: PrTab::Mine,
        fetched_at: "2026-09-04T11:59:00Z".to_owned(),
        loading: false,
        error: None,
        total,
        prs,
    }
}

/// A snapshot carrying only what a PR row build reads through the index.
fn snapshot(worktrees: Vec<Worktree>) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        revision: None,
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees,
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    }
}

fn rows(slice: &PrSlice, snapshot: &Snapshot, creating: &[(RepoId, u64)]) -> Vec<PrRow> {
    build_rows(
        &PrInputs {
            slice: Some(slice),
            worktrees: &snapshot.worktrees,
            creating,
            now: 1_788_523_200,
        },
        &SnapshotIndex::new(snapshot),
    )
}

#[test]
fn rows_sort_by_update_time_and_cap_at_one_hundred() {
    let prs: Vec<PullRequest> = (1..=120)
        .map(|index| {
            pr(
                index,
                &format!("2026-09-0{}T10:00:00Z", (index % 9) + 1),
                false,
                "feat/x",
            )
        })
        .collect();
    let slice = slice(prs, 120);
    let rows = rows(&slice, &snapshot(Vec::new()), &[]);
    assert_eq!(rows.len(), ALL_SCOPE_CAP);
    assert_eq!(hidden_rows(Some(&slice)), 20);
    assert!(rows[0].age <= rows[ALL_SCOPE_CAP - 1].age);
}

#[test]
fn a_pull_request_without_a_local_worktree_shows_the_dim_dot() {
    let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
    let rows = rows(&slice, &snapshot(Vec::new()), &[]);
    assert_eq!(rows[0].presence, StatusKind::NoSession);
    assert_eq!(rows[0].local, None);
}

#[test]
fn a_matching_worktree_lights_the_presence_glyph() {
    let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
    let rows = rows(
        &slice,
        &snapshot(vec![worktree("feat/x", "origin/main")]),
        &[],
    );
    assert!(rows[0].local.is_some());
    assert_eq!(
        rows[0].presence,
        StatusKind::Unknown,
        "a worktree with no status yet is unknown, never `none`"
    );
}

#[test]
fn a_pull_ref_worktree_matches_across_repositories() {
    let slice = slice(vec![pr(412, "2026-09-04T10:00:00Z", true, "feat/x")], 1);
    let rows = rows(
        &slice,
        &snapshot(vec![worktree("pr/412", "pull/412/head")]),
        &[],
    );
    assert!(rows[0].local.is_some());
}

#[test]
fn creating_a_worktree_spins_the_presence_glyph_without_moving_the_row() {
    let repo = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
    let slice = slice(vec![pr(7, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
    let rows = rows(&slice, &snapshot(Vec::new()), &[(repo, 7)]);
    assert_eq!(rows[0].presence, StatusKind::JobRunning);
}

#[test]
fn pr_presence_obeys_worktree_health_precedence() {
    let pull_requests = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
    let mut degraded = worktree("feat/x", "origin/main");
    degraded.degraded = Some(Degraded {
        kind: "post_create_hooks".to_owned(),
        step: "1".to_owned(),
        exit_code: Some(1),
        at: "2026-09-04T11:00:00Z".to_owned(),
        log_path: "/tmp/hooks.log".to_owned(),
    });
    let degraded_snapshot = snapshot(vec![degraded]);
    assert_eq!(
        rows(&pull_requests, &degraded_snapshot, &[])[0].presence,
        StatusKind::Degraded
    );

    let mut remote = worktree("feat/x", "origin/main");
    let host = HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}"));
    remote.host = Some(host.clone());
    let mut offline_snapshot = snapshot(vec![remote]);
    offline_snapshot.hosts.push(HostStatus {
        id: host,
        provider: "tailscale".to_owned(),
        version: None,
        link: fleet_proto::snapshot::LinkState::Down,
        address: None,
        agent_binaries: None,
        reachable: true,
        checked_at: "2026-09-04T11:59:00Z".to_owned(),
        error: Some("ssh timed out".to_owned()),
    });
    assert_eq!(
        rows(&pull_requests, &offline_snapshot, &[])[0].presence,
        StatusKind::HostUnreachable
    );
    let row = &rows(&pull_requests, &offline_snapshot, &[])[0];
    assert_eq!(row.host.as_ref().map(HostId::as_str), Some("devbox"));
    assert_eq!(row.host_link, Some(LinkState::Down));
}

#[test]
fn refreshing_counts_are_consistently_marked() {
    let cached = tab_of("Mine", Some(7));
    assert_eq!(cached.count, Some(7));
    assert!(!cached.loading);
    assert_eq!(cached.count_text().as_deref(), Some("7"));
    assert_eq!(
        fetch_stamp_text(true, Some(120)).as_ref(),
        "\u{27F3} refreshing · fetched 2m ago"
    );

    let settled = tab_of("Review", Some(4));
    assert_eq!(settled.count_text().as_deref(), Some("4"));
    let cold = tab_of("Review", None);
    assert!(cold.loading);
    assert_eq!(cold.count_text().as_deref(), Some("\u{2026}"));
}

#[test]
fn badge_states_follow_the_documented_priority() {
    assert_eq!(
        pr_badge_state(derive_pr_state(
            true,
            PrChecks::Fail,
            PrReviewDecision::Approved
        )),
        PrBadgeState::Draft
    );
    assert_eq!(
        pr_badge_state(derive_pr_state(
            false,
            PrChecks::Fail,
            PrReviewDecision::Approved
        )),
        PrBadgeState::CiFail
    );
    assert_eq!(
        pr_badge_state(derive_pr_state(
            false,
            PrChecks::Pass,
            PrReviewDecision::Approved
        )),
        PrBadgeState::Approved
    );
}

#[test]
fn the_filter_matches_number_title_author_and_branch() {
    let slice = slice(vec![pr(412, "2026-09-04T10:00:00Z", false, "feat/rut")], 1);
    let rows = rows(&slice, &snapshot(Vec::new()), &[]);
    assert!(matches(&rows[0], "412"));
    assert!(matches(&rows[0], "pr 412"));
    assert!(matches(&rows[0], "danny"));
    assert!(matches(&rows[0], "rut"));
    assert!(!matches(&rows[0], "nixos"));
}

#[test]
fn the_state_chip_is_the_review_state_alone() {
    assert_eq!(
        ReviewState::of(true, PrReviewDecision::Approved),
        ReviewState::Draft
    );
    assert_eq!(
        ReviewState::of(false, PrReviewDecision::ChangesRequested).badge(),
        PrBadgeState::Changes
    );
    assert_eq!(
        ReviewState::of(false, PrReviewDecision::Approved),
        ReviewState::Approved
    );
    assert_eq!(
        ReviewState::of(false, PrReviewDecision::ReviewRequired).badge(),
        PrBadgeState::Review
    );
    // A failing check no longer hides an approval: the chip still says Approved.
    let slice = slice(vec![pr(1, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
    let mut failing = slice.clone();
    failing.prs[0].checks = PrChecks::Fail;
    failing.prs[0].review_decision = PrReviewDecision::Approved;
    let row = &rows(&failing, &snapshot(Vec::new()), &[])[0];
    assert_eq!(row.review, ReviewState::Approved);
    assert_eq!(row.checks.kind, ChecksKind::Failing);
}

#[test]
fn checks_read_as_counts_or_words() {
    let passing = Checks::of(PrChecks::Pass, Some(6), Some(6));
    assert_eq!(
        (passing.label.as_str(), passing.sentence.as_str()),
        ("6 / 6", "All 6 passing")
    );
    let failing = Checks::of(PrChecks::Fail, Some(5), Some(6));
    assert_eq!(
        (failing.label.as_str(), failing.sentence.as_str()),
        ("1 failing", "1 of 6 failing")
    );
    assert_eq!(Checks::of(PrChecks::Fail, None, None).label, "failing");
    let running = Checks::of(PrChecks::Pending, Some(2), Some(6));
    assert_eq!(running.kind, ChecksKind::Running);
    assert_eq!(running.label, "running");
    assert_eq!(running.tone(), Tone::Secondary);
    let none = Checks::of(PrChecks::None, None, None);
    assert_eq!(
        (none.label.as_str(), none.tone()),
        ("\u{2014}", Tone::Muted)
    );
    assert_eq!(Checks::of(PrChecks::Pass, None, None).label, "passing");
}

#[test]
fn a_creating_row_is_tagged_and_a_local_one_has_a_worktree() {
    let repo = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
    let pulls = slice(vec![pr(7, "2026-09-04T10:00:00Z", false, "feat/x")], 1);
    assert!(rows(&pulls, &snapshot(Vec::new()), &[(repo, 7)])[0].creating);
    let local = rows(
        &pulls,
        &snapshot(vec![worktree("feat/x", "origin/main")]),
        &[],
    );
    assert!(!local[0].creating && local[0].local.is_some());
}
