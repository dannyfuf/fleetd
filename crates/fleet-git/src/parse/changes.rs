//! `git diff --numstat -z` and the Changes panel's `git log -z` records.

use std::path::PathBuf;

use crate::{AheadCommit, GitError, ObjectId, Result, parse::status::bytes_to_path};

/// One `--numstat` record: the destination path, then lines added and removed. Both counts are
/// `None` for a binary file, which git reports as `-\t-`.
pub(crate) type NumstatRecord = (PathBuf, Option<u64>, Option<u64>);

/// Parses `git diff --numstat -z`.
///
/// A plain record is `added\tremoved\tpath\0`. A rename or copy leaves the path empty and
/// follows with two fields, `old\0new\0`; the new path is the one recorded.
pub(crate) fn numstat(input: &[u8]) -> Result<Vec<NumstatRecord>> {
    let mut fields = input.split(|byte| *byte == 0);
    let mut records = Vec::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, |byte| *byte == b'\t');
        let added = count(parts.next())?;
        let removed = count(parts.next())?;
        let path = parts
            .next()
            .ok_or_else(|| GitError::parse("numstat", "record is missing its path"))?;
        let path = if path.is_empty() {
            let _previous = fields
                .next()
                .ok_or_else(|| GitError::parse("numstat", "rename is missing its source"))?;
            fields
                .next()
                .ok_or_else(|| GitError::parse("numstat", "rename is missing its destination"))?
        } else {
            path
        };
        records.push((bytes_to_path(path), added, removed));
    }
    Ok(records)
}

fn count(field: Option<&[u8]>) -> Result<Option<u64>> {
    let field = field.ok_or_else(|| GitError::parse("numstat", "record is missing a count"))?;
    if field == b"-" {
        return Ok(None);
    }
    std::str::from_utf8(field)
        .ok()
        .and_then(|text| text.parse().ok())
        .map(Some)
        .ok_or_else(|| GitError::parse("numstat", "count is not a number"))
}

/// Parses `git log -z --format=%H%x1f%s`: one `oid\x1fsubject` record per NUL.
pub(crate) fn ahead_commits(input: &[u8]) -> Result<Vec<AheadCommit>> {
    input
        .split(|byte| *byte == 0)
        .map(|record| record.strip_prefix(b"\n").unwrap_or(record))
        .filter(|record| !record.is_empty())
        .map(|record| {
            let at = record
                .iter()
                .position(|byte| *byte == 0x1f)
                .ok_or_else(|| GitError::parse("ahead commits", "record has no subject"))?;
            Ok(AheadCommit {
                oid: ObjectId(super::text(&record[..at]).trim().to_owned()),
                subject: super::text(&record[at + 1..]).into_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_reads_plain_binary_and_renamed_records() {
        let input = b"2\t1\tREADME.md\0-\t-\tlogo.png\x000\t0\t\0old name.rs\0new name.rs\0";
        let records = numstat(input).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            records,
            vec![
                (PathBuf::from("README.md"), Some(2), Some(1)),
                (PathBuf::from("logo.png"), None, None),
                (PathBuf::from("new name.rs"), Some(0), Some(0)),
            ]
        );
    }

    #[test]
    fn numstat_refuses_a_count_that_is_not_a_number() {
        assert!(numstat(b"x\t1\tREADME.md\0").is_err());
    }

    #[test]
    fn ahead_commits_keep_subjects_whole() {
        let input = b"3a1279e\x1fmark a card's run\0\nf3481f8\x1fdrive board workflows\0";
        let commits = ahead_commits(input).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].oid.0, "3a1279e");
        assert_eq!(commits[0].subject, "mark a card's run");
        assert_eq!(commits[1].subject, "drive board workflows");
    }
}
