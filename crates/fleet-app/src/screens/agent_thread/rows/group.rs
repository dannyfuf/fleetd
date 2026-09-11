//! Bucketing a run of adjacent settled work rows into one generated summary (`spec-B` §B1.2).
//!
//! The copy itself lives in the kit ([`format_group_summary`]); this module decides which
//! entries belong to a run and what each one counts as. Two rules matter and both are tested:
//!
//! - **A single visible tool call is not grouped** — it renders as a plain work row — *except*
//!   for edits, which always go through the summarizer so `changed 3 files` wins over one file
//!   name.
//! - **Group failure is neutral by default.** A group holding one failure and three successes
//!   draws no danger mark; only the **latest** entry failing marks the group. The failure still
//!   reaches the accessibility label.

use fleet_core::agents::{Item, ItemKind, ItemStatus, ToolKind};
use fleet_ui_kit::{Icon, ToolGroupCounts, WorkGroupRow, format_group_summary};
use gpui::SharedString;

/// How one entry of a run counts towards its summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// `read N files`.
    Read,
    /// `changed N files`, counted over **distinct** paths.
    Edit,
    /// `ran N commands`.
    Command,
    /// `searched the web N times`.
    WebSearch,
    /// `searched code N times`.
    CodeSearch,
    /// `used N subagents`.
    Agent,
    /// `used N tools`.
    Other,
}

/// What one item's action is, for the purposes of the summary.
pub(crate) fn action_of(item: &Item) -> Action {
    let ItemKind::Tool(call) = &item.kind else {
        return Action::Agent;
    };
    match &call.kind {
        ToolKind::Read => Action::Read,
        ToolKind::Edit | ToolKind::Write => Action::Edit,
        ToolKind::Bash => Action::Command,
        ToolKind::Fetch | ToolKind::Search => Action::WebSearch,
        ToolKind::Grep => Action::CodeSearch,
        ToolKind::Agent => Action::Agent,
        ToolKind::Todo | ToolKind::Skill | ToolKind::Mcp { .. } | ToolKind::Unknown { .. } => {
            Action::Other
        }
    }
}

/// Whether a run of these entries must be summarized even though it holds only one.
///
/// An edit does, because one file name is less useful than `changed 1 file` beside the rest of
/// the turn's history — and because the path is already on the row it would otherwise replace.
pub(crate) fn summarizes_alone(items: &[&Item]) -> bool {
    items.len() == 1 && items.iter().all(|item| action_of(item) == Action::Edit)
}

/// The counts a run contributes, with MCP servers hoisted and changed paths deduped.
pub(crate) fn counts(items: &[&Item]) -> ToolGroupCounts {
    let mut counts = ToolGroupCounts::default();
    let mut changed: Vec<String> = Vec::new();
    for item in items {
        if let ItemKind::Tool(call) = &item.kind
            && let ToolKind::Mcp { server } = &call.kind
        {
            let server = SharedString::new(server.as_str());
            if !counts.mcp_servers.contains(&server) {
                counts.mcp_servers.push(server);
            }
            continue;
        }
        match action_of(item) {
            Action::Read => counts.read += 1,
            Action::Edit => {
                // Distinct paths, plus the edits that carried no file detail at all.
                let path = match &item.kind {
                    ItemKind::Tool(call) => call
                        .diff
                        .as_ref()
                        .map(|diff| diff.path.display().to_string())
                        .or_else(|| {
                            call.input
                                .get("file_path")
                                .and_then(|value| value.as_str())
                                .map(str::to_owned)
                        }),
                    _ => None,
                };
                match path {
                    Some(path) if changed.contains(&path) => {}
                    Some(path) => {
                        changed.push(path);
                        counts.changed += 1;
                    }
                    None => counts.changed += 1,
                }
            }
            Action::Command => counts.commands += 1,
            Action::WebSearch => counts.web_searches += 1,
            Action::CodeSearch => counts.code_searches += 1,
            Action::Agent => counts.subagents += 1,
            Action::Other => counts.other += 1,
        }
    }
    counts
}

/// The group row for a run, or `None` when it has nothing to say.
pub(crate) fn row(items: &[&Item], expanded: bool) -> Option<WorkGroupRow> {
    let counts = counts(items);
    let summary = format_group_summary(&counts)?;
    Some(WorkGroupRow {
        summary,
        icon: icon_for(items),
        hidden: items.len(),
        expanded,
        latest_failed: items
            .last()
            .is_some_and(|item| item.status == ItemStatus::Failed),
    })
}

/// The leading glyph: the kind of the run's first entry, which is what a reader scans for.
fn icon_for(items: &[&Item]) -> Icon {
    items
        .first()
        .and_then(|item| match &item.kind {
            ItemKind::Tool(call) => Some(super::item::tool_icon(&call.kind)),
            _ => None,
        })
        .unwrap_or(Icon::Wrench)
}
