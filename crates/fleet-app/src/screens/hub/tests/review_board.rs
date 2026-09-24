//! The Review tab draws the context's Reviews board on a daemon with `board.reviews`, and keeps
//! the old flat `gh` list on one without it (UX-SPEC §3.5).

use fleet_core::board::{
    BoardView, CardDraft, StatusCategory, create_card, new_reviews_board, reviews_board_id,
};
use fleet_proto::response::BOARD_REVIEWS_CAPABILITY;
use gpui::Keystroke;

use super::*;
use crate::{
    bridge::RecordedRequests,
    state::{BoardScope, DaemonLink},
};

const NOW: &str = "2026-09-24T12:00:00Z";

/// A connected Hub on the Pull requests screen's Review tab, in context `zed`.
fn review_state(reviews: bool) -> AppState {
    let now = Instant::now();
    let mut source = snapshot(0);
    source.contexts = vec![context("zed")];
    source.repos = vec![repo("acme/api", "zed")];
    source.active_context = Some("zed".parse().unwrap_or_else(|error| panic!("{error}")));
    let mut state = AppState::new("/tmp/fleet-hub-review-board", now);
    state.apply_snapshot(source, now);
    state.daemon = DaemonLink::Connected;
    if reviews {
        state
            .daemon_capabilities
            .insert(BOARD_REVIEWS_CAPABILITY.to_owned());
    }
    state.screen = Screen::Hub { tab: HubTab::Prs };
    state.pr_tab = PrTab::Review;
    state
}

/// The Hub over a bridge that records what it asks, which is what the board mirror's loader
/// takes; the Test bridge has none to hand it.
fn live_hub_ctx(state: AppState, cx: &mut gpui::TestAppContext) -> (HubCtx, RecordedRequests) {
    let (bridge, recorded) = crate::bridge::Bridge::recording();
    let state = cx.new(|_| state);
    let hub = cx.new(|_| HubState::default());
    (
        HubCtx {
            state,
            hub,
            bridge: HubBridge::Live(bridge),
            rail_scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            pr_scroll: UniformListScrollHandle::new(),
        },
        recorded,
    )
}

/// `zed`'s Reviews board with a card in each named column (`None` is an empty board).
fn reviews_view(columns: &[&str]) -> BoardView {
    let mut board = new_reviews_board(&context("zed"), NOW);
    let mut cards = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let mut card = create_card(
            &mut board,
            &cards,
            format!("card-{index}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
            CardDraft {
                title: format!("Review {index}"),
                ..CardDraft::default()
            },
            NOW,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        card.status_id = column.parse().unwrap_or_else(|error| panic!("{error}"));
        cards.push(card);
    }
    BoardView {
        board,
        cards,
        live_runs: Vec::new(),
    }
}

/// Drops every request nothing answered, so the tasks awaiting them end before the test does
/// and hand back the entities they hold.
///
/// The queued requests carry the reply senders, and the channel outlives its receiver while a
/// task holds the bridge, so they are drained rather than dropped with the receiver.
fn finish(recorded: RecordedRequests, cx: &mut gpui::TestAppContext) {
    while !recorded.take().is_empty() {
        cx.run_until_parked();
    }
    cx.run_until_parked();
}

fn chain_action(chain: &[&str], key: &str) -> Option<&'static str> {
    crate::keymap::action_for_chain(
        chain,
        &Keystroke::parse(key).unwrap_or_else(|error| panic!("{error}")),
    )
    .map(|action| action.name())
}

/// Enters the tab, answers the load with `view`, and folds it.
fn show_board(ctx: &HubCtx, view: BoardView, cx: &mut gpui::TestAppContext) {
    cx.update(|cx| ctx.synchronize(cx));
    ctx.state.update(cx, |state, cx| {
        state.apply_board_view(view);
        cx.notify();
    });
    cx.update(|cx| ctx.synchronize(cx));
    cx.run_until_parked();
}

