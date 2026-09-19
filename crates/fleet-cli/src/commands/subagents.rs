//! Delegated native-agent command group.

use std::{io::Read, path::Path, time::SystemTime};

use fleet_client::{Client, DelegationRunRequest};
use fleet_core::agents::{AgentKind, Delegation, ModelSelection, PermissionMode, ThreadId};
use fleet_proto::{agents::ITEM_BODY_MAX_CHUNK_BYTES, error::ProtoError};

use super::{CommandOutput, validation};
use crate::{
    args::{
        AgentChoice, AgentModeChoice, SubagentCommand, SubagentCompleteArgs, SubagentIdArgs,
        SubagentListArgs, SubagentRunArgs, SubagentWaitArgs,
    },
    envelope::{PROTOCOL, SubagentEnvelope, SubagentsEnvelope, to_json},
    human,
};

const RESULT_CAP_BYTES: usize = ITEM_BODY_MAX_CHUNK_BYTES as usize;

/// Process context injected into delegated child sessions by the daemon.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Environment {
    pub(super) session: Option<String>,
    pub(super) delegation: Option<String>,
    pub(super) token: Option<String>,
}

impl Environment {
    pub(super) fn from_process() -> Self {
        Self {
            session: std::env::var("FLEET_SESSION").ok(),
            delegation: std::env::var("FLEET_DELEGATION").ok(),
            token: std::env::var("FLEET_DELEGATION_TOKEN").ok(),
        }
    }
}

pub(super) fn validate_context(
    command: &SubagentCommand,
    environment: &Environment,
) -> Result<(), ProtoError> {
    match command {
        SubagentCommand::Run(arguments) => {
            fallback_id(
                arguments.caller,
                environment.session.as_deref(),
                "fleet subagent run requires --caller <thread> or FLEET_SESSION",
                "FLEET_SESSION",
            )?;
        }
        SubagentCommand::Complete(arguments) => {
            fallback_id(
                arguments.id,
                environment.delegation.as_deref(),
                "fleet subagent complete requires <id> or FLEET_DELEGATION",
                "FLEET_DELEGATION",
            )?;
            required_id::<ThreadId>(
                environment.session.as_deref(),
                "fleet subagent complete requires FLEET_SESSION",
                "FLEET_SESSION",
            )?;
            required_value(
                environment.token.as_deref(),
                "fleet subagent complete requires FLEET_DELEGATION_TOKEN",
            )?;
        }
        SubagentCommand::Wait(_)
        | SubagentCommand::Status(_)
        | SubagentCommand::List(_)
        | SubagentCommand::Cancel(_) => {}
    }
    Ok(())
}

pub(super) async fn execute(
    client: &Client,
    command: SubagentCommand,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    match command {
        SubagentCommand::Run(arguments) => run(client, arguments, environment).await,
        SubagentCommand::Complete(arguments) => complete(client, arguments, environment).await,
        SubagentCommand::Wait(arguments) => wait(client, arguments).await,
        SubagentCommand::Status(arguments) => status(client, arguments).await,
        SubagentCommand::List(arguments) => list(client, arguments).await,
        SubagentCommand::Cancel(arguments) => cancel(client, arguments).await,
    }
}

async fn run(
    client: &Client,
    arguments: SubagentRunArgs,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    let caller = fallback_id(
        arguments.caller,
        environment.session.as_deref(),
        "fleet subagent run requires --caller <thread> or FLEET_SESSION",
        "FLEET_SESSION",
    )?;
    let brief = read_text(arguments.brief_file.as_deref(), "brief")?;
    let provider = provider(arguments.provider);
    let mode = arguments.mode.map(permission_mode);
    let model = model_selection(arguments.model, arguments.effort)?;
    let (delegation, warning) = client
        .delegation_run(DelegationRunRequest {
            caller,
            provider,
            brief,
            expectation: arguments.expectation,
            worktree: arguments.worktree,
            mode,
            model,
            title: arguments.title,
            fleet_path: caller_fleet_path(),
            env: std::collections::BTreeMap::new(),
            eager: arguments.eager,
        })
        .await?;
    if arguments.json {
        return Ok(CommandOutput::success(to_json(&SubagentEnvelope {
            protocol: PROTOCOL,
            delegation: &delegation,
            warning: warning.as_deref(),
        })?));
    }
    let mut text = format!(
        "delegation {} started, child thread {}",
        delegation.id, delegation.child
    );
    if let Some(warning) = warning {
        text.push('\n');
        text.push_str(&warning);
    }
    Ok(CommandOutput::success(text))
}

