//! Interactive-rebase plans and sequence-editor callback.

use std::{
    env,
    path::Path,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{GitError, MutationResult, ObjectId, Repository, Result, model::CommandKind};

/// Environment variable carrying a serialized one-shot rebase plan.
pub const SEQUENCE_INSTRUCTION_ENV: &str = "FLEET_GIT_SEQUENCE_INSTRUCTION";

/// Base argument passed to `git rebase --interactive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RebaseBase {
    /// Rebase every commit from the root.
    Root,
    /// Rebase commits after this revision expression.
    Reference(String),
}

/// Optional behavior for a `fixup` todo line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixupFlag {
    /// Use the fixup commit's message (`-C`).
    KeepMessage,
    /// Open the combined message for editing (`-c`).
    EditMessage,
}

/// Action assigned to a rebase todo commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RebaseAction {
    /// Replay unchanged.
    Pick,
    /// Fold into the previous commit and combine messages.
    Squash,
    /// Fold into the previous commit, normally discarding this message.
    Fixup {
        /// Optional commit-message behavior.
        flag: Option<FixupFlag>,
    },
    /// Ask Git to edit the message.
    Reword,
    /// Pause after applying the commit.
    Edit,
    /// Omit the commit.
    Drop,
}

/// One transformation of Git's todo file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TodoEdit {
    /// Change one commit's action, matching an OID prefix exactly once.
    Change {
        /// Full or abbreviated object identifier.
        oid: ObjectId,
        /// Replacement action.
        action: RebaseAction,
    },
    /// Move selected todo commits by an actionable-line offset.
    Move {
        /// Full or abbreviated object identifiers.
        oids: Vec<ObjectId>,
        /// Signed offset in chronological todo order.
        offset: isize,
    },
}

/// Complete one-shot interactive-rebase instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebasePlan {
    /// Revision before the first commit to rewrite.
    pub base: RebaseBase,
    /// Optional `--onto` target.
    pub onto: Option<String>,
    /// Todo transformations.
    pub edits: Vec<TodoEdit>,
    /// Enable Git's autostash support.
    pub autostash: bool,
    /// Preserve commits that become empty.
    pub keep_empty: bool,
}

/// Direction used by `Repository::move_commit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    /// Move toward HEAD/newer commits.
    Up,
    /// Move toward the root/older commits.
    Down,
}

impl Repository {
    /// Runs an interactive rebase whose todo is rewritten by `helper_exe`.
    pub async fn interactive_rebase(
        &self,
        plan: RebasePlan,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let instruction = serde_json::to_string(&plan)
            .map_err(|error| GitError::parse("rebase plan", error.to_string()))?;
        let editor = shell_quote(helper_exe);
        let mut command = self
            .command(CommandKind::Mutation)
            .args(["rebase", "--interactive"])
            .env("GIT_SEQUENCE_EDITOR", editor)
            .env("GIT_EDITOR", "true")
            .env(SEQUENCE_INSTRUCTION_ENV, instruction)
            .env("LANG", "C")
            .env("LC_MESSAGES", "C")
            .may_conflict();
        if plan.autostash {
            command = command.arg("--autostash");
        }
        if plan.keep_empty {
            command = command.args(["--keep-empty", "--empty=keep"]);
        }
        command = command.arg("--no-autosquash");
        if let Some(onto) = plan.onto {
            command = command.args(["--onto", &onto]);
        }
        command = match plan.base {
            RebaseBase::Root => command.arg("--root"),
            RebaseBase::Reference(base) => command.arg(base),
        };
        self.run_commands(vec![command]).await
    }