#[gpui::test]
fn the_review_tab_enters_the_reviews_scope_and_builds_no_flat_list(cx: &mut gpui::TestAppContext) {
    let (ctx, recorded) = live_hub_ctx(review_state(true), cx);
    // A `gh` slice left from before would be a flat list; the board must not build it.
    ctx.hub.update(cx, |hub, _| {
        hub.prs.review = Some(PrSlice {
            prs: vec![pull_request("acme/api", 7, NOW)],
            total: 1,
            ..pr_slice(PrTab::Review)
        });
    });
    cx.update(|cx| ctx.synchronize(cx));
    cx.run_until_parked();

    let zed: ContextId = "zed".parse().unwrap_or_else(|error| panic!("{error}"));
    ctx.state.read_with(cx, |state, _| {
        assert!(state.review_board_is_shown());
        assert_eq!(state.board.scope, Some(BoardScope::Reviews(zed.clone())));
        assert_eq!(state.hub_pane, HubPane::List);
        assert_eq!(state.context_chain(), vec!["Hub", "Prs", "Board"]);
    });
    let asked = recorded.take();
    assert!(
        asked.iter().any(|body| matches!(
            body,
            RequestBody::EnsureReviewsBoard { context_id } if context_id == &zed
        )),
        "{asked:?}"
    );
    assert!(
        !asked.iter().any(|body| matches!(
            body,
            RequestBody::ListPullRequests {
                tab: PrTab::Review,
                ..
            }
        )),
        "the review fetch is retired: {asked:?}"
    );
    cx.read(|cx| assert!(ctx.model(cx).prs.is_empty(), "no flat review list"));
}

#[gpui::test]
fn tab_on_the_review_board_returns_to_mine_and_gives_the_scope_back(cx: &mut gpui::TestAppContext) {
    let (ctx, recorded) = live_hub_ctx(review_state(true), cx);
    show_board(&ctx, reviews_view(&["reviewed"]), cx);
    assert_eq!(
        chain_action(&["Fleet", "Hub", "Prs", "Board"], "tab"),
        Some("prs::NextTab")
    );
    assert_eq!(
        chain_action(&["Fleet", "Hub", "Prs", "Board"], "shift-tab"),
        Some("prs::PrevTab")
    );

    cx.update(|cx| ctx.cycle_tab(cx));
    cx.update(|cx| ctx.synchronize(cx));
    let zed: ContextId = "zed".parse().unwrap_or_else(|error| panic!("{error}"));
    ctx.state.read_with(cx, |state, _| {
        assert_eq!(state.pr_tab, PrTab::Mine);
        assert!(!state.review_board_is_shown());
        assert_eq!(state.board.scope, Some(BoardScope::Context(zed)));
        assert_eq!(state.context_chain(), vec!["Hub", "Prs"]);
    });
    // And `Tab` on Mine is the board again.
    cx.update(|cx| ctx.cycle_tab(cx));
    ctx.state
        .read_with(cx, |state, _| assert_eq!(state.pr_tab, PrTab::Review));
    finish(recorded, cx);
}

#[gpui::test]
fn l_on_the_review_board_moves_a_column(cx: &mut gpui::TestAppContext) {
    let (ctx, recorded) = live_hub_ctx(review_state(true), cx);
    show_board(&ctx, reviews_view(&["pending", "reviewed"]), cx);
    let chain = ["Fleet", "Hub", "Prs", "Board"];
    assert_eq!(chain_action(&chain, "l"), Some("board::NextColumn"));
    assert_eq!(chain_action(&chain, "h"), Some("board::PrevColumn"));

    let (bridge, requests) = crate::bridge::Bridge::recording();
    let before = ctx.state.read_with(cx, |state, _| state.board.focus.column);
    cx.update(|cx| crate::screens::board::next_column(&ctx.state, &bridge, cx));
    let after = ctx.state.read_with(cx, |state, _| state.board.focus.column);
    assert!(after > before, "`l` moved from column {before} to {after}");
    finish(requests, cx);
    finish(recorded, cx);
}

#[gpui::test]
fn p_on_the_review_board_goes_back(cx: &mut gpui::TestAppContext) {
    let (ctx, recorded) = live_hub_ctx(review_state(true), cx);
    show_board(&ctx, reviews_view(&["reviewed"]), cx);
    let chain = ["Fleet", "Hub", "Prs", "Board"];
    assert_eq!(chain_action(&chain, "p"), Some("prs::Back"));
    assert_eq!(chain_action(&chain, "q"), Some("prs::Back"));

    cx.update(|cx| ctx.back_to_worktrees(cx));
    cx.update(|cx| ctx.synchronize(cx));
    ctx.state.read_with(cx, |state, _| {
        assert_eq!(
            state.screen,
            Screen::Hub {
                tab: HubTab::Worktrees
            }
        );
        assert!(!matches!(state.board.scope, Some(BoardScope::Reviews(_))));
    });
    finish(recorded, cx);
}

