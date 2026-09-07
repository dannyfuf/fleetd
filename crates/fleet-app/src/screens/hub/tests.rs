use fleet_core::{
    github::{PrChecks, PrReviewDecision},
    ids::RepoId,
    inspection::WorktreeInspection,
    model::{Context, Repo, RepoHooks},
    sessions::SessionState,
};
use fleet_proto::{response::PrSlice, snapshot::Snapshot};
use fleet_ui_kit::PrBadgeState;
use std::collections::VecDeque;

use super::*;

struct PendingHubRequest {
    body: RequestBody,
    reply: async_channel::Sender<Result<ResponseBody, ProtoError>>,
}

#[derive(Clone, Default)]
struct HubRequestHarness(Rc<RefCell<VecDeque<PendingHubRequest>>>);

impl HubRequestHarness {
    fn bridge(&self) -> HubBridge {
        let pending = self.0.clone();
        HubBridge::Test(Rc::new(move |body| {
            let (reply, response) = async_channel::bounded(1);
            pending
                .borrow_mut()
                .push_back(PendingHubRequest { body, reply });
            response
        }))
    }

    fn len(&self) -> usize {
        self.0.borrow().len()
    }

    fn respond_next(&self, response: Result<ResponseBody, ProtoError>) -> RequestBody {
        let request = self
            .0
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| panic!("expected a pending Hub request"));
        request
            .reply
            .try_send(response)
            .unwrap_or_else(|_| panic!("Hub reply receiver should still be live"));
        request.body
    }

    fn close_next(&self) -> RequestBody {
        self.0
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| panic!("expected a pending Hub request"))
            .body
    }
}

fn test_hub_ctx(state: AppState, cx: &mut gpui::TestAppContext) -> (HubCtx, HubRequestHarness) {
    let harness = HubRequestHarness::default();
    let state = cx.new(|_| state);
    let hub = cx.new(|_| HubState::default());
    (
        HubCtx {
            state,
            hub,
            bridge: harness.bridge(),
            rail_scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            pr_scroll: UniformListScrollHandle::new(),
        },
        harness,
    )
}

fn inspection(dirty: bool, unique: Option<u64>, merged: bool) -> WorktreeInspection {
    WorktreeInspection {
        worktree_id: WorktreeId::try_from("buk/payroll#a")
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        host: "local".to_owned(),
        path: "/tmp/wt".to_owned(),
        branch: "feat/x".to_owned(),
        base_ref: "origin/main".to_owned(),
        head: Some("abc".to_owned()),
        target_branch: "main".to_owned(),
        upstream: None,
        ahead: None,
        behind: None,
        upstream_gone: false,
        dirty,
        dirty_files: dirty.then_some(12),
        merged_into_target: merged,
        unique_commits: unique,
        published: true,
        merged,
        pr: None,
        session: SessionState::None,
        running: Vec::new(),
        inspected_at: "2026-09-04T11:59:00Z".to_owned(),
        warnings: Vec::new(),
        error: None,
    }
}

/// The default theme metrics, which is what the documented §2.1 frame is measured against.
fn pane_ch(width: f32, rail_collapsed: bool, detail_open: bool) -> f32 {
    list_pane_ch(
        width,
        rail_collapsed,
        detail_open,
        &fleet_ui_kit::theme::Metrics::default(),
    )
}

#[test]
fn the_ladder_matches_the_documented_frame() {
    // §2.1: 1280 px gives the list 138 ch, and 93 ch with the detail panel inset.
    assert!((pane_ch(1280.0, false, false) - 138.6).abs() < 0.2);
    assert!((pane_ch(1280.0, true, false) - 164.8).abs() < 0.2);
    assert!((pane_ch(1280.0, false, true) - 93.3).abs() < 0.2);
}

