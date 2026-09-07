use super::*;
use gpui::{EntityInputHandler, TestAppContext, WindowHandle};

fn test_window(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Lazygit>,
    async_channel::Receiver<GitRequest>,
    async_channel::Sender<GitEvent>,
) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let (bridge, requests, events) = crate::bridge::channel_bridge();
    let window = cx.add_window(|window, cx| {
        let mut pane = Lazygit::new(PathBuf::from("/fleet-lazygit-test-nonexistent"), cx);
        pane._event_task = Lazygit::spawn_event_loop(&bridge, cx);
        pane.bridge = bridge;
        pane.state.refreshing = false;
        pane.set_active(true, window, cx);
        pane
    });
    (window, requests, events)
}

fn patch(lines: usize, value: &str) -> Arc<fleet_git::Diff> {
    let mut patch = format!(
        "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -0,0 +1,{lines} @@\n"
    );
    for _ in 0..lines {
        patch.push_str(&format!("+{value}\n"));
    }
    Arc::new(fleet_git::parse::diff::parse(patch.as_bytes()).unwrap())
}

fn commit(subject: &str, body: &str) -> fleet_git::Commit {
    fleet_git::Commit {
        oid: fleet_git::ObjectId::from("aaaaaaaa"),
        parents: Vec::new(),
        author_name: String::new(),
        author_email: String::new(),
        authored_at: 0,
        committed_at: 0,
        subject: subject.to_owned(),
        body: body.to_owned(),
        decorations: Vec::new(),
        pushed: false,
    }
}

fn commit_with_oid(oid: &str, subject: &str) -> fleet_git::Commit {
    let mut commit = commit(subject, "");
    commit.oid = fleet_git::ObjectId::from(oid);
    commit
}

fn snapshot(
    operation: OperationState,
    stashes: Vec<fleet_git::StashEntry>,
) -> fleet_git::RepoSnapshot {
    fleet_git::RepoSnapshot {
        root: PathBuf::from("/repo"),
        head: fleet_git::Head::Unborn {
            name: "main".to_owned(),
        },
        operation,
        files: Vec::new(),
        local_branches: Vec::new(),
        remote_branches: Vec::new(),
        remotes: Vec::new(),
        tags: Vec::new(),
        commits: Vec::new(),
        reflog: Vec::new(),
        stashes,
        generation: 1,
    }
}

fn confirm(title: &str) -> Box<Confirm> {
    Box::new(Confirm {
        title: title.to_owned(),
        target: String::new(),
        facts: Vec::new(),
        danger: true,
        outcome: ConfirmOutcome::Request(Box::new(GitRequest::Shutdown)),
    })
}

#[gpui::test]
fn conflict_choice_requires_whole_file_consent(cx: &mut TestAppContext) {
    let (window, requests, _) = test_window(cx);
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::Conflict {
                path: "src/lib.rs".into(),
                file: Some(Arc::new(fleet_git::ConflictFile {
                    path: "src/lib.rs".into(),
                    content: Vec::new(),
                    conflicts: vec![
                        fleet_git::ConflictSection {
                            start: 0,
                            end: 1,
                            ours: Vec::new(),
                            base: None,
                            theirs: Vec::new(),
                        },
                        fleet_git::ConflictSection {
                            start: 2,
                            end: 3,
                            ours: Vec::new(),
                            base: None,
                            theirs: Vec::new(),
                        },
                    ],
                })),
                section: 1,
                error: None,
            };
            pane.resolve(ConflictChoice::Ours, "resolve ours", cx);
            let Some(Overlay::Confirm(confirm)) = pane.state.overlay() else {
                panic!("whole-file confirmation was not opened");
            };
            assert!(confirm.facts[0].contains("whole file"));
            assert!(confirm.facts[0].contains("2 conflict sections"));
        })
        .unwrap();
    assert!(requests.is_empty());
    window
        .update(cx, |pane, window, cx| {
            pane.confirm_accept(&lg_confirm::Accept, window, cx)
        })
        .unwrap();
    assert!(matches!(
        requests.try_recv(),
        Ok(GitRequest::Mutate {
            mutation,
            ..
        }) if matches!(*mutation, Mutation::ResolveConflict {
            choice: ConflictChoice::Ours,
            ..
        })
    ));
}

