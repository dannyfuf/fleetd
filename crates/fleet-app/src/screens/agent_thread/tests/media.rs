//! Composer attachment wiring: kit media event → registry upload → native-agent send.

use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Instant};

use fleet_core::agents::AttachmentSource;
use fleet_proto::{
    request::{RequestBody, StageOp},
    response::ResponseBody,
};
use fleet_ui_kit::{
    MultilineInputEvent, PendingAttachmentState, TextInputMedia, TranscriptRowKind,
};
use gpui::{
    AppContext as _, ClipboardItem, Entity, ExternalPaths, Image, ImageFormat, TestAppContext,
};

use super::fixtures::projection;
use crate::{
    bridge::{Bridge, BridgeCommand, RecordedRequests},
    screens::agent_thread::{AgentThreadEvent, AgentThreadView, ThreadHost},
    state::AppState,
};

#[derive(Default)]
struct Captured {
    commands: Vec<BridgeCommand>,
    notices: Vec<String>,
}

fn state_for(
    projection: &fleet_core::agents::ThreadProjection,
    cx: &mut TestAppContext,
) -> Entity<AppState> {
    let projection = projection.clone();
    cx.new(|_| {
        let mut app = AppState::new("/tmp/fleet-agent-media", Instant::now());
        app.agents
            .apply_summary(projection.summary(Default::default()));
        app.agents.install_snapshot(projection, &[]);
        app
    })
}

fn wire_media(
    view: &Entity<AgentThreadView>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut TestAppContext,
) -> Rc<RefCell<Captured>> {
    let captured = Rc::new(RefCell::new(Captured::default()));
    let seen = Rc::clone(&captured);
    let state = state.clone();
    let bridge = bridge.clone();
    cx.update(|cx| {
        cx.subscribe(view, move |view, event, cx| match event {
            AgentThreadEvent::StageMedia(media) => {
                crate::screens::agent_thread::media::stage_event(
                    &view,
                    &state,
                    &bridge,
                    media.clone(),
                    cx,
                );
            }
            AgentThreadEvent::CancelMedia(upload) => {
                crate::media::cancel(&state, &bridge, *upload, cx);
            }
            AgentThreadEvent::Command(command) => {
                seen.borrow_mut().commands.push(command.clone());
            }
            AgentThreadEvent::Notice(notice) => {
                seen.borrow_mut().notices.push(notice.to_string());
            }
            AgentThreadEvent::OpenInEditor(_)
            | AgentThreadEvent::Copy(_)
            | AgentThreadEvent::SelectThread(_)
            | AgentThreadEvent::AttachDelegation(_)
            | AgentThreadEvent::SelectCard(_)
            | AgentThreadEvent::LoadOlder
            | AgentThreadEvent::RefreshCheckpoints
            | AgentThreadEvent::AccountLogin => {}
        })
        .detach();
    });
    captured
}

fn emit_media(view: &Entity<AgentThreadView>, event: TextInputMedia, cx: &mut TestAppContext) {
    view.update(cx, |view, cx| {
        view.input().update(cx, |_input, cx| {
            cx.emit(MultilineInputEvent::Media(event));
        });
    });
    cx.run_until_parked();
}

#[track_caller]
fn finish_png_upload(requests: &RecordedRequests, cx: &mut TestAppContext, path: &str) {
    assert!(matches!(
        requests.respond_next(Ok(ResponseBody::Ack)),
        RequestBody::StageMedia {
            op: StageOp::Begin { .. },
            ..
        }
    ));
    cx.run_until_parked();
    assert!(matches!(
        requests.respond_next(Ok(ResponseBody::Ack)),
        RequestBody::StageMedia {
            op: StageOp::Chunk { .. },
            ..
        }
    ));
    cx.run_until_parked();
    assert!(matches!(
        requests.respond_next(Ok(ResponseBody::Path {
            path: path.to_owned(),
            host: None,
        })),
        RequestBody::StageMedia {
            op: StageOp::Finish { .. },
            ..
        }
    ));
    cx.run_until_parked();
}

#[gpui::test]
fn send_waits_while_attachment_metadata_is_being_checked(cx: &mut TestAppContext) {
    let projection = projection();
    let state = state_for(&projection, cx);
    let (bridge, requests) = Bridge::recording();
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let captured = wire_media(&view, &state, &bridge, cx);

    cx.update(|cx| {
        crate::screens::agent_thread::media::stage_event(
            &view,
            &state,
            &bridge,
            TextInputMedia::Pasted(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                vec![1, 2, 3],
            ))),
            cx,
        );
    });
    view.update(cx, |view, cx| {
        view.send_text("describe it".to_owned(), cx);
    });

    assert!(captured.borrow().commands.is_empty());
    assert_eq!(
        captured.borrow().notices.as_slice(),
        ["still copying clipboard.png"]
    );
    assert!(requests.take().is_empty());
    cx.run_until_parked();
}

