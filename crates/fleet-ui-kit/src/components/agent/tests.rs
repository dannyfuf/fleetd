//! The render smoke pass: every row kind, every tool state and every drawer occupant drawn
//! through a real window.
//!
//! `gallery_agent` is the acceptance test for how these *look*, and it cannot run in CI. This
//! is the part a machine can check: gpui panics on a duplicate `ElementId` among siblings and
//! on a malformed highlight range, so drawing every state once is what keeps the gallery
//! honest between visual passes.

use gpui::{TestAppContext, VisualTestContext, div, prelude::*};
use std::rc::Rc;

use super::rows::{
    DelegationResultCard, DelegationRow, DelegationRowStatus, RowContext, row_element,
};
use super::{
    ApprovalRequest, AssistantMetaRow, AssistantRow, CheckpointRow, Decision, DecisionDock,
    DecisionKind, DecisionQuestion, DiffRow, EmptyRow, ErrorRow, GateOutcome, GateRow, MetadataFit,
    MetadataRow, MetadataSegment, NoticeRow, PlanRow, QuestionOption, QuestionSet, ReasoningRow,
    RowActions, SubagentRow, ToolRow, ToolRowElement, ToolRowState, TranscriptList, TranscriptRow,
    TranscriptRowId, TranscriptRowKind, TurnFoldRow, TurnFooterRow, UserRow, UserRowState,
    WorkGroupRow, WorkLiveRow, WorkingPhase, WorkingRow, turn_footer_segments,
};
use crate::{
    components::parse_markdown_document,
    icons::Icon,
    theme::{ActiveTheme, Theme, ch},
};

fn delegation_rows() -> Vec<TranscriptRow> {
    let mut rows = [
        DelegationRowStatus::Starting,
        DelegationRowStatus::Working,
        DelegationRowStatus::Blocked,
        DelegationRowStatus::Done,
        DelegationRowStatus::Incomplete,
        DelegationRowStatus::Failed,
        DelegationRowStatus::Cancelled,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, status)| {
        TranscriptRow::new(
            TranscriptRowId::Item(format!("delegation-{index}").into()),
            TranscriptRowKind::Delegation(DelegationRow {
                provider_glyph: Icon::Sparkles,
                title: "verify the payroll reducer".into(),
                status,
                headline: Some("the regression test still fails on Windows".into()),
                elapsed: "14m 02s".into(),
                hint: "⏎ attach".into(),
            }),
        )
    })
    .collect::<Vec<_>>();
    let body = Rc::new(parse_markdown_document(
        "Checked the reducer and added coverage.\n\n- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight\n- nine",
    ));
    for expanded in [false, true] {
        rows.push(TranscriptRow::new(
            TranscriptRowId::Item(format!("delegation-result-{expanded}").into()),
            TranscriptRowKind::DelegationResult(DelegationResultCard {
                header: "codex finished · done · 14m 02s · 6 files".into(),
                body: body.clone(),
                collapsible: true,
                expanded,
                hint: (if expanded { "⏎ hide" } else { "⏎ show" }).into(),
            }),
        ));
    }
    rows
}

#[test]
fn delegation_status_words_and_liveness_are_closed() {
    let states = [
        (DelegationRowStatus::Starting, "starting", true),
        (DelegationRowStatus::Working, "working", true),
        (DelegationRowStatus::Blocked, "blocked", true),
        (DelegationRowStatus::Done, "done", false),
        (DelegationRowStatus::Incomplete, "incomplete", false),
        (DelegationRowStatus::Failed, "failed", false),
        (DelegationRowStatus::Cancelled, "cancelled", false),
    ];
    for (status, word, live) in states {
        assert_eq!(status.word(), word);
        assert_eq!(status.is_live(), live);
    }
}

