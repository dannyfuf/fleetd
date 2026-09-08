//! Mixed-host lifecycle fanout merge hook skeleton.

use fleet_core::ids::HostId;
use fleet_proto::{request::RequestBody, response::ResponseBody};

use crate::DaemonResult;

pub(crate) fn merge_lifecycle_fanout(
    _original: &RequestBody,
    _parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> Option<DaemonResult<ResponseBody>> {
    None
}
