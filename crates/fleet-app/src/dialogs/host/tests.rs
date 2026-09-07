use super::*;
use gpui::TestAppContext;
use std::{cell::Cell, rc::Rc, time::Instant};

struct Dropped(Rc<Cell<bool>>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[gpui::test]
fn drafts_are_isolated_and_released_with_their_window_model(cx: &mut TestAppContext) {
    let first = cx.new(|_| AppState::new("/tmp/first", Instant::now()));
    let second = cx.new(|_| AppState::new("/tmp/second", Instant::now()));
    let weak = cx.update(|cx| {
        with_host(&first, cx, |host| {
            host.open = Some(Dialogs::NewContext);
            host.context.name = TextFieldState::from_text("first");
        });
        with_host(&second, cx, |host| {
            host.open = Some(Dialogs::NewContext);
            host.context.name = TextFieldState::from_text("second");
        });
        close(&first, cx);
        assert!(with_host(&first, cx, |host| host.context.name.is_empty()));
        assert_eq!(
            with_host(&second, cx, |host| host.context.name.text().to_owned()),
            "second"
        );
        host_for(&first, cx).downgrade()
    });
    drop(first);
    cx.update(|_| ());
    assert!(weak.upgrade().is_none());
    cx.update(|cx| assert_eq!(cx.global::<DialogRegistry>().hosts.len(), 1));
}

#[gpui::test]
fn replacing_and_closing_cancel_queries_but_keep_accepted_completions(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/dialog", Instant::now()));
    let first = Rc::new(Cell::new(false));
    let second = Rc::new(Cell::new(false));
    let accepted = Rc::new(Cell::new(false));
    cx.update(|cx| {
        with_host(&state, cx, |host| host.open = Some(Dialogs::CloneRepo));
        for flag in [&first, &second] {
            let guard = Dropped(flag.clone());
            let task = cx.spawn(async move |_| {
                let _guard = guard;
                std::future::pending::<()>().await;
            });
            retain_task(&state, cx, "search", task);
        }
        let guard = Dropped(accepted.clone());
        complete_request(&state, cx, async move |_, _| {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
    });
    cx.run_until_parked();
    assert!(first.get());
    assert!(!second.get());
    cx.update(|cx| close(&state, cx));
    cx.run_until_parked();
    assert!(second.get());
    assert!(!accepted.get());
    drop(state);
    cx.update(|_| ());
    cx.run_until_parked();
    assert!(accepted.get());
}

#[gpui::test]
fn completed_requests_release_their_task_handles(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new("/tmp/dialog", Instant::now()));
    cx.update(|cx| complete_request(&state, cx, async move |_, _| {}));
    cx.run_until_parked();
    cx.update(|cx| assert!(with_host(&state, cx, |host| host.completions.is_empty())));
}
