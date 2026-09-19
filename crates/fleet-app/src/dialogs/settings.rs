//! §3.8.6 Settings (`,`) — a 180 px section rail beside a 540 px pane.

use fleet_core::{
    agents::{AgentKind, PermissionMode},
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

/// The section rail's width (§3.8.6).
const RAIL_WIDTH: f32 = 180.0;

mod draft;
mod persistence;

pub(crate) use persistence::editor_command;
mod schema;
#[cfg(test)]
mod tests;
mod view;

pub(super) use draft::SettingsState;
use draft::*;
pub(crate) use persistence::seed;
use persistence::*;
pub(super) use schema::Section;
pub(crate) use schema::refresh_rows;
use schema::*;
pub(crate) use view::render;
