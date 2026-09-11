//! The Git plumbing one checkpoint is made of, over the daemon's [`Shell`] boundary.
//!
//! Three properties shape every call here, and each one is a decision:
//!
//! 1. **Nothing touches the user's index or `HEAD`.** Every capture and every restore runs with
//!    `GIT_INDEX_FILE` pointed at a scratch file ([`ScratchIndex`]), so the user's staged work is
//!    still staged after a revert, `index.lock` is never contended with a `git` the user ran, and
//!    no branch, tag, stash or reflog entry is created. A checkpoint is a commit nothing points
//!    at except a `refs/fleet/` ref.
//! 2. **Plumbing, not porcelain.** `read-tree`, `write-tree`, `commit-tree`, `update-ref`,
//!    `checkout-index`, `for-each-ref`, `cat-file` and `diff` are stable, scriptable and
//!    locale-independent; `-z` output is parsed, never a human-facing listing. No hook runs: none
//!    of these commands has one.
//! 3. **The identity on a checkpoint commit is Fleet's.** `GIT_AUTHOR_*`/`GIT_COMMITTER_*` are
//!    set explicitly so a checkpoint is never attributed to the user, and so a capture still
//!    works in a repository where the user never configured `user.email` — which `commit-tree`
//!    would otherwise refuse.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::adapters::shell::{Shell, ShellCommand, ShellResult};

use super::error::{CheckpointError, Result};

/// Deadline for one Git invocation.
///
/// Generous because the two expensive calls are bounded by the size of a worktree the user chose:
/// `git add -A` stats every non-ignored file, and a restore writes every file a turn touched. It
/// stays bounded because a wedged `git` would otherwise hold the per-thread capture lock — and
/// therefore the turn that is waiting to start — forever.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Paths per `checkout-index` invocation, so a revert of a large turn cannot exceed `ARG_MAX`.
const PATHS_PER_INVOCATION: usize = 256;

/// The status letters `git diff --name-status` reports for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    /// Present in the later tree only: a file the turn created.
    Added,
    /// Present in the earlier tree only: a file the turn deleted.
    Deleted,
    /// In both trees, with different content or mode.
    Modified,
}

/// One path the working tree and a checkpoint disagree about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Difference {
    /// How the two trees differ.
    pub change: Change,
    /// Worktree-relative path.
    pub path: String,
}

/// A Git object identifier, as plumbing printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Oid(pub String);

/// One row of `git for-each-ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RefRow {
    /// Full ref name.
    pub name: String,
    /// Creator date in Unix seconds.
    pub created_at: i64,
}

/// Git plumbing for the checkpoint service.
#[derive(Clone)]
pub(super) struct Plumbing {
    shell: Arc<dyn Shell>,
}

impl Plumbing {
    /// Builds the plumbing over the daemon's process boundary.
    pub(super) fn new(shell: Arc<dyn Shell>) -> Self {
        Self { shell }
    }

    /// Whether `worktree` is inside a Git working tree.
    ///
    /// Decided by exit status, never by Git's English: `rev-parse` prints `true` on success and
    /// fails otherwise, so the check survives a locale and a reworded message.
    pub(super) async fn is_worktree(&self, worktree: &Path) -> Result<bool> {
        let result = self
            .run(worktree, None, &["rev-parse", "--is-inside-work-tree"])
            .await?;
        Ok(result.success() && result.stdout.trim() == "true")
    }

    /// Writes the working tree into `index` and returns the tree it makes.
    ///
    /// `paths` is the scope, and its three states are all meaningful: `None` captures the whole
    /// non-ignored working tree, `Some(paths)` captures exactly those files, and `Some(&[])`
    /// captures *nothing* — which is what a pre-edit checkpoint of a file that does not exist yet
    /// records, and why this is not an `is_empty()` check on a slice. `git add` with a pathspec
    /// that matches nothing is an error, so the empty scope skips the call and keeps the empty
    /// index Git just wrote.
    pub(super) async fn snapshot_tree(
        &self,
        worktree: &Path,
        index: &ScratchIndex,
        paths: Option<&[String]>,
    ) -> Result<Oid> {
        self.checked(
            worktree,
            Some(index),
            &["read-tree", "--empty"],
            "read-tree",
        )
        .await?;
        let mut add: Vec<String> = vec!["add".to_owned(), "--all".to_owned()];
        match paths {
            None => {
                self.checked_owned(worktree, Some(index), add, "add")
                    .await?;
            }
            Some([]) => {}
            Some(paths) => {
                add.push("--".to_owned());
                add.extend(paths.iter().cloned());
                self.checked_owned(worktree, Some(index), add, "add")
                    .await?;
            }
        }
        self.write_tree(worktree, index).await
    }

