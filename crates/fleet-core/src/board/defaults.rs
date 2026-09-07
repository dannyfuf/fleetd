//! Defaults for a context's local board.
use super::model::*;
use crate::{ids::StatusId, model::Context};

/// Creates the five initial ordered status columns.
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
pub fn new_board(context: &Context, now: &str) -> Board {
    Board {
        id: context.id.clone().into(),
        context_id: context.id.clone(),
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
