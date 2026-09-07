use super::*;

pub(crate) fn channel_bridge() -> (GitBridge, Receiver<GitRequest>, Sender<GitEvent>) {
    let (requests, reads) = async_channel::unbounded();
    let (events, receiver) = async_channel::unbounded();
    (
        GitBridge {
            requests,
            events: receiver,
            event_tx: events.clone(),
        },
        reads,
        events,
    )
}

#[test]
fn worker_disconnect_reports_failure() {
    let (bridge, requests, _events) = channel_bridge();
    requests.close();

    bridge.send(GitRequest::Mutate {
        label: "stage".to_owned(),
        mutation: Box::new(Mutation::StageAll),
    });
    bridge.send(GitRequest::FileDiff("changed.txt".into()));

    assert!(matches!(
        bridge.events.try_recv(),
        Ok(GitEvent::Failed { label, message })
            if label == "stage" && message.contains("worker disconnected")
    ));
    assert!(matches!(
        bridge.events.try_recv(),
        Ok(GitEvent::ReadFailed { label, message, .. })
            if label == "diff" && message.contains("worker disconnected")
    ));
}

#[test]
fn newer_reads_replace_queued_work_without_crossing_mutation_or_context_barriers() {
    let mut queue = VecDeque::new();
    assert!(!enqueue(&mut queue, GitRequest::FileDiff("a".into())));
    assert!(enqueue(&mut queue, GitRequest::FileDiff("b".into())));
    assert!(!enqueue(
        &mut queue,
        GitRequest::Mutate {
            label: "stage".into(),
            mutation: Box::new(Mutation::StageAll)
        }
    ));
    assert!(!enqueue(&mut queue, GitRequest::FileDiff("c".into())));
    assert!(enqueue(
        &mut queue,
        GitRequest::PathsDiff {
            key: "d".into(),
            paths: vec!["d/file".into()]
        }
    ));
    assert!(!enqueue(&mut queue, GitRequest::SetDiffContext(5)));
    assert!(!enqueue(&mut queue, GitRequest::FileDiff("e".into())));
    let queued: Vec<_> = queue.into_iter().collect();
    assert!(
        matches!(&queued[..], [GitRequest::FileDiff(b), GitRequest::Mutate { .. }, GitRequest::PathsDiff { key: d, .. }, GitRequest::SetDiffContext(5), GitRequest::FileDiff(e)] if b == Path::new("b") && d == Path::new("d") && e == Path::new("e"))
    );
}

#[test]
fn a_navigation_burst_keeps_only_the_last_read_and_accounts_for_every_replacement() {
    let mut queue = VecDeque::new();
    let superseded = (0..10_000)
        .filter(|index| enqueue(&mut queue, GitRequest::FileDiff(index.to_string().into())))
        .count();
    assert_eq!(superseded, 9999);
    assert_eq!(queue.len(), 1);
    assert!(
        matches!(queue.pop_front(), Some(GitRequest::FileDiff(path)) if path == Path::new("9999"))
    );
}

#[test]
fn immutable_cache_has_a_fixed_retention_bound() {
    let mut cache = ReadCache::default();
    for index in 0..100 {
        let oid = ObjectId::from(format!("{index:040x}"));
        let diff = Arc::new(fleet_git::parse::diff::parse(b"").unwrap());
        cache
            .commits
            .push_front((oid.clone(), diff.clone(), Arc::default()));
        cache.files.push_front((oid, "file".into(), diff));
        cache.trim();
    }
    assert_eq!(cache.commits.len(), ReadCache::CAPACITY);
    assert_eq!(cache.files.len(), ReadCache::CAPACITY);
}

/// Builds a repository with one committed-then-modified file and one untracked file.
fn fixture(name: &str) -> PathBuf {
    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    let root = std::env::temp_dir().join(format!("fleet-lazygit-{name}-{}", std::process::id()));
    let _ignored = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("the fixture root is writable");
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("tracked.txt"), "one\n").expect("write");
    git(&root, &["add", "tracked.txt"]);
    git(&root, &["commit", "-m", "first"]);
    std::fs::write(root.join("tracked.txt"), "two\n").expect("write");
    std::fs::write(root.join("fresh.txt"), "new\n").expect("write");
    root
}

#[tokio::test]
async fn worktree_diffs_reuse_the_snapshot_status_and_still_see_untracked_files() {
    let root = fixture("status-reuse");
    let repository = fleet_git::Repository::discover(&root)
        .await
        .expect("the fixture opens");
    let (events, received) = async_channel::unbounded();
    let helper = PathBuf::from("fleet-lazygit");
    let mut cache = ReadCache::default();

    for request in [
        GitRequest::Snapshot,
        GitRequest::FileDiff("fresh.txt".into()),
        GitRequest::FileDiff("tracked.txt".into()),
    ] {
        assert!(serve(&repository, &helper, request, &events, &mut cache).await);
    }

    let statuses = repository
        .recent_commands()
        .iter()
        .filter(|record| record.display_argv.iter().any(|arg| arg == "status"))
        .count();
    assert_eq!(statuses, 1, "only the snapshot reads status");

    let diffs: Vec<_> = std::iter::from_fn(|| received.try_recv().ok())
        .filter_map(|event| match event {
            GitEvent::FileDiff { path, unstaged, .. } => Some((path, unstaged)),
            _ => None,
        })
        .collect();
    assert_eq!(diffs.len(), 2);
    assert!(
        diffs.iter().all(|(path, unstaged)| unstaged
            .files
            .iter()
            .any(|file| file.new_path.as_deref() == Some(path.as_path()))),
        "the untracked file diffs through --no-index like the tracked one: {diffs:?}"
    );

    let _ignored = std::fs::remove_dir_all(&root);
}
