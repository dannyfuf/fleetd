//! Partial-patch construction.
//!
//! The transformation mirrors lazygit's `pkg/commands/patch` builder: unselected
//! lines belonging to the pre-image are demoted to context, unselected
//! post-image lines are omitted entirely, and every hunk header is recomputed so
//! the resulting patch applies cleanly on its own.

use std::{collections::HashSet, path::Path};

use crate::{
    Diff, DiffFile, DiffKind, DiffLine, GitError, Hunk, HunkSelection, LineKind, LineRange, Result,
};

/// Builds a minimal, self-consistent patch containing only the selected changes.
///
/// `reverse` must be `true` whenever the caller hands the patch to
/// `git apply --reverse`; the pre-image is then the patch's *new* side, so the
/// roles of added and removed lines swap.
pub(crate) fn build(diff: &Diff, selections: &[HunkSelection], reverse: bool) -> Result<Vec<u8>> {
    if diff.files.len() != 1 {
        return Err(GitError::parse(
            "patch selection",
            format!("expected one file, got {}", diff.files.len()),
        ));
    }
    let source = &diff.files[0];
    if let Some(selection) = selections
        .iter()
        .find(|selection| selection.hunk_index >= source.hunks.len())
    {
        return Err(GitError::parse(
            "patch selection",
            format!(
                "hunk index {} is outside the diff ({} hunks)",
                selection.hunk_index,
                source.hunks.len()
            ),
        ));
    }
    if source.binary {
        return Err(GitError::parse(
            "patch selection",
            "binary files cannot be staged line by line",
        ));
    }

    let mut hunks = Vec::new();
    let mut start_offset: i64 = 0;
    for (index, hunk) in source.hunks.iter().enumerate() {
        let selected = match selections
            .iter()
            .find(|selection| selection.hunk_index == index)
        {
            None => Some(HashSet::new()),
            Some(selection) => match &selection.lines {
                None => None,
                Some(lines) => {
                    if let Some(line) = lines.iter().find(|line| **line >= hunk.lines.len()) {
                        return Err(GitError::parse(
                            "patch selection",
                            format!("line index {line} is outside hunk {index}"),
                        ));
                    }
                    Some(lines.iter().copied().collect())
                }
            },
        };
        let lines = transform_lines(hunk, selected.as_ref(), reverse);
        let old_count = count(&lines, LineKind::Removed);
        let new_count = count(&lines, LineKind::Added);
        let (new_start, next_offset) =
            transform_header(old_count, new_count, hunk.old.start, start_offset);
        start_offset = next_offset;
        if lines
            .iter()
            .any(|line| matches!(line.kind, LineKind::Added | LineKind::Removed))
        {
            hunks.push(Hunk {
                old: LineRange {
                    start: hunk.old.start,
                    count: old_count,
                },
                new: LineRange {
                    start: new_start,
                    count: new_count,
                },
                header: hunk.header.clone(),
                lines,
            });
        }
    }
    if hunks.is_empty() {
        return Err(GitError::parse(
            "patch selection",
            "selection contains no changed lines",
        ));
    }
    let headers = header_lines(source, &hunks);
    let file = DiffFile {
        old_path: source.old_path.clone(),
        new_path: source.new_path.clone(),
        kind: source.kind,
        binary: source.binary,
        mode: source.mode.clone(),
        headers,
        hunks,
    };
    Ok(crate::parse::diff::render(&Diff { files: vec![file] }))
}