#[test]
fn a_narrow_window_docks_the_panel_instead_of_stealing_width() {
    assert!(detail_is_docked(1100.0));
    assert!(!detail_is_docked(1280.0));
    // Docked, the list keeps its full width.
    let docked = pane_ch(1100.0, false, true);
    assert!((docked - pane_ch(1100.0, false, false)).abs() < f32::EPSILON);
    assert!(docked > 72.0, "never below the 72 ch floor of §3.4");
}

#[test]
fn the_pr_cache_expires_on_the_documented_ttl() {
    let mut cache = PrCache::default();
    let now = Instant::now();
    assert!(cache.needs_fetch(now));
    assert!(cache.is_cold(PrTab::Mine));
    let state = scoped_state(RepoScope::All, 1);
    let key = cache::PrCacheKey::from_state(&state);
    let requests = cache.begin_fetch(key.clone(), now);
    assert!(!cache.needs_fetch(now), "a running fetch is not repeated");
    assert!(cache.apply(
        &key,
        PrTab::Mine,
        requests[0].1,
        pr_response(PrTab::Mine),
        now
    ));
    assert!(cache.apply(
        &key,
        PrTab::Review,
        requests[1].1,
        pr_response(PrTab::Review),
        now
    ));
    assert!(!cache.needs_fetch(now));
    assert!(!cache.needs_fetch(now + PR_TTL - Duration::from_secs(1)));
    assert!(cache.needs_fetch(now + PR_TTL));

    let abandoned = cache.begin_fetch(key.clone(), now);
    assert!(cache.apply(
        &key,
        PrTab::Review,
        abandoned[1].1,
        pr_response(PrTab::Review),
        now + Duration::from_secs(1),
    ));
    assert!(cache.loading(PrTab::Mine));
    assert!(!cache.loading(PrTab::Review));
    assert!(!cache.needs_fetch(now + Duration::from_secs(1)));
    assert!(cache.needs_fetch(now + PR_TTL));
    assert!(
        cache.loading(PrTab::Mine),
        "an expired in-flight request remains visibly identifiable while a retry is allowed"
    );
}

#[test]
fn an_inspection_slot_never_blanks_its_previous_values() {
    let mut slot = Inspected::ready(inspection(true, Some(1), false));
    slot.loading = true;
    assert!(slot.data.is_some());
    assert_eq!(
        Inspected::failed("gh unavailable").error.as_deref(),
        Some("gh unavailable")
    );
}

fn snapshot(count: usize) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        contexts: vec![],
        repos: vec![],
        clones: vec![],
        worktrees: (0..count)
            .map(|index| Worktree {
                id: format!("acme/api#branch-{index}")
                    .parse()
                    .expect("worktree id"),
                repo_id: "acme/api".parse().expect("repo id"),
                slug: format!("branch-{index}"),
                branch: format!("branch-{index}"),
                base_ref: "main".into(),
                path: "/tmp/worktree".into(),
                session: format!("acme/api#branch-{index}"),
                host: None,
                created_at: "2026-09-04T11:59:00Z".into(),
                last_opened_at: None,
                degraded: None,
            })
            .collect(),
        active_context: None,
        sessions: vec![],
        statuses: vec![],
        pools: vec![],
        hosts: vec![],
        jobs: vec![],
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    }
}

fn context(id: &str) -> Context {
    Context {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        name: id.to_owned(),
        owners: vec![id.to_owned()],
        created_at: "2026-09-04T09:00:00Z".to_owned(),
    }
}

