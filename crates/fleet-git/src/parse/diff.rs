//! Unified-patch parsing and rendering.

use std::path::PathBuf;

use crate::{
    CommitFile, Diff, DiffFile, DiffKind, DiffLine, GitError, Hunk, LineKind, LineRange,
    ModeChange, Result,
};

/// Parses a no-color unified patch.
///
/// Hunk bodies are framed by the line counts in their `@@` header rather than by
/// scanning for the next marker, so a patch whose content itself contains
/// `diff --git` or `@@ ` lines still parses correctly.
pub fn parse(input: &[u8]) -> Result<Diff> {
    let mut files = Vec::new();
    let mut file: Option<DiffFile> = None;
    let mut hunk: Option<Hunk> = None;
    let mut remaining = Remaining::default();
    for line in input.split_inclusive(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        if let Some(current_hunk) = hunk.as_mut()
            && remaining.accepts(line)
        {
            parse_hunk_line(current_hunk, line, &mut remaining);
            continue;
        }
        finish_hunk(&mut file, &mut hunk);
        if line.starts_with(b"diff --git ") {
            if let Some(previous) = file.take() {
                files.push(previous);
            }
            file = Some(DiffFile {
                old_path: None,
                new_path: None,
                kind: DiffKind::Modified,
                binary: false,
                mode: None,
                headers: vec![line.to_vec()],
                hunks: Vec::new(),
            });
            continue;
        }
        if line.starts_with(b"@@ ") && file.is_some() {
            let parsed = parse_hunk_header(line)?;
            remaining = Remaining {
                old: parsed.old.count,
                new: parsed.new.count,
            };
            hunk = Some(parsed);
            continue;
        }
        let Some(current) = file.as_mut() else {
            continue;
        };
        current.headers.push(line.to_vec());
        if let Some(path) = line.strip_prefix(b"--- ") {
            current.old_path = parse_header_path(path);
            if current.old_path.is_none() {
                current.kind = DiffKind::Added;
            }
        } else if let Some(path) = line.strip_prefix(b"+++ ") {
            current.new_path = parse_header_path(path);
            if current.new_path.is_none() {
                current.kind = DiffKind::Deleted;
            }
        } else if let Some(path) = line.strip_prefix(b"rename from ") {
            current.old_path = Some(crate::parse::status::bytes_to_path(path));
            current.kind = DiffKind::Renamed;
        } else if let Some(path) = line.strip_prefix(b"rename to ") {
            current.new_path = Some(crate::parse::status::bytes_to_path(path));
        } else if let Some(path) = line.strip_prefix(b"copy from ") {
            current.old_path = Some(crate::parse::status::bytes_to_path(path));
            current.kind = DiffKind::Copied;
        } else if let Some(path) = line.strip_prefix(b"copy to ") {
            current.new_path = Some(crate::parse::status::bytes_to_path(path));
        } else if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            current.binary = true;
        } else if let Some(mode) = line.strip_prefix(b"old mode ") {
            current
                .mode
                .get_or_insert(ModeChange {
                    old: None,
                    new: None,
                })
                .old = Some(String::from_utf8_lossy(mode).into_owned());
        } else if let Some(mode) = line.strip_prefix(b"new mode ") {
            current
                .mode
                .get_or_insert(ModeChange {
                    old: None,
                    new: None,
                })
                .new = Some(String::from_utf8_lossy(mode).into_owned());
        } else if line.starts_with(b"new file mode ") {
            current.kind = DiffKind::Added;
        } else if line.starts_with(b"deleted file mode ") {
            current.kind = DiffKind::Deleted;
        }
    }
    finish_hunk(&mut file, &mut hunk);
    if let Some(file) = file {
        files.push(file);
    }
    Ok(Diff { files })
}

/// Renders a parsed diff into an applicable unified patch.
#[must_use]
pub fn render(diff: &Diff) -> Vec<u8> {
    let mut output = Vec::new();
    for file in &diff.files {
        for header in &file.headers {
            output.extend_from_slice(header);
            output.push(b'\n');
        }
        for hunk in &file.hunks {
            output.extend_from_slice(
                format!(
                    "@@ -{},{} +{},{} @@",
                    hunk.old.start, hunk.old.count, hunk.new.start, hunk.new.count
                )
                .as_bytes(),
            );
            output.extend_from_slice(&hunk.header);
            output.push(b'\n');
            for line in &hunk.lines {
                output.push(match line.kind {
                    LineKind::Context => b' ',
                    LineKind::Added => b'+',
                    LineKind::Removed => b'-',
                    LineKind::NoNewline => b'\\',
                    LineKind::Other => b' ',
                });
                output.extend_from_slice(&line.content);
                output.push(b'\n');
            }
        }
    }
    output
}

