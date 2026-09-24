//! The git worker behind the Workspace's Changes panel.
//!
//! The panel shows what a worktree changed against the ref it was created from — the files,
//! committed or not, and the commits ahead — and a file's diff on request. Every read is a
//! `fleet-git` child process, so, like [`crate::root::Lazygit`]'s own worker, it runs on one
//! named thread parked on a current-thread Tokio runtime, and the UI only ever sees events.
//!
//! The worker refreshes by itself: once when it starts, then after every burst of filesystem
//! changes the repository watcher reports, [`CHANGES_DEBOUNCE`] after the burst goes quiet. An
//! embedder may also ask with [`ChangesWorker::refresh`] — Fleet does so on its worktree-status
//! cadence, so a watcher that could not start still leaves a panel that catches up.
//!
//! Dropping the last [`ChangesWorker`] closes its request channel, which ends the thread.

use std::{path::PathBuf, thread, time::Duration};

use async_channel::{Receiver, Sender};
use fleet_git::Repository;
pub use fleet_git::{AheadCommit, BranchChanges, BranchFile, DiffKind, ObjectId, Ref};

/// How long the watcher must stay quiet before a burst of changes becomes one refresh.
///
/// An agent's edit, a `cargo fmt`, a checkout: each touches many files within a few
/// milliseconds, and the panel needs to be read once after the burst, not once per file.
pub const CHANGES_DEBOUNCE: Duration = Duration::from_millis(300);

/// How many ahead-of-base commits a reading lists. The panel says how many there are in total.
pub const COMMIT_LIMIT: usize = 5;

/// Outstanding events the worker may queue before it waits for the UI.
///
/// Readings supersede each other and the UI drains the channel in one loop, so a handful is
/// already more than a live panel ever holds; a full queue only pauses the worker.
const EVENT_CAPACITY: usize = 8;

/// What the worker reports.
#[derive(Debug, Clone)]
pub enum ChangesEvent {
    /// A fresh reading. `Ok(None)` when the base names no commit in this repository.
    Loaded(Result<Option<BranchChanges>, String>),
    /// One path's unified diff against the base, as [`ChangesWorker::file_diff`] asked.
    FileDiff {
        /// The path the diff is for.
        path: PathBuf,
        /// The unified text; `Ok(None)` when the base names no commit.
        result: Result<Option<String>, String>,
    },
}

#[derive(Debug)]
enum ChangesRequest {
    Refresh,
    FileDiff(PathBuf),
}

/// The handle an embedder keeps for as long as it shows one worktree's changes.
#[derive(Debug)]
pub struct ChangesWorker {
    requests: Sender<ChangesRequest>,
    events: Receiver<ChangesEvent>,
}

impl ChangesWorker {
    /// Starts reading `path`'s changes against `base` on a thread of its own.
    #[must_use]
    pub fn start(path: PathBuf, base: String) -> Self {
        // Unbounded on purpose: requests are a refresh or a diff for a click, a person's pace,
        // and a refused send would have nowhere better to go.
        let (requests, request_rx) = async_channel::unbounded();
        let (event_tx, events) = async_channel::bounded(EVENT_CAPACITY);
        let failed = event_tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("fleet-changes-git".to_owned())
            .spawn(move || run_thread(path, Ref(base), &request_rx, &event_tx))
            && failed
                .try_send(ChangesEvent::Loaded(Err(format!(
                    "could not start the git thread: {error}"
                ))))
                .is_err()
        {
            tracing::warn!("changes: the worker thread did not start and nobody is listening");
        }
        Self { requests, events }
    }

    /// The stream the embedder drains in one foreground loop.
    #[must_use]
    pub fn events(&self) -> Receiver<ChangesEvent> {
        self.events.clone()
    }

    /// Reads the changes again now, whether or not the watcher saw anything.
    pub fn refresh(&self) {
        self.send(ChangesRequest::Refresh);
    }

    /// Reads one path's diff against the base; the answer arrives as [`ChangesEvent::FileDiff`].
    pub fn file_diff(&self, path: PathBuf) {
        self.send(ChangesRequest::FileDiff(path));
    }

    fn send(&self, request: ChangesRequest) {
        // Unbounded, so the only refusal is a worker that already stopped — its thread failed to
        // start or its repository could not be opened, both reported as a `Loaded` error.
        if let Err(error) = self.requests.try_send(request) {
            tracing::debug!(request = ?error.into_inner(), "changes: the worker has stopped");
        }
    }
}

fn run_thread(
    path: PathBuf,
    base: Ref,
    requests: &Receiver<ChangesRequest>,
    events: &Sender<ChangesEvent>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            report(
                events,
                ChangesEvent::Loaded(Err(format!("could not start the git runtime: {error}"))),
            );
            return;
        }
    };
    runtime.block_on(run(path, base, requests, events));
}

/// Sends from the worker thread's synchronous edges, where nothing awaits.
fn report(events: &Sender<ChangesEvent>, event: ChangesEvent) {
    if events.try_send(event).is_err() {
        tracing::debug!("changes: the panel went away before the worker could report");
    }
}

