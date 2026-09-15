//! Real git origins for a fixture.
//!
//! Fixtures build repositories with `git`, never by fabricating a `.git` directory: Fleet's
//! git layer shells out to the real binary for every clone, fetch, branch and status, so a
//! hand-written object store would only prove that the fake matched itself.
//!
//! Everything an object id depends on is pinned — author, committer, both dates, the file
//! bodies and the commit order — so the same preset built twice produces byte-identical
//! commits. What is left varying between two runs is what the *daemon* generates: job ids and
//! ISO-8601 timestamps.

use super::plan::{Fixture, Repository};
use anyhow::Context as _;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The identity every fixture commit is authored and committed by.
const IDENTITY: (&str, &str) = ("Fleet Fixture", "fixture@fleet.test");
/// The fixed author and committer date, so commit ids do not move between runs.
const DATE: &str = "2026-01-01T00:00:00+0000";
/// The default branch every fixture origin publishes.
pub const DEFAULT_BRANCH: &str = "main";

/// Builds one bare origin per repository and returns them keyed by `owner/name`.
///
/// The origins live beside the private `FLEET_HOME` rather than inside it: they stand in for
/// GitHub, and a Fleet home that contained its own remotes would not be the home a user has.
pub async fn build_origins(
    workspace: &Path,
    fixture: &Fixture,
    child_home: &Path,
) -> anyhow::Result<BTreeMap<String, PathBuf>> {
    let mut origins = BTreeMap::new();
    for repository in &fixture.repositories {
        let origin = build_origin(workspace, repository, child_home)
            .await
            .with_context(|| format!("build the {} origin", repository.slug()))?;
        origins.insert(repository.slug(), origin);
    }
    Ok(origins)
}

async fn build_origin(
    workspace: &Path,
    repository: &Repository,
    child_home: &Path,
) -> anyhow::Result<PathBuf> {
    let origin = workspace
        .join("origins")
        .join(&repository.owner)
        .join(format!("{}.git", repository.name));
    let seed = workspace
        .join("seed")
        .join(&repository.owner)
        .join(&repository.name);
    for directory in [&origin, &seed] {
        if let Some(parent) = directory.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create {}", parent.display()))?;
        }
    }

    let root = workspace;
    git(root, child_home, &["init", "--bare", &text(&origin)]).await?;
    git(root, child_home, &["init", &text(&seed)]).await?;
    write(&seed, "README.md", &format!("# {}\n", repository.name)).await?;
    write(
        &seed,
        "src/lib.rs",
        "//! A fixture crate.\n\npub fn answer() -> u32 {\n    42\n}\n",
    )
    .await?;
    git(&seed, child_home, &["add", "--all"]).await?;
    git(&seed, child_home, &["commit", "-m", "initial commit"]).await?;
    git(&seed, child_home, &["branch", "-M", DEFAULT_BRANCH]).await?;
    git(
        &seed,
        child_home,
        &["remote", "add", "origin", &text(&origin)],
    )
    .await?;
    git(&seed, child_home, &["push", "-u", "origin", DEFAULT_BRANCH]).await?;

    for branch in &repository.branches {
        git(
            &seed,
            child_home,
            &["checkout", "-b", branch, DEFAULT_BRANCH],
        )
        .await?;
        write(&seed, &format!("{branch}.md"), &format!("# {branch}\n")).await?;
        git(&seed, child_home, &["add", "--all"]).await?;
        git(
            &seed,
            child_home,
            &["commit", "-m", &format!("add {branch}")],
        )
        .await?;
        git(&seed, child_home, &["push", "origin", branch]).await?;
    }
    git(&seed, child_home, &["checkout", DEFAULT_BRANCH]).await?;

    // A clone reads `HEAD` to learn the default branch, and a bare repository initialised on a
    // machine configured for `master` would otherwise publish a branch that does not exist.
    git(
        root,
        child_home,
        &[
            "--git-dir",
            &text(&origin),
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{DEFAULT_BRANCH}"),
        ],
    )
    .await?;
    Ok(origin)
}

/// Writes a file into a worktree, creating its parent directories.
pub async fn write(root: &Path, relative: &str, body: &str) -> anyhow::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    tokio::fs::write(&path, body)
        .await
        .with_context(|| format!("write {}", path.display()))
}

/// Runs one git command with every source of variation pinned or shut off.
async fn git(cwd: &Path, child_home: &Path, args: &[&str]) -> anyhow::Result<()> {
    let (name, email) = IDENTITY;
    let output = tokio::process::Command::new("git")
        .args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            &format!("init.defaultBranch={DEFAULT_BRANCH}"),
            "-c",
            "advice.detachedHead=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .current_dir(cwd)
        // The child home is the run's, not the developer's, so no `~/.gitconfig`, no
        // credential helper and no signing key can change what a fixture commit contains.
        .env("HOME", child_home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", name)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_AUTHOR_DATE", DATE)
        .env("GIT_COMMITTER_NAME", name)
        .env("GIT_COMMITTER_EMAIL", email)
        .env("GIT_COMMITTER_DATE", DATE)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
