//! Card detail (BOARD §8) — *the card as prose on the left, as facts on the right.*
//!
//! Three text surfaces (title, description, one new comment) share a single buffer, because at
//! most one of them is ever being edited: `i`, `d` and `c` open it, `ctrl-s` saves it and `Esc`
//! throws it away. While it is open the bare letters type — `d` in a description is the letter
//! `d`, not "edit description" — which is the same rule §3.8.6 applies to its text rows.
//!
//! Nothing here is optimistic. Every save sends its request and waits: the reducer applies the
//! `Card` the daemon returns, and a refusal becomes a sticky line at the top of the dialog
//! rather than a change the user believes happened.

use fleet_core::{
    board::{Card, CardPatch, ConflictResolution},
    ids::CardId,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::{card_detail as card_actions, dialog},
    bridge::Bridge,
    dialogs::{
        DialogHost, Dialogs, host::complete_request, notify, read_host, root, typed_char, with_host,
    },
    presentation::now_unix,
    screens::board,
    state::AppState,
    views::board_card_detail::{self as detail, PropertyTarget},
};

/// The width of the right-hand property pane.
const PROPERTIES_WIDTH: f32 = 260.0;
/// The dialog's fixed height: the left pane scrolls inside it.
const DETAIL_HEIGHT: f32 = 620.0;

mod actions;
mod draft;
mod lifecycle;
#[cfg(test)]
mod tests;
mod view;

use actions::*;
pub(crate) use actions::{
    add_comment, close, create_worktree, edit_description, edit_property, edit_title, keep_local,
    next_property, open_remote, prev_property, save, take_remote,
};
pub(crate) use draft::CardDetailState;
use draft::*;
pub(crate) use lifecycle::seed;
use lifecycle::*;
pub(crate) use view::render;