#[gpui::test]
fn partial_patch_rejects_changed_preimage(cx: &mut TestAppContext) {
    let (window, requests, _) = test_window(cx);
    let displayed = patch(1, "reviewed");
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::FileDiff {
                path: "file.rs".into(),
                unstaged: Some(displayed.clone()),
                staged: None,
                error: None,
            };
            pane.state.staging = Some(Staging {
                path: "file.rs".into(),
                side: DiffSide::Unstaged,
                cursor: 0,
                anchor: None,
                line_mode: false,
                snapped: false,
            });
            pane.prepare_models(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            pane.snap_to_change();
            pane.staging_apply(&staging::Apply, window, cx);
        })
        .unwrap();

    assert!(matches!(
        requests.try_recv(),
        Ok(GitRequest::Mutate { mutation, .. })
            if matches!(&*mutation, Mutation::Patch { displayed: sent, .. }
                if Arc::ptr_eq(sent, &displayed))
    ));
}

#[gpui::test]
fn stash_index_shift_is_rejected(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    let reviewed = fleet_git::ObjectId::from("aaaaaaaa");
    window
        .update(cx, |pane, window, cx| {
            pane.state.snapshot = Some(Arc::new(snapshot(
                OperationState::None,
                vec![fleet_git::StashEntry {
                    index: 3,
                    oid: reviewed.clone(),
                    created_at: 0,
                    subject: "reviewed".to_owned(),
                }],
            )));
            pane.state.cursors.stashes.set_len(1);
            pane.stash_drop(&stash::Drop, window, cx);
            let Some(Overlay::Confirm(confirm)) = pane.state.overlay() else {
                panic!("stash confirmation was not opened");
            };
            assert!(matches!(
                &confirm.outcome,
                ConfirmOutcome::Request(request)
                    if matches!(request.as_ref(), GitRequest::Mutate { mutation, .. }
                        if matches!(mutation.as_ref(), Mutation::StashDrop { index: 3, oid }
                            if oid == &reviewed))
            ));
        })
        .unwrap();
}

#[gpui::test]
fn revert_uses_revert_commands(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.state.snapshot = Some(Arc::new(snapshot(OperationState::Reverting, Vec::new())));
            pane.operation_menu(&global::OperationMenu, window, cx);
            let Some(Overlay::Menu(menu)) = pane.state.overlay() else {
                panic!("operation menu was not opened");
            };
            let mutations = menu.items.iter().map(|item| match &item.action {
                MenuAction::Request(request) => match request.as_ref() {
                    GitRequest::Mutate { mutation, .. } => mutation.as_ref(),
                    request => panic!("unexpected request: {request:?}"),
                },
                action => panic!("unexpected menu action: {action:?}"),
            });
            assert!(matches!(
                mutations.collect::<Vec<_>>().as_slice(),
                [Mutation::RevertContinue, Mutation::RevertAbort]
            ));
        })
        .unwrap();
}

#[test]
fn reword_preserves_complete_message() {
    assert_eq!(
        super::commits_actions::commit_message(&commit(
            "subject",
            "body line one\nbody line two\n"
        )),
        "subject\n\nbody line one\nbody line two"
    );
    assert_eq!(
        super::commits_actions::commit_message(&commit("subject", "")),
        "subject"
    );
}

#[gpui::test]
fn failed_snapshot_releases_refresh_guard(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            pane.state.refreshing = true;
            pane.apply_event(
                GitEvent::ReadFailed {
                    label: "refresh".to_owned(),
                    identity: crate::bridge::ReadIdentity::Snapshot,
                    message: "snapshot failed".to_owned(),
                },
                Instant::now(),
            );
            assert!(!pane.state.refreshing);
        })
        .unwrap();
}

