use super::*;

/// Fractional wheel rows for the terminal currently under the pointer.
#[derive(Debug, Default)]
pub(crate) struct WheelAccumulator {
    terminal: Option<fleet_core::ids::TerminalId>,
    pub(super) remainder: f64,
}

impl WheelAccumulator {
    /// Reset when the pointer enters a different terminal, even without a wheel event.
    pub(crate) fn point_at(&mut self, terminal: fleet_core::ids::TerminalId) {
        self.reconcile(Some(terminal));
    }

    /// Reset when the active terminal changes, including an empty workspace.
    pub(crate) fn reconcile(&mut self, terminal: Option<fleet_core::ids::TerminalId>) {
        if self.terminal != terminal {
            self.terminal = terminal;
            self.remainder = 0.0;
        }
    }

    /// Convert GPUI content movement into signed whole terminal rows.
    pub(crate) fn steps(
        &mut self,
        terminal: fleet_core::ids::TerminalId,
        delta: gpui::ScrollDelta,
        cell_height: Pixels,
        lines_per_step: u32,
        phase: gpui::TouchPhase,
    ) -> i32 {
        self.point_at(terminal);
        let rows = match delta {
            gpui::ScrollDelta::Pixels(delta) => {
                f64::from(f32::from(delta.y)) / f64::from(f32::from(cell_height))
            }
            gpui::ScrollDelta::Lines(delta) => f64::from(delta.y) * f64::from(lines_per_step),
        };
        // GPUI preserves AppKit scrollingDeltaY and adds it to the content origin:
        // positive content motion is UP into Fleet history, hence the minus sign.
        if rows.is_finite() {
            self.remainder -= rows;
        }
        let steps = self.remainder.trunc() as i32;
        self.remainder = self.remainder.fract();
        if matches!(phase, gpui::TouchPhase::Ended | gpui::TouchPhase::Cancelled) {
            self.remainder = 0.0;
        }
        steps
    }
}

/// The smallest grid Fleet ever asks a PTY for.
///
/// A zero-sized PTY is not a valid `TIOCSWINSZ` argument and a one-column one makes every
/// full-screen app misbehave, so a window squeezed below the minimum still gets a usable shell.
pub(crate) const MIN_COLS: u16 = 2;
/// The smallest row count Fleet ever asks a PTY for. See [`MIN_COLS`].
pub(crate) const MIN_ROWS: u16 = 1;

/// The grid size that fits `area`, in cells.
///
/// `cell` is `theme.metrics.cell_w` × `theme.metrics.cell_h`. The result is clamped to
/// [`MIN_COLS`] × [`MIN_ROWS`] so a collapsed window never asks the daemon for an empty PTY.
#[must_use]
pub(crate) fn grid_size(area: Size<Pixels>, cell: Size<Pixels>, padding: Pixels) -> (u16, u16) {
    let usable = |extent: Pixels| f32::from(extent) - 2.0 * f32::from(padding);
    let fit = |extent: f32, unit: Pixels| {
        let unit = f32::from(unit);
        if unit <= 0.0 || !extent.is_finite() || extent <= 0.0 {
            return 0;
        }
        // `as` saturates at u16::MAX for a NaN-free positive float, and the floor is what fits.
        (extent / unit).floor().max(0.0).min(f32::from(u16::MAX)) as u16
    };
    (
        fit(usable(area.width), cell.width).max(MIN_COLS),
        fit(usable(area.height), cell.height).max(MIN_ROWS),
    )
}

/// An invisible element that reports the pixel bounds it was laid out into.
///
/// The Workspace fills the terminal area with it and turns the reported size into a
/// `ResizeTerminal` request. It draws nothing: only the layout pass is interesting.
pub(crate) fn measure(report: impl 'static + FnOnce(Bounds<Pixels>)) -> impl IntoElement {
    canvas(
        move |bounds, _window, _cx| report(bounds),
        |_bounds, (), _window, _cx| {},
    )
    .absolute()
    .size_full()
}

/// The viewport cell under a window position, clamped to the visible terminal grid.
///
/// `bounds` includes the grid's padding. The returned point therefore subtracts
/// `padding` before dividing by the measured cell size. Clamping lets a drag that ends
/// in the padding still select the first or last cell instead of losing the mouse-up event.
#[must_use]
pub(crate) fn cell_at_position(
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
    cell: Size<Pixels>,
    cols: u16,
    rows: u16,
    padding: Pixels,
) -> Option<CellPoint> {
    if cols == 0
        || rows == 0
        || bounds.size.width <= px(0.0)
        || bounds.size.height <= px(0.0)
        || cell.width <= px(0.0)
        || cell.height <= px(0.0)
    {
        return None;
    }
    let x = f32::from(position.x - bounds.origin.x) - f32::from(padding);
    let y = f32::from(position.y - bounds.origin.y) - f32::from(padding);
    let col = (x / f32::from(cell.width)).floor() as isize;
    let row = (y / f32::from(cell.height)).floor() as isize;
    Some(CellPoint::new(
        usize::try_from(row.clamp(0, isize::try_from(rows - 1).unwrap_or(isize::MAX))).unwrap_or(0),
        usize::try_from(col.clamp(0, isize::try_from(cols - 1).unwrap_or(isize::MAX))).unwrap_or(0),
    ))
}

/// The pixel size of one cell, from the theme metrics.
#[must_use]
pub(crate) fn cell_size(theme: &Theme) -> Size<Pixels> {
    gpui::size(theme.metrics.cell_w, theme.metrics.cell_h)
}

/// The 2 px amber reminder that `ctrl-s z` hid the header and the tab strip (§3.6).
pub(crate) fn zoom_bar(theme: &Theme) -> impl IntoElement {
    gpui::div()
        .h(theme.metrics.focus_ring_w)
        .w_full()
        .flex_none()
        .bg(theme.colors.warning)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MouseCell {
    pub(crate) viewport: CellPoint,
    pub(crate) absolute: AbsoluteCellPoint,
    pub(crate) cols: u16,
    pub(crate) alt_screen: bool,
    pub(crate) history_epoch: u64,
}

impl MouseCell {
    pub(crate) fn at(
        grid: &MirrorGrid,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        metrics: fleet_ui_kit::CellMetrics,
        padding: Pixels,
    ) -> Option<Self> {
        let viewport = cell_at_position(
            bounds,
            position,
            gpui::size(metrics.width, metrics.height),
            grid.cols,
            grid.rows,
            padding,
        )?;
        Some(Self {
            viewport,
            absolute: AbsoluteCellPoint::new(
                viewport_base(grid) + viewport.row as u64,
                viewport.col,
            ),
            cols: grid.cols,
            alt_screen: grid.modes.alt_screen,
            history_epoch: grid.viewport.history_epoch,
        })
    }
}
