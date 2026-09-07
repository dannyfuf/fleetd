use super::*;
use gpui::{TestAppContext, WindowHandle};

fn test_window(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<Lazygit>,
    async_channel::Receiver<GitRequest>,
    async_channel::Sender<GitEvent>,
) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let (bridge, requests, events) = crate::bridge::channel_bridge();
    let window = cx.add_window(|window, cx| {
        let mut pane = Lazygit::new(PathBuf::from("/fleet-lazygit-test-nonexistent"), cx);
        pane._event_task = Lazygit::spawn_event_loop(&bridge, cx);
        pane.bridge = bridge;
        pane.state.refreshing = false;
        pane.set_active(true, window, cx);
        pane
    });
    (window, requests, events)
}

fn patch(lines: usize, value: &str) -> Arc<fleet_git::Diff> {
    let mut patch = format!(
        "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -0,0 +1,{lines} @@\n"
    );
    for _ in 0..lines {
        patch.push_str(&format!("+{value}\n"));
    }
    Arc::new(fleet_git::parse::diff::parse(patch.as_bytes()).unwrap())
}

#[gpui::test]
fn hidden_panes_stop_timers_and_watcher_reads_and_refresh_on_activation(cx: &mut TestAppContext) {
    let (window, requests, events) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.set_active(false, window, cx);
            assert!(pane._ticker.is_none());
        })
        .unwrap();
    events
        .try_send(GitEvent::Changed(Box::new(fleet_git::ChangeEvent {
            paths: vec!["file.rs".into()],
            debounce: Duration::ZERO,
        })))
        .unwrap();
    cx.background_executor
        .advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    assert!(requests.is_empty());
    window
        .update(cx, |pane, window, cx| pane.set_active(true, window, cx))
        .unwrap();
    assert!(matches!(requests.try_recv(), Ok(GitRequest::Snapshot)));
    assert!(requests.is_empty());
}

#[gpui::test]
fn overlay_transitions_restore_pane_focus_without_stealing_host_focus(cx: &mut TestAppContext) {
    let (window, _, _events) = test_window(cx);
    window
        .update(cx, |pane, window, cx| {
            pane.open_help(&global::OpenHelp, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            assert!(pane.overlay_focus.is_focused(window));
            pane.help_close(&lg_help::Close, window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |pane, window, cx| {
            assert!(pane.focus.is_focused(window));
            pane.set_active(false, window, cx);
            let host = cx.focus_handle();
            window.focus(&host, cx);
            let _ = pane.render(window, cx);
            assert!(host.is_focused(window));
        })
        .unwrap();
}

#[gpui::test]
fn staged_only_models_are_retained_and_superseded_preparation_cannot_publish(
    cx: &mut TestAppContext,
) {
    let (window, _, _events) = test_window(cx);
    let first = patch(5000, "let obsolete = 1;");
    let latest = patch(50, "let current = 2;");
    window
        .update(cx, |pane, _, cx| {
            pane.state.main = MainContent::FileDiff {
                path: "file.rs".into(),
                unstaged: None,
                staged: Some(first),
            };
            pane.prepare_models(cx);
            pane.state.main = MainContent::FileDiff {
                path: "file.rs".into(),
                unstaged: None,
                staged: Some(latest.clone()),
            };
            pane.prepare_models(cx);
        })
        .unwrap();
    cx.run_until_parked();
    let prepared = window
        .update(cx, |pane, _, cx| {
            let model = pane.main_model();
            assert_eq!(model.rows.len(), 53);
            assert_eq!(model.rows[3].text, "let current = 2;");
            for _ in 0..100 {
                pane.prepare_models(cx);
                assert!(Rc::ptr_eq(&model, &pane.main_model()));
            }
            assert_eq!(pane.models.len(), 1);
            model
        })
        .unwrap();
    window
        .update(cx, |pane, window, cx| {
            pane.set_active(false, window, cx);
            assert!(pane.models.is_empty());
        })
        .unwrap();
    assert_eq!(prepared.rows.len(), 53);
}
