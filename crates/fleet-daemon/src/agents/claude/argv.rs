//! The Claude Code launch line, as one pure function with byte-exact goldens.
//!
//! Every flag is mandatory for a reason spec A.2.2 records, and three of them are corrections to
//! an earlier revision of the design:
//!
//! - **`--permission-prompt-tool stdio` is not optional.** With `--permission-prompts host`
//!   alone, a tool needing approval produces **no** `control_request` — it produces
//!   `{"type":"system","subtype":"permission_denied",…}` and the tool is silently refused. Fleet
//!   would show no card and the user an inexplicable denial.
//! - **`--effort` is real in 2.1.266** and is **not** echoed on `system/init`, so Fleet must
//!   remember what it launched with.
//! - **`system/init.permissionMode` is advisory**: `manual` comes back as `"default"`.
//!
//! User launch args are appended **last** so a user override wins.

use std::path::Path;

use fleet_core::agents::{PermissionMode, StartRequest};
use uuid::Uuid;

use crate::agents::harness::{HarnessResult, process};

/// Everything the launch line needs that is not in the [`StartRequest`].
#[derive(Debug, Clone)]
pub struct Launch<'a> {
    /// The session request.
    pub start: &'a StartRequest,
    /// The session id Fleet minted for a fresh session; the cursor is durable before the CLI
    /// speaks, so a crash between spawn and `system/init` still leaves a resumable thread.
    pub session_id: Uuid,
    /// The leaf attachments directory to grant. Granting the parent would grant its siblings.
    pub attachments_dir: Option<&'a Path>,
    /// Extra user-supplied launch arguments, appended last.
    pub user_args: &'a str,
}

/// The `--permission-mode` value for a Fleet mode.
///
/// 2.1.266 accepts six values (`acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`,
/// `plan`). `Ask` is `manual`: "ask before everything" is exactly what that value means, and the
/// value the ground-truth capture was taken with.
#[must_use]
pub const fn permission_mode_to_wire(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "manual",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Plan => "plan",
        PermissionMode::FullAccess => "bypassPermissions",
    }
}

/// The Fleet mode a `system/init.permissionMode` names.
///
/// Advisory only: the CLI reports `manual` as `"default"`, so Fleet keeps its own record and this
/// exists to notice a mode it did *not* ask for.
#[must_use]
pub fn permission_mode_from_wire(mode: &str) -> PermissionMode {
    match mode {
        "acceptEdits" => PermissionMode::AcceptEdits,
        "plan" => PermissionMode::Plan,
        "bypassPermissions" | "dontAsk" => PermissionMode::FullAccess,
        _ => PermissionMode::Ask,
    }
}

/// A stored resume cursor Fleet is willing to hand back to the CLI.
///
/// A cursor that is not a valid UUIDv4 is **dropped, not passed through**: the CLI would either
/// refuse it or open something that is not the conversation Fleet recorded.
#[must_use]
pub fn resume_cursor(cursor: Option<&String>) -> Option<String> {
    let cursor = cursor?;
    let parsed = Uuid::parse_str(cursor).ok()?;
    if parsed.get_version_num() == 4 {
        Some(cursor.clone())
    } else {
        tracing::warn!(
            target: "fleet::agents::claude",
            "ignoring a stored Claude resume cursor that is not a UUIDv4"
        );
        None
    }
}

