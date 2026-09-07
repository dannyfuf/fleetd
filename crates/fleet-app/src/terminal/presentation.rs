use super::*;
use fleet_core::ids::TerminalId;
use fleet_ui_kit::{TerminalGridCache, theme::TerminalPalette};
use std::sync::Arc;

/// The VT modes worth badging above the grid.
///
/// `TerminalModes` is zero-suppressed, so a plain shell produces an empty list and no badges.
/// The Kitty keyboard flags and focus-event reporting have no badge: neither changes what a
/// documented Fleet key does, which is the bar the badge row is drawn to.
#[must_use]
pub(crate) fn grid_modes(modes: &TerminalModes) -> Vec<KitTerminalMode> {
    let mut active = Vec::with_capacity(4);
    if modes.alt_screen {
        active.push(KitTerminalMode::AltScreen);
    }
    if modes.mouse_reporting {
        active.push(KitTerminalMode::MouseReporting);
    }
    if modes.bracketed_paste {
        active.push(KitTerminalMode::BracketedPaste);
    }
    if modes.app_cursor_keys {
        active.push(KitTerminalMode::ApplicationCursor);
    }
    active
}

/// Resolves a foreground color against the theme's terminal palette.
#[must_use]
pub(crate) fn foreground(color: Color, theme: &Theme) -> Hsla {
    match color {
        Color::Default => theme.terminal.foreground,
        Color::Palette(index) => theme.terminal.color(index),
        Color::Rgb { r, g, b } => rgb(r, g, b),
    }
}

/// Resolves a background color, mapping `Default` to "paint nothing" as the kit expects.
#[must_use]
pub(crate) fn background(color: Color, theme: &Theme) -> Option<Hsla> {
    match color {
        Color::Default => None,
        Color::Palette(index) => Some(theme.terminal.color(index)),
        Color::Rgb { r, g, b } => Some(rgb(r, g, b)),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    Hsla::from(Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    })
}

/// The underline style the three mutually exclusive underline flags encode.
#[must_use]
pub(crate) fn underline_style(attrs: CellAttrs) -> UnderlineStyle {
    if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
        UnderlineStyle::Curly
    } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if attrs.contains(CellAttrs::UNDERLINE) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    }
}

/// Converts one wire cell into the kit's painted cell.
#[must_use]
pub(crate) fn grid_cell(cell: &ProtoCell, theme: &Theme) -> GridCell {
    let attrs = cell.attrs;
    GridCell {
        text: gpui::SharedString::new(cell.text.as_str()),
        fg: foreground(cell.fg, theme),
        bg: background(cell.bg, theme),
        bold: attrs.contains(CellAttrs::BOLD),
        dim: attrs.contains(CellAttrs::DIM),
        italic: attrs.contains(CellAttrs::ITALIC),
        underline: underline_style(attrs),
        underline_color: cell
            .underline_color
            .and_then(|color| background(color, theme)),
        strikethrough: attrs.contains(CellAttrs::STRIKETHROUGH),
        inverse: attrs.contains(CellAttrs::INVERSE),
        blink: attrs.contains(CellAttrs::BLINK),
        invisible: attrs.contains(CellAttrs::INVISIBLE),
        width: match cell.width {
            ProtoWidth::Narrow => CellWidth::Narrow,
            ProtoWidth::Wide => CellWidth::Wide,
            ProtoWidth::Spacer => CellWidth::Spacer,
        },
    }
}

/// Converts the whole mirror into painted rows.
///
/// A row the daemon has never sent is painted as an empty row rather than as blanks: the kit
/// lays rows out at a fixed cell height, so an empty row already occupies the right space and
/// costs no text shaping.
#[must_use]
pub(crate) fn grid_rows(grid: &MirrorGrid, theme: &Theme) -> Vec<GridRow> {
    grid.lines
        .iter()
        .map(|line| GridRow::new(line.iter().map(|cell| grid_cell(cell, theme))))
        .collect()
}

