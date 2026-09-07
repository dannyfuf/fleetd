//! Worktree creation: one attempt scaffold shared by branch and pull-request creation.

use super::*;

/// What one create attempt should end up containing.
struct CreatePlan<'a> {
    repo: &'a Repo,
    config: &'a Config,
    slug: &'a str,
    branch: &'a str,
    base_ref: &'a str,
}

impl Worktrees {
    /// Claims or copies a prepared slot and atomically publishes a worktree.
    pub async fn create(
        &self,
        repo: RepoId,
        slug: String,
        branch: Option<String>,
        base: Option<String>,
        hooks: RepoHooks,
    ) -> DaemonResult<(bool, Worktree, Option<JobRecord>)> {
        self.await_startup_recovery().await;
        self.jobs.ensure_repo_available(&repo)?;
        validate_slug(&slug).map_err(|error| DaemonError::Validation(error.to_string()))?;
        let requested_branch = branch.clone();
        let branch = branch.unwrap_or_else(|| slug.clone());
        validate_branch(&branch).map_err(|error| DaemonError::Validation(error.to_string()))?;

        if let Some(existing) = self
            .existing_create_result(&repo, &slug, requested_branch.as_deref())
            .await?
        {
            return Ok((false, existing, None));
        }

        let service = self.clone();
        let title = format!("Create {repo}#{slug}");
        let (delivery, awaited) = JobDelivery::caller_gets_copy(copy_error);
        self.jobs.submit(
            JobKind::CreateWorktree,
            format!("{}#{}:{}", repo, slug, Uuid::new_v4()),
            title,
            true,
            true,
            move |context| async move {
                delivery.finish(
                    service
                        .create_local(repo, slug, branch, base, hooks, &context)
                        .await,
                )
            },
        );
        awaited.wait().await
    }

    /// Creates a worktree from `refs/swarm/pulls/<n>/head`.
    pub async fn create_from_pr(
        &self,
        repo: RepoId,
        number: u64,
    ) -> DaemonResult<(bool, Worktree, Option<JobRecord>)> {
        self.await_startup_recovery().await;
        self.jobs.ensure_repo_available(&repo)?;
        let pull = self.github.pull_request(&repo, number).await?;
        let slug = pull
            .as_ref()
            .filter(|pull| !pull.is_cross_repository)
            .map(|pull| slugify(&pull.head_ref_name))
            .filter(|slug| !slug.is_empty())
            .unwrap_or_else(|| format!("pr-{number}"));
        let branch = pull
            .filter(|pull| !pull.is_cross_repository)
            .map(|pull| pull.head_ref_name)
            .unwrap_or_else(|| format!("pr/{number}"));
        if let Some(existing) = self.existing_create_result(&repo, &slug, None).await? {
            return Ok((false, existing, None));
        }
        let service = self.clone();
        let target = format!("{}#{}:{}", repo, slug, Uuid::new_v4());
        let title = format!("Create {repo} pull request #{number}");
        let (delivery, awaited) = JobDelivery::caller_gets_copy(copy_error);
        self.jobs.submit(
            JobKind::CreateWorktree,
            target,
            title,
            true,
            true,
            move |context| async move {
                delivery.finish(
                    service
                        .create_pr_local(repo, slug, branch, number, &context)
                        .await,
                )
            },
        );
        awaited.wait().await
    }

    async fn create_local(
        &self,
        repo_id: RepoId,
        slug: String,
        branch: String,
        base: Option<String>,
        hooks: RepoHooks,
        context: &JobCtx,
    ) -> DaemonResult<(bool, Worktree, Option<JobRecord>)> {
        let (mut repo, config) = self.repo_and_config(&repo_id).await?;
        repo.hooks = hooks;
        let base_ref = base.unwrap_or_else(|| format!("origin/{}", repo.default_branch));
        let plan = CreatePlan {
            repo: &repo,
            config: &config,
            slug: &slug,
            branch: &branch,
            base_ref: &base_ref,
        };
        self.create_in_attempt(plan, context, async |attempt: &Path, fresh: bool| {
            if !fresh
                || base_ref != format!("origin/{}", repo.default_branch)
                || branch != repo.default_branch
            {
                self.fetch_attempt_refs(attempt, &repo, &base_ref, Some(&branch))
                    .await?;
            }
            check_cancelled(context)?;
            if self.git.remote_branch_exists(attempt, &branch).await? {
                self.git.checkout_branch(attempt, &branch).await?;
            } else {
                if !self.git.revision_exists(attempt, &base_ref).await? {
                    return Err(DaemonError::Git(format!(
                        "base ref {base_ref} does not exist"
                    )));
                }
                self.git
                    .checkout_new_branch(attempt, &branch, &base_ref)
                    .await?;
            }
            Ok(())
        })
        .await
    }

