//! Configured-host inspection and bootstrap commands.

use fleet_client::Client;
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    request::RequestBody,
    response::{DoctorCheck, DoctorStatus, ResponseBody},
    snapshot::{HostStatus, LinkState},
};

use crate::{
    args::{HostArgs, HostCommand},
    envelope::to_json,
    human,
};

use super::{CommandOutput, FAILURE, jobs::wait_for_job};

pub async fn run(client: &Client, arguments: HostArgs) -> Result<CommandOutput, ProtoError> {
    match arguments.command {
        HostCommand::List { json } => list(client, json).await,
        HostCommand::Doctor { id } => doctor(client, id).await,
        HostCommand::Bootstrap { id, git_ref } => bootstrap(client, id, git_ref).await,
    }
}

async fn list(client: &Client, json: bool) -> Result<CommandOutput, ProtoError> {
    let statuses = client.get_snapshot().await?.hosts;
    let text = if json {
        render_json(&statuses)?
    } else {
        render_statuses(&statuses)
    };
    Ok(CommandOutput::success(text))
}

async fn doctor(
    client: &Client,
    host: fleet_core::ids::HostId,
) -> Result<CommandOutput, ProtoError> {
    let checks = client.doctor_host(host).await?;
    let ok = checks
        .iter()
        .all(|check| check.status != DoctorStatus::Fail);
    Ok(CommandOutput::with_exit_code(
        render_doctor(&checks),
        if ok { 0 } else { FAILURE },
    ))
}

async fn bootstrap(
    client: &Client,
    host: fleet_core::ids::HostId,
    git_ref: Option<String>,
) -> Result<CommandOutput, ProtoError> {
    let response = client
        .request(RequestBody::BootstrapHost {
            host: host.clone(),
            git_ref,
        })
        .await?;
    let job = match response {
        ResponseBody::Job(job) => job,
        response => {
            return Err(ProtoError {
                kind: ErrorKind::Unknown,
                message: format!("bootstrap returned an unexpected response: {response:?}"),
            });
        }
    };
    wait_for_job(client, &job, "host bootstrap").await?;
    let lines = client.tail_job(job.id.clone(), 10_000).await?;
    Ok(CommandOutput::success(render_bootstrap(
        host.as_ref(),
        job.id.as_ref(),
        &lines,
    )))
}

/// Formats configured-host statuses as fixed-column rows.
#[must_use]
pub fn render_statuses(statuses: &[HostStatus]) -> String {
    let mut rows = vec![vec![
        "ID".to_owned(),
        "PROVIDER".to_owned(),
        "ADDRESS".to_owned(),
        "LINK".to_owned(),
        "VERSION".to_owned(),
        "REACHABLE".to_owned(),
        "AGENTS".to_owned(),
    ]];
    rows.extend(statuses.iter().map(|status| {
        vec![
            status.id.to_string(),
            value_or_dash(&status.provider).to_owned(),
            status.address.as_deref().unwrap_or("-").to_owned(),
            link_label(status.link).to_owned(),
            status.version.as_deref().unwrap_or("-").to_owned(),
            if status.reachable { "yes" } else { "no" }.to_owned(),
            agent_label(status),
        ]
    }));
    human::columns(&rows).join("\n")
}

/// Serializes host status using the stable CLI protocol-one envelope.
pub fn render_json(statuses: &[HostStatus]) -> Result<String, ProtoError> {
    to_json(&serde_json::json!({"protocol": 1, "hosts": statuses}))
}

/// Formats only the selected host's doctor checks.
#[must_use]
pub fn render_doctor(checks: &[DoctorCheck]) -> String {
    human::doctor(checks)
}

/// Formats retained bootstrap log lines followed by a stable completion line.
#[must_use]
pub fn render_bootstrap(host: &str, job: &str, lines: &[String]) -> String {
    let mut output = lines.to_vec();
    output.push(format!("Bootstrapped {host} ({job})"));
    output.join("\n")
}

fn value_or_dash(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn link_label(link: LinkState) -> &'static str {
    match link {
        LinkState::Connecting => "connecting",
        LinkState::Ready => "ready",
        LinkState::Down => "down",
        LinkState::Legacy => "legacy",
    }
}

fn agent_label(status: &HostStatus) -> String {
    let Some(agents) = &status.agent_binaries else {
        return "-".to_owned();
    };
    let mut available = Vec::new();
    if agents.claude {
        available.push("claude");
    }
    if agents.codex {
        available.push("codex");
    }
    if agents.opencode {
        available.push("opencode");
    }
    if available.is_empty() {
        "none".to_owned()
    } else {
        available.join(",")
    }
}
