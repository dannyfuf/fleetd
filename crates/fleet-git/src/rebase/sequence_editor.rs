//! Git sequence-editor callback and byte-preserving todo transformations.

use super::plan::{FixupFlag, RebaseAction, RebasePlan, TodoEdit};
use crate::{GitError, ObjectId, Result};
use std::{
    env,
    path::Path,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

/// Environment variable carrying a serialized one-shot rebase plan.
pub(crate) const SEQUENCE_INSTRUCTION_ENV: &str = "FLEET_GIT_SEQUENCE_INSTRUCTION";

/// Applies a serialized plan to a Git rebase todo file and replaces it atomically.
fn run_sequence_editor(todo_path: &Path, encoded_plan: &str) -> Result<()> {
    let plan: RebasePlan = serde_json::from_str(encoded_plan)
        .map_err(|error| GitError::parse("rebase plan", error.to_string()))?;
    let content = std::fs::read(todo_path).map_err(|source| GitError::Spawn {
        argv: vec![format!("read {}", todo_path.display())],
        source,
    })?;
    let mut lines: Vec<Vec<u8>> = content
        .split_inclusive(|byte| *byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    for edit in &plan.edits {
        match edit {
            TodoEdit::Change { oid, action } => change_action(&mut lines, oid, action)?,
            TodoEdit::Move { oids, offset } => move_lines(&mut lines, oids, *offset)?,
        }
    }
    let output: Vec<u8> = lines.into_iter().flatten().collect();
    atomic_write(todo_path, &output)
}

/// Handles a Git sequence-editor callback when the instruction environment is present.
#[must_use]
pub fn maybe_run_from_env() -> Option<ExitCode> {
    let instruction = env::var(SEQUENCE_INSTRUCTION_ENV).ok()?;
    let Some(todo_path) = env::args_os().nth(1) else {
        eprintln!("{SEQUENCE_INSTRUCTION_ENV} is set but Git supplied no todo path");
        return Some(ExitCode::from(2));
    };
    match run_sequence_editor(Path::new(&todo_path), &instruction) {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("fleet-git sequence editor: {error}");
            Some(ExitCode::from(2))
        }
    }
}

fn change_action(lines: &mut [Vec<u8>], requested: &ObjectId, action: &RebaseAction) -> Result<()> {
    let matches = matching_lines(lines, requested);
    if matches.len() != 1 {
        return Err(GitError::parse(
            "rebase todo",
            format!("OID {} matched {} lines", requested, matches.len()),
        ));
    }
    let index = matches[0];
    let info = line_info(&lines[index])
        .ok_or_else(|| GitError::parse("rebase todo", "matched line became invalid"))?;
    let mut replacement = Vec::new();
    replacement.extend_from_slice(&lines[index][..info.action_start]);
    replacement.extend_from_slice(action_text(action));
    replacement.push(b' ');
    replacement.extend_from_slice(&lines[index][info.oid_start..]);
    lines[index] = replacement;
    Ok(())
}

fn move_lines(lines: &mut [Vec<u8>], requested: &[ObjectId], offset: isize) -> Result<()> {
    if offset == 0 || requested.is_empty() {
        return Ok(());
    }
    let actionable: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line_info(line).map(|_| index))
        .collect();
    let mut selected_positions = Vec::new();
    for oid in requested {
        let matches = matching_lines(lines, oid);
        if matches.len() != 1 {
            return Err(GitError::parse(
                "rebase todo",
                format!("OID {oid} matched {} lines", matches.len()),
            ));
        }
        let position = actionable
            .iter()
            .position(|index| *index == matches[0])
            .ok_or_else(|| GitError::parse("rebase todo", "matched line is not actionable"))?;
        selected_positions.push(position);
    }
    selected_positions.sort_unstable();
    selected_positions.dedup();
    let selected_set: std::collections::HashSet<usize> =
        selected_positions.iter().copied().collect();
    let mut block = Vec::with_capacity(selected_positions.len());
    let mut remaining = Vec::with_capacity(actionable.len() - selected_positions.len());
    for (position, index) in actionable.iter().enumerate() {
        let line = std::mem::take(&mut lines[*index]);
        if selected_set.contains(&position) {
            block.push(line);
        } else {
            remaining.push(line);
        }
    }
    // `selected_positions` is sorted, so the block's anchor is its first entry;
    // no earlier selected line can shift it.
    let anchor = selected_positions[0];
    let destination = (anchor as isize + offset).clamp(0, remaining.len() as isize) as usize;
    let mut rebuilt = remaining;
    rebuilt.splice(destination..destination, block);
    for (slot, line) in actionable.into_iter().zip(rebuilt) {
        lines[slot] = line;
    }
    Ok(())
}

