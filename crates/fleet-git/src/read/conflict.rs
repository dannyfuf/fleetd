use crate::{ConflictFile, ConflictSection, GitError, Repository, Result};
use std::path::Path;

impl Repository {
    /// Reads and parses conflict-marker regions in a worktree file.
    pub async fn conflicted_file(&self, path: &Path) -> Result<ConflictFile> {
        let absolute = self.paths.worktree_root.join(path);
        let content = tokio::fs::read(&absolute)
            .await
            .map_err(|source| GitError::Spawn {
                argv: vec![format!("read {}", absolute.display())],
                source,
            })?;
        let marker_len = self.conflict_marker_len(path).await?;
        let conflicts = parse_conflicts(&content, marker_len)?;
        Ok(ConflictFile {
            path: path.to_path_buf(),
            content,
            conflicts,
        })
    }

    async fn conflict_marker_len(&self, path: &Path) -> Result<usize> {
        let output = self
            .runner
            .run(
                self.command(crate::CommandKind::Read)
                    .args(["check-attr", "-z", "conflict-marker-size"])
                    .paths([path]),
            )
            .await?;
        let mut fields = output.stdout.split(|byte| *byte == b'\0');
        let _path = fields.next();
        let attribute = fields.next();
        let value = fields.next();
        let complete = fields.next() == Some(&[][..]) && fields.next().is_none();
        if attribute != Some(b"conflict-marker-size") || !complete {
            return Err(GitError::parse(
                "conflict-marker-size attribute",
                "unexpected check-attr output",
            ));
        }
        Ok(value
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(7))
    }
}

fn parse_conflicts(content: &[u8], marker_len: usize) -> Result<Vec<ConflictSection>> {
    let mut sections = Vec::new();
    let mut cursor = 0;
    while let Some(start) = find_opening_marker(content, cursor, marker_len) {
        let ours_start = after_line(content, start)?;
        let separator = find_marker(content, ours_start, None, b'=', marker_len, false)
            .ok_or_else(|| GitError::parse("conflict markers", "missing ======= marker"))?;
        let base_marker = find_marker(content, ours_start, Some(separator), b'|', marker_len, true);
        let theirs_start = after_line(content, separator)?;
        let end_marker = find_marker(content, theirs_start, None, b'>', marker_len, true)
            .ok_or_else(|| GitError::parse("conflict markers", "missing >>>>>>> marker"))?;
        let end = after_line(content, end_marker)?;
        let (ours_end, base) = if let Some(base_marker) = base_marker {
            let base_start = after_line(content, base_marker)?;
            (base_marker, Some(content[base_start..separator].to_vec()))
        } else {
            (separator, None)
        };
        sections.push(ConflictSection {
            start,
            end,
            ours: content[ours_start..ours_end].to_vec(),
            base,
            theirs: content[theirs_start..end_marker].to_vec(),
        });
        cursor = end;
    }
    Ok(sections)
}

fn find_opening_marker(content: &[u8], start: usize, marker_len: usize) -> Option<usize> {
    lines(content, start, None).find_map(|(offset, line)| {
        (line.len() > marker_len
            && line[..marker_len].iter().all(|byte| *byte == b'<')
            && line[marker_len] == b' ')
            .then_some(offset)
    })
}

fn find_marker(
    content: &[u8],
    start: usize,
    end: Option<usize>,
    marker: u8,
    marker_len: usize,
    labeled: bool,
) -> Option<usize> {
    lines(content, start, end).find_map(|(offset, line)| {
        let matches = if labeled {
            line.len() > marker_len
                && line[..marker_len].iter().all(|byte| *byte == marker)
                && line[marker_len] == b' '
        } else {
            line.len() == marker_len && line.iter().all(|byte| *byte == marker)
        };
        matches.then_some(offset)
    })
}

fn lines(content: &[u8], start: usize, end: Option<usize>) -> impl Iterator<Item = (usize, &[u8])> {
    let end = end.unwrap_or(content.len()).min(content.len());
    let mut cursor = start.min(end);
    std::iter::from_fn(move || {
        if cursor >= end {
            return None;
        }
        let offset = cursor;
        let line_end = content[cursor..end]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(end, |relative| cursor + relative);
        cursor = (line_end + 1).min(end);
        let line = content[offset..line_end]
            .strip_suffix(b"\r")
            .unwrap_or(&content[offset..line_end]);
        Some((offset, line))
    })
}

fn after_line(content: &[u8], start: usize) -> Result<usize> {
    content[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| GitError::parse("conflict markers", "marker has no newline"))
}

#[cfg(test)]
mod tests {
    use super::parse_conflicts;
    #[test]
    fn parses_standard_and_diff3_conflicts() {
        let content = b"before\n<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> topic\nafter\n";
        let sections = parse_conflicts(content, 7).unwrap();
        assert_eq!(sections[0].ours, b"ours\n");
        assert_eq!(sections[0].base.as_deref(), Some(b"base\n".as_slice()));
        assert_eq!(sections[0].theirs, b"theirs\n");
    }

    #[test]
    fn inline_marker_is_not_a_conflict_opening() {
        let content = b"inline <<<<<<<<< not a marker\n<<<<<<<<< HEAD\nours\n=========\ntheirs\n>>>>>>>>> topic\n";
        let sections = parse_conflicts(content, 9).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].ours, b"ours\n");
        assert_eq!(sections[0].theirs, b"theirs\n");
    }

    #[test]
    fn parses_configured_short_marker_length() {
        let content = b"<<<< HEAD\nours\n====\ntheirs\n>>>> topic\n";
        let sections = parse_conflicts(content, 4).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].ours, b"ours\n");
        assert_eq!(sections[0].theirs, b"theirs\n");
    }
}
