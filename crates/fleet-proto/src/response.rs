//! Daemon-to-client request responses.

use fleet_core::{
    agents::{AgentThreadSummary, ItemId, SeqEvent, StreamKind, ThreadId, ThreadProjection},
    board::{BackendDescriptor, BackendSchema, BoardSummary, BoardView, Card},
    cache::RepoCache,
    config::Config,
    github::{PrTab, PullRequest},
    ids::{HostId, JobId, WorktreeId},
    inspection::WorktreeInspection,
    model::{Context, Repo, Worktree},
    sessions::{Session, Terminal, WorktreeStatus},
};
use serde::{Deserialize, Serialize};

use crate::{
    agents::{AgentRevertReport, AgentThreadWindow, TurnCheckpoint},
    error::ProtoError,
    job::JobRecord,
    snapshot::Snapshot,
};

/// A correlated daemon response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Correlation identifier copied from the request.
    pub id: u64,
    /// Successful response payload or stable protocol error.
    pub result: Result<ResponseBody, ProtoError>,
}

/// Capability name for committing only the worktree IDs reviewed by a prune dry run.
pub const PRUNE_REVIEWED_IDS_CAPABILITY: &str = "prune.reviewed_ids";

/// Additive metadata carried by the mandatory Hello response envelope.
///
/// Keeping this outside [`ResponseBody::Hello`] lets older IPC-v4 clients ignore it while newer
/// clients can distinguish daemons that honor exact reviewed prune IDs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResponse {
    /// The ordinary correlated Hello response.
    #[serde(flatten)]
    pub response: Response,
    /// Optional behaviors implemented by this daemon build.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Stable identity persisted by the daemon under its Fleet home.
    #[serde(default)]
    pub daemon_id: String,
    /// Source revision baked into the daemon build, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_commit: Option<String>,
}

/// Stable identity of one running daemon process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonIdentity {
    /// Operating-system process identifier.
    pub pid: u32,
    /// Per-process random identity, distinct even when the OS reuses a PID.
    pub boot_id: String,
}

/// Additive metadata carried only by a Pong response envelope.
///
/// An older IPC-v4 client deserializes this as an ordinary [`Response`] and ignores `daemon`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PongResponse {
    /// The ordinary correlated Pong response.
    #[serde(flatten)]
    pub response: Response,
    /// Daemon identity when the peer supports identity-bearing pings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonIdentity>,
}

/// Result of one worktree in a multi-delete request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDeleteResult {
    /// Requested worktree.
    pub worktree_id: WorktreeId,
    /// Whether it was deleted.
    pub ok: bool,
    /// Failure reason when deletion failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Recoverable trash entry name for undo after a successful deletion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trash_entry: Option<String>,
}

/// A worktree skipped by safe prune.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneSkipped {
    /// Skipped worktree.
    pub worktree_id: WorktreeId,
    /// Safety reason.
    pub reason: String,
    /// Derived merged state.
    pub merged: bool,
    /// Whether uncommitted changes exist.
    pub dirty: bool,
    /// Unique commits when calculable.
    pub unique_commits: Option<u64>,
    /// Keep-alive labels.
    pub running: Vec<String>,
}

/// Complete safe-prune outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneResult {
    /// Whether no deletions were performed.
    pub dry_run: bool,
    /// Deleted or would-delete worktree identifiers.
    pub deleted: Vec<WorktreeId>,
    /// Worktrees rejected by safety checks or failed deletion.
    pub skipped: Vec<PruneSkipped>,
}

/// One terminal kept while sleeping a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepKept {
    /// Terminal name.
    pub window: String,
    /// Comma-separated preservation reason.
    pub reason: String,
}

/// Session sleep outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepResult {
    /// Terminals preserved by policy.
    pub kept: Vec<SleepKept>,
    /// Closed terminal names.
    pub closed: Vec<String>,
    /// Whether no terminal remained and the session was removed.
    pub session_killed: bool,
}

/// Severity of one environment diagnostic result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    /// Check passed.
    Ok,
    /// Check is usable but deserves attention.
    Warn,
    /// Check failed.
    Fail,
}

/// One environment diagnostic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    /// Stable check name.
    pub check: String,
    /// Three-state diagnostic outcome.
    pub status: DoctorStatus,
    /// Human-readable observed detail.
    pub detail: String,
}

/// Pull requests and fetch state for one authored/review tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSlice {
    /// Tab represented by this slice.
    pub tab: PrTab,
    /// ISO-8601 time of the most recent successful fetch.
    pub fetched_at: String,
    /// Whether a replacement fetch is currently running.
    pub loading: bool,
    /// Most recent refresh error, while stale data remains usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Total result count before the client-visible cap.
    pub total: usize,
    /// Client-visible pull requests, capped by the service.
    pub prs: Vec<PullRequest>,
}