fn matching_lines(lines: &[Vec<u8>], requested: &ObjectId) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let info = line_info(line)?;
            let oid = std::str::from_utf8(&line[info.oid_start..info.oid_end]).ok()?;
            (oid.starts_with(requested.as_str()) || requested.as_str().starts_with(oid))
                .then_some(index)
        })
        .collect()
}

#[derive(Debug)]
struct LineInfo {
    action_start: usize,
    oid_start: usize,
    oid_end: usize,
}

fn line_info(line: &[u8]) -> Option<LineInfo> {
    let action_start = line.iter().position(|byte| !byte.is_ascii_whitespace())?;
    if line[action_start] == b'#' {
        return None;
    }
    let action_end = action_start
        + line[action_start..]
            .iter()
            .position(u8::is_ascii_whitespace)?;
    let action = &line[action_start..action_end];
    if !matches!(
        action,
        b"pick"
            | b"p"
            | b"reword"
            | b"r"
            | b"edit"
            | b"e"
            | b"squash"
            | b"s"
            | b"fixup"
            | b"f"
            | b"drop"
            | b"d"
    ) {
        return None;
    }
    let mut oid_start = action_end;
    while line.get(oid_start).is_some_and(u8::is_ascii_whitespace) {
        oid_start += 1;
    }
    if matches!(action, b"fixup" | b"f") && line.get(oid_start) == Some(&b'-') {
        // Skip the `-C` / `-c` flag that may follow a `fixup` action.
        while line
            .get(oid_start)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            oid_start += 1;
        }
        while line.get(oid_start).is_some_and(u8::is_ascii_whitespace) {
            oid_start += 1;
        }
    }
    let mut oid_end = oid_start;
    while line
        .get(oid_end)
        .is_some_and(|byte| byte.is_ascii_hexdigit())
    {
        oid_end += 1;
    }
    (oid_end > oid_start).then_some(LineInfo {
        action_start,
        oid_start,
        oid_end,
    })
}

