use std::{cell::RefCell, collections::VecDeque};

use super::*;
use crate::dialogs::create_worktree::view::unavailable_hosts;

enum RecordedRequest {
    Sent(RequestBody),
    Requested(RequestBody),
}

type TestReplySender = async_channel::Sender<Result<ResponseBody, fleet_proto::error::ProtoError>>;

#[derive(Clone, Default)]
struct FakeTransport {
    requests: Rc<RefCell<Vec<RecordedRequest>>>,
    replies: Rc<RefCell<VecDeque<TestReplySender>>>,
}

impl CreateTransport for FakeTransport {
    fn send(&self, body: RequestBody) {
        self.requests.borrow_mut().push(RecordedRequest::Sent(body));
    }

    fn request(&self, body: RequestBody) -> Reply {
        let (sender, receiver) = async_channel::bounded(1);
        self.requests
            .borrow_mut()
            .push(RecordedRequest::Requested(body));
        self.replies.borrow_mut().push_back(sender);
        receiver
    }
}

fn worktree() -> fleet_core::model::Worktree {
    fleet_core::model::Worktree {
        id: WorktreeId::try_from("buk/payroll#feature").unwrap(),
        repo_id: RepoId::try_from("buk/payroll").unwrap(),
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: "/tmp/feature".to_owned(),
        session: "buk/payroll#feature".to_owned(),
        host: None,
        created_at: String::new(),
        last_opened_at: None,
        degraded: None,
    }
}

fn snapshot_with_worktree() -> fleet_proto::snapshot::Snapshot {
    fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        revision: None,
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: vec![worktree()],
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: String::new(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    }
}

fn draft() -> CreateState {
    CreateState {
        repo: RepoId::try_from("buk/payroll").ok(),
        default_base: "origin/main".to_owned(),
        previous_base: Some("pull/412/head".to_owned()),
        base_refs: vec![
            "origin/main".to_owned(),
            "origin/release-2026".to_owned(),
            "origin/feat/payroll-import".to_owned(),
        ],
        ..CreateState::default()
    }
}

#[test]
fn large_base_ref_lists_keep_default_previous_and_matching_cap() {
    let mut draft = draft();
    draft.base_refs = (0..10_000).map(|i| format!("origin/feature-{i}")).collect();
    draft.branch = "feature-99".to_owned();
    let rows = draft.base_candidates();
    assert_eq!(rows.len(), BASE_LIMIT);
    assert_eq!(&rows[..2], &["origin/main", "pull/412/head"]);
    assert_eq!(rows[2], "origin/feature-99");
    draft.branch = "no-match-at-all".to_owned();
    assert_eq!(draft.base_candidates()[2], "origin/feature-0");
}

#[test]
fn default_base_is_first_and_previous_base_second() {
    let rows = draft().base_candidates();
    assert_eq!(rows[0], "origin/main");
    assert_eq!(rows[1], "pull/412/head");
    assert!(rows.contains(&"origin/release-2026".to_owned()));
    assert!(rows.len() <= BASE_LIMIT);
}

#[test]
fn base_rows_are_filtered_by_the_typed_branch_but_never_emptied() {
    let mut state = draft();
    state.branch = "import".to_owned();
    let rows = state.base_candidates();
    assert_eq!(rows[2], "origin/feat/payroll-import");
    state.branch = "zzzz".to_owned();
    let rows = state.base_candidates();
    assert!(
        rows.len() > 2,
        "an unmatchable branch must not hide every base"
    );
}

#[test]
fn the_preview_is_the_worktree_id_the_create_will_produce() {
    let mut state = draft();
    state.branch = "feat/RUT validator".to_owned();
    assert_eq!(
        state.preview_id().as_deref(),
        Some("buk/payroll#feat-rut-validator")
    );
}

#[test]
fn validation_states_the_failing_rule_and_blocks_enter() {
    let mut state = draft();
    state.branch = "feat/..bad".to_owned();
    assert_eq!(
        state.branch_error().as_deref(),
        Some("branch must not contain `..`")
    );
    assert!(!state.can_submit());
    state.branch = "feat/ok".to_owned();
    assert_eq!(state.branch_error(), None);
    assert!(state.can_submit());
}

