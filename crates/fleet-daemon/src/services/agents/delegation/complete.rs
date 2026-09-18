//! `DelegationComplete`: verify the token, record the result, finish an already-settled child.
//!
//! The body belongs to stage `service-complete` (phase 3, P3-T04).

use fleet_proto::{error::ProtoError, response::ResponseBody};

use super::{CompleteRequest, DelegationService, unsupported};

impl DelegationService {
    /// Accepts the child's report exactly once, trusting nothing but the delegation token.
    pub(crate) async fn complete(
        &self,
        _request: CompleteRequest,
    ) -> Result<ResponseBody, ProtoError> {
        Err(unsupported("complete"))
    }
}
