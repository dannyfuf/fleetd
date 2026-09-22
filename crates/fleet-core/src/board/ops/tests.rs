use super::*;
use crate::{
    board::{
        defaults::new_board,
        model::{Action, CardAgentPrefs, ColumnAgentPrefs, Conflict, RemoteLink},
        property::{PropertyKind, PropertySource},
    },
    ids::ContextId,
    model::Context,
};

mod automation;
mod cards;
mod patches;
mod query;
mod validation;

const NOW: &str = "2026-09-06T12:00:00Z";
const LATER: &str = "2026-09-06T13:00:00Z";

fn board() -> Board {
    new_board(
        &Context {
            id: ContextId::try_from("work").unwrap(),
            name: "Fleet".into(),
            owners: vec![],
            created_at: NOW.into(),
        },
        NOW,
    )
}

fn create(board: &mut Board, cards: &[Card], id: &str) -> Card {
    create_card(
        board,
        cards,
        id.parse().unwrap(),
        CardDraft {
            title: "Fix login".into(),
            ..CardDraft::default()
        },
        NOW,
    )
    .unwrap()
}

fn action(kind: ActionKind) -> Action {
    Action {
        kind,
        instructions: String::new(),
        expect: String::new(),
        agent: ColumnAgentPrefs::default(),
        env: Vec::new(),
    }
}

fn property(key: &str) -> PropertySchema {
    PropertySchema {
        key: key.into(),
        name: key.into(),
        kind: PropertyKind::Number,
        options: vec![],
        editable: true,
        source: PropertySource::Local,
        show_on_card: false,
    }
}
