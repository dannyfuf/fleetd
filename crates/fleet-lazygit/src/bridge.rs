//! Serial Git operations with independent watcher and command-log forwarding.

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::channel_bridge;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use async_channel::{Receiver, Sender};
use fleet_git::{
    ChangeEvent, ChangeKind, CommandEvent, Commit, CommitFile, CommitOptions, ConflictChoice,
    ConflictFile, Diff, DiffSide, FetchRequest, FileStatus, MergeOptions, MoveDirection,
    MutationResult, ObjectId, PatchAction, PatchSelection, PullRequest, PushRequest, Ref,
    RepoSnapshot, Repository, ResetMode, SnapshotOptions, StashOptions,
};

/// A mutation the UI asked for. Each variant carries everything the worker needs.
#[derive(Debug, Clone)]
pub enum Mutation {
    /// `git add -- <paths>`.
    Stage(Vec<PathBuf>),
    /// `git restore --staged -- <paths>`.
    Unstage(Vec<PathBuf>),
    /// Stage every change in the worktree.
    StageAll,
    /// Unstage everything in the index.
    UnstageAll,
    /// Discard worktree changes for the given paths.
    Discard(Vec<PathBuf>),
    /// Stage, unstage or discard a hunk/line selection.
    Patch {
        /// The selection to apply.
        selection: PatchSelection,
        /// What to do with it.
        action: PatchAction,
    },
    /// Commit the index.
    Commit {
        /// The commit message.
        message: String,
        /// Commit options (amend and friends).
        options: CommitOptions,
    },
    /// Check out a ref.
    Checkout(Ref),
    /// Create a branch and check it out.
    CheckoutNewBranch {
        /// Branch name.
        name: String,
        /// Optional start point.
        start_point: Option<Ref>,
    },
    /// Delete a local branch.
    DeleteBranch {
        /// Branch name.
        name: String,
        /// Pass `--force`.
        force: bool,
    },
    /// Rename a local branch.
    RenameBranch {
        /// Current name.
        old: String,
        /// New name.
        new: String,
    },
    /// Merge a ref into the checked-out branch.
    Merge(Ref),
    /// Rebase the checked-out branch onto a ref.
    RebaseOnto(Ref),
    /// Continue the in-progress rebase.
    RebaseContinue,
    /// Abort the in-progress rebase.
    RebaseAbort,
    /// Skip the current rebase step.
    RebaseSkip,
    /// Continue the in-progress merge.
    MergeContinue,
    /// Abort the in-progress merge.
    MergeAbort,
    /// Continue the in-progress cherry-pick.
    CherryPickContinue,
    /// Abort the in-progress cherry-pick.
    CherryPickAbort,
    /// Cherry-pick commits onto HEAD.
    CherryPick(Vec<ObjectId>),
    /// Revert one commit.
    Revert(ObjectId),
    /// Reset HEAD onto a ref.
    Reset {
        /// Target ref.
        to: Ref,
        /// Reset mode.
        mode: ResetMode,
    },
    /// Create a tag.
    CreateTag {
        /// Tag name.
        name: String,
        /// Target ref.
        target: Ref,
        /// Annotation message, if any.
        message: Option<String>,
    },
    /// Delete a tag.
    DeleteTag(String),
    /// Check out a remote branch as a local tracking branch.
    CheckoutRemoteBranch {
        /// Remote name.
        remote: String,
        /// Branch name on the remote.
        name: String,
    },
    /// Set a branch's upstream.
    SetUpstream {
        /// Local branch.
        branch: String,
        /// Remote name.
        remote: String,
        /// Remote branch name.
        remote_branch: String,
    },
    /// Remove a branch's upstream.
    UnsetUpstream(String),
    /// Create a stash entry.
    StashPush(StashOptions),
    /// Apply a stash entry.
    StashApply(usize),
    /// Apply and drop a stash entry.
    StashPop(usize),
    /// Drop a stash entry.
    StashDrop(usize),
    /// Create a branch from a stash entry.
    StashBranch {
        /// New branch name.
        name: String,
        /// Stash index.
        index: usize,
    },
    /// `git fetch`.
    Fetch(FetchRequest),
    /// `git pull`.
    Pull(PullRequest),
    /// `git push`.
    Push(PushRequest),
    /// Resolve a conflicted path by taking one side.
    ResolveConflict {
        /// Conflicted path.
        path: PathBuf,
        /// Which side to keep.
        choice: ConflictChoice,
    },
    /// Squash a commit into its parent.
    Squash(ObjectId),
    /// Fixup a commit into its parent.
    Fixup(ObjectId),
    /// Drop a commit.
    DropCommit(ObjectId),
    /// Reword a commit.
    Reword {
        /// The commit.
        oid: ObjectId,
        /// The new message.
        message: String,
    },
    /// Move a commit up or down the todo list.
    MoveCommit {
        /// The commit.
        oid: ObjectId,
        /// Direction; `Up` is toward HEAD.
        direction: MoveDirection,
    },
    /// Stop an interactive rebase at a commit.
    EditCommit(ObjectId),
}

