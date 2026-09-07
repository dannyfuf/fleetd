//! Terminal geometry, selection, input routing, and wire-to-kit presentation shared by
//! both surfaces.

use fleet_proto::terminal::{
    Cell as ProtoCell, CellAttrs, CellWidth as ProtoWidth, Color, CursorShape as ProtoShape,
    TerminalModes,
};
use fleet_ui_kit::{
    CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection,
    TerminalMode as KitTerminalMode, Theme, UnderlineStyle,
};
use gpui::{Bounds, Hsla, IntoElement, Pixels, Point, Rgba, Size, canvas, prelude::*, px};

use crate::state::MirrorGrid;

mod geometry;
mod presentation;
mod selection;
pub(crate) mod surface;

pub(crate) use geometry::*;
pub(crate) use presentation::*;
pub(crate) use selection::*;

#[cfg(test)]
mod tests;