    /// Squashes `oid` into its previous commit.
    pub async fn squash_into_previous(
        &self,
        oid: &ObjectId,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Squash, true, helper_exe)
            .await
    }

    /// Fixes `oid` into its previous commit, discarding its message.
    pub async fn fixup_into_previous(
        &self,
        oid: &ObjectId,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Fixup { flag: None }, true, helper_exe)
            .await
    }

    /// Drops `oid` from the current history.
    pub async fn drop_commit(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Drop, false, helper_exe)
            .await
    }

    /// Rewords `oid` without opening an editor.
    pub async fn reword_commit(
        &self,
        oid: &ObjectId,
        new_message: &str,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let mut result = self
            .action_rebase(oid, RebaseAction::Edit, false, helper_exe)
            .await?;
        let amended = self
            .commit(
                new_message,
                crate::CommitOptions {
                    amend: true,
                    ..crate::CommitOptions::default()
                },
            )
            .await?;
        result.records.extend(amended.records);
        if result.warning.is_none() {
            result.warning = amended.warning;
        }
        let continued = self.rebase_continue().await?;
        result.records.extend(continued.records);
        if result.warning.is_none() {
            result.warning = continued.warning;
        }
        Ok(result)
    }

    /// Moves `oid` one commit in the requested history direction.
    pub async fn move_commit(
        &self,
        oid: &ObjectId,
        direction: MoveDirection,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let offset = match direction {
            MoveDirection::Up => 1,
            MoveDirection::Down => -1,
        };
        let base = RebaseBase::Reference(format!("{}^^", oid.as_str()));
        self.interactive_rebase(
            RebasePlan {
                base,
                onto: None,
                edits: vec![TodoEdit::Move {
                    oids: vec![oid.clone()],
                    offset,
                }],
                autostash: true,
                keep_empty: true,
            },
            helper_exe,
        )
        .await
    }

    /// Starts a rebase that pauses after applying `oid`.
    pub async fn edit_commit(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult> {
        self.action_rebase(oid, RebaseAction::Edit, false, helper_exe)
            .await
    }

    async fn action_rebase(
        &self,
        oid: &ObjectId,
        action: RebaseAction,
        include_previous: bool,
        helper_exe: &Path,
    ) -> Result<MutationResult> {
        let suffix = if include_previous { "^^" } else { "^" };
        self.interactive_rebase(
            RebasePlan {
                base: RebaseBase::Reference(format!("{}{suffix}", oid.as_str())),
                onto: None,
                edits: vec![TodoEdit::Change {
                    oid: oid.clone(),
                    action,
                }],
                autostash: true,
                keep_empty: true,
            },
            helper_exe,
        )
        .await
    }
}

/// Applies a serialized plan to a Git rebase todo file and replaces it atomically.
pub fn run_sequence_editor(todo_path: &Path, encoded_plan: &str) -> Result<()> {
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
        selected_positions.push(
            actionable
                .iter()
                .position(|index| *index == matches[0])
                .expect("matching line is actionable"),
        );
    }
    selected_positions.sort_unstable();
    selected_positions.dedup();
    let mut ordered: Vec<Vec<u8>> = actionable
        .iter()
        .map(|index| lines[*index].clone())
        .collect();
    let selected_set: std::collections::HashSet<usize> =
        selected_positions.iter().copied().collect();
    let block: Vec<Vec<u8>> = ordered
        .iter()
        .enumerate()
        .filter(|(index, _)| selected_set.contains(index))
        .map(|(_, line)| line.clone())
        .collect();
    let remaining: Vec<Vec<u8>> = ordered
        .drain(..)
        .enumerate()
        .filter(|(index, _)| !selected_set.contains(index))
        .map(|(_, line)| line)
        .collect();
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

#[derive(Debug, Clone, Copy)]
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

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{FixupFlag, RebaseAction, RebaseBase, RebasePlan, TodoEdit, run_sequence_editor};
    use crate::ObjectId;

    const TODO: &str = "pick aaa111 one\n# a comment\npick bbb222 two\npick ccc333 three\n\n# Rebase aaa..ccc onto aaa\n#\n# Commands:\n# p, pick <commit> = use commit\n";

    fn apply(todo: &str, edits: Vec<TodoEdit>) -> String {
        let directory = tempdir().expect("temp dir");
        let path = directory.path().join("git-rebase-todo");
        std::fs::write(&path, todo).expect("write todo");
        let plan = RebasePlan {
            base: RebaseBase::Root,
            onto: None,
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
            onto: None,
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
