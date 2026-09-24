use super::*;
use crate::dialogs::help::guides::GUIDES;

fn labels(here: &Here) -> Vec<&str> {
    here.featured.iter().map(|row| row.label.as_ref()).collect()
}

#[test]
fn here_in_worktrees_lists_the_six_things_a_worktree_row_is_for() {
    let here = Here::new(&["Hub", "Worktrees"]);
    assert_eq!(here.heading.as_ref(), "Here in Worktrees");
    assert_eq!(here.place, Some(Place::Worktrees));
    assert_eq!(
        labels(&here),
        vec![
            "Open the selected worktree",
            "New worktree",
            "Open the Claude agent window",
            "Filter the list",
            "Show or hide details",
            "Delete the worktree safely",
        ]
    );
    let open = here.featured[0].kbd.as_ref().map(Kbd::chip_labels);
    assert_eq!(
        open.as_deref().map(|labels| labels[0].as_ref()),
        Some("⏎"),
        "a row shows the first key the table binds it to"
    );
}

#[test]
fn a_deeper_binding_shadows_the_hub_key_it_shares() {
    // On the board `a` assigns the card and `/` filters the cards: the Hub's `a` (Claude) and
    // `/` (filter the list) are out of reach there, so Help must not offer them.
    let here = Here::new(&["Hub", "Board"]);
    assert_eq!(here.heading.as_ref(), "Here in Board");
    assert!(here.runs("board::PickAssignee") && here.runs("board::Filter"));
    assert!(!here.runs("fleet::OpenAgentClaude"));
    assert!(!here.runs("hub::OpenFilter"));
    assert!(
        here.runs("board::GoBoard"),
        "the Hub's own keys still work on its board tab"
    );
    assert_eq!(
        labels(&here),
        vec![
            "Open the card",
            "New card",
            "Start a worktree for the card",
            "Open the card's worktree",
            "Move the card right",
            "Filter the cards",
        ]
    );
}

#[test]
fn a_terminal_reaches_the_keys_after_the_prefix_spelled_in_full() {
    let here = Here::new(&["Workspace", "Terminal"]);
    assert_eq!(here.heading.as_ref(), "Here in Terminal");
    assert!(here.runs("prefix::ToggleZoom"));
    assert!(here.runs("workspace::CopySelection"));
    assert!(
        !here.runs("worktrees::Open"),
        "no Hub key reaches a terminal"
    );
    assert!(
        !here.runs("fleet::OpenHelp"),
        "Help never offers to reopen itself"
    );
    let zoom = here
        .kbd("prefix::ToggleZoom")
        .map(|kbd| kbd.strokes().len());
    assert_eq!(zoom, Some(2), "the prefix, then the key");
    assert_eq!(labels(&here)[0], "Back to the hub");
}

#[test]
fn an_agent_thread_runs_its_composer_and_session_keys() {
    let here = Here::new(&["Agent", "AgentIdle"]);
    assert_eq!(here.place, Some(Place::AgentThread));
    assert!(here.runs("native_agent::Send"));
    assert!(here.runs("native_agent::Model"));
    assert!(
        here.runs("prefix::GoHub"),
        "the session rows are repeated in a thread"
    );
    assert!(
        !here.runs("prefix::Paste"),
        "a thread has no PTY to paste into"
    );
    assert_eq!(labels(&here)[0], "Send the message");
}

#[test]
fn the_daemon_banner_does_not_hide_the_surface_under_it() {
    let here = Here::new(&["Hub", "Worktrees", "Daemon", "Banner"]);
    assert_eq!(here.heading.as_ref(), "Here in Worktrees");
    assert!(here.runs("daemon::Reconnect"));
}

#[test]
fn a_power_user_types_a_word_and_enter_runs_it() {
    // `^s ?`, "zoom", `⏎`: the cursor starts on the best action that works here, even though
    // the Terminals guide is listed above it.
    let mut help = HelpState::new(&["Workspace", "Terminal"]);
    help.set_query("zoom".to_owned());
    assert!(
        matches!(help.items.first(), Some(Item::Guide(_))),
        "guides come first"
    );
    let selected = help.selected().expect("a row under the cursor");
    assert_eq!(help.runnable(selected), Some("prefix::ToggleZoom"));
}

#[test]
fn without_a_query_the_cursor_rests_on_the_shown_guide() {
    let mut help = HelpState::new(&["Hub", "Worktrees"]);
    assert_eq!(help.selected(), Some(Item::Guide(0)));
    help.move_cursor(1);
    assert_eq!(help.selected(), Some(Item::Guide(1)));
    assert_eq!(help.shown_guide(), 1);
    help.move_cursor(-3);
    assert!(matches!(help.selected(), Some(Item::Here(_))));
    assert_eq!(help.shown_guide(), 1, "a here row leaves the guide on show");
    assert_eq!(help.runnable(Item::Here(0)), Some("worktrees::Open"));
}

#[test]
fn the_table_leaves_text_editing_keys_to_their_own_place() {
    let all = search("", None);
    let editing = search("", Some(Place::EditingText));
    assert!(!editing.shortcuts.is_empty());
    assert!(
        all.shortcuts
            .iter()
            .all(|ix| shortcuts()[*ix].place != Place::EditingText)
    );
    let editing_ix = Place::ALL
        .iter()
        .position(|place| *place == Place::EditingText)
        .expect("a place");
    assert_eq!(all.counts[editing_ix + 1], editing.shortcuts.len());
    assert_eq!(all.counts[0], all.shortcuts.len());
}

#[test]
fn search_matches_labels_before_descriptions_and_finds_guides() {
    let results = search("agent", None);
    let first = &shortcuts()[results.shortcuts[0]];
    assert!(first.label.to_lowercase().contains("agent"));
    assert_eq!(GUIDES[results.guides[0]].id, "agent");
    assert!(search("zzzz-nothing", None).shortcuts.is_empty());
}

#[test]
fn a_numbered_range_is_one_row_that_never_runs_from_help() {
    let row = shortcuts()
        .iter()
        .find(|row| row.label.as_ref() == "Go to tab 1\u{2013}9" && row.place == Place::Terminal)
        .expect("the tab range");
    assert!(row.range_end.is_some());
    assert!(
        row.runnable(&Here::new(&["Workspace", "Terminal"]))
            .is_none(),
        "which of the nine was meant is the one thing the row cannot say"
    );
}

#[test]
fn every_guide_button_has_a_key_to_show() {
    let here = Here::default();
    for guide in GUIDES {
        for step in guide.steps {
            for button in step.actions {
                assert!(
                    here.kbd(button.actions[0]).is_some(),
                    "{}: `{}` shows no key",
                    guide.id,
                    button.label
                );
            }
        }
    }
}

#[test]
fn the_legend_reads_three_real_keys() {
    let legend = legend();
    assert_eq!(legend.len(), 3);
    assert!(legend[0].1.starts_with("press "));
    assert!(prefix_kbd().is_some());
}