fn repo(id: &str, context: &str) -> Repo {
    let parsed: RepoId = id.parse().unwrap_or_else(|error| panic!("{error}"));
    Repo {
        owner: parsed.owner().to_owned(),
        name: parsed.name().to_owned(),
        id: parsed,
        url: format!("https://github.com/{id}.git"),
        context_id: context.parse().unwrap_or_else(|error| panic!("{error}")),
        default_branch: "main".to_owned(),
        path: format!("/tmp/{id}"),
        cloned_at: "2026-09-04T09:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    }
}

fn worktree(id: &str, repo: &str, created_at: &str) -> Worktree {
    let slug = id.split_once('#').map_or(id, |(_, slug)| slug);
    Worktree {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        repo_id: repo.parse().unwrap_or_else(|error| panic!("{error}")),
        slug: slug.to_owned(),
        branch: slug.to_owned(),
        base_ref: "main".to_owned(),
        path: format!("/tmp/{slug}"),
        session: id.to_owned(),
        host: None,
        created_at: created_at.to_owned(),
        last_opened_at: None,
        degraded: None,
    }
}

fn pr_slice(tab: PrTab) -> PrSlice {
    PrSlice {
        tab,
        fetched_at: "2026-09-04T12:00:00Z".to_owned(),
        loading: false,
        error: None,
        total: 0,
        prs: Vec::new(),
    }
}

fn pr_response(tab: PrTab) -> Result<ResponseBody, ProtoError> {
    Ok(ResponseBody::PullRequests(vec![pr_slice(tab)]))
}

fn pull_request(repo: &str, number: u64, updated_at: &str) -> PullRequest {
    PullRequest {
        repo_id: repo.parse().unwrap_or_else(|error| panic!("{error}")),
        number,
        title: format!("PR {number}"),
        url: format!("https://github.com/{repo}/pull/{number}"),
        author: "octocat".to_owned(),
        head_ref_name: format!("feature-{number}"),
        base_ref_name: "main".to_owned(),
        is_draft: false,
        is_cross_repository: false,
        head_repo: None,
        review_decision: PrReviewDecision::None,
        checks: PrChecks::Pass,
        checks_passed: None,
        checks_total: None,
        additions: 1,
        deletions: 1,
        labels: Vec::new(),
        updated_at: updated_at.to_owned(),
    }
}

fn pr_response_with(tab: PrTab, prs: Vec<PullRequest>) -> Result<ResponseBody, ProtoError> {
    Ok(ResponseBody::PullRequests(vec![PrSlice {
        tab,
        fetched_at: "2026-09-04T12:00:00Z".to_owned(),
        loading: false,
        error: None,
        total: prs.len(),
        prs,
    }]))
}

fn scoped_state(scope: RepoScope, generation: u64) -> AppState {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-cache", now);
    state.scope = scope;
    state.link_generation = generation;
    state
}

/// A full frame for a terminal no Hub row mentions.
fn frame() -> fleet_proto::terminal::FrameUpdate {
    use fleet_proto::terminal::{CursorShape, CursorState, TerminalModes, ViewportInfo};
    fleet_proto::terminal::FrameUpdate {
        terminal: fleet_core::ids::TerminalId(1),
        seq: 1,
        cols: 1,
        rows: 1,
        full: true,
        shift: None,
        rows_changed: Vec::new(),
        cursor: CursorState {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
        },
        viewport: ViewportInfo {
            scrollback_len: 0,
            offset: 0,
            history_epoch: 0,
        },
        modes: TerminalModes::default(),
        title: None,
    }
}

#[test]
fn large_projection_is_shared_across_clock_cursor_and_terminal_only_updates() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-test", now);
    state.apply_snapshot(snapshot(10_000), now);
    let hub = HubState::default();
    let initial = projection::prepare(&state, &hub, 1_788_523_200);
    assert_eq!(initial.worktrees.len(), 10_000);
    assert!(initial.prs.is_empty());

    // Neither the clock nor the cursor is a projection input: 10 000 rows are built once.
    for tick in 0..20 {
        state.cursors.worktrees = tick;
        let cached = projection::prepare(&state, &hub, 1_788_523_201 + tick as i64);
        assert!(Rc::ptr_eq(&initial, &cached));
    }

    // Terminal output moves no snapshot revision, so it never rebuilds the Hub either.
    state.apply_frame(&frame());
    assert!(Rc::ptr_eq(
        &initial,
        &projection::prepare(&state, &hub, 1_788_523_220)
    ));

    let mut changed = snapshot(10_000);
    changed.worktrees[0].branch = "changed branch".into();
    state.apply_snapshot(changed, now);
    let updated = projection::prepare(&state, &hub, 1_788_523_221);
    assert!(!Rc::ptr_eq(&initial, &updated));
    assert!(
        updated
            .worktrees
            .iter()
            .any(|row| row.branch == "changed branch")
    );
    state.filter.query = "changed".into();
    assert_eq!(
        projection::prepare(&state, &hub, 1_788_523_222)
            .worktrees
            .len(),
        1
    );
    state.screen = Screen::Hub { tab: HubTab::Prs };
    assert!(
        projection::prepare(&state, &hub, 1_788_523_222)
            .worktrees
            .is_empty()
    );
}