/// A request from the UI to the git thread.
#[derive(Debug, Clone)]
pub enum GitRequest {
    /// Re-read the whole repository.
    Snapshot,
    /// Read both sides of a working-tree file's diff.
    FileDiff(PathBuf),
    /// Read both sides of the combined diff of several paths — a file-tree directory row.
    ///
    /// The answer comes back as [`GitEvent::FileDiff`] keyed by `key`, because the main panel
    /// draws a directory's combined patch exactly as it draws one file's.
    PathsDiff {
        /// The directory the rows belong to; the reply's `path`.
        key: PathBuf,
        /// Every file under it.
        paths: Vec<PathBuf>,
    },
    /// Read a commit's diff and file list.
    CommitDiff(ObjectId),
    /// Read a branch's diff against its merge base with HEAD.
    BranchDiff(String),
    /// Read a ref's commits — lazygit's sub-commits view.
    RefCommits {
        /// The ref to log.
        reference: String,
        /// The ref that decides `Commit::pushed`, when there is one.
        upstream: Option<String>,
        /// How many commits to load.
        limit: usize,
    },
    /// Read one commit's patch restricted to one path.
    CommitFileDiff {
        /// The commit.
        oid: ObjectId,
        /// The path inside it.
        path: PathBuf,
    },
    /// Read a stash entry's diff.
    StashDiff(usize),
    /// Set how many context lines every subsequent diff read asks git for (`{` / `}`).
    SetDiffContext(u32),
    /// Read a conflicted file's sections.
    Conflict(PathBuf),
    /// Run a mutation, then refresh.
    Mutate {
        /// A short human label used in errors and toasts.
        label: String,
        /// The mutation itself.
        mutation: Box<Mutation>,
    },
    /// Stop the worker; the app is quitting.
    Shutdown,
}

