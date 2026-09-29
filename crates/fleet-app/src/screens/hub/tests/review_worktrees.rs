//! Review worktrees (`pull/<n>/head` checkouts) in the Worktrees list: hidden by default, left out
//! of every count that must agree with the rows, and a cursor that stays put across `v`
//! (UX-SPEC §3.3).

use super::*;

/// A pull-request checkout, as the daemon's `create_pr_local` writes one.
fn review_worktree(id: &str, repo: &str, number: u64) -> Worktree {
    let mut review = worktree(id, repo, "2026-09-04T10:00:00Z");
    review.base_ref = format!("pull/{number}/head");
    review
}

/// One context, `acme`, with the given repositories and worktrees.
fn review_state(repos: &[&str], worktrees: Vec<Worktree>) -> AppState {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet-hub-reviews", now);
    let mut source = snapshot(0);
    source.contexts = vec![context("acme")];
    source.repos = repos.iter().map(|id| repo(id, "acme")).collect();
    source.worktrees = worktrees;
    state.apply_snapshot(source, now);
    state
}

fn rail_count(model: &projection::HubModel, repo: Option<&str>) -> Option<usize> {
    model
        .rail
        .iter()
        .find(|row| row.repo.as_ref().map(RepoId::as_str) == repo)
        .and_then(|row| row.count)
}

#[test]
fn hidden_review_worktrees_leave_the_rows_the_rail_counts_and_the_filter_total() {
    let mut state = review_state(
        &["acme/api"],
        vec![
            worktree("acme/api#one", "acme/api", "2026-09-04T10:00:00Z"),
            worktree("acme/api#two", "acme/api", "2026-09-04T10:00:00Z"),
            review_worktree("acme/api#pr-7", "acme/api", 7),
        ],
    );
    let hub = HubState::default();
    assert!(
        !state.show_review_worktrees,
        "review worktrees start hidden"
    );

    let hidden = projection::prepare(&state, &hub, 1_788_523_200);
    assert_eq!(hidden.worktrees.len(), 2);
    assert!(
        hidden
            .worktrees
            .iter()
            .all(|row| row.id.as_str() != "acme/api#pr-7")
    );
    assert_eq!(
        hidden.worktree_total, 2,
        "the total leaves hidden reviews out"
    );
    assert_eq!(hidden.review_hidden, 1);
    assert_eq!(
        rail_count(&hidden, None),
        Some(2),
        "`All` counts listed rows"
    );
    assert_eq!(rail_count(&hidden, Some("acme/api")), Some(2));
    assert!(
        hidden.worktree_summary.starts_with("2 "),
        "the subtitle counts listed rows: {}",
        hidden.worktree_summary
    );
    assert_eq!(
        hidden.worktree_reviews.as_deref(),
        Some("1 review worktree hidden"),
        "the subtitle's segment names what the list leaves out"
    );
    state.displayed_hub = hidden.displayed();
    assert_eq!(state.displayed_hub.review_hidden, 1);
    assert_eq!(crate::presentation::filter_counts(&state), (2, 2));

    // The flag is a projection input: flipping it rebuilds the model at once (the row counts
    // below would still read the hidden model otherwise), and only a flip rebuilds it.
    state.show_review_worktrees = true;
    let shown = projection::prepare(&state, &hub, 1_788_523_201);
    assert!(
        Rc::ptr_eq(&shown, &projection::prepare(&state, &hub, 1_788_523_202)),
        "an unchanged flag is a cache hit, not a rebuild per frame"
    );
    assert_eq!(shown.worktrees.len(), 3);
    assert_eq!(shown.worktree_total, 3);
    assert_eq!(shown.review_hidden, 0);
    assert_eq!(
        shown.worktree_reviews.as_deref(),
        Some("1 review worktree"),
        "while shown, the segment counts the review rows it marks"
    );
    assert_eq!(
        shown
            .worktrees
            .iter()
            .filter_map(|row| row.review)
            .collect::<Vec<_>>(),
        vec![7],
        "the shown review row carries its pull request for the `review` badge"
    );
    assert_eq!(rail_count(&shown, None), Some(3));
    assert_eq!(rail_count(&shown, Some("acme/api")), Some(3));
    state.displayed_hub = shown.displayed();
    assert_eq!(crate::presentation::filter_counts(&state), (3, 3));

    state.show_review_worktrees = false;
    let hidden_again = projection::prepare(&state, &hub, 1_788_523_203);
    assert_eq!(
        hidden_again.worktrees.len(),
        2,
        "flipping back rebuilds again"
    );
    assert_eq!(hidden_again.review_hidden, 1);
}

