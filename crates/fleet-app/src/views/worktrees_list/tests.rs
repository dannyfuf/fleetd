use std::collections::HashMap;

use fleet_core::inspection::WorktreeInspection;
use fleet_core::{
    github::{InspectionPrState, InspectionPullRequest},
    ids::{HostId, RepoId, WorktreeId},
    model::{Degraded, Worktree},
    sessions::{AgentActivity, SessionState, WorktreeStatus, WorktreeWindowStatus},
};

use fleet_proto::{
    job::{JobKind, JobRecord, JobStatus},
    snapshot::{HostStatus, LinkState, Snapshot},
};
use fleet_ui_kit::{Icon, PrBadgeState, StatusKind, Tone};

use super::model::{job_targets_worktree, owns_row};
use super::*;
use crate::{presentation::inspection_badge, views::detail::Inspected};

// `session_glyph` and `inspection_badge` live in `crate::presentation`; the row builder is
// their only consumer with a full case table, so the table is asserted here.
use crate::presentation::session_glyph;

fn worktree(slug: &str, opened: Option<&str>, created: &str) -> Worktree {
    Worktree {
        id: WorktreeId::try_from(format!("buk/payroll#{slug}"))
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        slug: slug.to_owned(),
        branch: format!("feat/{slug}"),
        base_ref: "origin/main".to_owned(),
        path: format!("/home/u/.fleet/worktrees/{slug}"),
        session: format!("payroll/{slug}"),
        host: None,
        created_at: created.to_owned(),
        last_opened_at: opened.map(str::to_owned),
        degraded: None,
    }
}

fn inspection(id: &WorktreeId, dirty: bool, inspected_at: &str) -> WorktreeInspection {
    WorktreeInspection {
        worktree_id: id.clone(),
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        host: "local".to_owned(),
        path: "/tmp/wt".to_owned(),
        branch: "feat/x".to_owned(),
        base_ref: "origin/main".to_owned(),
        head: Some("abc".to_owned()),
        target_branch: "main".to_owned(),
        upstream: None,
        ahead: None,
        behind: None,
        upstream_gone: false,
        dirty,
        dirty_files: dirty.then_some(12),
        merged_into_target: false,
        unique_commits: None,
        published: false,
        merged: false,
        pr: Some(InspectionPullRequest {
            number: 412,
            state: InspectionPrState::Open,
            url: "https://github.com/buk/payroll/pull/412".to_owned(),
            base_ref_name: "main".to_owned(),
            head_ref_oid: "abc".to_owned(),
        }),
        session: SessionState::None,
        running: Vec::new(),
        inspected_at: inspected_at.to_owned(),
        warnings: Vec::new(),
        error: None,
    }
}

/// A snapshot carrying exactly what a row build reads, so the tests exercise the same
/// `SnapshotIndex` path production uses.
fn snapshot(
    worktrees: Vec<Worktree>,
    statuses: Vec<WorktreeStatus>,
    jobs: Vec<JobRecord>,
) -> Snapshot {
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
        statuses,
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs,
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    }
}

/// 2026-09-04T12:00:00Z
const NOW: i64 = 1_788_523_200;

fn rows(snapshot: &Snapshot, inspections: &HashMap<WorktreeId, Inspected>) -> Vec<WorktreeRow> {
    rows_with(snapshot, inspections, &HashMap::new())
}

fn rows_with(
    snapshot: &Snapshot,
    inspections: &HashMap<WorktreeId, Inspected>,
    known_prs: &HashMap<(RepoId, u64), KnownPr>,
) -> Vec<WorktreeRow> {
    build_rows(
        &RowInputs {
            worktrees: snapshot.worktrees.iter().collect(),
            inspections,
            known_prs,
            home: Some(std::path::Path::new("/home/u")),
            now: NOW,
        },
        &crate::presentation::SnapshotIndex::new(snapshot),
    )
}