    /// Writes `index`'s current content as a tree.
    pub(super) async fn write_tree(&self, worktree: &Path, index: &ScratchIndex) -> Result<Oid> {
        let result = self
            .checked(worktree, Some(index), &["write-tree"], "write-tree")
            .await?;
        Ok(Oid(result.stdout.trim().to_owned()))
    }

    /// Commits `tree` with Fleet's own identity and no parent.
    ///
    /// Parentless on purpose: a checkpoint is a snapshot, not a history, and a parent would put
    /// the user's commits behind a ref they did not make — `git log --all` would then walk their
    /// branch twice.
    pub(super) async fn commit_tree(
        &self,
        worktree: &Path,
        tree: &Oid,
        message: &str,
    ) -> Result<Oid> {
        let result = self
            .checked_owned(
                worktree,
                None,
                vec![
                    "commit-tree".to_owned(),
                    tree.0.clone(),
                    "-m".to_owned(),
                    message.to_owned(),
                ],
                "commit-tree",
            )
            .await?;
        Ok(Oid(result.stdout.trim().to_owned()))
    }

    /// Points `name` at `commit`.
    pub(super) async fn update_ref(&self, worktree: &Path, name: &str, commit: &Oid) -> Result<()> {
        self.checked_owned(
            worktree,
            None,
            vec![
                "update-ref".to_owned(),
                name.to_owned(),
                commit.0.clone(),
                // The empty old value asserts the ref does not exist yet, so two captures that
                // somehow raced on one ordinal fail loudly instead of overwriting a tree the
                // other one recorded.
                String::new(),
            ],
            "update-ref",
        )
        .await?;
        Ok(())
    }

    /// Deletes `name`.
    ///
    /// Deleting a ref that is already gone succeeds, which is what makes both callers idempotent:
    /// the garbage collector and an explicit forget can reach the same ref, and a checkpoint
    /// whose ref someone removed by hand is already in the state both of them want.
    pub(super) async fn delete_ref(&self, worktree: &Path, name: &str) -> Result<()> {
        self.checked_owned(
            worktree,
            None,
            vec!["update-ref".to_owned(), "-d".to_owned(), name.to_owned()],
            "update-ref",
        )
        .await?;
        Ok(())
    }

