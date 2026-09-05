//! Snapshot, diff, and focused repository reads.

use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

use crate::{
    Branch, ChangeKind, Commit, CommitFile, ConflictFile, ConflictSection, Diff, DiffSide,
    GitError, Head, ObjectId, OperationState, Ref, ReflogEntry, Remote, RemoteBranchGroup,
    RepoSnapshot, Repository, Result, SnapshotOptions, StashEntry, Tag, model::CommandKind,
};

const DIFF_ARGS: [&str; 4] = ["--no-color", "--no-ext-diff", "--patch", "--find-renames"];

impl Repository {
    /// [`DIFF_ARGS`] plus the `-U<n>` the repository is currently configured for.
    ///
    /// Every `diff` read goes through this, so widening the context with
    /// [`Repository::set_diff_context`] moves the display and the patches built from it together.
    fn diff_args(&self) -> [OsString; 5] {
        let context = self.diff_context();
        [
            OsString::from(DIFF_ARGS[0]),
            OsString::from(DIFF_ARGS[1]),
            OsString::from(DIFF_ARGS[2]),
            OsString::from(DIFF_ARGS[3]),
            OsString::from(format!("-U{context}")),
        ]
    }
}

impl Repository {
    /// Loads a complete, bounded snapshot; independent reads execute concurrently.
    pub async fn snapshot(&self, options: SnapshotOptions) -> Result<RepoSnapshot> {
        let head = self.read_head();
        let files = self.read_status();
        let branches = self.read_local_branches();
        let remote_branches = self.read_remote_branches(options.include_remotes);
        let remotes = self.read_remotes(options.include_remotes);
        let tags = self.read_tags(options.include_tags);
        let commits = self.read_commits(options.commit_limit);
        let reflog = self.read_reflog(options.reflog_limit);
        let stashes = self.read_stashes();
        let (head, files, local_branches, remote_branches, remotes, tags, commits, reflog, stashes) = tokio::join!(
            head,
            files,
            branches,
            remote_branches,
            remotes,
            tags,
            commits,
            reflog,
            stashes
        );
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let reflog = reflog?;
        // lazygit orders local branches by *checkout* recency, taken from the HEAD reflog it has
        // already loaded, and only falls back to `-committerdate` for branches the reflog does
        // not mention. Doing it here costs no extra `git` call.
        let mut local_branches = local_branches?;
        order_by_checkout_recency(&mut local_branches, &reflog);
        Ok(RepoSnapshot {
            root: self.paths.worktree_root.clone(),
            head: head?,
            operation: detect_operation(&self.paths.git_dir).await,
            files: files?,
            local_branches,
            remote_branches: remote_branches?,
            remotes: remotes?,
            tags: tags?,
            commits: commits?,
            reflog,
            stashes: stashes?,
            generation,
        })
    }

