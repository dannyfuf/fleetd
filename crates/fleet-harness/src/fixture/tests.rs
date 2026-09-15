//! What a preset promises, checked against a real daemon that read it back.
//!
//! Every test here boots a preset the way a run does — build the origins, seed through a
//! private `fleetd`, shut it down — and then starts a *second*, fresh daemon against the same
//! home and asks it what it sees. That second daemon is the whole point: it is the one the
//! scenario's Fleet talks to, and it only knows what the seeding actually persisted.
//!
//! These tests launch the real `fleetd`, so they need it built:
//! `cargo build -p fleet-daemon && cargo test -p fleet-harness`.

use super::{Preset, apply_preset, plan};
use crate::env::{Daemon, HarnessEnv};
use fleet_core::{github::PrTab, ids::ContextId};
use fleet_proto::snapshot::Snapshot;
use std::{path::Path, process::Stdio};

/// A preset built into its own temporary world, and what a fresh daemon makes of it.
pub(super) struct Booted {
    /// Kept alive: dropping it deletes the home the assertions are about.
    _root: tempfile::TempDir,
    snapshot: Snapshot,
    /// The snapshot with every id and time the daemon generated replaced by a placeholder.
    redacted: serde_json::Value,
}

impl Booted {
    pub(super) fn context(&self) -> ContextId {
        self.snapshot
            .active_context
            .clone()
            .unwrap_or_else(|| panic!("a seeded preset selects its context"))
    }

    #[track_caller]
    pub(super) fn worktree_slugs(&self) -> Vec<String> {
        let mut slugs: Vec<String> = self
            .snapshot
            .worktrees
            .iter()
            .map(|worktree| worktree.id.to_string())
            .collect();
        slugs.sort();
        slugs
    }

    #[track_caller]
    pub(super) fn repo_slugs(&self) -> Vec<String> {
        let mut slugs: Vec<String> = self
            .snapshot
            .repos
            .iter()
            .map(|repo| repo.id.to_string())
            .collect();
        slugs.sort();
        slugs
    }
}

/// Builds one preset and reads it back through a daemon that did not seed it.
pub(super) async fn boot(preset: Preset) -> (Booted, Daemon, HarnessEnv) {
    let root = tempfile::Builder::new()
        .prefix(&format!("fleet-fixture-{preset}-"))
        .tempdir()
        .unwrap_or_else(|error| panic!("temporary root: {error}"));
    let environment = HarnessEnv::rooted(root.path())
        .unwrap_or_else(|error| panic!("lay the environment out: {error}"));
    apply_preset(preset, &environment)
        .await
        .unwrap_or_else(|error| panic!("apply the {preset} preset: {error:#}"));
    let daemon = Daemon::start(
        &environment.daemon,
        &environment,
        Stdio::null(),
        Stdio::null(),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("build fleet-daemon before running the fixture tests: {error:#}")
    });
    let client = daemon
        .client()
        .await
        .unwrap_or_else(|error| panic!("connect to the verification daemon: {error:#}"));
    let snapshot = client
        .get_snapshot()
        .await
        .unwrap_or_else(|error| panic!("read the verification snapshot: {error}"));
    let redacted = redact(
        serde_json::to_value(&snapshot)
            .unwrap_or_else(|error| panic!("serialize the snapshot: {error}")),
        root.path(),
    );
    (
        Booted {
            _root: root,
            snapshot,
            redacted,
        },
        daemon,
        environment,
    )
}

/// Replaces everything two runs of the same preset are *allowed* to differ in.
///
/// That is exactly the daemon's own generated data: ISO-8601 stamps (every one of which is
/// written to a key ending in `At`), the process id, and the absolute path of the temporary
/// root the run happens to occupy. Ids that Fleet derives — `acme/api`, `acme/api#feature`,
/// the context — are deliberately *not* redacted, because their stability is part of the
/// claim. Ids the daemon generates are UUIDs, and are matched by shape.
pub(super) fn redact(value: serde_json::Value, root: &Path) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .into_iter()
                .map(|(key, nested)| {
                    let nested =
                        if key.ends_with("At") || key == "at" || key == "pid" || key == "version" {
                            serde_json::Value::String(format!("<{key}>"))
                        } else {
                            redact(nested, root)
                        };
                    (key, nested)
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(|item| redact(item, root)).collect())
        }
        serde_json::Value::String(text) => serde_json::Value::String(mask_uuids(
            &text.replace(root.to_string_lossy().as_ref(), "<root>"),
        )),
        other => other,
    }
}

