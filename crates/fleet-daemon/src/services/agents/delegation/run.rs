//! `DelegationRun`: validate, mint the token, create the child, seed the transcript.

use std::collections::BTreeMap;

use chrono::Utc;
use fleet_core::agents::{
    Delegation, DelegationId, DelegationStatus, DeliveryState, ItemId, ItemKind, MessageOrigin,
    PermissionMode, UserInput,
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::ResponseBody,
};
use sha2::{Digest as _, Sha256};

use crate::{
    DaemonError,
    services::agents::{
        manager::CreateOptions,
        store::{OutboxAction, delegations},
    },
};

use super::{
    DelegationService, RunRequest,
    footer::{SAME_WORKTREE_WARNING, child_title, first_message},
    limits::{MAX_DEPTH, MAX_LIVE_CHILDREN_PER_CALLER, MAX_LIVE_DELEGATIONS},
};

impl DelegationService {
    /// Starts one delegation: a child thread, a durable record, and a row in the caller's
    /// transcript.
    pub(crate) async fn run(&self, request: RunRequest) -> Result<ResponseBody, ProtoError> {
        let caller = request.caller;
        let caller_exists = self
            .inner
            .store
            .delegation_write("check delegation caller", move |tx| {
                Ok((delegations::caller_exists(tx, caller)?, false))
            })
            .await
            .map_err(storage_error)?;
        if !caller_exists {
            return Err(not_found(format!(
                "caller-exists rule: delegation caller {caller} does not exist"
            )));
        }

        if let Some(owner) = self.inner.manager.owner_of(caller).await? {
            return Err(unsupported(format!(
                "caller-locality rule: delegation caller {caller} is a mirror owned by host {owner}"
            )));
        }

        let caller_turn = self
            .inner
            .manager
            .running_turn(caller)
            .await?
            .ok_or_else(|| {
                conflict(format!(
                    "running-turn rule: delegation caller {caller} has no running turn"
                ))
            })?;

        let caller_depth = self
            .inner
            .store
            .delegation_by_child(caller)
            .await
            .map_err(storage_error)?
            .map_or(0, |delegation| delegation.depth);
        if caller_depth >= MAX_DEPTH {
            return Err(conflict(format!(
                "depth-limit rule: caller depth {caller_depth} has reached the maximum {MAX_DEPTH}"
            )));
        }

        let live_children = self
            .inner
            .store
            .live_delegations(Some(caller))
            .await
            .map_err(storage_error)?;
        if live_children.len() >= MAX_LIVE_CHILDREN_PER_CALLER {
            return Err(conflict(format!(
                "live-child-limit rule: caller {caller} already has {} live children (maximum {MAX_LIVE_CHILDREN_PER_CALLER})",
                live_children.len()
            )));
        }

        let live_delegations = self
            .inner
            .store
            .live_delegations(None)
            .await
            .map_err(storage_error)?;
        if live_delegations.len() >= MAX_LIVE_DELEGATIONS {
            return Err(conflict(format!(
                "daemon-live-limit rule: this daemon already has {} live delegations (maximum {MAX_LIVE_DELEGATIONS})",
                live_delegations.len()
            )));
        }

        let config = self.inner.config.load().await.map_err(|error| {
            validation(format!(
                "provider-binary rule: could not read agent binary configuration: {error}"
            ))
        })?;
        let binary = config.agent_binaries.binary(request.provider);
        if binary.trim().is_empty() {
            return Err(unsupported(format!(
                "provider-binary rule: {} has no configured executable",
                request.provider.display_name()
            )));
        }
        crate::agents::harness::process::command_parts(binary).map_err(|error| {
            unsupported(format!(
                "provider-binary rule: the configured {} executable is invalid: {error}",
                request.provider.display_name()
            ))
        })?;

        let caller_record = self.inner.manager.record(caller).await?;
        let worktree = request
            .worktree
            .clone()
            .unwrap_or_else(|| caller_record.worktree.clone());
        self.inner
            .worktrees
            .path(worktree.clone())
            .await
            .map_err(|error| worktree_error(&worktree, error))?;

        let delegation_id = DelegationId::new();
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let token_sha256 = format!("{:x}", Sha256::digest(token.as_bytes()));
        let title = request
            .title
            .clone()
            .unwrap_or_else(|| child_title(request.provider, &request.brief));
        let extra_env = BTreeMap::from([
            ("FLEET_DELEGATION".to_owned(), delegation_id.to_string()),
            ("FLEET_DELEGATION_TOKEN".to_owned(), token),
        ]);
        let child = fleet_core::agents::ThreadId::new();
        let caller_item = ItemId::new();
        let delegation = Delegation {
            id: delegation_id,
            caller,
            caller_turn,
            caller_item,
            child,
            provider: request.provider,
            depth: caller_depth + 1,
            brief: request.brief,
            expectation: request.expectation,
            eager: request.eager,
            status: DelegationStatus::Starting,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: Utc::now(),
            finished: None,
            headline: None,
        };

        let stored = delegation.clone();
        let refused = self
            .inner
            .store
            .delegation_write("reserve delegation", move |tx| {
                let refusal = delegations::reserve(
                    tx,
                    &stored,
                    &token_sha256,
                    MAX_LIVE_CHILDREN_PER_CALLER,
                    MAX_LIVE_DELEGATIONS,
                )?;
                Ok((refusal, false))
            })
            .await
            .map_err(storage_error)?;
        if let Some(message) = refused {
            return Err(conflict(message));
        }

        let created = self
            .inner
            .manager
            .create_with(CreateOptions {
                thread: Some(child),
                worktree: worktree.clone(),
                provider: request.provider,
                model: request.model,
                mode: request.mode.unwrap_or(PermissionMode::FullAccess),
                resume_cursor: None,
                title: Some(title),
                parent: Some(caller),
                delegation: Some(delegation_id),
                extra_env,
            })
            .await;
        let created = match created {
            Ok(created) => created,
            Err(error) => {
                self.release_reservation(delegation_id).await?;
                return Err(error);
            }
        };
        match created {
            ResponseBody::AgentThreadCreated(summary) if summary.thread == child => {}
            other => {
                self.release_reservation(delegation_id).await?;
                return Err(validation(format!(
                    "child-creation rule: manager returned an unexpected response: {other:?}"
                )));
            }
        }

        if let Err(error) = self
            .inner
            .manager
            .append_item(
                caller,
                caller_turn,
                caller_item,
                ItemKind::Delegation {
                    id: delegation.id,
                    provider: delegation.provider,
                    child,
                    status: DelegationStatus::Starting,
                },
            )
            .await
        {
            let cleanup = self
                .compensate_created_child(&delegation, false, "caller item append failed")
                .await;
            return Err(with_cleanup(error, cleanup));
        }

        if let Err(error) = self
            .inner
            .manager
            .send(
                child,
                UserInput {
                    text: first_message(&delegation.brief, delegation.id, &delegation.expectation),
                    origin: MessageOrigin::User,
                    ..UserInput::default()
                },
            )
            .await
        {
            let cleanup = self
                .compensate_created_child(&delegation, true, "initial child send failed")
                .await;
            return Err(with_cleanup(error, cleanup));
        }

        let warning =
            (worktree == caller_record.worktree).then(|| SAME_WORKTREE_WARNING.to_owned());
        Ok(ResponseBody::DelegationStarted {
            delegation,
            warning,
        })
    }