    /// Returns a parsed unstaged or staged file diff.
    pub async fn diff_file(&self, path: &Path, side: DiffSide) -> Result<Diff> {
        let untracked = side == DiffSide::Unstaged
            && self
                .read_status()
                .await?
                .iter()
                .any(|file| file.path == path && file.worktree == ChangeKind::Untracked);
        let mut command = self
            .command(CommandKind::Read)
            .arg("diff")
            .args(self.diff_args());
        if side == DiffSide::Staged {
            command = command.arg("--cached");
        }
        if untracked {
            command = command
                .args(["--no-index", "--", "/dev/null"])
                .arg(path)
                .accept_exit_code(1);
        } else {
            command = command.arg("--").arg(path);
        }
        let output = self.runner.run(command.literal_pathspecs()).await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns one parsed diff covering several literal paths on one side.
    ///
    /// This is what a directory row in a file-tree view shows: the combined patch of every file
    /// under it. Tracked paths are one `git diff` call; an untracked path has no `git diff`
    /// output at all, so each one is rendered as its own all-added `--no-index` patch exactly as
    /// [`Repository::diff_file`] does, and those files follow the tracked ones.
    pub async fn diff_paths(&self, paths: &[PathBuf], side: DiffSide) -> Result<Diff> {
        if paths.is_empty() {
            return Ok(Diff::default());
        }
        let untracked: HashSet<PathBuf> = if side == DiffSide::Unstaged {
            self.read_status()
                .await?
                .into_iter()
                .filter(|file| file.worktree == ChangeKind::Untracked)
                .map(|file| file.path)
                .collect()
        } else {
            HashSet::new()
        };
        let mut diff = Diff::default();
        let tracked: Vec<&PathBuf> = paths
            .iter()
            .filter(|path| !untracked.contains(*path))
            .collect();
        if !tracked.is_empty() {
            let mut command = self
                .command(CommandKind::Read)
                .arg("diff")
                .args(self.diff_args());
            if side == DiffSide::Staged {
                command = command.arg("--cached");
            }
            command = command.arg("--");
            for path in tracked {
                command = command.arg(path);
            }
            let output = self.runner.run(command.literal_pathspecs()).await?;
            diff.files
                .extend(crate::parse::diff::parse(&output.stdout)?.files);
        }
        for path in paths.iter().filter(|path| untracked.contains(*path)) {
            let command = self
                .command(CommandKind::Read)
                .arg("diff")
                .args(self.diff_args())
                .args(["--no-index", "--", "/dev/null"])
                .arg(path)
                .accept_exit_code(1);
            let output = self.runner.run(command.literal_pathspecs()).await?;
            diff.files
                .extend(crate::parse::diff::parse(&output.stdout)?.files);
        }
        Ok(diff)
    }

    /// Returns a parsed commit patch, optionally restricted to literal paths.
    pub async fn diff_commit(&self, oid: &ObjectId, paths: &[PathBuf]) -> Result<Diff> {
        let mut command = self
            .command(CommandKind::Read)
            .args(["show", "--format="])
            .args(self.diff_args())
            .arg(oid.as_str());
        if !paths.is_empty() {
            command = command.arg("--");
            for path in paths {
                command = command.arg(path);
            }
            command = command.literal_pathspecs();
        }
        let output = self.runner.run(command).await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns a parsed stash patch.
    pub async fn diff_stash(&self, index: usize) -> Result<Diff> {
        let stash = format!("stash@{{{index}}}");
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["stash", "show"])
                    .args(self.diff_args())
                    .arg(stash),
            )
            .await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns a parsed patch from `from` to `to`.
    pub async fn diff_range(&self, from: &Ref, to: &Ref) -> Result<Diff> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .arg("diff")
                    .args(self.diff_args())
                    .arg(&from.0)
                    .arg(&to.0),
            )
            .await?;
        crate::parse::diff::parse(&output.stdout)
    }

    /// Returns the selected branch's changes since its merge-base with HEAD.
    pub async fn diff_branch(&self, name: &Ref) -> Result<Diff> {
        let base = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["merge-base", "HEAD"])
                    .arg(&name.0),
            )
            .await?;
        let base = String::from_utf8_lossy(&base.stdout).trim().to_owned();
        self.diff_range(&Ref(base), name).await
    }

    /// Lists files changed by one commit.
    pub async fn commit_files(&self, oid: &ObjectId) -> Result<Vec<CommitFile>> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--name-status",
                        "-r",
                        "-z",
                        "-M",
                    ])
                    .arg(oid.as_str()),
            )
            .await?;
        crate::parse::diff::commit_files(&output.stdout)
    }

    /// Lists a ref's commits, newest first — lazygit's sub-commits view.
    ///
    /// `upstream` decides `Commit::pushed`: every commit reachable from `reference` but not from
    /// `upstream` is unpushed. Pass `None` to leave the flag `false` for every commit.
    pub async fn commits_for_ref(
        &self,
        reference: &Ref,
        upstream: Option<&Ref>,
        limit: usize,
    ) -> Result<Vec<Commit>> {
        let mut commits = self.log_commits(&reference.0, limit).await?;
        if let Some(upstream) = upstream {
            self.mark_pushed(&mut commits, &upstream.0, &reference.0)
                .await?;
        }
        Ok(commits)
    }

    /// Lists configured remotes and distinct fetch/push URLs.
    pub async fn remotes(&self) -> Result<Vec<Remote>> {
        self.read_remotes(true).await
    }

    /// Returns the complete commit message bytes decoded lossily as UTF-8.
    pub async fn show_commit_message(&self, oid: &ObjectId) -> Result<String> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["show", "-s", "--format=%B"])
                    .arg(oid.as_str()),
            )
            .await?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Returns raw file bytes at a ref.
    pub async fn file_at_ref(&self, reference: &Ref, path: &Path) -> Result<Vec<u8>> {
        let spec = ref_path_spec(&reference.0, path);
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .arg("show")
                    .arg(spec)
                    .literal_pathspecs(),
            )
            .await?;
        Ok(output.stdout)
    }

    /// Reads and parses conflict-marker regions in a worktree file.
    pub async fn conflicted_file(&self, path: &Path) -> Result<ConflictFile> {
        let absolute = self.paths.worktree_root.join(path);
        let content = tokio::fs::read(&absolute)
            .await
            .map_err(|source| GitError::Spawn {
                argv: vec![format!("read {}", absolute.display())],
                source,
            })?;
        let conflicts = parse_conflicts(&content)?;
        Ok(ConflictFile {
            path: path.to_path_buf(),
            content,
            conflicts,
        })
    }

    async fn read_head(&self) -> Result<Head> {
        let branch_output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
                    .accept_exit_code(1),
            )
            .await?;
        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_owned();
        let oid_output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-parse", "--verify", "HEAD"])
                    .accept_exit_code(128),
            )
            .await?;
        let oid_text = String::from_utf8_lossy(&oid_output.stdout)
            .trim()
            .to_owned();
        if oid_text.is_empty() {
            return Ok(Head::Unborn {
                name: if branch.is_empty() {
                    "HEAD".to_owned()
                } else {
                    branch
                },
            });
        }
        let description = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["show", "-s", "--format=%s", "HEAD"]),
            )
            .await?;
        let description = String::from_utf8_lossy(&description.stdout)
            .trim()
            .to_owned();
        let oid = ObjectId(oid_text);
        if branch.is_empty() {
            Ok(Head::Detached { oid, description })
        } else {
            Ok(Head::Branch {
                name: branch,
                oid: Some(oid),
                description,
            })
        }
    }

    async fn read_status(&self) -> Result<Vec<crate::FileStatus>> {
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "status",
                        "--porcelain=v1",
                        "-z",
                        "--untracked-files=all",
                        "--find-renames",
                    ])
                    .foreground_read(),
            )
            .await?;
        crate::parse::status::parse(&output.stdout)
    }

    async fn read_local_branches(&self) -> Result<Vec<Branch>> {
        let format = "%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(subject)%00%(objectname)%00%(committerdate:unix)%00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "for-each-ref",
                "--sort=-committerdate",
                "--format",
                format,
                "refs/heads",
            ]))
            .await?;
        crate::parse::refs::local_branches(&output.stdout)
    }

    async fn read_remote_branches(&self, enabled: bool) -> Result<Vec<RemoteBranchGroup>> {
        if !enabled {
            return Ok(Vec::new());
        }
        let format =
            "%(refname:short)%00%(objectname)%00%(subject)%00%(committerdate:unix)%00%(symref)%00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "for-each-ref",
                "--sort=-committerdate",
                "--format",
                format,
                "refs/remotes",
            ]))
            .await?;
        crate::parse::refs::remote_branches(&output.stdout)
    }

    async fn read_tags(&self, enabled: bool) -> Result<Vec<Tag>> {
        if !enabled {
            return Ok(Vec::new());
        }
        let format = "%(refname:short)%00%(*objectname)%00%(creatordate:unix)%00%(subject)%00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "for-each-ref",
                "--sort=-creatordate",
                "--format",
                format,
                "refs/tags",
            ]))
            .await?;
        let mut tags = crate::parse::refs::tags(&output.stdout)?;
        for tag in &mut tags {
            if tag.oid.as_str().is_empty() {
                let peeled = self
                    .runner
                    .run(
                        self.command(CommandKind::Read)
                            .args(["rev-parse", "--verify"])
                            .arg(format!("{}^{{}}", tag.name)),
                    )
                    .await?;
                tag.oid = ObjectId(String::from_utf8_lossy(&peeled.stdout).trim().to_owned());
            }
        }
        Ok(tags)
    }

    async fn read_commits(&self, limit: usize) -> Result<Vec<Commit>> {
        let mut commits = self.log_commits("HEAD", limit).await?;
        self.mark_pushed(&mut commits, "@{upstream}", "HEAD")
            .await?;
        Ok(commits)
    }

    /// `git log <revision>`, parsed. An unresolvable revision yields an empty list.
    async fn log_commits(&self, revision: &str, limit: usize) -> Result<Vec<Commit>> {
        let format = "%x1e%H%x00%P%x00%aN%x00%ae%x00%at%x00%ct%x00%s%x00%b%x00%D%x00";
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "log",
                        "--no-show-signature",
                        "--decorate=short",
                        &format!("--max-count={limit}"),
                        &format!("--format={format}"),
                        revision,
                    ])
                    .accept_exit_code(128),
            )
            .await?;
        crate::parse::commits::commits(&output.stdout)
    }

    /// Sets `Commit::pushed` from `git rev-list <upstream>..<tip>`. A missing upstream is not an
    /// error: every commit simply keeps `pushed == false`.
    async fn mark_pushed(&self, commits: &mut [Commit], upstream: &str, tip: &str) -> Result<()> {
        let resolved = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-parse", "--verify", upstream])
                    .accept_exit_code(128),
            )
            .await?;
        if resolved.stdout.is_empty() {
            return Ok(());
        }
        let unpushed = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["rev-list", &format!("{upstream}..{tip}")]),
            )
            .await?;
        let unpushed: HashSet<&str> = std::str::from_utf8(&unpushed.stdout)
            .unwrap_or_default()
            .lines()
            .collect();
        for commit in commits.iter_mut() {
            commit.pushed = !unpushed.contains(commit.oid.as_str());
        }
        Ok(())
    }

    async fn read_reflog(&self, limit: usize) -> Result<Vec<ReflogEntry>> {
        let format = "%x1e%H%x00%P%x00%gd%x00%gs%x00%ct%x00";
        let output = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args([
                        "log",
                        "-g",
                        "--no-show-signature",
                        &format!("--max-count={limit}"),
                        &format!("--format={format}"),
                        "HEAD",
                    ])
                    .accept_exit_code(128),
            )
            .await?;
        crate::parse::commits::reflog(&output.stdout)
    }

    async fn read_stashes(&self) -> Result<Vec<StashEntry>> {
        let format = "%x1e%gd%x00%H%x00%ct%x00%gs%x00";
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "stash",
                "list",
                "-z",
                &format!("--format={format}"),
            ]))
            .await?;
        crate::parse::commits::stashes(&output.stdout)
    }

    async fn read_remotes(&self, enabled: bool) -> Result<Vec<Remote>> {
        if !enabled {
            return Ok(Vec::new());
        }
        let output = self
            .runner
            .run(self.command(CommandKind::Read).arg("remote"))
            .await?;
        let mut result = Vec::new();
        for name in String::from_utf8_lossy(&output.stdout).lines() {
            let fetch = self
                .runner
                .run(
                    self.command(CommandKind::Read)
                        .args(["remote", "get-url", name]),
                )
                .await?;
            let push = self
                .runner
                .run(
                    self.command(CommandKind::Read)
                        .args(["remote", "get-url", "--push", name])
                        .accept_exit_code(2),
                )
                .await?;
            result.push(Remote {
                name: name.to_owned(),
                fetch_url: nonempty_text(&fetch.stdout),
                push_url: nonempty_text(&push.stdout),
            });
        }
        Ok(result)
    }
}

