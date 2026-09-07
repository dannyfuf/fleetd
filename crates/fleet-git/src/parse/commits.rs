//! Fixed-field commit, reflog, and stash parsing.

use crate::{Commit, GitError, ObjectId, ReflogEntry, Result, StashEntry};

/// Parses commit records from the crate's pretty format.
pub fn commits(input: &[u8]) -> Result<Vec<Commit>> {
    split_records(input, 9, "commit log")?
        .into_iter()
        .map(|fields| {
            Ok(Commit {
                oid: ObjectId(crate::parse::text(fields[0]).into_owned()),
                parents: ids(fields[1]),
                author_name: crate::parse::text(fields[2]).into_owned(),
                author_email: crate::parse::text(fields[3]).into_owned(),
                authored_at: number(fields[4], "author timestamp")?,
                committed_at: number(fields[5], "committer timestamp")?,
                subject: crate::parse::text(fields[6]).into_owned(),
                body: crate::parse::text(fields[7]).into_owned(),
                decorations: crate::parse::text(fields[8])
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect(),
                pushed: false,
            })
        })
        .collect()
}

/// Parses HEAD reflog records.
pub fn reflog(input: &[u8]) -> Result<Vec<ReflogEntry>> {
    split_records(input, 5, "reflog")?
        .into_iter()
        .map(|fields| {
            Ok(ReflogEntry {
                oid: ObjectId(crate::parse::text(fields[0]).into_owned()),
                parents: ids(fields[1]),
                selector: crate::parse::text(fields[2]).into_owned(),
                subject: crate::parse::text(fields[3]).into_owned(),
                committed_at: number(fields[4], "reflog timestamp")?,
            })
        })
        .collect()
}

/// Parses stash records.
pub fn stashes(input: &[u8]) -> Result<Vec<StashEntry>> {
    split_records(input, 4, "stash list")?
        .into_iter()
        .map(|fields| {
            let selector = crate::parse::text(fields[0]);
            let index = selector
                .strip_prefix("stash@{")
                .and_then(|value| value.strip_suffix('}'))
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| {
                    GitError::parse("stash list", format!("invalid selector {selector}"))
                })?;
            Ok(StashEntry {
                index,
                oid: ObjectId(crate::parse::text(fields[1]).into_owned()),
                created_at: number(fields[2], "stash timestamp")?,
                subject: crate::parse::text(fields[3]).into_owned(),
            })
        })
        .collect()
}

fn split_records<'a>(
    input: &'a [u8],
    width: usize,
    context: &'static str,
) -> Result<Vec<Vec<&'a [u8]>>> {
    let mut cursor = 0;
    let mut records = Vec::new();
    while cursor < input.len() {
        while input
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b'\n' | 0))
        {
            cursor += 1;
        }
        if cursor == input.len() {
            break;
        }
        if input[cursor] != 0x1e {
            return Err(GitError::parse(context, "record is missing its prefix"));
        }
        cursor += 1;
        let mut fields = Vec::with_capacity(width);
        for _ in 0..width {
            let end = input[cursor..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|offset| cursor + offset)
                .ok_or_else(|| GitError::parse(context, "record is missing a field terminator"))?;
            fields.push(&input[cursor..end]);
            cursor = end + 1;
        }
        records.push(fields);
    }
    Ok(records)
}

fn ids(bytes: &[u8]) -> Vec<ObjectId> {
    crate::parse::text(bytes)
        .split_whitespace()
        .map(ObjectId::from)
        .collect()
}

