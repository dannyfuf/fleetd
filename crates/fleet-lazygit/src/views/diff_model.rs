//! Prepared diff rows preserve unified `(file, hunk, line)` staging coordinates.
//! Split rows index those same rows; all rows have one uniform height.

use gpui::SharedString;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fleet_git::{Diff, DiffKind, LineKind};

use super::intraline;
use super::syntax::{self, Runs};

/// Which layout the model was flattened for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum DiffViewMode {
    /// One column, `-` then `+`. lazygit's shape, and the one the staging cursor models.
    #[default]
    Unified,
    /// Old on the left, new on the right, aligned with filler rows.
    Split,
}

impl DiffViewMode {
    /// The other mode.
    #[must_use]
    pub(crate) fn toggled(self) -> Self {
        match self {
            DiffViewMode::Unified => DiffViewMode::Split,
            DiffViewMode::Split => DiffViewMode::Unified,
        }
    }
}

/// What a rendered diff row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowKind {
    /// The top half of a file-header card.
    FileHeader,
    /// The bottom half of a file-header card. Two rows, because `uniform_list` needs one height.
    FileHeaderFoot,
    /// An `@@ … @@` separator, with its expand affordance.
    HunkHeader,
    /// An added line.
    Added,
    /// A removed line.
    Removed,
    /// An unchanged line.
    Context,
    /// A file-level note: binary, mode-only change, empty rename.
    Note,
    /// `\ No newline at end of file` and friends.
    Other,
}

/// One rendered row of a diff, plus the coordinates a patch selection needs.
#[derive(Clone, Debug)]
pub(crate) struct DiffRow {
    /// What kind of line this is.
    pub(crate) kind: RowKind,
    /// The line's text, without the `+`/`-` marker and with tabs already expanded.
    pub(crate) text: SharedString,
    /// Line number on the old side.
    pub(crate) old_no: Option<u32>,
    /// Line number on the new side.
    pub(crate) new_no: Option<u32>,
    /// Index into `Diff::files`.
    pub(crate) file: usize,
    /// Index into `DiffFile::hunks`, for every row inside a hunk.
    pub(crate) hunk: Option<usize>,
    /// Index into `Hunk::lines`, for payload rows only.
    pub(crate) line: Option<usize>,
    /// Byte ranges of the words that changed, relative to [`DiffRow::text`].
    pub(crate) words: Vec<Range<usize>>,
}

impl DiffRow {
    /// Whether the row is an addition or a removal, i.e. selectable for staging.
    #[must_use]
    pub(crate) fn is_change(&self) -> bool {
        matches!(self.kind, RowKind::Added | RowKind::Removed)
    }

    fn blank(kind: RowKind, file: usize, text: String) -> Self {
        Self {
            kind,
            text: text.into(),
            old_no: None,
            new_no: None,
            file,
            hunk: None,
            line: None,
            words: Vec::new(),
        }
    }
}

/// One row of the split layout: which unified rows go in the left and right columns.
///
/// `None` on a side is Zed's `Block::Spacer` — alignment filler, drawn hatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SplitRow {
    /// The unified row shown in the old-side column.
    pub(crate) left: Option<usize>,
    /// The unified row shown in the new-side column.
    pub(crate) right: Option<usize>,
    /// Whether the row spans both columns: file headers, hunk separators and notes.
    pub(crate) full: bool,
}

/// Everything a file-header card shows, computed once.
#[derive(Clone, Debug)]
pub(crate) struct FileMeta {
    /// The path as displayed: the new path, or the old one for a deletion.
    pub(crate) path: String,
    /// The previous path, for a rename or copy.
    pub(crate) old_path: Option<String>,
    /// The file-level change kind.
    pub(crate) kind: DiffKind,
    /// Whether git called it binary.
    pub(crate) binary: bool,
    /// How many lines this file adds. Not in `fleet_git`; counted here.
    pub(crate) added: u32,
    /// How many lines this file removes.
    pub(crate) removed: u32,
    /// `100644 → 100755`, when the mode changed.
    pub(crate) mode: Option<String>,
    /// The grammar name for the payload lines, when one is recognised.
    pub(crate) language: Option<String>,
}

/// Allocation identity is valid while the owner retains the source Arc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelKey {
    diff: usize,
    mode: DiffViewMode,
}

