//! The kind column, the one-line summary, and the diff a tool row renders.
//!
//! Claude hands Fleet structured tools (`Read`, `Grep`, `Edit`), so the kind column is a match on
//! `tool_name`; Codex funnels everything through the shell and hands back a `commandActions[]`
//! parse instead (§4.2). Both feed the same `ToolKind`.

use std::path::PathBuf;

use fleet_core::agents::{ToolDiff, ToolKind};
use serde_json::Value;

use super::text::compact_json;

/// The normalized kind column for a Claude tool name.
#[must_use]
pub(crate) fn tool_kind(name: &str) -> ToolKind {
    match name {
        "Read" => ToolKind::Read,
        "Edit" | "NotebookEdit" => ToolKind::Edit,
        "Write" => ToolKind::Write,
        "Bash" | "BashOutput" | "KillShell" => ToolKind::Bash,
        "Glob" | "WebSearch" | "ToolSearch" => ToolKind::Search,
        "Grep" => ToolKind::Grep,
        "WebFetch" => ToolKind::Fetch,
        "Task" | "Workflow" => ToolKind::Agent,
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" => ToolKind::Todo,
        "Skill" => ToolKind::Skill,
        _ if name.starts_with("mcp__") => {
            let server = name
                .trim_start_matches("mcp__")
                .split("__")
                .next()
                .unwrap_or("unknown")
                .to_owned();
            ToolKind::Mcp { server }
        }
        _ => ToolKind::Unknown {
            name: name.to_owned(),
        },
    }
}

pub(crate) fn tool_summary(name: &str, input: &Value, diff: Option<&ToolDiff>) -> String {
    let string = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| input.get(key).and_then(Value::as_str))
            .unwrap_or_default()
    };
    match name {
        "Read" => string(&["file_path", "path"]).to_owned(),
        "Bash" => string(&["command"]).to_owned(),
        "Edit" | "Write" | "NotebookEdit" => {
            let path = string(&["file_path", "notebook_path", "path"]);
            diff.map_or_else(
                || path.to_owned(),
                |diff| format!("{path} +{} −{}", diff.added, diff.removed),
            )
        }
        "Grep" | "Glob" => {
            let pattern = string(&["pattern"]);
            let path = string(&["path"]);
            if path.is_empty() {
                pattern.to_owned()
            } else {
                format!("{pattern} in {path}")
            }
        }
        "WebFetch" => string(&["url"]).to_owned(),
        "Task" | "Workflow" => string(&["description", "prompt"]).to_owned(),
        "TodoWrite" => format!(
            "{} items",
            input
                .get("todos")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default()
        ),
        "Skill" => string(&["skill", "name"]).to_owned(),
        // §2 gives every tool call one 30 px row with a *one-line* summary, and §4.1 turns the
        // `can_use_tool` for these two into the gate itself. The whole questions payload — or a
        // whole plan — as compact JSON is neither one line nor new information above the card
        // that already carries it.
        "AskUserQuestion" => question_headers(input),
        "ExitPlanMode" => "plan".to_owned(),
        _ if name.starts_with("mcp__") => string(&["description", "query", "url"]).to_owned(),
        _ => permission_payload(name, input),
    }
}