/// Replaces every `8-4-4-4-12` hexadecimal group with `<uuid>`.
pub(super) fn mask_uuids(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let characters: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        if is_uuid(&characters[index..]) {
            out.push_str("<uuid>");
            index += 36;
        } else {
            out.push(characters[index]);
            index += 1;
        }
    }
    out
}

pub(super) fn is_uuid(window: &[char]) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    if window.len() < 36 {
        return false;
    }
    let mut index = 0;
    for (position, group) in GROUPS.into_iter().enumerate() {
        if position > 0 {
            if window[index] != '-' {
                return false;
            }
            index += 1;
        }
        for _ in 0..group {
            if !window[index].is_ascii_hexdigit() {
                return false;
            }
            index += 1;
        }
    }
    true
}

#[test]
fn every_preset_round_trips_through_the_word_a_scenario_writes() {
    for preset in Preset::all() {
        let parsed: Preset = preset
            .as_str()
            .parse()
            .unwrap_or_else(|error| panic!("parse {preset}: {error}"));
        assert_eq!(parsed, preset);
    }
    assert!(
        "sideways".parse::<Preset>().is_err(),
        "an unknown preset must be rejected at load, not at run time"
    );
}

#[test]
fn only_the_empty_preset_asks_for_nothing() {
    for preset in Preset::all() {
        let described = plan::describe(preset);
        assert_eq!(
            described.is_seeded(),
            preset != Preset::Empty,
            "{preset} disagrees with itself about whether it needs seeding"
        );
    }
}

#[tokio::test]
async fn the_fake_gh_answers_the_daemon_s_own_queries_from_fixture_data() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary root: {error}"));
    let environment = HarnessEnv::rooted(root.path())
        .unwrap_or_else(|error| panic!("lay the environment out: {error}"));
    apply_preset(Preset::Empty, &environment)
        .await
        .unwrap_or_else(|error| panic!("install the fake tools: {error:#}"));
    let fixture = plan::describe(Preset::Busy);
    super::tools::install(&environment, &fixture, &root.path().join("fixture"))
        .unwrap_or_else(|error| panic!("install the busy fixture's tools: {error:#}"));

    // Byte-for-byte the argv `crates/fleet-daemon/src/adapters/github.rs` builds for the
    // `Mine` tab; a fake that answered a different shape would pass here and fail in a run.
    let output = tokio::process::Command::new(environment.fake_bin.join("gh"))
        .args([
            "pr",
            "list",
            "--repo",
            "acme/api",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,title,url,author",
            "--author",
            "@me",
        ])
        .output()
        .await
        .unwrap_or_else(|error| panic!("run the fake gh: {error}"));
    assert!(output.status.success(), "the fake gh must exit zero");
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse the fake gh's answer: {error}"));
    let numbers: Vec<u64> = rows
        .iter()
        .filter_map(|row| row.get("number").and_then(serde_json::Value::as_u64))
        .collect();
    assert_eq!(
        numbers,
        vec![12, 13],
        "the `mine` tab holds the two authored pull requests"
    );

    let review = tokio::process::Command::new(environment.fake_bin.join("gh"))
        .args([
            "pr",
            "list",
            "--repo",
            "acme/api",
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number",
            "--search",
            "user-review-requested:@me",
        ])
        .output()
        .await
        .unwrap_or_else(|error| panic!("run the fake gh: {error}"));
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&review.stdout)
        .unwrap_or_else(|error| panic!("parse the fake gh's review answer: {error}"));
    assert_eq!(
        rows.len(),
        1,
        "the `review` tab holds the one requested review"
    );

    // An unknown repository is an empty list, never an error and never the network.
    let unknown = tokio::process::Command::new(environment.fake_bin.join("gh"))
        .args(["pr", "list", "--repo", "nobody/nothing", "--json", "number"])
        .output()
        .await
        .unwrap_or_else(|error| panic!("run the fake gh: {error}"));
    assert_eq!(String::from_utf8_lossy(&unknown.stdout).trim(), "[]");
}

#[tokio::test]
async fn the_empty_preset_is_a_fleet_that_has_never_been_run() {
    let (booted, mut daemon, environment) = boot(Preset::Empty).await;
    assert!(
        booted.snapshot.repos.is_empty(),
        "empty has no repositories"
    );
    assert!(
        booted.snapshot.worktrees.is_empty(),
        "empty has no worktrees"
    );
    assert!(booted.snapshot.contexts.is_empty(), "empty has no contexts");
    assert!(
        environment.fake_bin.join("gh").is_file() && environment.fake_bin.join("acli").is_file(),
        "even the empty preset may not reach the network"
    );
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));
}

