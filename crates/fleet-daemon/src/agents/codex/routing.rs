//! Subagent demultiplexing: one pure function and its three guards.
//!
//! Under Codex multi-agent a subagent is a **full app-server thread on the same stdio**, so the
//! notification stream is N threads interleaved and the adapter has to demultiplex. The routing
//! table below is t3code's, copied deliberately and tested against a captured wire trace — the
//! single best idea in that codebase — with **unknown methods degrading to `Parent`, never to
//! silent loss**. Two shipped t3code bugs came from a catch-all here.

use std::collections::{HashMap, HashSet};

/// Where a notification from a foreign thread belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildRoute {
    /// The parent thread sees it: it moves the parent's own state.
    Parent,
    /// It becomes a subagent activity row, attributed to the child.
    AgentEvent,
    /// Chatter: a delta or a parent-state mutator a child must not fire.
    Drop,
}

/// Methods that describe a child's own progress and become subagent rows.
pub const CHILD_AGENT_EVENT_METHODS: [&str; 10] = [
    "turn/started",
    "turn/completed",
    "thread/status/changed",
    "thread/tokenUsage/updated",
    "thread/settings/updated",
    "model/rerouted",
    "item/started",
    "item/completed",
    "thread/closed",
    "error",
];

/// Methods a child must never be allowed to fire at the parent.
///
/// The last four are **parent-state mutators**: a child firing one would let it rewrite the
/// parent's own archive state, compaction state or identity.
pub const CHILD_CHATTER_METHODS: [&str; 14] = [
    "item/agentMessage/delta",
    "item/reasoning/summaryTextDelta",
    "item/reasoning/summaryPartAdded",
    "item/reasoning/textDelta",
    "item/plan/delta",
    "item/commandExecution/outputDelta",
    "item/fileChange/patchUpdated",
    "item/mcpToolCall/progress",
    "turn/plan/updated",
    "turn/diff/updated",
    "thread/archived",
    "thread/unarchived",
    "thread/compacted",
    "thread/started",
];

/// Where a notification from a **registered child** thread belongs.
#[must_use]
pub fn route_child_notification(method: &str) -> ChildRoute {
    if CHILD_AGENT_EVENT_METHODS.contains(&method) {
        return ChildRoute::AgentEvent;
    }
    if CHILD_CHATTER_METHODS.contains(&method) {
        return ChildRoute::Drop;
    }
    // Unknown methods degrade to "the parent sees it", never to silent loss.
    ChildRoute::Parent
}

/// Methods suppressed for a thread that is neither the root nor a registered child.
///
/// Child traffic can precede its registration — a child's `thread/status/changed` arriving before
/// anything registers that child is in the captured trace — so these are suppressed for an
/// unregistered foreign thread **while its live turn id is still recorded**, so Stop reaches it
/// regardless of registration timing. A false-positive entry costs one ignored RPC; a missing one
/// costs a runaway agent.
pub const UNREGISTERED_SUPPRESSED: [&str; 6] = [
    "turn/started",
    "turn/completed",
    "turn/plan/updated",
    "model/rerouted",
    "thread/tokenUsage/updated",
    "thread/closed",
];

/// The subagent registry for one root thread.
#[derive(Debug, Default)]
pub struct Subagents {
    /// Registered child threads, by provider thread id.
    children: HashSet<String>,
    /// The live turn of every foreign thread, registered or not, so Stop can reach it.
    live_turns: HashMap<String, String>,
}

impl Subagents {
    /// Registers a child thread of `root`.
    ///
    /// **Never registers the root as its own child.** A `subAgentActivity` item describing the
    /// root can arrive on a child's stream, and registering it made t3code intercept the parent's
    /// own final message and `turn/completed`, hanging the thread as "working" forever.
    pub fn register(&mut self, root: &str, child: &str, agent_path: Option<&str>) -> bool {
        if child.is_empty() || child == root || agent_path == Some("/root") {
            tracing::debug!(
                target: "fleet::agents::codex",
                "refusing to register a Codex thread as its own subagent"
            );
            return false;
        }
        self.children.insert(child.to_owned())
    }

    /// Whether `thread` is a registered child.
    #[must_use]
    pub fn is_child(&self, thread: &str) -> bool {
        self.children.contains(thread)
    }

    /// Records a foreign thread's live turn, registered or not.
    pub fn note_live_turn(&mut self, thread: &str, turn: &str) {
        self.live_turns.insert(thread.to_owned(), turn.to_owned());
    }

    /// Forgets a foreign thread's live turn, on `thread/closed` or a terminal error.
    pub fn clear_live_turn(&mut self, thread: &str) {
        self.live_turns.remove(thread);
    }