#[test]
fn an_empty_branch_is_neither_invalid_nor_submittable() {
    let state = draft();
    assert_eq!(state.branch_error(), None);
    assert!(!state.can_submit());
}

fn host_status(id: &str, provider: &str, link: LinkState, reachable: bool) -> HostStatus {
    HostStatus {
        id: HostId::try_from(id).unwrap_or_else(|error| panic!("{error}")),
        provider: provider.to_owned(),
        version: None,
        link,
        address: None,
        agent_binaries: None,
        reachable,
        checked_at: "2026-09-04T12:00:00Z".to_owned(),
        error: (!reachable).then(|| "ssh: connect timed out after 5s".to_owned()),
    }
}

fn hosts(statuses: &[HostStatus]) -> Vec<HostChoice> {
    std::iter::once(HostChoice::local())
        .chain(statuses.iter().map(HostChoice::from_status))
        .collect()
}

#[test]
fn the_host_cycler_maps_local_to_no_host() {
    let mut state = draft();
    state.hosts = hosts(&[host_status("devbox", "tailscale", LinkState::Ready, true)]);
    assert_eq!(state.selected_host(), None, "`local` is not a host id");
    state.host_index = 1;
    assert_eq!(
        state.selected_host().map(|host| host.as_str().to_owned()),
        Some("devbox".to_owned())
    );
}

#[test]
fn an_unreachable_host_is_disabled_with_the_daemons_reason() {
    let mut state = draft();
    state.branch = "feat/ok".to_owned();
    state.hosts = hosts(&[
        host_status("devbox", "tailscale", LinkState::Down, false),
        host_status("archdev", "legacy", LinkState::Legacy, true),
        host_status("loopback", "command", LinkState::Ready, true),
    ]);
    assert!(state.can_submit(), "`local` is always submittable");

    state.host_index = 1;
    assert_eq!(
        state.host_blocked(),
        Some("ssh: connect timed out after 5s"),
        "the refusal is the daemon's own probe error, not a local guess"
    );
    assert!(!state.can_submit());

    state.host_index = 2;
    assert_eq!(
        state.host_blocked(),
        Some("legacy entry \u{2014} migrate it to a tailscale host")
    );
    assert!(!state.can_submit());

    state.host_index = 3;
    assert_eq!(state.host_blocked(), None);
    assert_eq!(
        state
            .selected_choice()
            .and_then(|choice| choice.provider.as_deref()),
        Some("command")
    );
    assert!(state.can_submit());
}

#[test]
fn a_blocked_default_host_never_becomes_the_seeded_selection() {
    let devbox = HostId::try_from("devbox").unwrap();
    let mut state = draft();
    state.hosts = hosts(&[host_status("devbox", "tailscale", LinkState::Down, false)]);
    assert!(!state.select_default_host(Some(&devbox)));
    assert_eq!(state.host_index, 0, "a refused host is not preselected");

    let mut state = draft();
    state.hosts = hosts(&[host_status("devbox", "tailscale", LinkState::Ready, true)]);
    assert!(state.select_default_host(Some(&devbox)));
    assert_eq!(state.host_index, 1);

    let mut state = draft();
    state.hosts = hosts(&[host_status("devbox", "tailscale", LinkState::Ready, true)]);
    state.host_touched = true;
    assert!(
        !state.select_default_host(Some(&devbox)),
        "a late config answer never moves a cycler the user already moved"
    );
}

#[test]
fn subsequence_matches_in_order_only() {
    assert!(FuzzyQuery::new("import").matches("origin/feat/payroll-import"));
    assert!(FuzzyQuery::new("").matches("origin/main"));
    assert!(!FuzzyQuery::new("z").matches("origin/main"));
    assert!(!FuzzyQuery::new("cb").matches("abc"));
}