/// The question headers of an `AskUserQuestion` input, as one line.
pub(crate) fn question_headers(input: &Value) -> String {
    let headers = input
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| {
            questions
                .iter()
                .filter_map(|question| {
                    question
                        .get("header")
                        .or_else(|| question.get("question"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    if headers.is_empty() {
        "question".to_owned()
    } else {
        headers
    }
}

pub(crate) fn permission_payload(tool_name: &str, input: &Value) -> String {
    let summary = tool_summary_without_fallback(tool_name, input);
    if summary.is_empty() {
        compact_json(input)
    } else {
        summary
    }
}

/// How many diff lines a permission card shows before it elides the rest.
///
/// §2 gives the card a fixed 760 px measure; a `Write` of a whole file would otherwise push its
/// own keys off the bottom. The head of the diff is the part that identifies the change, and
/// the elision line says how much was left out rather than pretending there was no more.
pub(crate) const PERMISSION_DIFF_MAX_LINES: usize = 40;

pub(crate) fn tool_summary_without_fallback(tool_name: &str, input: &Value) -> String {
    // §3.2: "`payload` is the invocation itself, never the model's prose description of it,
    // because it is what the card shows literally". For a write the invocation is the path
    // *and* the bytes: a card naming only the destination asks the user to allow a change they
    // have not been shown, while the same card for `Bash` shows the whole command.
    if matches!(tool_name, "Edit" | "Write" | "NotebookEdit") {
        return write_payload(tool_name, input);
    }
    let key = match tool_name {
        "Bash" => "command",
        "Read" => "file_path",
        "Grep" | "Glob" => "pattern",
        "WebFetch" => "url",
        "Task" => "description",
        "Skill" => "skill",
        _ => return String::new(),
    };
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The destination path plus the change itself, as the permission card reads it out.
pub(crate) fn write_payload(tool_name: &str, input: &Value) -> String {
    let path = ["file_path", "notebook_path", "path"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .unwrap_or_default();
    let Some(diff) = synthetic_diff(path, tool_name, input, &Value::Null) else {
        return path.to_owned();
    };
    // The `--- a/… +++ b/…` header repeats the path the first line already carries, and the
    // hunk header is for a diff parser, not for a reader deciding whether to allow this.
    let body = diff
        .lines()
        .skip_while(|line| {
            line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ")
        })
        .collect::<Vec<_>>();
    if body.is_empty() {
        return path.to_owned();
    }
    let elided = body.len().saturating_sub(PERMISSION_DIFF_MAX_LINES);
    let mut payload = String::from(path);
    for line in body.iter().take(PERMISSION_DIFF_MAX_LINES) {
        payload.push('\n');
        payload.push_str(line);
    }
    if elided > 0 {
        payload.push_str(&format!("\n… {elided} more lines"));
    }
    payload
}

pub(crate) fn derive_tool_diff(
    name: &str,
    input: &Value,
    result: Option<&Value>,
) -> Option<ToolDiff> {
    if !matches!(name, "Edit" | "Write" | "NotebookEdit") {
        return None;
    }
    let result = result.unwrap_or(&Value::Null);
    let path = result
        .get("filePath")
        .or_else(|| result.get("file_path"))
        .or_else(|| result.get("path"))
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("notebook_path"))
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)?;
    let unified = result
        .get("unified_diff")
        .or_else(|| result.get("diff"))
        .or_else(|| result.get("patch"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| structured_patch(path, result))
        .or_else(|| synthetic_diff(path, name, input, result))?;
    let (added, removed) = diff_counts(&unified);
    Some(ToolDiff {
        path: PathBuf::from(path),
        added,
        removed,
        unified,
    })
}

pub(crate) fn structured_patch(path: &str, result: &Value) -> Option<String> {
    let hunks = result.get("structuredPatch")?.as_array()?;
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n");
    for hunk in hunks {
        let old_start = hunk
            .get("oldStart")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let old_lines = hunk
            .get("oldLines")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let new_start = hunk
            .get("newStart")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let new_lines = hunk
            .get("newLines")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        diff.push_str(&format!(
            "@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n"
        ));
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            diff.push_str(line);
            diff.push('\n');
        }
    }
    (diff.lines().count() > 2).then_some(diff)
}

/// A unified diff for an `Edit`/`Write` whose result carried no `structuredPatch`.
///
/// The hunk header is written in full. Both readers of this text — `fleet_git::parse::diff` and
/// `fleet_lazygit::diff_view::hunk_header` — require `@@ <ranges> @@`, and a bare `@@` parses as
/// a file with zero hunks: §5's inline diff under the row would then render empty for every
/// Claude `Write`. The paths are relative (`a/…`), never `a//abs/path`.
pub(crate) fn synthetic_diff(
    path: &str,
    name: &str,
    input: &Value,
    result: &Value,
) -> Option<String> {
    let old = if name == "Write" {
        ""
    } else {
        input
            .get("old_string")
            .or_else(|| result.get("oldString"))?
            .as_str()?
    };
    let new = input
        .get("new_string")
        .or_else(|| input.get("content"))
        .or_else(|| result.get("newString"))?
        .as_str()?;
    let relative = path.trim_start_matches('/');
    let old_count = old.lines().count();
    let new_count = new.lines().count();
    // An empty side occupies no line, and unified diffs spell that start as 0.
    let old_start = usize::from(old_count > 0);
    let new_start = usize::from(new_count > 0);
    let mut diff = format!(
        "--- a/{relative}\n+++ b/{relative}\n@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
    );
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    Some(diff)
}

/// The added and removed line counts of a unified diff.
pub(crate) fn diff_counts(unified: &str) -> (u64, u64) {
    let added = unified
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count() as u64;
    let removed = unified
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count() as u64;
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_summary_shows_the_change_and_not_only_its_destination() {
        let input = serde_json::json!({"file_path": "/w/note.txt", "content": "one\ntwo"});
        let payload = permission_payload("Write", &input);
        assert!(payload.starts_with("/w/note.txt"), "{payload}");
        assert!(
            payload.contains("+one") && payload.contains("+two"),
            "{payload}"
        );
    }

    #[test]
    fn a_synthetic_diff_carries_a_hunk_header_a_parser_accepts() {
        let input = serde_json::json!({"file_path": "/w/note.txt", "content": "ok"});
        let diff = derive_tool_diff("Write", &input, None)
            .unwrap_or_else(|| panic!("a Write must derive a diff"));
        assert_eq!(diff.path, PathBuf::from("/w/note.txt"));
        assert!(diff.unified.contains("@@ -0,0 +1,1 @@"), "{}", diff.unified);
        assert_eq!((diff.added, diff.removed), (1, 0));
    }

    #[test]
    fn mcp_tools_carry_their_server_in_the_kind_column() {
        assert_eq!(
            tool_kind("mcp__linear__create_issue"),
            ToolKind::Mcp {
                server: "linear".to_owned()
            }
        );
        assert_eq!(
            tool_kind("Holograph"),
            ToolKind::Unknown {
                name: "Holograph".to_owned()
            }
        );
    }

    #[test]
    fn a_gate_shaped_tool_summarises_in_one_line_instead_of_dumping_its_payload() {
        let input = serde_json::json!({
            "questions": [
                {"header": "Pick a database", "question": "Which one?"},
                {"header": "Pick a runtime", "question": "Which one?"}
            ]
        });
        assert_eq!(
            tool_summary("AskUserQuestion", &input, None),
            "Pick a database · Pick a runtime"
        );
        assert_eq!(tool_summary("ExitPlanMode", &Value::Null, None), "plan");
    }
}
