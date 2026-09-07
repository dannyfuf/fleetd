use super::*;

/// Nonarchived cards in stable column order.
#[must_use]
pub fn column_cards<'a>(cards: &'a [Card], status_id: &StatusId) -> Vec<&'a Card> {
    let mut column: Vec<_> = cards
        .iter()
        .filter(|c| !c.archived && c.status_id == *status_id)
        .collect();
    column.sort_by(|a, b| {
        (a.position, &a.created_at, a.number).cmp(&(b.position, &b.created_at, b.number))
    });
    column
}
/// First configured status in the requested category.
#[must_use]
pub fn first_status_in(board: &Board, category: StatusCategory) -> Option<&Status> {
    board.statuses.iter().find(|s| s.category == category)
}
/// Renders the branch template as a nonempty slug of at most 48 ASCII characters.
///
/// The result also names a Git branch, so it never carries a `.`: a trailing period, a `..`
/// pair or a `.lock` component are all rejected by `validate_branch`, and an ordinary title
/// ("Fix the login bug.") must not be able to fail the whole card → worktree flow.
#[must_use]
pub fn worktree_slug(board: &Board, card: &Card) -> String {
    let rendered = board
        .settings
        .branch_template
        .replace("{key}", &card.display_key(board))
        .replace("{slug}", &crate::slug::slugify(&card.title));
    let mut slug = branch_safe_slug(&rendered);
    if slug.is_empty() {
        slug = branch_safe_slug(&card.local_key(board));
    }
    if slug.is_empty() {
        slug = "card".into();
    }
    slug.truncate(48);
    let trimmed = slug.trim_matches(['-', '_']);
    if trimmed.is_empty() {
        "card".into()
    } else {
        trimmed.to_owned()
    }
}

/// Slugifies `value` and folds the dots a slug allows but a branch name cannot carry.
fn branch_safe_slug(value: &str) -> String {
    crate::slug::slugify(&crate::slug::slugify(value).replace('.', "-"))
}
/// Summarizes nonarchived cards for snapshots.
#[must_use]
pub fn summarize(board: &Board, cards: &[Card]) -> BoardSummary {
    let cards: Vec<_> = cards.iter().filter(|c| !c.archived).collect();
    BoardSummary {
        id: board.id.clone(),
        context_id: board.context_id.clone(),
        name: board.name.clone(),
        prefix: board.prefix.clone(),
        backend_kind: board.backend.kind.clone(),
        card_count: cards.len(),
        open_count: cards
            .iter()
            .filter(|c| {
                board.statuses.iter().any(|s| {
                    s.id == c.status_id
                        && !matches!(
                            s.category,
                            StatusCategory::Completed | StatusCategory::Canceled
                        )
                })
            })
            .count(),
        dirty_count: cards.iter().filter(|c| c.dirty).count(),
        conflict_count: cards.iter().filter(|c| c.conflict.is_some()).count(),
        last_synced_at: board.sync.last_synced_at.clone(),
        last_error: board.sync.last_error.clone(),
    }
}