fn action_text(action: &RebaseAction) -> &'static [u8] {
    match action {
        RebaseAction::Pick => b"pick",
        RebaseAction::Squash => b"squash",
        RebaseAction::Fixup { flag: None } => b"fixup",
        RebaseAction::Fixup {
            flag: Some(FixupFlag::KeepMessage),
        } => b"fixup -C",
        RebaseAction::Fixup {
            flag: Some(FixupFlag::EditMessage),
        } => b"fixup -c",
        RebaseAction::Reword => b"reword",
        RebaseAction::Edit => b"edit",
        RebaseAction::Drop => b"drop",
    }
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GitError::parse("rebase todo", "todo path has no parent"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".fleet-git-todo-{}-{nonce}", std::process::id()));
    std::fs::write(&temporary, content).map_err(|source| GitError::Spawn {
        argv: vec![format!("write {}", temporary.display())],
        source,
    })?;
    std::fs::rename(&temporary, path).map_err(|source| GitError::Spawn {
        argv: vec![format!("rename {} {}", temporary.display(), path.display())],
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::super::plan::RebaseBase;
    use super::{FixupFlag, RebaseAction, RebasePlan, TodoEdit, run_sequence_editor};
    use crate::ObjectId;

    const TODO: &str = "pick aaa111 one\n# a comment\npick bbb222 two\npick ccc333 three\n\n# Rebase aaa..ccc onto aaa\n#\n# Commands:\n# p, pick <commit> = use commit\n";

    fn apply(todo: &str, edits: Vec<TodoEdit>) -> String {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("git-rebase-todo");
        std::fs::write(&path, todo).expect("write todo");
        let plan = RebasePlan {
            base: RebaseBase::Root,
            edits,
            autostash: false,
            keep_empty: false,
        };
        run_sequence_editor(&path, &serde_json::to_string(&plan).expect("encode plan"))
            .expect("run sequence editor");
        std::fs::read_to_string(&path).expect("read todo")
    }

    fn actions(todo: &str) -> Vec<String> {
        todo.lines()
            .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn changes_one_action_and_preserves_everything_else() {
        for (action, expected) in [
            (RebaseAction::Squash, "squash bbb222 two"),
            (RebaseAction::Fixup { flag: None }, "fixup bbb222 two"),
            (
                RebaseAction::Fixup {
                    flag: Some(FixupFlag::KeepMessage),
                },
                "fixup -C bbb222 two",
            ),
            (
                RebaseAction::Fixup {
                    flag: Some(FixupFlag::EditMessage),
                },
                "fixup -c bbb222 two",
            ),
            (RebaseAction::Reword, "reword bbb222 two"),
            (RebaseAction::Edit, "edit bbb222 two"),
            (RebaseAction::Drop, "drop bbb222 two"),
            (RebaseAction::Pick, "pick bbb222 two"),
        ] {
            let result = apply(
                TODO,
                vec![TodoEdit::Change {
                    oid: ObjectId::from("bbb"),
                    action,
                }],
            );
            assert_eq!(actions(&result)[1], expected);
            assert!(result.contains("# a comment"));
            assert!(result.contains("# p, pick <commit> = use commit"));
            assert_eq!(actions(&result).len(), 3);
        }
    }

    #[test]
    fn rewrites_an_existing_fixup_flag_rather_than_duplicating_it() {
        let todo = "pick aaa111 one\nfixup -C bbb222 two\n";
        let result = apply(
            todo,
            vec![TodoEdit::Change {
                oid: ObjectId::from("bbb222"),
                action: RebaseAction::Pick,
            }],
        );
        assert_eq!(actions(&result)[1], "pick bbb222 two");
    }

    #[test]
    fn moves_a_commit_in_both_directions_without_touching_comments() {
        let later = apply(
            TODO,
            vec![TodoEdit::Move {
                oids: vec![ObjectId::from("aaa111")],
                offset: 1,
            }],
        );
        assert_eq!(
            actions(&later),
            vec!["pick bbb222 two", "pick aaa111 one", "pick ccc333 three"]
        );
        assert!(later.contains("# a comment"));

        let earlier = apply(
            TODO,
            vec![TodoEdit::Move {
                oids: vec![ObjectId::from("ccc333")],
                offset: -1,
            }],
        );
        assert_eq!(
            actions(&earlier),
            vec!["pick aaa111 one", "pick ccc333 three", "pick bbb222 two"]
        );
    }

    #[test]
    fn move_offsets_are_clamped_to_the_todo_bounds() {
        let result = apply(
            TODO,
            vec![TodoEdit::Move {
                oids: vec![ObjectId::from("aaa111")],
                offset: 99,
            }],
        );
        assert_eq!(
            actions(&result),
            vec!["pick bbb222 two", "pick ccc333 three", "pick aaa111 one"]
        );
    }

    #[test]
    fn applies_several_edits_in_order() {
        let result = apply(
            TODO,
            vec![
                TodoEdit::Change {
                    oid: ObjectId::from("ccc"),
                    action: RebaseAction::Fixup { flag: None },
                },
                TodoEdit::Move {
                    oids: vec![ObjectId::from("ccc")],
                    offset: -1,
                },
            ],
        );
        assert_eq!(
            actions(&result),
            vec!["pick aaa111 one", "fixup ccc333 three", "pick bbb222 two"]
        );
    }

    #[test]
    fn rejects_object_ids_that_do_not_match_exactly_one_line() {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("git-rebase-todo");
        std::fs::write(&path, "pick aaa111 one\npick aaa222 two\n").expect("write todo");
        let plan = RebasePlan {
            base: RebaseBase::Root,
            edits: vec![TodoEdit::Change {
                oid: ObjectId::from("aaa"),
                action: RebaseAction::Drop,
            }],
            autostash: false,
            keep_empty: false,
        };
        let encoded = serde_json::to_string(&plan).expect("encode plan");
        assert!(run_sequence_editor(&path, &encoded).is_err());

        let plan = RebasePlan {
            edits: vec![TodoEdit::Change {
                oid: ObjectId::from("zzz"),
                action: RebaseAction::Drop,
            }],
            ..plan
        };
        let encoded = serde_json::to_string(&plan).expect("encode plan");
        assert!(run_sequence_editor(&path, &encoded).is_err());
    }
}
