//! Pull-request querying, caching, and assignment workflow orchestration.

use super::cache::{cache_is_fresh, fleet_home, read_cache, write_cache};

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::Utc;
use fleet_core::{
    cache::PrCache,
    github::{PrTab, PullRequest, worktree_matches_pr},
    ids::{ContextId, RepoId},
    model::Worktree,
    paths::FleetHome,
};
use fleet_proto::{job::JobKind, response::PrSlice};
use futures_util::future::join_all;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, github::Github as GithubAdapter},
    jobs::{JobManager, JobPolicy},
    stores::{config::ConfigStore, state::StateStore},
};

/// Pull-request service with cache and global GitHub concurrency handles.
#[derive(Clone)]
pub struct Github {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    github: Arc<dyn GithubAdapter>,
    files: Arc<dyn Files>,
    in_flight: Arc<Mutex<HashMap<String, PrFetchFlight>>>,
    generations: Arc<Mutex<HashMap<String, u64>>>,
    next_generation: Arc<AtomicU64>,
}

impl Github {
    /// Creates the pull-request service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        github: Arc<dyn GithubAdapter>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            config,
            state,
            jobs,
            github,
            files,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            generations: Arc::new(Mutex::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Loads cached PR slices and refreshes them with global concurrency four.
    pub async fn list_pull_requests(
        &self,
        repo: Option<RepoId>,
        context: Option<ContextId>,
        tab: PrTab,
        force: bool,
    ) -> DaemonResult<Vec<PrSlice>> {
        if repo.is_some() && context.is_some() {
            return Err(DaemonError::Validation(
                "repository and context PR scopes are mutually exclusive".to_owned(),
            ));
        }
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let selected = if let Some(repo) = repo {
            vec![
                state
                    .repos
                    .iter()
                    .find(|item| item.id == repo)
                    .cloned()
                    .ok_or_else(|| DaemonError::NotFound(format!("repository {repo}")))?,
            ]
        } else {
            let context = context.or_else(|| state.active_context_id.clone());
            if let Some(context) = &context
                && !state.contexts.iter().any(|item| &item.id == context)
            {
                return Err(DaemonError::NotFound(format!("context {context}")));
            }
            state
                .repos
                .iter()
                .filter(|item| context.as_ref().is_none_or(|id| &item.context_id == id))
                .cloned()
                .collect::<Vec<_>>()
        };
        if selected.is_empty() {
            return Ok(vec![PrSlice {
                tab,
                fetched_at: String::new(),
                loading: false,
                error: None,
                total: 0,
                prs: Vec::new(),
            }]);
        }

        let home = fleet_home(&self.state)?;
        let ttl = config.github.pr_ttl_seconds;
        let outcomes = join_all(
            selected
                .iter()
                .map(|item| self.fetch_repo_tab(home.clone(), item.id.clone(), tab, ttl, force)),
        )
        .await;
        let mut prs = Vec::new();
        let mut fetched_at = Vec::new();
        let mut errors = Vec::new();
        for outcome in outcomes {
            if !outcome.cache.fetched_at.is_empty() {
                fetched_at.push(outcome.cache.fetched_at.clone());
            }
            prs.extend(outcome.cache.prs);
            if let Some(error) = outcome.error {
                errors.push(error);
            }
        }
        prs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        let total = prs.len();
        if selected.len() > 1 {
            prs.truncate(100);
        }
        fetched_at.sort();
        errors.sort();
        errors.dedup();
        Ok(vec![PrSlice {
            tab,
            fetched_at: fetched_at.into_iter().next().unwrap_or_default(),
            loading: false,
            error: if errors.is_empty() {
                None
            } else {
                Some(concise(&errors.join(" · "), 120))
            },
            total,
            prs,
        }])
    }

    /// Finds the registered worktree represented by a pull request using core matching rules.
    pub async fn matching_worktree(
        &self,
        pull_request: &PullRequest,
    ) -> DaemonResult<Option<Worktree>> {
        Ok(self
            .state
            .load()
            .await?
            .worktrees
            .into_iter()
            .find(|worktree| worktree_matches_pr(worktree, pull_request)))
    }

    async fn fetch_repo_tab(
        &self,
        home: FleetHome,
        repo: RepoId,
        tab: PrTab,
        ttl_seconds: i64,
        force: bool,
    ) -> RepoTabOutcome {
        let path = home.pr_cache_path(&repo, tab);
        let cached = read_cache::<PrCache>(self.files.as_ref(), &path);
        if !force
            && cached
                .as_ref()
                .is_some_and(|cache| cache_is_fresh(&cache.fetched_at, ttl_seconds))
        {
            return RepoTabOutcome {
                cache: cached.unwrap_or_else(empty_cache),
                error: None,
            };
        }

        let key = format!("{repo}:{tab}");
        let receiver = {
            let mut flights = self
                .in_flight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(flight) = flights.get(&key)
                && flight.receiver.has_changed().is_ok()
                && (!force || flight.forced)
            {
                flight.receiver.clone()
            } else {
                let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                self.generations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(key.clone(), generation);
                let (sender, receiver) = watch::channel(None);
                let github = Arc::clone(&self.github);
                let semaphore = self.jobs.github_semaphore();
                let repo_for_job = repo.clone();
                let path_for_job = path.clone();
                let files = Arc::clone(&self.files);
                let generations = Arc::clone(&self.generations);
                let key_for_job = key.clone();
                let admission_sender = sender.clone();
                let delivery = PrFetchDelivery::new(
                    sender,
                    Arc::clone(&self.in_flight),
                    key.clone(),
                    generation,
                );
                flights.insert(
                    key.clone(),
                    PrFetchFlight {
                        forced: force,
                        generation,
                        receiver: receiver.clone(),
                    },
                );
                drop(flights);
                let submission = self.jobs.submit_for_repo(
                    repo.clone(),
                    JobKind::PrFetch,
                    format!("{key}:{}", Uuid::new_v4()),
                    format!("Fetch {tab} pull requests for {repo}"),
                    JobPolicy::new(true, true),
                    move |context| async move {
                        let mut result: DaemonResult<PrCache> = async {
                            let _permit = semaphore
                                .acquire_owned()
                                .await
                                .map_err(|error| DaemonError::Join(error.to_string()))?;
                            context.progress(format!("fetching {tab} pull requests"))?;
                            let mut prs = github.list_pull_requests(&repo_for_job, tab).await?;
                            if prs.iter().any(|pull| !pull.has_valid_url()) {
                                return Err(DaemonError::Github(format!(
                                    "invalid pull-request URL returned for {repo_for_job}"
                                )));
                            }
                            prs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                            Ok(PrCache {
                                fetched_at: Utc::now().to_rfc3339(),
                                prs,
                            })
                        }
                        .await;

                        let generations = generations
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let is_current = generations
                            .get(&key_for_job)
                            .is_some_and(|current| *current == generation);
                        if is_current && let Ok(cache) = &result {
                            result = write_cache(files.as_ref(), &path_for_job, cache)
                                .map(|()| cache.clone());
                        }
                        drop(generations);
                        let shared = match &result {
                            Ok(cache) => SharedFetchOutcome::Success(cache.clone()),
                            Err(error) => SharedFetchOutcome::Failure(error.to_string()),
                        };
                        delivery.publish(shared);
                        result.map(drop)
                    },
                );
                if let Err(error) = submission {
                    admission_sender
                        .send_replace(Some(SharedFetchOutcome::Failure(error.to_string())));
                }
                receiver
            }
        };
        await_fetch(receiver, cached).await
    }
}

#[derive(Clone)]
struct PrFetchFlight {
    forced: bool,
    generation: u64,
    receiver: watch::Receiver<Option<SharedFetchOutcome>>,
}

struct PrFetchDelivery {
    sender: Option<watch::Sender<Option<SharedFetchOutcome>>>,
    in_flight: Arc<Mutex<HashMap<String, PrFetchFlight>>>,
    key: String,
    generation: u64,
}

impl Clone for PrFetchDelivery {
    fn clone(&self) -> Self {
        Self {
            sender: None,
            in_flight: Arc::clone(&self.in_flight),
            key: self.key.clone(),
            generation: self.generation,
        }
    }
}

impl PrFetchDelivery {
    fn new(
        sender: watch::Sender<Option<SharedFetchOutcome>>,
        in_flight: Arc<Mutex<HashMap<String, PrFetchFlight>>>,
        key: String,
        generation: u64,
    ) -> Self {
        Self {
            sender: Some(sender),
            in_flight,
            key,
            generation,
        }
    }

