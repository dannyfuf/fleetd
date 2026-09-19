//! Defaults for a context's local board.

use super::model::{BackendRef, Board, BoardSettings, Status, StatusCategory, SyncState};
use crate::{
    ids::{BoardId, StatusId, WorktreeId},
    model::{Context, Worktree},
    slug::slugify,
};

const BOARD_ID_MAX_LEN: usize = 64;

/// Creates the five initial ordered status columns.
#[must_use]
pub fn default_statuses() -> Vec<Status> {
    [
        ("backlog", "Backlog", StatusCategory::Backlog),
        ("todo", "Todo", StatusCategory::Unstarted),
        ("in-progress", "In Progress", StatusCategory::Started),
        ("done", "Done", StatusCategory::Completed),
        ("canceled", "Canceled", StatusCategory::Canceled),
    ]
    .into_iter()
    .map(|(id, name, category)| Status {
        id: StatusId::try_from(id).expect("static status slug is valid"),
        name: name.into(),
        category,
        color: None,
    })
    .collect()
}

/// First three ASCII alphanumeric name characters, uppercased; FLT when absent.
#[must_use]
pub fn default_prefix(context: &Context) -> String {
    let prefix: String = context
        .name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(3)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if prefix.is_empty() {
        "FLT".into()
    } else {
        prefix
    }
}

/// Creates an empty local board with the context's identity and current timestamp.
#[must_use]
pub fn new_board(context: &Context, now: &str) -> Board {
    Board {
        id: context.id.clone().into(),
        context_id: context.id.clone(),
        worktree_id: None,
        name: context.name.clone(),
        prefix: default_prefix(context),
        next_number: 1,
        backend: BackendRef::default(),
        statuses: default_statuses(),
        labels: Vec::new(),
        properties: Vec::new(),
        default_repo_id: None,
        settings: BoardSettings::default(),
        sync: SyncState::default(),
        created_at: now.into(),
        updated_at: now.into(),
    }
}

/// Derives a stable board id from all three worktree-id parts.
#[must_use]
pub fn worktree_board_id(worktree: &WorktreeId) -> BoardId {
    let (owner, name) = worktree
        .repo()
        .split_once('/')
        .expect("a validated worktree id always contains an owner/name repository");
    let parts = [owner, name, worktree.slug()].map(board_id_part).join("-");
    let mut id = format!("wt-{parts}");
    id.truncate(BOARD_ID_MAX_LEN);
    while id.ends_with('-') {
        id.pop();
    }
    BoardId::try_from(id).expect("a derived worktree board id is always a valid board slug")
}

/// Creates an empty local board scoped to one worktree.
#[must_use]
pub fn new_worktree_board(context: &Context, worktree: &Worktree, now: &str) -> Board {
    let mut board = new_board(context, now);
    board.id = worktree_board_id(&worktree.id);
    board.worktree_id = Some(worktree.id.clone());
    board.name = worktree.slug.clone();
    board.prefix = worktree_prefix(&worktree.slug);
    board.default_repo_id = Some(worktree.repo_id.clone());
    board
}

fn board_id_part(value: &str) -> String {
    let part = slugify(&slugify(value).replace(['.', '_'], "-"));
    if part.is_empty() { "x".into() } else { part }
}

fn worktree_prefix(slug: &str) -> String {
    let prefix: String = slug
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(3)
        .map(|character| character.to_ascii_uppercase())
        .collect();
    if prefix.is_empty() {
        "WT".into()
    } else {
        prefix
    }
}