/// Builds the whole argv after the configured command line.
pub fn launch_args(launch: &Launch<'_>) -> HarnessResult<Vec<String>> {
    let mut args = vec![
        "-p".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--input-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        "--include-partial-messages".to_owned(),
        "--permission-mode".to_owned(),
        permission_mode_to_wire(launch.start.mode).to_owned(),
        "--permission-prompts".to_owned(),
        "host".to_owned(),
        "--permission-prompt-tool".to_owned(),
        "stdio".to_owned(),
    ];
    if let Some(model) = &launch.start.model {
        args.push("--model".to_owned());
        // The 1M context window is a *suffix on the model id* (`claude-opus-5[1m]`), not a
        // separate flag, which is why changing it costs a restart like any other model change.
        args.push(model.model.clone());
        if let Some(effort) = &model.effort
            && !effort.trim().is_empty()
        {
            args.push("--effort".to_owned());
            args.push(effort.clone());
        }
    }
    if let Some(directory) = launch.attachments_dir {
        args.push("--add-dir".to_owned());
        args.push(directory.to_string_lossy().into_owned());
    }
    // One or the other, never both.
    match resume_cursor(launch.start.resume_cursor.as_ref()) {
        Some(cursor) => {
            args.push("--resume".to_owned());
            args.push(cursor);
            if launch.start.fork {
                args.push("--fork-session".to_owned());
            }
        }
        None => {
            args.push("--session-id".to_owned());
            args.push(launch.session_id.to_string());
        }
    }
    args.extend(process::tokenize_user_args(launch.user_args)?);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fleet_core::agents::{AgentKind, ModelSelection, ThreadId};

    use super::*;

    fn start() -> StartRequest {
        StartRequest {
            thread: ThreadId::new(),
            worktree_path: PathBuf::from("/w"),
            provider: AgentKind::Claude,
            model: Some(ModelSelection {
                model: "claude-opus-5[1m]".to_owned(),
                effort: Some("high".to_owned()),
                provider: None,
            }),
            mode: PermissionMode::Ask,
            resume_cursor: None,
            fork: false,
            env: std::collections::BTreeMap::new(),
            sandbox: fleet_core::agents::SandboxPolicy::default(),
            approval_policy: fleet_core::agents::ApprovalPolicy::default(),
            permission_profile: None,
            title: None,
        }
    }

    fn built(start: &StartRequest, attachments: Option<&Path>, user: &str) -> Vec<String> {
        launch_args(&Launch {
            start,
            session_id: Uuid::parse_str("11111111-2222-4333-8444-555555555555")
                .unwrap_or_else(|error| panic!("{error}")),
            attachments_dir: attachments,
            user_args: user,
        })
        .unwrap_or_else(|error| panic!("{error}"))
    }

    /// The golden for a fresh session. Byte-exact, because a missing flag here is a silent
    /// behaviour change: no `--permission-prompt-tool` means no approval card at all.
    #[test]
    fn a_fresh_session_launches_with_every_mandatory_flag() {
        let attachments = PathBuf::from("/w/.fleet/attachments/thread-1");
        assert_eq!(
            built(&start(), Some(&attachments), ""),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-mode",
                "manual",
                "--permission-prompts",
                "host",
                "--permission-prompt-tool",
                "stdio",
                "--model",
                "claude-opus-5[1m]",
                "--effort",
                "high",
                "--add-dir",
                "/w/.fleet/attachments/thread-1",
                "--session-id",
                "11111111-2222-4333-8444-555555555555",
            ]
        );
    }

    #[test]
    fn a_resume_replaces_the_session_id_and_forks_only_when_asked() {
        let mut request = start();
        request.model = None;
        request.resume_cursor = Some("6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e".to_owned());
        let resumed = built(&request, None, "");
        assert!(!resumed.contains(&"--session-id".to_owned()));
        assert_eq!(
            resumed.iter().rev().take(2).collect::<Vec<_>>(),
            ["6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e", "--resume"]
        );
        assert!(!resumed.contains(&"--fork-session".to_owned()));

        request.fork = true;
        let forked = built(&request, None, "");
        assert_eq!(forked.last().map(String::as_str), Some("--fork-session"));
    }

    #[test]
    fn a_cursor_that_is_not_a_uuidv4_is_dropped_rather_than_passed_through() {
        let mut request = start();
        // A UUIDv7 — Codex's shape. Handing it to `claude --resume` opens nothing.
        request.resume_cursor = Some("01a089f2-5337-7470-adb1-219e71d62a35".to_owned());
        let args = built(&request, None, "");
        assert!(args.contains(&"--session-id".to_owned()));
        assert!(!args.contains(&"--resume".to_owned()));

        request.resume_cursor = Some("not-a-uuid".to_owned());
        assert!(built(&request, None, "").contains(&"--session-id".to_owned()));
    }

    #[test]
    fn user_launch_args_come_last_so_an_override_wins() {
        let args = built(
            &start(),
            None,
            "--model 'claude haiku 4.5' --settings x.json",
        );
        let model = args
            .iter()
            .rposition(|arg| arg == "--model")
            .unwrap_or_else(|| panic!("no --model"));
        assert_eq!(
            args.get(model + 1).map(String::as_str),
            Some("claude haiku 4.5")
        );
        assert_eq!(args.last().map(String::as_str), Some("x.json"));
    }

    #[test]
    fn every_mode_maps_to_a_value_the_cli_accepts() {
        // The six values 2.1.266 accepts; `default` is not one of them.
        let accepted = [
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "manual",
            "dontAsk",
            "plan",
        ];
        for mode in [
            PermissionMode::Ask,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::FullAccess,
        ] {
            let wire = permission_mode_to_wire(mode);
            assert!(accepted.contains(&wire), "{wire} is not a 2.1.266 mode");
        }
    }
}