impl ModelKey {
    pub(crate) fn new(diff: &Arc<Diff>, mode: DiffViewMode) -> Self {
        Self {
            diff: Arc::as_ptr(diff) as usize,
            mode,
        }
    }
}

/// How wide a tab expands to. Two, matching Pierre's `--diffs-tab-size`.
pub(crate) const TAB_WIDTH: usize = 2;

/// The rendered model of one patch.
#[derive(Debug)]
pub(crate) struct DiffModel {
    pub(crate) long_lines: std::collections::HashMap<usize, Arc<super::long_line::LongLine>>,
    /// One row per rendered line, in unified order. Always built, in both modes.
    pub(crate) rows: Vec<DiffRow>,
    /// The split layout, indexing [`DiffModel::rows`]. Empty in unified mode.
    pub(crate) split: Vec<SplitRow>,
    /// One entry per `Diff::files`.
    pub(crate) files: Vec<FileMeta>,
    /// Which layout this model was flattened for.
    pub(crate) mode: DiffViewMode,
    /// Fallback payload widths in scalar columns, one per rendered code column.
    pub(crate) payload_columns: [usize; 2],
    /// Font-measured payload advances, filled on the first render of this model.
    pub(crate) payload_advances: Cell<Option<[f32; 2]>>,
    /// How many characters one line-number gutter needs.
    pub(crate) digits: usize,
    /// Syntax runs, filled in off the foreground thread.
    syntax: RefCell<Vec<Runs>>,
}

impl DiffModel {
    /// An empty model, for "no changes to show".
    #[must_use]
    pub(crate) fn empty(mode: DiffViewMode) -> Self {
        Self {
            long_lines: Default::default(),
            rows: Vec::new(),
            split: Vec::new(),
            files: Vec::new(),
            mode,
            payload_columns: [0; 2],
            payload_advances: Cell::new(None),
            digits: 1,
            syntax: RefCell::new(Vec::new()),
        }
    }