    async fn create_pr_local(
        &self,
        repo_id: RepoId,
        slug: String,
        branch: String,
        number: u64,
        context: &JobCtx,
    ) -> DaemonResult<(bool, Worktree, Option<JobRecord>)> {
        let (repo, config) = self.repo_and_config(&repo_id).await?;
        let base_ref = format!("pull/{number}/head");
        let plan = CreatePlan {
            repo: &repo,
            config: &config,
            slug: &slug,
            branch: &branch,
            base_ref: &base_ref,
        };
        self.create_in_attempt(plan, context, async |attempt: &Path, _fresh: bool| {
            context.progress(format!("fetching pull request #{number}"))?;
            self.git.fetch_pull_request(attempt, number).await?;
            let pull_ref = format!("refs/swarm/pulls/{number}/head");
            self.git
                .checkout_force_branch(attempt, &branch, &pull_ref)
                .await
        })
        .await
    }

    /// Builds one worktree inside a private attempt directory: claim a prepared copy (or
    /// copy the pristine base), let `checkout` place the requested content, run prepare
    /// hooks unless the copy is already fresh, then publish atomically and schedule
    /// post-create hooks. A failed attempt is always discarded and the pool always refilled.
    async fn create_in_attempt(
        &self,
        plan: CreatePlan<'_>,
        context: &JobCtx,
        checkout: impl AsyncFnOnce(&Path, bool) -> DaemonResult<()>,
    ) -> DaemonResult<(bool, Worktree, Option<JobRecord>)> {
        let CreatePlan {
            repo,
            config,
            slug,
            branch,
            base_ref,
        } = plan;
        let id = worktree_id(&repo.id, slug)?;
        let root = repo_worktrees_dir(config, &repo.id);
        self.files.create_dir_all(&root)?;
        self.reclaim_publish_intent(&id, &root.join(slug)).await?;
        self.assert_create_conflicts(&id, repo, slug, branch, &root)
            .await?;

        let attempt = uuid_attempt_path(&root, slug, Uuid::new_v4());
        let mut attempt_guard = AttemptGuard::new(Arc::clone(&self.files), attempt.clone());
        context.progress("claiming prepared copy")?;
        let claimed = self
            .claim_or_fallback(repo, config, &root, &attempt, context)
            .await?;
        let result = async {
            let fresh = claimed
                && self
                    .pool
                    .is_fresh_copy(&attempt, repo, config)
                    .await
                    .unwrap_or(false);
            checkout(&attempt, fresh).await?;
            self.files.remove_file(&hot_marker_path(&attempt))?;
            if !fresh {
                hooks::run_prepare(self.shell.as_ref(), &attempt, &repo.hooks, context).await?;
            }
            let worktree = self
                .publish(repo, slug, branch, base_ref, &repo.hooks, &attempt)
                .await?;
            let post_create_job =
                self.schedule_post_create(worktree.clone(), repo.hooks.post_create.clone());
            Ok((true, worktree, post_create_job))
        }
        .await;

        if result.is_err() && self.files.exists(&attempt) {
            let cleanup = if matches!(result, Err(DaemonError::Conflict(_))) {
                self.trash_attempt(&attempt)
            } else {
                self.files.remove_detached(&attempt)
            };
            if let Err(error) = cleanup {
                tracing::warn!(%error, path = %attempt.display(), "failed to clean create attempt");
            }
        }
        attempt_guard.disarm();
        self.pool.refill(repo.id.clone());
        result
    }