#[test]
fn safety_details_prioritize_both_inspection_error_channels() {
    let mut data = inspection(false, Some(0), true);
    data.error = Some("git inspection failed".into());
    let mut slot = Inspected::ready(data);
    assert_eq!(slot.failure(), Some("git inspection failed"));
    slot.error = Some("transport failed".into());
    assert_eq!(slot.failure(), Some("transport failed"));
}

#[test]
fn unset_context_projects_the_chrome_context() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-context", now);
    let mut source = snapshot(0);
    source.contexts = vec![context("acme"), context("other")];
    source.repos = vec![repo("acme/api", "acme"), repo("other/web", "other")];
    source.worktrees = vec![
        worktree("acme/api#one", "acme/api", "2026-09-04T10:00:00Z"),
        worktree("other/web#two", "other/web", "2026-09-04T10:00:00Z"),
    ];
    source.active_context = None;
    state.apply_snapshot(source, now);

    assert_eq!(
        effective_context(&state).map(|item| item.id.as_str()),
        Some("acme")
    );
    let model = projection::prepare(&state, &HubState::default(), 1_788_523_200);
    assert_eq!(model.worktrees.len(), 1);
    assert_eq!(model.worktrees[0].id.as_str(), "acme/api#one");
}

#[gpui::test]
fn filter_shrink_preserves_selected_worktree_identity(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-filter", now);
    let mut source = snapshot(0);
    source.worktrees = vec![
        worktree("acme/api#drop-a", "acme/api", "2026-09-04T10:00:00Z"),
        worktree("acme/api#keep-b", "acme/api", "2026-09-04T10:00:00Z"),
        worktree("acme/api#keep-c", "acme/api", "2026-09-04T10:00:00Z"),
    ];
    state.apply_snapshot(source, now);
    state.cursors.worktrees = 1;
    state.overlay = Some(Overlay::Filter);
    state.filter.query = "keep".to_owned();
    let selected: WorktreeId = "acme/api#keep-b".parse().expect("worktree id");
    let (ctx, _harness) = test_hub_ctx(state, cx);
    ctx.hub.update(cx, |hub, _| {
        hub.selection.worktree = Some(selected.clone());
    });

    cx.update(|cx| {
        let model = ctx.model(cx);
        assert_eq!(model.worktrees.len(), 2);
        ctx.reconcile_selection(&model, cx);
    });

    cx.read(|cx| {
        assert_eq!(ctx.state.read(cx).cursors.worktrees, 0);
        assert_eq!(
            ctx.hub.read(cx).selection.worktree.as_ref(),
            Some(&selected)
        );
    });
}

#[test]
fn pr_cache_never_crosses_scope_or_generation() {
    let now = Instant::now();
    let first = scoped_state(RepoScope::Repo("acme/api".parse().expect("repo")), 3);
    let first_key = cache::PrCacheKey::from_state(&first);
    let mut cache = PrCache::default();
    let requests = cache.begin_fetch(first_key.clone(), now);
    assert!(cache.apply(
        &first_key,
        PrTab::Mine,
        requests[0].1,
        pr_response(PrTab::Mine),
        now
    ));
    assert!(cache.slice_for(PrTab::Mine, &first_key).is_some());

    let other = scoped_state(RepoScope::Repo("acme/web".parse().expect("repo")), 3);
    let other_key = cache::PrCacheKey::from_state(&other);
    assert!(cache.slice_for(PrTab::Mine, &other_key).is_none());
    cache.begin_fetch(other_key.clone(), now);
    assert!(cache.slice(PrTab::Mine).is_none());

    let reconnected = scoped_state(RepoScope::Repo("acme/web".parse().expect("repo")), 4);
    let reconnected_key = cache::PrCacheKey::from_state(&reconnected);
    assert!(cache.slice_for(PrTab::Mine, &reconnected_key).is_none());
}

