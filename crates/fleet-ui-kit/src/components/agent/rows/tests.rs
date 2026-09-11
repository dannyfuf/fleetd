//! The row model's own rules, tested without a window: identity, expandability and the vertical
//! rhythm ladder are pure, and the row diff is the property the whole streaming path rests on.

use super::*;
use crate::components::MarkdownDocument;
use crate::icons::Icon;

fn user(text: &str) -> TranscriptRow {
    TranscriptRow::ordinal(0, TranscriptRowKind::User(UserRow::new(text)))
}

fn tool(id: &str, summary: &str) -> TranscriptRow {
    TranscriptRow::new(
        TranscriptRowId::Item(id.into()),
        TranscriptRowKind::Work(ToolRow::new(id, "bash", summary)),
    )
}

fn assistant(text: &str) -> TranscriptRow {
    TranscriptRow::ordinal(
        0,
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: crate::components::parse_markdown_document(text),
            streaming: true,
            empty: false,
        }),
    )
}

#[test]
fn a_row_id_survives_a_status_change() {
    let running = tool("t1", "cargo test");
    let TranscriptRowKind::Work(row) = running.kind.clone() else {
        unreachable!("constructed a work row")
    };
    let done = TranscriptRow::new(
        running.id.clone(),
        TranscriptRowKind::Work(
            row.state(super::super::tool_row::ToolRowState::Done)
                .result("exit 0"),
        ),
    );
    assert_eq!(running.id, done.id);
    // The row changed, so it splices — but only itself.
    assert_eq!(
        diff_rows(&[running], &[done]),
        Some(RowSplice {
            old_range: 0..1,
            count: 1
        })
    );
}

/// The live row, a streaming reasoning row and the working row share one id on purpose:
/// *thinking → tool A running → tool A done* is one row changing its label, not three mounts.
#[test]
fn the_live_rows_share_one_identity() {
    let thinking = TranscriptRow::new(
        TranscriptRowId::LiveActivity,
        TranscriptRowKind::Reasoning(ReasoningRow {
            text: SharedString::default(),
            duration_ms: None,
            expanded: false,
        }),
    );
    let running = TranscriptRow::new(
        TranscriptRowId::LiveActivity,
        TranscriptRowKind::WorkLive(WorkLiveRow {
            label: "running cargo".into(),
            icon: Icon::Terminal,
            shimmer: true,
        }),
    );
    assert_eq!(thinking.id, running.id);
    assert_eq!(thinking.id.key(), "live-activity");
}

#[test]
fn only_rows_with_something_underneath_expand() {
    assert!(!user("hi").is_expandable());
    let mut long = UserRow::new("a very long message");
    long.collapsible = true;
    assert!(TranscriptRow::ordinal(0, TranscriptRowKind::User(long)).is_expandable());

    assert!(!tool("t1", "cargo test").is_expandable());
    assert!(
        TranscriptRow::new(
            TranscriptRowId::Item("t1".into()),
            TranscriptRowKind::Work(ToolRow::new("t1", "bash", "cargo test").body("exit 0")),
        )
        .is_expandable()
    );

    assert!(
        TranscriptRow::new(
            TranscriptRowId::LiveActivity,
            TranscriptRowKind::Reasoning(ReasoningRow {
                text: "half-up, per the spec".into(),
                duration_ms: Some(6_000),
                expanded: false,
            })
        )
        .is_expandable()
    );

    // A notice has no body on either wire, so it is deliberately not foldable.
    assert!(
        !TranscriptRow::ordinal(
            0,
            TranscriptRowKind::Notice(NoticeRow {
                text: "stop hook error".into()
            })
        )
        .is_expandable()
    );
    // A gate record expands only when it kept its payload.
    let gate = |payload: Option<SharedString>| {
        TranscriptRow::new(
            TranscriptRowId::Item("g1".into()),
            TranscriptRowKind::Gate(GateRow {
                outcome: GateOutcome::Allowed,
                label: "allowed once".into(),
                detail: "Bash: git push".into(),
                payload,
                expanded: false,
            }),
        )
    };
    assert!(!gate(None).is_expandable());
    assert!(gate(Some("git push --force".into())).is_expandable());
}