/// Parses `git diff-tree --name-status -z` output.
pub fn commit_files(input: &[u8]) -> Result<Vec<CommitFile>> {
    let fields: Vec<&[u8]> = input
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = fields[index];
        index += 1;
        let code = *status
            .first()
            .ok_or_else(|| GitError::parse("commit files", "empty status"))?;
        let first = fields
            .get(index)
            .ok_or_else(|| GitError::parse("commit files", "missing path"))?;
        index += 1;
        let (previous_path, path) = if matches!(code, b'R' | b'C') {
            let second = fields.get(index).ok_or_else(|| {
                GitError::parse("commit files", "rename/copy missing destination")
            })?;
            index += 1;
            (
                Some(crate::parse::status::bytes_to_path(first)),
                crate::parse::status::bytes_to_path(second),
            )
        } else {
            (None, crate::parse::status::bytes_to_path(first))
        };
        result.push(CommitFile {
            path,
            previous_path,
            kind: status_kind(code),
        });
    }
    Ok(result)
}

fn finish_hunk(file: &mut Option<DiffFile>, hunk: &mut Option<Hunk>) {
    if let (Some(file), Some(hunk)) = (file.as_mut(), hunk.take()) {
        file.hunks.push(hunk);
    }
}

fn parse_hunk_header(line: &[u8]) -> Result<Hunk> {
    let after = &line[3..];
    let end = find_bytes(after, b" @@")
        .ok_or_else(|| GitError::parse("diff hunk", "missing closing @@"))?;
    let ranges = &after[..end];
    let mut parts = ranges.split(|byte| *byte == b' ');
    let old = parse_range(parts.next().unwrap_or_default(), b'-')?;
    let new = parse_range(parts.next().unwrap_or_default(), b'+')?;
    let header = after[end + 3..].to_vec();
    Ok(Hunk {
        old,
        new,
        header,
        lines: Vec::new(),
    })
}

fn parse_range(value: &[u8], prefix: u8) -> Result<LineRange> {
    let value = value
        .strip_prefix(&[prefix])
        .ok_or_else(|| GitError::parse("diff hunk", "invalid range prefix"))?;
    let mut parts = value.split(|byte| *byte == b',');
    let start = parse_u32(parts.next().unwrap_or_default())?;
    let count = match parts.next() {
        Some(count) => parse_u32(count)?,
        None => 1,
    };
    Ok(LineRange { start, count })
}

fn parse_u32(bytes: &[u8]) -> Result<u32> {
    std::str::from_utf8(bytes)
        .map_err(|error| GitError::parse("diff hunk", error.to_string()))?
        .parse()
        .map_err(|error| GitError::parse("diff hunk", format!("invalid number: {error}")))
}

/// Line budget left in the hunk currently being parsed.
#[derive(Debug, Clone, Copy, Default)]
struct Remaining {
    old: u32,
    new: u32,
}

impl Remaining {
    /// Reports whether `line` still belongs to the open hunk.
    fn accepts(&self, line: &[u8]) -> bool {
        // The no-newline marker annotates the preceding line and consumes no budget.
        if line.first() == Some(&b'\\') {
            return true;
        }
        if self.old == 0 && self.new == 0 {
            return false;
        }
        match line.first() {
            Some(b'+') => self.new > 0,
            Some(b'-') => self.old > 0,
            // A context line (` `, or a bare empty line emitted by transports
            // that strip trailing whitespace) needs budget on both sides.
            Some(b' ') | None => self.old > 0 && self.new > 0,
            _ => false,
        }
    }

    fn consume(&mut self, kind: LineKind) {
        match kind {
            LineKind::Context | LineKind::Other => {
                self.old = self.old.saturating_sub(1);
                self.new = self.new.saturating_sub(1);
            }
            LineKind::Added => self.new = self.new.saturating_sub(1),
            LineKind::Removed => self.old = self.old.saturating_sub(1),
            LineKind::NoNewline => {}
        }
    }
}