#[test]
fn superseded_pr_reply_is_ignored() {
    let now = Instant::now();
    let state = scoped_state(RepoScope::All, 1);
    let key = cache::PrCacheKey::from_state(&state);
    let mut cache = PrCache::default();
    let old = cache.begin_fetch(key.clone(), now);
    let current = cache.begin_fetch(key.clone(), now);
    assert!(!cache.apply(&key, PrTab::Mine, old[0].1, pr_response(PrTab::Mine), now));
    assert!(cache.loading(PrTab::Mine));
    assert!(cache.apply(
        &key,
        PrTab::Mine,
        current[0].1,
        pr_response(PrTab::Mine),
        now
    ));
}

#[test]
fn pr_tabs_complete_independently() {
    let now = Instant::now();
    let state = scoped_state(RepoScope::All, 1);
    let key = cache::PrCacheKey::from_state(&state);
    let mut cache = PrCache::default();
    let requests = cache.begin_fetch(key.clone(), now);
    assert!(cache.apply(
        &key,
        PrTab::Review,
        requests[1].1,
        pr_response(PrTab::Review),
        now
    ));
    assert!(!cache.loading(PrTab::Review));
    assert!(cache.loading(PrTab::Mine));
    assert!(cache.error(PrTab::Review).is_none());
    assert!(cache.apply(
        &key,
        PrTab::Mine,
        requests[0].1,
        Err(client_error("mine failed")),
        now
    ));
    assert_eq!(cache.error(PrTab::Mine), Some("mine failed"));
    assert!(cache.error(PrTab::Review).is_none());
}

#[test]
fn refresh_removes_closed_pr_badges() {
    let api: RepoId = "acme/api".parse().expect("repo");
    let web: RepoId = "acme/web".parse().expect("repo");
    let mut badges = HashMap::from([
        (
            (api.clone(), "closed".to_owned()),
            (1, PrBadgeState::Review),
        ),
        (
            (web.clone(), "live".to_owned()),
            (2, PrBadgeState::Approved),
        ),
    ]);
    cache::replace_pr_badges(
        &mut badges,
        &std::collections::HashSet::from([api.clone()]),
        [((api.clone(), "new".to_owned()), (3, PrBadgeState::Draft))],
    );
    assert!(!badges.contains_key(&(api.clone(), "closed".to_owned())));
    assert!(badges.contains_key(&(api, "new".to_owned())));
    assert!(badges.contains_key(&(web, "live".to_owned())));
}

#[gpui::test]
fn refresh_applies_badges_only_to_the_current_context(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let api: RepoId = "acme/api".parse().expect("repo");
    let web: RepoId = "other/web".parse().expect("repo");
    let mut source = snapshot(0);
    source.contexts = vec![context("acme"), context("other")];
    source.repos = vec![repo("acme/api", "acme"), repo("other/web", "other")];
    let mut state = AppState::new("/tmp/fleet-hub-pr-badges", now);
    state.apply_snapshot(source, now);
    state.daemon = crate::state::DaemonLink::Connected;
    state.screen = Screen::Hub { tab: HubTab::Prs };
    state.pr_badges = HashMap::from([
        (
            (api.clone(), "closed".to_owned()),
            (1, PrBadgeState::Review),
        ),
        (
            (web.clone(), "live".to_owned()),
            (2, PrBadgeState::Approved),
        ),
    ]);
    let (ctx, harness) = test_hub_ctx(state, cx);

    cx.update(|cx| ctx.fetch_pull_requests(false, cx));
    assert_eq!(harness.len(), 2);
    assert!(matches!(
        harness.respond_next(pr_response(PrTab::Mine)),
        RequestBody::ListPullRequests {
            tab: PrTab::Mine,
            ..
        }
    ));
    cx.run_until_parked();
    assert!(matches!(
        harness.respond_next(pr_response(PrTab::Review)),
        RequestBody::ListPullRequests {
            tab: PrTab::Review,
            ..
        }
    ));
    cx.run_until_parked();

    cx.read(|cx| {
        let badges = &ctx.state.read(cx).pr_badges;
        assert!(!badges.contains_key(&(api, "closed".to_owned())));
        assert!(badges.contains_key(&(web, "live".to_owned())));
    });
}