/// Candidate base refs and their refresh state for the Create dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseRefs {
    /// Ordered `origin/*` ref candidates.
    pub refs: Vec<String>,
    /// Whether a fetch is currently updating the candidates.
    pub fetching: bool,
    /// ISO-8601 time at which candidates were last refreshed.
    pub fetched_at: String,
}

/// Current process-match count or validation error for one keep-alive rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAliveRuleMatch {
    /// Stable keep-alive rule identifier.
    pub rule_id: String,
    /// Number of live processes matching the rule.
    pub count: u64,
    /// Pattern or process-observation error for a skipped rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Every successful daemon result payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // BOARD.md requires inline Card and CardWorktree payloads.
pub enum ResponseBody {
    /// Current native-agent thread summaries.
    AgentThreads(Vec<AgentThreadSummary>),
    /// Summary of a newly allocated native-agent thread.
    AgentThreadCreated(AgentThreadSummary),
    /// Materialized thread state and the ordered persisted tail after it.
    ///
    /// The version-6 unbounded answer to `AgentThreadOpen`, retained byte for byte for peers
    /// without [`AGENT_WINDOW_CAPABILITY`](crate::AGENT_WINDOW_CAPABILITY). `projection` never
    /// changes meaning: narrowing it from "the whole thread" to "a window" would repurpose a
    /// field (`rust-ipc-protocol` Rule 7), so the bounded answer is
    /// [`ResponseBody::AgentThreadWindow`] instead. Above
    /// [`SNAPSHOT_MAX_WIRE_BYTES`](crate::agents::SNAPSHOT_MAX_WIRE_BYTES) the daemon refuses
    /// this shape with [`snapshot_ceiling_error`](crate::agents::snapshot_ceiling_error) rather
    /// than emitting a frame the peer cannot decode.
    AgentThreadSnapshot {
        /// Materialized reducer projection.
        projection: ThreadProjection,
        /// Events after the snapshot or requested cursor.
        events_after: Vec<SeqEvent>,
    },
    /// A bounded window of a thread's transcript, with the page metadata to walk backwards.
    ///
    /// Answers an `AgentThreadOpen` for which
    /// [`RequestBody::wants_window`](crate::request::RequestBody::wants_window) holds. Boxed
    /// because most responses on this wire are an ack: an inline window would make every
    /// `ResponseBody` as large as the largest transcript slice.
    AgentThreadWindow(Box<AgentThreadWindow>),
    /// A slice of a stored item body, for an output a window elided.
    AgentItemBodyChunk {
        /// Owning thread.
        thread: ThreadId,
        /// Item whose body this slices.
        item: ItemId,
        /// Stream the slice came from.
        stream: StreamKind,
        /// Byte offset this slice starts at.
        offset: u64,
        /// Total stored bytes in that stream, so the client knows when it is done.
        total: u64,
        /// The slice itself.
        text: String,
    },
    /// A native-agent mutation was accepted.
    AgentAck,
    /// The Fleet-owned checkpoints one thread's worktree can be reverted to, oldest first.
    AgentCheckpoints(Vec<TurnCheckpoint>),
    /// What a revert put back, and what it removed.
    AgentReverted(AgentRevertReport),
    /// Board summaries for a context or all contexts.
    Boards(Vec<BoardSummary>),
    /// Full board document view.
    Board(BoardView),
    /// Created or changed card.
    Card(Card),
    /// Card and the worktree created from it.
    CardWorktree {
        /// Updated linked card.
        card: Card,
        /// Created or adopted worktree.
        worktree: Worktree,
        /// Whether this request created the worktree rather than reusing an existing one.
        #[serde(default)]
        created: bool,
    },
    /// Backend schema for a board.
    BoardBackendSchema(BackendSchema),
    /// Registered board backend kinds and their generic settings schemas.
    BoardBackends(Vec<BackendDescriptor>),
    /// Registered watch identifier.
    WatchStarted(fleet_core::watches::WatchId),
    /// Session's current and recently completed watches.
    Watches(Vec<fleet_core::watches::Watch>),
    /// Atomic output catch-up result.
    WatchTail(crate::watch::WatchTail),
    /// Successful protocol negotiation.
    Hello {
        /// Negotiated protocol version.
        protocol: u32,
        /// Daemon build version.
        server: String,
    },
    /// Complete daemon snapshot.
    Snapshot(Snapshot),
    /// Operation succeeded without a richer payload.
    Ack,
    /// Created or updated context.
    Context(Context),
    /// Registered repository.
    Repo(Repo),
    /// Repository clone was accepted as a background job.
    CloneStarted(JobRecord),
    /// GitHub repository discovery results.
    RemoteRepos(RepoCache),
    /// Candidate base refs for worktree creation.
    BaseRefs(BaseRefs),
    /// Created or idempotently returned worktree.
    Worktree {
        /// Whether this request created the worktree.
        created: bool,
        /// Resulting worktree.
        worktree: Worktree,
        /// Post-create hook job, when hooks were scheduled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_create_job: Option<Box<JobRecord>>,
    },
    /// Multi-worktree deletion results.
    WorktreesDeleted(#[serde(default)] Vec<WorktreeDeleteResult>),
    /// Worktree inspection results.
    Inspections(#[serde(default)] Vec<WorktreeInspection>),
    /// Safe-prune outcome.
    Pruned(PruneResult),
    /// Session sleep outcome.
    Slept(SleepResult),
    /// Resolved local or remote worktree path.
    Path {
        /// Path as interpreted by the owning daemon.
        path: String,
        /// Owning host; absent means local.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<HostId>,
    },
    /// Pull-request query results.
    PullRequests(Vec<PrSlice>),
    /// Refreshed runtime worktree statuses.
    Statuses(Vec<WorktreeStatus>),
    /// Created or changed session.
    Session(Session),
    /// Current daemon-owned sessions.
    Sessions(Vec<Session>),
    /// Currently active worktree session, when any.
    CurrentSession(Option<fleet_core::ids::SessionId>),
    /// Created or changed terminal.
    Terminal(Terminal),
    /// Current and recently completed jobs.
    Jobs(Vec<JobRecord>),
    /// One accepted or changed job.
    Job(JobRecord),
    /// A job was cancelled.
    JobCancelled(JobId),
    /// Trailing job log lines.
    JobLog(Vec<String>),
    /// Effective merged configuration.
    Config(Config),
    /// Live match counts for configured sleep rules.
    KeepAliveRuleMatches(Vec<KeepAliveRuleMatch>),
    /// Environment diagnostic results.
    Doctor(Vec<DoctorCheck>),
    /// Daemon liveness response.
    Pong,
    /// Daemon version information.
    Version {
        /// Daemon build version.
        version: String,
        /// Supported wire protocol version.
        protocol: u32,
    },
    /// Daemon accepted an explicit shutdown request.
    ShuttingDown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PROTOCOL_VERSION, assert_round_trip, error::ErrorKind};

    #[test]
    fn successful_and_failed_results_round_trip() {
        assert_round_trip(HelloResponse {
            response: Response {
                id: 8,
                result: Ok(ResponseBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    server: "fleetd test".to_owned(),
                }),
            },
            capabilities: vec![crate::REMOTE_MACHINES_CAPABILITY.to_owned()],
            daemon_id: "daemon-test".to_owned(),
            build_commit: Some("abc123".to_owned()),
        });
        assert_round_trip(Response {
            id: 9,
            result: Ok(ResponseBody::Version {
                version: "fleet 0.1.0".to_owned(),
                protocol: PROTOCOL_VERSION,
            }),
        });
        assert_round_trip(Response {
            id: 10,
            result: Err(ProtoError {
                kind: ErrorKind::Validation,
                message: "bad input".to_owned(),
            }),
        });
    }