#[gpui::test]
fn read_failure_is_recorded_only_for_its_exact_slot(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            pane.state.main = MainContent::FileDiff {
                path: "src/lib.rs".into(),
                unstaged: None,
                staged: None,
                error: None,
            };
            pane.apply_event(
                GitEvent::ReadFailed {
                    label: "diff".to_owned(),
                    identity: crate::bridge::ReadIdentity::FileDiff("src/other.rs".into()),
                    message: "wrong slot".to_owned(),
                },
                Instant::now(),
            );
            assert!(matches!(
                &pane.state.main,
                MainContent::FileDiff { error: None, .. }
            ));

            pane.apply_event(
                GitEvent::ReadFailed {
                    label: "diff".to_owned(),
                    identity: crate::bridge::ReadIdentity::FileDiff("src/lib.rs".into()),
                    message: "permission denied".to_owned(),
                },
                Instant::now(),
            );
            assert!(matches!(
                &pane.state.main,
                MainContent::FileDiff { error: Some(error), .. }
                    if error == "diff: permission denied"
            ));
        })
        .unwrap();
}

#[gpui::test]
fn sub_commit_refresh_retains_selected_oid(cx: &mut TestAppContext) {
    let (window, requests, _) = test_window(cx);
    let retained_patch = patch(1, "retained");
    window
        .update(cx, |pane, _, _| {
            pane.state.main = MainContent::SubCommits {
                reference: "main".to_owned(),
                commits: vec![
                    commit_with_oid("11111111", "one"),
                    commit_with_oid("22222222", "two"),
                ]
                .into(),
                shown: Some(fleet_git::ObjectId::from("22222222")),
                diff: Some(retained_patch.clone()),
                commits_error: None,
                diff_error: None,
            };
            pane.state.cursors.main.set_len(2);
            pane.state.cursors.main.set(1);
            pane.apply_event(
                GitEvent::RefCommits {
                    reference: "main".to_owned(),
                    commits: vec![
                        commit_with_oid("33333333", "three"),
                        commit_with_oid("22222222", "two"),
                        commit_with_oid("11111111", "one"),
                    ]
                    .into(),
                },
                Instant::now(),
            );
            let MainContent::SubCommits { shown, diff, .. } = &pane.state.main else {
                panic!("sub-commits view was replaced");
            };
            assert_eq!(pane.state.cursors.main.index(), 1);
            assert_eq!(
                shown.as_ref().map(fleet_git::ObjectId::as_str),
                Some("22222222")
            );
            assert!(
                diff.as_ref()
                    .is_some_and(|diff| Arc::ptr_eq(diff, &retained_patch))
            );

            pane.apply_event(
                GitEvent::RefCommits {
                    reference: "main".to_owned(),
                    commits: vec![
                        commit_with_oid("33333333", "three"),
                        commit_with_oid("11111111", "one"),
                    ]
                    .into(),
                },
                Instant::now(),
            );
            let MainContent::SubCommits { shown, diff, .. } = &pane.state.main else {
                panic!("sub-commits view was replaced");
            };
            assert_eq!(
                shown.as_ref().map(fleet_git::ObjectId::as_str),
                Some("11111111")
            );
            assert!(diff.is_none());
        })
        .unwrap();
    assert!(matches!(
        requests.try_recv(),
        Ok(GitRequest::CommitDiff(oid)) if oid.as_str() == "11111111"
    ));
}

#[gpui::test]
fn success_preserves_unrelated_error(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            pane.state.last_error = Some("earlier read failed".to_owned());
            pane.apply_event(
                GitEvent::Mutated {
                    label: "fetch".to_owned(),
                    warning: None,
                },
                Instant::now(),
            );
            assert_eq!(
                pane.state.last_error.as_deref(),
                Some("earlier read failed")
            );
        })
        .unwrap();
}