fn number(bytes: &[u8], context: &'static str) -> Result<i64> {
    let text = crate::parse::text(bytes);
    text.parse()
        .map_err(|error| GitError::parse(context, format!("{text}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::{commits, reflog, stashes};

    #[test]
    fn message_may_contain_record_separator() {
        let input: &[u8] = b"\x1eaaa\x00\x00Ada\x00ada@example.test\x001\x002\x00subject\x00body before \x1e body after\x00\x00\n\x1ebbb\x00\x00Grace\x00grace@example.test\x003\x004\x00next\x00\x00\x00";
        let parsed = commits(input).expect("parse commits containing record separator");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].body, "body before \u{1e} body after");
        assert_eq!(parsed[1].oid.as_str(), "bbb");
    }

    #[test]
    fn invalid_utf8_identity_not_erased() {
        let input: &[u8] =
            b"\x1eaaa\x00\x00A\xffda\x00ada@example.test\x001\x002\x00subject\x00\x00\x00";
        let parsed = commits(input).expect("parse invalid UTF-8 identity");

        assert!(!parsed[0].author_name.is_empty());
        assert!(parsed[0].author_name.contains("\\xff"));
    }

    #[test]
    fn parses_commit_records_with_bodies_and_decorations() {
        let input: &[u8] = b"\x1eaaa\x00bbb ccc\x00Ada Lovelace\x00ada@example.test\x001700000000\x001700000001\x00subject line\x00body line one\nbody line two\n\x00HEAD -> main, tag: v1, origin/main\x00\n\x1ebbb\x00\x00Grace\x00grace@example.test\x001600000000\x001600000000\x00root commit\x00\x00\x00";
        let parsed = commits(input).expect("parse commits");
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].oid.as_str(), "aaa");
        assert_eq!(parsed[0].parents.len(), 2);
        assert_eq!(parsed[0].author_name, "Ada Lovelace");
        assert_eq!(parsed[0].author_email, "ada@example.test");
        assert_eq!(parsed[0].authored_at, 1_700_000_000);
        assert_eq!(parsed[0].committed_at, 1_700_000_001);
        assert_eq!(parsed[0].subject, "subject line");
        assert!(parsed[0].body.contains("body line two"));
        assert_eq!(
            parsed[0].decorations,
            vec!["HEAD -> main", "tag: v1", "origin/main"]
        );
        assert!(!parsed[0].pushed);

        // A root commit has no parents, no body, and no decorations.
        assert!(parsed[1].parents.is_empty());
        assert_eq!(parsed[1].body, "");
        assert!(parsed[1].decorations.is_empty());
    }

    #[test]
    fn parses_reflog_selectors_and_actions() {
        let input: &[u8] = b"\x1eaaa\x00bbb\x00HEAD@{0}\x00commit: subject\x001700000000\x00\n\x1ebbb\x00\x00HEAD@{1}\x00rebase (start): checkout main\x001699999999\x00";
        let parsed = reflog(input).expect("parse reflog");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].selector, "HEAD@{0}");
        assert_eq!(parsed[0].subject, "commit: subject");
        assert_eq!(parsed[1].parents.len(), 0);
    }

    #[test]
    fn parses_stash_indexes_out_of_order() {
        let input: &[u8] = b"\x1estash@{0}\x00aaa\x001700000000\x00On main: work\x00\n\x1estash@{3}\x00bbb\x001600000000\x00WIP on topic: 1234567 subject\x00";
        let parsed = stashes(input).expect("parse stashes");
        assert_eq!(parsed[0].index, 0);
        assert_eq!(parsed[0].subject, "On main: work");
        assert_eq!(parsed[1].index, 3);
        assert_eq!(parsed[1].oid.as_str(), "bbb");
    }

    #[test]
    fn tolerates_the_extra_nul_terminator_of_stash_list_z() {
        // `git stash list -z` terminates each record with NUL instead of newline,
        // which adds one empty field beyond the format's own trailing `%x00`.
        let input: &[u8] = b"\x1estash@{0}\x00aaa\x001700000000\x00On main: work\x00\x00";
        let parsed = stashes(input).expect("parse stashes");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].subject, "On main: work");
    }

    #[test]
    fn keeps_a_genuinely_empty_trailing_field() {
        let input: &[u8] =
            b"\x1eaaa\x00\x00Grace\x00grace@example.test\x001\x002\x00subject\x00\x00\x00";
        let parsed = commits(input).expect("parse commits");
        assert_eq!(parsed[0].body, "");
        assert!(parsed[0].decorations.is_empty());
    }

    #[test]
    fn rejects_malformed_records_and_accepts_empty_input() {
        assert!(commits(b"").expect("empty").is_empty());
        assert!(commits(b"\x1eaaa\x00bbb\x00").is_err());
        assert!(stashes(b"\x1enot-a-selector\x00aaa\x001\x00subject\x00").is_err());
    }
}
