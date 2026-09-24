use std::time::Instant;

use gpui::TestAppContext;

use super::*;
use crate::action_catalogue;

fn key(source: &str) -> Keystroke {
    Keystroke::parse(source).unwrap_or_else(|error| panic!("{source:?}: {error}"))
}

/// `(column, key, label)` for every item, the way a person reads the menu.
fn rows(model: &PrefixMenuModel) -> Vec<(&'static str, String, &'static str)> {
    model
        .columns
        .iter()
        .flat_map(|column| {
            column
                .items
                .iter()
                .map(move |item| (column.title, item.key.unparse(), item.label))
        })
        .collect()
}

#[track_caller]
fn assert_row(model: &PrefixMenuModel, column: &str, keys: &str, label: &str) {
    let rows = rows(model);
    assert!(
        rows.iter()
            .any(|(title, key, text)| *title == column && key == keys && *text == label),
        "no {column} / {keys} / {label} row in {rows:#?}"
    );
}

#[test]
fn the_workspace_menu_reads_in_the_five_columns_of_the_mockup() {
    let model = PrefixMenuModel::build(PrefixSurface::Workspace, &["Workspace", "Prefix"]);
    let titles: Vec<_> = model.columns.iter().map(|column| column.title).collect();
    assert_eq!(
        titles,
        ["Tabs", "Session", "Terminal", "Agents", "Panels"],
        "{:#?}",
        rows(&model)
    );
    assert_row(&model, "Tabs", "c", "New terminal");
    assert_row(&model, "Tabs", "h", "Previous tab");
    assert_row(&model, "Session", "s", "Back to hub");
    assert_row(&model, "Terminal", "[", "Scroll back");
    assert_row(&model, "Agents", "a", "New Claude thread");
    assert_row(&model, "Panels", "?", "All shortcuts");
    let range = model
        .items()
        .find(|item| item.range_end.is_some())
        .unwrap_or_else(|| panic!("no range row in {:#?}", rows(&model)));
    assert_eq!(
        (range.key.clone(), range.range_end.clone()),
        (key("1"), Some(key("9")))
    );
    assert_eq!(range.action.name(), "prefix::SelectTab1");
}

#[test]
fn the_prefix_itself_and_escape_are_the_header_not_rows() {
    let model = PrefixMenuModel::build(PrefixSurface::Workspace, &["Workspace", "Prefix"]);
    assert_eq!(model.prefix, Some(key("ctrl-s")));
    assert!(model.literal, "^s ^s sends a literal ^s to the PTY");
    let (close, action) = model
        .close
        .as_ref()
        .unwrap_or_else(|| panic!("escape cancels the prefix"));
    assert_eq!(
        (close.clone(), action.name()),
        (key("escape"), "prefix::Cancel")
    );
    assert!(
        model
            .items()
            .all(|item| !keymap::is_prefix_key(&item.key) && item.key.key != "escape"),
        "{:#?}",
        rows(&model)
    );
}

/// Every command of the one-shot prefix table is on the menu, and every row runs what its key
/// runs: the menu is the table, not a selection from it.
#[test]
fn every_workspace_prefix_row_is_reachable_from_the_menu() {
    let model = PrefixMenuModel::build(PrefixSurface::Workspace, &["Workspace", "Prefix"]);
    for item in model.items() {
        let resolved = keymap::action_for_keystroke("Workspace > Prefix", &item.key)
            .map(|action| action.name());
        assert_eq!(resolved, Some(item.action.name()), "{}", item.label);
    }
    let entries: Vec<_> = model
        .items()
        .filter_map(|item| action_catalogue::entry(item.action.name()))
        .collect();
    for spec in keymap::table()
        .into_iter()
        .filter(|spec| spec.context == "Workspace > Prefix")
    {
        if matches!(spec.action, "prefix::SendLiteral" | "prefix::Cancel") {
            continue;
        }
        let entry = action_catalogue::entry(spec.action)
            .unwrap_or_else(|| panic!("{} has a catalogue entry", spec.action));
        assert!(
            entries.iter().any(|known| std::ptr::eq(*known, entry)),
            "{} ({}) is missing from the menu",
            spec.action,
            spec.keys
        );
    }
}

#[test]
fn a_thread_lists_its_own_chords_and_sends_no_literal() {
    let chain = ["Agent", "AgentIdle"];
    let model = PrefixMenuModel::build(PrefixSurface::AgentThread, &chain);
    assert_eq!(model.prefix, Some(key("ctrl-s")));
    assert!(!model.literal, "there is no PTY behind a thread");
    assert!(model.close.is_some());
    assert_row(&model, "Session", "s", "Back to hub");
    for item in model.items() {
        assert_eq!(
            keymap::chord_action_for_chain(&chain, &item.key).map(|action| action.name()),
            Some(item.action.name()),
            "{}",
            item.label
        );
    }
}

#[test]
fn the_agent_window_lists_its_own_prefix_table() {
    let model = PrefixMenuModel::build(PrefixSurface::AgentPopup, &["Agent", "Prefix"]);
    assert_eq!(model.prefix, Some(key("ctrl-s")));
    assert!(model.literal);
    let actions: Vec<_> = model.items().map(|item| item.action.name()).collect();
    assert!(actions.contains(&"agent::Hide"), "{actions:?}");
    assert!(actions.contains(&"prefix::EnterScroll"), "{actions:?}");
    assert!(
        !actions.contains(&"prefix::GoHub"),
        "the Workspace table does not leak into the popup: {actions:?}"
    );
    let hide = model
        .items()
        .find(|item| item.action.name() == "agent::Hide")
        .map(|item| item.key.clone());
    assert_eq!(
        hide,
        Some(key("q")),
        "`^q` works without the prefix; the menu lists what the prefix adds"
    );
}

#[gpui::test]
fn the_menu_waits_out_its_delay_and_restarts_it_per_prefix(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::for_mode(fleet_ui_kit::ThemeMode::Dark)));
    let state = cx.new(|_| AppState::new("/tmp/fleet-prefix-test", Instant::now()));
    let half = Duration::from_millis(cx.update(|cx| cx.theme().motion.prefix_hint_delay) / 2);
    let mut menu = PrefixMenuState::default();
    let held = Some(PrefixSurface::Workspace);
    cx.update(|cx| menu.reconcile(held, &state, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(half);
    cx.update(|cx| menu.reconcile(None, &state, cx));
    cx.update(|cx| menu.reconcile(held, &state, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(half);
    cx.run_until_parked();
    assert!(!menu.visible(), "a released prefix restarts the delay");
    assert!(menu.model.is_none(), "nothing is built before the reveal");
    cx.executor().advance_clock(half);
    cx.run_until_parked();
    assert!(menu.visible());
    cx.update(|cx| menu.reconcile(held, &state, cx));
    assert!(
        menu.model.is_some(),
        "the pass after the reveal builds the menu"
    );
    cx.update(|cx| menu.reconcile(None, &state, cx));
    assert!(!menu.visible());
    assert!(menu.model.is_none());
}

#[gpui::test]
fn dropping_the_surface_cancels_a_pending_reveal(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::for_mode(fleet_ui_kit::ThemeMode::Dark)));
    let state = cx.new(|_| AppState::new("/tmp/fleet-prefix-test", Instant::now()));
    let mut menu = PrefixMenuState::default();
    let visible = Rc::clone(&menu.visible);
    cx.update(|cx| menu.reconcile(Some(PrefixSurface::Workspace), &state, cx));
    cx.run_until_parked();
    drop(menu);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!visible.get());
}
