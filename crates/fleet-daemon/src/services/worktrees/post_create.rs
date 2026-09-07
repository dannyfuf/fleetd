//! Post-create hooks run detached, so they survive a daemon restart and are reconciled
//! from a persisted intent.

use super::*;

const RUNNER: &str = r#"status=0
failed=0
index=0
for hook do
  index=$((index + 1))
  printf 'post-create: %s\n' "$hook"
  sh -c "$hook"
  code=$?
  if [ "$code" -ne 0 ] && [ "$status" -eq 0 ]; then
    status=$code
    failed=$index
  fi
done
tmp="${FLEET_STATUS_PATH}.tmp.$$"
printf '%s %s\n' "$failed" "$status" > "$tmp"
mv "$tmp" "$FLEET_STATUS_PATH"
exit "$status""#;

impl Worktrees {
    /// Schedules the detached post-create runner, or nothing when a repository has no hooks.
    pub(super) fn schedule_post_create(
        &self,
        worktree: Worktree,
        hooks: Vec<String>,
    ) -> DaemonResult<Option<JobRecord>> {
        if hooks.is_empty() {
            return Ok(None);
        }
        let service = self.clone();
        let target = worktree.id.to_string();
        let repo = worktree.repo_id.clone();
        let id = self.jobs.submit_for_repo(
            repo,
            JobKind::PostCreateHooks,
            target,
            format!("Run hooks for {}", worktree.id),
            JobPolicy::new(false, true),
            move |context| async move {
                service
                    .run_detached_post_create(worktree, hooks, &context)
                    .await
            },
        )?;
        Ok(self.jobs.record(&id))
    }

    async fn run_detached_post_create(
        &self,
        worktree: Worktree,
        hooks: Vec<String>,
        context: &JobCtx,
    ) -> DaemonResult<()> {
        let log_path = self.jobs.log_path(&context.id);
        let status_path = log_path.with_extension("post-create.status");
        let intent_path = self
            .post_create_intents_dir()?
            .join(format!("{}.json", context.id));
        let intent = PostCreateIntent {
            worktree,
            hooks,
            pid: None,
            status_path,
            log_path,
            intent_path,
        };
        self.write_post_create_intent(&intent)?;
        self.launch_detached_post_create(intent, context, RUNNER)
            .await
    }

    async fn launch_detached_post_create(
        &self,
        mut intent: PostCreateIntent,
        context: &JobCtx,
        runner: &str,
    ) -> DaemonResult<()> {
        let command = ShellCommand::new("sh")
            .args(
                [
                    "-c".to_owned(),
                    runner.to_owned(),
                    "fleet-post-create".to_owned(),
                ]
                .into_iter()
                .chain(intent.hooks.iter().cloned()),
            )
            .cwd(Path::new(&intent.worktree.path))
            .env(
                "FLEET_STATUS_PATH",
                intent.status_path.to_string_lossy().into_owned(),
            );
        context.progress("post-create runner detached")?;
        let process = context
            .spawn_detached_child(Arc::clone(&self.shell), command, &intent.log_path)
            .await?;
        intent.pid = Some(process.pid);
        self.write_post_create_intent(&intent)?;
        self.finish_detached_post_create(intent).await
    }