/// `spec-B` §B1.3, in the order the table states it.
#[test]
fn the_rhythm_ladder_is_checked_in_order() {
    let detail = TranscriptRow::new(
        TranscriptRowId::Item("t1".into()),
        TranscriptRowKind::Diff(DiffRow {
            item: "t1".into(),
            unified: "@@".into(),
        }),
    );
    assert_eq!(detail.rhythm(), TranscriptRhythm::Detail);

    // A header with its details directly below leaves no gap, whatever kind it is.
    let attached = tool("t1", "cargo test").attached(true);
    assert_eq!(attached.rhythm(), TranscriptRhythm::Attached);

    let fold = TranscriptRow::new(
        TranscriptRowId::Turn("turn-1".into()),
        TranscriptRowKind::TurnFold(TurnFoldRow {
            label: "worked 22s".into(),
            expanded: false,
        }),
    );
    assert_eq!(fold.rhythm(), TranscriptRhythm::Fold);
    assert_eq!(tool("t1", "cargo").rhythm(), TranscriptRhythm::Work);
    assert_eq!(user("hi").rhythm(), TranscriptRhythm::Block);
    assert_eq!(
        TranscriptRow::ordinal(
            0,
            TranscriptRowKind::Checkpoint(CheckpointRow {
                label: "compacted".into()
            })
        )
        .rhythm(),
        TranscriptRhythm::Block
    );
}

#[test]
fn an_unchanged_projection_splices_nothing() {
    let rows = vec![user("a"), tool("t1", "cargo test")];
    assert_eq!(diff_rows(&rows, &rows), None);
}

#[test]
fn appending_splices_only_the_new_tail() {
    let old = vec![user("a")];
    let mut new = old.clone();
    new.push(tool("t1", "cargo"));
    assert_eq!(
        diff_rows(&old, &new),
        Some(RowSplice {
            old_range: 1..1,
            count: 1
        })
    );
}

/// The load-bearing property of the streaming path: a text delta splices one row and every
/// other row keeps the height the list measured for it.
#[test]
fn a_streaming_delta_splices_exactly_one_row() {
    let old = vec![user("a"), assistant("Reading the red"), tool("t1", "x")];
    let new = vec![
        user("a"),
        assistant("Reading the reducer first"),
        tool("t1", "x"),
    ];
    assert_eq!(
        diff_rows(&old, &new),
        Some(RowSplice {
            old_range: 1..2,
            count: 1
        })
    );
}

#[test]
fn removing_a_row_splices_only_that_index() {
    let old = vec![user("a"), tool("t1", "x"), user("c")];
    let new = vec![user("a"), user("c")];
    assert_eq!(
        diff_rows(&old, &new),
        Some(RowSplice {
            old_range: 1..2,
            count: 0
        })
    );
}

#[test]
fn an_ordinal_row_keys_on_its_position_and_an_item_row_on_its_id() {
    assert_eq!(TranscriptRowId::Ordinal(3).key(), "row-3");
    assert_eq!(TranscriptRowId::Item("item-9".into()).key(), "item-9");
    assert_eq!(TranscriptRowId::Turn("turn-2".into()).key(), "turn-2");
}

#[test]
fn a_default_document_is_an_empty_assistant_row() {
    let row = TranscriptRow::ordinal(
        0,
        TranscriptRowKind::Assistant(AssistantRow {
            markdown: MarkdownDocument::default(),
            streaming: false,
            empty: true,
        }),
    );
    assert!(!row.is_expandable());
    assert_eq!(row.rhythm(), TranscriptRhythm::Block);
}
