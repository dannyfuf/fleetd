//! Remote repository-ensure and worktree-create hook skeleton.

use fleet_core::{
    ids::{HostId, RepoId},
    model::RepoHooks,
};
use fleet_proto::response::ResponseBody;

use crate::{DaemonError, DaemonResult};

use super::Router;

pub(crate) async fn ensure_repo_then_create(
    _router: &Router,
    _host: &HostId,
    _repo: RepoId,
    _slug: String,
    _branch: Option<String>,
    _base: Option<String>,
    _hooks: RepoHooks,
) -> DaemonResult<ResponseBody> {
    Err(DaemonError::Unsupported(
        "ensure_repo_then_create: not implemented".to_owned(),
    ))
}