/// A message from the git thread to the UI.
#[derive(Debug)]
pub enum GitEvent {
    /// The repository was discovered.
    Opened {
        /// The worktree root.
        root: PathBuf,
    },
    /// The repository could not be opened; the app shows the message and stops.
    OpenFailed {
        /// Why.
        message: String,
    },
    /// A fresh snapshot.
    Snapshot(Box<RepoSnapshot>),
    /// Both sides of a file diff.
    FileDiff {
        /// The path that was read.
        path: PathBuf,
        /// The worktree-vs-index diff.
        unstaged: Arc<Diff>,
        /// The index-vs-HEAD diff.
        staged: Arc<Diff>,
    },
    /// A commit's diff plus its file list.
    CommitDiff {
        /// The commit.
        oid: ObjectId,
        /// Its diff.
        diff: Arc<Diff>,
        /// Its changed files.
        files: Arc<[CommitFile]>,
    },
    /// A branch's diff.
    BranchDiff {
        /// Branch name.
        name: String,
        /// Its diff.
        diff: Arc<Diff>,
    },
    /// A ref's commits.
    RefCommits {
        /// The ref that was logged.
        reference: String,
        /// Its commits, newest first.
        commits: Arc<[Commit]>,
    },
    /// One commit's patch for one path.
    CommitFileDiff {
        /// The commit.
        oid: ObjectId,
        /// The path.
        path: PathBuf,
        /// Its patch.
        diff: Arc<Diff>,
    },
    /// A stash entry's diff.
    StashDiff {
        /// Stash index.
        index: usize,
        /// Its diff.
        diff: Arc<Diff>,
    },
    /// A conflicted file's parsed sections.
    Conflict {
        /// Conflicted path.
        path: PathBuf,
        /// The parsed file.
        file: Arc<ConflictFile>,
    },
    /// A mutation succeeded.
    Mutated {
        /// The label the request carried.
        label: String,
        /// Notable output from a successful command.
        warning: Option<String>,
    },
    /// A **mutation** failed. Always followed by a fresh snapshot.
    Failed {
        /// The label the request carried.
        label: String,
        /// The git error, stderr included.
        message: String,
    },
    /// A **read** failed.
    ///
    /// A separate variant from [`GitEvent::Failed`] on purpose: a failing diff must not clear the
    /// in-progress guard a mutation armed, and must not fire a mutation's escalation dialog.
    ReadFailed {
        /// Which read.
        label: String,
        /// The git error, stderr included.
        message: String,
    },
    /// A queued read was replaced before execution.
    ReadSuperseded,
    /// One command-log event.
    Command(Box<CommandEvent>),
    /// The command-log broadcast dropped events; the log is re-seeded from `recent_commands`.
    CommandsReseeded(Vec<fleet_git::CommandRecord>),
    /// The watcher saw the worktree change.
    Changed(Box<ChangeEvent>),
}

/// The handle the UI keeps. Cloning it is cheap and safe from any thread.
#[derive(Clone, Debug)]
pub struct GitBridge {
    requests: Sender<GitRequest>,
    events: Receiver<GitEvent>,
}

impl GitBridge {
    /// Parks a current-thread Tokio runtime on one background thread and opens the repository.
    ///
    /// Every Git operation is a child process the worker awaits, so one thread multiplexes them
    /// all; only `tokio::fs` probes need real threads, and the runtime caps those at two.
    #[must_use]
    pub fn start(path: PathBuf) -> Self {
        let (requests, request_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::unbounded();
        let thread_events = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-lazygit-git".to_owned())
            .spawn(move || run_thread(path, request_rx, thread_events))
        {
            let _ignored = event_tx.try_send(GitEvent::OpenFailed {
                message: format!("could not start the git thread: {error}"),
            });
        }
        Self { requests, events }
    }

    /// The stream the root view drains in one `cx.spawn` loop.
    #[must_use]
    pub fn events(&self) -> Receiver<GitEvent> {
        self.events.clone()
    }

    /// Fire-and-forget: every outcome arrives as an event.
    pub fn send(&self, request: GitRequest) {
        let _ignored = self.requests.try_send(request);
    }
}

/// Builds the runtime and blocks on the worker loop.
fn run_thread(path: PathBuf, requests: Receiver<GitRequest>, events: Sender<GitEvent>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ignored = events.try_send(GitEvent::OpenFailed {
                message: format!("could not start the git runtime: {error}"),
            });
            return;
        }
    };
    runtime.block_on(run(path, &requests, &events));
}