    /// Resolves the tree a ref's commit points at, or `None` when the ref does not exist.
    pub(super) async fn resolve_tree(&self, worktree: &Path, name: &str) -> Result<Option<Oid>> {
        let revision = format!("{name}^{{tree}}");
        let result = self
            .run(
                worktree,
                None,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &revision,
                ],
            )
            .await?;
        if !result.success() {
            return Ok(None);
        }
        let oid = result.stdout.trim().to_owned();
        Ok((!oid.is_empty()).then_some(Oid(oid)))
    }

    /// Reads a checkpoint commit's raw text, for the record in its message body.
    pub(super) async fn read_commit(&self, worktree: &Path, name: &str) -> Result<String> {
        let result = self
            .checked_owned(
                worktree,
                None,
                vec!["cat-file".to_owned(), "commit".to_owned(), name.to_owned()],
                "cat-file",
            )
            .await?;
        Ok(result.stdout)
    }

    /// Lists every ref under `prefix`, oldest ref name first.
    ///
    /// Tab-separated because a ref name can contain neither a tab nor a newline, so the format is
    /// unambiguous without `-z`, which `for-each-ref` does not offer.
    pub(super) async fn list_refs(&self, worktree: &Path, prefix: &str) -> Result<Vec<RefRow>> {
        let result = self
            .checked_owned(
                worktree,
                None,
                vec![
                    "for-each-ref".to_owned(),
                    "--format=%(refname)\t%(creatordate:unix)".to_owned(),
                    prefix.to_owned(),
                ],
                "for-each-ref",
            )
            .await?;
        Ok(result
            .stdout
            .lines()
            .filter_map(|line| {
                let (name, created_at) = line.split_once('\t')?;
                Some(RefRow {
                    name: name.to_owned(),
                    created_at: created_at.trim().parse().unwrap_or_default(),
                })
            })
            .collect())
    }

    /// Lists how `later` differs from `earlier`, optionally narrowed to `paths`.
    pub(super) async fn diff_trees(
        &self,
        worktree: &Path,
        earlier: &Oid,
        later: &Oid,
        paths: &[String],
    ) -> Result<Vec<Difference>> {
        let mut args = vec![
            "diff".to_owned(),
            "--name-status".to_owned(),
            "-z".to_owned(),
            // A rename would report one path with two names and hide the second from the
            // restore; a checkpoint is content, so renames are just an add and a delete.
            "--no-renames".to_owned(),
            earlier.0.clone(),
            later.0.clone(),
        ];
        if !paths.is_empty() {
            args.push("--".to_owned());
            args.extend(paths.iter().cloned());
        }
        let result = self.checked_owned(worktree, None, args, "diff").await?;
        Ok(parse_name_status(&result.stdout))
    }

    /// Writes `paths` out of `index` into the working tree, overwriting what is there.
    pub(super) async fn checkout_paths(
        &self,
        worktree: &Path,
        index: &ScratchIndex,
        paths: &[String],
    ) -> Result<()> {
        for chunk in paths.chunks(PATHS_PER_INVOCATION) {
            let mut args = vec![
                "checkout-index".to_owned(),
                "--force".to_owned(),
                "--".to_owned(),
            ];
            args.extend(chunk.iter().cloned());
            self.checked_owned(worktree, Some(index), args, "checkout-index")
                .await?;
        }
        Ok(())
    }

    /// Loads one tree into `index` without touching the working tree.
    pub(super) async fn read_tree(
        &self,
        worktree: &Path,
        index: &ScratchIndex,
        tree: &Oid,
    ) -> Result<()> {
        self.checked_owned(
            worktree,
            Some(index),
            vec!["read-tree".to_owned(), tree.0.clone()],
            "read-tree",
        )
        .await?;
        Ok(())
    }

    async fn checked(
        &self,
        worktree: &Path,
        index: Option<&ScratchIndex>,
        args: &[&str],
        operation: &'static str,
    ) -> Result<ShellResult> {
        let owned = args.iter().map(|argument| (*argument).to_owned()).collect();
        self.checked_owned(worktree, index, owned, operation).await
    }

    async fn checked_owned(
        &self,
        worktree: &Path,
        index: Option<&ScratchIndex>,
        args: Vec<String>,
        operation: &'static str,
    ) -> Result<ShellResult> {
        let result = self.run_owned(worktree, index, args).await?;
        if result.success() {
            return Ok(result);
        }
        Err(CheckpointError::git(
            operation,
            result.status,
            &result.stderr,
            &result.stdout,
        ))
    }

    async fn run(
        &self,
        worktree: &Path,
        index: Option<&ScratchIndex>,
        args: &[&str],
    ) -> Result<ShellResult> {
        let owned = args.iter().map(|argument| (*argument).to_owned()).collect();
        self.run_owned(worktree, index, owned).await
    }

    async fn run_owned(
        &self,
        worktree: &Path,
        index: Option<&ScratchIndex>,
        args: Vec<String>,
    ) -> Result<ShellResult> {
        let mut command = ShellCommand::new("git")
            .args(args)
            .cwd(worktree)
            .timeout(GIT_TIMEOUT);
        for (key, value) in environment(index) {
            command = command.env(key, value);
        }
        self.shell.run(command).await.map_err(|error| match error {
            crate::DaemonError::Filesystem { path, source } => {
                CheckpointError::Filesystem { path, source }
            }
            other => CheckpointError::Git {
                operation: "invocation",
                message: other.to_string(),
            },
        })
    }
}