fn parse_hunk_line(hunk: &mut Hunk, line: &[u8], remaining: &mut Remaining) {
    let (kind, content) = match line.first() {
        Some(b' ') => (LineKind::Context, &line[1..]),
        Some(b'+') => (LineKind::Added, &line[1..]),
        Some(b'-') => (LineKind::Removed, &line[1..]),
        Some(b'\\') => (LineKind::NoNewline, &line[1..]),
        None => (LineKind::Context, line),
        _ => (LineKind::Other, line),
    };
    remaining.consume(kind);
    let (old_no, new_no) = match kind {
        LineKind::Context => {
            let old = next_old(hunk);
            let new = next_new(hunk);
            (Some(old), Some(new))
        }
        LineKind::Removed => (Some(next_old(hunk)), None),
        LineKind::Added => (None, Some(next_new(hunk))),
        LineKind::NoNewline | LineKind::Other => (None, None),
    };
    hunk.lines.push(DiffLine {
        kind,
        content: content.to_vec(),
        old_no,
        new_no,
    });
}

fn next_old(hunk: &Hunk) -> u32 {
    hunk.old.start
        + hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind, LineKind::Context | LineKind::Removed))
            .count() as u32
}

fn next_new(hunk: &Hunk) -> u32 {
    hunk.new.start
        + hunk
            .lines
            .iter()
            .filter(|line| matches!(line.kind, LineKind::Context | LineKind::Added))
            .count() as u32
}

fn parse_header_path(value: &[u8]) -> Option<PathBuf> {
    let value = value.split(|byte| *byte == b'\t').next().unwrap_or(value);
    if value == b"/dev/null" {
        return None;
    }
    let value = value
        .strip_prefix(b"a/")
        .or_else(|| value.strip_prefix(b"b/"))
        .unwrap_or(value);
    Some(crate::parse::status::bytes_to_path(value))
}