#[test]
fn delegation_rows_use_work_rhythm_and_only_results_expand() {
    let rows = delegation_rows();
    let result_hints = rows
        .iter()
        .filter_map(|row| match &row.kind {
            TranscriptRowKind::DelegationResult(result) => Some(result.hint.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(result_hints, ["⏎ show", "⏎ hide"]);
    for row in &rows {
        assert_eq!(row.rhythm(), super::TranscriptRhythm::Work);
        assert_eq!(
            row.is_expandable(),
            matches!(&row.kind, TranscriptRowKind::DelegationResult(_))
        );
    }
}

/// One row of every kind, with the sub-states that change what is drawn.
fn every_row_kind() -> Vec<TranscriptRow> {
    let document = parse_markdown_document("A paragraph, and `code`.\n\n```rs\nlet x = 1;\n```");
    let mut kinds = vec![
        TranscriptRowKind::User(UserRow {
            attachments: vec!["payroll.rs".into()].into(),
            collapsible: true,
            expanded: true,
            ..UserRow::new("fix the rounding")
        }),
        TranscriptRowKind::User(UserRow {
            state: UserRowState::Sending,
            steered: true,
            ..UserRow::new("also the changelog")
        }),
        TranscriptRowKind::User(UserRow {
            state: UserRowState::Failed,
            ..UserRow::new("and the version")
        }),
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: Rc::new(document.clone()),
            streaming: true,
            empty: false,
        }),
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: Rc::new(document.clone()),
            streaming: false,
            empty: true,
        }),
        TranscriptRowKind::AssistantMeta(AssistantMetaRow {
            updated_at: "12:04".into(),
        }),
        TranscriptRowKind::Reasoning(ReasoningRow {
            text: "half-up".into(),
            duration_ms: Some(12_000),
            expanded: true,
        }),
        TranscriptRowKind::Reasoning(ReasoningRow {
            text: String::new().into(),
            duration_ms: None,
            expanded: false,
        }),
        TranscriptRowKind::WorkLive(WorkLiveRow {
            label: "running cargo".into(),
            icon: Icon::Terminal,
            shimmer: true,
        }),
        TranscriptRowKind::WorkGroup(WorkGroupRow {
            summary: "read 3 files and ran 2 commands".into(),
            icon: Icon::Wrench,
            hidden: 5,
            expanded: false,
            latest_failed: true,
        }),
        TranscriptRowKind::Subagent(SubagentRow {
            summary: "spawned 3 subagents".into(),
            status: Some("2 working".into()),
            tokens: Some("24.1k".into()),
            children: vec!["explore · reading".into()].into(),
            expanded: true,
            live: true,
        }),
        TranscriptRowKind::Diff(DiffRow {
            item: "t1".into(),
            unified: "@@ -1 +1 @@".into(),
        }),
        TranscriptRowKind::TurnFold(TurnFoldRow {
            label: "worked 22s · 14 steps".into(),
            expanded: true,
        }),
        TranscriptRowKind::TurnFooter(TurnFooterRow {
            segments: turn_footer_segments(Some(12_400), Some(0.42), Some((2, 36, 3))).into(),
            diff: true,
            revert: true,
        }),
        TranscriptRowKind::Plan(PlanRow {
            title: "fix the rounding".into(),
            markdown: Rc::new(document),
            collapsible: true,
            expanded: false,
        }),
        TranscriptRowKind::Checkpoint(CheckpointRow {
            label: "compacted 120k → 30k".into(),
        }),
        TranscriptRowKind::Notice(NoticeRow {
            text: "a config warning".into(),
        }),
        TranscriptRowKind::Error(ErrorRow {
            message: "API error 529".into(),
            retryable: true,
        }),
        TranscriptRowKind::Empty(EmptyRow {
            message: "new claude thread".into(),
        }),
    ];
    kinds.extend(delegation_rows().into_iter().map(|row| row.kind));
    for state in [
        ToolRowState::Running,
        ToolRowState::Done,
        ToolRowState::Failed,
        ToolRowState::Denied,
        ToolRowState::Stopped,
        ToolRowState::Severe,
    ] {
        kinds.push(TranscriptRowKind::Work(
            ToolRow::new("t", "bash", "cargo test")
                .icon(Icon::Terminal)
                .state(state)
                .result("exit 0")
                .body("output")
                .expanded(true),
        ));
    }
    for outcome in [
        GateOutcome::Allowed,
        GateOutcome::Declined,
        GateOutcome::Answered,
        GateOutcome::Withdrawn,
    ] {
        kinds.push(TranscriptRowKind::Gate(GateRow {
            outcome,
            label: "allowed once".into(),
            detail: "Bash: git push".into(),
            payload: Some("git push".into()),
            expanded: true,
        }));
    }
    for phase in [
        WorkingPhase::Working,
        WorkingPhase::Starting,
        WorkingPhase::Compacting,
        WorkingPhase::Resuming,
        WorkingPhase::Parked,
    ] {
        kinds.push(TranscriptRowKind::Working(WorkingRow {
            phase,
            started_at: None,
            harness: "claude".into(),
            detail: Some("weekly limit resets in ~3h".into()),
        }));
    }
    kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| TranscriptRow::new(TranscriptRowId::Ordinal(index), kind))
        .collect()
}

/// Drawing every row kind must not panic, and every row must be reachable: gpui rejects a
/// duplicate `ElementId` among siblings, which is exactly what a hand-built id would produce.
#[gpui::test]
fn every_row_kind_draws(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let rows = every_row_kind();
    let count = rows.len();
    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut list = TranscriptList::new(cx);
                    list.set_rows(rows, cx);
                    // An owner-supplied body reaches the rows that have a slot for one.
                    list.set_row_body(|_row, _cx| Some(div().into_any_element()), cx);
                    list
                })
            })
        })
        .expect("test window");
    let list = window.root(cx).expect("transcript");
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    list.read_with(visual, |list, _| assert_eq!(list.rows().len(), count));

    // Every row focused in turn: the focus ring and the row-focus keys must not panic either.
    for index in 0..count {
        visual.update(|_window, cx| {
            list.update(cx, |list, cx| {
                list.focus_row(Some(index), cx);
                let _ = list.event_for_key("enter");
            });
        });
        visual.run_until_parked();
    }
}

