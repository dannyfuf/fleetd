//! §3.8.6 Settings (`,`) — a 180 px section rail beside a 540 px pane.

use fleet_core::{
    config::{Agent, CloneProtocol, Config},
    sleep::KeepAliveKind,
};
use fleet_proto::{
    request::RequestBody,
    response::{KeepAliveRuleMatch, ResponseBody},
};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, KeyDownEvent, Window, div, px};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{DialogHost, clear_all, notify, read_host, root, step, typed_char, with_host},
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
use schema::*;
pub(super) use schema::{Section, refresh_rows};
pub(crate) use view::render;