    /// Flattens a patch.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn build(diff: &Diff, mode: DiffViewMode) -> Self {
        Self::build_cancellable(diff, mode, &AtomicBool::new(false))
            .unwrap_or_else(|| Self::empty(mode))
    }

    pub(crate) fn build_cancellable(
        diff: &Diff,
        mode: DiffViewMode,
        cancelled: &AtomicBool,
    ) -> Option<Self> {
        let mut rows: Vec<DiffRow> = Vec::new();
        let mut files: Vec<FileMeta> = Vec::new();
        let mut split: Vec<SplitRow> = Vec::new();
        let mut highest = 0u32;

        for (file_index, file) in diff.files.iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            files.push(meta(file));
            let header = rows.len();
            rows.push(DiffRow::blank(
                RowKind::FileHeader,
                file_index,
                files[file_index].path.clone(),
            ));
            rows.push(DiffRow::blank(
                RowKind::FileHeaderFoot,
                file_index,
                String::new(),
            ));
            if mode == DiffViewMode::Split {
                split.push(SplitRow {
                    left: Some(header),
                    right: None,
                    full: true,
                });
                split.push(SplitRow {
                    left: Some(header + 1),
                    right: None,
                    full: true,
                });
            }

            if file.binary {
                push_full(&mut rows, &mut split, mode, |rows| {
                    rows.push(DiffRow::blank(
                        RowKind::Note,
                        file_index,
                        "Binary file differs — stage the whole file".to_owned(),
                    ));
                });
                continue;
            }
            if file.hunks.is_empty() {
                let note = match (&files[file_index].mode, files[file_index].kind) {
                    (Some(mode), _) => format!("Mode changed: {mode}"),
                    (None, DiffKind::Renamed | DiffKind::Copied) => {
                        "Renamed with no content change".to_owned()
                    }
                    _ => "No content change".to_owned(),
                };
                push_full(&mut rows, &mut split, mode, |rows| {
                    rows.push(DiffRow::blank(RowKind::Note, file_index, note));
                });
                continue;
            }

            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                if cancelled.load(Ordering::Relaxed) {
                    return None;
                }
                let text = format!(
                    "@@ -{},{} +{},{} @@{}",
                    hunk.old.start,
                    hunk.old.count,
                    hunk.new.start,
                    hunk.new.count,
                    String::from_utf8_lossy(&hunk.header)
                );
                let separator = rows.len();
                rows.push(DiffRow {
                    kind: RowKind::HunkHeader,
                    text: text.into(),
                    old_no: None,
                    new_no: None,
                    file: file_index,
                    hunk: Some(hunk_index),
                    line: None,
                    words: Vec::new(),
                });
                if mode == DiffViewMode::Split {
                    split.push(SplitRow {
                        left: Some(separator),
                        right: None,
                        full: true,
                    });
                }

                let first_line = rows.len();
                for (line_index, line) in hunk.lines.iter().enumerate() {
                    if line_index % 256 == 0 && cancelled.load(Ordering::Relaxed) {
                        return None;
                    }
                    highest = highest
                        .max(line.old_no.unwrap_or(0))
                        .max(line.new_no.unwrap_or(0));
                    rows.push(DiffRow {
                        kind: match line.kind {
                            LineKind::Added => RowKind::Added,
                            LineKind::Removed => RowKind::Removed,
                            LineKind::Context => RowKind::Context,
                            LineKind::NoNewline | LineKind::Other => RowKind::Other,
                        },
                        text: expand_tabs(&String::from_utf8_lossy(&line.content)).into(),
                        old_no: line.old_no,
                        new_no: line.new_no,
                        file: file_index,
                        hunk: Some(hunk_index),
                        line: Some(line_index),
                        words: Vec::new(),
                    });
                }

                // Word-level marks: block pairing, then `similar` inside each pair.
                let blocks = intraline::change_blocks(hunk);
                for block in &blocks {
                    if !block.pairable() {
                        continue;
                    }
                    for (old_line, new_line) in block.pairs() {
                        if cancelled.load(Ordering::Relaxed) {
                            return None;
                        }
                        let old_row = first_line + old_line;
                        let new_row = first_line + new_line;
                        let Some((removed, added)) =
                            intraline::word_spans(&rows[old_row].text, &rows[new_row].text)
                        else {
                            continue;
                        };
                        rows[old_row].words = removed;
                        rows[new_row].words = added;
                    }
                }

                if mode == DiffViewMode::Split {
                    layout_split(&mut split, hunk, first_line, &blocks);
                }
            }
        }

        let payload_columns = payload_columns(&rows, &split, mode);
        let digits = highest.to_string().len().max(2);
        let count = rows.len();
        Some(Self {
            long_lines: Default::default(),
            rows,
            split: if mode == DiffViewMode::Split {
                split
            } else {
                Vec::new()
            },
            files,
            mode,
            payload_columns,
            payload_advances: Cell::new(None),
            digits,
            syntax: RefCell::new(vec![Vec::new(); count]),
        })
    }

    /// How many rows the list shows in this model's mode.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        match self.mode {
            DiffViewMode::Unified => self.rows.len(),
            DiffViewMode::Split => self.split.len(),
        }
    }

    /// Whether there is nothing to render.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The syntax runs for one unified row, or an empty slice while the pass is in flight.
    #[must_use]
    pub(crate) fn runs_for(
        &self,
        row: usize,
    ) -> std::cell::Ref<'_, [(Range<usize>, syntax::Bucket)]> {
        std::cell::Ref::map(self.syntax.borrow(), |syntax| {
            syntax.get(row).map(Vec::as_slice).unwrap_or_default()
        })
    }

    /// The background highlighting jobs for this model: one per side per hunk.
    ///
    /// A hunk's `-` and `+` lines are two different versions of the file, so they are handed to
    /// two separate parsers; context lines belong to both and are parsed twice, with the new
    /// side's answer winning in [`DiffModel::apply_syntax`].
    #[must_use]
    pub(crate) fn syntax_jobs(&self) -> Vec<syntax::Job> {
        let mut jobs: Vec<syntax::Job> = Vec::new();
        let mut old_side: Vec<(usize, SharedString)> = Vec::new();
        let mut new_side: Vec<(usize, SharedString)> = Vec::new();
        let mut language: Option<String> = None;
        let flush = |jobs: &mut Vec<syntax::Job>,
                     old_side: &mut Vec<(usize, SharedString)>,
                     new_side: &mut Vec<(usize, SharedString)>,
                     language: &Option<String>| {
            if let Some(language) = language {
                for lines in [std::mem::take(old_side), std::mem::take(new_side)] {
                    if !lines.is_empty() {
                        jobs.push(syntax::Job {
                            language: language.clone(),
                            lines,
                        });
                    }
                }
            } else {
                old_side.clear();
                new_side.clear();
            }
        };

        let mut count = 0;
        for (index, row) in self.rows.iter().enumerate() {
            if count >= syntax::MAX_LINES {
                break;
            }
            count += match row.kind {
                RowKind::Added | RowKind::Removed => 1,
                RowKind::Context => 2,
                _ => 0,
            };
            match row.kind {
                RowKind::FileHeader => {
                    flush(&mut jobs, &mut old_side, &mut new_side, &language);
                    language = self
                        .files
                        .get(row.file)
                        .and_then(|file| file.language.clone());
                }
                RowKind::HunkHeader => {
                    flush(&mut jobs, &mut old_side, &mut new_side, &language);
                }
                RowKind::Added => new_side.push((index, row.text.clone())),
                RowKind::Removed => old_side.push((index, row.text.clone())),
                RowKind::Context => {
                    old_side.push((index, row.text.clone()));
                    new_side.push((index, row.text.clone()));
                }
                RowKind::FileHeaderFoot | RowKind::Note | RowKind::Other => {}
            }
        }
        flush(&mut jobs, &mut old_side, &mut new_side, &language);
        jobs
    }

    /// Installs the result of a background pass.
    pub(crate) fn apply_syntax(&self, runs: Vec<(usize, Runs)>) {
        let mut syntax = self.syntax.borrow_mut();
        for (row, line) in runs {
            if let Some(slot) = syntax.get_mut(row) {
                *slot = line;
            }
        }
    }
}

