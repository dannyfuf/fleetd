use super::{CommandOutput, FAILURE, UPDATE_RESTART, unknown, wait_event};
use crate::{
    args::DoctorArgs,
    envelope::{PROTOCOL, ResetStateEnvelope, to_json},
    human,
};
use fleet_client::Client;
use fleet_core::ids::JobId;
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    job::{JobRecord, JobStatus},
};

pub(super) async fn doctor(
    client: &Client,
    arguments: DoctorArgs,
) -> Result<CommandOutput, ProtoError> {
    if arguments.reset_state {
        let archived_path = client.reset_state().await?;
        let text = if arguments.json {
            to_json(&ResetStateEnvelope {
                protocol: PROTOCOL,
                archived_path: &archived_path,
            })?
        } else {
            format!("Reset Fleet state; archived broken state at {archived_path}")
        };
        return Ok(CommandOutput::success(text));
    }
    let checks = client.doctor().await?;
    let ok = checks
        .iter()
        .all(|check| check.status != fleet_proto::response::DoctorStatus::Fail);
    Ok(CommandOutput::with_exit_code(
        human::doctor(&checks),
        if ok { 0 } else { FAILURE },
    ))
}

pub(super) async fn import_from_swarm(client: &Client) -> Result<CommandOutput, ProtoError> {
    let job = client.import_from_swarm().await?;
    Ok(CommandOutput::success(format!("Import started {}", job.id)))
}

pub(super) async fn update(client: &Client) -> Result<CommandOutput, ProtoError> {
    let job = client.update().await?;
    wait_for_job(client, &job, "update").await?;
    Ok(CommandOutput::with_exit_code(
        format!("Updated {}", job.id),
        UPDATE_RESTART,
    ))
}

pub(super) async fn wait_for_job(
    client: &Client,
    initial: &JobRecord,
    operation: &str,
) -> Result<(), ProtoError> {
    let mut events = client.events();
    let mut status = initial.status.clone();
    let mut baseline = true;
    loop {
        match status {
            JobStatus::Succeeded => return Ok(()),
            JobStatus::Failed { error } => {
                return Err(unknown(format!("{operation} failed: {error}")));
            }
            JobStatus::Cancelled => {
                return Err(ProtoError {
                    kind: ErrorKind::Cancelled,
                    message: format!("{operation} was cancelled"),
                });
            }
            JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling => {}
        }
        let event = if baseline {
            baseline = false;
            None
        } else {
            wait_event(&mut events, |event| match event {
                Event::JobUpdated(job) => job.id == initial.id,
                _ => false,
            })
            .await?
        };
        status = match event {
            Some(Event::JobUpdated(job)) => job.status,
            _ => {
                client
                    .list_jobs()
                    .await?
                    .into_iter()
                    .find(|job| job.id == initial.id)
                    .ok_or_else(|| missing_job(&initial.id))?
                    .status
            }
        };
    }
}

fn missing_job(id: &JobId) -> ProtoError {
    ProtoError {
        kind: ErrorKind::NotFound,
        message: format!("update job `{id}` is no longer available"),
    }
}
