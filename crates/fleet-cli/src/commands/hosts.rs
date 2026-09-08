//! Configured-host CLI skeleton.

use fleet_client::Client;
use fleet_proto::error::ProtoError;

use crate::args::HostArgs;

use super::CommandOutput;

pub async fn run(_client: &Client, _arguments: HostArgs) -> Result<CommandOutput, ProtoError> {
    Ok(CommandOutput::with_exit_code(
        "fleet host: not implemented".to_owned(),
        1,
    ))
}
