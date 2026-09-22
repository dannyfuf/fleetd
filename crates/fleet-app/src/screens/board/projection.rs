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
}

/// The board model for this frame, rebuilt only when one of its inputs changed.
///
/// `now` is not an input: like the Hub's projection it only stamps the relative synced age, and
/// re-deriving a whole board once a second to move that label is the cost this cache exists to
/// avoid. `focus` is not an input either — which tile is selected is decided when the tile is
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
    let key = ProjectionKey {
        source: state.board.revision,
        board: state.board().map(|view| view.board.id.clone()),
        query: state.board.filter.clone(),
        backend: backend.clone(),
        marks: state.board.marks.revision,
    };
    if cache.key.as_ref() != Some(&key) {
        let marks = marks(state);
        cache.model = Rc::new(state.board().map_or_else(BoardModel::default, |view| {
            board_screen::build(view, &state.board.filter, backend.as_deref(), now, &marks)
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
        needs_you: marks.needs_you,
    }
}
