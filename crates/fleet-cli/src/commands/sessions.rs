use super::{CommandOutput, parse_id, validation};
use crate::{
    args::{AgentChoice, AgentStatusArgs, AgentStatusChoice, SleepArgs},
    envelope::{AgentStatusEnvelope, PROTOCOL, SleepEnvelope, to_json},
    human,
};
use fleet_client::Client;
use fleet_core::{
    config::Agent,
    ids::{SessionId, WorktreeId},
    sessions::AgentActivity,
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::SleepResult,
};
use std::{borrow::Cow, ffi::OsString};

pub(super) async fn sleep(
    client: &Client,
    arguments: SleepArgs,
) -> Result<CommandOutput, ProtoError> {
    let result = resolve_and_sleep(client, arguments.session.as_deref()).await?;
    let text = if arguments.json {
        to_json(&SleepEnvelope {
            protocol: PROTOCOL,
            kept: &result.kept,
            closed: &result.closed,
            session_killed: result.session_killed,
        })?
    } else {
        human::sleep(&result)
    };
    Ok(CommandOutput::success(text))
}

async fn resolve_and_sleep(
    client: &Client,
    target: Option<&str>,
) -> Result<SleepResult, ProtoError> {
    if let Some(target) = target {
        if target.contains('#') {
            return client.sleep_worktree(parse_id::<WorktreeId>(target)?).await;
        }
        return client.sleep_session(parse_id::<SessionId>(target)?).await;
    }
    if let Some(session) = std::env::var_os("FLEET_SESSION") {
        let session = session
            .into_string()
            .map_err(|_| validation("FLEET_SESSION is not valid UTF-8"))?;
        return client.sleep_session(parse_id::<SessionId>(&session)?).await;
    }
    if let Some(session) = client.current_session().await? {
        return client.sleep_session(session).await;
    }
    let sessions = client.list_sessions().await?;
    let mut sessions = sessions.into_iter();
    match (sessions.next(), sessions.next()) {
        (Some(session), None) => client.sleep_session(session.id).await,
        (None, _) => Err(ProtoError {
            kind: ErrorKind::NotFound,
            message: "no current or running session".to_owned(),
        }),
        _ => Err(validation(
            "session is required when more than one Fleet session is running",
        )),
    }
}

pub(super) async fn agent(
    client: &Client,
    requested: Option<AgentChoice>,
) -> Result<CommandOutput, ProtoError> {
    let selected = match requested {
        Some(AgentChoice::Claude) => Agent::Claude,
        Some(AgentChoice::Opencode) => Agent::Opencode,
        None => client.get_config().await?.agent,
    };
    let session = client.ensure_session(None, Some(selected), false).await?;
    Ok(CommandOutput::success(format!(
        "{}\nOpen it in the Fleet app: fleet",
        session.id
    )))
}

pub(super) async fn agent_status(
    client: &Client,
    arguments: AgentStatusArgs,
) -> Result<CommandOutput, ProtoError> {
    let (session, terminal_id) =
        resolve_agent_status_target(&arguments, |name| std::env::var_os(name))?;
    let activity = match arguments.activity {
        AgentStatusChoice::Working => AgentActivity::Working,
        AgentStatusChoice::Finished => AgentActivity::Idle,
    };
    let text = if arguments.json {
        to_json(&AgentStatusEnvelope {
            protocol: PROTOCOL,
            ok: true,
            session: &session,
            terminal_id,
            activity,
        })?
    } else {
        String::new()
    };
    client
        .set_agent_activity(session, terminal_id, activity)
        .await?;
    Ok(CommandOutput::success(text))
}

pub(super) fn resolve_agent_status_target(
    arguments: &AgentStatusArgs,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<(SessionId, fleet_core::ids::TerminalId), ProtoError> {
    let session: Cow<'_, str> = match &arguments.session {
        Some(session) => Cow::Borrowed(session),
        None => Cow::Owned(
            env("FLEET_SESSION")
                .ok_or_else(|| validation("--session is required when FLEET_SESSION is not set"))?
                .into_string()
                .map_err(|_| validation("FLEET_SESSION is not valid UTF-8"))?,
        ),
    };
    let terminal_id = match arguments.terminal_id {
        Some(terminal_id) => terminal_id,
        None => env("FLEET_TERMINAL_ID")
            .ok_or_else(|| {
                validation("--terminal-id is required when FLEET_TERMINAL_ID is not set")
            })?
            .into_string()
            .map_err(|_| validation("FLEET_TERMINAL_ID is not valid UTF-8"))?
            .parse::<u64>()
            .map_err(|_| validation("FLEET_TERMINAL_ID must be numeric"))?,
    };
    Ok((
        parse_id(&session)?,
        fleet_core::ids::TerminalId(terminal_id),
    ))
}