/// Rewrites the two path headers so the patch stands alone.
///
/// Rename and copy metadata is dropped and both sides point at the destination
/// path, which keeps `git apply` from trying to redo the rename while only part
/// of the content is being staged.
fn header_lines(file: &DiffFile, hunks: &[Hunk]) -> Vec<Vec<u8>> {
    let renamed = matches!(file.kind, DiffKind::Renamed | DiffKind::Copied);
    let old_side_is_empty = hunks.iter().all(|hunk| hunk.old.count == 0);
    let new_side_is_empty = hunks.iter().all(|hunk| hunk.new.count == 0);
    let old = if renamed || (file.kind == DiffKind::Added && !old_side_is_empty) {
        file.new_path.as_deref()
    } else {
        file.old_path.as_deref()
    };
    let new = if file.kind == DiffKind::Deleted && !new_side_is_empty {
        file.old_path.as_deref()
    } else {
        file.new_path.as_deref()
    };
    let mut headers = Vec::new();
    if let Some(header) = diff_header(old, new) {
        headers.push(header);
    }
    if let Some(mode) = &file.mode {
        match file.kind {
            DiffKind::Added if old_side_is_empty => {
                push_mode(&mut headers, b"new file mode ", mode.new.as_deref());
            }
            DiffKind::Deleted if new_side_is_empty => {
                push_mode(&mut headers, b"deleted file mode ", mode.old.as_deref());
            }
            DiffKind::Added | DiffKind::Deleted => {}
            _ => {
                push_mode(&mut headers, b"old mode ", mode.old.as_deref());
                push_mode(&mut headers, b"new mode ", mode.new.as_deref());
            }
        }
    }
    headers.push(path_header(b"--- ", b"a/", old));
    headers.push(path_header(b"+++ ", b"b/", new));
    headers
}

fn diff_header(old: Option<&Path>, new: Option<&Path>) -> Option<Vec<u8>> {
    let old = old.or(new)?;
    let new = new.unwrap_or(old);
    let mut header = b"diff --git ".to_vec();
    header.extend_from_slice(&crate::parse::path::prefixed_path(b"a/", old));
    header.push(b' ');
    header.extend_from_slice(&crate::parse::path::prefixed_path(b"b/", new));
    Some(header)
}

fn push_mode(headers: &mut Vec<Vec<u8>>, marker: &[u8], mode: Option<&str>) {
    if let Some(mode) = mode {
        let mut header = marker.to_vec();
        header.extend_from_slice(mode.as_bytes());
        headers.push(header);
    }
}

fn path_header(marker: &[u8], prefix: &[u8], path: Option<&Path>) -> Vec<u8> {
    let mut line = marker.to_vec();
    match path {
        Some(path) => {
            line.extend_from_slice(&crate::parse::path::prefixed_path(prefix, path));
        }
        None => line.extend_from_slice(b"/dev/null"),
    }
    line
}

/// Applies the selection to one hunk's lines.
///
/// `selected` of `None` keeps the hunk whole; otherwise it holds the zero-based
/// indexes of the changed lines to keep.
fn transform_lines(hunk: &Hunk, selected: Option<&HashSet<usize>>, reverse: bool) -> Vec<DiffLine> {
    let mut lines: Vec<DiffLine> = Vec::with_capacity(hunk.lines.len());
    // Unselected pre-image lines become context, but they are buffered so they
    // land *after* any selected post-image line of the same change block.
    let mut pending: Vec<DiffLine> = Vec::new();
    let mut saw_unselected_post_image = false;
    let mut skip_no_newline_at: Option<usize> = None;
    for (index, line) in hunk.lines.iter().enumerate() {
        match line.kind {
            LineKind::Context | LineKind::Other => {
                lines.append(&mut pending);
                saw_unselected_post_image = false;
                lines.push(line.clone());
            }
            LineKind::NoNewline => {
                if skip_no_newline_at != Some(index) {
                    lines.append(&mut pending);
                    lines.push(line.clone());
                }
            }
            LineKind::Added | LineKind::Removed => {
                let is_pre_image = (line.kind == LineKind::Removed) != reverse;
                let is_selected = selected.is_none_or(|set| set.contains(&index));
                if is_selected {
                    if is_pre_image || saw_unselected_post_image {
                        lines.append(&mut pending);
                    }
                    lines.push(line.clone());
                } else if is_pre_image {
                    pending.push(DiffLine {
                        kind: LineKind::Context,
                        content: line.content.clone(),
                        old_no: line.old_no,
                        new_no: line.new_no,
                    });
                } else {
                    saw_unselected_post_image = true;
                    if line.kind == LineKind::Added {
                        // The `\ No newline` marker belongs to the omitted line.
                        skip_no_newline_at = Some(index + 1);
                    }
                }
            }
        }
    }
    lines.append(&mut pending);
    lines
}