#[test]
fn sorting_is_most_recently_opened_first() {
    let a = worktree("a", Some("2026-09-04T10:00:00Z"), "2026-09-01T10:00:00Z");
    let b = worktree("b", None, "2026-09-03T10:00:00Z");
    let c = worktree("c", Some("2026-09-04T11:00:00Z"), "2026-09-02T10:00:00Z");
    let mut rows = vec![&a, &b, &c];
    sort_rows(&mut rows);
    let slugs: Vec<&str> = rows.iter().map(|row| row.slug.as_str()).collect();
    assert_eq!(slugs, vec!["c", "a", "b"]);
}

#[test]
fn agent_activity_outranks_attachment_and_sleep_state() {
    assert_eq!(
        session_glyph(SessionState::Detached, true, AgentActivity::Working),
        StatusKind::AgentWorking
    );
    assert_eq!(
        session_glyph(SessionState::Attached, false, AgentActivity::Idle),
        StatusKind::AgentFinished
    );
    assert_eq!(
        session_glyph(SessionState::Detached, true, AgentActivity::Unknown),
        StatusKind::Sleeping
    );
    assert_eq!(
        session_glyph(SessionState::Detached, false, AgentActivity::Unknown),
        StatusKind::DetachedAwake
    );
    assert_eq!(
        session_glyph(SessionState::None, false, AgentActivity::Unknown),
        StatusKind::NoSession
    );
    assert_eq!(
        session_glyph(SessionState::Unknown, false, AgentActivity::Unknown),
        StatusKind::Unknown
    );
}

#[test]
fn an_inspected_row_carries_its_dirty_mark_and_pr_badge() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let mut cache = HashMap::new();
    cache.insert(
        worktree.id.clone(),
        Inspected::ready(inspection(&worktree.id, true, "2026-09-04T11:59:30Z")),
    );
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &cache);
    assert!(rows[0].dirty);
    assert_eq!(rows[0].pr, Some((412, PrBadgeState::Review)));
    assert!(!rows[0].stale_marks);
}

#[test]
fn marks_go_stale_after_ten_minutes() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let mut cache = HashMap::new();
    cache.insert(
        worktree.id.clone(),
        Inspected::ready(inspection(&worktree.id, true, "2026-09-04T11:30:00Z")),
    );
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &cache);
    assert!(rows[0].stale_marks);
}

#[test]
fn an_errored_inspection_draws_no_derived_mark() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let mut cache = HashMap::new();
    cache.insert(worktree.id.clone(), Inspected::failed("gh unavailable"));
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &cache);
    assert!(!rows[0].dirty);
    assert_eq!(rows[0].pr, None);
    assert!(rows[0].inspect_error);
}

#[test]
fn a_running_job_replaces_the_phase_and_the_age() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let jobs = vec![JobRecord {
        id: "job-1".parse().unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::CreateWorktree,
        target: worktree.id.to_string(),
        title: "create".to_owned(),
        status: JobStatus::Running,
        progress: Some("copying files\u{2026}".to_owned()),
        log_path: "/tmp/j.log".to_owned(),
        started_at: "2026-09-04T11:59:00Z".to_owned(),
        finished_at: None,
        cancellable: true,
        retryable: false,
    }];
    let cache = HashMap::new();
    let snapshot = snapshot(vec![worktree], Vec::new(), jobs);
    let rows = rows(&snapshot, &cache);
    assert_eq!(rows[0].glyph, StatusKind::JobRunning);
    assert_eq!(rows[0].phase.as_deref(), Some("copying files\u{2026}"));
    assert!(!rows[0].deleting);
}

#[test]
fn uuid_target_and_cancelling_own_row() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let job = JobRecord {
        id: "job-cancelling"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::DeleteWorktree,
        target: format!("{}:550e8400-e29b-41d4-a716-446655440000", worktree.id),
        title: "delete".to_owned(),
        status: JobStatus::Cancelling,
        progress: None,
        log_path: "/tmp/j.log".to_owned(),
        started_at: "2026-09-04T11:59:00Z".to_owned(),
        finished_at: None,
        cancellable: false,
        retryable: false,
    };
    assert!(owns_row(&job));
    assert!(job_targets_worktree(&job, &worktree.id));
    let other = WorktreeId::try_from("buk/payroll#other").unwrap_or_else(|error| panic!("{error}"));
    assert!(!job_targets_worktree(&job, &other));
}

