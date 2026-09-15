//! Real daemon jobs, on demand (P4-T05).
//!
//! The jobs panel, the toast stack, the coalescing rule and the sticky error slot are all
//! driven by `Event::JobUpdated` and `Event::Toast`, which no fixture can pre-bake: the job
//! registry lives in the running daemon, not in `state.json`, so a job seeded before the run
//! starts is gone by the time Fleet connects. Everything here therefore runs against the
//! *live* client of a running scenario, and every one of the four shapes below is a genuine
//! daemon job submitted through `fleet-client`'s ordinary typed operations — nothing reaches
//! into the registry, and nothing fakes an event.
//!
//! What each shape lights up, per `docs/UX-SPEC.md`'s notification law and
//! `docs/KEYMAP.md` [A18]:
//!
//! | Shape | Job | Surface |
//! | --- | --- | --- |
//! | [`Injected::Success`] | `CreateWorktree` | a `Created … · J` toast and a `succeeded` row |
//! | [`Injected::Failure`] | `Clone` | the sticky error slot, which `!` focuses |
//! | [`Injected::LongRunning`] | `PostCreateHooks` | a `running` row and the job ticker |
//! | [`Injected::Repeated`] | `DeleteWorktree` ×N | one toast carrying a count |

use super::{plan::Fixture, seed::await_job};
use anyhow::Context as _;
use fleet_client::Client;
use fleet_core::{
    ids::{ContextId, RepoId},
    model::RepoHooks,
};
use fleet_proto::job::{JobKind, JobRecord, JobStatus};

/// The prefix every injected worktree slug carries, so a dump can tell fixture scaffolding
/// from the rows the preset promised.
pub const SLUG_PREFIX: &str = "injected";
/// The repository name a failed clone is injected under; no origin of that name exists.
const UNREACHABLE: (&str, &str) = ("fixture", "unreachable");
/// How many tenth-of-a-second ticks an injected long-running hook waits before giving up.
///
/// Post-create hooks are launched detached and their job is *not* cancellable
/// (`JobPolicy::new(false, true)` in `fleet-daemon/src/services/worktrees/post_create.rs`), so
/// a hook that slept for an hour would outlive the run that started it. This one ends itself
/// after a minute whatever happens, and [`Injection::stop`] ends it sooner.
const LONG_TICKS: u32 = 600;
/// The file whose appearance in the worktree ends a long-running injected hook.
const STOP_FILE: &str = ".fleet-harness-stop";

/// One notification surface, expressed as the job that produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injected {
    /// A job that reaches `succeeded`, with the success toast the app shows for it.
    Success,
    /// A job that reaches `failed`, filling the sticky error slot with its message.
    Failure,
    /// A job that stays `running` until it is cancelled or the run ends.
    LongRunning,
    /// The same toast text `count` times inside the coalescing window, so the stack shows one
    /// row with a count rather than `count` rows.
    Repeated { count: u8 },
}

/// What one injection produced, so a scenario's runner can end or dismiss it later.
#[derive(Debug, Clone)]
pub struct Injection {
    /// The jobs the injection submitted, in submission order.
    pub jobs: Vec<JobRecord>,
    /// Where [`Injection::stop`] writes to end a long-running job early.
    stop: Option<std::path::PathBuf>,
}

impl Injection {
    /// Ends a [`Injected::LongRunning`] job now instead of letting it time itself out.
    ///
    /// Any other injection has already finished, so this is a no-op for it.
    pub fn stop(&self) -> anyhow::Result<()> {
        let Some(path) = &self.stop else {
            return Ok(());
        };
        std::fs::write(path, "stop\n").with_context(|| format!("write {}", path.display()))
    }
}