/// Returns the hunk's new-side start and the running offset for later hunks.
fn transform_header(
    old_count: u32,
    new_count: u32,
    old_start: u32,
    start_offset: i64,
) -> (u32, i64) {
    // A hunk that lost its whole pre-image starts one line later; one that lost
    // its whole post-image starts one line earlier.
    let adjustment = if old_count == 0 {
        1
    } else if new_count == 0 {
        -1
    } else {
        0
    };
    let new_start = (i64::from(old_start) + start_offset + adjustment).max(0) as u32;
    let next_offset = start_offset + i64::from(new_count) - i64::from(old_count);
    (new_start, next_offset)
}

fn count(lines: &[DiffLine], changed: LineKind) -> u32 {
    lines
        .iter()
        .filter(|line| {
            matches!(line.kind, LineKind::Context | LineKind::Other) || line.kind == changed
        })
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::{HunkSelection, parse::diff::parse};

    #[cfg(unix)]
    #[test]
    fn patch_preserves_quoted_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;

        let parsed = parse(
            b"diff --git \"a/name\\377\" \"b/name\\377\"\n--- \"a/name\\377\"\n+++ \"b/name\\377\"\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .expect("parse quoted path");
        assert_eq!(
            parsed.files[0]
                .new_path
                .as_deref()
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"name\xff"
        );

        let patch = build(
            &parsed,
            &[HunkSelection {
                hunk_index: 0,
                lines: None,
            }],
            false,
        )
        .expect("build quoted patch");
        assert!(patch.starts_with(
            b"diff --git \"a/name\\377\" \"b/name\\377\"\n--- \"a/name\\377\"\n+++ \"b/name\\377\"\n"
        ));
    }

    #[test]
    fn new_executable_retains_mode() {
        let parsed = parse(
            b"diff --git a/script b/script\nnew file mode 100755\n--- /dev/null\n+++ b/script\n@@ -0,0 +1 @@\n+echo hi\n",
        )
        .expect("parse executable addition");
        let patch = build(
            &parsed,
            &[HunkSelection {
                hunk_index: 0,
                lines: None,
            }],
            false,
        )
        .expect("build executable patch");

        assert!(patch.starts_with(
            b"diff --git a/script b/script\nnew file mode 100755\n--- /dev/null\n+++ b/script\n"
        ));
    }

    fn text(diff: &[u8], selections: &[HunkSelection], reverse: bool) -> String {
        let parsed = parse(diff).expect("parse fixture");
        String::from_utf8(build(&parsed, selections, reverse).expect("build patch"))
            .expect("utf-8 patch")
    }

    const REPLACEMENT: &[u8] = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n same\n-old\n+new\n-tail\n+replacement\n";

    #[test]
    fn omits_unselected_additions_and_contextualizes_removals() {
        let patch = text(
            REPLACEMENT,
            &[HunkSelection {
                hunk_index: 0,
                lines: Some(vec![4]),
            }],
            false,
        );
        assert!(!patch.contains("+new"));
        assert!(patch.contains(" old\n"));
        assert!(patch.contains(" tail\n"));
        assert!(patch.contains("+replacement"));
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
        assert!(patch.starts_with("diff --git a/a b/a\n--- a/a\n+++ b/a\n"));
    }

    #[test]
    fn reverse_selection_contextualizes_additions_instead() {
        let patch = text(
            REPLACEMENT,
            &[HunkSelection {
                hunk_index: 0,
                lines: Some(vec![1]),
            }],
            true,
        );
        // Only the selected removal survives; unselected additions become context.
        assert!(patch.contains("-old"));
        assert!(patch.contains(" new\n"));
        assert!(patch.contains(" replacement\n"));
        assert!(!patch.contains("-tail"));
        assert!(patch.contains("@@ -1,4 +1,3 @@"));
    }

    #[test]
    fn selecting_a_removal_drops_the_paired_addition() {
        let patch = text(
            REPLACEMENT,
            &[HunkSelection {
                hunk_index: 0,
                lines: Some(vec![1]),
            }],
            false,
        );
        assert!(patch.contains("-old"));
        assert!(!patch.contains("+new"));
        assert!(patch.contains("@@ -1,3 +1,2 @@"));
    }

    #[test]
    fn later_hunk_starts_shift_by_the_earlier_selection() {
        let fixture = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,3 @@\n one\n+inserted\n two\n@@ -10,2 +11,3 @@\n ten\n+also\n eleven\n";
        let patch = text(
            fixture,
            &[
                HunkSelection {
                    hunk_index: 0,
                    lines: None,
                },
                HunkSelection {
                    hunk_index: 1,
                    lines: None,
                },
            ],
            false,
        );
        assert!(patch.contains("@@ -1,2 +1,3 @@"));
        assert!(patch.contains("@@ -10,2 +11,3 @@"));

        // Dropping the first hunk pulls the second hunk's new start back.
        let patch = text(
            fixture,
            &[HunkSelection {
                hunk_index: 1,
                lines: None,
            }],
            false,
        );
        assert!(!patch.contains("+inserted"));
        assert!(patch.contains("@@ -10,2 +10,3 @@"));
    }

    #[test]
    fn retains_full_addition_and_deletion_hunks() {
        let added = text(
            b"diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n@@ -0,0 +1,1 @@\n+x\n",
            &[HunkSelection {
                hunk_index: 0,
                lines: None,
            }],
            false,
        );
        assert!(
            added.starts_with("diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n")
        );
        assert!(added.contains("@@ -0,0 +1,1 @@"));

        let deleted = text(
            b"diff --git a/a b/a\ndeleted file mode 100644\n--- a/a\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n",
            &[HunkSelection {
                hunk_index: 0,
                lines: None,
            }],
            false,
        );
        assert!(
            deleted.starts_with(
                "diff --git a/a b/a\ndeleted file mode 100644\n--- a/a\n+++ /dev/null\n"
            )
        );
        assert!(deleted.contains("@@ -1,1 +0,0 @@"));
    }

    #[test]
    fn drops_the_no_newline_marker_of_an_omitted_addition() {
        let fixture = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n\\ No newline at end of file\n";
        let patch = text(
            fixture,
            &[HunkSelection {
                hunk_index: 0,
                lines: Some(vec![1]),
            }],
            false,
        );
        assert!(!patch.contains("No newline"));
    }

    #[test]
    fn rename_headers_collapse_onto_the_destination_path() {
        let fixture = b"diff --git a/old b/new\nsimilarity index 80%\nrename from old\nrename to new\n--- a/old\n+++ b/new\n@@ -1,1 +1,1 @@\n-a\n+b\n";
        let patch = text(
            fixture,
            &[HunkSelection {
                hunk_index: 0,
                lines: None,
            }],
            false,
        );
        assert!(patch.starts_with("diff --git a/new b/new\n--- a/new\n+++ b/new\n"));
        assert!(!patch.contains("rename from"));
    }

    #[test]
    fn rejects_empty_out_of_range_and_binary_selections() {
        let diff = parse(REPLACEMENT).unwrap();
        assert!(build(&diff, &[], false).is_err());
        assert!(
            build(
                &diff,
                &[HunkSelection {
                    hunk_index: 4,
                    lines: None
                }],
                false
            )
            .is_err()
        );
        assert!(
            build(
                &diff,
                &[HunkSelection {
                    hunk_index: 0,
                    lines: Some(vec![99])
                }],
                false
            )
            .is_err()
        );
        let binary = parse(b"diff --git a/b b/b\nBinary files a/b and b/b differ\n").unwrap();
        assert!(
            build(
                &binary,
                &[HunkSelection {
                    hunk_index: 0,
                    lines: None
                }],
                false
            )
            .is_err()
        );
    }
}