#[gpui::test]
fn the_review_chip_counts_the_board_cards_waiting_on_you(cx: &mut gpui::TestAppContext) {
    let (ctx, recorded) = live_hub_ctx(review_state(true), cx);
    // Two finished reviews wait in `Reviewed` (Started, no run); the pending one does not.
    let view = reviews_view(&["pending", "reviewed", "reviewed", "published"]);
    assert!(view.board.statuses.iter().any(|status| {
        status.id.as_str() == "reviewed" && status.category == StatusCategory::Started
    }));
    show_board(&ctx, view, cx);

    ctx.state
        .read_with(cx, |state, _| assert_eq!(state.review_pr_count, 2));
    cx.read(|cx| {
        let hub = ctx.hub.read(cx);
        assert_eq!(hub.prs.review_board.waiting, 2);
        assert_eq!(hub.prs.review_board.empty, None);
    });

    // A Mine answer never writes the chip once the Review tab is a board.
    let key = ctx
        .state
        .read_with(cx, |state, _| cache::PrCacheKey::from_state(state));
    let request = ctx.hub.update(cx, |hub, _| {
        hub.prs.set_review_retired(true);
        hub.prs.begin_fetch(key.clone(), Instant::now())[0].1
    });
    let mut async_cx = cx.to_async();
    ctx.apply_slices(
        key,
        PrTab::Mine,
        request,
        pr_response(PrTab::Mine),
        &mut async_cx,
    );
    ctx.state
        .read_with(cx, |state, _| assert_eq!(state.review_pr_count, 2));
    finish(recorded, cx);
}

#[gpui::test]
fn the_review_chip_reads_the_summary_while_another_board_is_shown(cx: &mut gpui::TestAppContext) {
    let mut state = review_state(true);
    state.screen = Screen::Hub {
        tab: HubTab::Worktrees,
    };
    let zed: ContextId = "zed".parse().unwrap_or_else(|error| panic!("{error}"));
    let mut source = state.snapshot.clone().unwrap_or_else(|| snapshot(0));
    source.boards = vec![BoardSummary {
        id: reviews_board_id(&zed),
        context_id: zed,
        worktree_id: None,
        kind: fleet_core::board::BoardKind::Reviews,
        name: "Reviews".to_owned(),
        prefix: "REV".to_owned(),
        backend_kind: "local".to_owned(),
        card_count: 5,
        open_count: 4,
        dirty_count: 0,
        conflict_count: 0,
        working_count: 1,
        attention_count: 1,
        idle_started: 2,
        last_synced_at: None,
        last_error: None,
    }];
    state.apply_snapshot(source, Instant::now());
    let (ctx, recorded) = live_hub_ctx(state, cx);
    cx.update(|cx| ctx.synchronize(cx));
    ctx.state
        .read_with(cx, |state, _| assert_eq!(state.review_pr_count, 3));
    finish(recorded, cx);
}

/// A run of `card` in its column, live when `outcome` is `None`.
fn card_run(
    card: &fleet_core::board::Card,
    outcome: Option<fleet_core::board::RunOutcome>,
) -> fleet_core::board::CardRun {
    fleet_core::board::CardRun {
        id: fleet_core::agents::DelegationId::new(),
        thread_id: None,
        status_id: card.status_id.clone(),
        action: fleet_core::board::ActionKind::Prompt,
        provider: fleet_core::agents::AgentKind::Codex,
        model: None,
        effort: None,
        started_at: NOW.to_owned(),
        ended_at: outcome.is_some().then(|| NOW.to_owned()),
        outcome,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    }
}

