use super::*;

#[derive(Default)]
pub(super) struct ProjectionCache {
    key: Option<ProjectionKey>,
    model: Rc<HubModel>,
}

/// Every input the model is derived from, as revisions and cheap values.
#[derive(PartialEq, Eq)]
struct ProjectionKey {
    source: u64,
    cache: u64,
    connection: u64,
    scope: RepoScope,
    pane: HubPane,
    tab: PrTab,
    screen: Screen,
    query: String,
}

/// The Hub model for this frame, rebuilt only when one of its inputs changed.
pub(super) fn prepare(state: &AppState, hub: &HubState, now: i64) -> Rc<HubModel> {
    let mut cache = hub.projection.borrow_mut();
    let key = ProjectionKey {
        source: state.snapshot_revision,
        cache: hub.presentation_revision,
        connection: state.link_generation,
        scope: state.scope.clone(),
        pane: state.hub_pane,
        tab: state.pr_tab,
        screen: state.screen.clone(),
        query: state.filter.query.clone(),
    };
    if cache.key.as_ref() != Some(&key) {
        cache.model = Rc::new(model(state, hub, now));
        cache.key = Some(key);
    }
    cache.model.clone()
}

/// Everything both the renderer and the key handlers need, derived once per use.
#[derive(Default)]
pub struct HubModel {
    /// The rail rows, filtered when the rail owns the filter.
    pub rail: Rc<[RailRow]>,
    /// How many rail rows exist before filtering, including the pinned `All` row.
    pub repo_total: usize,
    /// The worktree rows, sorted and filtered.
    pub worktrees: Rc<[WorktreeRow]>,
    /// The pull-request rows of the active tab, filtered.
    pub prs: Rc<[PrRow]>,
    /// How many worktrees the scope holds before the filter.
    pub worktree_total: usize,
    /// How many PR rows exist before filtering, after the documented display cap.
    pub pr_total: usize,
    /// How many pull requests the §3.5 cap hid.
    pub pr_hidden: usize,
    pub(super) prepared_at: i64,
}

impl HubModel {
    pub(super) fn displayed(&self) -> crate::presentation::DisplayedHub {
        crate::presentation::DisplayedHub {
            repos: self
                .rail
                .iter()
                .map(|row| crate::presentation::DisplayedRepo {
                    kind: match row.kind {
                        RailKind::All => crate::presentation::DisplayedRepoKind::All,
                        RailKind::Repo => crate::presentation::DisplayedRepoKind::Repo,
                        RailKind::Cloning => crate::presentation::DisplayedRepoKind::Cloning,
                        RailKind::CloneFailed => {
                            crate::presentation::DisplayedRepoKind::CloneFailed
                        }
                    },
                    repo: row.repo.clone(),
                    job: row.job.clone(),
                })
                .collect(),
            repo_total: self.repo_total,
            worktrees: self
                .worktrees
                .iter()
                .map(|row| crate::presentation::DisplayedWorktree {
                    id: row.id.clone(),
                    repo: row.repo.clone(),
                })
                .collect(),
            worktree_total: self.worktree_total,
            prs: self
                .prs
                .iter()
                .map(|row| crate::presentation::DisplayedPr {
                    repo: row.repo.clone(),
                    number: row.number,
                    local: row.local.clone(),
                })
                .collect(),
            pr_total: self.pr_total,
        }
    }
}

