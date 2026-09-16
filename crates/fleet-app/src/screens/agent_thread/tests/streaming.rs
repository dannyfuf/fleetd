//! Mixed-batch regressions for incremental transcript updates.

use fleet_core::agents::{
    Applied, ItemKind, ItemStatus, RetryState, Seq, SessionState, StreamKind, TurnId, TurnOutcome,
    TurnState,
};
use fleet_ui_kit::TranscriptRowKind;
use gpui::{AppContext as _, TestAppContext};

use super::fixtures::{assistant, projection, running_turn, settled_turn, user};
use crate::screens::agent_thread::AgentThreadView;

#[gpui::test]
fn a_final_delta_is_remeasured_before_the_same_batch_settles(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        cx.set_reduce_motion(true);
    });
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let prose = assistant(turn, "wri", ItemStatus::InProgress);
    let item = prose.id;
    let mut base = projection();
    base.items = vec![prompt.clone(), prose];
    base.turns = vec![running_turn(turn, prompt.id)];
    base.turn = TurnState::Running(turn);
    base.session = SessionState::Running;
    base.last_seq = Seq(9);
    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));

    let mut next = base;
    next.items[1].kind = ItemKind::AssistantText {
        text: "writing it".to_owned(),
    };
    next.items[1].status = ItemStatus::Completed;
    next.items[1].ended = Some(super::fixtures::at(2));
    next.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    next.turn = TurnState::Settled(turn, TurnOutcome::Completed);
    next.session = SessionState::Ready;
    next.last_seq = Seq(11);

    view.update(cx, |view, cx| {
        view.sync_batch(
            &next,
            &[Applied::Text {
                item,
                stream: StreamKind::AssistantText,
                appended: 3..10,
            }],
            cx,
        );
    });

    view.read_with(cx, |view, _| {
        assert_eq!(view.patched_rows(), 1, "the final delta skipped patch_row");
        assert!(view.rows().iter().any(|row| {
            matches!(&row.kind, TranscriptRowKind::Assistant(assistant)
                if *assistant.markdown == fleet_ui_kit::parse_markdown_document("writing it"))
        }));
        assert!(
            view.rows()
                .iter()
                .any(|row| matches!(&row.kind, TranscriptRowKind::TurnFooter(_))),
            "the structural settlement still has to add its footer"
        );
    });
}

#[gpui::test]
fn a_text_delta_that_clears_retrying_rebuilds_the_trailing_rows(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        cx.set_reduce_motion(true);
    });
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let prose = assistant(turn, "a", ItemStatus::InProgress);
    let item = prose.id;
    let mut base = projection();
    base.items = vec![prompt.clone(), prose];
    base.turns = vec![running_turn(turn, prompt.id)];
    base.turn = TurnState::Running(turn);
    base.session = SessionState::Running;
    base.retrying = Some(RetryState {
        attempt: 2,
        retry_in_ms: 4_000,
        reason: "rate limited".to_owned(),
    });
    base.last_seq = Seq(9);
    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));

    let mut next = base;
    next.items[1].kind = ItemKind::AssistantText {
        text: "ab".to_owned(),
    };
    next.retrying = None;
    next.last_seq = Seq(10);
    view.update(cx, |view, cx| {
        view.sync(
            &next,
            &Applied::Text {
                item,
                stream: StreamKind::AssistantText,
                appended: 1..2,
            },
            cx,
        );
    });

    view.read_with(cx, |view, _| {
        assert_eq!(view.patched_rows(), 1);
        assert!(
            view.rows()
                .iter()
                .all(|row| !matches!(&row.kind, TranscriptRowKind::Notice(_))),
            "the cleared retrying notice stayed visible"
        );
    });
}