#[gpui::test]
fn the_review_count_is_the_same_with_the_board_held_and_without_it(cx: &mut gpui::TestAppContext) {
    use fleet_core::board::RunOutcome;

    let (ctx, recorded) = live_hub_ctx(review_state(true), cx);
    // Pending (not started) · reviewing with a live run · reviewing, failed · reviewed, idle ·
    // reviewed, failed (wants you and idle: counted once) · published.
    let mut view = reviews_view(&[
        "pending",
        "reviewing",
        "reviewing",
        "reviewed",
        "reviewed",
        "published",
    ]);
    let live = card_run(&view.cards[1], None);
    view.cards[1].runs.push(live);
    let failed = card_run(&view.cards[2], Some(RunOutcome::Failed));
    view.cards[2].runs.push(failed);
    let failed = card_run(&view.cards[4], Some(RunOutcome::Failed));
    view.cards[4].runs.push(failed);
    show_board(&ctx, view.clone(), cx);
    let held = ctx.state.read_with(cx, |state, _| state.review_pr_count);
    assert_eq!(held, 3);

    // The daemon's summary of the same cards, and a mirror that no longer holds the board.
    let summary = fleet_core::board::summarize(
        &view.board,
        &view.cards,
        &[],
        &chrono::Utc::now().to_rfc3339(),
    );
    assert_eq!(summary.attention_count + summary.idle_started, 3);
    ctx.state.update(cx, |state, cx| {
        let mut source = state.snapshot.clone().unwrap_or_else(|| snapshot(0));
        source.boards = vec![summary];
        state.apply_snapshot(source, Instant::now());
        state.board.view = None;
        state.screen = Screen::Hub {
            tab: HubTab::Worktrees,
        };
        cx.notify();
    });
    cx.update(|cx| ctx.synchronize(cx));
    ctx.state
        .read_with(cx, |state, _| assert_eq!(state.review_pr_count, held));
    finish(recorded, cx);
}

#[gpui::test]
fn an_empty_reviews_board_says_so_and_offers_the_schedule(cx: &mut gpui::TestAppContext) {
    let mut state = review_state(true);
    state
        .daemon_capabilities
        .insert(fleet_proto::response::SCHEDULES_CAPABILITY.to_owned());
    let (ctx, recorded) = live_hub_ctx(state, cx);
    show_board(&ctx, reviews_view(&[]), cx);
    let board = reviews_board_id(&"zed".parse().unwrap_or_else(|error| panic!("{error}")));
    let asked = recorded.take();
    assert!(
        asked.iter().any(|body| matches!(
            body,
            RequestBody::ListSchedules { board_id: Some(id) } if id == &board
        )),
        "{asked:?}"
    );
    // Before the schedules answer, the empty state makes no offer it cannot back.
    cx.read(|cx| assert_eq!(ctx.hub.read(cx).prs.review_board.empty, Some(false)));
    ctx.state.update(cx, |state, cx| {
        state.schedules.apply(&board, Vec::new());
        cx.notify();
    });
    cx.update(|cx| ctx.synchronize(cx));
    cx.read(|cx| assert_eq!(ctx.hub.read(cx).prs.review_board.empty, Some(true)));
    assert_eq!(prs_screen::REVIEWS_EMPTY, "No reviews yet.");
    assert_eq!(
        prs_screen::REVIEWS_ADD_SCHEDULE,
        "\u{23CE} add the GitHub review schedule"
    );
    finish(recorded, cx);
}