#[test]
fn a_hidden_degraded_review_worktree_lights_no_rail_glyph_or_issue_chip() {
    let mut review = review_worktree("acme/api#pr-7", "acme/api", 7);
    review.degraded = Some(fleet_core::model::Degraded {
        kind: "post_create_hooks".to_owned(),
        step: "1".to_owned(),
        exit_code: Some(1),
        at: "2026-09-04T11:00:00Z".to_owned(),
        log_path: "/tmp/hooks.log".to_owned(),
    });
    let mut state = review_state(
        &["acme/api"],
        vec![
            worktree("acme/api#one", "acme/api", "2026-09-04T10:00:00Z"),
            review,
        ],
    );
    let hub = HubState::default();
    let repo_row = |model: &projection::HubModel| {
        model
            .rail
            .iter()
            .find(|row| row.kind == RailKind::Repo)
            .map(|row| (row.glyph, row.issues.clone()))
            .unwrap_or_else(|| panic!("acme/api has a rail row"))
    };

    let (glyph, issues) = repo_row(&projection::prepare(&state, &hub, 1_788_523_200));
    assert_ne!(glyph, fleet_ui_kit::StatusKind::Degraded);
    assert_eq!(issues, None);

    state.show_review_worktrees = true;
    let (glyph, issues) = repo_row(&projection::prepare(&state, &hub, 1_788_523_201));
    assert_eq!(glyph, fleet_ui_kit::StatusKind::Degraded);
    assert_eq!(issues.as_deref(), Some("1 issue"));
}

#[test]
fn the_hidden_review_count_follows_the_repository_scope_but_rail_counts_do_not() {
    let mut state = review_state(
        &["acme/api", "acme/web"],
        vec![
            worktree("acme/api#one", "acme/api", "2026-09-04T10:00:00Z"),
            review_worktree("acme/api#pr-7", "acme/api", 7),
            worktree("acme/web#own", "acme/web", "2026-09-04T10:00:00Z"),
            review_worktree("acme/web#pr-9", "acme/web", 9),
        ],
    );
    let hub = HubState::default();

    let all = projection::prepare(&state, &hub, 1_788_523_200);
    assert_eq!(all.worktree_total, 2);
    assert_eq!(all.review_hidden, 2);

    state.scope = RepoScope::Repo("acme/api".parse().expect("repo id"));
    let scoped = projection::prepare(&state, &hub, 1_788_523_201);
    assert_eq!(scoped.worktree_total, 1);
    assert_eq!(
        scoped.review_hidden, 1,
        "a review worktree in another repository is not hidden from this scope"
    );
    assert_eq!(rail_count(&scoped, None), Some(2));
    assert_eq!(rail_count(&scoped, Some("acme/api")), Some(1));
    assert_eq!(
        rail_count(&scoped, Some("acme/web")),
        Some(1),
        "the rail counts the context, not the selected repository"
    );
}

/// A review worktree created at `created_at`, so a test can place it between its neighbours.
fn review_worktree_at(id: &str, repo: &str, number: u64, created_at: &str) -> Worktree {
    let mut review = review_worktree(id, repo, number);
    review.created_at = created_at.to_owned();
    review
}

/// The live Worktrees list (connected, list pane focused) over `worktrees`, with the cursor and
/// its identity anchor on `anchor`, reconciled once the way the Hub's state observer does.
fn review_list(
    worktrees: Vec<Worktree>,
    shown: bool,
    anchor: &str,
    cx: &mut gpui::TestAppContext,
) -> (HubCtx, HubRequestHarness) {
    let mut state = review_state(&["acme/api"], worktrees);
    state.show_review_worktrees = shown;
    state.daemon = crate::state::DaemonLink::Connected;
    state.screen = Screen::Hub {
        tab: HubTab::Worktrees,
    };
    state.hub_pane = HubPane::List;
    let (ctx, harness) = test_hub_ctx(state, cx);
    let anchor: WorktreeId = anchor.parse().expect("worktree id");
    cx.update(|cx| {
        let index = ctx
            .model(cx)
            .worktrees
            .iter()
            .position(|row| row.id == anchor)
            .unwrap_or_else(|| panic!("{anchor} is not a listed row"));
        ctx.state
            .update(cx, |state, _| state.cursors.worktrees = index);
        ctx.hub
            .update(cx, |hub, _| hub.selection.worktree = Some(anchor));
        ctx.synchronize(cx);
    });
    (ctx, harness)
}

