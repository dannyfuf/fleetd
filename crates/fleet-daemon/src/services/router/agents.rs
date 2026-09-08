//! Native-agent routing hooks.

use fleet_proto::request::RequestBody;

use super::{Resolver, Target};

#[must_use]
pub(crate) fn classify_agent(body: &RequestBody, resolver: &dyn Resolver) -> Target {
    use RequestBody::*;
    match body {
        AgentThreadCreate { worktree, .. } => resolver
            .host_of_worktree(worktree)
            .map_or(Target::Local, Target::Host),
        AgentThreadOpen { thread, .. }
        | AgentThreadClose { thread }
        | AgentSend { thread, .. }
        | AgentInterrupt { thread }
        | AgentRespond { thread, .. }
        | AgentSetMode { thread, .. }
        | AgentSetModel { thread, .. }
        | AgentMarkSeen { thread, .. }
        | AgentStop { thread } => resolver
            .host_of_thread(thread)
            .map_or(Target::Local, Target::Host),
        AgentThreadList => Target::Local,
        _ => Target::Unsupported("not an agent request"),
    }
}

pub(crate) fn register_thread_events() -> crate::DaemonResult<()> {
    Err(crate::DaemonError::Unsupported(
        "register_thread_events: not implemented".to_owned(),
    ))
}
