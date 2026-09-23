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
        request.title(),
        "Delete worktree fix-rut-validator?",
        "the title names the worktree; the subtitle carries the full id and the path"
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
    assert_eq!(request.consequence(&risky), "The copy moves to the trash.");
    let losing = Facts {
        risky: true,
        losses: Losses {
            session: true,
            unpushed: 2,
            dirty: Some(Some(3)),
            commits_unknown: false,
        },
        ..Facts::default()
    };
    assert_eq!(
        request.consequence(&losing),
        "The session is killed and the copy moves to the trash. The 2 unpushed commits and \
         3 uncommitted files exist only here and will be lost."
    );
    let one = Facts {
        risky: true,
        losses: Losses {
            unpushed: 1,
            ..Losses::default()
        },
        ..Facts::default()
    };
    assert_eq!(
        request.consequence(&one),
        "The copy moves to the trash. The 1 unpushed commit exists only here and will be lost."
    );
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

/// A transport that records the wire instead of reaching a daemon.
#[derive(Clone, Default)]
struct RecordingTransport(std::rc::Rc<std::cell::RefCell<Vec<RequestBody>>>);

impl SessionTransport for RecordingTransport {
    fn send(&self, body: RequestBody) {
        self.0.borrow_mut().push(body);
    }

    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>> {
        self.send(body);
        let (_reply, answer) = async_channel::bounded(1);
        answer
    }
}

/// `x` on a delegation row stages the child, the dialog adopts it, and only the accepted
/// dialog sends `DelegationCancel` — cancelling a child is irreversible, so nothing reaches the
/// daemon before the user has read back which child it is.
#[gpui::test]
fn a_staged_delegation_cancel_reaches_the_dialog_and_only_then_the_wire(
    cx: &mut gpui::TestAppContext,
) {
    let delegation = DelegationId::new();
    let child = ThreadId::new();
    let state = cx.new(|_| AppState::new("/tmp/fleet-delegation-cancel", Instant::now()));
    let wire = RecordingTransport::default();

    cx.update(|cx| {
        ConfirmRequest::stage_delegation_cancel(
            &state,
            delegation,
            child,
            AgentKind::Codex,
            "verify the payroll reducer".to_owned(),
            cx,
        );
        state.update(cx, |app, _| {
            app.open_overlay(crate::state::Overlay::Dialog(
                crate::dialogs::Dialogs::Confirm,
            ));
        });
        adopt_staged_delegation_cancel(&state, cx);
    });

    assert!(
        wire.0.borrow().is_empty(),
        "staging the confirm must not reach the daemon"
    );
    let adopted = cx
        .update(|cx| {
            crate::dialogs::read_host(&state, cx, |host, _| host.confirm.delegation_cancel.clone())
        })
        .unwrap_or_else(|| panic!("the dialog adopts the staged child"));
    assert_eq!(adopted.id, delegation);
    assert_eq!(adopted.child, child);
    assert_eq!(adopted.provider, AgentKind::Codex);

    cx.update(|cx| commit_delegation_cancel(&state, &wire, delegation, cx));

    assert_eq!(
        wire.0.borrow().as_slice(),
        [RequestBody::DelegationCancel { delegation }]
    );
    state.read_with(cx, |app, _| assert!(app.overlay.is_none()));
}

#[test]
fn confirmed_prune_stays_open_when_any_reviewed_item_was_skipped() {
    let result = PruneResult {
        dry_run: false,
        deleted: Vec::new(),
        skipped: vec![fleet_proto::response::PruneSkipped {
            worktree_id: WorktreeId::try_from("buk/payroll#offline").expect("worktree"),
            reason: "host dev-box is unreachable".to_owned(),
            merged: false,
            dirty: false,
            unique_commits: None,
            running: Vec::new(),
        }],
    };

    assert!(prune_requires_review(&result));
}