/// The absolute path of the `fleet` this process is, for the daemon to hand a child.
///
/// Sent as a hint so a delegated child can run a bare `fleet subagent complete`; the daemon
/// prepends the *directory* this names to the child's `PATH`. Whatever file name the
/// orchestrator invoked us as is sent verbatim — by definition it is a `fleet` that speaks this
/// protocol version, which is the only property the daemon needs of it.
///
/// Both steps are fallible and neither is worth failing a delegation over: without the hint the
/// daemon falls back to its own resolution and the child behaves exactly as it did before the
/// field existed. So every failure — no `/proc`-equivalent answer, a deleted binary, a path that
/// is not UTF-8 and therefore has no wire representation — degrades to `None`. The CLI has no
/// `tracing` subscriber and its stdout is a machine-readable envelope, so there is nowhere to
/// report this that a caller would benefit from reading.
fn caller_fleet_path() -> Option<String> {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(_) => return None,
    };
    // Canonicalising resolves a symlinked launcher to the real binary, so the directory the
    // daemon derives is the one that actually holds it.
    let resolved = match executable.canonicalize() {
        Ok(resolved) => resolved,
        Err(_) => return None,
    };
    resolved.to_str().map(str::to_owned)
}

async fn complete(
    client: &Client,
    arguments: SubagentCompleteArgs,
    environment: &Environment,
) -> Result<CommandOutput, ProtoError> {
    let delegation = fallback_id(
        arguments.id,
        environment.delegation.as_deref(),
        "fleet subagent complete requires <id> or FLEET_DELEGATION",
        "FLEET_DELEGATION",
    )?;
    let child = required_id(
        environment.session.as_deref(),
        "fleet subagent complete requires FLEET_SESSION",
        "FLEET_SESSION",
    )?;
    let token = required_value(
        environment.token.as_deref(),
        "fleet subagent complete requires FLEET_DELEGATION_TOKEN",
    )?;
    let mut result = read_text(arguments.result_file.as_deref(), "result")?;
    if arguments.json_result {
        serde_json::from_str::<serde_json::Value>(&result)
            .map_err(|error| validation(format!("result is not valid JSON: {error}")))?;
    }
    let original_bytes = result.len();
    let notice = if original_bytes > RESULT_CAP_BYTES {
        truncate_utf8(&mut result, RESULT_CAP_BYTES);
        Some(format!(
            "result exceeded {RESULT_CAP_BYTES} bytes and was truncated"
        ))
    } else {
        None
    };
    let delegation = client
        .delegation_complete(delegation, child, token, result, arguments.blocked)
        .await?;
    let text = if arguments.json {
        to_json(&SubagentEnvelope {
            protocol: PROTOCOL,
            delegation: &delegation,
            warning: None,
        })?
    } else {
        "reported".to_owned()
    };
    Ok(CommandOutput::success(text).with_stderr(notice))
}

async fn wait(client: &Client, arguments: SubagentWaitArgs) -> Result<CommandOutput, ProtoError> {
    let delegation = client
        .delegation_wait(arguments.id, arguments.timeout.saturating_mul(1_000), None)
        .await?;
    let terminal = delegation.status.is_terminal();
    // `delivered_message` is the *terminal* template: it opens with "finished:" and prints a
    // report body. Rendering it for a timed-out wait told the caller its live child had finished
    // running, so the non-terminal answer gets its own line. JSON is unchanged either way — the
    // envelope already carries the status, and callers parse it.
    let text = if arguments.json {
        to_json(&SubagentEnvelope {
            protocol: PROTOCOL,
            delegation: &delegation,
            warning: None,
        })?
    } else if terminal {
        human::delivered_message(&delegation, SystemTime::now())
    } else {
        human::still_running_message(&delegation, SystemTime::now())
    };
    Ok(CommandOutput::with_exit_code(
        text,
        if terminal { 0 } else { 2 },
    ))
}