#[gpui::test]
fn opening_initiates_base_ref_fetch(cx: &mut gpui::TestAppContext) {
    let repo = RepoId::try_from("buk/payroll").unwrap();
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.scope = RepoScope::Repo(repo.clone());
        state
    });
    let transport = FakeTransport::default();
    cx.update(|cx| seed_with_transport(&state, &transport, cx));
    cx.run_until_parked();
    {
        let requests = transport.requests.borrow();
        assert!(matches!(
            requests.as_slice(),
            [RecordedRequest::Requested(RequestBody::ListBaseRefs {
                force: false,
                ..
            })]
        ));
    }

    transport
        .replies
        .borrow_mut()
        .pop_front()
        .unwrap()
        .try_send(Ok(ResponseBody::BaseRefs(
            fleet_proto::response::BaseRefs {
                refs: vec!["origin/main".to_owned()],
                fetching: false,
                fetched_at: String::new(),
            },
        )))
        .unwrap();
    cx.run_until_parked();
    {
        let requests = transport.requests.borrow();
        assert!(matches!(
            requests.as_slice(),
            [
                RecordedRequest::Requested(RequestBody::ListBaseRefs { force: false, .. }),
                RecordedRequest::Requested(RequestBody::ListBaseRefs { force: true, .. })
            ]
        ));
    }
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.create.base_refs, ["origin/main"]);
            assert!(host.create.fetching);
        })
    });
    transport
        .replies
        .borrow_mut()
        .pop_front()
        .unwrap()
        .try_send(Ok(ResponseBody::BaseRefs(
            fleet_proto::response::BaseRefs {
                refs: vec!["origin/main".to_owned(), "origin/release".to_owned()],
                fetching: false,
                fetched_at: String::new(),
            },
        )))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.create.base_refs, ["origin/main", "origin/release"]);
            assert!(!host.create.fetching);
        })
    });
}

#[gpui::test]
fn base_ref_failure_stops_spinner_and_retries(cx: &mut gpui::TestAppContext) {
    let repo = RepoId::try_from("buk/payroll").unwrap();
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.scope = RepoScope::Repo(repo.clone());
        state
    });
    let transport = FakeTransport::default();
    cx.update(|cx| seed_with_transport(&state, &transport, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(BASE_REF_TIMEOUT);
    cx.run_until_parked();
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert!(!host.create.fetching);
            assert_eq!(
                host.create.base_error.as_deref(),
                Some("base-ref cache timed out")
            );
            assert!(host.create.should_retry_base_refs());
        });
        submit_with_transport(false, &state, &transport, cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert!(host.create.fetching);
            assert_eq!(host.create.base_error, None);
        })
    });
    assert_eq!(transport.requests.borrow().len(), 2);
}

/// Typing into the branch editor mirrors the text, retires the refused create and returns the
/// base cursor to the top row — §3.8.1's "the list follows what you type".
#[gpui::test]
fn typing_mirrors_the_branch_and_clears_the_refusal(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.snapshot = Some(snapshot_with_worktree());
        state
    });
    let transport = FakeTransport::default();
    cx.update(|cx| {
        seed_with_transport(&state, &transport, cx);
        with_host(&state, cx, |host| {
            host.create.error = Some("already exists".to_owned());
            host.create.base_cursor = 3;
        });
    });
    let input = cx.update(|cx| {
        read_host(&state, cx, |host, _| host.create_branch.clone())
            .unwrap_or_else(|| panic!("the dialog seeds its branch editor"))
    });
    cx.update(|cx| input.update(cx, |input, cx| input.set_text("feature", cx)));
    cx.run_until_parked();
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.create.branch, "feature");
            assert_eq!(host.create.error, None);
            assert_eq!(host.create.base_cursor, 0);
        });
    });
}

struct CreateWithoutOpeningView {
    state: Entity<AppState>,
    transport: FakeTransport,
    focus: FocusHandle,
}

impl gpui::Render for CreateWithoutOpeningView {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let state = self.state.clone();
        let transport = self.transport.clone();
        root(&self.focus).on_action(
            move |_: &create_actions::CreateWithoutOpening, _window, cx| {
                submit_with_transport(false, &state, &transport, cx);
            },
        )
    }
}