async fn run(path: PathBuf, requests: &Receiver<GitRequest>, events: &Sender<GitEvent>) {
    let repository = match Repository::discover(&path).await {
        Ok(repository) => repository,
        Err(error) => {
            let _ignored = events
                .send(GitEvent::OpenFailed {
                    message: error.to_string(),
                })
                .await;
            return;
        }
    };
    if events
        .send(GitEvent::Opened {
            root: repository.paths().worktree_root.clone(),
        })
        .await
        .is_err()
    {
        return;
    }

    let helper = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fleet-lazygit"));
    let mut commands = repository.subscribe_commands();
    let watcher = fleet_git::watch::RepoWatcher::new(repository.paths());
    let (_watcher, changes) = match watcher {
        Ok((watcher, changes)) => (Some(watcher), Some(changes)),
        Err(error) => {
            tracing::warn!(%error, "fleet-lazygit: the filesystem watcher did not start");
            (None, None)
        }
    };

    let worker = process_requests(&repository, &helper, requests, events);
    tokio::pin!(worker);
    loop {
        let change_next = async {
            match &changes {
                Some(changes) => changes.recv().await.ok(),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = &mut worker => return,
            command = commands.recv() => match command {
                Ok(event) => {
                    if events.send(GitEvent::Command(Box::new(event))).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // A lagged broadcast leaves a hole no later event can fill; re-seed instead.
                    let records = repository.recent_commands();
                    if events.send(GitEvent::CommandsReseeded(records)).await.is_err() {
                        return;
                    }
                }
            },
            change = change_next => match change {
                Some(change) => {
                    if events.send(GitEvent::Changed(Box::new(change))).await.is_err() {
                        return;
                    }
                }
                None => return,
            },
        }
    }
}

/// Coalesce only within a run of reads. Mutations, context changes and shutdown are barriers.
fn enqueue(queue: &mut VecDeque<GitRequest>, request: GitRequest) -> bool {
    let mut superseded = false;
    if let Some(key) = read_key(&request) {
        let previous = queue
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, pending)| read_key(pending).is_some())
            .find(|(_, pending)| read_key(pending) == Some(key))
            .map(|(index, _)| index);
        if let Some(index) = previous {
            queue.remove(index);
            superseded = key != 0;
        }
    }
    queue.push_back(request);
    superseded
}

fn read_key(request: &GitRequest) -> Option<u8> {
    match request {
        GitRequest::Snapshot => Some(0),
        GitRequest::FileDiff(_) | GitRequest::PathsDiff { .. } => Some(1),
        GitRequest::CommitDiff(_) => Some(2),
        GitRequest::BranchDiff(_) => Some(3),
        GitRequest::RefCommits { .. } => Some(4),
        GitRequest::CommitFileDiff { .. } => Some(5),
        GitRequest::StashDiff(_) => Some(6),
        GitRequest::Conflict(_) => Some(7),
        _ => None,
    }
}

async fn process_requests(
    repository: &Repository,
    helper: &Path,
    requests: &Receiver<GitRequest>,
    events: &Sender<GitEvent>,
) {
    let mut queue = VecDeque::new();
    let mut cache = ReadCache::default();
    loop {
        if queue.is_empty() {
            let Ok(request) = requests.recv().await else {
                return;
            };
            queue.push_back(request);
        }
        let mut superseded = 0;
        for _ in 0..256 {
            let Ok(request) = requests.try_recv() else {
                break;
            };
            superseded += usize::from(enqueue(&mut queue, request));
        }
        for _ in 0..superseded {
            if events.send(GitEvent::ReadSuperseded).await.is_err() {
                return;
            }
        }
        let Some(request) = queue.pop_front() else {
            continue;
        };
        if !serve(repository, helper, request, events, &mut cache).await {
            return;
        }
    }
}

#[derive(Default)]
struct ReadCache {
    commits: VecDeque<(ObjectId, Arc<Diff>, Arc<[CommitFile]>)>,
    files: VecDeque<(ObjectId, PathBuf, Arc<Diff>)>,
    /// The untracked entries of the last snapshot this worker sent, or `None` before the first.
    ///
    /// A worktree diff only consults status to tell an untracked path from a tracked one, so the
    /// untracked subset is the whole input. Requests are served in order, so this is exactly the
    /// status the UI selected the path from — the refresh boundary `diff_file_with_status`
    /// documents — and reusing it drops one `git status` per diff request.
    untracked: Option<Vec<FileStatus>>,
}

impl ReadCache {
    const CAPACITY: usize = 8;