/// `]` on a card whose child is working asks one question, and `y` is what moves it.
///
/// Contracts §5.5: the sentence names the column and the age of the run, the primary verb is
/// `Move`, and the request that leaves is the plain move with `cancel_run` set — the app never
/// cancels the run separately, because two requests are two chances to half-succeed.
#[gpui::test]
fn a_confirmed_move_cancels_the_run_and_an_unstaged_column_moves_nothing(
    cx: &mut gpui::TestAppContext,
) {
    let card = CardId::try_from("card-0").unwrap_or_else(|error| panic!("{error}"));
    let status = StatusId::try_from("in-progress").unwrap_or_else(|error| panic!("{error}"));
    let request = ConfirmRequest::MoveCancelsRun {
        card: card.clone(),
        key: "FLE-1".to_owned(),
        target: "In Progress".to_owned(),
        elapsed: "2m".to_owned(),
    };
    assert_eq!(request.title(), "Move FLE-1?");
    assert_eq!(
        request.consequence(&Facts::default()),
        "FLE-1 is working (2m). Move to In Progress and cancel the run?"
    );
    assert_eq!(request.action_label(0), "Move");
    assert_eq!(request.target(), "FLE-1");
    assert!(
        !request.always_strong() && !request.rechecks(),
        "a move is confirmed with `y` and has no facts to re-check"
    );

    let state = cx.new(|_| AppState::new("/tmp/fleet-move-cancels-run", Instant::now()));
    let wire = RecordingTransport::default();
    cx.update(|cx| {
        crate::dialogs::with_host(&state, cx, |host| {
            host.confirm = ConfirmState {
                request: Some(request.clone()),
                // A dropped card stages its place in the column too, and the confirmed move
                // carries it: the dialog is the same one `[` raises, not a second path.
                move_target: Some(MoveTarget {
                    status: status.clone(),
                    index: Some(1),
                }),
                ..ConfirmState::default()
            };
        });
        state.update(cx, |app, _| {
            app.open_overlay(crate::state::Overlay::Dialog(
                crate::dialogs::Dialogs::Confirm,
            ));
        });
        commit(&state, &wire, ConfirmKey::Lower, cx);
    });
    assert_eq!(
        wire.0.borrow().as_slice(),
        [RequestBody::MoveCard {
            card_id: card,
            status_id: status,
            index: Some(1),
            cancel_run: true,
        }]
    );
    state.read_with(cx, |app, _| assert!(app.overlay.is_none()));

    // A reconnect or a context switch drops the staged column with the board it belonged to.
    // Confirming then does exactly what answering `n` does: nothing at all.
    cx.update(|cx| {
        crate::dialogs::with_host(&state, cx, |host| {
            host.confirm = ConfirmState {
                request: Some(request),
                move_target: None,
                ..ConfirmState::default()
            };
        });
        commit(&state, &wire, ConfirmKey::Lower, cx);
    });
    assert_eq!(
        wire.0.borrow().len(),
        1,
        "a move with no column to move into sends nothing"
    );
    cx.run_until_parked();
}

/// `X` on a board card stages the run, the dialog adopts it, and only `y` reaches the wire.
///
/// The delegation cancel's own pattern (`confirm.rs`), for the key that stops a card's child:
/// staging is what lets the sentence be written where the card is — a live child and an owed
/// slot are different facts — while the daemon hears nothing until the dialog is accepted.
#[gpui::test]
fn a_staged_card_run_cancel_reaches_the_dialog_and_only_then_the_wire(
    cx: &mut gpui::TestAppContext,
) {
    let card = CardId::try_from("card-0").unwrap_or_else(|error| panic!("{error}"));
    let state = cx.new(|_| AppState::new("/tmp/fleet-card-run-cancel", Instant::now()));
    let wire = RecordingTransport::default();

    cx.update(|cx| {
        ConfirmRequest::stage_card_run_cancel(
            &state,
            card.clone(),
            "FLE-1".to_owned(),
            "child stops and reports no further work".to_owned(),
            cx,
        );
        state.update(cx, |app, _| {
            app.open_overlay(crate::state::Overlay::Dialog(
                crate::dialogs::Dialogs::Confirm,
            ));
        });
        adopt_staged_board_confirm(&state, cx);
    });

    assert!(
        wire.0.borrow().is_empty(),
        "staging the confirm must not reach the daemon"
    );
    let adopted = cx
        .update(|cx| {
            crate::dialogs::read_host(&state, cx, |host, _| host.confirm.card_run_cancel.clone())
        })
        .unwrap_or_else(|| panic!("the dialog adopts the staged run"));
    assert_eq!(adopted.key, "FLE-1");

    cx.update(|cx| commit_card_run_cancel(&state, &wire, card.clone(), cx));

    assert_eq!(
        wire.0.borrow().as_slice(),
        [RequestBody::CardRunCancel { card_id: card }]
    );
    state.read_with(cx, |app, _| assert!(app.overlay.is_none()));
    cx.run_until_parked();
}