/// The branch names the HEAD reflog says were checked out, newest first, with the timestamp of
/// the entry that named each one.
///
/// lazygit reads the **`from`** side of `checkout: moving from <a> to <b>`
/// (`branchNameRegex` in `pkg/commands/git_commands/branch_loader.go`): the branch you just left
/// is the most recently used one after the branch you are on now.
#[must_use]
pub(crate) fn checkout_recency(reflog: &[ReflogEntry]) -> Vec<(String, i64)> {
    const PREFIX: &str = "checkout: moving from ";
    let mut seen: Vec<(String, i64)> = Vec::new();
    for entry in reflog {
        let Some(rest) = entry.subject.strip_prefix(PREFIX) else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        if name.is_empty() || seen.iter().any(|(known, _)| known == name) {
            continue;
        }
        seen.push((name.to_owned(), entry.committed_at));
    }
    seen
}

/// Reorders local branches the way lazygit does: the checked-out branch first, then every branch
/// the reflog mentions in checkout order, then the rest in the order `for-each-ref` produced
/// (`-committerdate`). A branch matched in the reflog also gets its `checked_out_at`.
pub(crate) fn order_by_checkout_recency(branches: &mut Vec<Branch>, reflog: &[ReflogEntry]) {
    let recency = checkout_recency(reflog);
    let mut ordered: Vec<Branch> = Vec::with_capacity(branches.len());
    for (name, at) in recency {
        // The head branch is placed first below, so it never joins the recency run.
        if let Some(index) = branches
            .iter()
            .position(|branch| !branch.is_head && branch.name.eq_ignore_ascii_case(&name))
        {
            let mut branch = branches.remove(index);
            branch.checked_out_at = Some(at);
            ordered.push(branch);
        }
    }
    ordered.append(branches);
    if let Some(index) = ordered.iter().position(|branch| branch.is_head) {
        let head = ordered.remove(index);
        ordered.insert(0, head);
    }
    *branches = ordered;
}