#[test]
fn a_degraded_worktree_shows_the_hook_failure() {
    let mut worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    worktree.degraded = Some(Degraded {
        kind: "post_create_hooks".to_owned(),
        step: "1".to_owned(),
        exit_code: Some(1),
        at: "2026-09-04T11:00:00Z".to_owned(),
        log_path: "/tmp/hooks.log".to_owned(),
    });
    let cache = HashMap::new();
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &cache);
    assert!(rows[0].degraded);
    assert_eq!(rows[0].glyph, StatusKind::Degraded);
}

#[test]
fn the_filter_matches_branch_repo_and_keep_alive_labels() {
    let worktree = worktree("rut", None, "2026-09-01T10:00:00Z");
    let statuses = vec![WorktreeStatus {
        worktree_id: worktree.id.clone(),
        session: SessionState::Attached,
        windows: Vec::new(),
        running: vec!["claude".to_owned()],
        agent_activity: fleet_core::sessions::AgentActivity::Unknown,
        agent_activity_changed_at: None,
    }];
    let cache = HashMap::new();
    let snapshot = snapshot(vec![worktree], statuses, Vec::new());
    let rows = rows(&snapshot, &cache);
    assert!(matches(&rows[0], "rut"));
    assert!(matches(&rows[0], "payroll"));
    assert!(matches(&rows[0], "claude"));
    assert!(!matches(&rows[0], "nixos"));
    assert!(matches(&rows[0], ""));
}

#[test]
fn a_closed_pull_request_renders_no_badge() {
    assert_eq!(inspection_badge(InspectionPrState::Closed), None);
    assert_eq!(
        inspection_badge(InspectionPrState::Merged),
        Some(PrBadgeState::Merged)
    );
}

fn host_status(id: &str, provider: &str, link: LinkState, reachable: bool) -> HostStatus {
    HostStatus {
        id: HostId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
        provider: provider.to_owned(),
        version: None,
        link,
        address: None,
        agent_binaries: None,
        reachable,
        checked_at: "2026-09-04T12:00:00Z".to_owned(),
        error: (!reachable).then(|| "ssh timed out".to_owned()),
    }
}

#[test]
fn the_host_badge_is_provider_and_link_aware() {
    assert_eq!(
        host_badge(Some("tailscale"), Some(LinkState::Ready), false),
        (Icon::Cloud, Tone::Secondary)
    );
    assert_eq!(
        host_badge(Some("tailscale"), Some(LinkState::Connecting), false),
        (Icon::Cloud, Tone::Muted)
    );
    assert_eq!(
        host_badge(Some("tailscale"), Some(LinkState::Down), false),
        (Icon::CloudOff, Tone::Warning)
    );
    assert_eq!(
        host_badge(Some("tailscale"), Some(LinkState::Ready), true),
        (Icon::CloudOff, Tone::Warning)
    );
    assert_eq!(
        host_badge(Some("command"), Some(LinkState::Ready), false),
        (Icon::Server, Tone::Secondary)
    );
    assert_eq!(
        host_badge(Some("legacy"), Some(LinkState::Legacy), false),
        (Icon::Unplug, Tone::Warning)
    );
    // No status has arrived yet: the chip stays neutral rather than crying offline.
    assert_eq!(
        host_badge(None, None, false),
        (Icon::Cloud, Tone::Secondary)
    );
}

