use super::*;
use crate::views::diff_model::RowKind;
use std::path::PathBuf;

#[test]
fn the_error_slot_prefers_the_line_carrying_gits_marker() {
    assert_eq!(
        error_line("Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\nfailed"),
        "CONFLICT (content): Merge conflict in a.txt"
    );
    assert_eq!(
        error_line("fatal: a branch named 'main' already exists"),
        "fatal: a branch named 'main' already exists"
    );
    assert_eq!(error_line("\n  plain trouble  \nmore"), "plain trouble");
    assert_eq!(error_line(""), "");
}

fn file(path: &str, index: ChangeKind, worktree: ChangeKind) -> FileStatus {
    FileStatus {
        path: PathBuf::from(path),
        previous_path: None,
        index,
        worktree,
        conflict: None,
    }
}

#[test]
fn short_status_matches_git_porcelain() {
    assert_eq!(
        short_status(&file("a", ChangeKind::Untracked, ChangeKind::Untracked)),
        ['?', '?']
    );
    assert_eq!(
        short_status(&file("a", ChangeKind::Modified, ChangeKind::Modified)),
        ['M', 'M']
    );
    assert_eq!(
        short_status(&file("a", ChangeKind::Added, ChangeKind::Unmodified)),
        ['A', ' ']
    );
    assert_eq!(
        short_status(&file("a", ChangeKind::Unmodified, ChangeKind::Modified)),
        [' ', 'M']
    );
}

#[test]
fn staged_and_unstaged_predicates_follow_lazygit() {
    let fully_staged = file("a", ChangeKind::Modified, ChangeKind::Unmodified);
    assert!(has_staged(&fully_staged));
    assert!(!has_unstaged(&fully_staged));

    let untracked = file("a", ChangeKind::Untracked, ChangeKind::Untracked);
    assert!(!has_staged(&untracked));
    assert!(has_unstaged(&untracked));

    let mixed = file("a", ChangeKind::Modified, ChangeKind::Modified);
    assert!(has_staged(&mixed));
    assert!(has_unstaged(&mixed));
}

#[test]
fn upstream_status_puts_behind_first() {
    let mut branch = Branch {
        name: "main".to_owned(),
        oid: ObjectId::from("abc"),
        is_head: false,
        upstream: Some(fleet_git::Upstream {
            name: "origin/main".to_owned(),
            ahead: 3,
            behind: 5,
            gone: false,
        }),
        subject: String::new(),
        committed_at: 0,
        checked_out_at: None,
    };
    assert_eq!(upstream_status(&branch).as_deref(), Some("↓5↑3"));
    branch.upstream = Some(fleet_git::Upstream {
        name: "origin/main".to_owned(),
        ahead: 0,
        behind: 0,
        gone: false,
    });
    assert_eq!(upstream_status(&branch).as_deref(), Some("✓"));
    if let Some(upstream) = branch.upstream.as_mut() {
        upstream.gone = true;
    }
    assert_eq!(upstream_status(&branch).as_deref(), Some("gone"));
    branch.upstream = None;
    assert_eq!(upstream_status(&branch), None);
}

#[test]
fn screen_mode_cycles_in_both_directions() {
    assert_eq!(ScreenMode::Normal.next(), ScreenMode::Half);
    assert_eq!(ScreenMode::Half.next(), ScreenMode::Full);
    assert_eq!(ScreenMode::Full.next(), ScreenMode::Normal);
    assert_eq!(ScreenMode::Normal.prev(), ScreenMode::Full);
}

#[test]
fn side_panels_cycle_and_wrap() {
    assert_eq!(PanelId::Status.next_side(), PanelId::Files);
    assert_eq!(PanelId::Stash.next_side(), PanelId::Status);
    assert_eq!(PanelId::Status.prev_side(), PanelId::Stash);
}

#[test]
fn an_overlay_replaces_the_context_chain() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    state.focused = PanelId::Files;
    assert_eq!(state.context_chain(), vec!["Panels", "Files"]);
    state.push_overlay(Overlay::Help { top: 0 });
    assert_eq!(state.context_chain(), vec!["LgDialog", "LgHelp"]);
    assert!(state.pop_overlay());
    assert_eq!(state.context_chain(), vec!["Panels", "Files"]);
}