    #[test]
    fn payload_carrying_bodies_round_trip() {
        let bodies = vec![
            ResponseBody::RemoteRepos(RepoCache {
                fetched_at: "2026-09-04T12:00:00Z".to_owned(),
                repos: Vec::new(),
            }),
            ResponseBody::BaseRefs(BaseRefs {
                refs: vec!["origin/main".to_owned()],
                fetching: false,
                fetched_at: "2026-09-04T12:00:00Z".to_owned(),
            }),
            ResponseBody::PullRequests(vec![PrSlice {
                tab: PrTab::Review,
                fetched_at: "2026-09-04T12:00:00Z".to_owned(),
                loading: true,
                error: Some("temporary failure".to_owned()),
                total: 101,
                prs: Vec::new(),
            }]),
            ResponseBody::Statuses(Vec::new()),
            ResponseBody::KeepAliveRuleMatches(vec![KeepAliveRuleMatch {
                rule_id: "claude".to_owned(),
                count: 2,
                error: None,
            }]),
        ];
        for (id, body) in bodies.into_iter().enumerate() {
            assert_round_trip(Response {
                id: id as u64,
                result: Ok(body),
            });
        }
    }

    #[test]
    fn agent_response_bodies_round_trip() {
        use fleet_core::{
            agents::{AgentKind, PermissionMode, ThreadId, ThreadProjection},
            ids::WorktreeId,
        };

        let projection = ThreadProjection::new(
            ThreadId::new(),
            WorktreeId::try_from("acme/api#native-agents")
                .unwrap_or_else(|error| panic!("{error}")),
            AgentKind::Codex,
        );
        let summary = projection.summary(Default::default());
        let turn = fleet_core::agents::TurnId::new();
        let checkpoint =
            crate::agents::CheckpointId::from_parts(4, crate::agents::CheckpointScope::File, turn);
        for body in [
            ResponseBody::AgentThreads(vec![summary.clone()]),
            ResponseBody::AgentThreadCreated(summary.clone()),
            ResponseBody::AgentThreadSnapshot {
                projection,
                events_after: Vec::new(),
            },
            ResponseBody::AgentThreadWindow(Box::new(AgentThreadWindow {
                summary,
                session: crate::agents::AgentSessionView {
                    tools: vec!["Read".to_owned()],
                    ..crate::agents::AgentSessionView::default()
                },
                window: crate::agents::TranscriptWindow::default(),
                page: Some(crate::agents::TranscriptPage {
                    before_cursor: Some("fat.1.0000".to_owned()),
                    has_more: true,
                    thread_seq: fleet_core::agents::Seq(12),
                }),
                head_seq: fleet_core::agents::Seq(12),
                projected_seq: fleet_core::agents::Seq(12),
                events_after: Vec::new(),
                synchronized: true,
            })),
            ResponseBody::AgentItemBodyChunk {
                thread: ThreadId::new(),
                item: ItemId::new(),
                stream: StreamKind::CommandOutput,
                offset: 262_144,
                total: 1_048_576,
                text: "…".to_owned(),
            },
            ResponseBody::AgentAck,
            ResponseBody::AgentCheckpoints(vec![crate::agents::TurnCheckpoint {
                id: checkpoint.clone(),
                scope: crate::agents::CheckpointScope::File,
                turn,
                ordinal: 4,
                at: chrono::Utc::now(),
            }]),
            ResponseBody::AgentReverted(crate::agents::AgentRevertReport {
                thread: ThreadId::new(),
                checkpoint,
                restored: 1,
                deleted: 0,
                paths: vec!["src/lib.rs".to_owned()],
            }),
        ] {
            assert_round_trip(body);
        }
        assert_round_trip(PermissionMode::Ask);
    }

