//! `DelegationRun`: validate, mint the token, create the child, seed the transcript.
//!
//! The body belongs to stage `service-run` (phase 3, P3-T03). The shape is here so every other
//! half of the service — dispatch, the worker, the CLI — can be written against it first.

use fleet_proto::{error::ProtoError, response::ResponseBody};

use super::{DelegationService, RunRequest, unsupported};

impl DelegationService {
    /// Starts one delegation: a child thread, a durable record, and a row in the caller's
    /// transcript, in that order.
    pub(crate) async fn run(&self, _request: RunRequest) -> Result<ResponseBody, ProtoError> {
        Err(unsupported("run"))
    }
}