#[test]
fn a_remote_row_carries_its_provider_and_link_state() {
    let mut remote = worktree("a", None, "2026-09-01T10:00:00Z");
    remote.host = Some(HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}")));
    let mut snapshot = snapshot(vec![remote], Vec::new(), Vec::new());
    snapshot.hosts = vec![host_status("devbox", "tailscale", LinkState::Ready, true)];
    let cache = HashMap::new();
    let rows = rows(&snapshot, &cache);
    assert_eq!(rows[0].host.as_deref(), Some("devbox"));
    assert_eq!(rows[0].host_provider.as_deref(), Some("tailscale"));
    assert_eq!(rows[0].host_link, Some(LinkState::Ready));
    assert!(!rows[0].host_unreachable);
    assert_eq!(rows[0].glyph, StatusKind::Unknown);
}

#[test]
fn a_reachable_host_with_a_dropped_link_still_reads_offline() {
    let mut remote = worktree("a", None, "2026-09-01T10:00:00Z");
    remote.host = Some(HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}")));
    let mut snapshot = snapshot(vec![remote], Vec::new(), Vec::new());
    snapshot.hosts = vec![host_status("devbox", "tailscale", LinkState::Down, true)];
    let cache = HashMap::new();
    let rows = rows(&snapshot, &cache);
    assert!(rows[0].host_unreachable);
    assert_eq!(rows[0].glyph, StatusKind::HostUnreachable);
    assert_eq!(
        host_badge(
            rows[0].host_provider.as_deref(),
            rows[0].host_link,
            rows[0].host_unreachable
        ),
        (Icon::CloudOff, Tone::Warning)
    );
}

#[test]
fn a_legacy_host_is_not_offline_merely_for_having_no_link() {
    let mut remote = worktree("a", None, "2026-09-01T10:00:00Z");
    remote.host = Some(HostId::try_from("archdev").unwrap_or_else(|error| panic!("{error}")));
    let mut snapshot = snapshot(vec![remote], Vec::new(), Vec::new());
    snapshot.hosts = vec![host_status("archdev", "legacy", LinkState::Legacy, true)];
    let cache = HashMap::new();
    let rows = rows(&snapshot, &cache);
    assert!(!rows[0].host_unreachable);
    assert_eq!(rows[0].host_link, Some(LinkState::Legacy));
}

#[test]
fn a_worktree_with_no_status_yet_is_unknown_not_none() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let cache = HashMap::new();
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &cache);
    assert_eq!(rows[0].glyph, StatusKind::Unknown);
}

#[test]
fn the_repo_column_survives_a_narrow_pane_in_all_scope() {
    let keys = |pane_ch: f32, all: bool| -> Vec<String> {
        columns(pane_ch, all)
            .into_iter()
            .map(|column| column.key.to_string())
            .collect()
    };
    assert!(keys(138.0, false).contains(&"repo".to_owned()));
    assert!(!keys(93.0, false).contains(&"repo".to_owned()));
    assert!(keys(93.0, true).contains(&"repo".to_owned()));
    let all = keys(93.0, true);
    let branch = all.iter().position(|key| key == "branch");
    let repo = all.iter().position(|key| key == "repo");
    assert_eq!(repo, branch.map(|index| index + 1), "repo follows the name");
}

#[test]
fn below_seventy_two_ch_the_name_pr_age_and_actions_survive() {
    let keys: Vec<String> = columns(70.0, false)
        .into_iter()
        .map(|column| column.key.to_string())
        .collect();
    assert_eq!(keys, vec!["branch", "pr", "age", "actions"]);
}

#[test]
fn every_column_but_the_actions_has_a_head() {
    for column in columns(200.0, true) {
        let head = column_head(column.key.as_ref());
        assert_eq!(
            head.is_empty(),
            column.key.as_ref() == "actions",
            "{}",
            column.key
        );
    }
}

fn window(
    index: u32,
    name: &str,
    agent: Option<&str>,
    activity: AgentActivity,
) -> WorktreeWindowStatus {
    WorktreeWindowStatus {
        index,
        name: name.to_owned(),
        command: name.to_owned(),
        keep_alive: Vec::new(),
        agent: agent.map(str::to_owned),
        agent_activity: activity,
        agent_attention: None,
        agent_activity_changed_at: None,
    }
}

