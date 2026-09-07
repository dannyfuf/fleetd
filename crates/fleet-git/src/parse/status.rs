//! Porcelain-v1 `-z` status parsing.

use std::path::PathBuf;

use crate::{ChangeKind, ConflictKind, FileStatus, GitError, Result};

/// Parses `git status --porcelain=v1 -z` bytes.
pub fn parse(input: &[u8]) -> Result<Vec<FileStatus>> {
    let fields: Vec<&[u8]> = input.split(|byte| *byte == 0).collect();
    let mut files = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        if field.is_empty() {
            index += 1;
            continue;
        }
        if field.len() < 3 || field[2] != b' ' {
            return Err(GitError::parse(
                "porcelain status",
                format!("invalid record at byte field {index}"),
            ));
        }
        let x = field[0];
        let y = field[1];
        let path = bytes_to_path(&field[3..]);
        let renamed = matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C');
        let previous_path = if renamed {
            index += 1;
            let old = fields
                .get(index)
                .filter(|old| !old.is_empty())
                .ok_or_else(|| {
                    GitError::parse("porcelain status", "rename/copy record has no source path")
                })?;
            Some(bytes_to_path(old))
        } else {
            None
        };
        files.push(FileStatus {
            path,
            previous_path,
            index: change(x),
            worktree: change(y),
            conflict: conflict(x, y),
        });
        index += 1;
    }
    Ok(files)
}

fn change(value: u8) -> ChangeKind {
    match value {
        b' ' => ChangeKind::Unmodified,
        b'A' => ChangeKind::Added,
        b'M' => ChangeKind::Modified,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        b'T' => ChangeKind::TypeChanged,
        b'?' => ChangeKind::Untracked,
        b'!' => ChangeKind::Ignored,
        b'U' => ChangeKind::Unmerged,
        other => ChangeKind::Unknown(other),
    }
}

fn conflict(x: u8, y: u8) -> Option<ConflictKind> {
    match (x, y) {
        (b'A', b'A') => Some(ConflictKind::BothAdded),
        (b'U', b'U') => Some(ConflictKind::BothModified),
        (b'D', b'D') => Some(ConflictKind::BothDeleted),
        (b'A', b'U') => Some(ConflictKind::AddedByUs),
        (b'U', b'A') => Some(ConflictKind::DeletedByUs),
        (b'D', b'U') => Some(ConflictKind::DeletedByUsModifiedByThem),
        (b'U', b'D') => Some(ConflictKind::ModifiedByUsDeletedByThem),
        (b'U', _) | (_, b'U') => Some(ConflictKind::Other),
        _ => None,
    }
}

pub(super) fn bytes_to_path(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        PathBuf::from(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::{ChangeKind, ConflictKind};

    #[test]
    fn parses_spaces_tabs_renames_copies_and_non_utf8_paths() {
        let input: &[u8] = b" M space name\x00R  new\tname\x00old name\x00C  copy\x00source\x00?? bad-\xff\x00!! ignored\x00 D gone\x00MM both sides\x00";
        let files = parse(input).expect("parse status");
        assert_eq!(files.len(), 7);

        assert_eq!(files[0].path.to_string_lossy(), "space name");
        assert_eq!(files[0].index, ChangeKind::Unmodified);
        assert_eq!(files[0].worktree, ChangeKind::Modified);

        assert_eq!(files[1].index, ChangeKind::Renamed);
        assert_eq!(files[1].path.to_string_lossy(), "new\tname");
        assert_eq!(
            files[1].previous_path.as_ref().unwrap().to_string_lossy(),
            "old name"
        );

        assert_eq!(files[2].index, ChangeKind::Copied);
        assert_eq!(
            files[2].previous_path.as_ref().unwrap().to_string_lossy(),
            "source"
        );

        assert_eq!(files[3].index, ChangeKind::Untracked);
        assert_eq!(files[3].worktree, ChangeKind::Untracked);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(files[3].path.as_os_str().as_bytes(), b"bad-\xff");
        }

        assert_eq!(files[4].index, ChangeKind::Ignored);
        assert_eq!(files[5].worktree, ChangeKind::Deleted);
        assert_eq!(files[6].index, ChangeKind::Modified);
        assert_eq!(files[6].worktree, ChangeKind::Modified);
        assert!(files.iter().all(|file| file.conflict.is_none()));
    }

    #[test]
    fn classifies_every_unmerged_combination() {
        let input: &[u8] = b"AA a\x00UU b\x00DD c\x00AU d\x00UA e\x00DU f\x00UD g\x00";
        let files = parse(input).expect("parse conflicts");
        let kinds: Vec<_> = files.iter().map(|file| file.conflict.unwrap()).collect();
        assert_eq!(
            kinds,
            vec![
                ConflictKind::BothAdded,
                ConflictKind::BothModified,
                ConflictKind::BothDeleted,
                ConflictKind::AddedByUs,
                ConflictKind::DeletedByUs,
                ConflictKind::DeletedByUsModifiedByThem,
                ConflictKind::ModifiedByUsDeletedByThem,
            ]
        );
    }

    #[test]
    fn empty_input_yields_no_records_and_short_records_error() {
        assert!(parse(b"").expect("empty").is_empty());
        assert!(parse(b"XY\x00").is_err());
        assert!(parse(b"R  only-destination\x00").is_err());
    }
}
