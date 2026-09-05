//! The bridge between gpui's foreground executor and `fleet-git`'s Tokio runtime.
//!
//! gpui owns the main thread and is not a Tokio runtime; every `fleet_git::Repository` method is
//! `async` and shells out to `git`. [`GitBridge::start`] therefore parks a multi-threaded Tokio
//! runtime on **one** background thread, owns the `Repository` and its [`RepoWatcher`] there, and
//! exchanges two `async-channel` streams with the UI:
//!
//! ```text
//!   gpui foreground                background thread (tokio)
//!   ──────────────                 ─────────────────────────
//!   GitBridge::send(request) ────▶ Repository::… ──▶ git
//!   GitUiState  ◀──GitEvent─────── snapshots, diffs, command log, fs changes
//! ```
//!
//! Nothing else in the crate talks to `fleet-git`, and the UI thread never blocks: every send is
//! a `try_send` on an unbounded channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use async_channel::{Receiver, Sender};
use fleet_git::{
    ChangeEvent, CommandEvent, Commit, CommitFile, CommitOptions, ConflictChoice, ConflictFile,
    Diff, DiffSide, FetchRequest, MergeOptions, MoveDirection, MutationResult, ObjectId,
    PatchAction, PatchSelection, PullRequest, PushRequest, Ref, RepoSnapshot, Repository,
    ResetMode, SnapshotOptions, StashOptions,
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
    /// Discard every worktree change.
    DiscardAll,
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
        files: Vec<CommitFile>,
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
        commits: Vec<Commit>,
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
    /// Parks a multi-threaded Tokio runtime on one background thread and opens the repository.
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
    let runtime = match tokio::runtime::Builder::new_multi_thread()
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

    loop {
        let change_next = async {
            match &changes {
                Some(changes) => changes.recv().await.ok(),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            request = requests.recv() => match request {
                Ok(GitRequest::Shutdown) | Err(_) => return,
                Ok(request) => {
                    if !serve(&repository, &helper, request, events).await {
                        return;
                    }
                }
            },
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

/// Serves one request. Returns `false` when the UI has gone away.
async fn serve(
    repository: &Repository,
    helper: &Path,
    request: GitRequest,
    events: &Sender<GitEvent>,
) -> bool {
    match request {
        GitRequest::Shutdown => false,
        GitRequest::Snapshot => send_snapshot(repository, events).await,
        GitRequest::FileDiff(path) => {
            let unstaged = repository.diff_file(&path, DiffSide::Unstaged).await;
            let staged = repository.diff_file(&path, DiffSide::Staged).await;
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
        GitRequest::PathsDiff { key, paths } => {
            let unstaged = repository.diff_paths(&paths, DiffSide::Unstaged).await;
            let staged = repository.diff_paths(&paths, DiffSide::Staged).await;
            match (unstaged, staged) {
                (Ok(unstaged), Ok(staged)) => events
                    .send(GitEvent::FileDiff {
                        path: key,
                        unstaged: Arc::new(unstaged),
                        staged: Arc::new(staged),
                    })
                    .await
                    .is_ok(),
                (Err(error), _) | (_, Err(error)) => fail(events, "diff", &describe(&error)).await,
            }
        }
        GitRequest::CommitDiff(oid) => {
            let diff = repository.diff_commit(&oid, &[]).await;
            let files = repository.commit_files(&oid).await;
            match (diff, files) {
                (Ok(diff), Ok(files)) => events
                    .send(GitEvent::CommitDiff {
                        oid,
                        diff: Arc::new(diff),
                        files,
                    })
                    .await
                    .is_ok(),
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
                    .send(GitEvent::RefCommits { reference, commits })
                    .await
                    .is_ok(),
                Err(error) => fail(events, "ref commits", &describe(&error)).await,
            }
        }
        GitRequest::CommitFileDiff { oid, path } => {
            match repository
                .diff_commit(&oid, std::slice::from_ref(&path))
                .await
            {
                Ok(diff) => events
                    .send(GitEvent::CommitFileDiff {
                        oid,
                        path,
                        diff: Arc::new(diff),
                    })
                    .await
                    .is_ok(),
                Err(error) => fail(events, "commit file diff", &describe(&error)).await,
            }
        }
        GitRequest::SetDiffContext(lines) => {
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
            send_snapshot(repository, events).await
        }
    }
}

async fn send_snapshot(repository: &Repository, events: &Sender<GitEvent>) -> bool {
    match repository.snapshot(SnapshotOptions::default()).await {
        Ok(snapshot) => events
            .send(GitEvent::Snapshot(Box::new(snapshot)))
            .await
            .is_ok(),
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
        Mutation::DiscardAll => repository.discard_all().await,
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
