//! Terminal attachment forwarding hook skeletons.

use fleet_core::ids::{HostId, TerminalId};

use crate::{DaemonError, DaemonResult};

pub(crate) fn on_attach(_host: &HostId, _local: TerminalId) -> DaemonResult<()> {
    Err(DaemonError::Unsupported(
        "on_attach: not implemented".to_owned(),
    ))
}

pub(crate) fn on_detach(_host: &HostId, _local: TerminalId) -> DaemonResult<()> {
    Err(DaemonError::Unsupported(
        "on_detach: not implemented".to_owned(),
    ))
}
