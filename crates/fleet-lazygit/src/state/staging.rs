use super::*;

/// Staging mode: a hunk/line selection over one side of one file's diff.
#[derive(Clone, Debug)]
pub(crate) struct Staging {
    /// The file being staged.
    pub(crate) path: PathBuf,
    /// Which side the selection applies to.
    pub(crate) side: DiffSide,
    /// The cursor row, an index into the flattened staging rows.
    pub(crate) cursor: usize,
    /// The anchor row of a range selection (`v`).
    pub(crate) anchor: Option<usize>,
    /// `false` selects whole hunks, `true` selects individual lines.
    pub(crate) line_mode: bool,
    /// Whether the cursor has already been placed on a change line.
    ///
    /// Snapping is a one-shot per open, side flip and mode toggle: the periodic file-diff
    /// re-read must not drag a selection the user is still moving back to the first change.
    pub(crate) snapped: bool,
}

/// The rows of the hunk the cursor sits in, as `(start, end)` indexes into `rows`.
///
/// Keyed by **(file, hunk)**, not by the hunk index alone: a diff with two `DiffFile`s numbers
/// its hunks from zero inside each file, so matching on the index alone would splice two files'
/// hunk 0 into one selection.
#[must_use]
pub(crate) fn hunk_range(rows: &[DiffRow], cursor: usize) -> Option<(usize, usize)> {
    let row = rows.get(cursor)?;
    let hunk = row.hunk?;
    let key = (row.file, hunk);
    let matches = |candidate: &DiffRow| {
        candidate.line.is_some() && (candidate.file, candidate.hunk) == (key.0, Some(key.1))
    };
    let start = rows.iter().position(matches)?;
    let end = rows.iter().rposition(matches)?;
    Some((start, end))
}

/// The `HunkSelection`s the rows `start..=end` describe, restricted to one file.
///
/// `fleet_git::PatchSelection` names exactly one path and its `hunk_index` is an index into that
/// file's hunks, so a range that happens to span two `DiffFile`s contributes only the rows of the
/// file the range starts in. The returned `Vec` is empty when nothing selectable was covered.
#[must_use]
pub(crate) fn selection_hunks(rows: &[DiffRow], start: usize, end: usize) -> Vec<HunkSelection> {
    let Some(slice) = rows.get(start..=end) else {
        return Vec::new();
    };
    let Some(file) = slice
        .iter()
        .find(|row| row.is_change() && row.hunk.is_some() && row.line.is_some())
        .map(|row| row.file)
    else {
        return Vec::new();
    };
    let mut hunks: Vec<HunkSelection> = Vec::new();
    for row in slice {
        let (Some(hunk_index), Some(line)) = (row.hunk, row.line) else {
            continue;
        };
        if row.file != file || !row.is_change() {
            continue;
        }
        match hunks
            .iter_mut()
            .find(|selection| selection.hunk_index == hunk_index)
        {
            Some(selection) => {
                if let Some(lines) = &mut selection.lines {
                    lines.push(line);
                }
            }
            None => hunks.push(HunkSelection {
                hunk_index,
                lines: Some(vec![line]),
            }),
        }
    }
    hunks
}
