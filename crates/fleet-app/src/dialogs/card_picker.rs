//! The one dialog every card property is edited through (BOARD §8).
//!
//! *Every property is a list you can type into.* Status, priority, assignee, labels, estimate,
//! due date, repository and every backend-declared custom property share one surface: a query
//! field over a [`FuzzyList`] of the values the field can take. The kinds that are not closed
//! sets — an assignee nobody has used yet, an estimate that is not on the Fibonacci ladder, a
//! date — accept the typed query itself as the value, which is why the field is an input and
//! not a menu.
//!
//! `Labels` and custom `MultiSelect` properties use multiple selection: `space` toggles the
//! highlighted label, `Enter` applies the whole set at once, because a picker that closes after
//! one label makes tagging a card four keystrokes per label.

use fleet_core::{
    board::{CardPatch, Priority, PropertyKind, PropertyValue},
    ids::{CardId, LabelId, RepoId, StatusId},
};
use fleet_proto::request::RequestBody;
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div};

use crate::{
    actions::{dialog, settings as settings_actions},
    bridge::Bridge,
    dialogs::{DialogHost, Dialogs, notify, read_host, root, step, type_into, with_host},
    screens::board,
    state::AppState,
};

/// The estimate ladder §8 offers before the typed value.
const ESTIMATES: [u32; 7] = [0, 1, 2, 3, 5, 8, 13];
/// How many rows the list shows at once.
const PICKER_ROWS: usize = 8;

mod draft;
mod lifecycle;
mod schema;
#[cfg(test)]
mod tests;
mod view;

use draft::*;
pub(crate) use draft::{CardPickerState, PickerKind};
pub(crate) use lifecycle::seed;
use lifecycle::*;
use schema::*;
pub(crate) use view::render;
