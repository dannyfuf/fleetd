use super::*;

fn inspection() -> WorktreeInspection {
    WorktreeInspection {
        worktree_id: WorktreeId::try_from("buk/payroll#fix-rut")
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        host: "local".to_owned(),
        path: "/tmp/wt".to_owned(),
        branch: "fix-rut".to_owned(),
        base_ref: "origin/main".to_owned(),
        head: Some("abc".to_owned()),
        target_branch: "origin/main".to_owned(),
        upstream: Some("origin/fix-rut".to_owned()),
        ahead: Some(0),
        behind: Some(0),
        upstream_gone: false,
        dirty: false,
        dirty_files: Some(0),
        merged_into_target: true,
        unique_commits: Some(0),
        published: true,
        merged: true,
        pr: None,
        session: SessionState::None,
        running: Vec::new(),
        inspected_at: "2026-09-04T12:00:00Z".to_owned(),
        warnings: Vec::new(),
        error: None,
    }
}

#[test]
fn a_clean_merged_unattached_worktree_is_a_compact_lowercase_confirm() {
    let facts = worktree_facts(Some(&inspection()), false, 1_788_523_200);
    assert!(facts.list.is_compact());
    assert_eq!(facts.list.confirm_key(), ConfirmKey::Lower);
    assert!(!facts.risky);
    assert_eq!(facts.age_secs, Some(0));
}

#[test]
fn an_unknown_unique_commit_count_escalates_to_upper() {
    let mut probe = inspection();
    probe.unique_commits = None;
    let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
    assert_eq!(facts.list.confirm_key(), ConfirmKey::Upper);
    assert!(!facts.list.is_compact());
}

#[test]
fn a_detached_session_is_never_called_attached() {
    let mut probe = inspection();
    probe.session = SessionState::Detached;
    probe.running = vec!["claude".to_owned()];
    let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
    let text = facts
        .list
        .ordered()
        .iter()
        .map(|fact| fact.text.to_string())
        .collect::<Vec<_>>();
    assert!(
        text.contains(&"session running, detached \u{00b7} claude running".to_owned()),
        "\u{a7}2.5 wording for a detached session, got {text:?}"
    );

    probe.session = SessionState::Attached;
    let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
    let text = facts
        .list
        .ordered()
        .iter()
        .map(|fact| fact.text.to_string())
        .collect::<Vec<_>>();
    assert!(
        text.contains(&"session attached \u{00b7} claude running".to_owned()),
        "\u{a7}3.8.3 wording for an attached session, got {text:?}"
    );
}

#[test]
fn dirt_and_a_session_are_risks_that_expand_the_dialog() {
    let mut probe = inspection();
    probe.dirty = true;
    probe.dirty_files = Some(12);
    probe.session = SessionState::Attached;
    probe.running = vec!["claude".to_owned()];
    let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
    assert!(facts.risky);
    assert!(!facts.list.is_compact());
    assert_eq!(
        facts.list.confirm_key(),
        ConfirmKey::Lower,
        "known risks still confirm with `y`; only unknowns escalate"
    );
}

#[test]
fn warnings_arrive_verbatim_as_unknown_facts() {
    let mut probe = inspection();
    probe.warnings = vec![fleet_core::inspection::WARNING_GH_UNAVAILABLE.to_owned()];
    let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
    assert_eq!(facts.list.confirm_key(), ConfirmKey::Upper);
}

#[test]
fn facts_that_have_not_arrived_yet_are_loading_and_strong() {
    let facts = worktree_facts(None, true, 0);
    assert_eq!(facts.list.confirm_key(), ConfirmKey::Upper);
    assert_eq!(facts.age_secs, None);
}

#[test]
fn repo_and_context_deletes_are_always_strong() {
    let repo = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
    assert!(
        ConfirmRequest::DeleteRepo {
            repo: repo.clone(),
            worktrees: 8
        }
        .always_strong()
    );
    assert!(!ConfirmRequest::Prune { repo }.always_strong());
}

#[test]
fn a_merge_fact_names_the_remote_ref_the_daemon_compared_against() {
    assert_eq!(target_ref("main"), "origin/main");
    assert_eq!(
        target_ref("release/2026"),
        "origin/release/2026",
        "a slash is part of a bare branch name, not remote qualification"
    );
    assert_eq!(target_ref("origin/main"), "origin/main");
    assert_eq!(target_ref(""), "the base ref");
}

#[test]
fn caption_and_keyboard_share_confirmation_policy() {
    let request = ConfirmRequest::KillSession {
        session: SessionId::try_from("buk/payroll#fix-rut").unwrap(),
        terminals: 1,
        running: vec!["claude".to_owned()],
        unsaved: false,
    };
    let draft = ConfirmState {
        request: Some(request),
        ..ConfirmState::default()
    };
    let policy = confirmation_policy(&draft, 0);
    assert_eq!(policy.key, ConfirmKey::Upper);
    assert!(!admits(policy, ConfirmKey::Lower));
    assert!(admits(policy, ConfirmKey::Upper));
}

