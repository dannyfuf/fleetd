//! The rail's row model: aggregation, names, clones, filtering and the issue chip.
use super::*;

fn repo(owner: &str, name: &str, context: &str) -> Repo {
    Repo {
        id: RepoId::try_from(format!("{owner}/{name}")).unwrap_or_else(|error| panic!("{error}")),
        owner: owner.to_owned(),
        name: name.to_owned(),
        url: format!("git@github.com:{owner}/{name}.git"),
        context_id: ContextId::try_from(context).unwrap_or_else(|error| panic!("{error}")),
        default_branch: "main".to_owned(),
        path: format!("/home/u/.fleet/repos/{owner}/{name}"),
        cloned_at: "2026-09-01T10:00:00Z".to_owned(),
        hooks: fleet_core::model::RepoHooks::default(),
    }
}

fn worktree(repo_id: &str, slug: &str) -> Worktree {
    Worktree {
        id: fleet_core::ids::WorktreeId::try_from(format!("{repo_id}#{slug}"))
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: RepoId::try_from(repo_id).unwrap_or_else(|error| panic!("{error}")),
        slug: slug.to_owned(),
        branch: slug.to_owned(),
        base_ref: "origin/main".to_owned(),
        path: format!("/home/u/.fleet/worktrees/{slug}"),
        session: format!("s/{slug}"),
        host: None,
        created_at: "2026-09-02T10:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    }
}

fn clone_job(owner: &str, name: &str, context: &str, status: CloneStatus) -> CloneJob {
    CloneJob {
        id: RepoId::try_from(format!("{owner}/{name}")).unwrap_or_else(|error| panic!("{error}")),
        owner: owner.to_owned(),
        name: name.to_owned(),
        url: format!("git@github.com:{owner}/{name}.git"),
        context_id: ContextId::try_from(context).unwrap_or_else(|error| panic!("{error}")),
        default_branch: "main".to_owned(),
        path: "/tmp/repo".to_owned(),
        staging_path: "/tmp/staging".to_owned(),
        log_path: "/tmp/clone.log".to_owned(),
        pid: None,
        started_at: "2026-09-04T10:00:00Z".to_owned(),
        status,
        error: None,
    }
}

fn job(
    id: &str,
    kind: JobKind,
    target: &str,
    status: JobStatus,
    started_at: &str,
    progress: Option<&str>,
) -> JobRecord {
    JobRecord {
        id: JobId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
        kind,
        target: target.to_owned(),
        title: id.to_owned(),
        status,
        progress: progress.map(str::to_owned),
        log_path: "/tmp/job.log".to_owned(),
        started_at: started_at.to_owned(),
        finished_at: None,
        cancellable: true,
        retryable: true,
    }
}

#[test]
fn unknown_outranks_every_other_session_state() {
    assert_eq!(
        aggregate_glyph([
            StatusKind::NoSession,
            StatusKind::Attached,
            StatusKind::Unknown
        ]),
        StatusKind::Unknown
    );
    assert_eq!(
        aggregate_glyph([StatusKind::NoSession, StatusKind::Attached]),
        StatusKind::Attached
    );
    assert_eq!(
        aggregate_glyph([StatusKind::Sleeping, StatusKind::NoSession]),
        StatusKind::Sleeping
    );
    assert_eq!(aggregate_glyph([]), StatusKind::NoSession);
}

#[test]
fn an_unreachable_host_aggregates_like_unknown() {
    assert_eq!(
        aggregate_glyph([StatusKind::Attached, StatusKind::HostUnreachable]),
        StatusKind::HostUnreachable
    );
}

#[test]
fn finished_agents_aggregate_above_working_and_plain_sessions() {
    assert_eq!(
        aggregate_glyph([
            StatusKind::Attached,
            StatusKind::AgentWorking,
            StatusKind::AgentFinished,
        ]),
        StatusKind::AgentFinished
    );
    assert_eq!(
        aggregate_glyph([StatusKind::AgentFinished, StatusKind::Unknown]),
        StatusKind::Unknown
    );
}

#[test]
fn names_disambiguate_only_on_collision() {
    let repos = vec![
        repo("buk", "payroll", "buk"),
        repo("acme", "payroll", "buk"),
        repo("buk", "www", "buk"),
    ];
    let rows = rail_rows(None, &repos, &[], &[], &RepoGlyphs::default(), &[]);
    assert_eq!(rows[2].name.as_ref(), "buk/payroll");
    assert_eq!(rows[3].name.as_ref(), "www");
}

#[test]
fn all_is_pinned_first_and_counts_every_worktree_in_the_context() {
    let repos = vec![repo("buk", "www", "buk"), repo("buk", "payroll", "buk")];
    let worktrees = vec![
        worktree("buk/payroll", "a"),
        worktree("buk/payroll", "b"),
        worktree("buk/www", "c"),
    ];
    let rows = rail_rows(
        Some(&repos[0].context_id),
        &repos,
        &[],
        &worktrees,
        &RepoGlyphs::default(),
        &[],
    );
    assert_eq!(rows[0].kind, RailKind::All);
    assert_eq!(rows[0].count, Some(3));
    assert_eq!(rows[1].name.as_ref(), "payroll");
    assert_eq!(rows[1].count, Some(2));
    assert_eq!(rows[2].name.as_ref(), "www");
}

#[test]
fn clones_sort_where_the_repo_will_live() {
    let repos = vec![repo("buk", "aaa", "buk"), repo("buk", "zzz", "buk")];
    let clones = vec![clone_job("buk", "mmm", "buk", CloneStatus::Cloning)];
    let rows = rail_rows(
        Some(&repos[0].context_id),
        &repos,
        &clones,
        &[],
        &RepoGlyphs::default(),
        &[],
    );
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_ref()).collect();
    assert_eq!(names, vec!["All", "aaa", "mmm", "zzz"]);
    assert_eq!(rows[2].kind, RailKind::Cloning);
    assert_eq!(rows[2].glyph, StatusKind::Cloning);
}

