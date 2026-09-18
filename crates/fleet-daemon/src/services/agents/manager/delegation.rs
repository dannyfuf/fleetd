//! The seams the delegation service calls, and the facts the transition half reads.
//!
//! `docs/NATIVE-AGENTS.md` §15 gives the delegation service two jobs the manager must not do for
//! it — deciding a delegation's state, and delivering its result — and three jobs it must not do
//! for itself: minting a sequence, writing a transcript row, and reading a thread's reducer
//! state. These are the verbs that split the two. Every one of them takes the thread's
//! serialized-operation gate exactly like [`super::commands`] does, so the service can never
//! reach the write path around it.
//!
//! [`delegation_facts`] is the other direction: the write path calls it *before* an event is
//! applied and hands the answer to the store, because the pure rules in
//! [`crate::services::agents::delegation::transition`] run inside the writer's transaction, where
//! no projection is in reach.

use fleet_core::agents::{
    AgentEvent, ItemId, ItemKind, ItemPatch, ItemStatus, ThreadId, ThreadProjection, ToolCall,
    ToolKind, TurnId, TurnState,
};
use fleet_proto::error::ProtoError;

use crate::services::agents::delegation::transition::DelegationFacts;

use super::{AgentSessionManager, AgentThreadRecord, conflict};

/// How much of the child's last assistant message the transition half is given.
///
/// It is a fallback result and a headline, never the transcript: four kilobytes is more than a
/// report's first paragraph and small enough to carry on every event of a delegated child.
const LAST_ASSISTANT_TEXT_CAP: usize = 4 * 1024;

/// The label every event these verbs append carries when the provider gave none.
const RAW: &str = "delegation";

impl AgentSessionManager {
    /// The turn this thread is running, or `None` when it is idle.
    ///
    /// Taken under the operation gate so the answer is not read between a `send` and the
    /// announcement it is waiting for: `run` refuses a caller that is not inside a turn, and a
    /// racy `None` there would refuse a caller that is perfectly able to host a delegation row.
    pub async fn running_turn(&self, thread: ThreadId) -> Result<Option<TurnId>, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let _operation = runtime.operation.lock().await;
        let state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(match state.projection.turn {
            TurnState::Running(turn) => Some(turn),
            _ => None,
        })
    }

    /// Appends one `ItemStarted` under `turn` and answers the item it minted.
    ///
    /// Refused when `turn` is not the running turn: an item under a settled turn would be a row
    /// the reducer accepts and no client draws in a live position, and the delegation row has to
    /// land in the turn the caller is speaking in.
    pub async fn append_item(
        &self,
        thread: ThreadId,
        turn: TurnId,
        kind: ItemKind,
    ) -> Result<ItemId, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        let running = matches!(
            runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .projection
                .turn,
            TurnState::Running(active) if active == turn
        );
        if !running {
            return Err(conflict(format!(
                "agent thread {thread} is not running turn {turn}"
            )));
        }
        let item = ItemId::new();
        self.apply_one(
            &runtime,
            &operation,
            AgentEvent::ItemStarted {
                turn,
                item,
                kind,
                parent: None,
            },
            Some(RAW.to_owned()),
        )
        .await?;
        Ok(item)
    }

    /// Patches one existing item and, when `complete` is given, settles it in the same operation.
    ///
    /// The two events are one verb because the mirror of a terminal delegation is exactly that
    /// pair, and a client that saw the patch without the settlement would draw a finished
    /// delegation as a running row until the next event arrived.
    pub async fn patch_item(
        &self,
        thread: ThreadId,
        item: ItemId,
        patch: ItemPatch,
        complete: Option<ItemStatus>,
    ) -> Result<(), ProtoError> {
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        self.apply_one(
            &runtime,
            &operation,
            AgentEvent::ItemUpdated { item, patch },
            Some(RAW.to_owned()),
        )
        .await?;
        if let Some(status) = complete {
            self.apply_one(
                &runtime,
                &operation,
                AgentEvent::ItemCompleted { item, status },
                Some(RAW.to_owned()),
            )
            .await?;
        }
        Ok(())
    }

    /// One thread's durable metadata, hydrating it if this is its first use.
    pub async fn record(&self, thread: ThreadId) -> Result<AgentThreadRecord, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let record = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record
            .clone();
        Ok(record)
    }

    /// One thread's reducer state, hydrating it if this is its first use.
    pub async fn projection(&self, thread: ThreadId) -> Result<ThreadProjection, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let projection = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .projection
            .clone();
        Ok(projection)
    }
}

/// What the transition half needs to know about a child thread, read before the event applies.
///
/// Empty for a thread that is not a delegated child, and that emptiness is load-bearing rather
/// than an optimisation detail: the rules read these facts only about a child, and gathering
/// them costs a walk of the transcript, which no ordinary thread may pay once per streamed
/// delta.
pub(crate) fn delegation_facts(
    projection: &ThreadProjection,
    record: &AgentThreadRecord,
) -> DelegationFacts {
    if record.delegation.is_none() {
        return DelegationFacts::default();
    }
    let last_assistant_text = projection
        .items
        .iter()
        .rev()
        .find_map(|item| match &item.kind {
            ItemKind::AssistantText { text } if !text.trim().is_empty() => Some(text),
            _ => None,
        })
        .map(|text| head(text, LAST_ASSISTANT_TEXT_CAP));
    let mut files_changed: Vec<String> = Vec::new();
    for item in &projection.items {
        let ItemKind::Tool(call) = &item.kind else {
            continue;
        };
        for path in tool_paths(call) {
            if !files_changed.contains(&path) {
                files_changed.push(path);
            }
        }
    }
    DelegationFacts {
        // Every gate kind the reducer tracks is one of the three §15 calls blocking, so an open
        // gate of any kind is the blocked signal.
        gate_open: !projection.gates.is_empty(),
        background_live: !projection.background_tasks.is_empty(),
        last_assistant_text,
        files_changed,
        stop_cause: record.stop_cause,
    }
}

/// The worktree-relative or absolute paths one edit-shaped tool call writes.
///
/// Provider-neutral by reading both shapes rather than by branching on the harness: Codex's
/// `fileChange` item maps to `{"paths": [...]}` and Claude's `Edit`/`Write`/`NotebookEdit` to a
/// single `file_path`. A tool that is not an edit, or names no path, yields nothing.
pub(super) fn tool_paths(call: &ToolCall) -> Vec<String> {
    if !matches!(call.kind, ToolKind::Edit | ToolKind::Write) {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(listed) = call.input.get("paths").and_then(|value| value.as_array()) {
        paths.extend(
            listed
                .iter()
                .filter_map(|value| value.as_str())
                .map(ToOwned::to_owned),
        );
    }
    for key in ["file_path", "notebook_path", "path"] {
        if let Some(path) = call.input.get(key).and_then(|value| value.as_str()) {
            paths.push(path.to_owned());
        }
    }
    paths
}

/// The first `cap` bytes of `text`, cut on a character boundary.
fn head(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