#[tokio::test]
async fn the_one_repo_preset_boots_with_one_clean_repository_and_worktree() {
    let (booted, mut daemon, _environment) = boot(Preset::OneRepo).await;
    assert_eq!(booted.repo_slugs(), vec!["acme/api".to_owned()]);
    assert_eq!(booted.worktree_slugs(), vec!["acme/api#feature".to_owned()]);
    assert!(
        booted
            .snapshot
            .worktrees
            .iter()
            .all(|w| w.degraded.is_none()),
        "one-repo is clean"
    );
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));
}

#[tokio::test]
async fn the_busy_preset_boots_twice_into_the_same_world() {
    let (first, mut daemon, _environment) = boot(Preset::Busy).await;

    assert_eq!(
        first.repo_slugs(),
        vec!["acme/api".to_owned(), "acme/web".to_owned()]
    );
    assert_eq!(
        first.worktree_slugs(),
        vec![
            "acme/api#broken".to_owned(),
            "acme/api#feature".to_owned(),
            "acme/api#hotfix".to_owned(),
            "acme/web#spike".to_owned(),
        ]
    );
    let degraded: Vec<String> = first
        .snapshot
        .worktrees
        .iter()
        .filter(|worktree| worktree.degraded.is_some())
        .map(|worktree| worktree.id.to_string())
        .collect();
    assert_eq!(
        degraded,
        vec!["acme/api#broken".to_owned()],
        "the failing post-create hook must have finished before the run's daemon starts"
    );

    let dirty = first
        .snapshot
        .worktrees
        .iter()
        .find(|worktree| worktree.id.as_ref() == "acme/api#feature")
        .map(|worktree| Path::new(&worktree.path).join("NOTES.md"))
        .unwrap_or_else(|| panic!("busy publishes acme/api#feature"));
    assert!(
        dirty.is_file(),
        "a worktree the preset calls dirty must carry an uncommitted file at {}",
        dirty.display()
    );

    // The pull-request rows come from the fake `gh`, through the daemon's own PR service.
    let client = daemon
        .client()
        .await
        .unwrap_or_else(|error| panic!("connect for pull requests: {error:#}"));
    let mine = client
        .list_pull_requests(None, Some(first.context()), PrTab::Mine, true)
        .await
        .unwrap_or_else(|error| panic!("list pull requests: {error}"));
    let numbers: Vec<u64> = mine
        .iter()
        .flat_map(|slice| slice.prs.iter().map(|pull| pull.number))
        .collect();
    assert!(
        numbers.contains(&12) && numbers.contains(&13) && numbers.contains(&4),
        "the PR screen must have content; saw {numbers:?}"
    );
    drop(client);
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));

    let (second, mut daemon, _environment) = boot(Preset::Busy).await;
    assert_eq!(
        first.redacted, second.redacted,
        "two runs of the same preset must differ only in the ids and times the daemon generates"
    );
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the second verification daemon down: {error}"));
}

#[tokio::test]
async fn the_board_preset_boots_with_cards_in_their_columns() {
    let (booted, mut daemon, environment) = boot(Preset::Board).await;
    let client = daemon
        .client()
        .await
        .unwrap_or_else(|error| panic!("connect for the board: {error:#}"));
    let boards = client
        .list_boards(Some(booted.context()))
        .await
        .unwrap_or_else(|error| panic!("list boards: {error}"));
    assert_eq!(boards.len(), 1, "the board preset seeds exactly one board");
    let view = client
        .get_board(boards[0].id.clone())
        .await
        .unwrap_or_else(|error| panic!("read the board: {error}"));
    assert_eq!(view.cards.len(), 5, "the board preset seeds five cards");
    let occupied: std::collections::BTreeSet<_> = view
        .cards
        .iter()
        .map(|card| card.status_id.to_string())
        .collect();
    assert!(
        occupied.len() >= 2,
        "cards must land in more than one column, saw {occupied:?}"
    );
    assert!(
        environment.fake_bin.join("acli").is_file(),
        "a board scenario must never reach Atlassian"
    );
    drop(client);
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));
}