#[test]
fn a_failed_clone_keeps_its_row_until_dismissed() {
    let clones = vec![clone_job("buk", "old-api", "buk", CloneStatus::Failed)];
    let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
    let rows = rail_rows(
        Some(&context),
        &[],
        &clones,
        &[],
        &RepoGlyphs::default(),
        &[],
    );
    assert_eq!(rows[1].kind, RailKind::CloneFailed);
    assert_eq!(rows[1].glyph, StatusKind::CloneFailed);
}

#[test]
fn failed_clone_opens_its_own_job() {
    let clones = vec![clone_job("buk", "old-api", "buk", CloneStatus::Failed)];
    let failed = job(
        "job-clone-failed",
        JobKind::Clone,
        "buk/old-api:startup-reconcile",
        JobStatus::Failed {
            error: "network".to_owned(),
        },
        "2026-09-04T10:00:00Z",
        None,
    );
    let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
    let rows = rail_rows(
        Some(&context),
        &[],
        &clones,
        &[],
        &RepoGlyphs::default(),
        std::slice::from_ref(&failed),
    );
    assert_eq!(rows[1].job.as_ref(), Some(&failed.id));
}

#[test]
fn child_delete_does_not_mark_repo_deleting() {
    let repos = vec![repo("buk", "payroll", "buk")];
    let child_delete = job(
        "job-delete-child",
        JobKind::DeleteWorktree,
        "buk/payroll#feature:attempt-1",
        JobStatus::Running,
        "2026-09-04T10:00:00Z",
        None,
    );
    let rows = rail_rows(
        Some(&repos[0].context_id),
        &repos,
        &[],
        &[],
        &RepoGlyphs::default(),
        &[child_delete],
    );
    assert_eq!(rows[1].kind, RailKind::Repo);
}

#[test]
fn clone_progress_uses_current_attempt() {
    let clones = vec![clone_job("buk", "api", "buk", CloneStatus::Cloning)];
    let jobs = vec![
        job(
            "job-old",
            JobKind::Clone,
            "buk/api",
            JobStatus::Failed {
                error: "network".to_owned(),
            },
            "2026-09-04T10:00:00Z",
            Some("25%"),
        ),
        job(
            "job-current",
            JobKind::Clone,
            "buk/api:startup-reconcile",
            JobStatus::Running,
            "2026-09-04T10:01:00Z",
            Some("70%"),
        ),
    ];
    let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
    let rows = rail_rows(
        Some(&context),
        &[],
        &clones,
        &[],
        &RepoGlyphs::default(),
        &jobs,
    );
    assert_eq!(rows[1].percent, Some(70));
}

#[test]
fn same_name_clone_rows_have_distinct_identity() {
    let clones = vec![
        clone_job("buk", "api", "buk", CloneStatus::Cloning),
        clone_job("acme", "api", "buk", CloneStatus::Cloning),
    ];
    let context = ContextId::try_from("buk").unwrap_or_else(|error| panic!("{error}"));
    let rows = rail_rows(
        Some(&context),
        &[],
        &clones,
        &[],
        &RepoGlyphs::default(),
        &[],
    );
    assert_eq!(rows[1].name.as_ref(), "acme/api");
    assert_eq!(rows[2].name.as_ref(), "buk/api");
    assert_ne!(rail_row_id(&rows[1]), rail_row_id(&rows[2]));
}

#[test]
fn filtering_keeps_the_all_row() {
    let repos = vec![repo("buk", "payroll", "buk"), repo("buk", "www", "buk")];
    let context = repos[0].context_id.clone();
    let rows = rail_rows(
        Some(&context),
        &repos,
        &[],
        &[],
        &RepoGlyphs::default(),
        &[],
    );
    let filtered: Vec<_> = rows.iter().filter(|row| matches(row, "pay")).collect();
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].kind, RailKind::All);
    assert_eq!(filtered[1].name.as_ref(), "payroll");
    assert_eq!(rows.iter().filter(|row| matches(row, "nothing")).count(), 1);
}

#[test]
fn a_degraded_or_unreachable_worktree_is_an_issue_on_its_repo() {
    let repos = vec![repo("buk", "payroll", "buk"), repo("buk", "www", "buk")];
    let (payroll, www) = (repos[0].id.clone(), repos[1].id.clone());
    let mut glyphs = RepoGlyphs::default();
    glyphs.add(&payroll, StatusKind::Degraded);
    glyphs.add(&payroll, StatusKind::HostUnreachable);
    glyphs.add(&payroll, StatusKind::Attached);
    glyphs.add(&www, StatusKind::AgentWorking);
    let rows = rail_rows(Some(&repos[0].context_id), &repos, &[], &[], &glyphs, &[]);
    assert_eq!(rows[1].issues.as_deref(), Some("2 issues"));
    assert_eq!(rows[2].issues, None);
    assert_eq!(dot_tone(rows[2].glyph), Tone::Success);
}

#[test]
fn a_clone_row_says_what_it_is_doing() {
    assert_eq!(clone_status(false, Some(40)).as_ref(), "cloning 40%");
    assert_eq!(clone_status(false, None).as_ref(), "cloning\u{2026}");
    assert_eq!(clone_status(true, Some(40)).as_ref(), "failed");
}