#[test]
fn empty_inspection_reply_finishes_current_request() {
    let id: WorktreeId = "buk/payroll#a".parse().expect("worktree");
    let mut hub = HubState::default();
    let superseded = hub.begin_inspection(id.clone());
    let request = hub.begin_inspection(id.clone());
    assert!(!hub.apply_inspection(&id, superseded, Ok(ResponseBody::Inspections(Vec::new()))));
    assert!(hub.inspections.get(&id).is_some_and(|slot| slot.loading));
    assert!(hub.apply_inspection(&id, request, Ok(ResponseBody::Inspections(Vec::new()))));
    let slot = hub.inspections.get(&id).expect("inspection slot");
    assert!(!slot.loading);
    assert!(slot.error.is_none());
}

#[test]
fn failed_refresh_preserves_inspection_data() {
    let id: WorktreeId = "buk/payroll#a".parse().expect("worktree");
    let mut hub = HubState::default();
    hub.inspections.insert(
        id.clone(),
        Inspected::ready(inspection(true, Some(1), false)),
    );
    let request = hub.begin_inspection(id.clone());
    assert!(hub.apply_inspection(&id, request, Err(client_error("offline"))));
    let slot = hub.inspections.get(&id).expect("inspection slot");
    assert!(slot.data.is_some());
    assert_eq!(slot.error.as_deref(), Some("offline"));
    assert!(!slot.loading);
}

#[test]
fn deleted_or_recreated_worktree_invalidates_inspection() {
    let old = worktree("buk/payroll#a", "buk/payroll", "2026-09-04T10:00:00Z");
    let id = old.id.clone();
    let mut hub = HubState::default();
    hub.reconcile_inspection_identities(std::slice::from_ref(&old));
    hub.inspections.insert(
        id.clone(),
        Inspected::ready(inspection(false, Some(0), true)),
    );
    let recreated = worktree("buk/payroll#a", "buk/payroll", "2026-09-05T10:00:00Z");
    hub.reconcile_inspection_identities(std::slice::from_ref(&recreated));
    assert!(!hub.inspections.contains_key(&id));

    hub.inspections.insert(
        id.clone(),
        Inspected::ready(inspection(false, Some(0), true)),
    );
    hub.reconcile_inspection_identities(&[]);
    assert!(!hub.inspections.contains_key(&id));
}

#[gpui::test]
fn synchronization_reconciles_identities_once_per_snapshot_revision(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-reconciliation", now);
    state.apply_snapshot(snapshot(10_000), now);
    let (ctx, _harness) = test_hub_ctx(state, cx);

    cx.update(|cx| ctx.synchronize(cx));
    assert_eq!(
        ctx.hub
            .read_with(cx, |hub, _| hub.inspection_reconciliations),
        1
    );

    for cursor in 0..20 {
        ctx.state.update(cx, |state, _| {
            state.cursors.worktrees = cursor;
        });
        cx.update(|cx| ctx.synchronize(cx));
    }
    assert_eq!(
        ctx.hub
            .read_with(cx, |hub, _| hub.inspection_reconciliations),
        1
    );

    ctx.state.update(cx, |state, _| {
        let mut changed = snapshot(10_000);
        changed.worktrees[0].created_at = "2026-09-05T10:00:00Z".to_owned();
        state.apply_snapshot(changed, now);
    });
    cx.update(|cx| ctx.synchronize(cx));
    assert_eq!(
        ctx.hub
            .read_with(cx, |hub, _| hub.inspection_reconciliations),
        2
    );
}