#[gpui::test]
fn duplicate_without_opening_stays_in_hub(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.snapshot = Some(snapshot_with_worktree());
        state.overlay = Some(crate::state::Overlay::Dialog(
            crate::dialogs::Dialogs::CreateWorktree,
        ));
        state
    });
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.create = draft();
            host.create.branch = "feature".to_owned();
        })
    });
    let transport = FakeTransport::default();
    let window = cx.add_window(|_, cx| CreateWithoutOpeningView {
        state: state.clone(),
        transport: transport.clone(),
        focus: cx.focus_handle(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |view, window, cx| {
            window.focus(&view.focus, cx);
            window.dispatch_action(Box::new(create_actions::CreateWithoutOpening), cx);
        })
        .unwrap();
    visual.update(|_, cx| {
        let app = state.read(cx);
        assert!(app.overlay.is_none());
        assert!(matches!(app.screen, Screen::Hub { .. }));
    });
    assert!(transport.requests.borrow().is_empty());
}

#[gpui::test]
fn opening_touches_worktree_before_ensuring_session(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.snapshot = Some(snapshot_with_worktree());
        state.overlay = Some(crate::state::Overlay::Dialog(
            crate::dialogs::Dialogs::CreateWorktree,
        ));
        state
    });
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.create = draft();
            host.create.branch = "feature".to_owned();
        })
    });
    let transport = FakeTransport::default();
    cx.update(|cx| submit_with_transport(true, &state, &transport, cx));
    let requests = transport.requests.borrow();
    assert!(matches!(
        requests.as_slice(),
        [
            RecordedRequest::Sent(RequestBody::TouchWorktreeOpened { id }),
            RecordedRequest::Requested(RequestBody::EnsureSession {
                worktree: Some(ensured),
                ..
            })
        ] if id == ensured
    ));
}

fn tailscale_entry(node: &str) -> fleet_core::model::HostConfigEntry {
    fleet_core::model::HostConfigEntry::Tailscale {
        node: node.to_owned(),
        user: Some("df".to_owned()),
        ssh_options: Vec::new(),
        identity_file: None,
        ssh_host: None,
        fleetd: "fleetd".to_owned(),
        fleet_home: Some("~/.fleet".to_owned()),
    }
}

fn config_with_default(default: &str) -> fleet_core::config::Config {
    let mut config = fleet_core::config::default_config("/tmp/fleet");
    config.hosts.insert(
        HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}")),
        tailscale_entry("devbox"),
    );
    config.default_host = default.to_owned();
    config
}

fn seeded_with_hosts(
    statuses: Vec<HostStatus>,
    cx: &mut gpui::TestAppContext,
) -> (Entity<AppState>, FakeTransport) {
    let repo = RepoId::try_from("buk/payroll").unwrap();
    let mut snapshot = snapshot_with_worktree();
    snapshot.hosts = statuses;
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.scope = RepoScope::Repo(repo);
        state.snapshot = Some(snapshot);
        state
    });
    let transport = FakeTransport::default();
    cx.update(|cx| seed_with_transport(&state, &transport, cx));
    (state, transport)
}

fn answer_get_config(transport: &FakeTransport, config: fleet_core::config::Config) {
    let index = transport
        .requests
        .borrow()
        .iter()
        .position(|request| matches!(request, RecordedRequest::Requested(RequestBody::GetConfig)))
        .unwrap_or_else(|| panic!("the dialog never asked for the configuration"));
    transport
        .replies
        .borrow_mut()
        .remove(index)
        .unwrap_or_else(|| panic!("no reply slot for the configuration request"))
        .try_send(Ok(ResponseBody::Config(config)))
        .unwrap_or_else(|error| panic!("{error}"));
}

#[gpui::test]
fn the_picker_seeds_the_configured_default_host(cx: &mut gpui::TestAppContext) {
    let (state, transport) = seeded_with_hosts(
        vec![
            host_status("devbox", "tailscale", LinkState::Ready, true),
            host_status("archdev", "legacy", LinkState::Legacy, true),
        ],
        cx,
    );
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            let labels: Vec<&str> = host
                .create
                .hosts
                .iter()
                .map(|choice| choice.label.as_str())
                .collect();
            assert_eq!(labels, ["local", "devbox", "archdev"]);
            assert_eq!(host.create.host_index, 0, "local until config answers");
        });
    });
    answer_get_config(&transport, config_with_default("devbox"));
    cx.run_until_parked();
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.create.host_index, 1);
            assert_eq!(
                host.create.selected_host().map(|id| id.as_str().to_owned()),
                Some("devbox".to_owned())
            );
        });
    });
}

