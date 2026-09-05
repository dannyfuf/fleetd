//! Fixed-field NUL ref parsing.

use std::collections::BTreeMap;

use crate::{Branch, GitError, ObjectId, RemoteBranch, RemoteBranchGroup, Result, Tag, Upstream};

/// Parses local branch records emitted by the crate's `for-each-ref` format.
pub fn local_branches(input: &[u8]) -> Result<Vec<Branch>> {
    records(input, 7, "local branches")?
        .into_iter()
        .map(|fields| {
            let tracking = text(fields[2]);
            let (ahead, behind) = parse_tracking(text(fields[3]));
            Ok(Branch {
                is_head: text(fields[0]) == "*",
                name: text(fields[1]).to_owned(),
                upstream: if tracking.is_empty() {
                    None
                } else {
                    Some(Upstream {
                        name: tracking.to_owned(),
                        ahead,
                        behind,
                    })
                },
                subject: text(fields[4]).to_owned(),
                oid: ObjectId(text(fields[5]).to_owned()),
                committed_at: number(fields[6], "branch timestamp")?,
                checked_out_at: None,
            })
        })
        .collect()
}

/// Parses and groups remote-tracking branch records.
pub fn remote_branches(input: &[u8]) -> Result<Vec<RemoteBranchGroup>> {
    let mut groups: BTreeMap<String, Vec<RemoteBranch>> = BTreeMap::new();
    for fields in records(input, 5, "remote branches")? {
        let name = text(fields[0]).to_owned();
        if name.ends_with("/HEAD") {
            continue;
        }
        let Some((remote, branch)) = name.split_once('/') else {
            continue;
        };
        groups
            .entry(remote.to_owned())
            .or_default()
            .push(RemoteBranch {
                name: name.clone(),
                branch: branch.to_owned(),
                oid: ObjectId(text(fields[1]).to_owned()),
                subject: text(fields[2]).to_owned(),
                committed_at: number(fields[3], "remote branch timestamp")?,
            });
    }
    Ok(groups
        .into_iter()
        .map(|(remote, branches)| RemoteBranchGroup { remote, branches })
        .collect())
}

/// Parses tag records.
pub fn tags(input: &[u8]) -> Result<Vec<Tag>> {
    records(input, 4, "tags")?
        .into_iter()
        .map(|fields| {
            Ok(Tag {
                name: text(fields[0]).to_owned(),
                oid: ObjectId(text(fields[1]).to_owned()),
                created_at: number_or_zero(fields[2]),
                subject: text(fields[3]).to_owned(),
            })
        })
        .collect()
}

/// Splits `for-each-ref` output into fixed-width records.
///
/// Every record the crate requests ends with a trailing `%00` and Git appends a
/// newline per ref, so records are newline-delimited and fields NUL-delimited.
/// No requested placeholder can contain a newline, which makes the framing exact.
fn records<'a>(input: &'a [u8], width: usize, context: &'static str) -> Result<Vec<Vec<&'a [u8]>>> {
    let mut records = Vec::new();
    for line in input.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut fields: Vec<&[u8]> = line.split(|byte| *byte == 0).collect();
        if fields.last().is_some_and(|field| field.is_empty()) {
            fields.pop();
        }
        if fields.len() != width {
            return Err(GitError::parse(
                context,
                format!("expected {width} fields, got {}", fields.len()),
            ));
        }
        records.push(fields);
    }
    Ok(records)
}

fn parse_tracking(value: &str) -> (usize, usize) {
    let mut ahead = 0;
    let mut behind = 0;
    for component in value.trim_matches(['[', ']']).split(',') {
        let component = component.trim();
        if let Some(value) = component.strip_prefix("ahead ") {
            ahead = value.parse().unwrap_or(0);
        } else if let Some(value) = component.strip_prefix("behind ") {
            behind = value.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or_default()
}

fn number(bytes: &[u8], context: &'static str) -> Result<i64> {
    text(bytes)
        .parse()
        .map_err(|error| GitError::parse(context, format!("{}: {error}", text(bytes))))
}

fn number_or_zero(bytes: &[u8]) -> i64 {
    text(bytes).parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{local_branches, remote_branches, tags};

    #[test]
    fn parses_local_branches_with_and_without_tracking() {
        let input: &[u8] = b"*\x00main\x00origin/main\x00[ahead 2, behind 1]\x00tip subject\x00aaaaaaa\x001700000000\x00\n \x00topic\x00\x00\x00other subject\x00bbbbbbb\x001700000001\x00\n \x00gone\x00origin/gone\x00[gone]\x00third\x00ccccccc\x001700000002\x00\n";
        let branches = local_branches(input).expect("parse branches");
        assert_eq!(branches.len(), 3);

        assert!(branches[0].is_head);
        assert_eq!(branches[0].name, "main");
        let upstream = branches[0].upstream.as_ref().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!((upstream.ahead, upstream.behind), (2, 1));
        assert_eq!(branches[0].subject, "tip subject");
        assert_eq!(branches[0].committed_at, 1_700_000_000);

        assert!(!branches[1].is_head);
        assert!(branches[1].upstream.is_none());

        let gone = branches[2].upstream.as_ref().unwrap();
        assert_eq!((gone.ahead, gone.behind), (0, 0));
    }

    #[test]
    fn groups_remote_branches_and_skips_symbolic_head() {
        let input: &[u8] = b"origin/main\x00aaa\x00subject\x001\x00\x00\norigin/topic\x00bbb\x00other\x002\x00\x00\nupstream/main\x00ccc\x00third\x003\x00\x00\norigin/HEAD\x00aaa\x00\x000\x00refs/remotes/origin/main\x00\n";
        let groups = remote_branches(input).expect("parse remotes");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].remote, "origin");
        assert_eq!(groups[0].branches.len(), 2);
        assert_eq!(groups[0].branches[0].branch, "main");
        assert_eq!(groups[0].branches[0].name, "origin/main");
        assert_eq!(groups[1].remote, "upstream");
    }

    #[test]
    fn parses_annotated_and_lightweight_tags() {
        let input: &[u8] = b"v1\x00aaa\x001700000000\x00release one\x00\nv2\x00\x00\x00\x00\n";
        let parsed = tags(input).expect("parse tags");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "v1");
        assert_eq!(parsed[0].oid.as_str(), "aaa");
        assert_eq!(parsed[0].created_at, 1_700_000_000);
        // A lightweight tag has no peeled object; the reader fills it in later.
        assert_eq!(parsed[1].oid.as_str(), "");
        assert_eq!(parsed[1].created_at, 0);
    }

    #[test]
    fn rejects_truncated_records() {
        assert!(local_branches(b"*\x00main\x00\n").is_err());
        assert!(tags(b"").expect("empty tags").is_empty());
    }
}