fn status_kind(code: u8) -> DiffKind {
    match code {
        b'A' => DiffKind::Added,
        b'D' => DiffKind::Deleted,
        b'M' => DiffKind::Modified,
        b'R' => DiffKind::Renamed,
        b'C' => DiffKind::Copied,
        b'T' => DiffKind::TypeChanged,
        b'U' => DiffKind::Unmerged,
        _ => DiffKind::Unknown,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{commit_files, parse, render};
    use crate::{DiffKind, LineKind};

    #[test]
    fn parses_mode_changes_renames_binaries_and_no_newline_markers() {
        let fixture: &[u8] = b"diff --git a/a b/a\nold mode 100644\nnew mode 100755\nindex 111..222 100644\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@ fn heading()\n-old\n+new\n same\n\\ No newline at end of file\ndiff --git a/old b/new\nsimilarity index 100%\nrename from old\nrename to new\ndiff --git a/bin b/bin\nindex 333..444 100644\nBinary files a/bin and b/bin differ\n";
        let diff = parse(fixture).expect("parse diff");
        assert_eq!(diff.files.len(), 3);

        let first = &diff.files[0];
        assert_eq!(first.old_path.as_deref(), Some(std::path::Path::new("a")));
        assert_eq!(first.new_path.as_deref(), Some(std::path::Path::new("a")));
        let mode = first.mode.as_ref().expect("mode change");
        assert_eq!(mode.old.as_deref(), Some("100644"));
        assert_eq!(mode.new.as_deref(), Some("100755"));
        let hunk = &first.hunks[0];
        assert_eq!(hunk.header, b" fn heading()");
        assert_eq!(hunk.old.start, 1);
        assert_eq!(hunk.old.count, 2);
        assert_eq!(hunk.lines[0].kind, LineKind::Removed);
        assert_eq!(hunk.lines[1].kind, LineKind::Added);
        assert_eq!(hunk.lines[2].kind, LineKind::Context);
        assert_eq!(hunk.lines[3].kind, LineKind::NoNewline);

        assert_eq!(diff.files[1].kind, DiffKind::Renamed);
        assert_eq!(
            diff.files[1].old_path.as_deref(),
            Some(std::path::Path::new("old"))
        );
        assert_eq!(
            diff.files[1].new_path.as_deref(),
            Some(std::path::Path::new("new"))
        );

        assert!(diff.files[2].binary);
        assert!(diff.files[2].hunks.is_empty());

        assert!(String::from_utf8_lossy(&render(&diff)).contains("\\ No newline at end of file"));
    }

    #[test]
    fn numbers_lines_across_multiple_hunks() {
        let fixture: &[u8] = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,4 @@\n one\n+inserted\n two\n three\n@@ -10,2 +11,2 @@\n-ten\n+TEN\n";
        let diff = parse(fixture).expect("parse diff");
        let first = &diff.files[0].hunks[0];
        assert_eq!(first.lines[0].old_no, Some(1));
        assert_eq!(first.lines[0].new_no, Some(1));
        assert_eq!(first.lines[1].old_no, None);
        assert_eq!(first.lines[1].new_no, Some(2));
        assert_eq!(first.lines[2].old_no, Some(2));
        assert_eq!(first.lines[2].new_no, Some(3));

        let second = &diff.files[0].hunks[1];
        assert_eq!(second.lines[0].old_no, Some(10));
        assert_eq!(second.lines[0].new_no, None);
        assert_eq!(second.lines[1].new_no, Some(11));
    }

    #[test]
    fn parses_addition_only_and_deletion_only_hunks() {
        let fixture: &[u8] = b"diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n@@ -0,0 +1,2 @@\n+x\n+y\ndiff --git a/b b/b\ndeleted file mode 100644\n--- a/b\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n";
        let diff = parse(fixture).expect("parse diff");
        assert_eq!(diff.files[0].kind, DiffKind::Added);
        assert!(diff.files[0].old_path.is_none());
        assert_eq!(diff.files[0].hunks[0].old.count, 0);
        assert_eq!(diff.files[1].kind, DiffKind::Deleted);
        assert!(diff.files[1].new_path.is_none());
        assert_eq!(diff.files[1].hunks[0].new.count, 0);
    }

    #[test]
    fn hunk_line_counts_frame_content_that_looks_like_diff_markers() {
        // The hunk body itself contains `diff --git` and `@@` lines; the header
        // counts must keep them inside the hunk instead of starting a new file.
        let fixture: &[u8] = b"diff --git a/p.patch b/p.patch\n--- a/p.patch\n+++ b/p.patch\n@@ -1,3 +1,3 @@\n diff --git a/x b/x\n-@@ -1 +1 @@\n+@@ -2 +2 @@\n ok\ndiff --git a/second b/second\n--- a/second\n+++ b/second\n@@ -1 +1 @@\n-a\n+b\n";
        let diff = parse(fixture).expect("parse diff");
        assert_eq!(diff.files.len(), 2);
        assert_eq!(diff.files[0].hunks.len(), 1);
        assert_eq!(diff.files[0].hunks[0].lines.len(), 4);
        assert_eq!(
            diff.files[1].new_path.as_deref(),
            Some(std::path::Path::new("second"))
        );
    }

    #[test]
    fn treats_a_bare_empty_line_inside_a_hunk_as_context() {
        let fixture: &[u8] =
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n one\n\n-two\n+TWO\n";
        let diff = parse(fixture).expect("parse diff");
        let hunk = &diff.files[0].hunks[0];
        assert_eq!(hunk.lines[1].kind, LineKind::Context);
        assert!(hunk.lines[1].content.is_empty());
        assert_eq!(hunk.lines[2].kind, LineKind::Removed);
    }

    #[test]
    fn rendering_round_trips_a_parsed_patch() {
        let fixture: &[u8] =
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
        let diff = parse(fixture).expect("parse diff");
        assert_eq!(render(&diff), fixture);
        assert!(diff.to_text_lossy().contains("+TWO"));
    }

    #[test]
    fn parses_commit_file_name_status_records() {
        let files = commit_files(b"M\x00a\x00R100\x00old\x00new\x00A\x00added\x00D\x00gone\x00")
            .expect("parse");
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].kind, DiffKind::Modified);
        assert_eq!(files[1].kind, DiffKind::Renamed);
        assert_eq!(
            files[1].previous_path.as_deref(),
            Some(std::path::Path::new("old"))
        );
        assert_eq!(files[1].path, std::path::Path::new("new"));
        assert_eq!(files[2].kind, DiffKind::Added);
        assert_eq!(files[3].kind, DiffKind::Deleted);
        assert!(commit_files(b"R100\x00only-source\x00").is_err());
    }

    #[test]
    fn empty_and_headerless_input_is_accepted() {
        assert!(parse(b"").expect("empty").files.is_empty());
        assert!(
            parse(b"@@ -1 +1 @@\n-a\n+b\n")
                .expect("orphan hunk")
                .files
                .is_empty()
        );
    }
}