    /// Every `(thread, turn)` Stop has to reach before the parent.
    #[must_use]
    pub fn live_turns(&self) -> Vec<(String, String)> {
        let mut turns = self
            .live_turns
            .iter()
            .map(|(thread, turn)| (thread.clone(), turn.clone()))
            .collect::<Vec<_>>();
        turns.sort();
        turns
    }

    /// How a notification for `thread` should be routed, given the root thread.
    #[must_use]
    pub fn route(&self, root: &str, thread: &str, method: &str) -> ChildRoute {
        if thread == root {
            return ChildRoute::Parent;
        }
        if self.is_child(thread) {
            return route_child_notification(method);
        }
        // Unregistered foreign thread: thread-level notifications are suppressed, everything
        // else still reaches the parent so nothing is lost silently.
        if UNREGISTERED_SUPPRESSED.contains(&method) || method.starts_with("thread/") {
            return ChildRoute::Drop;
        }
        route_child_notification(method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "01a089f2-5337-7470-adb1-219e71d62a35";
    const CHILD: &str = "01a089f3-0000-7000-8000-000000000001";

    #[test]
    fn an_unknown_method_reaches_the_parent_rather_than_being_dropped() {
        assert_eq!(
            route_child_notification("thread/somethingBrandNew"),
            ChildRoute::Parent
        );
        assert_eq!(route_child_notification("error"), ChildRoute::AgentEvent);
        assert_eq!(
            route_child_notification("item/agentMessage/delta"),
            ChildRoute::Drop
        );
    }

    /// Guard 1: the root is never its own child, however it is named.
    #[test]
    fn the_root_is_never_registered_as_its_own_child() {
        let mut subagents = Subagents::default();
        assert!(!subagents.register(ROOT, ROOT, None));
        assert!(!subagents.register(ROOT, CHILD, Some("/root")));
        assert!(!subagents.register(ROOT, "", None));
        assert!(subagents.register(ROOT, CHILD, Some("/root/explorer")));
        assert!(subagents.is_child(CHILD));
        // The parent's own `turn/completed` still reaches the parent.
        assert_eq!(
            subagents.route(ROOT, ROOT, "turn/completed"),
            ChildRoute::Parent
        );
    }

    /// Guard 2: child traffic can precede its registration.
    #[test]
    fn an_unregistered_childs_thread_events_are_suppressed_but_its_turn_is_recorded() {
        let mut subagents = Subagents::default();
        assert_eq!(
            subagents.route(ROOT, CHILD, "thread/status/changed"),
            ChildRoute::Drop
        );
        assert_eq!(
            subagents.route(ROOT, CHILD, "turn/started"),
            ChildRoute::Drop
        );
        // …and Stop still reaches it.
        subagents.note_live_turn(CHILD, "turn-1");
        assert_eq!(
            subagents.live_turns(),
            vec![(CHILD.to_owned(), "turn-1".to_owned())]
        );
        subagents.clear_live_turn(CHILD);
        assert!(subagents.live_turns().is_empty());
    }

    /// The captured wire trace: every method the real 0.147.0 capture produced, routed.
    #[test]
    fn the_captured_trace_routes_method_by_method() {
        let mut subagents = Subagents::default();
        subagents.register(ROOT, CHILD, Some("/root/explorer"));
        let trace = [
            // (thread, method, expected)
            (ROOT, "thread/started", ChildRoute::Parent),
            (ROOT, "thread/status/changed", ChildRoute::Parent),
            (ROOT, "turn/started", ChildRoute::Parent),
            (ROOT, "item/started", ChildRoute::Parent),
            (ROOT, "item/agentMessage/delta", ChildRoute::Parent),
            (ROOT, "item/completed", ChildRoute::Parent),
            (ROOT, "thread/tokenUsage/updated", ChildRoute::Parent),
            (ROOT, "account/rateLimits/updated", ChildRoute::Parent),
            (ROOT, "turn/completed", ChildRoute::Parent),
            // The same methods from a registered child.
            (CHILD, "thread/started", ChildRoute::Drop),
            (CHILD, "thread/status/changed", ChildRoute::AgentEvent),
            (CHILD, "turn/started", ChildRoute::AgentEvent),
            (CHILD, "item/started", ChildRoute::AgentEvent),
            (CHILD, "item/agentMessage/delta", ChildRoute::Drop),
            (CHILD, "item/completed", ChildRoute::AgentEvent),
            (CHILD, "thread/tokenUsage/updated", ChildRoute::AgentEvent),
            (CHILD, "turn/completed", ChildRoute::AgentEvent),
            (CHILD, "error", ChildRoute::AgentEvent),
            (CHILD, "thread/compacted", ChildRoute::Drop),
        ];
        for (thread, method, expected) in trace {
            assert_eq!(
                subagents.route(ROOT, thread, method),
                expected,
                "{method} from {thread}"
            );
        }
    }
}