pub(crate) fn is_panned_payload(kind: RowKind) -> bool {
    matches!(
        kind,
        RowKind::Added | RowKind::Removed | RowKind::Context | RowKind::Other
    )
}

fn payload_columns(rows: &[DiffRow], split: &[SplitRow], mode: DiffViewMode) -> [usize; 2] {
    let width = |index: usize| {
        rows.get(index)
            .filter(|row| is_panned_payload(row.kind))
            .map_or(0, |row| row.text.chars().count())
    };
    match mode {
        DiffViewMode::Unified => [
            rows.iter()
                .filter(|row| is_panned_payload(row.kind))
                .map(|row| row.text.chars().count())
                .max()
                .unwrap_or(0),
            0,
        ],
        DiffViewMode::Split => {
            split
                .iter()
                .filter(|layout| !layout.full)
                .fold([0, 0], |mut widest, layout| {
                    widest[0] = widest[0].max(layout.left.map_or(0, width));
                    widest[1] = widest[1].max(layout.right.map_or(0, width));
                    widest
                })
        }
    }
}

/// Pushes one row that spans both split columns.
fn push_full(
    rows: &mut Vec<DiffRow>,
    split: &mut Vec<SplitRow>,
    mode: DiffViewMode,
    build: impl FnOnce(&mut Vec<DiffRow>),
) {
    let at = rows.len();
    build(rows);
    if mode == DiffViewMode::Split {
        split.push(SplitRow {
            left: Some(at),
            right: None,
            full: true,
        });
    }
}

/// Lays one hunk out side by side, with filler rows where one side is shorter.
///
/// Zed's `determine_spacer` compares wrap rows because it soft-wraps; we do not, so comparing
/// row counts inside each change block is the whole algorithm.
fn layout_split(
    split: &mut Vec<SplitRow>,
    hunk: &fleet_git::Hunk,
    first_line: usize,
    blocks: &[intraline::ChangeBlock],
) {
    let shared = |split: &mut Vec<SplitRow>, range: Range<usize>| {
        split.extend(range.map(|index| SplitRow {
            left: Some(first_line + index),
            right: Some(first_line + index),
            full: false,
        }));
    };
    let mut cursor = 0;
    for block in blocks {
        let Some(&start) = block.removed.first().or(block.added.first()) else {
            continue;
        };
        shared(split, cursor..start);
        for slot in 0..block.removed.len().max(block.added.len()) {
            split.push(SplitRow {
                left: block.removed.get(slot).map(|index| first_line + index),
                right: block.added.get(slot).map(|index| first_line + index),
                full: false,
            });
        }
        cursor = start + block.removed.len() + block.added.len();
    }
    shared(split, cursor..hunk.lines.len());
}