    async fn claim_or_fallback(
        &self,
        repo: &Repo,
        config: &Config,
        root: &Path,
        attempt: &Path,
        context: &JobCtx,
    ) -> DaemonResult<bool> {
        let lock = self.jobs.repo_lock(&repo.id);
        let _guard = lock.lock().await;
        check_cancelled(context)?;
        if self.pool.claim_into_locked(attempt, config, root)? {
            context.progress("prepared-copy-claimed")?;
            return Ok(true);
        }
        context.progress("no prepared copy; copying pristine base")?;
        let mut copy = CancellableCopy::start(
            Arc::clone(&self.files),
            PathBuf::from(&repo.path),
            attempt.to_path_buf(),
            None,
        );
        copy.wait().await?;
        copy.disarm();
        Ok(false)
    }

    async fn fetch_attempt_refs(
        &self,
        attempt: &Path,
        repo: &Repo,
        base_ref: &str,
        requested_branch: Option<&str>,
    ) -> DaemonResult<()> {
        let default = branch_refspec(&repo.default_branch);
        let mut mandatory = vec![default];
        if let Some(base_branch) = base_ref.strip_prefix("origin/")
            && base_branch != repo.default_branch
        {
            mandatory.push(branch_refspec(base_branch));
        }
        let mut combined = mandatory.clone();
        if let Some(branch) = requested_branch
            && branch != repo.default_branch
            && base_ref != format!("origin/{branch}")
        {
            combined.push(branch_refspec(branch));
        }
        if let Err(combined_error) = self.git.fetch_refs(attempt, "origin", &combined).await {
            self.git
                .fetch_refs(attempt, "origin", &mandatory)
                .await
                .map_err(|_| combined_error)?;
        }
        if !self
            .git
            .remote_branch_exists(attempt, &repo.default_branch)
            .await?
        {
            return Err(DaemonError::Git(format!(
                "default branch origin/{} is missing",
                repo.default_branch
            )));
        }
        if base_ref != format!("origin/{}", repo.default_branch)
            && !self.git.revision_exists(attempt, base_ref).await?
        {
            return Err(DaemonError::Git(format!(
                "selected base ref {base_ref} is missing"
            )));
        }
        Ok(())
    }

    async fn existing_create_result(
        &self,
        repo: &RepoId,
        slug: &str,
        explicit_branch: Option<&str>,
    ) -> DaemonResult<Option<Worktree>> {
        let id = worktree_id(repo, slug)?;
        let state = self.state.load().await?;
        let Some(existing) = state.worktrees.into_iter().find(|item| item.id == id) else {
            return Ok(None);
        };
        if let Some(branch) = explicit_branch
            && existing.branch != branch
        {
            return Err(DaemonError::Conflict(format!(
                "worktree {id} exists with branch {}, not {branch}",
                existing.branch
            )));
        }
        Ok(Some(existing))
    }

    async fn assert_create_conflicts(
        &self,
        id: &WorktreeId,
        repo: &Repo,
        slug: &str,
        branch: &str,
        root: &Path,
    ) -> DaemonResult<()> {
        let canonical = root.join(slug);
        let session = SessionId::local(&repo.name, slug)
            .map_err(|error| DaemonError::Validation(error.to_string()))?
            .to_string();
        let state = self.state.load().await?;
        if let Some(existing) = state.worktrees.iter().find(|item| item.id == *id) {
            if existing.branch == branch {
                return Err(DaemonError::Conflict(format!(
                    "worktree {id} already exists"
                )));
            }
            return Err(DaemonError::Conflict(format!(
                "worktree {id} exists with branch {}",
                existing.branch
            )));
        }
        if state
            .worktrees
            .iter()
            .any(|item| item.session == session || item.path == canonical.to_string_lossy())
        {
            return Err(DaemonError::Conflict(format!(
                "worktree path or session conflicts for {id}"
            )));
        }
        if self.files.exists(&canonical) {
            return Err(DaemonError::Conflict(format!(
                "worktree destination already exists: {}",
                canonical.display()
            )));
        }
        Ok(())
    }
}