/// Every drawer occupant, every tool state and the metadata strip's collapse ladder, drawn.
#[gpui::test]
fn every_component_state_draws(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::dark()));
    let approval = |allows_edit| {
        DecisionKind::Approval(
            ApprovalRequest::new("Bash", "git push --force")
                .rationale("the branch diverged")
                .caution("this came from fetched content")
                .allows_edit(allows_edit),
        )
    };
    let question = DecisionKind::Question(
        QuestionSet::new(vec![
            DecisionQuestion::new("scope", "which?")
                .options(vec![
                    QuestionOption::new("a").description("the first"),
                    QuestionOption::new("b"),
                ])
                .multi_select(true)
                .allow_other(true),
            DecisionQuestion::new("tests", "add one?").options(vec![QuestionOption::new("yes")]),
        ])
        .cursor(1)
        .selected(vec![vec![0], vec![]]),
    );
    let plan = DecisionKind::PlanReady {
        title: "fix the rounding".into(),
        markdown: Some(parse_markdown_document("1. read\n2. fix")),
    };
    let decisions = vec![
        Decision::new("g1", "claude wants to run a command", approval(true)).queued(0, 3),
        Decision::new("g2", "codex wants to change a file", approval(false)),
        Decision::new("g3", "claude has questions", question).answering(true),
        Decision::new("g4", "plan ready", plan),
    ];
    let segments = vec![
        MetadataSegment::pinned("claude-opus-5"),
        MetadataSegment::new("high"),
        MetadataSegment::new("supervised"),
        MetadataSegment::new("build"),
    ];

    let window = cx
        .update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Panels {
                    decisions,
                    segments,
                    fit: MetadataFit::new(),
                    delegation_rows: delegation_rows(),
                })
            })
        })
        .expect("test window");
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
}

/// A view that draws every non-list component state at once.
struct Panels {
    decisions: Vec<Decision>,
    segments: Vec<MetadataSegment>,
    fit: MetadataFit,
    delegation_rows: Vec<TranscriptRow>,
}

impl gpui::Render for Panels {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tools = [
            ToolRowState::Running,
            ToolRowState::Done,
            ToolRowState::Failed,
            ToolRowState::Denied,
            ToolRowState::Stopped,
            ToolRowState::Severe,
        ]
        .into_iter()
        .enumerate()
        .map(|(key, state)| {
            ToolRowElement::new(
                ToolRow::new("t", "Run", "cargo test")
                    .icon(Icon::Terminal)
                    .state(state)
                    .result("exit 1")
                    .detail("4.1s")
                    .actions(RowActions {
                        copy: true,
                        diff: key % 2 == 0,
                        open: key % 3 == 0,
                        revert: false,
                    }),
                key,
            )
            .focused(key == 0)
            .on_action(|_, _, _| {})
            .harness_part("agents.tool")
            .into_any_element()
        });
        // The same segments at four widths: the memo is per width, and the pinned block never
        // collapses.
        let strips = [
            gpui::px(520.0),
            gpui::px(300.0),
            gpui::px(200.0),
            gpui::px(120.0),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, available)| {
            let mut segments = self.segments.clone();
            if index == 0 {
                segments[0] = segments[0].clone().target("caller-thread");
            }
            let fit = self
                .fit
                .fit(available, index as u32, &segments, theme.space.md, ch(3.0));
            div()
                .w(available)
                .child(
                    MetadataRow::new(segments, fit)
                        .trailing(vec![MetadataSegment::new("34%")])
                        .on_target(|_, _, _| {}),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();
        let delegation_rows = self
            .delegation_rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                row_element(
                    row,
                    RowContext {
                        index,
                        focused: false,
                        visible: true,
                        working_label: None,
                        body: None,
                        toggle: None,
                        action: Some(Rc::new(|_, _, _, _| {})),
                        action_kbd: None,
                    },
                    window,
                    cx,
                )
            })
            .collect::<Vec<_>>();

        div()
            .size_full()
            .flex()
            .flex_col()
            .children(tools)
            .children(delegation_rows)
            .children(strips)
            .children(self.decisions.iter().map(|decision| {
                DecisionDock::new(decision.clone())
                    .on_action(|_, _, _| {})
                    .kbd_for(|_, _, _| crate::components::Kbd::parse("y").ok())
            }))
    }
}
