use super::{
    CommandOutput, FAILURE, jobs::wait_for_job, parse_id, parse_ids, validation, wait_event,
};
use crate::{
    args::{
        CreateArgs, DeleteArgs, InspectArgs, JsonArgs, KillArgs, OpenArgs, PathArgs, PruneArgs,
        VERSION_DISPLAY,
    },
    envelope::{
        CreateEnvelope, DeleteEnvelope, InspectEnvelope, ListEnvelope, OkEnvelope, PROTOCOL,
        PruneEnvelope, StatusEnvelope, single_line, to_json,
    },
    human,
};
use fleet_client::Client;
use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    model::{CloneStatus, Repo, RepoHooks, Worktree},
    validate::{validate_branch, validate_slug},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    snapshot::Snapshot,
};

pub(super) async fn create(
    client: &Client,
    arguments: CreateArgs,
) -> Result<CommandOutput, ProtoError> {
    let repo_id = parse_id::<RepoId>(&arguments.repo)?;
    validate_slug(&arguments.slug).map_err(|error| validation(error.to_string()))?;
    if let Some(branch) = &arguments.branch {
        validate_branch(branch).map_err(|error| validation(error.to_string()))?;
    }
    if let Some(default_branch) = &arguments.default_branch {
        validate_branch(default_branch).map_err(|error| validation(error.to_string()))?;
    }
    let host = parse_host(arguments.host.as_deref())?;
    let supplied_hooks = arguments.hooks.as_deref().map(parse_hooks).transpose()?;
    let mut repo = ensure_repo(client, &repo_id, &arguments).await?;

    if let Some(hooks) = supplied_hooks {
        repo = client.set_repo_hooks(repo.id, hooks).await?;
    }
    let hooks = repo.hooks;
    let base = arguments
        .base
        .or_else(|| Some(format!("origin/{}", repo.default_branch)));
    let result = client
        .create_worktree(repo.id, arguments.slug, arguments.branch, base, host, hooks)
        .await?;
    if let Some(job) = &result.post_create_job {
        wait_for_job(client, job, "post-create hooks").await?;
    }
    let text = if arguments.json {
        to_json(&CreateEnvelope {
            protocol: PROTOCOL,
            created: result.created,
            worktree: &result.worktree,
        })?
    } else if result.created {
        format!("Created {}", result.worktree.id)
    } else {
        format!("Existing {}", result.worktree.id)
    };
    Ok(CommandOutput::success(text))
}

async fn ensure_repo(
    client: &Client,
    repo_id: &RepoId,
    arguments: &CreateArgs,
) -> Result<Repo, ProtoError> {
    let snapshot = client.get_snapshot().await?;
    if let Some(repo) = snapshot.repos.into_iter().find(|repo| &repo.id == repo_id) {
        return Ok(repo);
    }
    let url = arguments.url.clone().ok_or_else(|| {
        validation(format!(
            "repository `{repo_id}` is not registered; --url is required"
        ))
    })?;
    let context = if let Some(context) = snapshot
        .contexts
        .iter()
        .find(|context| context.owners.iter().any(|owner| owner == repo_id.owner()))
    {
        context.id.clone()
    } else if let Some(active) = snapshot.active_context {
        active
    } else if let Some(context) = snapshot.contexts.first() {
        context.id.clone()
    } else {
        client
            .create_context(repo_id.owner(), vec![repo_id.owner().to_owned()])
            .await?
            .id
    };

    let mut events = client.events();
    let job = client
        .clone_repo(
            repo_id.owner(),
            repo_id.name(),
            url,
            context,
            arguments.default_branch.clone(),
        )
        .await?;
    let mut snapshot = client.get_snapshot().await?;
    loop {
        if let Some(repo) = cloned_repo(snapshot, repo_id)? {
            return Ok(repo);
        }
        snapshot = match wait_event(&mut events, |event| match event {
            Event::SnapshotChanged(_) => true,
            Event::JobUpdated(updated) => updated.id == job.id,
            _ => false,
        })
        .await?
        {
            Some(Event::SnapshotChanged(snapshot)) => snapshot,
            _ => client.get_snapshot().await?,
        };
    }
}

fn cloned_repo(snapshot: Snapshot, repo_id: &RepoId) -> Result<Option<Repo>, ProtoError> {
    if let Some(repo) = snapshot.repos.into_iter().find(|repo| &repo.id == repo_id) {
        return Ok(Some(repo));
    }
    if let Some(clone) = snapshot
        .clones
        .into_iter()
        .find(|clone| &clone.id == repo_id)
        && clone.status == CloneStatus::Failed
    {
        return Err(ProtoError {
            kind: ErrorKind::Git,
            message: single_line(clone.error.as_deref().unwrap_or("repository clone failed")),
        });
    }
    Ok(None)
}

pub(super) async fn open(
    client: &Client,
    arguments: OpenArgs,
) -> Result<CommandOutput, ProtoError> {
    let snapshot = client.get_snapshot().await?;
    let worktree = resolve_open_target(snapshot.worktrees, &arguments.target)?;
    let session = client
        .ensure_session(Some(worktree.id.clone()), None, true)
        .await?;
    client.touch_worktree_opened(worktree.id).await?;
    Ok(CommandOutput::success(format!(
        "{}\nOpen it in the Fleet app: fleet",
        session.id
    )))
}

