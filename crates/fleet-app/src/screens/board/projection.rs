use std::collections::HashMap;

use gpui::SharedString;

use super::*;

#[derive(Default)]
pub(super) struct ProjectionCache {
    key: Option<ProjectionKey>,
    model: Rc<BoardModel>,
}

/// Every input the model is derived from, as revisions and cheap values.
///
/// The revision is `BoardState::revision`, not `AppState::snapshot_revision`: a card mutation
/// lands through `AppState::apply_card` and never through a snapshot, so a board keyed on the
/// snapshot would keep drawing the columns the card left.
#[derive(PartialEq, Eq)]
struct ProjectionKey {
    source: u64,
    board: Option<BoardId>,
    query: String,
    backend: Option<String>,
    /// `CardMarks::revision`: a mark can move with no card and no view behind it — a pending
    /// run going `Stalled` is a clock tick — and the model draws the marks.
    marks: u64,
    /// The branch and pull request of every worktree a card links, which live in the snapshot
    /// and the PR badge cache rather than in the board.
    links: Vec<(WorktreeId, board_screen::LinkedBranch)>,
    /// `SchedulesMirror::revision` and the `schedules` capability, which the header strip is
    /// built from (BOARD §11.8).
    schedules: (u64, bool),
    /// The wall clock's minute while a run is live or the board has schedules, so `working 4m`
    /// and the strip's `next 14:05` tick once a minute and a board with neither is never
    /// rebuilt by the clock at all.
    minute: Option<i64>,
}

/// The board model for this frame, rebuilt only when one of its inputs changed.
///
/// `now` is an input only as the minute, and only while a run is live: it stamps the relative
/// synced age and a live run's `working 4m`, and re-deriving a whole board once a second to move
/// those labels is the cost this cache exists to avoid. `focus` is not an input either — which tile is selected is decided when the tile is
/// composed, not when the model is built.
pub(super) fn prepare(
    state: &AppState,
    cache: &RefCell<ProjectionCache>,
    now: i64,
) -> Rc<BoardModel> {
    let mut cache = cache.borrow_mut();
    let backend = state
        .board()
        .map(|view| state.backend_label(&view.board.backend.kind));
    let has_schedules = state
        .board()
        .is_some_and(|view| !state.schedules.for_board(&view.board.id).is_empty());
    let key = ProjectionKey {
        source: state.board.revision,
        board: state.board().map(|view| view.board.id.clone()),
        query: state.board.filter.clone(),
        backend: backend.clone(),
        marks: state.board.marks.revision,
        links: links(state),
        schedules: (state.schedules.revision, state.board_takes_schedules()),
        minute: (state.board.marks.working > 0 || has_schedules).then_some(now / 60),
    };
    if cache.key.as_ref() != Some(&key) {
        let mut marks = marks(state);
        marks.links = key.links.iter().cloned().collect();
        cache.model = Rc::new(state.board().map_or_else(BoardModel::default, |view| {
            board_screen::build(view, &state.board.filter, backend.as_deref(), now, &marks)
                .with_schedules(board_screen::ScheduleStrip::of(
                    state.board_takes_schedules(),
                    state.schedules.for_board(&view.board.id),
                    now,
                ))
        }));
        cache.key = Some(key);
    }
    cache.model.clone()
}

/// The marks the model draws, in the view layer's own vocabulary.
///
/// `AppState::refresh_card_marks` did the folding; this only restates its answer in the types
/// `views::board_screen` speaks, which are the kit's. It runs inside the rebuild branch alone,
/// so a frame that reuses the cached model copies nothing — and a rebuild already allocates a
/// title, a key and a chip list per card, beside which two `Copy` marks are noise.
fn marks(state: &AppState) -> board_screen::BoardMarks {
    let marks = &state.board.marks;
    board_screen::BoardMarks {
        by_card: marks
            .by_card
            .iter()
            .map(|(card, tile)| {
                (
                    card.clone(),
                    board_screen::TileMark {
                        run: tile.run,
                        blocked: tile.blocked,
                    },
                )
            })
            .collect(),
        working: marks.working,
        waiting: marks.waiting,
        needs_you: marks.needs_you,
        links: HashMap::new(),
    }
}

/// The branch of every worktree a card on the shown board links, with its pull request.
///
/// Read per frame to key the cache, so it walks only the cards that link a worktree and asks
/// two maps about each: a board's linked cards are the handful in flight, never the backlog.
fn links(state: &AppState) -> Vec<(WorktreeId, board_screen::LinkedBranch)> {
    let (Some(view), Some(snapshot)) = (state.board(), state.snapshot.as_ref()) else {
        return Vec::new();
    };
    let mut links: Vec<(WorktreeId, board_screen::LinkedBranch)> = Vec::new();
    for worktree in view
        .cards
        .iter()
        .filter_map(|card| card.worktree_id.as_ref())
    {
        if links.iter().any(|(linked, _)| linked == worktree) {
            continue;
        }
        let Some(found) = snapshot.worktrees.iter().find(|item| &item.id == worktree) else {
            continue;
        };
        let pr = state
            .pr_badges
            .get(&(found.repo_id.clone(), found.branch.clone()))
            .copied();
        links.push((
            worktree.clone(),
            board_screen::LinkedBranch {
                branch: SharedString::from(found.branch.clone()),
                pr,
            },
        ));
    }
    links
}