#[test]
fn copy_path_always_resolves_visible_state() {
    assert_eq!(
        actions::copy_path_outcome(Ok(ResponseBody::Path("/tmp/wt".to_owned()))).expect("path"),
        "/tmp/wt"
    );
    assert!(actions::copy_path_outcome(Ok(ResponseBody::Ack)).is_err());
    assert!(actions::copy_path_outcome(Err(client_error("offline"))).is_err());
}

#[gpui::test]
fn copy_path_reports_a_closed_reply_channel(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-copy-path", now);
    let mut source = snapshot(0);
    source.worktrees = vec![worktree(
        "acme/api#selected",
        "acme/api",
        "2026-09-04T10:00:00Z",
    )];
    state.apply_snapshot(source, now);
    state.daemon = crate::state::DaemonLink::Connected;
    let (ctx, harness) = test_hub_ctx(state, cx);

    cx.update(|cx| ctx.copy_path(cx));
    assert!(matches!(
        harness.close_next(),
        RequestBody::WorktreePath { .. }
    ));
    cx.run_until_parked();

    cx.read(|cx| {
        assert_eq!(
            ctx.state
                .read(cx)
                .sticky_error
                .as_ref()
                .map(|error| error.text.as_str()),
            Some("the Fleet daemon reply channel closed")
        );
    });
}

#[test]
fn pr_create_is_single_flight_and_intent_gated() {
    let key = ("acme/api".parse().expect("repo"), 42);
    let other = ("acme/api".parse().expect("repo"), 43);
    let mut hub = HubState::default();
    assert!(hub.begin_pr_creation(key.clone(), false, false));
    assert!(!hub.begin_pr_creation(key.clone(), true, true));
    assert_eq!(hub.creating, vec![key.clone()]);
    let intent = hub.finish_pr_creation(&key).expect("creation intent");
    assert!(intent.open);
    assert!(intent.sleep_previous);
    assert!(navigation::pr_navigation_matches(
        &Screen::Hub { tab: HubTab::Prs },
        Some(&key),
        &key
    ));
    assert!(!navigation::pr_navigation_matches(
        &Screen::Hub { tab: HubTab::Prs },
        Some(&other),
        &key
    ));
    assert!(!navigation::pr_navigation_matches(
        &Screen::Hub {
            tab: HubTab::Worktrees
        },
        Some(&key),
        &key
    ));
}

#[gpui::test]
fn filter_activation_uses_hub_selection_and_pr_creation_state(cx: &mut gpui::TestAppContext) {
    let mut state = AppState::new("/tmp/fleet-filter-hub-state", Instant::now());
    state.daemon = crate::state::DaemonLink::Connected;
    state.screen = Screen::Hub { tab: HubTab::Prs };
    let (ctx, harness) = test_hub_ctx(state, cx);
    ctx.hub.update(cx, |hub, _| {
        hub.selection.worktree =
            Some(WorktreeId::try_from("acme/api#stale").expect("selection anchor"));
    });

    cx.update(|cx| {
        ctx.activate_filter_target(
            DisplayedTarget::Repo(RepoId::try_from("acme/api").expect("repo")),
            cx,
        );
    });
    assert!(
        ctx.hub
            .read_with(cx, |hub, _| hub.selection.worktree.is_none())
    );

    let key = (RepoId::try_from("acme/api").expect("repo"), 42);
    cx.update(|cx| {
        ctx.activate_filter_target(
            DisplayedTarget::PullRequest(crate::presentation::DisplayedPr {
                repo: key.0.clone(),
                number: key.1,
                local: None,
            }),
            cx,
        );
    });
    assert_eq!(ctx.hub.read_with(cx, |hub, _| hub.creating.clone()), [key]);
    assert!(matches!(
        harness.close_next(),
        RequestBody::CreateWorktreeFromPr { number: 42, .. }
    ));
}