/// Builds the whole Hub model from the snapshot and the caches.
fn model(state: &AppState, hub: &HubState, now: i64) -> HubModel {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return HubModel::default();
    };
    let index = crate::presentation::SnapshotIndex::new(snapshot);
    let context = effective_context(state).map(|context| &context.id);
    let mut scoped: Vec<&Worktree> = snapshot
        .worktrees
        .iter()
        .filter(|worktree| {
            context.is_none_or(|context| {
                index
                    .repo(&worktree.repo_id)
                    .is_some_and(|repo| &repo.context_id == context)
            })
        })
        .collect();
    let mut glyphs = repos_rail::RepoGlyphs::default();
    for worktree in &scoped {
        let status = index.status(&worktree.id);
        let unreachable = worktree
            .host
            .as_ref()
            .is_some_and(|host| index.host(host).is_some_and(|host| !host.reachable));
        let slept = index
            .sessions_for_worktree(&worktree.id)
            .iter()
            .any(|session| session.slept_at.is_some());
        let job = index
            .jobs_for_target(worktree.id.as_str())
            .iter()
            .any(|job| worktrees_list::owns_row(job));
        glyphs.add(
            &worktree.repo_id,
            crate::presentation::row_glyph(
                status.map_or(SessionState::Unknown, |status| status.session),
                slept,
                status.map_or(AgentActivity::Unknown, |status| status.agent_activity),
                worktree.degraded.is_some(),
                unreachable,
                job,
            ),
        );
    }
    let mut rail = repos_rail::rail_rows(
        context,
        &snapshot.repos,
        &snapshot.clones,
        &snapshot.worktrees,
        &glyphs,
        &snapshot.jobs,
    );
    let repo_total = rail.len();
    let query = state.filter.query.as_str();
    if state.hub_pane == HubPane::Repos {
        rail.retain(|row| repos_rail::matches(row, query));
    }
    let list_query = if state.hub_pane == HubPane::List {
        query
    } else {
        ""
    };
    scoped.retain(|worktree| match &state.scope {
        RepoScope::All => true,
        RepoScope::Repo(repo) => &worktree.repo_id == repo,
    });
    let worktree_total = scoped.len();
    let worktrees = if matches!(
        state.screen,
        Screen::Hub {
            tab: HubTab::Worktrees
        }
    ) {
        worktrees_list::sort_rows(&mut scoped);
        worktrees_list::build_rows(
            &RowInputs {
                worktrees: scoped,
                inspections: &hub.inspections,
                now,
            },
            &index,
        )
        .into_iter()
        .filter(|row| worktrees_list::matches(row, list_query))
        .collect()
    } else {
        Rc::default()
    };
    let cache_key = cache::PrCacheKey::from_state(state);
    let slice = hub.prs.slice_for(state.pr_tab, &cache_key);
    let (prs, pr_total) = if matches!(state.screen, Screen::Hub { tab: HubTab::Prs }) {
        let rows = prs_screen::build_rows(
            &PrInputs {
                slice,
                worktrees: &snapshot.worktrees,
                creating: &hub.creating,
                now,
            },
            &index,
        );
        let total = rows.len();
        (
            rows.into_iter()
                .filter(|row| prs_screen::matches(row, list_query))
                .collect(),
            total,
        )
    } else {
        (Rc::default(), 0)
    };
    HubModel {
        rail: rail.into(),
        repo_total,
        worktrees,
        prs,
        worktree_total,
        pr_total,
        pr_hidden: prs_screen::hidden_rows(slice),
        prepared_at: now,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use fleet_proto::snapshot::{DaemonInfo, Snapshot};

    use super::*;

    fn worktree(id: &str, opened: &str) -> Worktree {
        let slug = id.split_once('#').map_or(id, |(_, slug)| slug);
        Worktree {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            repo_id: "acme/api".parse().unwrap_or_else(|error| panic!("{error}")),
            slug: slug.to_owned(),
            branch: slug.to_owned(),
            base_ref: "main".to_owned(),
            path: format!("/tmp/{slug}"),
            session: id.to_owned(),
            host: None,
            created_at: "2026-09-04T09:00:00Z".to_owned(),
            last_opened_at: Some(opened.to_owned()),
            degraded: None,
        }
    }

    #[test]
    fn filter_matches_hub_projection() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_snapshot(
            Snapshot {
                boards: Vec::new(),
                generated_at: String::new(),
                contexts: Vec::new(),
                repos: Vec::new(),
                clones: Vec::new(),
                worktrees: vec![
                    worktree("acme/api#older", "2026-09-04T10:00:00Z"),
                    worktree("acme/api#newest", "2026-09-04T11:00:00Z"),
                ],
                active_context: None,
                sessions: Vec::new(),
                statuses: Vec::new(),
                pools: Vec::new(),
                hosts: Vec::new(),
                jobs: Vec::new(),
                daemon: DaemonInfo {
                    version: String::new(),
                    pid: 1,
                    started_at: String::new(),
                    home: String::new(),
                },
            },
            now,
        );
        state.filter.query = "newest".to_owned();
        let model = prepare(&state, &HubState::default(), 1_788_523_200);
        state.displayed_hub = model.displayed();

        assert_eq!(crate::presentation::filter_counts(&state), (1, 2));
        assert!(matches!(
            crate::presentation::filter_target(&state),
            Some(crate::presentation::DisplayedTarget::Worktree(id))
                if id.as_str() == "acme/api#newest"
        ));
    }
}