    async fn release_reservation(&self, delegation: DelegationId) -> Result<(), ProtoError> {
        self.inner
            .store
            .delegation_write("release delegation reservation", move |tx| {
                delegations::delete(tx, delegation)?;
                Ok(((), false))
            })
            .await
            .map_err(storage_error)
    }

    /// Stops a child whose creation succeeded but whose delegation setup did not finish.
    ///
    /// Before the caller row commits, the reservation is invisible and can be released. After it
    /// commits, the durable row must remain and become terminal even when provider shutdown also
    /// fails, so capacity is never held by an unreachable live child.
    async fn compensate_created_child(
        &self,
        delegation: &Delegation,
        caller_item_committed: bool,
        reason: &'static str,
    ) -> Result<(), ProtoError> {
        let stop = self.inner.manager.stop(delegation.child).await;
        if !caller_item_committed {
            let release = self.release_reservation(delegation.id).await;
            return match (stop, release) {
                (_, Ok(())) => Ok(()),
                (Ok(_), Err(error)) | (Err(_), Err(error)) => Err(error),
            };
        }
        if stop.is_ok() {
            return Ok(());
        }

        let id = delegation.id;
        let now = Utc::now();
        let reason = reason.to_owned();
        let changed = self
            .inner
            .store
            .delegation_write("terminalize failed delegation startup", move |tx| {
                let Some(mut current) = delegations::get(tx, id)? else {
                    anyhow::bail!("delegation {id} does not exist");
                };
                if !current.status.is_terminal() {
                    current.status = DelegationStatus::Failed;
                    current.status_payload = Some(reason);
                    current.finished = Some(now);
                    delegations::update(tx, &current)?;
                    delegations::mark_done_for(tx, id, OutboxAction::Recover, now)?;
                    delegations::enqueue(tx, id, OutboxAction::Deliver, now)?;
                }
                Ok((current, true))
            })
            .await
            .map_err(storage_error)?;
        self.publish_changed(changed);
        Ok(())
    }
}

fn with_cleanup(mut original: ProtoError, cleanup: Result<(), ProtoError>) -> ProtoError {
    if let Err(error) = cleanup {
        original.message = format!(
            "{}; child cleanup also failed: {}",
            original.message, error.message
        );
    }
    original
}

fn storage_error(error: anyhow::Error) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Fs,
        message: one_line(&format!("native-agent storage failed: {error:#}")),
    }
}

fn worktree_error(worktree: &fleet_core::ids::WorktreeId, error: DaemonError) -> ProtoError {
    let message =
        format!("worktree-resolution rule: worktree {worktree} could not be resolved: {error}");
    match error {
        DaemonError::NotFound(_) => not_found(message),
        DaemonError::Remote(_) | DaemonError::Unsupported(_) => unsupported(message),
        _ => validation(message),
    }
}

fn not_found(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::NotFound, message)
}

fn conflict(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::Conflict, message)
}

fn validation(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::Validation, message)
}

fn unsupported(message: impl Into<String>) -> ProtoError {
    error(ErrorKind::Unsupported, message)
}

fn error(kind: ErrorKind, message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind,
        message: one_line(&message.into()),
    }
}

fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}
