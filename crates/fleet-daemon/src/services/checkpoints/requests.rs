//! The request handlers and the periodic sweep, wired onto [`Services`].
//!
//! They live here rather than in `services/dispatch.rs` so the whole checkpoint feature is one
//! directory: dispatch holds two arms that name these two methods and nothing else.
//!
//! There is deliberately **no** capture entry point here. A capture needs the turn identifier,
//! which is minted inside `AgentSessionManager::send`, and that manager already holds the
//! `Worktrees` it takes to resolve a path — so the call belongs there, against a
//! [`Checkpoints`](super::Checkpoints) clone, and not behind a `Services` method the manager
//! cannot reach. `docs/NATIVE-AGENTS.md` §13 phase 8 records that seam.
//!
//! The thread → worktree lookup goes through the agent thread listing rather than the agent
//! manager's internals, because that listing is also where `host` comes from: a checkpoint of a
//! **remote** thread belongs to the daemon that owns its worktree, and routing already sends the
//! request there (`services/router/agents.rs`). The check here is the second guard, for the same
//! reason `AgentSessionManager` keeps one: a mutation that reached the wrong daemon must refuse,
//! never touch a local path that happens to exist.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use fleet_core::{agents::ThreadId, ids::WorktreeId};
use fleet_proto::{agents::CheckpointId, response::ResponseBody};
use tokio_util::sync::CancellationToken;

use crate::{DaemonError, DaemonResult, error::remote_unsupported, services::Services};

/// How often orphaned checkpoint refs are swept.
///
/// Long, because the sweep is a correctness backstop and not the primary path: a thread Fleet
/// deletes has its refs removed on the spot. What this catches is a daemon killed mid-deletion
/// and a database replaced underneath one — neither of which is urgent, and both of which cost a
/// `for-each-ref` per worktree to find.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

impl Services {
    /// Answers `AgentCheckpoints`.
    pub(crate) async fn agent_checkpoints(&self, thread: ThreadId) -> DaemonResult<ResponseBody> {
        let worktree = self.agent_thread_worktree(thread).await?;
        let checkpoints = self.checkpoints.list(&worktree, thread).await?;
        Ok(ResponseBody::AgentCheckpoints(
            checkpoints.iter().map(super::Checkpoint::to_wire).collect(),
        ))
    }

    /// Answers `AgentRevert`.
    pub(crate) async fn revert_agent_checkpoint(
        &self,
        thread: ThreadId,
        checkpoint: CheckpointId,
    ) -> DaemonResult<ResponseBody> {
        let worktree = self.agent_thread_worktree(thread).await?;
        let report = self
            .checkpoints
            .revert(&worktree, thread, &checkpoint)
            .await?;
        Ok(ResponseBody::AgentReverted(report))
    }

    /// Sweeps checkpoint refs whose thread this daemon no longer lists.
    ///
    /// The live set comes from `AgentThreadList` and **not** from `AgentSessionManager::summaries`,
    /// which answers an empty vector when the transcript database cannot be read. An empty live
    /// set means "every checkpoint is an orphan", so reading it from a call that cannot fail
    /// would turn one unreadable database into every thread's undo history deleted. A listing
    /// failure here skips the sweep entirely.
    pub(crate) async fn sweep_agent_checkpoints(&self) -> DaemonResult<usize> {
        let live = match self.agents.list().await {
            Ok(ResponseBody::AgentThreads(threads)) => threads,
            Ok(other) => {
                return Err(DaemonError::Protocol(format!(
                    "agent thread listing answered {other:?} instead of a thread list"
                )));
            }
            // A store that cannot be read keeps its own kind: `run_sweep` logs this and skips
            // the round, which is the whole point of asking a call that can fail.
            Err(error) => return Err(crate::error::from_proto_error(error)),
        };

        let mut by_worktree: HashMap<WorktreeId, HashSet<ThreadId>> = HashMap::new();
        for summary in live {
            // A mirrored thread's worktree — and therefore its checkpoints — lives on the owner,
            // and its refs are that daemon's to sweep.
            if summary.host.is_none() {
                by_worktree
                    .entry(summary.worktree)
                    .or_default()
                    .insert(summary.thread);
            }
        }

        let state = self.state.load().await?;
        let mut swept = 0;
        for worktree in state.worktrees {
            if worktree.host.is_some() {
                continue;
            }
            let path = PathBuf::from(&worktree.path);
            if !path.is_absolute() {
                continue;
            }
            let live = by_worktree.remove(&worktree.id).unwrap_or_default();
            swept += self.checkpoints.retain(&path, &live).await?;
        }
        Ok(swept)
    }

    /// The absolute path of the worktree one agent thread runs in.
    async fn agent_thread_worktree(&self, thread: ThreadId) -> DaemonResult<PathBuf> {
        let summary = self
            .agents
            .summaries()
            .await
            .into_iter()
            .find(|summary| summary.thread == thread)
            .ok_or_else(|| DaemonError::NotFound(format!("agent thread {thread}")))?;
        if summary.host.is_some() {
            return Err(remote_unsupported());
        }
        Ok(PathBuf::from(self.worktrees.path(summary.worktree).await?))
    }
}

/// Runs the orphan sweep until the daemon shuts down.
///
/// Ticks immediately, so a daemon that was killed mid-deletion cleans up at the next start
/// rather than an hour into it.
pub(crate) async fn run_sweep(services: Arc<Services>, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(SWEEP_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {
                if let Err(error) = services.sweep_agent_checkpoints().await {
                    tracing::warn!(
                        target: "fleet::agents",
                        %error,
                        "could not sweep orphaned fleet checkpoints"
                    );
                }
            }
        }
    }
}