#[gpui::test]
fn successful_retry_clears_matching_error(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            pane.apply_event(
                GitEvent::Failed {
                    label: "push\u{1f}first".to_owned(),
                    message: "rejected".to_owned(),
                },
                Instant::now(),
            );
            pane.apply_event(
                GitEvent::Mutated {
                    label: "fetch\u{1f}unrelated".to_owned(),
                    warning: None,
                },
                Instant::now(),
            );
            assert_eq!(pane.state.last_error.as_deref(), Some("push: rejected"));

            pane.apply_event(
                GitEvent::Mutated {
                    label: "push\u{1f}retry".to_owned(),
                    warning: None,
                },
                Instant::now(),
            );
            assert!(pane.state.last_error.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn escalation_requires_matching_request(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            pane.state.escalation = Some(("delete branch\u{1f}2".to_owned(), confirm("force")));
            assert!(
                pane.take_escalation("delete branch\u{1f}1", "branch is not fully merged")
                    .is_none()
            );
            assert!(pane.state.escalation.is_some());
            assert!(
                pane.take_escalation("delete branch\u{1f}2", "permission denied")
                    .is_none()
            );
            assert!(pane.state.escalation.is_none());

            pane.state.escalation = Some(("delete branch\u{1f}3".to_owned(), confirm("force")));
            let escalation = pane.take_escalation(
                "delete branch\u{1f}3",
                "error: branch 'topic' is not fully merged",
            );
            assert_eq!(
                escalation.map(|confirm| confirm.title),
                Some("force".to_owned())
            );
        })
        .unwrap();
}

#[gpui::test]
fn content_change_resets_scroll(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            pane.state.main = MainContent::Summary;
            pane.state.focused = PanelId::Files;
            pane.scroll_main
                .scroll_to_item_strict(17, gpui::ScrollStrategy::Top);
            pane.refresh_main();
            assert_eq!(pane.scroll_main.logical_scroll_top_index(), 0);
        })
        .unwrap();
}

#[gpui::test]
fn watcher_runtime_failure_is_visible(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, _| {
            assert!(pane.apply_event(
                GitEvent::WatcherFailed {
                    message: "backend stopped".to_owned(),
                },
                Instant::now(),
            ));
            assert_eq!(
                pane.state.last_error.as_deref(),
                Some("watcher: backend stopped")
            );
        })
        .unwrap();
}

#[gpui::test]
fn secondary_pan_uses_secondary_extent(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::FileDiff {
                path: "file.rs".into(),
                unstaged: Some(patch(2, "x")),
                staged: Some(patch(2, &"wide".repeat(200))),
                error: None,
            };
            pane.main_px_w = 80.0;
            pane.prepare_models(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, _, cx| {
            pane.pan_diff_slot(SLOT_SECONDARY, -500.0, cx);
            assert!(pane.state.main_h_scroll > 0.0);
        })
        .unwrap();
}

#[gpui::test]
fn drilldown_keyboard_pan_uses_patch_extent(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::CommitFiles {
                oid: fleet_git::ObjectId::from("aaaaaaaa"),
                subject: "subject".to_owned(),
                files: Arc::default(),
                whole: None,
                shown: Some("src/lib.rs".into()),
                diff: Some(patch(2, &"wide".repeat(200))),
                whole_error: None,
                diff_error: None,
            };
            pane.main_px_w = 80.0;
            pane.prepare_models(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            pane.scroll_right(&list::ScrollRight, window, cx);
            assert!(pane.state.main_h_scroll > 0.0);
        })
        .unwrap();
}

#[gpui::test]
fn context_refreshes_drilldown(cx: &mut TestAppContext) {
    let (window, requests, _) = test_window(cx);
    let refreshed = patch(1, "refreshed whole patch");
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::CommitFiles {
                oid: fleet_git::ObjectId::from("aaaaaaaa"),
                subject: "subject".to_owned(),
                files: Arc::default(),
                whole: Some(patch(1, "whole")),
                shown: Some("src/lib.rs".into()),
                diff: Some(patch(1, "file")),
                whole_error: None,
                diff_error: None,
            };
            pane.set_context(7, cx);
        })
        .unwrap();
    assert!(matches!(
        requests.try_recv(),
        Ok(GitRequest::SetDiffContext(7))
    ));
    assert!(matches!(
        requests.try_recv(),
        Ok(GitRequest::CommitDiff(oid)) if oid.as_str() == "aaaaaaaa"
    ));
    assert!(matches!(
        requests.try_recv(),
        Ok(GitRequest::CommitFileDiff { oid, path })
            if oid.as_str() == "aaaaaaaa" && path == std::path::Path::new("src/lib.rs")
    ));
    window
        .update(cx, |pane, _, _| {
            pane.state.cursors.main.set(0);
            pane.apply_event(
                GitEvent::CommitDiff {
                    oid: fleet_git::ObjectId::from("aaaaaaaa"),
                    diff: refreshed.clone(),
                    files: Arc::default(),
                },
                Instant::now(),
            );
            let MainContent::CommitFiles {
                shown, whole, diff, ..
            } = &pane.state.main
            else {
                panic!("commit-files drill-down was replaced");
            };
            assert!(shown.is_none());
            assert!(
                whole
                    .as_ref()
                    .is_some_and(|diff| Arc::ptr_eq(diff, &refreshed))
            );
            assert!(
                diff.as_ref()
                    .is_some_and(|diff| Arc::ptr_eq(diff, &refreshed))
            );
        })
        .unwrap();
}

