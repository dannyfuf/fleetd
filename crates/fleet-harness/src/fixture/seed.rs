//! Seeding a hermetic home through the daemon's own API.
//!
//! A preset is applied before the run's `fleetd` starts, so the fixture brings up a private
//! daemon of its own against the same `FLEET_HOME`, drives it through `fleet-client`'s typed
//! operations, and shuts it down again. Everything a preset claims — contexts, repositories,
//! worktrees, the board, the configuration — is therefore written by the same code that
//! writes it in production, and a fixture cannot drift from what the daemon would have
//! written itself. Nothing durable is lost when the seeding daemon exits: state, boards and
//! `config.json` are files, and the job registry is deliberately not.
//!
//! The one thing this module writes directly is nothing at all; `config.json` goes through
//! `RequestBody::SetConfig`, which deep-merges and persists under
//! `fleet_core::config::CONFIG_VERSION`.

use super::{
    git,
    plan::{Board, Fixture, Repository, Worktree},
};
use crate::env::{Daemon, HarnessEnv};
use anyhow::Context as _;
use fleet_client::Client;
use fleet_core::{
    board::CardDraft,
    ids::{ContextId, JobId, RepoId},
    model::RepoHooks,
};
use fleet_proto::{job::JobStatus, request::RequestBody};
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};

/// How long a seeded clone or worktree job is given before the preset gives up.
const JOB_TIMEOUT: Duration = Duration::from_secs(120);
/// How often a seeded job's completion is probed.
const JOB_INTERVAL: Duration = Duration::from_millis(20);

/// Brings a private daemon up against the run's home, seeds the preset, and shuts it down.
pub async fn seed(
    environment: &HarnessEnv,
    fixture: &Fixture,
    origins: &BTreeMap<String, std::path::PathBuf>,
    workspace: &Path,
) -> anyhow::Result<()> {
    let log_path = workspace.join("seed-fleetd.log");
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("create {}", log_path.display()))?;
    let errors = log.try_clone().context("clone the seeding daemon log")?;
    let mut daemon = Daemon::start(
        &environment.daemon,
        environment,
        Stdio::from(log),
        Stdio::from(errors),
    )
    .await
    .context("start the fixture's own fleetd")?;

    let seeded = write_everything(&daemon, fixture, origins, environment).await;
    // The run's fleetd binds the same socket, so the seeding one is shut down on every path
    // out; a seeding failure is the more useful of the two errors and wins.
    let closed = daemon.shutdown().await;
    seeded?;
    closed.context("shut the fixture's fleetd down")
}

async fn write_everything(
    daemon: &Daemon,
    fixture: &Fixture,
    origins: &BTreeMap<String, std::path::PathBuf>,
    environment: &HarnessEnv,
) -> anyhow::Result<()> {
    let client = daemon.client().await?;
    configure(&client, fixture, environment).await?;
    let context = client
        .create_context(fixture.context.clone(), fixture.owners.clone())
        .await
        .with_context(|| format!("create the {} context", fixture.context))?;
    for repository in &fixture.repositories {
        let origin = origins.get(&repository.slug()).with_context(|| {
            format!(
                "the {} origin was not built before seeding",
                repository.slug()
            )
        })?;
        clone(&client, repository, origin, &context.id).await?;
        for worktree in &repository.worktrees {
            publish(&client, repository, worktree).await?;
        }
        clear_hooks(&client, repository).await?;
    }
    if let Some(board) = &fixture.board {
        seed_board(&client, &context.id, board).await?;
    }
    client
        .set_active_context(Some(context.id))
        .await
        .context("select the fixture's context")?;
    Ok(())
}

/// Persists the settings that make a preset the same world twice.
///
/// The prepared-copy pool is switched off rather than left at its default of one: a pool that
/// builds and refreshes in the background would put jobs, worktree directories and timing
/// into a run that never asked for them, and a scenario that wants pool behaviour can turn it
/// back on. `agentCommands` **and** `agentBinaries` point at the shims `super::tools` wrote: the
/// first is the PTY pane's shell line, the second is what the daemon `execve`s for a native
/// thread, and a scenario that exercises native threads needs the second.
async fn configure(
    client: &Client,
    fixture: &Fixture,
    environment: &HarnessEnv,
) -> anyhow::Result<()> {
    let mut patch = serde_json::json!({
        "hotPoolSize": 0,
        "hotRefreshIntervalMs": 0,
        "jobs": {"warnBeforeQuit": false}
    });
    if !fixture.agents.is_empty() {
        let mut commands = serde_json::Map::new();
        for agent in &fixture.agents {
            let shim = environment.fake_bin.join(agent.provider.executable());
            commands.insert(
                agent.provider.flag().to_owned(),
                serde_json::Value::String(shim.to_string_lossy().into_owned()),
            );
        }
        if let Some(object) = patch.as_object_mut() {
            object.insert(
                "agentCommands".to_owned(),
                serde_json::Value::Object(commands.clone()),
            );
            object.insert(
                "agentBinaries".to_owned(),
                serde_json::Value::Object(commands),
            );
        }
    }
    match client.request(RequestBody::SetConfig { patch }).await {
        Ok(_) => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "persist the fixture configuration: {error}"
        )),
    }
}