    fn trim(&mut self) {
        self.commits.truncate(Self::CAPACITY);
        self.files.truncate(Self::CAPACITY);
    }
}

/// Serves one request. Returns `false` when the UI has gone away.
async fn serve(
    repository: &Repository,
    helper: &Path,
    request: GitRequest,
    events: &Sender<GitEvent>,
    cache: &mut ReadCache,
) -> bool {
    match request {
        GitRequest::Shutdown => false,
        GitRequest::Snapshot => send_snapshot(repository, events, cache).await,
        GitRequest::FileDiff(path) => {
            let (unstaged, staged) = match &cache.untracked {
                Some(status) => (
                    repository
                        .diff_file_with_status(&path, DiffSide::Unstaged, status)
                        .await,
                    repository
                        .diff_file_with_status(&path, DiffSide::Staged, status)
                        .await,
                ),
                None => (
                    repository.diff_file(&path, DiffSide::Unstaged).await,
                    repository.diff_file(&path, DiffSide::Staged).await,
                ),
            };
            send_file_diff(events, path, unstaged, staged).await
        }
        GitRequest::PathsDiff { key, paths } => {
            let (unstaged, staged) = match &cache.untracked {
                Some(status) => (
                    repository
                        .diff_paths_with_status(&paths, DiffSide::Unstaged, status)
                        .await,
                    repository
                        .diff_paths_with_status(&paths, DiffSide::Staged, status)
                        .await,
                ),
                None => (
                    repository.diff_paths(&paths, DiffSide::Unstaged).await,
                    repository.diff_paths(&paths, DiffSide::Staged).await,
                ),
            };
            send_file_diff(events, key, unstaged, staged).await
        }
        GitRequest::CommitDiff(oid) => {
            if let Some((_, diff, files)) =
                cache.commits.iter().find(|(cached, _, _)| *cached == oid)
            {
                return events
                    .send(GitEvent::CommitDiff {
                        oid,
                        diff: diff.clone(),
                        files: files.clone(),
                    })
                    .await
                    .is_ok();
            }
            let diff = repository.diff_commit(&oid, &[]).await;
            let files = repository.commit_files(&oid).await;
            match (diff, files) {
                (Ok(diff), Ok(files)) => {
                    let diff = Arc::new(diff);
                    let files: Arc<[CommitFile]> = files.into();
                    cache
                        .commits
                        .push_front((oid.clone(), diff.clone(), files.clone()));
                    cache.trim();
                    events
                        .send(GitEvent::CommitDiff { oid, diff, files })
                        .await
                        .is_ok()
                }
                (Err(error), _) | (_, Err(error)) => {
                    fail(events, "commit diff", &describe(&error)).await
                }
            }
        }
        GitRequest::BranchDiff(name) => {
            match repository.diff_branch(&Ref::from(name.clone())).await {
                Ok(diff) => events
                    .send(GitEvent::BranchDiff {
                        name,
                        diff: Arc::new(diff),
                    })
                    .await
                    .is_ok(),
                Err(error) => fail(events, "branch diff", &describe(&error)).await,
            }
        }
        GitRequest::RefCommits {
            reference,
            upstream,
            limit,
        } => {
            let upstream = upstream.map(Ref::from);
            match repository
                .commits_for_ref(&Ref::from(reference.clone()), upstream.as_ref(), limit)
                .await
            {
                Ok(commits) => events
                    .send(GitEvent::RefCommits {
                        reference,
                        commits: commits.into(),
                    })
                    .await
                    .is_ok(),
                Err(error) => fail(events, "ref commits", &describe(&error)).await,
            }
        }
        GitRequest::CommitFileDiff { oid, path } => {
            if let Some((_, _, diff)) = cache
                .files
                .iter()
                .find(|(cached, cached_path, _)| *cached == oid && *cached_path == path)
            {
                return events
                    .send(GitEvent::CommitFileDiff {
                        oid,
                        path,
                        diff: diff.clone(),
                    })
                    .await
                    .is_ok();
            }
            match repository
                .diff_commit(&oid, std::slice::from_ref(&path))
                .await
            {
                Ok(diff) => {
                    let diff = Arc::new(diff);
                    cache
                        .files
                        .push_front((oid.clone(), path.clone(), diff.clone()));
                    cache.trim();
                    events
                        .send(GitEvent::CommitFileDiff { oid, path, diff })
                        .await
                        .is_ok()
                }
                Err(error) => fail(events, "commit file diff", &describe(&error)).await,
            }
        }
        GitRequest::SetDiffContext(lines) => {
            // Widening the context invalidates every stored patch, but not the status behind them.
            cache.commits.clear();
            cache.files.clear();
            repository.set_diff_context(lines);
            true
        }
        GitRequest::StashDiff(index) => match repository.diff_stash(index).await {
            Ok(diff) => events
                .send(GitEvent::StashDiff {
                    index,
                    diff: Arc::new(diff),
                })
                .await
                .is_ok(),
            Err(error) => fail(events, "stash diff", &describe(&error)).await,
        },
        GitRequest::Conflict(path) => match repository.conflicted_file(&path).await {
            Ok(file) => events
                .send(GitEvent::Conflict {
                    path,
                    file: Arc::new(file),
                })
                .await
                .is_ok(),
            Err(error) => fail(events, "conflict", &describe(&error)).await,
        },
        GitRequest::Mutate { label, mutation } => {
            let result = apply(repository, helper, *mutation).await;
            let event = match result {
                Ok(result) => GitEvent::Mutated {
                    label,
                    warning: result.warning,
                },
                Err(error) => GitEvent::Failed {
                    label,
                    message: describe(&error),
                },
            };
            if events.send(event).await.is_err() {
                return false;
            }
            // Refresh after every mutation, successful or not: a failed command can still have
            // moved the index (a partially applied patch, a merge that left conflicts).
            send_snapshot(repository, events, cache).await
        }
    }
}