/// Closes every request the Hub queued (the sweep, the debounced inspection) and lets the tasks
/// awaiting them end, so no task holds the entities past teardown.
fn close_requests(harness: &HubRequestHarness, cx: &mut gpui::TestAppContext) {
    cx.run_until_parked();
    while harness.len() > 0 {
        harness.close_next();
        cx.run_until_parked();
    }
}

/// What `Shell::toggle_review_worktrees` does, followed by the synchronize its notify triggers
/// through the Hub's state observer (the test context has no observer).
fn set_reviews_shown(ctx: &HubCtx, shown: bool, cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        ctx.state.update(cx, |state, cx| {
            state.show_review_worktrees = shown;
            cx.notify();
        });
        ctx.synchronize(cx);
    });
}

/// Every reader of the Worktrees cursor, read out of the app in one go so the assertions run in
/// [`landed`]'s own body, where `#[track_caller]` can point at the step that broke.
struct Readers {
    cursor: usize,
    rows: usize,
    displayed: Vec<WorktreeId>,
    detail_row: Option<WorktreeId>,
    anchor: Option<WorktreeId>,
    inspect_target: Option<WorktreeId>,
}

/// The row the cursor is on, as its index and id, after asserting that every reader of the
/// cursor names that same row: the identity anchor, the detail panel's row, the harness's
/// displayed list and the inspection target.
#[track_caller]
fn landed(ctx: &HubCtx, cx: &mut gpui::TestAppContext) -> (usize, String) {
    let readers = cx.read(|cx| {
        let state = ctx.state.read(cx);
        let hub = ctx.hub.read(cx);
        Readers {
            cursor: state.cursors.worktrees,
            rows: ctx.model(cx).worktrees.len(),
            displayed: state
                .displayed_hub
                .worktrees
                .iter()
                .map(|row| row.id.clone())
                .collect(),
            detail_row: ctx.selected_worktree(cx).map(|row| row.id),
            anchor: hub.selection.worktree.clone(),
            inspect_target: hub.inspect_target.clone(),
        }
    });
    let Readers {
        cursor,
        rows,
        displayed,
        detail_row,
        anchor,
        inspect_target,
    } = readers;
    assert_eq!(
        displayed.len(),
        rows,
        "the harness sees the rows the list draws"
    );
    let Some(row) = detail_row else {
        panic!("cursor {cursor} is past the {rows} rows");
    };
    assert_eq!(
        displayed.get(cursor),
        Some(&row),
        "the harness row under the cursor"
    );
    assert_eq!(anchor.as_ref(), Some(&row), "the identity anchor");
    assert_eq!(inspect_target.as_ref(), Some(&row), "the inspection target");
    (cursor, row.as_str().to_owned())
}

#[gpui::test]
fn hiding_the_review_row_under_the_cursor_lands_on_its_neighbour_and_showing_keeps_it(
    cx: &mut gpui::TestAppContext,
) {
    let (ctx, harness) = review_list(
        vec![
            worktree("acme/api#own-a", "acme/api", "2026-09-04T12:00:00Z"),
            review_worktree_at("acme/api#pr-7", "acme/api", 7, "2026-09-04T11:00:00Z"),
            worktree("acme/api#own-c", "acme/api", "2026-09-04T10:00:00Z"),
        ],
        true,
        "acme/api#pr-7",
        cx,
    );
    assert_eq!(landed(&ctx, cx), (1, "acme/api#pr-7".to_owned()));

    set_reviews_shown(&ctx, false, cx);
    assert_eq!(
        landed(&ctx, cx),
        (1, "acme/api#own-c".to_owned()),
        "the cursor keeps its slot, so it lands on the row that moved up into it"
    );

    set_reviews_shown(&ctx, true, cx);
    assert_eq!(
        landed(&ctx, cx),
        (2, "acme/api#own-c".to_owned()),
        "the anchor moved with the cursor, so showing reviews again does not jump back"
    );
    close_requests(&harness, cx);
}

#[gpui::test]
fn hiding_a_review_row_at_the_end_clamps_to_the_last_row(cx: &mut gpui::TestAppContext) {
    let (ctx, harness) = review_list(
        vec![
            worktree("acme/api#own-a", "acme/api", "2026-09-04T12:00:00Z"),
            worktree("acme/api#own-b", "acme/api", "2026-09-04T11:00:00Z"),
            review_worktree_at("acme/api#pr-7", "acme/api", 7, "2026-09-04T10:00:00Z"),
        ],
        true,
        "acme/api#pr-7",
        cx,
    );
    assert_eq!(landed(&ctx, cx), (2, "acme/api#pr-7".to_owned()));

    set_reviews_shown(&ctx, false, cx);
    assert_eq!(landed(&ctx, cx), (1, "acme/api#own-b".to_owned()));

    set_reviews_shown(&ctx, true, cx);
    assert_eq!(landed(&ctx, cx), (1, "acme/api#own-b".to_owned()));
    close_requests(&harness, cx);
}

