//! §3.8.6 Settings (`,`) — a section rail beside a pane of cards of setting rows, a search in
//! the header, and a footer that says whether anything is unsaved and asks once before `Esc`
//! discards it.

use fleet_core::{
    agents::AgentKind,
    config::{Agent, CloneProtocol, Config},
    sleep::KeepAliveKind,
};
use fleet_proto::{
    request::RequestBody,
    response::{KeepAliveRuleMatch, ResponseBody},
};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, AppContext, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{DialogHost, notify, read_host, root, step, with_host},
    presentation::{age_secs, now_unix},
    state::{AppState, Screen},
    views::workspace_tabs,
};

mod choice;
mod draft;
mod interaction;
mod persistence;

pub(crate) use persistence::editor_command;
mod schema;
#[cfg(test)]
mod tests;
mod view;

use choice::*;
pub(super) use draft::SettingsState;
use draft::*;
use interaction::*;
pub(crate) use persistence::seed;
use persistence::*;
pub(super) use schema::Section;
pub(crate) use schema::refresh_rows;
use schema::*;
pub(crate) use view::render;