fn meta(file: &fleet_git::DiffFile) -> FileMeta {
    let display = |path: &std::path::PathBuf| path.display().to_string();
    let new_path = file.new_path.as_ref().map(display);
    let old_path = file.old_path.as_ref().map(display);
    let path = new_path
        .clone()
        .or_else(|| old_path.clone())
        .unwrap_or_default();
    let mut added = 0u32;
    let mut removed = 0u32;
    for line in file.hunks.iter().flat_map(|hunk| hunk.lines.iter()) {
        match line.kind {
            LineKind::Added => added += 1,
            LineKind::Removed => removed += 1,
            _ => {}
        }
    }
    let mode = file.mode.as_ref().and_then(|mode| {
        let old = mode.old.as_deref()?;
        let new = mode.new.as_deref()?;
        (old != new).then(|| format!("{old} \u{2192} {new}"))
    });
    FileMeta {
        language: (!file.binary)
            .then(|| syntax::language_for_path(&path))
            .flatten(),
        old_path: old_path.filter(|old| Some(old) != new_path.as_ref()),
        path,
        kind: file.kind,
        binary: file.binary,
        added,
        removed,
        mode,
    }
}

/// Expands tabs to the next tab stop, counting columns in characters.
///
/// Zed never lets a `\t` reach the shaper, and neither can we: a tab inside a `StyledText` would
/// shift every highlight range that follows it off its glyph.
#[must_use]
pub(crate) fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + TAB_WIDTH);
    let mut column = 0usize;
    for character in text.chars() {
        if character == '\t' {
            let width = TAB_WIDTH - (column % TAB_WIDTH);
            out.extend(std::iter::repeat_n(' ', width));
            column += width;
        } else {
            out.push(character);
            column += 1;
        }
    }
    out
}

/// The `+` / `-` / ` ` marker lazygit prints in the sign column.
#[must_use]
pub(crate) fn marker(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Added => "+",
        RowKind::Removed => "-",
        RowKind::Context => " ",
        RowKind::HunkHeader | RowKind::FileHeader | RowKind::FileHeaderFoot | RowKind::Note => "",
        RowKind::Other => "\\",
    }
}