fn status(
    worktree: &Worktree,
    session: SessionState,
    windows: Vec<WorktreeWindowStatus>,
) -> WorktreeStatus {
    let activity = fleet_core::sessions::aggregate_agent_activity(&windows).0;
    WorktreeStatus {
        worktree_id: worktree.id.clone(),
        session,
        windows,
        running: Vec::new(),
        agent_activity: activity,
        agent_activity_changed_at: Some("2026-09-04T11:56:00Z".to_owned()),
    }
}

#[test]
fn the_session_reads_as_words() {
    let working = worktree("spike", None, "2026-09-01T10:00:00Z");
    let terminal = worktree("hotfix", None, "2026-09-01T10:00:00Z");
    let idle = worktree("feature", None, "2026-09-01T10:00:00Z");
    let statuses = vec![
        status(
            &working,
            SessionState::Attached,
            vec![
                window(0, "zsh", None, AgentActivity::Unknown),
                window(1, "claude", Some("claude"), AgentActivity::Working),
            ],
        ),
        status(
            &terminal,
            SessionState::Detached,
            vec![window(0, "zsh", None, AgentActivity::Unknown)],
        ),
        status(&idle, SessionState::None, Vec::new()),
    ];
    let snapshot = snapshot(vec![working, terminal, idle], statuses, Vec::new());
    let rows = rows(&snapshot, &HashMap::new());
    let words: Vec<&str> = rows.iter().map(|row| row.session.text.as_ref()).collect();
    assert_eq!(
        words,
        ["claude working \u{b7} 2 tabs", "1 terminal", "No session"]
    );
    assert_eq!(rows[0].session.dot, Some(Tone::Success));
    assert!(rows[2].session.quiet && rows[2].session.dot.is_none());
    assert_eq!(rows[0].detail.session_state.as_ref(), "claude is working");
    assert_eq!(rows[0].detail.session_age, Some(240));
    assert_eq!(
        rows[0].detail.session_tabs.as_deref(),
        Some("2 tabs: zsh, claude \u{2014} kept by fleetd")
    );
    assert_eq!(rows[2].detail.session_tabs, None);
}

#[test]
fn a_live_session_paints_the_branch_blue_and_a_failure_takes_the_icon_over() {
    let live = worktree("spike", None, "2026-09-01T10:00:00Z");
    let mut broken = worktree("broken", None, "2026-09-01T10:00:00Z");
    broken.degraded = Some(Degraded {
        kind: "post_create_hooks".to_owned(),
        step: "1".to_owned(),
        exit_code: Some(1),
        at: "2026-09-04T11:00:00Z".to_owned(),
        log_path: "/tmp/hooks.log".to_owned(),
    });
    let statuses = vec![status(
        &live,
        SessionState::Attached,
        vec![window(0, "zsh", None, AgentActivity::Unknown)],
    )];
    let jobs = vec![JobRecord {
        id: "job-hooks"
            .parse()
            .unwrap_or_else(|error| panic!("{error}")),
        kind: JobKind::PostCreateHooks,
        target: broken.id.to_string(),
        title: "hooks".to_owned(),
        status: JobStatus::Failed {
            error: "exit 1".to_owned(),
        },
        progress: None,
        log_path: "/tmp/hooks.log".to_owned(),
        started_at: "2026-09-04T10:59:00Z".to_owned(),
        finished_at: Some("2026-09-04T11:00:00Z".to_owned()),
        cancellable: false,
        retryable: true,
    }];
    let snapshot = snapshot(vec![live, broken], statuses, jobs);
    let rows = rows(&snapshot, &HashMap::new());
    assert_eq!(rows[0].name_icon.icon, Icon::GitBranch);
    assert_eq!(rows[0].name_icon.tone, Tone::Accent);
    assert_eq!(rows[1].name_icon.icon, Icon::TriangleAlert);
    assert_eq!(
        rows[1].hook_job.as_ref().map(|id| id.as_str()),
        Some("job-hooks")
    );
    assert!(rows[1].needs_attention && !rows[0].needs_attention);
}