/// Submits one injection against a running daemon and returns once the app can see it.
///
/// `Success`, `Failure` and `Repeated` return after their jobs have reached a terminal state,
/// so the caller never has to guess when the toast arrives. `LongRunning` returns as soon as
/// the job is running, because waiting for it is the one thing it exists not to do.
/// `success_ordinal` belongs to the scenario run and advances before each successful-job
/// submission, so retries cannot reuse a worktree slug.
pub async fn inject(
    client: &Client,
    fixture: &Fixture,
    context: &ContextId,
    success_ordinal: &mut u8,
    injected: Injected,
) -> anyhow::Result<Injection> {
    match injected {
        Injected::Success => {
            let ordinal = *success_ordinal;
            *success_ordinal = ordinal
                .checked_add(1)
                .context("successful job injection ordinal exceeded 255")?;
            one(success(client, fixture, ordinal).await?)
        }
        Injected::LongRunning => long_running(client, fixture).await,
        Injected::Failure => one(failure(client, context).await?),
        Injected::Repeated { count } => repeated(client, fixture, count).await,
    }
}

fn one(job: JobRecord) -> anyhow::Result<Injection> {
    Ok(Injection {
        jobs: vec![job],
        stop: None,
    })
}

/// A `CreateWorktree` job, which is the smallest real unit of successful daemon work.
async fn success(client: &Client, fixture: &Fixture, ordinal: u8) -> anyhow::Result<JobRecord> {
    let repo = primary(fixture)?;
    let slug = format!("{SLUG_PREFIX}-{ordinal}");
    let result = client
        .create_worktree(repo, slug.clone(), None, None, None, RepoHooks::default())
        .await
        .map_err(|error| anyhow::anyhow!("inject a successful job as {slug}: {error}"))?;
    // `create_worktree` returns once the worktree is published, so its own job has finished;
    // the record the app sees is the one the registry holds now.
    latest(
        client,
        result.worktree.id.as_ref(),
        &JobKind::CreateWorktree,
    )
    .await
}

/// A `Clone` of an origin that does not exist: the daemon's git clone fails, the job fails,
/// and the failure lands in the sticky error slot the way any real failure does.
async fn failure(client: &Client, context: &ContextId) -> anyhow::Result<JobRecord> {
    let (owner, name) = UNREACHABLE;
    let job = client
        .clone_repo(
            owner,
            name,
            "/nonexistent/fleet-harness/unreachable.git",
            context.clone(),
            None,
        )
        .await
        .map_err(|error| anyhow::anyhow!("inject a failing clone: {error}"))?;
    let status = await_job(client, &job.id).await?;
    anyhow::ensure!(
        matches!(status, JobStatus::Failed { .. }),
        "the injected clone was expected to fail, and ended {status:?}"
    );
    record(client, &job).await
}

