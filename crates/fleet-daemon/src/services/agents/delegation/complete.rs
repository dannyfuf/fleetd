//! `DelegationComplete`: verify the token, record the result, finish an already-settled child.

use chrono::Utc;
use fleet_core::agents::{Delegation, DelegationResult, DelegationStatus, ResultSource};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::ResponseBody,
};
use rusqlite::Transaction;
use sha2::{Digest, Sha256};

use crate::services::agents::store::{self, OutboxAction};

use super::{CompleteRequest, DelegationService, limits::RESULT_CAP_BYTES};

impl DelegationService {
    /// Accepts the child's report exactly once, trusting nothing but the delegation token.
    pub(crate) async fn complete(
        &self,
        request: CompleteRequest,
    ) -> Result<ResponseBody, ProtoError> {
        let token_sha256 = digest(request.token.as_bytes());
        let report_sha256 = hex_digest(request.result.as_bytes());

        // Validate before reading the child projection. Besides avoiding unnecessary hydration,
        // this keeps an unauthenticated request from probing whether a child thread exists.
        let checked_delegation = request.delegation;
        let checked_child = request.child;
        let checked_report_sha256 = report_sha256.clone();
        let checked = self
            .inner
            .store
            .delegation_write("validate delegation report", move |tx| {
                Ok((
                    inspect(
                        tx,
                        checked_delegation,
                        checked_child,
                        &token_sha256,
                        &checked_report_sha256,
                    )?,
                    false,
                ))
            })
            .await
            .map_err(storage_error)?;
        let current = match checked {
            Inspection::Accept(current) => current,
            Inspection::Idempotent(current) => {
                return Ok(ResponseBody::Delegation(current));
            }
            Inspection::Refused(error) => return Err(error),
        };

        // A completed turn can reach the store before its CLI report does. Only that state needs
        // the reducer projection; every other completion avoids hydrating the child.
        let background_live = if current.status == DelegationStatus::Settling {
            !self
                .inner
                .manager
                .projection(current.child)
                .await?
                .background_tasks
                .is_empty()
        } else {
            false
        };

        let now = Utc::now();
        let accepted_request = request;
        let accepted_report_sha256 = report_sha256;
        let outcome = self
            .inner
            .store
            .delegation_write("complete delegation", move |tx| {
                let inspection = inspect(
                    tx,
                    accepted_request.delegation,
                    accepted_request.child,
                    &token_sha256,
                    &accepted_report_sha256,
                )?;
                let mut delegation = match inspection {
                    Inspection::Accept(delegation) => delegation,
                    Inspection::Idempotent(delegation) => {
                        return Ok((Completion::Idempotent(delegation), false));
                    }
                    Inspection::Refused(error) => {
                        return Ok((Completion::Refused(error), false));
                    }
                };

                let result = reported_result(&accepted_request.result);
                store::delegations::set_report(
                    tx,
                    delegation.id,
                    &result,
                    &accepted_report_sha256,
                    now,
                )?;
                delegation.result = Some(result);

                let wake = if accepted_request.blocked {
                    delegation.status = DelegationStatus::Blocked;
                    delegation.status_payload = Some("reported blocked".to_owned());
                    store::delegations::update(tx, &delegation)?;
                    false
                } else if delegation.status == DelegationStatus::Settling && !background_live {
                    delegation.status = DelegationStatus::Succeeded;
                    delegation.status_payload = None;
                    delegation.finished = Some(now);
                    store::delegations::update(tx, &delegation)?;
                    store::delegations::enqueue(tx, delegation.id, OutboxAction::Deliver, now)?;
                    true
                } else {
                    false
                };

                Ok((Completion::Changed(delegation), wake))
            })
            .await
            .map_err(storage_error)?;

        let delegation = match outcome {
            Completion::Changed(delegation) => {
                self.publish_changed(delegation.clone());
                delegation
            }
            Completion::Idempotent(delegation) => delegation,
            Completion::Refused(error) => return Err(error),
        };
        Ok(ResponseBody::Delegation(delegation))
    }
}

#[derive(Debug)]
enum Inspection {
    Accept(Delegation),
    Idempotent(Delegation),
    Refused(ProtoError),
}

#[derive(Debug)]
enum Completion {
    Changed(Delegation),
    Idempotent(Delegation),
    Refused(ProtoError),
}

fn inspect(
    tx: &Transaction<'_>,
    delegation_id: fleet_core::agents::DelegationId,
    child: fleet_core::agents::ThreadId,
    token_sha256: &[u8; 32],
    report_sha256: &str,
) -> anyhow::Result<Inspection> {
    let Some(delegation) = store::delegations::get(tx, delegation_id)? else {
        return Ok(Inspection::Refused(not_found(format!(
            "delegation {} does not exist",
            delegation_id
        ))));
    };
    let stored_token =
        store::delegations::token_hash(tx, delegation_id)?.and_then(|hash| decode_digest(&hash));
    if !stored_token.is_some_and(|stored| constant_time_eq(token_sha256, &stored)) {
        return Ok(Inspection::Refused(validation(
            "delegation token does not match",
        )));
    }
    if child != delegation.child {
        return Ok(Inspection::Refused(validation(
            "this delegation belongs to another thread",
        )));
    }

    // A report may have made the delegation terminal in the same commit that first accepted it.
    // Judge repeats before the generic terminal refusal so an identical retry stays idempotent.
    if let Some((first_hash, reported_at)) = store::delegations::report_meta(tx, delegation_id)? {
        return if first_hash == report_sha256 {
            Ok(Inspection::Idempotent(delegation))
        } else {
            Ok(Inspection::Refused(conflict(format!(
                "delegation {} already reported at {}",
                delegation_id,
                reported_at.to_rfc3339()
            ))))
        };
    }
    if delegation.status.is_terminal() {
        return Ok(Inspection::Refused(conflict(format!(
            "delegation {} is already terminal ({})",
            delegation_id,
            delegation.status.word()
        ))));
    }
    Ok(Inspection::Accept(delegation))
}

fn reported_result(text: &str) -> DelegationResult {
    let elided = text.len() > RESULT_CAP_BYTES;
    let text = if elided {
        let mut end = RESULT_CAP_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    } else {
        text.to_owned()
    };
    DelegationResult {
        text,
        files_changed: Vec::new(),
        source: ResultSource::Reported,
        elided,
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decode_digest(encoded: &str) -> Option<[u8; 32]> {
    if encoded.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(decoded)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn storage_error(error: anyhow::Error) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Fs,
        message: one_line(&format!("native-agent storage failed: {error:#}")),
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

fn error(kind: ErrorKind, message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind,
        message: one_line(&message.into()),
    }
}

fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}
