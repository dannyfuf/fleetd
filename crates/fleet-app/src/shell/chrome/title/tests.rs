use super::*;
use crate::shell::chrome::replace_if_changed;
use fleet_core::model::{Context, Repo, RepoHooks, Worktree};
use std::time::Instant;

fn context(id: &str) -> Context {
    Context {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        name: id.to_owned(),
        owners: vec![id.to_owned()],
        created_at: "2026-09-04T09:00:00Z".to_owned(),
    }
}

fn repo(id: &str, context: &Context) -> Repo {
    let (owner, name) = id.split_once('/').unwrap_or_else(|| panic!("{id}"));
    Repo {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        owner: owner.to_owned(),
        name: name.to_owned(),
        url: String::new(),
        context_id: context.id.clone(),
        default_branch: "main".to_owned(),
        path: format!("/tmp/{name}"),
        cloned_at: String::new(),
        hooks: RepoHooks::default(),
    }
}

fn worktree(repo: &Repo, slug: &str) -> Worktree {
    let id = format!("{}#{slug}", repo.id);
    Worktree {
        id: id.parse().unwrap_or_else(|error| panic!("{error}")),
        repo_id: repo.id.clone(),
        slug: slug.to_owned(),
        branch: slug.to_owned(),
        base_ref: "main".to_owned(),
        path: format!("/tmp/{slug}"),
        session: id,
        host: None,
        created_at: String::new(),
        last_opened_at: None,
        degraded: None,
    }
}

/// Two contexts: `acme` with three worktrees over two repos, `oss` with one.
fn two_context_state() -> AppState {
    let acme = context("acme");
    let oss = context("oss");
    let api = repo("acme/api", &acme);
    let web = repo("acme/web", &acme);
    let tool = repo("dannyfuf/tool", &oss);
    let snapshot = fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-04T12:00:00Z".to_owned(),
        revision: None,
        contexts: vec![acme.clone(), oss],
        worktrees: vec![
            worktree(&api, "one"),
            worktree(&api, "two"),
            worktree(&web, "three"),
            worktree(&tool, "four"),
        ],
        repos: vec![api, web, tool],
        clones: Vec::new(),
        active_context: Some(acme.id),
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "0.1.0".to_owned(),
            pid: 1,
            started_at: "2026-09-04T09:00:00Z".to_owned(),
            home: "/tmp/fleet".to_owned(),
        },
    };
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_snapshot(snapshot, now);
    state
}

#[test]
fn the_hub_title_names_the_active_context_and_counts_its_worktrees() {
    let mut state = two_context_state();
    state.review_pr_count = 2;
    let model = TitleModel::build(&state);
    let Place::Hub(place) = &model.place else {
        panic!("the Hub draws the switcher and the nav: {:?}", model.place);
    };
    let names: Vec<&str> = place
        .contexts
        .iter()
        .map(|entry| entry.name.as_ref())
        .collect();
    assert_eq!(
        names,
        ["acme", "oss"],
        "every context is in the switcher, in order"
    );
    assert_eq!(place.active, Some(0));
    assert_eq!(
        place.worktrees, 3,
        "Worktrees counts the active context's worktrees, not every worktree"
    );
    assert_eq!(
        place.reviews, 2,
        "Pull requests counts the PRs waiting for your review"
    );
    assert_eq!(place.section, HubTab::Worktrees);
}

#[test]
fn an_empty_first_run_draws_an_empty_title_bar() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let mut snapshot = two_context_state()
        .snapshot
        .unwrap_or_else(|| panic!("the fixture has a snapshot"));
    snapshot.contexts.clear();
    snapshot.repos.clear();
    snapshot.worktrees.clear();
    snapshot.active_context = None;
    state.apply_snapshot(snapshot, now);
    assert!(state.is_first_run());
    assert_eq!(TitleModel::build(&state).place, Place::FirstRun);
}

#[test]
fn the_model_is_unchanged_by_state_the_bar_does_not_show() {
    let mut state = two_context_state();
    let mut model = TitleModel::build(&state);
    state.breadcrumb_row = Some("elsewhere".to_owned());
    assert!(
        !replace_if_changed(&mut model, TitleModel::build(&state)),
        "a notify that changes nothing the bar draws must not repaint it"
    );
    state.review_pr_count = 5;
    assert!(replace_if_changed(&mut model, TitleModel::build(&state)));
}

#[test]
fn the_daemon_pill_is_amber_while_retrying_and_red_once_down() {
    use crate::state::{DaemonLink, DaemonLossReason};

    assert_eq!(daemon_pill(&DaemonLink::Connected), None);
    assert_eq!(
        daemon_pill(&DaemonLink::Starting),
        Some(DaemonPill::Reconnecting)
    );
    assert_eq!(
        daemon_pill(&DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
            reason: DaemonLossReason::ConnectionLost,
        }),
        Some(DaemonPill::Reconnecting)
    );
    assert_eq!(
        daemon_pill(&DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
            reason: DaemonLossReason::Stopped,
        }),
        Some(DaemonPill::Down)
    );
}

#[test]
fn only_the_hub_digits_select_a_context_by_key() {
    assert!(select_context(0).is_none());
    assert_eq!(
        select_context(1).map(|action| action.name()),
        Some("hub::SelectContext1")
    );
    assert_eq!(
        select_context(9).map(|action| action.name()),
        Some("hub::SelectContext9")
    );
    assert!(
        select_context(10).is_none(),
        "a context past nine has no key and switches by request"
    );
}
