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
        let conflicts = parse_conflicts(&content)?;
        Ok(ConflictFile {
            path: path.to_path_buf(),
            content,
            conflicts,
        })
    }
}

fn parse_conflicts(content: &[u8]) -> Result<Vec<ConflictSection>> {
    let mut sections = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = find_at_line_start(&content[cursor..], b"<<<<<<< ") {
        let start = cursor + relative;
        let ours_start = after_line(content, start)?;
        let separator = find_at_line_start(&content[ours_start..], b"=======")
            .ok_or_else(|| GitError::parse("conflict markers", "missing ======= marker"))?
            + ours_start;
        let base_marker = find_at_line_start(&content[ours_start..separator], b"||||||| ")
            .map(|offset| ours_start + offset);
        let theirs_start = after_line(content, separator)?;
        let end_marker = find_at_line_start(&content[theirs_start..], b">>>>>>> ")
            .ok_or_else(|| GitError::parse("conflict markers", "missing >>>>>>> marker"))?
            + theirs_start;
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

fn find_at_line_start(content: &[u8], needle: &[u8]) -> Option<usize> {
    content
        .windows(needle.len())
        .position(|window| window == needle)
        .filter(|index| *index == 0 || content[index - 1] == b'\n')
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
        let sections = parse_conflicts(content).unwrap();
        assert_eq!(sections[0].ours, b"ours\n");
        assert_eq!(sections[0].base.as_deref(), Some(b"base\n".as_slice()));
        assert_eq!(sections[0].theirs, b"theirs\n");
    }
}
