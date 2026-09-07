use std::path::{Path, PathBuf};

use fleet_git::{CommandOutcome, DiffSide, Ref, SnapshotOptions};

mod support;
use support::{TestRepo, git, git_output};

#[tokio::test]
async fn snapshot_batches_tag_and_remote_metadata() {
    let repo = TestRepo::new().await;
    repo.write("base", "base\n");
    let base = repo.commit("base");
    for index in 0..16 {
        git(repo.path(), &["tag", &format!("lightweight-{index}")]);
        git(
            repo.path(),
            &[
                "remote",
                "add",
                &format!("remote-{index}"),
                &format!("https://example.test/fetch-{index}"),
            ],
        );
        git(
            repo.path(),
            &[
                "remote",
                "set-url",
                "--push",
                &format!("remote-{index}"),
                &format!("https://example.test/push-{index}"),
            ],
        );
    }
    git(
        repo.path(),
        &["tag", "-a", "annotated", "-m", "tag message"],
    );
    let before = repo.repository.recent_commands().last().unwrap().id;
    let snapshot = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(snapshot.tags.len(), 17);
    assert!(snapshot.tags.iter().all(|tag| tag.oid == base));
    assert_eq!(snapshot.remotes.len(), 16);
    for remote in snapshot.remotes {
        let index = remote.name.strip_prefix("remote-").unwrap();
        assert_eq!(
            remote.fetch_url,
            Some(format!("https://example.test/fetch-{index}"))
        );
        assert_eq!(
            remote.push_url,
            Some(format!("https://example.test/push-{index}"))
        );
    }
    let commands: Vec<_> = repo
        .repository
        .recent_commands()
        .into_iter()
        .filter(|record| record.id > before)
        .collect();
    assert_eq!(
        commands
            .iter()
            .filter(|record| record
                .display_argv
                .get(1)
                .is_some_and(|arg| arg == "remote"))
            .count(),
        1
    );
    assert_eq!(
        commands
            .iter()
            .filter(|record| record
                .display_argv
                .last()
                .is_some_and(|arg| arg == "refs/tags"))
            .count(),
        1
    );
    assert!(
        !commands
            .iter()
            .any(|record| record.display_argv.iter().any(|arg| arg.ends_with("^{}")))
    );
}

#[tokio::test]
async fn batched_remotes_keep_git_url_rewrites_and_first_push_url() {
    let repo = TestRepo::new().await;
    git(
        repo.path(),
        &["remote", "add", "origin", "short:project with spaces"],
    );
    git(
        repo.path(),
        &[
            "config",
            "url.https://example.test/fetch/.insteadOf",
            "short:",
        ],
    );
    git(
        repo.path(),
        &[
            "config",
            "url.ssh://example.test/push/.pushInsteadOf",
            "short:",
        ],
    );
    for explicit in [false, true] {
        if explicit {
            git(
                repo.path(),
                &[
                    "remote",
                    "set-url",
                    "--push",
                    "origin",
                    "ssh://first.test/project",
                ],
            );
            git(
                repo.path(),
                &[
                    "remote",
                    "set-url",
                    "--add",
                    "--push",
                    "origin",
                    "ssh://second.test/project",
                ],
            );
        }
        let remotes = repo.remotes().await;
        assert_eq!(remotes.len(), 1);
        assert_eq!(
            remotes[0].fetch_url.as_deref(),
            Some(git_output(repo.path(), &["remote", "get-url", "origin"]).trim())
        );
        assert_eq!(
            remotes[0].push_url.as_deref(),
            Some(git_output(repo.path(), &["remote", "get-url", "--push", "origin"]).trim())
        );
    }
}

#[tokio::test]
async fn unusual_remote_urls_use_the_original_query_contract() {
    let repo = TestRepo::new().await;
    git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://example.test/project\nsecond line",
        ],
    );
    let remotes = repo.remotes().await;
    assert_eq!(
        remotes[0].fetch_url.as_deref(),
        Some(git_output(repo.path(), &["remote", "get-url", "origin"]).trim())
    );
    assert_eq!(remotes[0].push_url, remotes[0].fetch_url);
}