async fn send_file_diff(
    events: &Sender<GitEvent>,
    path: PathBuf,
    unstaged: fleet_git::Result<Diff>,
    staged: fleet_git::Result<Diff>,
) -> bool {
    match (unstaged, staged) {
        (Ok(unstaged), Ok(staged)) => events
            .send(GitEvent::FileDiff {
                path,
                unstaged: Arc::new(unstaged),
                staged: Arc::new(staged),
            })
            .await
            .is_ok(),
        (Err(error), _) | (_, Err(error)) => fail(events, "diff", &describe(&error)).await,
    }
}

async fn send_snapshot(
    repository: &Repository,
    events: &Sender<GitEvent>,
    cache: &mut ReadCache,
) -> bool {
    match repository.snapshot(SnapshotOptions::default()).await {
        Ok(snapshot) => {
            cache.untracked = Some(
                snapshot
                    .files
                    .iter()
                    .filter(|file| file.worktree == ChangeKind::Untracked)
                    .cloned()
                    .collect(),
            );
            events
                .send(GitEvent::Snapshot(Box::new(snapshot)))
                .await
                .is_ok()
        }
        Err(error) => fail(events, "refresh", &describe(&error)).await,
    }
}

/// What the UI is told about a failure.
///
/// `GitError`'s own `Display` leads with the redacted argv, which is the least interesting part
/// in a one-row error slot, so a failure that carries Git's own output reports that instead.
/// Everything else (spawn failures, timeouts, parse errors) keeps its full message.
fn describe(error: &fleet_git::GitError) -> String {
    match error {
        fleet_git::GitError::Exit { message, .. }
        | fleet_git::GitError::Conflict { message, .. }
            if !message.trim().is_empty() =>
        {
            message.trim().to_owned()
        }
        other => other.to_string(),
    }
}

/// Reports a failed **read**. Mutations report through [`GitEvent::Failed`] instead, because only
/// they own an in-progress guard and an escalation dialog.
async fn fail(events: &Sender<GitEvent>, label: &str, message: &str) -> bool {
    events
        .send(GitEvent::ReadFailed {
            label: label.to_owned(),
            message: message.to_owned(),
        })
        .await
        .is_ok()
}