#[gpui::test]
fn an_unreachable_default_host_leaves_the_picker_on_local(cx: &mut gpui::TestAppContext) {
    let (state, transport) = seeded_with_hosts(
        vec![host_status("devbox", "tailscale", LinkState::Down, false)],
        cx,
    );
    answer_get_config(&transport, config_with_default("devbox"));
    cx.run_until_parked();
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.create.host_index, 0);
            assert_eq!(host.create.selected_host(), None);
        });
    });
}

#[test]
fn a_blocked_host_states_its_reason_and_keeps_the_cycler_live() {
    let mut draft = draft();
    draft.branch = "feat/ok".to_owned();
    draft.hosts = hosts(&[
        host_status("devbox", "tailscale", LinkState::Down, false),
        host_status("archdev", "tailscale", LinkState::Ready, true),
    ]);

    draft.host_index = 1;
    assert!(draft.host_blocked().is_some());
    assert!(!draft.can_submit(), "`Enter` is refused on a blocked host");
    assert_eq!(
        unavailable_hosts(&draft),
        ["devbox unavailable \u{2014} ssh: connect timed out after 5s"],
        "the blocked host still states why"
    );

    draft.host_index = 0;
    assert!(draft.can_submit());
    assert_eq!(
        unavailable_hosts(&draft).len(),
        1,
        "the reason stays while local is chosen"
    );

    draft.hosts.clear();
    assert!(
        draft.selected_choice().is_none(),
        "the whole row is zero-suppressed when no host is configured"
    );
}

#[gpui::test]
fn a_click_chooses_a_reachable_host_and_ignores_a_blocked_one(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        let state = cx.new(|_| AppState::new("/tmp/fleet", Instant::now()));
        with_host(&state, cx, |host| {
            host.create = draft();
            host.create.hosts = hosts(&[
                host_status("devbox", "tailscale", LinkState::Ready, true),
                host_status("archdev", "tailscale", LinkState::Down, false),
            ]);
        });
        select_host(&state, 2, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.create.host_index, 0, "a blocked host is not clickable");
            assert!(!host.create.host_touched);
        });
        select_host(&state, 1, cx);
        with_host(&state, cx, |host| {
            assert_eq!(host.create.host_index, 1);
            assert!(
                host.create.host_touched,
                "a click freezes the default-host seeding"
            );
        });
    });
}

#[gpui::test]
fn the_open_after_box_decides_what_enter_does(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let state = cx.new(|_| AppState::new("/tmp/fleet", Instant::now()));
        with_host(&state, cx, |host| host.create = draft());
        with_host(&state, cx, |host| {
            assert!(!host.create.stay_in_hub, "the box starts checked");
        });
        set_open_after(&state, false, cx);
        with_host(&state, cx, |host| assert!(host.create.stay_in_hub));
        set_open_after(&state, true, cx);
        with_host(&state, cx, |host| assert!(!host.create.stay_in_hub));
    });
}

/// Two displayed worktree rows, `first` above `second`, as the Hub list draws them.
fn displayed_rows(ids: &[&str]) -> Vec<crate::presentation::DisplayedWorktree> {
    ids.iter()
        .map(|id| crate::presentation::DisplayedWorktree {
            id: WorktreeId::try_from(*id).unwrap_or_else(|error| panic!("{error}")),
            repo: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
        })
        .collect()
}

#[test]
fn late_creation_does_not_override_navigation() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.displayed_hub.worktrees = displayed_rows(&["buk/payroll#feature", "buk/payroll#other"]);
    let intent = NavigationIntent::capture(&state);
    state.cursors.worktrees = 1;
    assert!(!intent.matches(&state));
}

#[test]
fn the_new_row_shifting_the_cursor_is_not_navigation() {
    let mut state = AppState::new("/tmp/fleet", Instant::now());
    state.displayed_hub.worktrees = displayed_rows(&["buk/payroll#feature"]);
    let intent = NavigationIntent::capture(&state);
    // The snapshot that lists the created worktree inserts it above and keeps `feature` selected.
    state.displayed_hub.worktrees = displayed_rows(&["buk/payroll#created", "buk/payroll#feature"]);
    state.cursors.worktrees = 1;
    assert!(
        intent.matches(&state),
        "the same row is selected, so Create must still open the worktree"
    );
}