    fn publish(&self, outcome: SharedFetchOutcome) {
        if let Some(sender) = &self.sender {
            sender.send_replace(Some(outcome));
        }
    }
}

impl Drop for PrFetchDelivery {
    fn drop(&mut self) {
        if self.sender.is_none() {
            return;
        }
        let mut flights = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if flights
            .get(&self.key)
            .is_some_and(|flight| flight.generation == self.generation)
        {
            flights.remove(&self.key);
        }
    }
}

#[derive(Clone)]
enum SharedFetchOutcome {
    Success(PrCache),
    Failure(String),
}

struct RepoTabOutcome {
    cache: PrCache,
    error: Option<String>,
}

fn empty_cache() -> PrCache {
    PrCache {
        fetched_at: String::new(),
        prs: Vec::new(),
    }
}

async fn await_fetch(
    mut receiver: watch::Receiver<Option<SharedFetchOutcome>>,
    cached: Option<PrCache>,
) -> RepoTabOutcome {
    loop {
        if let Some(outcome) = receiver.borrow().clone() {
            return match outcome {
                SharedFetchOutcome::Success(cache) => RepoTabOutcome { cache, error: None },
                SharedFetchOutcome::Failure(error) => RepoTabOutcome {
                    cache: cached.unwrap_or_else(empty_cache),
                    error: Some(concise(&error, 120)),
                },
            };
        }
        if receiver.changed().await.is_err() {
            return RepoTabOutcome {
                cache: cached.unwrap_or_else(empty_cache),
                error: Some("operation cancelled".to_owned()),
            };
        }
    }
}

fn concise(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect::<String>()
}
