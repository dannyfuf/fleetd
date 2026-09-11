//! The generated summary of a run of adjacent settled tool rows.
//!
//! `spec-B` §B1.2: a group of settled work collapses to one toggle row whose label is bucketed
//! by action — `read 3 files and ran 2 commands` — and named MCP servers hoist to the front.
//! The copy is generated here, once, so the projection hands over counts and the row draws a
//! sentence. Fleet lowercases where t3code sentence-cases: the transcript's voice is the
//! terminal's.

use gpui::SharedString;

/// How many calls of each action a group holds.
///
/// A count of zero contributes no clause, so a group of three reads and one command states
/// exactly two of them. `changed` counts **distinct** changed paths plus the edits that carried
/// no file detail, which is why `changed 3 files` wins over one file name (§B1.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolGroupCounts {
    /// Files read.
    pub read: usize,
    /// Distinct paths changed.
    pub changed: usize,
    /// Commands run.
    pub commands: usize,
    /// Web searches.
    pub web_searches: usize,
    /// Code searches.
    pub code_searches: usize,
    /// Subagents used.
    pub subagents: usize,
    /// Anything unmapped.
    pub other: usize,
    /// The MCP servers this group called, in first-seen order and already deduped.
    pub mcp_servers: Vec<SharedString>,
}

impl ToolGroupCounts {
    /// How many calls the group holds in total, MCP calls included.
    #[must_use]
    pub fn total(&self) -> usize {
        self.read
            + self.changed
            + self.commands
            + self.web_searches
            + self.code_searches
            + self.subagents
            + self.other
            + self.mcp_servers.len()
    }

    /// Whether the group has nothing to say.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// `N file` / `N files`.
fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

/// Lowercase the first character of a clause that is not the first in the sentence.
///
/// Fleet's own clauses are already lowercase; this exists so a hoisted MCP phrase or a future
/// caller-supplied clause cannot capitalise mid-sentence.
fn lowercase_first(clause: &str) -> String {
    let mut chars = clause.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Join clauses the way English does: `a`, `a and b`, `a, b, and c`.
fn join(clauses: &[String]) -> String {
    match clauses {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {}", lowercase_first(second)),
        [first, middle @ .., last] => {
            let mut text = first.clone();
            for clause in middle {
                text.push_str(", ");
                text.push_str(&lowercase_first(clause));
            }
            text.push_str(", and ");
            text.push_str(&lowercase_first(last));
            text
        }
    }
}

/// The MCP clause: `used github and linear integrations`.
fn mcp_clause(servers: &[SharedString]) -> Option<String> {
    let names: Vec<String> = servers.iter().map(|name| name.to_string()).collect();
    if names.is_empty() {
        return None;
    }
    let noun = if names.len() == 1 {
        "integration"
    } else {
        "integrations"
    };
    Some(format!("used {} {noun}", join(&names)))
}

/// The group's one-line summary, or `None` when the group holds nothing.
///
/// Named MCP servers hoist to the front, because "which integration did it call" is the thing a
/// reader scans a group header for.
#[must_use]
pub fn format_group_summary(counts: &ToolGroupCounts) -> Option<SharedString> {
    let mut clauses: Vec<String> = Vec::new();
    clauses.extend(mcp_clause(&counts.mcp_servers));
    for (count, verb, noun) in [
        (counts.read, "read", "file"),
        (counts.changed, "changed", "file"),
        (counts.commands, "ran", "command"),
        (counts.web_searches, "searched the web", "time"),
        (counts.code_searches, "searched code", "time"),
        (counts.subagents, "used", "subagent"),
        (counts.other, "used", "tool"),
    ] {
        if count > 0 {
            clauses.push(format!("{verb} {}", plural(count, noun)));
        }
    }
    if clauses.is_empty() {
        return None;
    }
    Some(SharedString::from(join(&clauses)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts() -> ToolGroupCounts {
        ToolGroupCounts::default()
    }

    #[test]
    fn one_bucket_states_itself() {
        let summary = format_group_summary(&ToolGroupCounts {
            read: 3,
            ..counts()
        });
        assert_eq!(summary.as_deref(), Some("read 3 files"));
    }

    #[test]
    fn a_single_call_keeps_the_singular() {
        let summary = format_group_summary(&ToolGroupCounts {
            commands: 1,
            ..counts()
        });
        assert_eq!(summary.as_deref(), Some("ran 1 command"));
    }

    #[test]
    fn two_buckets_join_with_and() {
        let summary = format_group_summary(&ToolGroupCounts {
            read: 3,
            commands: 2,
            ..counts()
        });
        assert_eq!(summary.as_deref(), Some("read 3 files and ran 2 commands"));
    }

    #[test]
    fn three_buckets_take_the_serial_comma() {
        let summary = format_group_summary(&ToolGroupCounts {
            read: 3,
            changed: 1,
            commands: 2,
            ..counts()
        });
        assert_eq!(
            summary.as_deref(),
            Some("read 3 files, changed 1 file, and ran 2 commands")
        );
    }

    #[test]
    fn named_servers_hoist_to_the_front() {
        let summary = format_group_summary(&ToolGroupCounts {
            read: 1,
            mcp_servers: vec!["github".into(), "linear".into()],
            ..counts()
        });
        assert_eq!(
            summary.as_deref(),
            Some("used github and linear integrations and read 1 file")
        );
    }

    #[test]
    fn subagents_and_unmapped_tools_are_different_nouns() {
        assert_eq!(
            format_group_summary(&ToolGroupCounts {
                subagents: 2,
                ..counts()
            })
            .as_deref(),
            Some("used 2 subagents")
        );
        assert_eq!(
            format_group_summary(&ToolGroupCounts {
                other: 2,
                ..counts()
            })
            .as_deref(),
            Some("used 2 tools")
        );
    }

    #[test]
    fn an_empty_group_has_nothing_to_say() {
        assert!(format_group_summary(&counts()).is_none());
        assert!(counts().is_empty());
    }

    #[test]
    fn the_total_counts_every_bucket_including_integrations() {
        let counts = ToolGroupCounts {
            read: 1,
            changed: 2,
            commands: 3,
            web_searches: 1,
            code_searches: 1,
            subagents: 1,
            other: 1,
            mcp_servers: vec!["github".into()],
        };
        assert_eq!(counts.total(), 11);
    }
}