#[gpui::test]
fn enter_remote_resets_identity(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.state.snapshot = Some(Arc::new(fleet_git::RepoSnapshot {
                root: PathBuf::from("/repo"),
                head: fleet_git::Head::Unborn {
                    name: "main".to_owned(),
                },
                operation: OperationState::None,
                files: Vec::new(),
                local_branches: Vec::new(),
                remote_branches: vec![fleet_git::RemoteBranchGroup {
                    remote: "origin".to_owned(),
                    branches: vec![
                        fleet_git::RemoteBranch {
                            name: "origin/main".to_owned(),
                            branch: "main".to_owned(),
                            oid: fleet_git::ObjectId::from("aaaaaaaa"),
                            subject: String::new(),
                            committed_at: 0,
                        },
                        fleet_git::RemoteBranch {
                            name: "origin/topic".to_owned(),
                            branch: "topic".to_owned(),
                            oid: fleet_git::ObjectId::from("bbbbbbbb"),
                            subject: String::new(),
                            committed_at: 0,
                        },
                    ],
                }],
                remotes: vec![fleet_git::Remote {
                    name: "origin".to_owned(),
                    fetch_url: None,
                    push_url: None,
                }],
                tags: Vec::new(),
                commits: Vec::new(),
                reflog: Vec::new(),
                stashes: Vec::new(),
                generation: 1,
            }));
            pane.state.focused = PanelId::Branches;
            pane.state.branch_tab = BranchTab::Remotes;
            pane.state.cursors.remotes.set_len(1);
            pane.enter_remote(&remotes::Enter, window, cx);
            assert_eq!(pane.state.cursors.remote_branches.len(), 2);
            assert_eq!(pane.state.cursors.remote_branches.index(), 0);
            assert!(matches!(
                &pane.state.main,
                MainContent::BranchDiff { name, .. } if name == "origin/main"
            ));
        })
        .unwrap();
}

#[gpui::test]
fn failed_submission_retains_draft(cx: &mut TestAppContext) {
    let (window, requests, _) = test_window(cx);
    let label = window
        .update(cx, |pane, window, cx| {
            pane.open_prompt(
                Prompt {
                    title: "New branch".to_owned(),
                    subtitle: None,
                    buffer: crate::state::Buffer::single_line().with_text("topic"),
                    kind: PromptKind::NewBranch { start_point: None },
                },
                window,
                cx,
            );
            pane.submit_prompt(window, cx);
            match requests.try_recv().unwrap() {
                GitRequest::Mutate { label, .. } => label,
                request => panic!("unexpected request: {request:?}"),
            }
        })
        .unwrap();
    window
        .update(cx, |pane, _, app| {
            pane.apply_event(
                GitEvent::Failed {
                    label,
                    message: "branch exists".to_owned(),
                },
                Instant::now(),
            );
            assert!(matches!(pane.state.overlay(), Some(Overlay::Prompt(_))));
            assert_eq!(
                pane.prompt_input.as_ref().unwrap().read(app).text(),
                "topic"
            );
        })
        .unwrap();
}