#[test]
fn beginning_recheck_clears_stale_prune_preview() {
    let repo = RepoId::try_from("buk/payroll").unwrap();
    let mut draft = ConfirmState {
        request: Some(ConfirmRequest::Prune { repo }),
        prune: Some(PruneResult {
            dry_run: true,
            deleted: vec![WorktreeId::try_from("buk/payroll#old").unwrap()],
            skipped: Vec::new(),
        }),
        error: Some("fetch failed".to_owned()),
        ..ConfirmState::default()
    };
    draft.begin_recheck();
    let policy = confirmation_policy(&draft, 0);
    assert!(draft.loading);
    assert_eq!(draft.error, None);
    assert_eq!(draft.prune, None);
    assert!(!policy.authorized);
    assert!(!admits(policy, ConfirmKey::Upper));
}

#[test]
fn failed_recheck_blocks_stale_prune_preview() {
    let repo = RepoId::try_from("buk/payroll").unwrap();
    let deleted = WorktreeId::try_from("buk/payroll#old").unwrap();
    let mut draft = ConfirmState {
        request: Some(ConfirmRequest::Prune { repo }),
        prune: Some(PruneResult {
            dry_run: true,
            deleted: vec![deleted.clone()],
            skipped: Vec::new(),
        }),
        ..ConfirmState::default()
    };
    draft.begin_recheck();
    draft.loading = false;
    draft.error = Some("fetch failed".to_owned());

    let policy = confirmation_policy(&draft, 0);
    assert!(!policy.authorized);
    assert!(!admits(policy, ConfirmKey::Upper));
    assert!(reviewed_prune_ids(&draft).is_empty());
    assert!(
        draft
            .prune
            .as_ref()
            .is_none_or(|result| !result.deleted.contains(&deleted))
    );
}

#[test]
fn delete_failure_keeps_confirm_open_with_reason() {
    let id = WorktreeId::try_from("buk/payroll#fix-rut").unwrap();
    let results = vec![fleet_proto::response::WorktreeDeleteResult {
        worktree_id: id.clone(),
        ok: false,
        reason: Some("trash move failed".to_owned()),
        trash_entry: None,
    }];
    assert_eq!(
        delete_outcome(&id, &results),
        Err("trash move failed".to_owned())
    );
}

#[test]
fn every_action_has_the_wording_the_spec_fixes() {
    let id = WorktreeId::try_from("buk/payroll#fix-rut-validator")
        .unwrap_or_else(|error| panic!("{error}"));
    let request = ConfirmRequest::DeleteWorktree { id };
    assert_eq!(
        request.title(true),
        "Delete buk/payroll#fix-rut-validator?",
        "compact: the id is the title"
    );
    assert_eq!(
        request.title(false),
        "Delete worktree",
        "expanded: row 1 carries the id, so the title must not repeat it"
    );
    let benign = Facts::default();
    assert_eq!(
        request.consequence(&benign),
        "Moves the copy to trash, then removes it in the background."
    );
    let risky = Facts {
        risky: true,
        ..Facts::default()
    };
    assert!(request.consequence(&risky).contains("kills the session"));
}

#[test]
fn prune_list_count_tracks_deleted_and_expanded_kept_sections() {
    let mut draft = ConfirmState::default();
    assert_eq!(draft.list_len(), 0);
    draft.prune = Some(PruneResult {
        deleted: vec![WorktreeId::try_from("buk/payroll#one").unwrap()],
        dry_run: true,
        skipped: Vec::new(),
    });
    draft.update_list();
    assert_eq!(draft.list_len(), 2);
    draft.show_keep = true;
    draft.update_list();
    assert_eq!(draft.list_len(), 3);
    assert_eq!(draft.list.as_ref().unwrap().item_count(), 3);
}

#[test]
fn commit_never_expands_reviewed_set() {
    let reviewed = WorktreeId::try_from("buk/payroll#reviewed").unwrap();
    let confirm = ConfirmState {
        prune: Some(PruneResult {
            deleted: vec![reviewed.clone()],
            dry_run: true,
            skipped: Vec::new(),
        }),
        ..ConfirmState::default()
    };

    assert_eq!(reviewed_prune_ids(&confirm), [reviewed]);
}

#[test]
fn reviewed_prune_requires_the_negotiated_capability() {
    let repo = RepoId::try_from("buk/payroll").expect("repo");
    let reviewed = WorktreeId::try_from("buk/payroll#reviewed").expect("worktree");
    let confirm = ConfirmState {
        prune: Some(PruneResult {
            dry_run: true,
            deleted: vec![reviewed.clone()],
            skipped: Vec::new(),
        }),
        ..ConfirmState::default()
    };

    let error = reviewed_prune_request(&confirm, repo.clone(), false)
        .expect_err("an old daemon must be refused");
    assert!(error.contains("update or restart fleetd"));
    assert_eq!(
        reviewed_prune_request(&confirm, repo, true).expect("capable daemon"),
        RequestBody::PruneWorktrees {
            dry_run: false,
            fetch: false,
            kill_sessions: false,
            repo: Some(RepoId::try_from("buk/payroll").expect("repo")),
            ids: Some(vec![reviewed]),
        }
    );
}
