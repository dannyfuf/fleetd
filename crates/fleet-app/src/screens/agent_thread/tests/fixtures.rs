//! Projection builders shared by every agent-tab test.
//!
//! One place so a test reads as the case it is pinning rather than as twenty lines of struct
//! literal, and so a new field on a core type breaks one file instead of forty tests.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, TimeZone, Utc};
use fleet_core::{
    agents::{
        AgentKind, FileDelta, GateId, GateKind, Item, ItemId, ItemKind, ItemStatus, OpenGate,
        PermissionChoice, PermissionOption, ProviderOptionId, Question, QuestionOption, Seq,
        ThreadId, ThreadProjection, ToolCall, ToolDiff, ToolKind, TurnEnd, TurnId, TurnOutcome,
        TurnRecord, Usage,
    },
    ids::WorktreeId,
};
use gpui::SharedString;

use crate::screens::agent_thread::rows::{PendingSend, ResolvedGate, RowInputs};

/// A fixed instant, so no test depends on the wall clock.
pub(super) fn at(offset: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + offset, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

pub(super) fn worktree() -> WorktreeId {
    "acme/payroll#feat-x"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

pub(super) fn projection() -> ThreadProjection {
    let mut projection = ThreadProjection::new(ThreadId::new(), worktree(), AgentKind::Claude);
    projection.session = fleet_core::agents::SessionState::Ready;
    projection
}

pub(super) fn item(turn: TurnId, kind: ItemKind, status: ItemStatus) -> Item {
    Item {
        id: ItemId::new(),
        turn,
        parent: None,
        kind,
        status,
        children: Vec::new(),
        started: at(0),
        ended: (status != ItemStatus::InProgress).then(|| at(6)),
    }
}

pub(super) fn user(turn: TurnId, text: &str) -> Item {
    item(
        turn,
        ItemKind::UserMessage {
            text: text.to_owned(),
            attachments: Vec::new(),
            steered: false,
        },
        ItemStatus::Completed,
    )
}

pub(super) fn assistant(turn: TurnId, text: &str, status: ItemStatus) -> Item {
    item(
        turn,
        ItemKind::AssistantText {
            text: text.to_owned(),
        },
        status,
    )
}

pub(super) fn reasoning(turn: TurnId, text: &str, status: ItemStatus) -> Item {
    item(
        turn,
        ItemKind::Reasoning {
            summary: [(0, text.to_owned())].into_iter().collect(),
            raw: std::collections::BTreeMap::new(),
        },
        status,
    )
}

pub(super) fn tool(turn: TurnId, kind: ToolKind, status: ItemStatus, summary: &str) -> Item {
    item(
        turn,
        ItemKind::Tool(Box::new(ToolCall {
            kind,
            name: "Bash".to_owned(),
            input: serde_json::json!({}),
            summary: Some(summary.to_owned()),
            result: None,
            output: String::new(),
            diff: None,
            exit_code: None,
            duration_ms: None,
            extra: std::collections::BTreeMap::new(),
        })),
        status,
    )
}

/// A tool item whose input names a command, for the expanded-body order.
pub(super) fn command(turn: TurnId, status: ItemStatus, line: &str) -> Item {
    let mut entry = tool(turn, ToolKind::Bash, status, line);
    if let ItemKind::Tool(call) = &mut entry.kind {
        call.input = serde_json::json!({ "command": line });
    }
    entry
}

/// An edit item carrying a unified diff.
pub(super) fn edit(turn: TurnId, status: ItemStatus, path: &str) -> Item {
    let mut entry = tool(turn, ToolKind::Edit, status, path);
    if let ItemKind::Tool(call) = &mut entry.kind {
        call.diff = Some(ToolDiff {
            path: path.into(),
            added: 3,
            removed: 1,
            unified: format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new\n"),
        });
        call.input = serde_json::json!({ "file_path": path });
    }
    entry
}

pub(super) fn plan(turn: TurnId, markdown: &str) -> Item {
    item(
        turn,
        ItemKind::Plan {
            text: markdown.to_owned(),
        },
        ItemStatus::Completed,
    )
}

pub(super) fn subagent(turn: TurnId, status: ItemStatus) -> Item {
    item(
        turn,
        ItemKind::Subagent {
            name: "explore".to_owned(),
            description: "read the payroll module".to_owned(),
            result: None,
        },
        status,
    )
}

pub(super) fn running_turn(id: TurnId, user_item: ItemId) -> TurnRecord {
    TurnRecord {
        id,
        user_item: Some(user_item),
        started_at: at(0),
        ended: None,
        blocked_ms: 0,
    }
}

pub(super) fn settled_turn(id: TurnId, user_item: ItemId, outcome: TurnOutcome) -> TurnRecord {
    TurnRecord {
        id,
        user_item: Some(user_item),
        started_at: at(0),
        blocked_ms: 0,
        ended: Some(TurnEnd {
            outcome,
            usage: Usage {
                total_tokens: 12_400,
                ..Usage::default()
            },
            duration_ms: 48_000,
            files_changed: vec![FileDelta {
                path: "src/lib.rs".into(),
                added: 36,
                removed: 3,
            }],
        }),
    }
}

pub(super) fn permission_gate(payload: &str, options: &[PermissionChoice]) -> OpenGate {
    OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "claude wants to run a command".to_owned(),
            payload: payload.to_owned(),
            rationale: None,
            options: options
                .iter()
                .map(|choice| PermissionOption {
                    id: ProviderOptionId(format!("{choice:?}")),
                    label: *choice,
                })
                .collect(),
        },
        opened_seq: Seq(4),
        blocked_since: None,
    }
}