#[gpui::test]
fn pr_creation_reply_does_not_navigate_after_the_cursor_moves(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-pr-navigation", now);
    state.apply_snapshot(snapshot(0), now);
    state.daemon = crate::state::DaemonLink::Connected;
    state.screen = Screen::Hub { tab: HubTab::Prs };
    let key = cache::PrCacheKey::from_state(&state);
    let (ctx, harness) = test_hub_ctx(state, cx);
    ctx.hub.update(cx, |hub, _| {
        let requests = hub.prs.begin_fetch(key.clone(), now);
        assert!(hub.prs.apply(
            &key,
            PrTab::Mine,
            requests[0].1,
            pr_response_with(
                PrTab::Mine,
                vec![
                    pull_request("acme/api", 42, "2026-09-04T12:00:00Z"),
                    pull_request("acme/api", 43, "2026-09-04T11:00:00Z"),
                ],
            ),
            now,
        ));
        hub.invalidate();
    });

    cx.update(|cx| ctx.open_pr(true, false, cx));
    assert_eq!(harness.len(), 1);
    cx.update(|cx| ctx.set_cursor(1, 2, true, cx));
    assert!(matches!(
        harness.respond_next(Ok(ResponseBody::Worktree {
            created: true,
            worktree: worktree("acme/api#feature-42", "acme/api", "2026-09-04T12:00:00Z",),
            post_create_job: None,
        })),
        RequestBody::CreateWorktreeFromPr { number: 42, .. }
    ));
    cx.run_until_parked();

    assert_eq!(harness.len(), 0, "no EnsureSession request is issued");
    cx.read(|cx| {
        assert_eq!(ctx.state.read(cx).screen, Screen::Hub { tab: HubTab::Prs });
    });
}

#[test]
fn restore_acknowledgement_requires_ack() {
    let mut token = Some("trash-entry".to_owned());
    if actions::restore_acknowledged(Err(client_error("restore failed"))).is_ok() {
        token = None;
    }
    assert_eq!(token.as_deref(), Some("trash-entry"));
    if actions::restore_acknowledged(Ok(ResponseBody::Ack)).is_ok() {
        token = None;
    }
    assert!(token.is_none());
}

#[gpui::test]
fn failed_restore_retains_undo_token(cx: &mut gpui::TestAppContext) {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-restore", now);
    state.daemon = crate::state::DaemonLink::Connected;
    state.last_trash_entry = Some("trash-entry".to_owned());
    let (ctx, harness) = test_hub_ctx(state, cx);

    cx.update(|cx| ctx.undo_delete(cx));
    assert_eq!(harness.len(), 1);
    cx.update(|cx| ctx.undo_delete(cx));
    assert_eq!(harness.len(), 1, "restore is single-flight");
    assert_eq!(
        ctx.hub
            .read_with(cx, |hub, _| hub.restoring_trash.clone())
            .as_deref(),
        Some("trash-entry")
    );
    assert!(matches!(
        harness.respond_next(Err(client_error("restore failed"))),
        RequestBody::RestoreTrash { .. }
    ));
    cx.run_until_parked();

    cx.read(|cx| {
        let state = ctx.state.read(cx);
        assert_eq!(state.last_trash_entry.as_deref(), Some("trash-entry"));
        assert_eq!(
            state.sticky_error.as_ref().map(|error| error.text.as_str()),
            Some("restore failed")
        );
        assert!(ctx.hub.read(cx).restoring_trash.is_none());
    });

    cx.update(|cx| ctx.undo_delete(cx));
    assert!(matches!(
        harness.respond_next(Ok(ResponseBody::Ack)),
        RequestBody::RestoreTrash { .. }
    ));
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(ctx.state.read(cx).last_trash_entry.is_none());
        assert!(ctx.hub.read(cx).restoring_trash.is_none());
    });
}
