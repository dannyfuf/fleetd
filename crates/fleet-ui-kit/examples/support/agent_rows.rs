//! The sample thread the agent gallery draws.
//!
//! `docs/DESIGN-SYSTEM.md` §8.3 makes the gallery the acceptance test, which means this fixture
//! has to contain **every** row kind and every state of each — including the ones a realistic
//! transcript would never show together. It lives beside the gallery rather than inside it so
//! the panel code stays readable.

use std::time::Instant;

use fleet_ui_kit::prelude::*;

/// What the gallery's toggles currently say.
pub struct Thread {
    /// Whether every expandable row is open.
    pub expanded: bool,
    /// Whether the live tail streams (the caret and the shimmer).
    pub streaming: bool,
    /// Whether the transcript shows its empty state instead of the thread.
    pub empty: bool,
    /// When the working row's clock started.
    pub started_at: Instant,
}

/// Every row kind the transcript can draw, in the order a real turn emits them.
pub fn sample_thread(thread: Thread) -> Vec<TranscriptRow> {
    if thread.empty {
        return vec![TranscriptRow::ordinal(
            0,
            TranscriptRowKind::Empty(EmptyRow {
                message: "new claude thread · feat/payroll-fix · claude-opus-5".into(),
            }),
        )];
    }
    let expanded = thread.expanded;
    let mut rows = Vec::new();
    let mut push = |kind: TranscriptRowKind, id: Option<TranscriptRowId>| {
        let index = rows.len();
        rows.push(TranscriptRow::new(
            id.unwrap_or(TranscriptRowId::Ordinal(index)),
            kind,
        ));
    };

    push(
        TranscriptRowKind::Checkpoint(CheckpointRow {
            label: format_resumed(7_200_000),
        }),
        None,
    );
    push(
        TranscriptRowKind::User(UserRow {
            text: "fix the payroll rounding and add a test".into(),
            attachments: vec!["payroll.rs".into(), "spec.md".into()],
            state: UserRowState::Sent,
            steered: false,
            collapsible: true,
            expanded,
        }),
        None,
    );
    // A steer: it joined a turn that was already running, and it is still in flight.
    push(
        TranscriptRowKind::User(UserRow {
            state: UserRowState::Sending,
            steered: true,
            ..UserRow::new("also update the CHANGELOG")
        }),
        None,
    );
    push(
        TranscriptRowKind::User(UserRow {
            state: UserRowState::Failed,
            ..UserRow::new("and bump the version")
        }),
        None,
    );
    push(
        TranscriptRowKind::Reasoning(ReasoningRow {
            text: "the spec says half-up; the reducer truncates toward zero".into(),
            duration_ms: Some(12_000),
            expanded,
        }),
        None,
    );
    push(
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: parse_markdown_document(
                "Reading the reducer first, then the projection it feeds.",
            ),
            streaming: false,
            empty: false,
        }),
        None,
    );
    push(
        TranscriptRowKind::Work(
            ToolRow::new("t-read", "read", "crates/fleet-core/src/payroll.rs")
                .icon(Icon::Eye)
                .state(ToolRowState::Done)
                .result("120 lines")
                .body("pub fn round(cents: i64) -> i64 { cents / 100 }")
                .expanded(expanded),
        ),
        Some(TranscriptRowId::Item("t-read".into())),
    );
    push(
        TranscriptRowKind::WorkGroup(WorkGroupRow {
            summary: format_group_summary(&ToolGroupCounts {
                read: 3,
                commands: 2,
                ..ToolGroupCounts::default()
            })
            .unwrap_or_default(),
            icon: Icon::Wrench,
            hidden: 5,
            expanded,
            latest_failed: false,
        }),
        None,
    );
    push(
        TranscriptRowKind::Subagent(SubagentRow {
            summary: "spawned 3 subagents · explore, verify, write".into(),
            status: Some("2 working".into()),
            tokens: Some(format_token_count(24_100)),
            children: vec![
                "explore · reading crates/fleet-core/src/payroll.rs".into(),
                "verify · running cargo test -p fleet-core".into(),
                "write · idle".into(),
            ],
            expanded,
            live: true,
        }),
        Some(TranscriptRowId::Item("t-agent".into())),
    );
    push(
        TranscriptRowKind::Work(
            ToolRow::new("t-edit", "edit", "crates/fleet-core/src/payroll.rs")
                .icon(Icon::FilePen)
                .state(ToolRowState::Done)
                .result(format_file_delta(14, 3))
                .expanded(expanded),
        ),
        Some(TranscriptRowId::Item("t-edit".into())),
    );
    if expanded {
        push(
            TranscriptRowKind::Diff(DiffRow {
                item: "t-edit".into(),
                unified: "@@ -1,3 +1,3 @@\n-cents / 100\n+cents.div_euclid(100)".into(),
            }),
            Some(TranscriptRowId::Item("t-edit".into())),
        );
    }
    push(
        TranscriptRowKind::Work(
            ToolRow::new("t-bash", "bash", "cargo test -p fleet-core")
                .icon(Icon::Terminal)
                .state(ToolRowState::Failed)
                .result(format_exit(101))
                .body("error[E0308]: mismatched types")
                .expanded(expanded),
        ),
        Some(TranscriptRowId::Item("t-bash".into())),
    );
    push(
        TranscriptRowKind::Gate(GateRow {
            outcome: GateOutcome::Allowed,
            label: "allowed once".into(),
            detail: "Bash: git push --force origin main".into(),
            payload: Some("git push --force origin main".into()),
            expanded,
        }),
        Some(TranscriptRowId::Item("gate-record".into())),
    );
    push(
        TranscriptRowKind::Plan(PlanRow {
            title: "fix the payroll rounding".into(),
            markdown: parse_markdown_document(
                "1. read the current reducer\n2. add a failing test\n3. fix the projection",
            ),
            collapsible: true,
            expanded,
        }),
        Some(TranscriptRowId::Item("plan".into())),
    );
    push(
        TranscriptRowKind::Notice(NoticeRow {
            text: "stop hook error occurred · ctrl+o to see".into(),
        }),
        None,
    );
    push(
        TranscriptRowKind::Error(ErrorRow {
            message: "API error 529 — overloaded".into(),
            retryable: true,
        }),
        None,
    );
    push(
        TranscriptRowKind::TurnFold(TurnFoldRow {
            label: format_worked(Some(22_000), 14),
            expanded,
        }),
        Some(TranscriptRowId::Turn("turn-1".into())),
    );
    push(
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: parse_markdown_document(
                "Fixed the rounding in `round`, and the reducer test now covers the \
                 half-up case.\n\n```rust\nlet cents = cents.div_euclid(100);\n```",
            ),
            streaming: false,
            empty: false,
        }),
        None,
    );
    push(
        TranscriptRowKind::AssistantMeta(AssistantMetaRow {
            updated_at: "12:04".into(),
        }),
        None,
    );
    push(
        TranscriptRowKind::TurnFooter(TurnFooterRow {
            segments: turn_footer_segments(Some(12_400), Some(0.42), Some((2, 36, 3))),
            diff: true,
            revert: true,
        }),
        Some(TranscriptRowId::Turn("turn-1".into())),
    );
    push(
        TranscriptRowKind::Checkpoint(CheckpointRow {
            label: format_compacted(120_000, Some(30_000)),
        }),
        None,
    );
    // The live tail: the working row, then the row that changes its own label in place.
    push(
        TranscriptRowKind::Working(WorkingRow {
            phase: WorkingPhase::Working,
            started_at: Some(thread.started_at),
            harness: "claude".into(),
            detail: None,
        }),
        Some(TranscriptRowId::LiveActivity),
    );
    push(
        TranscriptRowKind::WorkLive(WorkLiveRow {
            label: "running cargo test -p fleet-core --all-features".into(),
            icon: Icon::Terminal,
            shimmer: thread.streaming,
        }),
        Some(TranscriptRowId::LiveActivity),
    );
    push(
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: parse_markdown_document("Re-running the suite to confir"),
            streaming: thread.streaming,
            empty: false,
        }),
        None,
    );
    rows
}