#[gpui::test]
fn pasted_image_becomes_a_ready_chip_and_the_send_carries_its_path(cx: &mut TestAppContext) {
    cx.update(|cx| {
        fleet_ui_kit::Theme::init(fleet_ui_kit::ThemeMode::Dark, cx);
        crate::keymap::init(cx);
    });
    let projection = projection();
    let state = state_for(&projection, cx);
    let (bridge, requests) = Bridge::recording();
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let captured = wire_media(&view, &state, &bridge, cx);

    emit_media(
        &view,
        TextInputMedia::Pasted(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            vec![137, 80, 78, 71],
        ))),
        cx,
    );
    view.read_with(cx, |view, _| {
        assert_eq!(view.test_pending_attachments().len(), 1);
    });

    finish_png_upload(
        &requests,
        cx,
        "/fleet/agents/attachments/thread/clipboard.png",
    );
    view.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_pending_attachments()[0].state,
            PendingAttachmentState::Ready
        ));
    });

    view.update(cx, |view, cx| view.send_text("describe it".to_owned(), cx));
    let input = captured
        .borrow()
        .commands
        .iter()
        .find_map(|command| match command {
            BridgeCommand::AgentSend { input, .. } => Some(input.clone()),
            _ => None,
        })
        .expect("the ready attachment is sent");
    assert_eq!(input.text, "describe it");
    assert!(matches!(
        input.attachments.as_slice(),
        [fleet_core::agents::Attachment {
            name: Some(name),
            media_type,
            source: AttachmentSource::Path(path),
        }] if name == "clipboard.png"
            && media_type == "image/png"
            && path == &PathBuf::from("/fleet/agents/attachments/thread/clipboard.png")
    ));
    view.read_with(cx, |view, _| {
        assert!(view.rows().iter().any(|row| {
            matches!(
                &row.kind,
                TranscriptRowKind::User(user)
                    if user.attachments.as_ref() == ["clipboard.png"]
            )
        }));
    });

    let item = input
        .item
        .expect("the optimistic send has a client item id");
    view.update(cx, |view, cx| {
        view.send_failed(item, "thread stopped", cx);
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input().read(cx).text(cx), "describe it");
        assert_eq!(view.test_pending_attachments().len(), 1);
        assert!(matches!(
            view.test_pending_attachments()[0].state,
            PendingAttachmentState::Ready
        ));
    });
}

#[gpui::test]
fn removing_a_chip_during_staging_cancels_and_drops_a_late_result(cx: &mut TestAppContext) {
    let projection = projection();
    let state = state_for(&projection, cx);
    let (bridge, requests) = Bridge::recording();
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let _captured = wire_media(&view, &state, &bridge, cx);
    emit_media(
        &view,
        TextInputMedia::Pasted(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            vec![1, 2, 3],
        ))),
        cx,
    );
    let id = view.read_with(cx, |view, _| view.test_pending_attachments()[0].id);

    view.update(cx, |view, cx| view.remove_pending_attachment(id, cx));
    cx.run_until_parked();
    let requests = requests.take();
    assert!(matches!(
        requests.as_slice(),
        [
            RequestBody::StageMedia {
                op: StageOp::Begin { .. },
                ..
            },
            RequestBody::StageMedia {
                op: StageOp::Cancel,
                ..
            }
        ]
    ));

    view.update(cx, |view, cx| {
        view.test_complete_attachment(id, PathBuf::from("/late/clipboard.png"), cx);
    });
    view.read_with(cx, |view, _| {
        assert!(view.test_pending_attachments().is_empty());
    });
}

#[gpui::test]
fn a_ninth_attachment_is_refused_before_staging(cx: &mut TestAppContext) {
    let projection = projection();
    let state = state_for(&projection, cx);
    let (bridge, requests) = Bridge::recording();
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    for index in 0..8 {
        view.update(cx, |view, _| {
            view.test_add_ready_attachment(
                &format!("ready-{index}.png"),
                PathBuf::from(format!("/staged/ready-{index}.png")),
            );
        });
    }
    let captured = wire_media(&view, &state, &bridge, cx);

    emit_media(
        &view,
        TextInputMedia::Dropped(ExternalPaths(vec![PathBuf::from("/tmp/ninth.png")].into())),
        cx,
    );

    assert!(requests.take().is_empty());
    assert_eq!(
        captured.borrow().notices.as_slice(),
        [crate::screens::agent_thread::media::ATTACHMENT_COUNT_REFUSAL]
    );
    view.read_with(cx, |view, _| {
        assert_eq!(view.test_pending_attachments().len(), 8);
    });
}

#[gpui::test]
fn an_unreachable_thread_refuses_media_with_the_placeholder_wording(cx: &mut TestAppContext) {
    let projection = projection();
    let state = state_for(&projection, cx);
    let (bridge, requests) = Bridge::recording();
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let captured = wire_media(&view, &state, &bridge, cx);
    view.update(cx, |view, cx| {
        view.set_host(
            Some(ThreadHost {
                name: "dev-box".into(),
                unreachable: true,
            }),
            cx,
        );
    });

    emit_media(
        &view,
        TextInputMedia::Dropped(ExternalPaths(vec![PathBuf::from("/tmp/design.png")].into())),
        cx,
    );

    assert!(requests.take().is_empty());
    assert_eq!(
        captured.borrow().notices.as_slice(),
        ["dev-box is unreachable — nothing can be sent yet"]
    );
}

#[gpui::test]
fn a_ready_attachment_without_text_is_a_sendable_message(cx: &mut TestAppContext) {
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let commands = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&commands);
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let AgentThreadEvent::Command(command) = event {
                seen.borrow_mut().push(command.clone());
            }
        })
        .detach();
    });
    view.update(cx, |view, _| {
        view.test_add_ready_attachment("only.webp", PathBuf::from("/staged/only.webp"));
    });

    view.update(cx, |view, cx| view.send_text(String::new(), cx));

    assert!(matches!(
        commands.borrow().as_slice(),
        [BridgeCommand::AgentSend { input, .. }]
            if input.text.is_empty() && input.attachments.len() == 1
    ));
}