async fn run(
    path: PathBuf,
    base: Ref,
    requests: &Receiver<ChangesRequest>,
    events: &Sender<ChangesEvent>,
) {
    let repository = match Repository::discover(&path).await {
        Ok(repository) => repository,
        Err(error) => {
            report(events, ChangesEvent::Loaded(Err(error.to_string())));
            return;
        }
    };
    let (_watcher, changes) = match fleet_git::watch::RepoWatcher::new(repository.paths()) {
        Ok((watcher, changes)) => (Some(watcher), Some(changes)),
        Err(error) => {
            // The embedder's own refresh cadence still reaches the panel.
            tracing::warn!(%error, "changes: the filesystem watcher did not start");
            (None, None)
        }
    };
    if !load(&repository, &base, events).await {
        return;
    }
    loop {
        let changed = async {
            match &changes {
                Some(changes) => changes.recv().await.ok(),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            // A request comes first: a closed request channel is the embedder going away, and a
            // click's diff should not wait behind a debounce.
            biased;
            request = requests.recv() => match request {
                Ok(ChangesRequest::Refresh) => {
                    if !load(&repository, &base, events).await {
                        return;
                    }
                }
                Ok(ChangesRequest::FileDiff(path)) => {
                    let result = repository
                        .branch_file_diff(&base, &path)
                        .await
                        .map_err(|error| error.to_string());
                    if events.send(ChangesEvent::FileDiff { path, result }).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            change = changed => {
                if change.is_none() {
                    return;
                }
                // Wait for the burst to go quiet: every change inside the window restarts it.
                if let Some(changes) = &changes {
                    loop {
                        match tokio::time::timeout(CHANGES_DEBOUNCE, changes.recv()).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(_)) => return,
                            Err(_) => break,
                        }
                    }
                }
                if !load(&repository, &base, events).await {
                    return;
                }
            }
        }
    }
}

/// Reads and reports the changes once. `false` when the embedder has gone away.
async fn load(repository: &Repository, base: &Ref, events: &Sender<ChangesEvent>) -> bool {
    let result = repository
        .branch_changes(base, COMMIT_LIMIT)
        .await
        .map_err(|error| error.to_string());
    events.send(ChangesEvent::Loaded(result)).await.is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap_or_else(|error| panic!("git runs: {error}"));
        assert!(status.success(), "git {args:?}");
    }

    /// A repository on `feature` one commit ahead of `main`, with one uncommitted edit.
    fn fixture(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("fleet-changes-{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        }
        std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        git(&root, &["init", "--initial-branch=main"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "user.name", "Test"]);
        std::fs::write(root.join("README.md"), "one\n").unwrap_or_else(|error| panic!("{error}"));
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-m", "first"]);
        git(&root, &["checkout", "-b", "feature"]);
        std::fs::write(root.join("run.rs"), "fn run() {}\n")
            .unwrap_or_else(|error| panic!("{error}"));
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-m", "add the runner"]);
        std::fs::write(root.join("README.md"), "one\ntwo\n")
            .unwrap_or_else(|error| panic!("{error}"));
        root
    }

    fn next_reading(events: &Receiver<ChangesEvent>) -> Option<BranchChanges> {
        loop {
            match events.recv_blocking() {
                Ok(ChangesEvent::Loaded(result)) => {
                    return result.unwrap_or_else(|error| panic!("{error}"));
                }
                Ok(ChangesEvent::FileDiff { .. }) => {}
                Err(error) => panic!("the worker stopped: {error}"),
            }
        }
    }

    fn paths(changes: &BranchChanges) -> Vec<String> {
        changes
            .files
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_worker_reads_on_start_on_request_and_answers_a_file_diff() {
        let root = fixture("worker");
        let worker = ChangesWorker::start(root.clone(), "main".to_owned());
        let events = worker.events();

        let first = next_reading(&events).unwrap_or_else(|| panic!("main resolves"));
        assert_eq!(paths(&first), ["README.md", "run.rs"]);
        assert_eq!(first.ahead, 1);
        assert_eq!(first.commits[0].subject, "add the runner");

        std::fs::write(root.join("notes.md"), "n\n").unwrap_or_else(|error| panic!("{error}"));
        worker.refresh();
        // The watcher may report the write first; either way a later reading lists the file.
        let mut reading = next_reading(&events).unwrap_or_else(|| panic!("main resolves"));
        while !paths(&reading).contains(&"notes.md".to_owned()) {
            reading = next_reading(&events).unwrap_or_else(|| panic!("main resolves"));
        }

        worker.file_diff(PathBuf::from("README.md"));
        let diff = loop {
            match events.recv_blocking() {
                Ok(ChangesEvent::FileDiff { path, result }) => {
                    assert_eq!(path, PathBuf::from("README.md"));
                    break result
                        .unwrap_or_else(|error| panic!("{error}"))
                        .unwrap_or_else(|| panic!("main resolves"));
                }
                Ok(ChangesEvent::Loaded(_)) => {}
                Err(error) => panic!("the worker stopped: {error}"),
            }
        };
        assert!(diff.contains("+two"), "{diff}");
        drop(worker);
        drop(events);
        std::fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn a_base_that_names_no_commit_reads_as_nothing_to_compare() {
        let root = fixture("no-base");
        let worker = ChangesWorker::start(root.clone(), "origin/nowhere".to_owned());
        assert!(next_reading(&worker.events()).is_none());
        drop(worker);
        std::fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    }
}