async fn status(client: &Client, arguments: SubagentIdArgs) -> Result<CommandOutput, ProtoError> {
    let delegation = client.delegation_get(arguments.id).await?;
    single_delegation(delegation, arguments.json)
}

async fn list(client: &Client, arguments: SubagentListArgs) -> Result<CommandOutput, ProtoError> {
    let delegations = client.delegation_list(arguments.caller).await?;
    let text = if arguments.json {
        to_json(&SubagentsEnvelope {
            protocol: PROTOCOL,
            delegations: &delegations,
        })?
    } else {
        human::subagents(&delegations, SystemTime::now())
    };
    Ok(CommandOutput::success(text))
}

async fn cancel(client: &Client, arguments: SubagentIdArgs) -> Result<CommandOutput, ProtoError> {
    let delegation = client.delegation_cancel(arguments.id).await?;
    if arguments.json {
        single_delegation(delegation, true)
    } else {
        Ok(CommandOutput::success("cancelled".to_owned()))
    }
}

fn single_delegation(delegation: Delegation, json: bool) -> Result<CommandOutput, ProtoError> {
    let text = if json {
        to_json(&SubagentEnvelope {
            protocol: PROTOCOL,
            delegation: &delegation,
            warning: None,
        })?
    } else {
        human::subagents(std::slice::from_ref(&delegation), SystemTime::now())
    };
    Ok(CommandOutput::success(text))
}

fn provider(choice: AgentChoice) -> AgentKind {
    match choice {
        AgentChoice::Claude => AgentKind::Claude,
        AgentChoice::Codex => AgentKind::Codex,
    }
}

fn permission_mode(choice: AgentModeChoice) -> PermissionMode {
    match choice {
        AgentModeChoice::Ask => PermissionMode::Ask,
        AgentModeChoice::AcceptEdits => PermissionMode::AcceptEdits,
        AgentModeChoice::Plan => PermissionMode::Plan,
        AgentModeChoice::Auto => PermissionMode::Auto,
        AgentModeChoice::DontAsk => PermissionMode::DontAsk,
        AgentModeChoice::FullAccess => PermissionMode::FullAccess,
    }
}

/// Builds the optional `ModelSelection` from `--model` and `--effort`.
///
/// Neither flag means no selection at all, which is what lets the daemon apply its configured
/// per-provider defaults; naming only the model keeps that default effort, because `create_with`
/// fills an absent effort and leaves a stated one alone.
///
/// An effort without a model is sent as a selection whose `model` is the empty string, which is
/// that field's documented "keep the configured default" sentinel: the daemon fills it from the
/// per-provider default in `create_with`, and an adapter handed an empty one names no model but
/// still spends the effort. A *present but blank* `--model` is still a validation error — the
/// caller typed the flag, so they meant something by it, and silently reading `--model ""` as
/// "the default" would hide a shell-quoting mistake.
fn model_selection(
    model: Option<String>,
    effort: Option<String>,
) -> Result<Option<ModelSelection>, ProtoError> {
    if effort
        .as_ref()
        .is_some_and(|effort| effort.trim().is_empty())
    {
        return Err(validation("effort cannot be empty"));
    }
    if model.as_ref().is_some_and(|model| model.trim().is_empty()) {
        return Err(validation("model cannot be empty"));
    }
    match (model, effort) {
        (None, None) => Ok(None),
        (model, effort) => Ok(Some(ModelSelection {
            model: model.unwrap_or_default(),
            effort,
            provider: None,
        })),
    }
}

fn fallback_id<T>(
    explicit: Option<T>,
    fallback: Option<&str>,
    missing: &str,
    variable: &str,
) -> Result<T, ProtoError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match explicit {
        Some(value) => Ok(value),
        None => required_id(fallback, missing, variable),
    }
}

