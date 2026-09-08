//! Board settings (BOARD §8) — *the facts that change how the board behaves.*
//!
//! The rows follow §3.8.6's model exactly, which is why they answer the same keys: `j` / `k`
//! move, `space` toggles, `h` / `l` cycle a closed choice, and a text row simply types — a
//! bare `j` in the name field is the letter `j`, never a cursor move.
//!
//! # The backend rows are generic on purpose
//!
//! Nothing here knows what a Jira project key is. The **Backend** row cycles the kinds the
//! daemon registers (`ListBoardBackends`), and every row under it is one entry of that
//! descriptor's `settings_schema`: a `PropertyKind` decides whether the row is a text field, a
//! toggle, a number or a cycler, the schema's `key` is the JSON key written back into
//! `BackendRef.settings`, and the schema's `name` is what the row is called. A backend the
//! daemon adds tomorrow gets a complete settings form here with no change to `fleet-app` at
//! all — which is exactly the property `docs/BOARD-JIRA.md` §6 asks for.

use fleet_core::{
    board::{
        BackendRef, BoardPatch, BoardSettings, ConflictPolicy, PropertyKind, PropertyOption,
        PropertySchema,
    },
    ids::{BoardId, RepoId},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::prelude::*;
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{
        DialogHost, Dialogs, host::complete_request, notify, read_host, root, step, typed_char,
        with_host,
    },
    state::AppState,
};

/// How wide the row labels are.
const LABEL_WIDTH: f32 = 150.0;
/// The longest identifier prefix the contract allows.
const MAX_PREFIX: usize = 8;
/// What a multi-select settings row is typed as, and split back on.
const MULTI_SEPARATOR: char = ',';

mod draft;
mod persistence;
mod schema;
#[cfg(test)]
mod tests;
mod view;

pub(super) use draft::BoardSettingsState;
use draft::*;
pub(crate) use persistence::seed;
use persistence::*;
use schema::*;
pub(crate) use view::render;