#[test]
fn a_stale_snapshot_is_dropped() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    let mut snapshot = snapshot_with_generation(7);
    state.apply_snapshot(Box::new(snapshot.clone()));
    assert_eq!(state.epoch, 7);
    snapshot.generation = 5;
    snapshot
        .files
        .push(file("late", ChangeKind::Modified, ChangeKind::Modified));
    state.apply_snapshot(Box::new(snapshot));
    assert_eq!(state.epoch, 7);
    assert!(state.files().is_empty());
}

#[test]
fn the_file_cursor_follows_the_file_across_a_refresh() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    let mut snapshot = snapshot_with_generation(1);
    snapshot.files = vec![
        file("a", ChangeKind::Modified, ChangeKind::Unmodified),
        file("b", ChangeKind::Modified, ChangeKind::Unmodified),
        file("c", ChangeKind::Modified, ChangeKind::Unmodified),
    ];
    state.apply_snapshot(Box::new(snapshot.clone()));
    state.cursors.files.set(2);
    state.remember_selection();
    assert_eq!(state.sel_file, Some(PathBuf::from("c")));

    snapshot.generation = 2;
    snapshot.files.remove(0);
    state.apply_snapshot(Box::new(snapshot));
    assert_eq!(state.cursors.files.index(), 1);
    assert_eq!(
        state.selected_file().map(|f| f.path.clone()),
        Some(PathBuf::from("c"))
    );
}

#[test]
fn a_directory_row_survives_a_refresh_and_asks_for_its_children_diff() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    let mut snapshot = snapshot_with_generation(1);
    snapshot.files = vec![
        file("src/app/one", ChangeKind::Unmodified, ChangeKind::Modified),
        file("src/app/two", ChangeKind::Unmodified, ChangeKind::Modified),
        file("top", ChangeKind::Unmodified, ChangeKind::Modified),
    ];
    state.focused = PanelId::Files;
    state.apply_snapshot(Box::new(snapshot.clone()));
    // Row 0 is the compressed `src/app` directory, then its two files, then `top`.
    assert_eq!(state.file_tree.len(), 4);
    state.cursors.files.set(0);
    state.remember_selection();
    assert_eq!(state.sel_file, Some(PathBuf::from("src/app")));

    let requests = state.refresh_main();
    assert!(matches!(
        requests.as_slice(),
        [GitRequest::PathsDiff { key, paths }]
            if key == Path::new("src/app") && paths.len() == 2
    ));
    assert!(state.selected_file().is_none(), "a directory is not a file");

    // A new file under the directory must not move the cursor off it.
    snapshot.generation = 2;
    snapshot.files.push(file(
        "src/app/three",
        ChangeKind::Modified,
        ChangeKind::Unmodified,
    ));
    state.apply_snapshot(Box::new(snapshot));
    assert_eq!(state.cursors.files.index(), 0);
    assert_eq!(state.sel_file, Some(PathBuf::from("src/app")));
    let row = state.file_row().expect("the directory row");
    assert!(row.is_dir && row.staged && row.unstaged);
}

fn reflog_entry(oid: &str, selector: &str, subject: &str, committed_at: i64) -> ReflogEntry {
    ReflogEntry {
        oid: ObjectId::from(oid),
        parents: Vec::new(),
        selector: selector.to_owned(),
        subject: subject.to_owned(),
        committed_at,
    }
}

fn stash_entry(oid: &str, index: usize, subject: &str) -> StashEntry {
    StashEntry {
        index,
        oid: ObjectId::from(oid),
        created_at: index as i64,
        subject: subject.to_owned(),
    }
}