async fn detect_operation(git_dir: &Path) -> OperationState {
    let merge = git_dir.join("rebase-merge");
    let apply = git_dir.join("rebase-apply");
    if merge.is_dir() || apply.is_dir() {
        let directory = if merge.is_dir() { &merge } else { &apply };
        let done = read_count(directory.join("done")).await;
        let remaining = read_count(directory.join("git-rebase-todo")).await;
        return OperationState::Rebasing {
            interactive: merge.is_dir() || directory.join("interactive").exists(),
            onto: read_trimmed(directory.join("onto")).await,
            head_name: read_trimmed(directory.join("head-name")).await,
            done,
            total: match (done, remaining) {
                (Some(done), Some(remaining)) => Some(done + remaining),
                _ => None,
            },
        };
    }
    if git_dir.join("MERGE_HEAD").exists() {
        return OperationState::Merging;
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return OperationState::CherryPicking;
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return OperationState::Reverting;
    }
    if git_dir.join("BISECT_START").exists() {
        return OperationState::Bisecting;
    }
    OperationState::None
}

async fn read_trimmed(path: PathBuf) -> Option<String> {
    tokio::fs::read(path)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn read_count(path: PathBuf) -> Option<usize> {
    tokio::fs::read(path).await.ok().map(|bytes| {
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| {
                let line = trim_ascii(line);
                !line.is_empty() && !line.starts_with(b"#")
            })
            .count()
    })
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn nonempty_text(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn ref_path_spec(reference: &str, path: &Path) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let mut bytes = reference.as_bytes().to_vec();
        bytes.push(b':');
        bytes.extend_from_slice(path.as_os_str().as_bytes());
        OsString::from_vec(bytes)
    }
    #[cfg(not(unix))]
    {
        OsString::from(format!("{reference}:{}", path.to_string_lossy()))
    }
}