async fn clone(
    client: &Client,
    repository: &Repository,
    origin: &Path,
    context: &ContextId,
) -> anyhow::Result<()> {
    let job = client
        .clone_repo(
            repository.owner.clone(),
            repository.name.clone(),
            origin.to_string_lossy().into_owned(),
            context.clone(),
            Some(git::DEFAULT_BRANCH.to_owned()),
        )
        .await
        .map_err(|error| anyhow::anyhow!("clone {}: {error}", repository.slug()))?;
    let status = await_job(client, &job.id).await?;
    match status {
        JobStatus::Succeeded => Ok(()),
        other => anyhow::bail!("cloning {} ended {other:?}", repository.slug()),
    }
}

async fn publish(
    client: &Client,
    repository: &Repository,
    worktree: &Worktree,
) -> anyhow::Result<()> {
    let repo = RepoId::try_from(repository.slug())
        .map_err(|error| anyhow::anyhow!("name {}: {error}", repository.slug()))?;
    let result = client
        .create_worktree(
            repo,
            worktree.slug.clone(),
            None,
            worktree.base.clone(),
            None,
            RepoHooks {
                prepare: Vec::new(),
                post_create: worktree.hook.commands(),
            },
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("create {}#{}: {error}", repository.slug(), worktree.slug)
        })?;
    // A hook that fails is the point of the degraded row, so the outcome is awaited rather
    // than required: what must be deterministic is that the job has finished before the run's
    // own daemon starts and reads the worktree back.
    if let Some(job) = result.post_create_job {
        await_job(client, &job.id).await?;
    }
    for file in &worktree.dirty_files {
        git::write(
            Path::new(&result.worktree.path),
            file,
            "an uncommitted fixture change\n",
        )
        .await?;
    }
    Ok(())
}

/// Puts the repository's hooks back to none once its worktrees are published.
///
/// `create_worktree` persists the hooks it was given onto the repository, so without this the
/// last worktree a preset seeds would decide what every *later* worktree in the run does — a
/// `busy` fixture would leave `acme/api` permanently configured to fail its post-create hook.
/// The already-degraded worktree keeps its degraded record; only the repository forgets.
async fn clear_hooks(client: &Client, repository: &Repository) -> anyhow::Result<()> {
    if repository
        .worktrees
        .iter()
        .all(|worktree| worktree.hook.commands().is_empty())
    {
        return Ok(());
    }
    let repo = RepoId::try_from(repository.slug())
        .map_err(|error| anyhow::anyhow!("name {}: {error}", repository.slug()))?;
    client
        .set_repo_hooks(repo, RepoHooks::default())
        .await
        .map(drop)
        .map_err(|error| anyhow::anyhow!("clear {}'s hooks: {error}", repository.slug()))
}

async fn seed_board(client: &Client, context: &ContextId, board: &Board) -> anyhow::Result<()> {
    let view = client
        .create_board(
            context.clone(),
            Some(format!("{} board", board.prefix)),
            Some(board.prefix.clone()),
            None,
        )
        .await
        .map_err(|error| anyhow::anyhow!("create the {} board: {error}", board.prefix))?;
    let statuses = view.board.statuses.clone();
    anyhow::ensure!(
        !statuses.is_empty(),
        "a new board must come with at least one column"
    );
    for card in &board.cards {
        let column = card.column.min(statuses.len() - 1);
        let draft = CardDraft {
            title: card.title.clone(),
            description: card.description.clone(),
            status_id: Some(statuses[column].id.clone()),
            ..CardDraft::default()
        };
        client
            .create_card(view.board.id.clone(), draft)
            .await
            .map_err(|error| anyhow::anyhow!("create the card {:?}: {error}", card.title))?;
    }
    Ok(())
}

/// Waits for one daemon job to reach a terminal state, and reports which one.
///
/// The daemon publishes job updates as events, but a fixture has no view to drive from them
/// and the registry is authoritative, so this polls it — the same shape `Daemon::client` uses
/// to wait for readiness, and bounded the same way.
pub async fn await_job(client: &Client, job: &JobId) -> anyhow::Result<JobStatus> {
    let deadline = tokio::time::Instant::now() + JOB_TIMEOUT;
    loop {
        let jobs = client
            .list_jobs()
            .await
            .map_err(|error| anyhow::anyhow!("list jobs while waiting for {job}: {error}"))?;
        if let Some(record) = jobs.into_iter().find(|record| &record.id == job) {
            match record.status {
                JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => {}
                terminal => return Ok(terminal),
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("job {job} did not finish within {JOB_TIMEOUT:?}");
        }
        tokio::time::sleep(JOB_INTERVAL).await;
    }
}