pub(super) async fn list(
    client: &Client,
    arguments: JsonArgs,
) -> Result<CommandOutput, ProtoError> {
    let snapshot = client.get_snapshot().await?;
    let text = if arguments.json {
        to_json(&ListEnvelope {
            protocol: PROTOCOL,
            version: VERSION_DISPLAY,
            repos: &snapshot.repos,
            worktrees: &snapshot.worktrees,
        })?
    } else {
        human::list(snapshot.repos.len(), snapshot.worktrees.len())
    };
    Ok(CommandOutput::success(text))
}

pub(super) async fn inspect(
    client: &Client,
    arguments: InspectArgs,
) -> Result<CommandOutput, ProtoError> {
    let ids = parse_ids::<WorktreeId>(&arguments.ids)?;
    let repo = arguments
        .repo
        .as_deref()
        .map(parse_id::<RepoId>)
        .transpose()?;
    let inspections = client.inspect_worktrees(ids, repo, arguments.fetch).await?;
    let text = if arguments.json {
        to_json(&InspectEnvelope {
            protocol: PROTOCOL,
            worktrees: &inspections,
        })?
    } else {
        human::inspect(&inspections)
    };
    Ok(CommandOutput::success(text))
}

pub(super) async fn delete(
    client: &Client,
    arguments: DeleteArgs,
) -> Result<CommandOutput, ProtoError> {
    let ids = parse_ids::<WorktreeId>(&arguments.ids)?;
    let results = client.delete_worktrees(ids).await?;
    let ok = results.iter().all(|result| result.ok);
    let text = if arguments.json {
        to_json(&DeleteEnvelope {
            protocol: PROTOCOL,
            ok,
            results: &results,
        })?
    } else {
        human::delete(&results)
    };
    Ok(CommandOutput::with_exit_code(
        text,
        if ok { 0 } else { FAILURE },
    ))
}

pub(super) async fn prune(
    client: &Client,
    arguments: PruneArgs,
) -> Result<CommandOutput, ProtoError> {
    let repo = arguments
        .repo
        .as_deref()
        .map(parse_id::<RepoId>)
        .transpose()?;
    let result = client
        .prune_worktrees(
            arguments.dry_run,
            !arguments.no_fetch,
            arguments.kill_sessions,
            repo,
        )
        .await?;
    let text = if arguments.json {
        to_json(&PruneEnvelope {
            protocol: PROTOCOL,
            dry_run: result.dry_run,
            deleted: &result.deleted,
            skipped: &result.skipped,
        })?
    } else {
        human::prune(&result)
    };
    Ok(CommandOutput::success(text))
}

pub(super) async fn kill(
    client: &Client,
    arguments: KillArgs,
) -> Result<CommandOutput, ProtoError> {
    let id = parse_id::<WorktreeId>(&arguments.id)?;
    let text = if arguments.json {
        to_json(&OkEnvelope {
            protocol: PROTOCOL,
            ok: true,
        })?
    } else {
        format!("Killed {id}")
    };
    client.kill_worktree(id).await?;
    Ok(CommandOutput::success(text))
}

pub(super) async fn status(
    client: &Client,
    arguments: JsonArgs,
) -> Result<CommandOutput, ProtoError> {
    let statuses = client.refresh_statuses(None).await?;
    let text = if arguments.json {
        to_json(&StatusEnvelope {
            protocol: PROTOCOL,
            statuses: &statuses,
        })?
    } else {
        human::status(&statuses)
    };
    Ok(CommandOutput::success(text))
}

pub(super) async fn path(
    client: &Client,
    arguments: PathArgs,
) -> Result<CommandOutput, ProtoError> {
    let id = parse_id::<WorktreeId>(&arguments.id)?;
    Ok(CommandOutput::success(client.worktree_path(id).await?))
}

fn resolve_open_target(worktrees: Vec<Worktree>, target: &str) -> Result<Worktree, ProtoError> {
    let mut matches = worktrees.into_iter().filter(|worktree| {
        worktree.id.as_str() == target
            || worktree.session == target
            || target.split_once('/') == Some((worktree.repo_id.name(), worktree.slug.as_str()))
    });
    match (matches.next(), matches.next()) {
        (Some(worktree), None) => Ok(worktree),
        (None, _) => Err(ProtoError {
            kind: ErrorKind::NotFound,
            message: format!("worktree or session `{target}` was not found"),
        }),
        _ => Err(ProtoError {
            kind: ErrorKind::Conflict,
            message: format!("target `{target}` matches more than one worktree"),
        }),
    }
}

pub(super) fn parse_hooks(source: &str) -> Result<RepoHooks, ProtoError> {
    let value: serde_json::Value = serde_json::from_str(source)
        .map_err(|error| validation(format!("invalid hooks: {error}")))?;
    let Some(object) = value.as_object() else {
        return Err(validation("invalid hooks: expected a JSON object"));
    };
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "prepare" | "postCreate"))
    {
        return Err(validation(format!("invalid hooks: unknown field `{key}`")));
    }
    serde_json::from_value(value).map_err(|error| validation(format!("invalid hooks: {error}")))
}

pub(super) fn parse_host(host: Option<&str>) -> Result<Option<HostId>, ProtoError> {
    match host {
        None | Some("local") => Ok(None),
        Some(host) => parse_id(host).map(Some),
    }
}