/// A `PostCreateHooks` job that waits, so the jobs panel has something `running` in it.
///
/// The hook polls for [`STOP_FILE`] in its own worktree and gives up after [`LONG_TICKS`],
/// which is what keeps an uncancellable detached hook from outliving the run.
async fn long_running(client: &Client, fixture: &Fixture) -> anyhow::Result<Injection> {
    let repo = primary(fixture)?;
    let slug = format!("{SLUG_PREFIX}-long");
    let result = client
        .create_worktree(
            repo,
            slug.clone(),
            None,
            None,
            None,
            RepoHooks {
                prepare: Vec::new(),
                post_create: vec![format!(
                    "tick=0; while [ ! -f {STOP_FILE} ] && [ \"$tick\" -lt {LONG_TICKS} ]; \
                     do sleep 0.1; tick=$((tick+1)); done"
                )],
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("inject a long-running job as {slug}: {error}"))?;
    let job = result
        .post_create_job
        .context("a post-create hook was configured but no job was submitted for it")?;
    Ok(Injection {
        jobs: vec![job],
        stop: Some(std::path::Path::new(&result.worktree.path).join(STOP_FILE)),
    })
}

/// `count` deletions of the same worktree, which produce `count` identical `Deleted … · J`
/// toasts and therefore exactly one toast row carrying a count.
async fn repeated(client: &Client, fixture: &Fixture, count: u8) -> anyhow::Result<Injection> {
    anyhow::ensure!(count > 0, "a repeated injection needs at least one job");
    let repo = primary(fixture)?;
    let mut jobs = Vec::new();
    for _ in 0..count {
        let result = client
            .create_worktree(
                repo.clone(),
                format!("{SLUG_PREFIX}-repeat"),
                None,
                None,
                None,
                RepoHooks::default(),
            )
            .await
            .map_err(|error| anyhow::anyhow!("inject a repeated job: {error}"))?;
        let deleted = client
            .delete_worktrees(vec![result.worktree.id.clone()])
            .await
            .map_err(|error| anyhow::anyhow!("delete the repeated injection: {error}"))?;
        for outcome in &deleted {
            anyhow::ensure!(
                outcome.ok,
                "deleting the repeated injection failed: {:?}",
                outcome.reason
            );
        }
        jobs.push(
            latest(
                client,
                result.worktree.id.as_ref(),
                &JobKind::DeleteWorktree,
            )
            .await?,
        );
    }
    Ok(Injection { jobs, stop: None })
}

fn primary(fixture: &Fixture) -> anyhow::Result<RepoId> {
    let repository = fixture
        .primary_repository()
        .context("job injection needs a seeded repository; use a preset other than `empty`")?;
    RepoId::try_from(repository.slug())
        .map_err(|error| anyhow::anyhow!("name {}: {error}", repository.slug()))
}

/// Re-reads one job from the registry, so the caller holds the record the app holds.
async fn record(client: &Client, job: &JobRecord) -> anyhow::Result<JobRecord> {
    let jobs = client
        .list_jobs()
        .await
        .map_err(|error| anyhow::anyhow!("list jobs: {error}"))?;
    jobs.into_iter()
        .find(|record| record.id == job.id)
        .with_context(|| format!("job {} left the registry before it could be read", job.id))
}

/// The most recent job of one kind whose target names `target`.
///
/// The registry keys worktree work by `<worktree id>` or `<worktree id>:<attempt uuid>`
/// (`fleet-daemon/src/services/worktrees/{creation,trash,post_create}.rs`), so the id is a
/// prefix rather than the whole target, and the kind is what separates the create from the
/// delete that followed it.
async fn latest(client: &Client, target: &str, kind: &JobKind) -> anyhow::Result<JobRecord> {
    let attempt = format!("{target}:");
    let jobs = client
        .list_jobs()
        .await
        .map_err(|error| anyhow::anyhow!("list jobs: {error}"))?;
    jobs.into_iter()
        .filter(|record| {
            &record.kind == kind && (record.target == target || record.target.starts_with(&attempt))
        })
        .max_by(|left, right| left.started_at.cmp(&right.started_at))
        .with_context(|| format!("no {kind:?} job was recorded for {target}"))
}

#[cfg(test)]
mod tests {
    use super::{Injected, inject};
    use crate::fixture::{Preset, plan, tests::boot};

    #[tokio::test]
    async fn two_success_injections_use_distinct_per_run_slugs() {
        let (booted, mut daemon, _environment) = boot(Preset::OneRepo).await;
        let context = booted.context();
        let client = daemon
            .client()
            .await
            .unwrap_or_else(|error| panic!("connect for job injection: {error:#}"));
        let fixture = plan::describe(Preset::OneRepo);
        let mut success_ordinal = 0;

        let first = inject(
            &client,
            &fixture,
            &context,
            &mut success_ordinal,
            Injected::Success,
        )
        .await
        .unwrap_or_else(|error| panic!("inject the first successful job: {error:#}"));
        let second = inject(
            &client,
            &fixture,
            &context,
            &mut success_ordinal,
            Injected::Success,
        )
        .await
        .unwrap_or_else(|error| panic!("inject the second successful job: {error:#}"));

        let first_worktree = first.jobs[0].target.split(':').next();
        let second_worktree = second.jobs[0].target.split(':').next();
        assert_eq!(first_worktree, Some("acme/api#injected-0"));
        assert_eq!(second_worktree, Some("acme/api#injected-1"));
        assert_ne!(first_worktree, second_worktree);

        drop(client);
        daemon
            .shutdown()
            .await
            .unwrap_or_else(|error| panic!("shut the verification daemon down: {error}"));
    }
}