#[tokio::test]
async fn supplied_status_avoids_queries_for_file_and_directory_diffs() {
    let repo = TestRepo::new().await;
    repo.write("tracked", "old\n");
    repo.commit("base");
    repo.write("tracked", "new\n");
    repo.write("untracked", "added\n");
    let paths = [PathBuf::from("tracked"), PathBuf::from("untracked")];
    let status = repo
        .repository
        .snapshot(SnapshotOptions::default())
        .await
        .unwrap()
        .files;
    let expected = repo
        .repository
        .diff_paths(&paths, DiffSide::Unstaged)
        .await
        .unwrap();
    let before = repo.repository.recent_commands().last().unwrap().id;
    let actual = repo
        .repository
        .diff_paths_with_status(&paths, DiffSide::Unstaged, &status)
        .await
        .unwrap();
    assert_eq!(actual, expected);
    for path in paths {
        let diff = repo
            .repository
            .diff_file_with_status(&path, DiffSide::Unstaged, &status)
            .await
            .unwrap();
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].new_path.as_deref(), Some(path.as_path()));
    }
    let commands: Vec<_> = repo
        .repository
        .recent_commands()
        .into_iter()
        .filter(|record| record.id > before)
        .collect();
    assert_eq!(commands.len(), 4);
    assert!(
        commands
            .iter()
            .all(|record| record.display_argv[1] == "diff")
    );
    assert!(
        repo.repository
            .diff_file_with_status(Path::new("missing"), DiffSide::Staged, &status)
            .await
            .unwrap()
            .files
            .is_empty()
    );
}

#[tokio::test]
async fn pushed_classification_is_bounded_by_requested_history() {
    let repo = TestRepo::new().await;
    for index in 0..12 {
        repo.write("history", &format!("{index}\n"));
        repo.commit(&format!("commit {index}"));
    }
    let expected = git_output(repo.path(), &["rev-list", "HEAD~7..HEAD"]);
    for limit in [0, 1, 4, 9, 20] {
        let commits = repo
            .repository
            .commits_for_ref(&Ref::from("HEAD"), Some(&Ref::from("HEAD~7")), limit)
            .await
            .unwrap();
        assert_eq!(commits.len(), limit.min(12));
        for commit in &commits {
            assert_eq!(
                commit.pushed,
                !expected.lines().any(|oid| oid == commit.oid.as_str())
            );
        }
        if limit > 0 {
            let recent = repo.repository.recent_commands();
            let command = recent.last().unwrap();
            assert!(
                command
                    .display_argv
                    .iter()
                    .any(|arg| arg == &format!("--max-count={}", commits.len()))
            );
            let CommandOutcome::Success { stdout_preview, .. } = &command.outcome else {
                panic!("rev-list failed");
            };
            assert!(
                stdout_preview
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .count()
                    <= commits.len()
            );
        }
    }
}

#[tokio::test]
async fn bounded_pushed_classification_matches_full_ancestry_across_merges() {
    let repo = TestRepo::new().await;
    repo.write("tree", "shared tree\n");
    repo.commit("seed");
    let tree = git_output(repo.path(), &["rev-parse", "HEAD^{tree}"]);
    let parents: &[&[usize]] = &[
        &[],
        &[0],
        &[0],
        &[2],
        &[1],
        &[3],
        &[4, 3],
        &[5],
        &[6],
        &[0],
        &[9],
        &[8, 7],
        &[10],
        &[11, 12],
        &[2],
        &[14],
        &[13, 15],
    ];
    let mut oids = Vec::<String>::new();
    for (index, parents) in parents.iter().enumerate() {
        let mut command = std::process::Command::new("git");
        command.current_dir(repo.path()).args([
            "commit-tree",
            tree.trim(),
            "-m",
            &format!("node {index}"),
        ]);
        for parent in *parents {
            command.args(["-p", &oids[*parent]]);
        }
        // Deliberately skew dates so merge traversal cannot rely on monotonic timestamps.
        let date = format!("{} +0000", 1_700_000_000 + (index * 7 % 17) * 100);
        let output = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "commit-tree: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        oids.push(String::from_utf8(output.stdout).unwrap().trim().to_owned());
    }
    let tip = oids.last().unwrap();
    for upstream in [&oids[0], &oids[3], &oids[6], &oids[12], &oids[15], tip] {
        let expected = git_output(repo.path(), &["rev-list", &format!("{upstream}..{tip}")]);
        for limit in [0, 1, 2, 4, 8, 30] {
            let commits = repo
                .repository
                .commits_for_ref(
                    &Ref::from(tip.as_str()),
                    Some(&Ref::from(upstream.as_str())),
                    limit,
                )
                .await
                .unwrap();
            for commit in commits {
                assert_eq!(
                    commit.pushed,
                    !expected.lines().any(|oid| oid == commit.oid.as_str()),
                    "upstream {upstream}, limit {limit}, commit {}",
                    commit.oid
                );
            }
        }
    }
}