#[test]
fn retains_reflog_and_stash_identity() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    let mut snapshot = snapshot_with_generation(1);
    snapshot.reflog = vec![
        reflog_entry("aaa", "HEAD@{0}", "first", 10),
        reflog_entry("bbb", "HEAD@{1}", "selected", 9),
    ];
    snapshot.stashes = vec![
        stash_entry("ccc", 0, "first stash"),
        stash_entry("ddd", 1, "selected stash"),
    ];
    state.apply_snapshot(Box::new(snapshot));

    state.focused = PanelId::Commits;
    state.commit_tab = CommitTab::Reflog;
    state.cursors.reflog.set(1);
    state.remember_selection();
    state.focused = PanelId::Stash;
    state.cursors.stashes.set(1);
    state.remember_selection();

    let mut refreshed = snapshot_with_generation(2);
    refreshed.reflog = vec![
        reflog_entry("eee", "HEAD@{0}", "new", 11),
        reflog_entry("aaa", "HEAD@{1}", "first", 10),
        reflog_entry("bbb", "HEAD@{2}", "selected", 9),
    ];
    refreshed.stashes = vec![
        stash_entry("fff", 0, "new stash"),
        stash_entry("ccc", 1, "first stash"),
        stash_entry("ddd", 2, "selected stash"),
    ];
    state.apply_snapshot(Box::new(refreshed));

    assert_eq!(state.cursors.reflog.index(), 2);
    assert_eq!(
        state.selected_reflog().map(|entry| entry.oid.as_str()),
        Some("bbb")
    );
    assert_eq!(state.cursors.stashes.index(), 2);
    assert_eq!(
        state.selected_stash().map(|entry| entry.oid.as_str()),
        Some("ddd")
    );
}

fn remote_branch(name: &str) -> RemoteBranch {
    RemoteBranch {
        name: format!("origin/{name}"),
        branch: name.to_owned(),
        oid: ObjectId::from(name),
        subject: String::new(),
        committed_at: 0,
    }
}

#[test]
fn restores_remote_branch_by_name() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    state.remote_drill = Some("origin".to_owned());
    state.focused = PanelId::Branches;
    state.branch_tab = BranchTab::Remotes;
    let mut snapshot = snapshot_with_generation(1);
    snapshot.remote_branches = vec![fleet_git::RemoteBranchGroup {
        remote: "origin".to_owned(),
        branches: vec![remote_branch("alpha"), remote_branch("selected")],
    }];
    state.apply_snapshot(Box::new(snapshot));
    state.cursors.remote_branches.set(1);
    state.remember_selection();

    let mut refreshed = snapshot_with_generation(2);
    refreshed.remote_branches = vec![fleet_git::RemoteBranchGroup {
        remote: "origin".to_owned(),
        branches: vec![
            remote_branch("new"),
            remote_branch("alpha"),
            remote_branch("selected"),
        ],
    }];
    state.apply_snapshot(Box::new(refreshed));

    assert_eq!(state.cursors.remote_branches.index(), 2);
    assert_eq!(
        state
            .selected_remote_branch()
            .map(|branch| branch.name.as_str()),
        Some("origin/selected")
    );
}

#[test]
fn refresh_keeps_immutable_commit_view() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    state.apply_snapshot(Box::new(snapshot_with_generation(1)));
    state.focused = PanelId::Main;
    state.main = MainContent::CommitFiles {
        oid: ObjectId::from("immutable"),
        subject: "subject".to_owned(),
        files: Arc::default(),
        whole: None,
        shown: None,
        diff: None,
        whole_error: None,
        diff_error: None,
    };

    assert!(
        state
            .apply_snapshot(Box::new(snapshot_with_generation(2)))
            .is_empty()
    );
    assert!(matches!(
        &state.main,
        MainContent::CommitFiles { oid, .. } if oid.as_str() == "immutable"
    ));

    state.main = MainContent::SubCommits {
        reference: "main".to_owned(),
        commits: Arc::default(),
        shown: None,
        diff: None,
        commits_error: None,
        diff_error: None,
    };
    let mut refreshed = snapshot_with_generation(3);
    refreshed.local_branches.push(Branch {
        name: "main".to_owned(),
        oid: ObjectId::from("tip"),
        is_head: true,
        upstream: Some(fleet_git::Upstream {
            name: "origin/main".to_owned(),
            ahead: 0,
            behind: 0,
            gone: false,
        }),
        subject: String::new(),
        committed_at: 0,
        checked_out_at: None,
    });
    let requests = state.apply_snapshot(Box::new(refreshed));
    assert!(matches!(
        requests.as_slice(),
        [GitRequest::RefCommits { reference, upstream: Some(upstream), .. }]
            if reference == "main" && upstream == "origin/main"
    ));
}