#[gpui::test]
fn prompt_accepts_ime_text(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.open_prompt(
                Prompt {
                    title: "New branch".to_owned(),
                    subtitle: None,
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::NewBranch { start_point: None },
                },
                window,
                cx,
            );
            let input = pane.prompt_input.clone().unwrap();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "e", Some(1..1), window, cx);
                input.replace_and_mark_text_in_range(Some(0..1), "é", Some(1..1), window, cx);
                input.unmark_text(window, cx);
                assert_eq!(input.text(), "é");
            });
        })
        .unwrap();
}

#[gpui::test]
fn cancelling_single_line_prompt_restores_pane_focus(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.open_prompt(
                Prompt {
                    title: "New branch".to_owned(),
                    subtitle: None,
                    buffer: crate::state::Buffer::single_line(),
                    kind: PromptKind::NewBranch { start_point: None },
                },
                window,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            pane.prompt_cancel(&prompt::Cancel, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, _| {
            assert!(pane.focus.is_focused(window));
            assert!(pane.prompt_input.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn help_uses_pre_overlay_context(cx: &mut TestAppContext) {
    let (window, _, _) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.state.focused = PanelId::Main;
            pane.state.main = MainContent::Conflict {
                path: "file.rs".into(),
                file: None,
                section: 0,
                error: None,
            };
            pane.open_help(&global::OpenHelp, window, cx);
            assert_eq!(
                crate::overlays::help_chain(pane),
                vec!["Panels", "Main", "Conflict"]
            );
        })
        .unwrap();
}

#[gpui::test]
fn hidden_panes_stop_timers_and_watcher_reads_and_refresh_on_activation(cx: &mut TestAppContext) {
    let (window, requests, events) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.set_active(false, window, cx);
            assert!(pane._ticker.is_none());
        })
        .unwrap();
    events
        .try_send(GitEvent::Changed(Box::new(fleet_git::ChangeEvent {
            paths: vec!["file.rs".into()],
            debounce: Duration::ZERO,
        })))
        .unwrap();
    cx.background_executor
        .advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    assert!(requests.is_empty());
    window
        .update(cx, |pane, window, cx| pane.set_active(true, window, cx))
        .unwrap();
    assert!(matches!(requests.try_recv(), Ok(GitRequest::Snapshot)));
    assert!(requests.is_empty());
}

#[gpui::test]
fn overlay_transitions_restore_pane_focus_without_stealing_host_focus(cx: &mut TestAppContext) {
    let (window, _, _events) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.open_help(&global::OpenHelp, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            assert!(pane.overlay_focus.is_focused(window));
            pane.help_close(&lg_help::Close, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            assert!(pane.focus.is_focused(window));
            pane.set_active(false, window, cx);
            let host = cx.focus_handle();
            window.focus(&host, cx);
            let _ = pane.render(window, cx);
            assert!(host.is_focused(window));
        })
        .unwrap();
}

#[gpui::test]
fn staged_only_models_are_retained_and_superseded_preparation_cannot_publish(
    cx: &mut TestAppContext,
) {
    let (window, _, _events) = test_window(cx);
    let first = patch(5000, "let obsolete = 1;");
    let latest = patch(50, "let current = 2;");
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::FileDiff {
                path: "file.rs".into(),
                unstaged: None,
                staged: Some(first),
                error: None,
            };
            pane.prepare_models(cx);
            pane.state.main = MainContent::FileDiff {
                path: "file.rs".into(),
                unstaged: None,
                staged: Some(latest.clone()),
                error: None,
            };
            pane.prepare_models(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let prepared = window
        .update(cx, |pane, _, cx| {
            let model = pane.main_model();
            assert_eq!(model.rows.len(), 53);
            assert_eq!(model.rows[3].text, "let current = 2;");
            for _ in 0..100 {
                pane.prepare_models(cx);
                assert!(Rc::ptr_eq(&model, &pane.main_model()));
            }
            assert_eq!(pane.models.len(), 1);
            model
        })
        .unwrap();
    window
        .update(cx, |pane, window, cx| {
            pane.set_active(false, window, cx);
            assert!(pane.models.is_empty());
        })
        .unwrap();
    assert_eq!(prepared.rows.len(), 53);
}
