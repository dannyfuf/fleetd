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
fn editing_context_words_follow_the_existing_dialog_drafts(cx: &mut TestAppContext) {
    let mut host = DialogHost::default();

    assert_eq!(
        dialog_key_context(&Dialogs::CreateWorktree, &host),
        "CreateEditing"
    );
    host.create.field = create_worktree::Field::Host;
    assert_eq!(
        dialog_key_context(&Dialogs::CreateWorktree, &host),
        "Create"
    );

    assert_eq!(dialog_key_context(&Dialogs::Settings, &host), "Settings");
    host.settings.editing = Some("claude".to_owned());
    assert_eq!(
        dialog_key_context(&Dialogs::Settings, &host),
        "SettingsEditing"
    );

    assert_eq!(
        dialog_key_context(&Dialogs::BoardSettings, &host),
        "BoardSettings"
    );
    host.board_settings_input =
        Some(cx.new(|cx| TextInput::new(fleet_ui_kit::InputMode::SingleLine, cx)));
    assert_eq!(
        dialog_key_context(&Dialogs::BoardSettings, &host),
        "BoardSettingsEditing"
    );
    assert_eq!(
        dialog_key_context(&Dialogs::CardPicker, &host),
        "CardPicker"
    );
    assert_eq!(
        dialog_key_context(&Dialogs::CardDetail, &host),
        "CardDetail"
    );
}

#[gpui::test]
fn derived_dialog_word_is_mirrored_into_the_app_context_chain(cx: &mut TestAppContext) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/dialog-context", Instant::now());
        state.open_overlay(Overlay::Dialog(Dialogs::Settings));
        state
    });
    cx.update(|cx| {
        let host = host_for(&state, cx);
        host.update(cx, |host, _| {
            host.settings.editing = Some("claude".to_owned());
        });
        sync_dialog_key_context(&state, &host, cx);
        assert_eq!(
            state.read(cx).context_chain(),
            vec!["Dialog", "SettingsEditing"]
        );
    });
}

#[gpui::test]
fn drafts_are_isolated_and_released_with_their_window_model(cx: &mut TestAppContext) {
    let first = cx.new(|_| AppState::new("/tmp/first", Instant::now()));
    let second = cx.new(|_| AppState::new("/tmp/second", Instant::now()));
    let weak = cx.update(|cx| {
        with_host(&first, cx, |host| {
            host.open = Some(Dialogs::NewContext);
            host.context.name = "first".to_owned();
        });
        with_host(&second, cx, |host| {
            host.open = Some(Dialogs::NewContext);
            host.context.name = "second".to_owned();
        });
        close(&first, cx);
        assert!(with_host(&first, cx, |host| host.context.name.is_empty()));
        assert_eq!(
            with_host(&second, cx, |host| host.context.name.clone()),
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

#[gpui::test]
fn agents_picker_opens_with_the_agents_seed_and_leaves_prefix(cx: &mut TestAppContext) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/dialog", Instant::now());
        state.screen = crate::state::Screen::Workspace {
            session: "widgets/feature"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.enter_prefix();
        state
    });
    cx.update(|cx| open_agents_picker(&state, cx));
    cx.read(|cx| {
        let state = state.read(cx);
        assert_eq!(state.palette_seed.as_deref(), Some("!"));
        assert_eq!(state.overlay, Some(Overlay::Palette));
        assert_ne!(state.terminal_mode, crate::state::TerminalMode::Prefix);
    });
}

/// Help describes the surface under it, its field owns the keyboard, typing searches, a row
/// runs by closing Help and leaving the action for the surface behind, and closing drops the
/// draft with its editor.
#[gpui::test]
fn help_searches_from_its_field_and_runs_a_row_on_the_surface_behind(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        crate::keymap::init(cx);
    });
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/help-dialog", Instant::now());
        state.open_overlay(Overlay::Dialog(Dialogs::Help));
        state
    });
    cx.update(|cx| {
        with_host(&state, cx, |host| host.open = Some(Dialogs::Help));
        super::super::help::seed(&state, cx);
        assert!(
            focused_input(&state, cx).is_some(),
            "typing goes to the search field"
        );
        let here = with_host(&state, cx, |host| {
            host.help.as_ref().map(|help| help.here.heading.to_string())
        });
        assert_eq!(here.as_deref(), Some("Here in Worktrees"));
        let input = with_host(&state, cx, |host| host.help_input.clone()).expect("a field");
        input.update(cx, |input, cx| input.set_text("new worktree", cx));
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let action = with_host(&state, cx, |host| {
            let help = host.help.as_ref()?;
            help.runnable(help.selected()?)
        });
        assert_eq!(action, Some("worktrees::Create"));
        super::super::help::run(&state, "worktrees::Create", cx);
        assert!(
            state.read(cx).overlay.is_none(),
            "running closes Help first"
        );
        assert_eq!(state.read(cx).pending_action, Some("worktrees::Create"));
        close(&state, cx);
        assert!(with_host(&state, cx, |host| host.help.is_none()
            && host.help_input.is_none()));
    });
}
