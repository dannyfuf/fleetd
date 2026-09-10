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
    };
    if cache.key.as_ref() != Some(&key) {
        cache.model = Rc::new(state.board().map_or_else(BoardModel::default, |view| {
            board_screen::build(view, &state.board.filter, backend.as_deref(), now)
        }));
        cache.key = Some(key);
    }
    cache.model.clone()
}
