//! Board settings (BOARD §8) — *the facts that change how the board behaves.*
//!
//! The dialog is the shared settings shell of §3.8.6 — a section rail, a pane of cards, one row
//! grammar — so it answers the same keys as Settings: `j` / `k` move, `space` flips a switch,
//! `h` / `l` step a closed choice, and `\u{23ce}` opens a text or number row's box in place
//! (`\u{23ce}` again keeps what was typed, `esc` puts the old value back). Landing on a row never
//! opens its editor, so a bare `j` always moves.
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
    agents::{AgentKind, PermissionMode},
    board::{
        Action, ActionKind, BackendRef, Board, BoardPatch, BoardSettings, ColumnAgentPrefs,
        ColumnAutomation, ConflictPolicy, MAX_LIVE_RUNS_PER_BOARD, PropertyKind, PropertyOption,
        PropertySchema, RunLocation, Status, StatusCategory, apply_workflow_preset,
        normalise_automation, validate_automation,
    },
    ids::{BoardId, CardId, RepoId, StatusId},
};
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};
use std::rc::Rc;

use crate::{
    actions::{board_settings as board_settings_actions, dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{
        DialogHost, Dialogs, footer, host::complete_request, notify, read_host, root, step,
        with_host,
    },
    screens::agent_thread::presentation::mode_label,
    state::AppState,
};

/// The longest identifier prefix the contract allows.
const MAX_PREFIX: usize = 8;
/// How many lines a multi-line editor opens at: the resting box's three (`value_area_h`), from
/// which it grows to [`VALUE_BOX_MAX_ROWS`] while it is being typed into.
const MULTILINE_MIN_ROWS: usize = 3;
/// What a multi-select settings row is typed as, and split back on.
const MULTI_SEPARATOR: char = ',';

/// What an empty backend text row suggests.
#[must_use]
fn input_placeholder(row: &BackendRow) -> &'static str {
    match row.kind {
        PropertyKind::MultiSelect => "comma, separated, values",
        PropertyKind::Date => "YYYY-MM-DD",
        _ if row.required => "required",
        _ => "optional",
    }
}

mod catalogue;
mod columns;
mod draft;
mod keys;
mod persistence;
mod pointer;
mod schedules;
mod schema;
#[cfg(test)]
mod tests;
mod view;

use catalogue::*;
use columns::*;
pub(super) use draft::BoardSettingsState;
use draft::*;
/// `C` opens this dialog on one named section; `,` opens it on the remembered one.
pub(crate) use keys::open_on_section;
use keys::*;
use persistence::*;
pub(crate) use persistence::{automation_locked, seed};
use pointer::*;
#[cfg(test)]
pub(crate) use schedules::requests::schedules_form_probe;
pub(crate) use schedules::requests::{
    load_board_schedules, load_schedules, open_schedules_section, refresh_stale_schedules,
    sync_open_list,
};
use schedules::*;
/// `T` and the Review board's empty state open the Schedules section; the event loop refreshes
/// the schedules mirror.
pub(crate) use schedules::{SCHEDULES_UNSUPPORTED, skipped_run_notice};
pub(crate) use schema::BoardSection;
use schema::*;
pub(crate) use view::render;
#[cfg(test)]
use view::reveal_delta;