#[test]
fn collapsing_a_directory_moves_the_selection_onto_it() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    let mut snapshot = snapshot_with_generation(1);
    snapshot.files = vec![
        file("src/app/one", ChangeKind::Unmodified, ChangeKind::Modified),
        file("top", ChangeKind::Unmodified, ChangeKind::Modified),
    ];
    state.focused = PanelId::Files;
    state.apply_snapshot(Box::new(snapshot));
    state.cursors.files.set(1);
    state.remember_selection();
    assert_eq!(state.sel_file, Some(PathBuf::from("src/app/one")));

    state.toggle_file_collapsed(Path::new("src/app"));
    assert_eq!(state.cursors.files.index(), 0);
    assert_eq!(state.sel_file, Some(PathBuf::from("src/app")));

    // The flat layout lists every file and no directory.
    state.toggle_file_tree_mode();
    assert_eq!(state.file_tree.len(), 2);
    assert!(state.file_tree.shared_rows().iter().all(|row| !row.is_dir));
}

#[test]
fn the_buffer_edits_by_characters() {
    let mut buffer = Buffer::single_line();
    buffer.insert("héllo");
    assert_eq!(buffer.value(), "héllo");
    assert_eq!(buffer.caret(), 5);
    buffer.left();
    buffer.backspace();
    assert_eq!(buffer.value(), "hélo");
    buffer.insert("\n");
    assert_eq!(buffer.value(), "hélo");
    let mut multi = Buffer::multi_line();
    multi.insert("one\ntwo");
    assert_eq!(multi.lines_with_caret().0.as_ref(), &["one", "two"]);
    multi.delete_to_line_start();
    assert_eq!(multi.value(), "one\n");
}

#[test]
fn buffer_preserves_emoji_graphemes() {
    let mut buffer = Buffer::single_line().with_text("a👍🏽e\u{301}👨‍👩‍👧‍👦z");

    buffer.home();
    for caret in [1, 3, 5, 12, 13] {
        buffer.right();
        assert_eq!(buffer.caret(), caret);
    }

    buffer.left();
    buffer.backspace();
    assert_eq!(buffer.value(), "a👍🏽e\u{301}z");
    buffer.backspace();
    assert_eq!(buffer.value(), "a👍🏽z");
    buffer.backspace();
    assert_eq!(buffer.value(), "az");
}

#[test]
fn time_ago_uses_lazygits_unit_letters() {
    assert_eq!(time_ago(3), "3s");
    assert_eq!(time_ago(120), "2m");
    assert_eq!(time_ago(7_200), "2h");
    assert_eq!(time_ago(172_800), "2d");
    assert_eq!(time_ago(1_209_600), "2w");
}

#[test]
fn a_failing_read_never_clears_the_mutation_guard() {
    let mut state = GitUiState::new(PathBuf::from("/tmp"));
    state.begin_mutation();
    state.begin_read();
    state.begin_read();
    assert!(state.mutation_in_flight());

    // Two reads fail while the mutation is still out; the guard must survive both.
    state.finish_read();
    state.finish_read();
    state.finish_read(); // an extra answer cannot push the counter negative
    assert_eq!(state.pending_reads, 0);
    assert!(state.mutation_in_flight());

    state.finish_mutation();
    assert!(!state.mutation_in_flight());
    state.finish_mutation();
    assert_eq!(state.pending, 0);
}

fn diff_row(file: usize, hunk: Option<usize>, line: Option<usize>, kind: RowKind) -> DiffRow {
    DiffRow {
        kind,
        text: String::new().into(),
        old_no: None,
        new_no: None,
        file,
        hunk,
        line,
        words: Vec::new(),
    }
}

/// Two files, one hunk each, numbered 0 in both.
fn two_file_rows() -> Vec<DiffRow> {
    vec![
        diff_row(0, None, None, RowKind::FileHeader),
        diff_row(0, Some(0), None, RowKind::HunkHeader),
        diff_row(0, Some(0), Some(0), RowKind::Added),
        diff_row(0, Some(0), Some(1), RowKind::Removed),
        diff_row(1, None, None, RowKind::FileHeader),
        diff_row(1, Some(0), None, RowKind::HunkHeader),
        diff_row(1, Some(0), Some(0), RowKind::Added),
    ]
}

