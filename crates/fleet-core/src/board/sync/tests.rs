use super::*;
use crate::{
    board::{
        defaults::new_board,
        ops::{CardPatch, add_comment, apply_card_patch, validate_board},
        property::PropertyKind,
    },
    model::Context,
};

mod apply;
mod push;
mod reconcile;
mod schema;

const NOW: &str = "2026-09-06T12:00:00Z";
const LATER: &str = "2026-09-06T13:00:00Z";

fn board() -> Board {
    let mut board = new_board(
        &Context {
            id: "work".parse().unwrap(),
            name: "Fleet".into(),
            owners: vec![],
            created_at: NOW.into(),
        },
        NOW,
    );
    board.backend.kind = "fake".into();
    board
}

fn card(board: &mut Board, id: &str) -> Card {
    create_card(
        board,
        &[],
        id.parse().unwrap(),
        CardDraft {
            title: "Title".into(),
            ..CardDraft::default()
        },
        NOW,
    )
    .unwrap()
}

fn status(id: &str, name: &str, category: Option<StatusCategory>) -> RemoteStatus {
    RemoteStatus {
        id: id.into(),
        name: name.into(),
        category,
    }
}

fn remote(key: &str) -> RemoteCard {
    RemoteCard {
        key: key.into(),
        title: "Title".into(),
        version: Some("1".into()),
        updated_at: Some(NOW.into()),
        status: status("remote-todo", "Todo", Some(StatusCategory::Unstarted)),
        ..RemoteCard::default()
    }
}

fn linked(board: &mut Board) -> Card {
    let mut card = card(board, "a");
    apply_remote(board, &mut card, &remote("R-1"), NOW);
    card
}

fn caps() -> BackendCapabilities {
    BackendCapabilities {
        pull: true,
        push_updates: true,
        push_create: true,
        transitions: true,
        comments: true,
        custom_properties: true,
        incremental: true,
    }
}

fn pull(remote: RemoteCard) -> PullResult {
    PullResult {
        cards: vec![remote],
        ..PullResult::default()
    }
}

fn property(key: &str, source: PropertySource) -> PropertySchema {
    PropertySchema {
        key: key.into(),
        name: key.into(),
        kind: PropertyKind::Text,
        options: vec![],
        editable: true,
        source,
        show_on_card: false,
    }
}

fn comment(id: &str) -> RemoteComment {
    RemoteComment {
        id: id.into(),
        author: Some("A".into()),
        body: "Comment".into(),
        created_at: NOW.into(),
    }
}

fn conflict(card: &mut Card, remote: RemoteCard) {
    card.dirty = true;
    card.conflict = Some(Conflict {
        detected_at: NOW.into(),
        remote,
        fields: vec!["title".into()],
    });
}