#[tokio::test]
async fn the_agents_preset_points_the_configured_commands_at_the_scripted_binary() {
    let (booted, mut daemon, environment) = boot(Preset::Agents).await;
    assert_eq!(booted.worktree_slugs(), vec!["acme/api#agent".to_owned()]);
    let client = daemon
        .client()
        .await
        .unwrap_or_else(|error| panic!("connect for the configuration: {error:#}"));
    let config = client
        .get_config()
        .await
        .unwrap_or_else(|error| panic!("read the configuration: {error}"));
    for provider in ["claude", "codex"] {
        let shim = environment.fake_bin.join(provider);
        let configured = match provider {
            "claude" => &config.agent_commands.claude,
            _ => &config.agent_commands.codex,
        };
        assert_eq!(
            configured,
            &shim.to_string_lossy().into_owned(),
            "{provider} must be launched from the fixture's shim, never from the developer's PATH"
        );
        let version = tokio::process::Command::new(&shim)
            .arg("--version")
            .output()
            .await
            .unwrap_or_else(|error| panic!("probe the {provider} shim: {error}"));
        assert!(
            version.status.success(),
            "the daemon probes an agent with --version before it speaks the protocol"
        );
    }
    drop(client);
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));
}

#[tokio::test]
async fn each_injected_shape_is_a_real_daemon_job_of_the_kind_it_claims() {
    use super::jobs::{Injected, inject};
    use fleet_proto::job::{JobKind, JobStatus};

    let (booted, mut daemon, _environment) = boot(Preset::OneRepo).await;
    let context = booted.context();
    let client = daemon
        .client()
        .await
        .unwrap_or_else(|error| panic!("connect for job injection: {error:#}"));
    let fixture = plan::describe(Preset::OneRepo);
    let mut success_ordinal = 0;

    let success = inject(
        &client,
        &fixture,
        &context,
        &mut success_ordinal,
        Injected::Success,
    )
    .await
    .unwrap_or_else(|error| panic!("inject a successful job: {error:#}"));
    assert_eq!(success.jobs.len(), 1);
    assert_eq!(success.jobs[0].kind, JobKind::CreateWorktree);
    assert_eq!(
        success.jobs[0].status,
        JobStatus::Succeeded,
        "the toast the app shows is the one a succeeded CreateWorktree emits"
    );

    let failure = inject(
        &client,
        &fixture,
        &context,
        &mut success_ordinal,
        Injected::Failure,
    )
    .await
    .unwrap_or_else(|error| panic!("inject a failing job: {error:#}"));
    assert_eq!(failure.jobs[0].kind, JobKind::Clone);
    let JobStatus::Failed { error } = &failure.jobs[0].status else {
        panic!(
            "the injected clone must fail, not {:?}",
            failure.jobs[0].status
        );
    };
    assert!(
        !error.is_empty(),
        "the sticky error slot shows this text, so it may not be empty"
    );

    let repeated = inject(
        &client,
        &fixture,
        &context,
        &mut success_ordinal,
        Injected::Repeated { count: 2 },
    )
    .await
    .unwrap_or_else(|error| panic!("inject repeated jobs: {error:#}"));
    assert_eq!(
        repeated.jobs.len(),
        2,
        "a repeated injection runs once per count"
    );
    let targets: std::collections::BTreeSet<String> = repeated
        .jobs
        .iter()
        .map(|job| job.target.split(':').next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        targets.len(),
        1,
        "the toasts only coalesce because every repetition names the same worktree"
    );

    let long = inject(
        &client,
        &fixture,
        &context,
        &mut success_ordinal,
        Injected::LongRunning,
    )
    .await
    .unwrap_or_else(|error| panic!("inject a long-running job: {error:#}"));
    assert_eq!(long.jobs[0].kind, JobKind::PostCreateHooks);
    assert!(
        matches!(long.jobs[0].status, JobStatus::Queued | JobStatus::Running),
        "a long-running injection returns while the job is still going, not after it"
    );
    // A detached post-create hook cannot be cancelled through the job API, so the injection
    // ends it by hand; without this the run would leave a shell polling behind it.
    long.stop()
        .unwrap_or_else(|error| panic!("stop the long-running job: {error:#}"));
    let status = super::await_job(&client, &long.jobs[0].id)
        .await
        .unwrap_or_else(|error| panic!("wait for the stopped job: {error:#}"));
    assert_eq!(status, JobStatus::Succeeded, "a stopped hook exits zero");

    drop(client);
    daemon
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));
}