pub(super) fn question(
    header: &str,
    prompt: &str,
    options: &[&str],
    multi_select: bool,
    allows_other: bool,
) -> Question {
    Question {
        id: prompt.to_owned(),
        header: header.to_owned(),
        prompt: prompt.to_owned(),
        options: options
            .iter()
            .map(|label| QuestionOption {
                id: ProviderOptionId((*label).to_owned()),
                label: (*label).to_owned(),
                description: String::new(),
            })
            .collect(),
        multi_select,
        allows_other,
        is_secret: false,
        blocking: true,
    }
}

pub(super) fn question_gate(questions: Vec<Question>) -> OpenGate {
    OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Question { questions },
        opened_seq: Seq(4),
        blocked_since: None,
    }
}

pub(super) fn plan_gate(markdown: &str) -> OpenGate {
    OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Plan {
            markdown: markdown.to_owned(),
            steps: Vec::new(),
        },
        opened_seq: Seq(4),
        blocked_since: None,
    }
}

/// Everything a row build needs beside the projection, with every local set empty.
#[derive(Default)]
pub(super) struct Locals {
    pub(super) expanded: HashSet<ItemId>,
    pub(super) unfolded: HashSet<TurnId>,
    pub(super) expanded_gates: HashSet<GateId>,
    pub(super) resolved: Vec<ResolvedGate>,
    pub(super) pending: Vec<PendingSend>,
    pub(super) checkpoints: HashMap<TurnId, fleet_proto::agents::CheckpointId>,
}

impl Locals {
    pub(super) fn expand(mut self, item: ItemId) -> Self {
        self.expanded.insert(item);
        self
    }

    pub(super) fn unfold(mut self, turn: TurnId) -> Self {
        self.unfolded.insert(turn);
        self
    }

    pub(super) fn checkpoint(mut self, turn: TurnId) -> Self {
        self.checkpoints.insert(
            turn,
            fleet_proto::agents::CheckpointId::from_parts(
                1,
                fleet_proto::agents::CheckpointScope::Turn,
                turn,
            ),
        );
        self
    }

    pub(super) fn resolve(mut self, resolved: ResolvedGate) -> Self {
        self.resolved.push(resolved);
        self
    }

    pub(super) fn pend(mut self, pending: PendingSend) -> Self {
        self.pending.push(pending);
        self
    }

    pub(super) fn inputs<'a>(&'a self, projection: &'a ThreadProjection) -> RowInputs<'a> {
        RowInputs {
            projection,
            expanded: &self.expanded,
            unfolded: &self.unfolded,
            expanded_gates: &self.expanded_gates,
            resolved: &self.resolved,
            pending: &self.pending,
            checkpoints: &self.checkpoints,
            started_at: None,
            parked: None,
            empty: SharedString::new_static("new claude thread"),
        }
    }
}

/// An optimistic bubble, with the echo count a fresh projection implies.
pub(super) fn pending(text: &str, steered: bool) -> PendingSend {
    PendingSend {
        id: ItemId::new(),
        text: text.to_owned(),
        steered,
        failed: false,
        echoes: 0,
    }
}