/// The kit cursor for a mirror grid.
///
/// `focused` is the app's own focus, not the PTY's: an unfocused terminal draws the hollow
/// cursor so a window that is not taking keys cannot look like one that is.
#[must_use]
pub(crate) fn grid_cursor(grid: &MirrorGrid, focused: bool) -> GridCursor {
    GridCursor {
        row: grid.cursor.row as usize,
        col: grid.cursor.col as usize,
        visible: grid.cursor.visible,
        shape: if focused {
            match grid.cursor.shape {
                ProtoShape::Block => CursorShape::Block,
                ProtoShape::Bar => CursorShape::Bar,
                ProtoShape::Underline => CursorShape::Underline,
            }
        } else {
            CursorShape::Hollow
        },
    }
}

/// A change key for one wire row: FNV-1a over the bytes the row contributes to its painted
/// form.
///
/// The mirror exposes a frame sequence but no per-row revision, so a changed frame has to say
/// which rows moved. Destructuring every cell field keeps a new [`ProtoCell`] field from
/// silently falling out of the key.
fn row_digest(line: &[ProtoCell]) -> u64 {
    fn eat(hash: u64, bytes: &[u8]) -> u64 {
        bytes.iter().fold(hash, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }
    fn eat_color(hash: u64, color: Option<Color>) -> u64 {
        match color {
            None => eat(hash, &[0]),
            Some(Color::Default) => eat(hash, &[1]),
            Some(Color::Palette(index)) => eat(hash, &[2, index]),
            Some(Color::Rgb { r, g, b }) => eat(hash, &[3, r, g, b]),
        }
    }
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for cell in line {
        let ProtoCell {
            text,
            fg,
            bg,
            underline_color,
            attrs,
            width,
        } = cell;
        hash = eat(hash, text.as_bytes());
        hash = eat_color(eat(hash, &[0xff]), Some(*fg));
        hash = eat_color(hash, Some(*bg));
        hash = eat_color(hash, *underline_color);
        hash = eat(hash, &attrs.bits().to_le_bytes());
        hash = eat(hash, &[*width as u8]);
    }
    hash
}

/// Retains the kit snapshot across cursor/selection frames, converting only the rows whose wire
/// cells changed since the last published frame.
#[derive(Default)]
pub(crate) struct TerminalPresentation {
    identity: Option<(Option<TerminalId>, u16, u16, u64)>,
    palette: Option<TerminalPalette>,
    sequence: Option<u64>,
    /// One change key per published row — the grid itself is never copied.
    digests: Vec<u64>,
    rows: Arc<[GridRow]>,
    pub(crate) paint: TerminalGridCache,
}

impl TerminalPresentation {
    pub(crate) fn snapshot(&self) -> Arc<[GridRow]> {
        Arc::clone(&self.rows)
    }

    /// Republishes the snapshot for `grid`, converting only the rows whose wire cells changed.
    pub(crate) fn update(
        &mut self,
        terminal: Option<TerminalId>,
        grid: &MirrorGrid,
        theme: &Theme,
    ) {
        let identity = (terminal, grid.cols, grid.rows, grid.viewport.history_epoch);
        let reset = self.identity != Some(identity) || self.palette != Some(theme.terminal);
        if !reset && self.sequence == Some(grid.seq) {
            return;
        }
        let mut converted = 0;
        if reset || self.digests.len() != grid.lines.len() {
            self.rows = grid_rows(grid, theme).into();
            self.digests = grid.lines.iter().map(|line| row_digest(line)).collect();
            converted = grid.lines.len();
        } else {
            for (index, line) in grid.lines.iter().enumerate() {
                let digest = row_digest(line);
                if self.digests[index] != digest {
                    Arc::make_mut(&mut self.rows)[index] =
                        GridRow::new(line.iter().map(|cell| grid_cell(cell, theme)));
                    self.digests[index] = digest;
                    converted += 1;
                }
            }
        }
        self.identity = Some(identity);
        self.palette = Some(theme.terminal);
        self.sequence = Some(grid.seq);
        tracing::trace!(
            terminal = ?terminal,
            converted_rows = converted,
            rows = grid.rows,
            "terminal presentation updated"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published(
        cache: &mut TerminalPresentation,
        terminal: TerminalId,
        grid: &MirrorGrid,
        theme: &Theme,
    ) -> Arc<[GridRow]> {
        cache.update(Some(terminal), grid, theme);
        cache.snapshot()
    }

    fn mirror(cols: u16, rows: u16) -> MirrorGrid {
        let mut grid = MirrorGrid::new(cols, rows);
        let cell = ProtoCell {
            text: "x".into(),
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: CellAttrs::empty(),
            width: ProtoWidth::Narrow,
        };
        grid.lines = vec![vec![cell; usize::from(cols)]; usize::from(rows)];
        grid.primed = true;
        grid.seq = 1;
        grid
    }

    #[test]
    fn cursor_only_frames_reuse_all_twelve_thousand_cells() {
        let mut grid = mirror(200, 60);
        let theme = Theme::for_mode(fleet_ui_kit::ThemeMode::Dark);
        let mut cache = TerminalPresentation::default();
        let first = published(&mut cache, TerminalId(1), &grid, &theme);
        for seq in 2..=120 {
            grid.seq = seq;
            grid.cursor.col = (seq % 200) as u16;
            grid.cursor.visible = seq % 2 == 0;
            let next = published(&mut cache, TerminalId(1), &grid, &theme);
            assert!(Arc::ptr_eq(&first, &next));
        }
        assert_eq!(first.len(), 60);
        assert_eq!(
            first.iter().map(|row| row.cells.len()).sum::<usize>(),
            12_000
        );
    }

    #[test]
    fn dirty_rows_publish_new_snapshots_without_mutating_previous_frames() {
        let mut grid = mirror(200, 60);
        let theme = Theme::for_mode(fleet_ui_kit::ThemeMode::Dark);
        let mut cache = TerminalPresentation::default();
        let first = published(&mut cache, TerminalId(1), &grid, &theme);
        for row in 0..60 {
            grid.seq += 1;
            grid.lines[row][0].text = "y".into();
            let current = published(&mut cache, TerminalId(1), &grid, &theme);
            assert_eq!(current.as_ref(), grid_rows(&grid, &theme));
            assert_eq!(first[row].cells[0].text.as_ref(), "x");
            assert!(Arc::ptr_eq(
                &current,
                &published(&mut cache, TerminalId(1), &grid, &theme)
            ));
        }
    }

    #[test]
    fn identity_dimensions_history_and_palette_invalidate_cached_rows() {
        let mut grid = mirror(4, 2);
        let mut theme = Theme::for_mode(fleet_ui_kit::ThemeMode::Dark);
        let mut cache = TerminalPresentation::default();
        let first = published(&mut cache, TerminalId(1), &grid, &theme);
        let other = published(&mut cache, TerminalId(2), &grid, &theme);
        assert!(!Arc::ptr_eq(&first, &other));
        grid.viewport.history_epoch += 1;
        let epoch = published(&mut cache, TerminalId(2), &grid, &theme);
        assert!(!Arc::ptr_eq(&other, &epoch));
        grid.cols = 5;
        let resized = published(&mut cache, TerminalId(2), &grid, &theme);
        assert!(!Arc::ptr_eq(&epoch, &resized));
        theme = Theme::for_mode(fleet_ui_kit::ThemeMode::Light);
        let light = published(&mut cache, TerminalId(2), &grid, &theme);
        assert!(!Arc::ptr_eq(&resized, &light));
        assert_eq!(light[0].cells[0].fg, theme.terminal.foreground);
        assert_ne!(resized[0].cells[0].fg, light[0].cells[0].fg);
    }
}
