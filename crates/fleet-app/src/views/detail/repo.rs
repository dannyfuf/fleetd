use super::*;

/// Everything the repository variant needs.
pub struct RepoProps<'a> {
    /// The repository under the cursor.
    pub repo: &'a Repo,
    /// How many worktrees it owns.
    pub worktrees: usize,
    /// How many of them have an attached session.
    pub live: usize,
    /// Its prepared-copy pool, when the daemon reports one.
    pub pool: Option<&'a PoolStatus>,
    /// `$HOME`, for tilde collapsing.
    pub home: &'a str,
    /// The current epoch second.
    pub now: i64,
}

/// §3.4's repo variant.
#[must_use]
pub fn repo(props: RepoProps<'_>, cx: &App) -> AnyElement {
    let RepoProps {
        repo,
        worktrees,
        live,
        pool,
        home,
        now,
    } = props;
    let pool_line = pool.map(|pool| {
        let refreshed = pool
            .refreshed_at
            .as_deref()
            .and_then(|iso| age_secs(iso, now))
            .map(|age| format!(" · refreshed {} ago", format_age(age)))
            .unwrap_or_default();
        format!("{}/{} ready{refreshed}", pool.ready, pool.size)
    });

    let mut list = KeyValueList::new()
        .row("owner", FactValue::known(repo.owner.clone()))
        .row("default", FactValue::known(repo.default_branch.clone()))
        .mono_row("path", path_value(&repo.path, home, cx))
        .row("worktrees", FactValue::known(worktrees.to_string()))
        .row("live", FactValue::known(live.to_string()))
        .row(
            "prepare",
            FactValue::from_option(
                (!repo.hooks.prepare.is_empty())
                    .then(|| format!("{} commands", repo.hooks.prepare.len())),
            ),
        )
        .row(
            "post-create",
            FactValue::from_option(
                (!repo.hooks.post_create.is_empty())
                    .then(|| format!("{} commands", repo.hooks.post_create.len())),
            ),
        )
        .mono_row("url", FactValue::known(repo.url.clone()));
    if let Some(pool_line) = pool_line {
        list = list.row("prepared", FactValue::known(pool_line));
    }

    let hooks: Vec<AnyElement> = repo
        .hooks
        .prepare
        .iter()
        .chain(repo.hooks.post_create.iter())
        .map(|command| {
            Text::data_small(truncate(command, line_budget(cx), Truncate::Tail))
                .muted()
                .into_any_element()
        })
        .collect();

    let mut children = vec![
        head(
            Icon::FolderGit2,
            SharedString::from(repo.name.clone()),
            SharedString::from(repo.id.to_string()),
            cx,
        ),
        block(vec![list.into_any_element()], cx),
    ];
    if !hooks.is_empty() {
        let mut hook_block = vec![SectionHeader::new("Hooks").into_any_element()];
        hook_block.extend(hooks);
        children.push(block(hook_block, cx));
    }
    variant(children)
}

/// §3.4's clone-job variant: status, staging path, log path, error in red.
#[must_use]
pub fn clone_job(clone: &CloneJob, home: &str, cx: &App) -> AnyElement {
    let status = match clone.status {
        CloneStatus::Starting => "starting",
        CloneStatus::Cloning => "cloning",
        CloneStatus::Failed => "failed",
    };
    let mut children = vec![
        head(
            Icon::CloudDownload,
            SharedString::from(clone.name.clone()),
            SharedString::from(clone.id.to_string()),
            cx,
        ),
        block(
            vec![
                KeyValueList::new()
                    .row("status", FactValue::known(status))
                    .mono_row("staging", path_value(&clone.staging_path, home, cx))
                    .mono_row("log", path_value(&clone.log_path, home, cx))
                    .into_any_element(),
            ],
            cx,
        ),
    ];
    if let Some(error) = clone.error.as_deref() {
        children.push(block(
            vec![
                Text::ui(error.to_owned())
                    .tone(Tone::Danger)
                    .into_any_element(),
                Text::hint("\u{23CE}  open in the jobs panel").into_any_element(),
            ],
            cx,
        ));
    }
    variant(children)
}