/// Nothing a board key staged survives the dialog that adopted it.
#[gpui::test]
fn an_adopted_board_confirm_is_taken_out_of_the_staging_set(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/fleet-board-confirm-staging", Instant::now()));
    let status = StatusId::try_from("done").unwrap_or_else(|error| panic!("{error}"));
    cx.update(|cx| {
        let target = MoveTarget {
            status,
            index: None,
        };
        ConfirmRequest::stage_move_target(&state, target.clone(), cx);
        adopt_staged_board_confirm(&state, cx);
        crate::dialogs::with_host(&state, cx, |host| {
            assert_eq!(host.confirm.move_target, Some(target));
            host.confirm = ConfirmState::default();
        });
        // The next confirm — a delete, say — must not inherit the column `[` staged.
        adopt_staged_board_confirm(&state, cx);
        crate::dialogs::with_host(&state, cx, |host| {
            assert_eq!(host.confirm.move_target, None);
        });
    });
}

/// What `dialog.message` reports is the sentence the card on screen actually asks.
///
/// One test over all three shapes, because the ordering is the claim: the two staged cancels
/// draw their own card and never reach `ConfirmRequest::consequence`, so a reader that asked
/// the request first would report the *move*'s sentence over a cancel's dialog
/// (`docs/TESTING-HARNESS.md` §3).
#[test]
fn the_reported_consequence_is_the_sentence_the_open_card_draws() {
    assert_eq!(
        consequence(&ConfirmState::default()),
        None,
        "a confirm opened with no target asks nothing"
    );

    let card = CardId::try_from("card-0").unwrap_or_else(|error| panic!("{error}"));
    let mut draft = ConfirmState {
        request: Some(ConfirmRequest::MoveCancelsRun {
            card: card.clone(),
            key: "FLE-1".to_owned(),
            target: "Done".to_owned(),
            elapsed: "2m".to_owned(),
        }),
        ..ConfirmState::default()
    };
    assert_eq!(
        consequence(&draft).as_deref(),
        Some("FLE-1 is working (2m). Move to Done and cancel the run?"),
        "contracts §5.5 fixes the move confirm's sentence word for word"
    );

    draft.card_run_cancel = Some(CardRunCancelDraft {
        card,
        key: "FLE-1".to_owned(),
        fact: "child stops and reports no further work".to_owned(),
    });
    assert_eq!(
        consequence(&draft).as_deref(),
        Some(CARD_RUN_CANCEL_CONSEQUENCE),
        "the board's `X` draws its own card, whatever request was published"
    );

    draft.delegation_cancel = Some(DelegationCancelDraft {
        id: DelegationId::new(),
        child: ThreadId::new(),
        provider: AgentKind::Codex,
        title: "verify the payroll reducer".to_owned(),
    });
    assert_eq!(
        consequence(&draft).as_deref(),
        Some(DELEGATION_CANCEL_CONSEQUENCE),
        "a transcript's `x` is chosen first, exactly as `render` chooses it"
    );
}
