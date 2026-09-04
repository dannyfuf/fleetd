//! Pull-request querying, caching, and assignment workflow orchestration.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use fleet_core::{
    cache::PrCache,
    github::{PrTab, PullRequest, worktree_matches_pr},
    ids::{ContextId, JobId, RepoId},
    model::Worktree,
    paths::FleetHome,
};
use fleet_proto::{job::JobKind, response::PrSlice};
use futures_util::future::join_all;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, github::Github as GithubAdapter},
    jobs::JobManager,
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
    in_flight: Arc<Mutex<HashMap<String, JobId>>>,
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
        }
    }

    /// Returns the authenticated GitHub viewer while respecting the global GitHub limit.
    ///
    /// # Errors
    ///
    /// Returns a GitHub error when the CLI lookup fails or yields an empty login, and a
    /// cancellation error when the detached job is cancelled before delivering its result.
    pub async fn viewer_login(&self) -> DaemonResult<String> {
        let github = Arc::clone(&self.github);
        let semaphore = self.jobs.github_semaphore();
        let (sender, receiver) = oneshot::channel::<Result<String, String>>();
        let sender = Arc::new(Mutex::new(Some(sender)));
        self.jobs.submit(
            JobKind::PrFetch,
            format!("viewer:{}", Uuid::new_v4()),
            "Read GitHub viewer",
            true,
            true,
            move |context| async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|error| DaemonError::Join(error.to_string()))?;
                context.progress("reading GitHub viewer")?;
                match github.viewer_login().await.and_then(|login| {
                    if login.is_empty() {
                        Err(DaemonError::Github(
                            "GitHub viewer login was empty".to_owned(),
                        ))
                    } else {
                        Ok(login)
                    }
                }) {
                    Ok(login) => {
                        if let Some(sender) = sender
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                        {
                            let _ignored = sender.send(Ok(login));
                        }
                        Ok(())
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if let Some(sender) = sender
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                        {
                            let _ignored = sender.send(Err(message));
                        }
                        Err(error)
                    }
                }
            },
        );
        match receiver.await {
            Ok(Ok(login)) => Ok(login),
            Ok(Err(error)) => Err(DaemonError::Github(error)),
            Err(_) => Err(DaemonError::Cancelled),
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
        let cached = read_cache(self.files.as_ref(), &path);
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
        if let Some(previous) = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()
        {
            let _ignored = self.jobs.cancel(&previous);
        }
        let github = Arc::clone(&self.github);
        let semaphore = self.jobs.github_semaphore();
        let repo_for_job = repo.clone();
        let path_for_job = path.clone();
        let files = Arc::clone(&self.files);
        let (sender, receiver) = oneshot::channel::<Result<PrCache, String>>();
        let sender = Arc::new(Mutex::new(Some(sender)));
        let id = self.jobs.submit(
            JobKind::PrFetch,
            format!("{key}:{}", Uuid::new_v4()),
            format!("Fetch {tab} pull requests for {repo}"),
            true,
            true,
            move |context| async move {
                let result: DaemonResult<PrCache> = async {
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
                    let cache = PrCache {
                        fetched_at: Utc::now().to_rfc3339(),
                        prs,
                    };
                    write_cache(files.as_ref(), &path_for_job, &cache)?;
                    Ok(cache)
                }
                .await;
                match result {
                    Ok(cache) => {
                        if let Some(sender) = sender
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                        {
                            let _ignored = sender.send(Ok(cache));
                        }
                        Ok(())
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if let Some(sender) = sender
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                        {
                            let _ignored = sender.send(Err(message));
                        }
                        Err(error)
                    }
                }
            },
        );
        self.in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.clone(), id.clone());
        let result = receiver.await;
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if in_flight.get(&key) == Some(&id) {
            in_flight.remove(&key);
        }
        drop(in_flight);

        match result {
            Ok(Ok(cache)) => RepoTabOutcome { cache, error: None },
            Ok(Err(error)) => RepoTabOutcome {
                cache: cached.unwrap_or_else(empty_cache),
                error: Some(concise(&error, 120)),
            },
            Err(_) => RepoTabOutcome {
                cache: cached.unwrap_or_else(empty_cache),
                error: Some("operation cancelled".to_owned()),
            },
        }
    }
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

fn cache_is_fresh(fetched_at: &str, ttl_seconds: i64) -> bool {
    if ttl_seconds < 0 {
        return false;
    }
    DateTime::parse_from_rfc3339(fetched_at)
        .ok()
        .map(|fetched| Utc::now().signed_duration_since(fetched.with_timezone(&Utc)))
        .is_some_and(|age| age.num_milliseconds() >= 0 && age.num_seconds() < ttl_seconds)
}

fn read_cache(files: &dyn Files, path: &Path) -> Option<PrCache> {
    files
        .read_text(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn write_cache(files: &dyn Files, path: &Path, cache: &PrCache) -> DaemonResult<()> {
    let mut text = serde_json::to_string(cache)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

fn fleet_home(state: &StateStore) -> DaemonResult<FleetHome> {
    state
        .path()
        .parent()
        .map(|path| FleetHome::new(path.to_path_buf()))
        .ok_or_else(|| DaemonError::Validation("state path has no parent".to_owned()))
}

fn concise(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect::<String>()
}
