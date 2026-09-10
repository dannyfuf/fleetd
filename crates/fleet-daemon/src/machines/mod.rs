//! Machine transport providers, daemon links, and the configured registry.

mod child;
mod command;
mod legacy;
pub mod link;
mod provider;
mod registry;
pub mod ssh;
pub mod tailscale;

pub use child::ChildStream;
pub use command::CommandMachine;
pub use legacy::LegacyMachine;
pub use link::{LinkOptions, RemoteEndpoint, RemoteHello, RemoteLink, WRITE_BUDGET};
pub use provider::{
    AsyncDuplex, ExecOutput, MachineAddress, MachineError, MachineLifecycle, MachineProvider,
    ProbeReport,
};
pub use registry::Machines;
pub use tailscale::TailscaleMachine;