#[gpui::test]
fn showing_review_rows_above_the_cursor_keeps_the_same_worktree(cx: &mut gpui::TestAppContext) {
    let (ctx, harness) = review_list(
        vec![
            review_worktree_at("acme/api#pr-7", "acme/api", 7, "2026-09-04T12:00:00Z"),
            review_worktree_at("acme/api#pr-8", "acme/api", 8, "2026-09-04T11:30:00Z"),
            worktree("acme/api#own-a", "acme/api", "2026-09-04T11:00:00Z"),
            worktree("acme/api#own-c", "acme/api", "2026-09-04T10:00:00Z"),
        ],
        false,
        "acme/api#own-c",
        cx,
    );
    assert_eq!(landed(&ctx, cx), (1, "acme/api#own-c".to_owned()));

    set_reviews_shown(&ctx, true, cx);
    assert_eq!(
        landed(&ctx, cx),
        (3, "acme/api#own-c".to_owned()),
        "rows appearing above move the index, not the worktree"
    );

    set_reviews_shown(&ctx, false, cx);
    assert_eq!(landed(&ctx, cx), (1, "acme/api#own-c".to_owned()));
    close_requests(&harness, cx);
}

#[gpui::test]
fn a_scope_of_only_review_worktrees_keeps_its_anchor_while_hidden(cx: &mut gpui::TestAppContext) {
    let (ctx, harness) = review_list(
        vec![
            review_worktree_at("acme/api#pr-7", "acme/api", 7, "2026-09-04T12:00:00Z"),
            review_worktree_at("acme/api#pr-8", "acme/api", 8, "2026-09-04T11:00:00Z"),
        ],
        true,
        "acme/api#pr-8",
        cx,
    );
    assert_eq!(landed(&ctx, cx), (1, "acme/api#pr-8".to_owned()));

    set_reviews_shown(&ctx, false, cx);
    let pr_8: WorktreeId = "acme/api#pr-8".parse().expect("worktree id");
    cx.read(|cx| {
        let state = ctx.state.read(cx);
        assert!(state.displayed_hub.worktrees.is_empty());
        assert_eq!(state.cursors.worktrees, 0);
        assert!(ctx.selected_worktree(cx).is_none(), "no detail panel row");
        assert_eq!(ctx.hub.read(cx).inspect_target, None);
        assert_eq!(
            ctx.hub.read(cx).selection.worktree.as_ref(),
            Some(&pr_8),
            "an empty list has no neighbour to move the anchor to"
        );
    });

    set_reviews_shown(&ctx, true, cx);
    assert_eq!(landed(&ctx, cx), (1, "acme/api#pr-8".to_owned()));
    close_requests(&harness, cx);
}

/// Back from a review worktree opened elsewhere (the Reviews board, the PR screen, the palette)
/// names a row the list hides; the cursor stays on the row it was on (UX-SPEC §3.1).
#[gpui::test]
fn back_to_a_hidden_review_worktree_leaves_the_cursor_where_it_was(cx: &mut gpui::TestAppContext) {
    let (ctx, harness) = review_list(
        vec![
            worktree("acme/api#own-a", "acme/api", "2026-09-04T12:00:00Z"),
            review_worktree_at("acme/api#pr-7", "acme/api", 7, "2026-09-04T11:00:00Z"),
            worktree("acme/api#own-c", "acme/api", "2026-09-04T10:00:00Z"),
        ],
        false,
        "acme/api#own-c",
        cx,
    );
    assert_eq!(landed(&ctx, cx), (1, "acme/api#own-c".to_owned()));

    cx.update(|cx| {
        ctx.state.update(cx, |state, cx| {
            state.pending_worktree_focus = Some("acme/api#pr-7".parse().expect("worktree id"));
            cx.notify();
        });
        ctx.synchronize(cx);
    });
    assert_eq!(landed(&ctx, cx), (1, "acme/api#own-c".to_owned()));
    cx.read(|cx| assert_eq!(ctx.state.read(cx).pending_worktree_focus, None));
    close_requests(&harness, cx);
}