#[test]
fn the_git_facts_are_words() {
    let worktree = worktree("spike", None, "2026-09-02T12:00:00Z");
    let mut facts = inspection(&worktree.id, false, "2026-09-04T11:59:59Z");
    facts.ahead = Some(2);
    facts.behind = Some(0);
    facts.published = true;
    facts.upstream = Some("origin/spike".to_owned());
    let mut cache = HashMap::new();
    cache.insert(worktree.id.clone(), Inspected::ready(facts));
    let repo = worktree.repo_id.clone();
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let mut known = HashMap::new();
    known.insert(
        (repo, 412),
        KnownPr {
            state: PrBadgeState::Approved,
            title: "Ship the settings dialog".into(),
        },
    );
    let rows = rows_with(&snapshot, &cache, &known);
    let row = &rows[0];
    assert_eq!(row.ahead.as_deref(), Some("\u{2191}2"));
    assert_eq!(
        row.pr,
        Some((412, PrBadgeState::Approved)),
        "the PR cache outranks the inspection"
    );
    assert_eq!(row.detail.path.as_ref(), "~/.fleet/worktrees/spike");
    assert_eq!(row.detail.created_age, Some(172_800));
    assert_eq!(
        row.detail.subtitle.as_ref(),
        "buk/payroll \u{b7} from origin/main \u{b7} on this Mac"
    );
    let GitFacts::Known(git) = &row.detail.git else {
        panic!(
            "an inspected row states its git facts: {:?}",
            row.detail.git
        );
    };
    assert_eq!(git.changes.as_ref(), "Clean");
    assert_eq!(git.versus.as_ref(), "vs origin/main");
    assert_eq!(git.ahead_behind.as_deref(), Some("2 ahead \u{b7} 0 behind"));
    assert_eq!(git.published.as_ref(), "Yes, origin/spike");
    assert_eq!(
        git.pr.as_ref().map(|pr| pr.link.as_ref()),
        Some("#412 Ship the settings dialog")
    );
    assert_eq!(git.checked_age, Some(1));
}

#[test]
fn an_uninspected_worktree_says_it_was_not_checked() {
    let worktree = worktree("a", None, "2026-09-01T10:00:00Z");
    let snapshot = snapshot(vec![worktree], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &HashMap::new());
    assert_eq!(rows[0].detail.git, GitFacts::NotChecked);
}

#[test]
fn the_summary_counts_repositories_and_what_needs_attention() {
    let mut a = worktree("a", None, "2026-09-01T10:00:00Z");
    a.degraded = Some(Degraded {
        kind: "post_create_hooks".to_owned(),
        step: "1".to_owned(),
        exit_code: Some(1),
        at: "2026-09-04T11:00:00Z".to_owned(),
        log_path: "/tmp/hooks.log".to_owned(),
    });
    let mut b = worktree("b", None, "2026-09-01T10:00:00Z");
    b.repo_id = RepoId::try_from("acme/web").unwrap_or_else(|error| panic!("{error}"));
    let c = worktree("c", None, "2026-09-01T10:00:00Z");
    let snapshot = snapshot(vec![a, b, c], Vec::new(), Vec::new());
    let rows = rows(&snapshot, &HashMap::new());
    assert_eq!(
        summary(&rows, None).as_ref(),
        "3 across 2 repositories \u{b7} 1 needs attention"
    );
    let payroll = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
    let scoped: Vec<WorktreeRow> = rows
        .iter()
        .filter(|row| row.repo == payroll)
        .cloned()
        .collect();
    assert_eq!(
        summary(&scoped, Some(&payroll)).as_ref(),
        "2 in buk/payroll \u{b7} 1 needs attention"
    );
    assert_eq!(summary(&[], None).as_ref(), "No worktrees yet");
}