#[test]
fn a_hunk_range_stops_at_the_file_boundary() {
    let rows = two_file_rows();
    // Keyed by hunk index alone this would run from row 2 to row 6, across both files.
    assert_eq!(hunk_range(&rows, 2), Some((2, 3)));
    assert_eq!(hunk_range(&rows, 6), Some((6, 6)));
    assert_eq!(hunk_range(&rows, 0), None);
    assert_eq!(hunk_range(&rows, 99), None);
}

#[test]
fn a_selection_covers_one_file_even_when_the_range_spans_two() {
    let rows = two_file_rows();
    let hunks = selection_hunks(&rows, 2, 6);
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].hunk_index, 0);
    // Only the first file's two changed lines, never the second file's line 0.
    assert_eq!(hunks[0].lines.as_deref(), Some(&[0usize, 1][..]));

    let second = selection_hunks(&rows, 6, 6);
    assert_eq!(second[0].lines.as_deref(), Some(&[0usize][..]));
    assert!(selection_hunks(&rows, 0, 1).is_empty());
}

#[test]
fn a_menu_letter_finds_its_row() {
    let item = |key: &str| MenuItem {
        key: key.to_owned(),
        label: key.to_owned(),
        action: MenuAction::Request(Box::new(GitRequest::Snapshot)),
    };
    let menu = Menu::new("Reset options", vec![item("s"), item("m"), item("h")]);
    assert_eq!(
        menu.item_for_key("h").map(|item| item.key.clone()),
        Some("h".to_owned())
    );
    assert!(menu.item_for_key("x").is_none());
}

#[test]
fn recency_prefers_the_checkout_timestamp() {
    let mut branch = Branch {
        name: "feature".to_owned(),
        oid: ObjectId::from("abc"),
        is_head: false,
        upstream: None,
        subject: String::new(),
        committed_at: 0,
        checked_out_at: Some(100),
    };
    assert_eq!(recency(&branch, 160), "1m");
    branch.checked_out_at = None;
    assert_eq!(recency(&branch, 160), "2m");
    branch.is_head = true;
    assert_eq!(recency(&branch, 160), "  *");
}

fn snapshot_with_generation(generation: u64) -> RepoSnapshot {
    RepoSnapshot {
        root: PathBuf::from("/tmp"),
        head: Head::Unborn {
            name: "main".to_owned(),
        },
        operation: OperationState::None,
        files: Vec::new(),
        local_branches: Vec::new(),
        remote_branches: Vec::new(),
        remotes: Vec::new(),
        tags: Vec::new(),
        commits: Vec::new(),
        reflog: Vec::new(),
        stashes: Vec::new(),
        generation,
    }
}

#[test]
fn unchanged_file_snapshots_retain_the_shared_tree_rows() {
    let mut state = GitUiState::new(PathBuf::from("/repo"));
    let mut first = snapshot_with_generation(1);
    first.files.push(file(
        "src/main.rs",
        ChangeKind::Unmodified,
        ChangeKind::Modified,
    ));
    state.apply_snapshot(Box::new(first.clone()));
    let rows = state.file_tree.shared_rows();
    first.generation = 2;
    state.apply_snapshot(Box::new(first));
    assert!(Arc::ptr_eq(&rows, &state.file_tree.shared_rows()));
}

#[test]
fn bulk_deletion_preserves_unicode_boundaries_suffix_and_prepared_lines() {
    let mut buffer = Buffer::multi_line().with_text(format!(
        "first\nprefix\u{2003}{}  tail",
        "é".repeat(100_000)
    ));
    for _ in 0..4 {
        buffer.left();
    }
    assert!(buffer.delete_word());
    assert_eq!(buffer.value(), "first\nprefix\u{2003}tail");
    assert!(buffer.delete_to_line_start());
    assert_eq!(buffer.value(), "first\ntail");
    let (lines, line, column) = buffer.lines_with_caret();
    assert_eq!(lines.as_ref(), &["first", "tail"]);
    assert_eq!((line, column), (1, 0));
    assert!(!buffer.delete_to_line_start());
}
