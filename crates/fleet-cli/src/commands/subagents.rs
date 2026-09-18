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
    let model = arguments.model.map(model_selection).transpose()?;
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
        .delegation_wait(arguments.id, arguments.timeout.saturating_mul(1_000))
        .await?;
    let terminal = delegation.status.is_terminal();
    let text = if arguments.json {
        to_json(&SubagentEnvelope {
            protocol: PROTOCOL,
            delegation: &delegation,
            warning: None,
        })?
    } else {
        human::delivered_message(&delegation, SystemTime::now())
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

fn model_selection(value: String) -> Result<ModelSelection, ProtoError> {
    if value.trim().is_empty() {
        return Err(validation("model cannot be empty"));
    }
    Ok(ModelSelection {
        model: value,
        effort: None,
        provider: None,
    })
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
    fn utf8_truncation_never_splits_a_character() {
        let mut value = format!("{}é", "a".repeat(RESULT_CAP_BYTES - 1));
        truncate_utf8(&mut value, RESULT_CAP_BYTES);
        assert_eq!(value.len(), RESULT_CAP_BYTES - 1);
    }
}