pub(crate) fn parse_conflicts(content: &[u8]) -> Result<Vec<ConflictSection>> {
    let mut sections = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = find_at_line_start(&content[cursor..], b"<<<<<<< ") {
        let start = cursor + relative;
        let ours_start = after_line(content, start)?;
        let separator = find_at_line_start(&content[ours_start..], b"=======")
            .ok_or_else(|| GitError::parse("conflict markers", "missing ======= marker"))?
            + ours_start;
        let base_marker = find_at_line_start(&content[ours_start..separator], b"||||||| ")
            .map(|offset| ours_start + offset);
        let theirs_start = after_line(content, separator)?;
        let end_marker = find_at_line_start(&content[theirs_start..], b">>>>>>> ")
            .ok_or_else(|| GitError::parse("conflict markers", "missing >>>>>>> marker"))?
            + theirs_start;
        let end = after_line(content, end_marker)?;
        let (ours_end, base) = if let Some(base_marker) = base_marker {
            let base_start = after_line(content, base_marker)?;
            (base_marker, Some(content[base_start..separator].to_vec()))
        } else {
            (separator, None)
        };
        sections.push(ConflictSection {
            start,
            end,
            ours: content[ours_start..ours_end].to_vec(),
            base,
            theirs: content[theirs_start..end_marker].to_vec(),
        });
        cursor = end;
    }
    Ok(sections)
}