fn required_id<T>(value: Option<&str>, missing: &str, variable: &str) -> Result<T, ProtoError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = required_value(value, missing)?;
    value
        .parse()
        .map_err(|error| validation(format!("invalid {variable}: {error}")))
}

fn required_value(value: Option<&str>, missing: &str) -> Result<String, ProtoError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| validation(missing))
}

fn read_text(path: Option<&Path>, name: &str) -> Result<String, ProtoError> {
    match path {
        Some(path) => std::fs::read_to_string(path).map_err(|error| read_error(name, path, error)),
        None => {
            let mut text = String::new();
            std::io::stdin()
                .lock()
                .read_to_string(&mut text)
                .map_err(|error| {
                    validation(format!("could not read {name} from stdin: {error}"))
                })?;
            Ok(text)
        }
    }
}

fn read_error(name: &str, path: &Path, error: std::io::Error) -> ProtoError {
    validation(format!(
        "could not read {name} file {}: {error}",
        path.display()
    ))
}

fn truncate_utf8(value: &mut String, cap: usize) {
    let mut boundary = cap.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    value.truncate(boundary);
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{DelegationId, ThreadId};

    use super::*;

    #[test]
    fn environment_fallbacks_are_validated_and_explicit_values_win() {
        let explicit: ThreadId = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        assert_eq!(
            fallback_id(Some(explicit), Some("invalid"), "missing", "FLEET_SESSION").unwrap(),
            explicit
        );
        assert_eq!(
            fallback_id::<ThreadId>(
                None,
                Some("00000000-0000-4000-8000-000000000002"),
                "missing",
                "FLEET_SESSION"
            )
            .unwrap()
            .to_string(),
            "00000000-0000-4000-8000-000000000002"
        );
        assert!(
            fallback_id::<DelegationId>(None, None, "required fallback", "FLEET_DELEGATION")
                .unwrap_err()
                .message
                .contains("required fallback")
        );
    }

    #[test]
    fn a_model_selection_is_built_from_every_legal_flag_pairing() {
        assert_eq!(model_selection(None, None).unwrap(), None);
        assert_eq!(
            model_selection(Some("opus".to_owned()), None).unwrap(),
            Some(ModelSelection {
                model: "opus".to_owned(),
                effort: None,
                provider: None,
            })
        );
        assert_eq!(
            model_selection(Some("opus".to_owned()), Some("high".to_owned())).unwrap(),
            Some(ModelSelection {
                model: "opus".to_owned(),
                effort: Some("high".to_owned()),
                provider: None,
            })
        );
        // Free text, never an enum: the ladder belongs to the provider, which is the only thing
        // that can say a value is wrong.
        assert_eq!(
            model_selection(Some("opus".to_owned()), Some("xhigh".to_owned()))
                .unwrap()
                .and_then(|selection| selection.effort),
            Some("xhigh".to_owned())
        );
    }

    /// `--effort` alone rides on the empty-model sentinel rather than being refused or dropped.
    #[test]
    fn an_effort_without_a_model_asks_the_daemon_for_its_default_model() {
        assert_eq!(
            model_selection(None, Some("high".to_owned())).unwrap(),
            Some(ModelSelection {
                model: String::new(),
                effort: Some("high".to_owned()),
                provider: None,
            })
        );
    }

    #[test]
    fn a_blank_flag_value_is_a_validation_error() {
        for empty in ["", "   "] {
            assert!(
                model_selection(Some("opus".to_owned()), Some(empty.to_owned()))
                    .unwrap_err()
                    .message
                    .contains("effort cannot be empty")
            );
            assert!(
                model_selection(Some(empty.to_owned()), None)
                    .unwrap_err()
                    .message
                    .contains("model cannot be empty")
            );
        }
    }

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        let mut value = format!("{}é", "a".repeat(RESULT_CAP_BYTES - 1));
        truncate_utf8(&mut value, RESULT_CAP_BYTES);
        assert_eq!(value.len(), RESULT_CAP_BYTES - 1);
    }
}
