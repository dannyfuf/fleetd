use super::*;
use gpui::{Context, FocusHandle, Render, Window, div};

struct InputView {
    state: Entity<AppState>,
    focus: FocusHandle,
}
impl Render for InputView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = with_host(&self.state, cx, |host| field(&host.context.name));
        actions(
            div()
                .key_context("Dialog")
                .track_focus(&self.focus)
                .size_full(),
            &self.state,
            |host| &mut host.context.name,
            notify,
        )
        .child(name.focused(true))
    }
}

#[gpui::test]
fn shared_actions_edit_the_selected_buffer_and_preserve_clear_semantics(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| AppState::new("/tmp/input", std::time::Instant::now()));
    cx.update(|cx| {
        with_host(&state, cx, |host| {
            host.context.name = TextFieldState::from_text("héllo")
        })
    });
    let window = cx.add_window(|_, cx| InputView {
        state: state.clone(),
        focus: cx.focus_handle(),
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(&mut visual, |view, window, cx| {
            window.focus(&view.focus, cx)
        })
        .expect("focus input");
    visual.simulate_keystrokes("left backspace");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.context.name.text(), "hélo")
        })
    });
    visual.simulate_keystrokes("ctrl-a right ctrl-w");
    visual.update(|_, cx| {
        with_host(&state, cx, |host| {
            assert_eq!(host.context.name.text(), "élo");
            assert_eq!(host.context.name.caret_chars(), 0);
        })
    });
    // `ctrl-u` empties the buffer even with the caret at its start, unlike the kit's
    // delete-to-start.
    visual.simulate_keystrokes("ctrl-e ctrl-a ctrl-u");
    visual.update(|_, cx| with_host(&state, cx, |host| assert!(host.context.name.is_empty())));
}