    pub(super) async fn finish_detached_post_create(
        &self,
        intent: PostCreateIntent,
    ) -> DaemonResult<()> {
        if let Some(pid) = intent.pid {
            while pid_is_alive(pid) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        let status = match self.files.read_text(&intent.status_path) {
            Ok(status) => status,
            Err(error) => {
                let recorded = self
                    .record_post_create_failure(
                        &intent.worktree,
                        "detached hook runner did not record completion".to_owned(),
                        None,
                        intent.log_path.clone(),
                    )
                    .await;
                if recorded.is_ok() {
                    self.cleanup_post_create_intent(&intent);
                }
                return Err(error);
            }
        };
        let mut fields = status.split_whitespace();
        let failed = fields
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let exit_code = fields
            .next()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(-1);
        if exit_code == 0 {
            let cleared = self.clear_post_create_failure(&intent.worktree).await;
            self.cleanup_post_create_intent(&intent);
            return cleared;
        }
        let command = intent
            .hooks
            .get(failed.saturating_sub(1))
            .map_or("unknown hook", String::as_str);
        self.record_post_create_failure(
            &intent.worktree,
            format!("hook {failed}: {command}"),
            Some(exit_code),
            intent.log_path.clone(),
        )
        .await?;
        self.cleanup_post_create_intent(&intent);
        Err(DaemonError::Shell(
            "one or more post-create hooks failed".to_owned(),
        ))
    }

    fn cleanup_post_create_intent(&self, intent: &PostCreateIntent) {
        if let Err(error) = self.files.remove_file(&intent.status_path) {
            tracing::warn!(%error, path = %intent.status_path.display(), "failed to remove completed hook status");
        }
        if let Err(error) = self.files.remove_file(&intent.intent_path) {
            tracing::warn!(%error, path = %intent.intent_path.display(), "failed to remove completed hook intent");
        }
    }

    fn write_post_create_intent(&self, intent: &PostCreateIntent) -> DaemonResult<()> {
        let directory = self.post_create_intents_dir()?;
        self.files.create_dir_all(&directory)?;
        let mut text = serde_json::to_string_pretty(intent)?;
        text.push('\n');
        self.files.atomic_write_text(&intent.intent_path, &text)
    }

    pub(super) async fn recover_post_create_intents(&self) -> DaemonResult<()> {
        let directory = self.post_create_intents_dir()?;
        if !self.files.exists(&directory) {
            return Ok(());
        }
        let registered = self
            .state
            .load()
            .await?
            .worktrees
            .into_iter()
            .map(|worktree| worktree.id)
            .collect::<std::collections::BTreeSet<_>>();
        for path in self.files.list(&directory)? {
            let Ok(text) = self.files.read_text(&path) else {
                continue;
            };
            let Ok(mut intent) = serde_json::from_str::<PostCreateIntent>(&text) else {
                continue;
            };
            intent.intent_path = path;
            if !registered.contains(&intent.worktree.id) {
                let _ignored = self.files.remove_file(&intent.status_path);
                let _ignored = self.files.remove_file(&intent.intent_path);
                continue;
            }
            let service = self.clone();
            let target = intent.worktree.id.to_string();
            let repo = intent.worktree.repo_id.clone();
            self.jobs.submit_for_repo(
                repo,
                JobKind::PostCreateHooks,
                target,
                format!("Reconcile hooks for {}", intent.worktree.id),
                JobPolicy::new(false, false),
                move |context| async move {
                    if intent.pid.is_some() || service.files.exists(&intent.status_path) {
                        service.finish_detached_post_create(intent).await
                    } else {
                        service
                            .launch_detached_post_create(intent, &context, RUNNER)
                            .await
                    }
                },
            )?;
        }
        Ok(())
    }

    fn post_create_intents_dir(&self) -> DaemonResult<PathBuf> {
        self.state
            .path()
            .parent()
            .map(|home| home.join("cache").join("post-create"))
            .ok_or_else(|| DaemonError::Validation("state path has no parent".to_owned()))
    }

    async fn record_post_create_failure(
        &self,
        worktree: &Worktree,
        step: String,
        exit_code: Option<i32>,
        log_path: PathBuf,
    ) -> DaemonResult<()> {
        let id = worktree.id.clone();
        let degraded = Degraded {
            kind: "post_create_hooks".to_owned(),
            step,
            exit_code,
            at: Utc::now().to_rfc3339(),
            log_path: log_path.to_string_lossy().into_owned(),
        };
        self.state
            .transaction(move |state| {
                let item = state
                    .worktrees
                    .iter_mut()
                    .find(|item| item.id == id)
                    .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
                item.degraded = Some(degraded);
                Ok(())
            })
            .await
    }

    async fn clear_post_create_failure(&self, worktree: &Worktree) -> DaemonResult<()> {
        let id = worktree.id.clone();
        self.state
            .transaction(move |state| {
                if let Some(item) = state.worktrees.iter_mut().find(|item| item.id == id)
                    && item
                        .degraded
                        .as_ref()
                        .is_some_and(|degraded| degraded.kind == "post_create_hooks")
                {
                    item.degraded = None;
                }
                Ok(())
            })
            .await
    }
}