#[gpui::test]
fn reentering_review_retries_one_failed_schedule_load(cx: &mut gpui::TestAppContext) {
    let (ctx, recorded) = live_hub_ctx(schedules_state(), cx);
    let view = reviews_view(&[]);
    let board = view.board.id.clone();

    cx.update(|cx| ctx.synchronize(cx));
    assert!(matches!(
        recorded.respond_next(Ok(ResponseBody::BoardBackends(Vec::new()))),
        RequestBody::ListBoardBackends {}
    ));
    cx.run_until_parked();
    assert!(matches!(
        recorded.respond_next(Ok(ResponseBody::Board(view.clone()))),
        RequestBody::EnsureReviewsBoard { .. }
    ));
    cx.run_until_parked();
    cx.update(|cx| ctx.synchronize(cx));
    loop {
        let request = recorded.respond_next_with(|body| match body {
            RequestBody::ListPullRequests { tab, .. } => pr_response(*tab),
            RequestBody::ListSchedules { .. } => Err(ProtoError {
                kind: ErrorKind::Unknown,
                message: "schedules.json is unreadable".to_owned(),
            }),
            _ => Err(ProtoError {
                kind: ErrorKind::Unknown,
                message: "unrelated request is outside this test".to_owned(),
            }),
        });
        cx.run_until_parked();
        if matches!(
            request,
            RequestBody::ListSchedules { board_id: Some(ref id) } if id == &board
        ) {
            break;
        }
    }
    for _ in 0..3 {
        cx.update(|cx| ctx.synchronize(cx));
    }
    assert!(
        recorded
            .take()
            .into_iter()
            .all(|body| !matches!(body, RequestBody::ListSchedules { .. })),
        "a failed answer is not retried during the same activation"
    );

    cx.update(|cx| ctx.cycle_tab(cx));
    cx.update(|cx| ctx.synchronize(cx));
    recorded.take();
    cx.run_until_parked();
    cx.update(|cx| ctx.cycle_tab(cx));
    cx.update(|cx| ctx.synchronize(cx));
    recorded.take();
    cx.run_until_parked();
    ctx.state.update(cx, |state, cx| {
        state.board.loading = false;
        state.apply_board_view(view);
        cx.notify();
    });
    cx.update(|cx| ctx.synchronize(cx));
    assert!(matches!(
        recorded.respond_next(Ok(ResponseBody::Schedules(Vec::new()))),
        RequestBody::ListSchedules { board_id: Some(id) } if id == board
    ));
    cx.run_until_parked();
    cx.update(|cx| ctx.synchronize(cx));
    cx.read(|cx| {
        assert_eq!(ctx.hub.read(cx).prs.review_board.empty, Some(true));
    });
    assert!(
        cx.update(|cx| ctx.add_review_schedule(cx)),
        "the empty-state CTA and Enter action are enabled"
    );

    for _ in 0..3 {
        cx.update(|cx| ctx.synchronize(cx));
    }
    assert!(
        recorded
            .take()
            .into_iter()
            .all(|body| !matches!(body, RequestBody::ListSchedules { .. })),
        "a successful answer is not loaded again without another activation"
    );
    finish(recorded, cx);
}

/// An empty Reviews board with its schedules answered (none), on a daemon with `schedules`.
fn empty_board_offering_the_schedule(
    ctx: &HubCtx,
    recorded: &RecordedRequests,
    cx: &mut gpui::TestAppContext,
) {
    show_board(ctx, reviews_view(&[]), cx);
    recorded.take();
    let board = reviews_board_id(&"zed".parse().unwrap_or_else(|error| panic!("{error}")));
    ctx.state.update(cx, |state, cx| {
        state.schedules.apply(&board, Vec::new());
        cx.notify();
    });
    cx.update(|cx| ctx.synchronize(cx));
    cx.read(|cx| assert_eq!(ctx.hub.read(cx).prs.review_board.empty, Some(true)));
}

fn schedules_state() -> AppState {
    let mut state = review_state(true);
    state
        .daemon_capabilities
        .insert(fleet_proto::response::SCHEDULES_CAPABILITY.to_owned());
    state
}

#[gpui::test]
fn enter_on_the_empty_review_board_opens_the_schedules_starter(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let (ctx, recorded) = live_hub_ctx(schedules_state(), cx);
    empty_board_offering_the_schedule(&ctx, &recorded, cx);

    cx.update(|cx| ctx.open_review_card(cx));
    ctx.state.read_with(cx, |state, _| {
        assert_eq!(
            state.overlay,
            Some(Overlay::Dialog(Dialogs::BoardSettings)),
            "`Enter` opens Board settings itself"
        );
    });
    // The host seeds the dialog on the overlay transition; the section request waits for it.
    let (bridge, requests) = crate::bridge::Bridge::recording();
    cx.update(|cx| {
        crate::dialogs::seed(&Dialogs::BoardSettings, &ctx.state, &bridge, cx);
        let (section, form) = crate::dialogs::schedules_form_probe(&ctx.state, cx);
        assert_eq!(section, crate::dialogs::BoardSection::Schedules);
        assert_eq!(form.as_deref(), Some("GitHub reviews"));
    });
    finish(requests, cx);
    finish(recorded, cx);
}

