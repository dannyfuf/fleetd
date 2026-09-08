//! Request, response, event, and fanout translation skeletons.

use fleet_core::ids::HostId;
use fleet_proto::{event::Event, request::RequestBody, response::ResponseBody};

use crate::{DaemonError, DaemonResult};

use super::RemoteIds;

pub fn to_remote(body: RequestBody, _host: &HostId, _ids: &RemoteIds) -> RequestBody {
    body
}

pub fn response_to_local(body: ResponseBody, _host: &HostId, _ids: &RemoteIds) -> ResponseBody {
    body
}

pub fn event_to_local(event: Event, _host: &HostId, _ids: &RemoteIds) -> Option<Event> {
    Some(event)
}

pub fn merge_fanout(
    _original: &RequestBody,
    _parts: Vec<(HostId, DaemonResult<ResponseBody>)>,
) -> DaemonResult<ResponseBody> {
    Err(DaemonError::Unsupported(
        "merge_fanout: not implemented".to_owned(),
    ))
}