    #[test]
    fn a_window_response_is_measured_against_the_two_mebibyte_budget() {
        use fleet_core::{
            agents::{AgentKind, Item, ItemKind, Seq, ThreadProjection, TurnId},
            ids::WorktreeId,
        };

        let thread = ThreadId::new();
        let worktree = WorktreeId::try_from("acme/api#native-agents")
            .unwrap_or_else(|error| panic!("{error}"));
        let projection = ThreadProjection::new(thread, worktree, AgentKind::Codex);
        let turn = TurnId::new();
        // One item per 4 KiB of output; 2 MiB of tool output is what a `cargo build` transcript
        // looks like, and it is the shape that made the unbounded snapshot undecodable.
        // Built from the wire shape rather than a struct literal: `fleet-proto` deliberately
        // does not depend on `chrono`, and a fixture that decodes is a fixture that proves the
        // field names too.
        let template: Item = serde_json::from_value(serde_json::json!({
            "id": ItemId::new(),
            "turn": turn,
            "kind": {"type": "assistant_text", "data": {"text": ""}},
            "status": "completed",
            "started": "2026-09-07T12:00:00Z",
        }))
        .unwrap_or_else(|error| panic!("{error}"));
        let heavy = crate::agents::TranscriptWindow {
            items: (0..600)
                .map(|index| Item {
                    id: ItemId::new(),
                    kind: ItemKind::AssistantText {
                        text: format!("{index}").repeat(2_048),
                    },
                    ..template.clone()
                })
                .collect(),
            ..crate::agents::TranscriptWindow::default()
        };
        let bytes = heavy.wire_bytes().unwrap_or_else(|error| panic!("{error}"));

        assert!(
            bytes > crate::agents::WINDOW_MAX_WIRE_BYTES,
            "the fixture has to be heavier than the budget to prove the budget bites: {bytes}"
        );
        assert!(
            !heavy
                .fits_wire_budget()
                .unwrap_or_else(|error| panic!("{error}")),
        );

        // The same window narrowed to what a first paint needs fits with room to spare, which is
        // the property the daemon's `turn_limit` clamp has to preserve.
        let narrowed = crate::agents::TranscriptWindow {
            items: heavy.items.into_iter().take(10).collect(),
            ..crate::agents::TranscriptWindow::default()
        };
        assert!(
            narrowed
                .fits_wire_budget()
                .unwrap_or_else(|error| panic!("{error}"))
        );

        let window = AgentThreadWindow {
            summary: projection.summary(Default::default()),
            session: crate::agents::AgentSessionView::default(),
            window: narrowed,
            page: None,
            head_seq: Seq(600),
            projected_seq: Seq(600),
            events_after: Vec::new(),
            synchronized: true,
        };
        assert!(
            window
                .fits_wire_budget()
                .unwrap_or_else(|error| panic!("{error}")),
            "a window response is measured whole, envelope included"
        );
    }
}