/// The empty state's button is the pointer's way to the starter (ADR 0023): its click runs
/// what `Enter` runs, and there is nothing to run once the board has a schedule.
#[gpui::test]
fn the_empty_review_boards_button_opens_the_schedules_starter(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let (ctx, recorded) = live_hub_ctx(schedules_state(), cx);
    empty_board_offering_the_schedule(&ctx, &recorded, cx);

    assert!(cx.update(|cx| ctx.add_review_schedule(cx)));
    ctx.state.read_with(cx, |state, _| {
        assert_eq!(state.overlay, Some(Overlay::Dialog(Dialogs::BoardSettings)));
    });
    let (bridge, requests) = crate::bridge::Bridge::recording();
    cx.update(|cx| {
        crate::dialogs::seed(&Dialogs::BoardSettings, &ctx.state, &bridge, cx);
        let (section, form) = crate::dialogs::schedules_form_probe(&ctx.state, cx);
        assert_eq!(section, crate::dialogs::BoardSection::Schedules);
        assert_eq!(form.as_deref(), Some("GitHub reviews"));
    });
    finish(requests, cx);
    finish(recorded, cx);
}

#[gpui::test]
fn a_refused_enter_leaves_no_section_request_behind(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let (ctx, recorded) = live_hub_ctx(schedules_state(), cx);
    empty_board_offering_the_schedule(&ctx, &recorded, cx);
    ctx.state.update(cx, |state, cx| {
        state.daemon = DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
            reason: crate::state::DaemonLossReason::ConnectionLost,
        };
        cx.notify();
    });

    cx.update(|cx| ctx.open_review_card(cx));
    ctx.state.read_with(cx, |state, _| {
        assert_eq!(state.overlay, None, "nothing opened")
    });
    // The next `,` opens the dialog plainly: no starter form was left waiting for it.
    ctx.state.update(cx, |state, cx| {
        state.daemon = DaemonLink::Connected;
        state.open_overlay(Overlay::Dialog(Dialogs::BoardSettings));
        cx.notify();
    });
    let (bridge, requests) = crate::bridge::Bridge::recording();
    cx.update(|cx| {
        crate::dialogs::seed(&Dialogs::BoardSettings, &ctx.state, &bridge, cx);
        let (_, form) = crate::dialogs::schedules_form_probe(&ctx.state, cx);
        assert_eq!(form, None);
    });
    finish(requests, cx);
    finish(recorded, cx);
}

#[gpui::test]
fn the_mine_fetch_is_unchanged_and_asks_for_mine_alone(cx: &mut gpui::TestAppContext) {
    let mut state = review_state(true);
    state.pr_tab = PrTab::Mine;
    let (ctx, harness) = test_hub_ctx(state, cx);
    cx.update(|cx| ctx.fetch_pull_requests(false, cx));
    assert_eq!(harness.len(), 1, "Mine alone");
    assert!(matches!(
        harness.respond_next(pr_response_with(
            PrTab::Mine,
            vec![pull_request("acme/api", 1, NOW)]
        )),
        RequestBody::ListPullRequests {
            tab: PrTab::Mine,
            force: false,
            ..
        }
    ));
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(ctx.model(cx).prs.len(), 1, "Mine lists as before");
        let hub = ctx.hub.read(cx);
        assert!(hub.prs.review_retired());
        assert!(!hub.prs.loading(PrTab::Review));
    });
}

#[gpui::test]
fn an_old_daemon_keeps_the_flat_review_list(cx: &mut gpui::TestAppContext) {
    let (ctx, harness) = test_hub_ctx(review_state(false), cx);
    cx.update(|cx| ctx.fetch_pull_requests(false, cx));
    assert_eq!(harness.len(), 2, "both tabs");
    harness.respond_next(pr_response(PrTab::Mine));
    cx.run_until_parked();
    assert!(matches!(
        harness.respond_next(pr_response_with(
            PrTab::Review,
            vec![pull_request("acme/api", 9, NOW)]
        )),
        RequestBody::ListPullRequests {
            tab: PrTab::Review,
            ..
        }
    ));
    cx.run_until_parked();
    ctx.state.read_with(cx, |state, _| {
        assert!(!state.review_board_is_shown());
        assert_eq!(state.context_chain(), vec!["Hub", "Prs"]);
        assert_eq!(state.review_pr_count, 1);
    });
    cx.read(|cx| {
        let model = ctx.model(cx);
        assert_eq!(model.prs.len(), 1);
        assert_eq!(model.prs[0].number, 9);
    });
}