fn find_at_line_start(content: &[u8], needle: &[u8]) -> Option<usize> {
    content
        .windows(needle.len())
        .position(|window| window == needle)
        .filter(|index| *index == 0 || content[index - 1] == b'\n')
}

fn after_line(content: &[u8], start: usize) -> Result<usize> {
    content[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| GitError::parse("conflict markers", "marker has no newline"))
}

#[cfg(test)]
mod tests {
    use super::{checkout_recency, order_by_checkout_recency, parse_conflicts};
    use crate::{Branch, ObjectId, ReflogEntry};

    fn entry(subject: &str, at: i64) -> ReflogEntry {
        ReflogEntry {
            oid: ObjectId::from("a"),
            parents: Vec::new(),
            selector: String::new(),
            subject: subject.to_owned(),
            committed_at: at,
        }
    }

    fn branch(name: &str, is_head: bool, committed_at: i64) -> Branch {
        Branch {
            name: name.to_owned(),
            oid: ObjectId::from(name),
            is_head,
            upstream: None,
            subject: String::new(),
            committed_at,
            checked_out_at: None,
        }
    }

    #[test]
    fn checkout_recency_reads_the_from_side_once_per_branch() {
        let reflog = vec![
            entry("checkout: moving from feature to main", 300),
            entry("commit: work", 250),
            entry("checkout: moving from main to feature", 200),
            entry("checkout: moving from topic to main", 100),
        ];
        assert_eq!(
            checkout_recency(&reflog),
            vec![
                ("feature".to_owned(), 300),
                ("main".to_owned(), 200),
                ("topic".to_owned(), 100),
            ]
        );
        assert!(checkout_recency(&[entry("commit: only", 1)]).is_empty());
    }

    #[test]
    fn branches_are_ordered_head_first_then_by_checkout_recency() {
        // `for-each-ref --sort=-committerdate` order on the way in.
        let mut branches = vec![
            branch("feature", false, 400),
            branch("main", true, 300),
            branch("topic", false, 200),
            branch("stale", false, 100),
        ];
        let reflog = vec![
            entry("checkout: moving from topic to main", 300),
            entry("checkout: moving from feature to topic", 200),
        ];
        order_by_checkout_recency(&mut branches, &reflog);
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "topic", "feature", "stale"]);
        assert_eq!(branches[1].checked_out_at, Some(300));
        assert_eq!(branches[2].checked_out_at, Some(200));
        assert_eq!(branches[3].checked_out_at, None);
    }

    #[test]
    fn an_empty_reflog_keeps_committerdate_order_with_the_head_first() {
        let mut branches = vec![
            branch("feature", false, 400),
            branch("main", true, 300),
            branch("topic", false, 200),
        ];
        order_by_checkout_recency(&mut branches, &[]);
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "feature", "topic"]);
    }

    #[test]
    fn parses_standard_and_diff3_conflicts() {
        let content = b"before\n<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> topic\nafter\n";
        let sections = parse_conflicts(content).unwrap();
        assert_eq!(sections[0].ours, b"ours\n");
        assert_eq!(sections[0].base.as_deref(), Some(b"base\n".as_slice()));
        assert_eq!(sections[0].theirs, b"theirs\n");
    }
}