/// The environment every checkpoint invocation runs with.
fn environment(index: Option<&ScratchIndex>) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::from([
        // Fleet's identity, not the user's, and a repository with no configured identity still
        // checkpoints.
        ("GIT_AUTHOR_NAME".to_owned(), "Fleet".to_owned()),
        ("GIT_AUTHOR_EMAIL".to_owned(), "fleet@localhost".to_owned()),
        ("GIT_COMMITTER_NAME".to_owned(), "Fleet".to_owned()),
        (
            "GIT_COMMITTER_EMAIL".to_owned(),
            "fleet@localhost".to_owned(),
        ),
        // Nothing here reaches a network or a credential helper, and a checkpoint must never be
        // the thing that blocks a turn on a password prompt.
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
        // Every pathspec this service passes is a path an agent edited, not a pattern. Without
        // this, a file called `report[1].txt` is a character-class glob: the capture would miss
        // it and the revert would restore whatever else the pattern happened to match.
        ("GIT_LITERAL_PATHSPECS".to_owned(), "1".to_owned()),
    ]);
    if let Some(index) = index {
        environment.insert(
            "GIT_INDEX_FILE".to_owned(),
            index.path().to_string_lossy().into_owned(),
        );
    }
    environment
}

/// Parses `git diff --name-status -z` output, which is `status NUL path NUL` per change.
fn parse_name_status(output: &str) -> Vec<Difference> {
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    let mut differences = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        let change = match status.chars().next() {
            Some('A') => Change::Added,
            Some('D') => Change::Deleted,
            // `M` is a content change, `T` a type change (a file became a symlink); both are
            // restored by writing the checkpoint's version back.
            Some('M' | 'T') => Change::Modified,
            // `U` is an unmerged index entry and `X` means Git itself is confused. Neither can
            // come out of a tree-to-tree diff, and guessing at one would restore the wrong thing.
            _ => continue,
        };
        differences.push(Difference {
            change,
            path: path.to_owned(),
        });
    }
    differences
}

/// A throwaway index file, so no Git call in this service can disturb the user's own.
///
/// Lives outside the worktree, because an index file *inside* it would be staged by the very
/// `git add --all` that reads it.
#[derive(Debug)]
pub(super) struct ScratchIndex {
    path: PathBuf,
}

impl ScratchIndex {
    /// Allocates a unique scratch index path. Nothing is written until Git writes it.
    pub(super) fn new() -> Self {
        Self {
            path: std::env::temp_dir()
                .join(format!("fleet-checkpoint-{}.index", uuid::Uuid::new_v4())),
        }
    }

    /// The path handed to `GIT_INDEX_FILE`.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                target: "fleet::agents",
                path = %self.path.display(),
                %error,
                "could not remove a checkpoint scratch index"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_status_stream_is_parsed_by_its_nul_fields() {
        let output = "M\0src/lib.rs\0A\0src/new file.rs\0D\0gone.rs\0T\0link\0";

        assert_eq!(
            parse_name_status(output),
            vec![
                Difference {
                    change: Change::Modified,
                    path: "src/lib.rs".to_owned()
                },
                Difference {
                    change: Change::Added,
                    path: "src/new file.rs".to_owned()
                },
                Difference {
                    change: Change::Deleted,
                    path: "gone.rs".to_owned()
                },
                Difference {
                    change: Change::Modified,
                    path: "link".to_owned()
                },
            ]
        );
    }

    #[test]
    fn an_unmergeable_or_truncated_row_is_dropped_rather_than_guessed_at() {
        assert_eq!(parse_name_status("U\0conflict.rs\0"), Vec::new());
        assert_eq!(parse_name_status("M\0"), Vec::new());
        assert_eq!(parse_name_status(""), Vec::new());
    }

    #[test]
    fn every_invocation_carries_fleets_identity_and_no_prompt() {
        let environment = environment(None);

        assert_eq!(
            environment.get("GIT_AUTHOR_NAME").map(String::as_str),
            Some("Fleet")
        );
        assert_eq!(
            environment.get("GIT_COMMITTER_EMAIL").map(String::as_str),
            Some("fleet@localhost")
        );
        assert_eq!(
            environment.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            environment.get("GIT_LITERAL_PATHSPECS").map(String::as_str),
            Some("1")
        );
        assert!(!environment.contains_key("GIT_INDEX_FILE"));
    }

    #[test]
    fn a_scratch_index_is_unique_and_never_inside_a_worktree() {
        let first = ScratchIndex::new();
        let second = ScratchIndex::new();

        assert_ne!(first.path(), second.path());
        assert!(first.path().starts_with(std::env::temp_dir()));
        assert_eq!(
            environment(Some(&first)).get("GIT_INDEX_FILE").cloned(),
            Some(first.path().to_string_lossy().into_owned())
        );
    }
}