/// One plain run covering the whole line. Test scaffolding for [`DiffModel::apply_syntax`].
#[cfg(test)]
#[must_use]
fn plain_runs(text: &str) -> Runs {
    vec![(0..text.len(), super::syntax::Bucket::Plain)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_git::parse;

    const PATCH: &[u8] = concat!(
        "diff --git a/demo.ts b/demo.ts\n",
        "--- a/demo.ts\n",
        "+++ b/demo.ts\n",
        "@@ -1,2 +1,3 @@\n",
        " const one = 1;\n",
        "-const two = 2;\n",
        "+const two = 3;\n",
        "+const three = 4;\n",
    )
    .as_bytes();

    fn model(mode: DiffViewMode) -> DiffModel {
        let diff = parse::diff::parse(PATCH).expect("parses");
        DiffModel::build(&diff, mode)
    }

    #[test]
    fn cancelled_model_work_produces_no_rows() {
        let diff = parse::diff::parse(PATCH).unwrap();
        assert!(
            DiffModel::build_cancellable(&diff, DiffViewMode::Unified, &AtomicBool::new(true))
                .is_none()
        );
    }

    #[test]
    fn flatten_lays_out_a_two_row_header_then_the_hunk() {
        let model = model(DiffViewMode::Unified);
        assert_eq!(model.rows[0].kind, RowKind::FileHeader);
        assert_eq!(model.rows[1].kind, RowKind::FileHeaderFoot);
        assert_eq!(model.rows[2].kind, RowKind::HunkHeader);
        assert_eq!(model.rows[3].kind, RowKind::Context);
        assert_eq!(model.rows[4].kind, RowKind::Removed);
        assert_eq!(model.rows[5].kind, RowKind::Added);
        assert_eq!(model.rows[6].kind, RowKind::Added);
        assert_eq!(model.len(), 7);
    }

    #[test]
    fn rows_keep_the_staging_coordinates() {
        let model = model(DiffViewMode::Unified);
        assert_eq!(model.rows[4].file, 0);
        assert_eq!(model.rows[4].hunk, Some(0));
        assert_eq!(model.rows[4].line, Some(1));
        assert!(model.rows[4].is_change());
        assert!(!model.rows[3].is_change());
        // Headers carry no hunk or line, so a selection can never include them.
        assert_eq!(model.rows[0].hunk, None);
        assert_eq!(model.rows[2].line, None);
    }

    #[test]
    fn file_meta_counts_and_detects_the_language() {
        let model = model(DiffViewMode::Unified);
        let file = &model.files[0];
        assert_eq!(file.path, "demo.ts");
        assert_eq!(file.added, 2);
        assert_eq!(file.removed, 1);
        assert_eq!(file.language.as_deref(), Some("TypeScript"));
        assert!(!file.binary);
        assert_eq!(model.digits, 2);
    }

    #[test]
    fn the_paired_line_gets_word_marks() {
        let model = model(DiffViewMode::Unified);
        let removed = &model.rows[4];
        let added = &model.rows[5];
        assert_eq!(removed.words.len(), 1);
        assert_eq!(&removed.text[removed.words[0].clone()], "2");
        assert_eq!(&added.text[added.words[0].clone()], "3");
        // The unpaired added line has no partner, so no marks.
        assert!(model.rows[6].words.is_empty());
    }

    #[test]
    fn split_aligns_the_two_sides_with_filler() {
        let model = model(DiffViewMode::Split);
        assert_eq!(model.len(), model.split.len());
        // Two header rows and one separator span both columns.
        assert!(model.split[0].full);
        assert!(model.split[2].full);
        let payload: Vec<SplitRow> = model
            .split
            .iter()
            .copied()
            .filter(|row| !row.full)
            .collect();
        assert_eq!(payload.len(), 3);
        // The context line appears on both sides.
        assert_eq!(payload[0].left, payload[0].right);
        // One removal against two additions: the second added row has no left partner.
        assert!(payload[1].left.is_some() && payload[1].right.is_some());
        assert_eq!(payload[2].left, None);
        assert!(payload[2].right.is_some());
    }

    #[test]
    fn syntax_jobs_split_the_two_sides_of_a_hunk() {
        let model = model(DiffViewMode::Unified);
        let jobs = model.syntax_jobs();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|job| job.language == "TypeScript"));
        // Old side: context + removal. New side: context + two additions.
        let sizes: Vec<usize> = jobs.iter().map(|job| job.lines.len()).collect();
        assert!(sizes.contains(&2) && sizes.contains(&3), "{sizes:?}");
    }

    #[test]
    fn syntax_is_applied_by_unified_row() {
        let model = model(DiffViewMode::Unified);
        assert!(model.runs_for(5).is_empty());
        model.apply_syntax(vec![(5, plain_runs(&model.rows[5].text))]);
        assert_eq!(model.runs_for(5).len(), 1);
    }

    #[test]
    fn a_binary_file_gets_a_note_and_no_payload_rows() {
        let patch = concat!(
            "diff --git a/logo.png b/logo.png\n",
            "index 0000000..1111111 100644\n",
            "Binary files a/logo.png and b/logo.png differ\n",
        );
        let diff = parse::diff::parse(patch.as_bytes()).expect("parses");
        let model = DiffModel::build(&diff, DiffViewMode::Unified);
        assert_eq!(model.rows.len(), 3);
        assert_eq!(model.rows[2].kind, RowKind::Note);
        assert!(model.files[0].binary);
        assert_eq!(model.files[0].language, None);
        assert!(!model.rows.iter().any(DiffRow::is_change));
    }

    #[test]
    fn tabs_expand_before_anything_measures_a_column() {
        assert_eq!(expand_tabs("\tx"), "  x");
        assert_eq!(expand_tabs("a\tb"), "a b");
        assert_eq!(expand_tabs("no tabs"), "no tabs");
    }

    #[test]
    fn the_key_notices_a_different_patch_at_the_same_shape() {
        let diff = std::sync::Arc::new(parse::diff::parse(PATCH).expect("parses"));
        let same = ModelKey::new(&diff, DiffViewMode::Unified);
        assert_eq!(same, ModelKey::new(&diff, DiffViewMode::Unified));
        assert_ne!(same, ModelKey::new(&diff, DiffViewMode::Split));
        let other = std::sync::Arc::new(parse::diff::parse(PATCH).expect("parses"));
        assert_ne!(same, ModelKey::new(&other, DiffViewMode::Unified));
    }
}