async fn apply(
    repository: &Repository,
    helper: &Path,
    mutation: Mutation,
) -> fleet_git::Result<MutationResult> {
    match mutation {
        Mutation::Stage(paths) => repository.stage_paths(&paths).await,
        Mutation::Unstage(paths) => repository.unstage_paths(&paths).await,
        Mutation::StageAll => repository.stage_all().await,
        Mutation::UnstageAll => repository.unstage_all().await,
        Mutation::Discard(paths) => repository.discard_paths(&paths).await,
        Mutation::Patch { selection, action } => {
            repository.apply_patch_selection(selection, action).await
        }
        Mutation::Commit { message, options } => repository.commit(&message, options).await,
        Mutation::Checkout(reference) => repository.checkout(&reference).await,
        Mutation::CheckoutNewBranch { name, start_point } => {
            repository
                .checkout_new_branch(&name, start_point.as_ref())
                .await
        }
        Mutation::DeleteBranch { name, force } => repository.delete_branch(&name, force).await,
        Mutation::RenameBranch { old, new } => repository.rename_branch(&old, &new).await,
        Mutation::Merge(reference) => repository.merge(&reference, MergeOptions::default()).await,
        Mutation::RebaseOnto(reference) => repository.rebase_onto(&reference).await,
        Mutation::RebaseContinue => repository.rebase_continue().await,
        Mutation::RebaseAbort => repository.rebase_abort().await,
        Mutation::RebaseSkip => repository.rebase_skip().await,
        Mutation::MergeContinue => repository.merge_continue().await,
        Mutation::MergeAbort => repository.merge_abort().await,
        Mutation::CherryPickContinue => repository.cherry_pick_continue().await,
        Mutation::CherryPickAbort => repository.cherry_pick_abort().await,
        Mutation::CherryPick(oids) => repository.cherry_pick(&oids).await,
        Mutation::Revert(oid) => repository.revert(&oid).await,
        Mutation::Reset { to, mode } => repository.reset(&to, mode).await,
        Mutation::CreateTag {
            name,
            target,
            message,
        } => {
            repository
                .create_tag(&name, &target, message.as_deref())
                .await
        }
        Mutation::DeleteTag(name) => repository.delete_tag(&name).await,
        Mutation::CheckoutRemoteBranch { remote, name } => {
            repository.checkout_remote_branch(&remote, &name).await
        }
        Mutation::SetUpstream {
            branch,
            remote,
            remote_branch,
        } => {
            repository
                .set_upstream(&branch, &remote, &remote_branch)
                .await
        }
        Mutation::UnsetUpstream(branch) => repository.unset_upstream(&branch).await,
        Mutation::StashPush(options) => repository.stash_push(options).await,
        Mutation::StashApply(index) => repository.stash_apply(index).await,
        Mutation::StashPop(index) => repository.stash_pop(index).await,
        Mutation::StashDrop(index) => repository.stash_drop(index).await,
        Mutation::StashBranch { name, index } => repository.stash_branch(&name, index).await,
        Mutation::Fetch(request) => repository.fetch(request).await,
        Mutation::Pull(request) => repository.pull(request).await,
        Mutation::Push(request) => repository.push(request).await,
        Mutation::ResolveConflict { path, choice } => {
            repository.resolve_conflict(&path, choice).await
        }
        Mutation::Squash(oid) => repository.squash_into_previous(&oid, helper).await,
        Mutation::Fixup(oid) => repository.fixup_into_previous(&oid, helper).await,
        Mutation::DropCommit(oid) => repository.drop_commit(&oid, helper).await,
        Mutation::Reword { oid, message } => repository.reword_commit(&oid, &message, helper).await,
        Mutation::MoveCommit { oid, direction } => {
            repository.move_commit(&oid, direction, helper).await
        }
        Mutation::EditCommit(oid) => repository.edit_commit(&oid, helper).await,
    }
}
