//! Help's card: the search header, the Guides and All shortcuts tabs, and the footer.
//!
//! Composes what [`super::model`] prepared; it filters, ranks and resolves nothing itself.

mod shortcuts;

use fleet_ui_kit::{
    Button, ButtonSize, ButtonStyle, Dialog, EmptyState, Icon, IconSize, Kbd, KbdSize, Row,
    RowColumn, Segment, SegmentedControl, Text, Tone, prelude::*,
};
use gpui::{
    AnyElement, App, Entity, FocusHandle, ScrollStrategy, SharedString, Window, div, relative,
};

use super::{
    guides::{GUIDES, Guide},
    model::{HelpState, Item, Tab, shortcuts},
    run,
};
use crate::{
    action_catalogue::Place,
    actions::{dialog, help as help_actions},
    dialogs::{DialogHost, Dialogs, HELP_GUIDES_W, notify, root, with_host},
    presentation::{age_secs, now_unix},
    state::AppState,
};

/// Renders the help card (§3.8.7).
pub(crate) fn render(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (Some(help), Some(input)) = (
        host.read(cx).help.as_ref(),
        host.read(cx).help_input.clone(),
    ) else {
        return root(focus).into_any_element();
    };
    let cx: &App = cx;
    let theme = cx.theme();
    let viewport = window.viewport_size();
    // A window narrower than the card gets the whole window, less the scrim's margin.
    let width = Dialogs::Help
        .width(cx)
        .min(viewport.width - theme.space.xl * 2.0);
    let height = Dialogs::Help
        .height()
        .unwrap_or(viewport.height)
        .min(viewport.height - theme.space.xl * 2.0);
    let run_chip = Kbd::for_action(&dialog::Confirm, window, cx);

    let search = div()
        .flex()
        .items_center()
        .gap(theme.space.md)
        .w_full()
        .child(
            // The one editor of the dialog, so it is `dialog.field[0]` as well as its own name.
            div()
                .flex_1()
                .min_w_0()
                .child(input.harness_target("help.search"))
                .harness_target_indexed("dialog.field", 0),
        )
        .children(help.searching().then(|| {
            let count = match help.tab {
                Tab::Guides => help.results.guides.len() + help.results.shortcuts.len(),
                Tab::Shortcuts => help.results.shortcuts.len(),
            };
            Text::caption(match count {
                1 => "1 match".to_owned(),
                count => format!("{count} matches"),
            })
            .flex_none()
        }));

    let tabs_state = state.clone();
    let tabs = SegmentedControl::new(
        "help-tabs",
        [Segment::new("Guides"), Segment::new("All shortcuts")],
    )
    .active(Some(help.tab.index()))
    .harness_segments("help.tab")
    .on_select(move |ix, _, cx| {
        let tab = if ix == 0 { Tab::Guides } else { Tab::Shortcuts };
        edit(&tabs_state, cx, |help| help.set_tab(tab));
    });

    let body = div()
        .flex()
        .size_full()
        .min_h_0()
        .map(|body| match help.tab {
            Tab::Guides => body
                .child(guides_sidebar(help, state, cx))
                .child(guides_content(help, state, run_chip.clone(), cx)),
            Tab::Shortcuts => body
                .child(shortcuts::places_sidebar(help, state, cx))
                .child(shortcuts::shortcuts_content(
                    help,
                    state,
                    run_chip.clone(),
                    cx,
                )),
        });

    let doctor_state = state.clone();
    let doctor = Button::new("help-doctor", "Run doctor")
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .on_click(move |_, _, cx| run(&doctor_state, "daemon::RunDoctor", cx));

    root(focus)
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorDown, _, cx| step_cursor(&state, 1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::CursorUp, _, cx| step_cursor(&state, -1, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &dialog::Confirm, _, cx| confirm(&state, cx)
        })
        .on_action({
            let state = state.clone();
            move |_: &help_actions::SwitchTab, _, cx| {
                edit(&state, cx, |help| help.set_tab(help.tab.other()));
            }
        })
        .child(
            Dialog::new("Help")
                .dismiss_action(Dialogs::Help.dismiss_action())
                .width(width)
                .height(height)
                .header_fill(search)
                .header_actions(tabs)
                .flush_body(true)
                .body(body)
                .footer_start(footer_line(help, state, cx))
                .actions(vec![doctor]),
        )
        .into_any_element()
}

/// Applies `change` to the open draft and repaints.
pub(super) fn edit(state: &Entity<AppState>, cx: &mut App, change: impl FnOnce(&mut HelpState)) {
    with_host(state, cx, |host| {
        if let Some(help) = host.help.as_mut() {
            change(help);
        }
    });
    notify(state, cx);
}

/// `↓` / `↑`: move the cursor, and keep it in view.
fn step_cursor(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    edit(state, cx, |help| {
        help.move_cursor(delta);
        match help.tab {
            Tab::Guides => {
                if let Some(child) = sidebar_child(help, help.cursor) {
                    help.side_scroll.scroll_to_item(child);
                }
            }
            Tab::Shortcuts => help
                .table_scroll
                .scroll_to_item(help.cursor, ScrollStrategy::Nearest),
        }
    });
}

/// `⏎`: run the row under the cursor where that is possible; on a guide found by search, open
/// the guide.
fn confirm(state: &Entity<AppState>, cx: &mut App) {
    let Some((item, action)) = with_host(state, cx, |host| {
        let help = host.help.as_ref()?;
        let item = help.selected()?;
        Some((item, help.runnable(item)))
    }) else {
        return;
    };
    match (item, action) {
        (_, Some(action)) => run(state, action, cx),
        (Item::Guide(guide), None) => open_guide(state, guide, cx),
        _ => {}
    }
}

/// Shows `guide` with the search cleared, as a click on it in the guide list would.
pub(super) fn open_guide(state: &Entity<AppState>, guide: usize, cx: &mut App) {
    let input = with_host(state, cx, |host| {
        if let Some(help) = host.help.as_mut() {
            help.open_guide(guide);
        }
        host.help_input.clone()
    });
    // Clearing the field reports a change, which the draft already agrees with.
    if let Some(input) = input {
        input.update(cx, |input, cx| input.clear(cx));
    }
    notify(state, cx);
}

/// The position of the row `cursor` among the sidebar's children, headings included.
fn sidebar_child(help: &HelpState, cursor: usize) -> Option<usize> {
    let item = help.items.get(cursor)?;
    let guides = help
        .items
        .iter()
        .filter(|item| matches!(item, Item::Guide(_)))
        .count();
    let before = help.items.len() - guides;
    Some(match (help.searching(), item) {
        // Here heading, the here rows, the Guides heading, the guides.
        (false, Item::Here(_)) => cursor + 1,
        (false, _) => cursor + 2,
        // Guides heading when there are guide hits, then the Actions heading.
        (true, Item::Guide(_)) => cursor + 1,
        (true, _) => cursor + usize::from(guides > 0) + usize::from(before > 0),
    })
}

/// A sidebar heading: the section's name, and optionally a hint at its right.
pub(super) fn heading(label: &'static str, hint: Option<&'static str>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .px(theme.space.sm)
        .pt(theme.space.md)
        .pb(theme.space.xs)
        .child(Text::sentence_label(label))
        .children(hint.map(|hint| Text::caption(hint).faint()))
        .into_any_element()
}

/// A heading whose name is prepared text rather than a literal.
fn heading_text(label: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .px(theme.space.sm)
        .pt(theme.space.md)
        .pb(theme.space.xs)
        .child(Text::sentence_label(label))
        .child(Text::caption("click or press").faint())
        .into_any_element()
}

/// The left column of the Guides tab: "Here in …" and the guides, or what a search matched.
fn guides_sidebar(help: &HelpState, state: &Entity<AppState>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut children: Vec<AnyElement> = Vec::new();
    let searching = help.searching();
    let mut shown_actions_heading = false;
    if searching && help.results.guides.is_empty() && help.items.is_empty() {
        children.push(
            div()
                .p(theme.space.md)
                .child(EmptyState::new(format!(
                    "Nothing matches \u{201c}{}\u{201d}.",
                    help.query.trim()
                )))
                .into_any_element(),
        );
    }
    if !searching {
        children.push(heading_text(help.here.heading.clone(), cx));
    } else if !help.results.guides.is_empty() {
        children.push(heading("Guides", None, cx));
    }
    for (ix, item) in help.items.iter().enumerate() {
        let selected = ix == help.cursor;
        let row = match *item {
            Item::Here(here) => {
                let Some(row) = help.here.featured.get(here) else {
                    continue;
                };
                let action = row.action;
                let run_state = state.clone();
                sidebar_row(row.label.clone(), None, row.kbd.clone(), selected)
                    .on_click(move |_, _, cx| run(&run_state, action, cx))
                    .harness_target_indexed("help.here", here)
                    .into_any_element()
            }
            Item::Guide(guide) => {
                if !searching && guide == 0 {
                    children.push(heading("Guides", None, cx));
                }
                let Some(entry) = GUIDES.get(guide) else {
                    continue;
                };
                let select_state = state.clone();
                let shown = selected || (!searching && help.guide == guide);
                sidebar_row(entry.title.into(), Some(entry.icon), None, shown)
                    .id(entry.id)
                    .on_click(move |_, _, cx| {
                        edit(&select_state, cx, |help| help.set_cursor(ix));
                    })
                    .harness_target_indexed("help.guide", guide)
                    .into_any_element()
            }
            Item::Shortcut(shortcut) => {
                if !shown_actions_heading {
                    children.push(heading("Actions", None, cx));
                    shown_actions_heading = true;
                }
                let Some(row) = shortcuts().get(shortcut) else {
                    continue;
                };
                let select_state = state.clone();
                let position = ix - help.results.guides.len();
                sidebar_row(row.label.clone(), None, row.kbd.clone(), selected)
                    .on_click(move |_, _, cx| {
                        edit(&select_state, cx, |help| help.set_cursor(ix));
                    })
                    .harness_target_indexed("help.shortcut", position)
                    .into_any_element()
            }
        };
        children.push(row);
    }
    div()
        .id("help-sidebar")
        .flex()
        .flex_col()
        .flex_none()
        .w(HELP_GUIDES_W)
        .h_full()
        .overflow_y_scroll()
        .track_scroll(&help.side_scroll)
        .px(theme.space.md)
        .pb(theme.space.md)
        .bg(theme.colors.surface)
        .border_r(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .children(children)
        .into_any_element()
}

/// One clickable row of a sidebar list: an optional glyph, the label and an optional key.
fn sidebar_row(label: SharedString, icon: Option<Icon>, kbd: Option<Kbd>, selected: bool) -> Row {
    Row::new()
        .selected(selected)
        .children_leading(icon)
        .column(RowColumn::flex(Text::ui(label).ellipsize()))
        .children_column(kbd.map(|kbd| RowColumn::auto(kbd.size(KbdSize::Small))))
}

/// `Row` helpers for optional slots, so the builders read top to bottom.
trait RowSlots {
    fn children_leading(self, icon: Option<Icon>) -> Self;
    fn children_column(self, column: Option<RowColumn>) -> Self;
}

impl RowSlots for Row {
    fn children_leading(self, icon: Option<Icon>) -> Self {
        match icon {
            Some(icon) => self.leading(icon.el().size(IconSize::Medium)),
            None => self,
        }
    }

    fn children_column(self, column: Option<RowColumn>) -> Self {
        match column {
            Some(column) => self.column(column),
            None => self,
        }
    }
}

/// The right column of the Guides tab: the shown guide, or the action a search selected.
fn guides_content(
    help: &HelpState,
    state: &Entity<AppState>,
    run_chip: Option<Kbd>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let content = match help.selected() {
        Some(Item::Shortcut(shortcut)) if help.searching() => {
            action_detail(help, shortcut, state, run_chip, cx)
        }
        _ => match GUIDES.get(help.shown_guide()) {
            Some(guide) => guide_view(guide, help, state, cx),
            None => div().into_any_element(),
        },
    };
    div()
        .id("help-guide")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .track_scroll(&help.guide_scroll)
        .px(theme.space.xl)
        .py(theme.space.lg)
        .child(content)
        .into_any_element()
}

/// One guide: its title, its intro, a card per step, and the terminal callout.
fn guide_view(
    guide: &'static Guide,
    help: &HelpState,
    state: &Entity<AppState>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let steps = guide.steps.iter().enumerate().map(|(n, step)| {
        let buttons = step.actions.iter().enumerate().map(|(m, button)| {
            let runnable = help.here.first_runnable(button.actions);
            let shown = runnable.or_else(|| button.actions.first().copied());
            let kbd = shown.and_then(|action| help.here.kbd(action));
            let destructive = shown
                .and_then(crate::action_catalogue::info)
                .is_some_and(|info| info.destructive);
            let run_state = state.clone();
            let mut control = Button::new(("help-step", n * 8 + m), button.label)
                .size(ButtonSize::Compact)
                .when(destructive, |control| control.style(ButtonStyle::Danger))
                .disabled(runnable.is_none());
            // A guide teaches keys, so its buttons are among the few that keep the chip on the
            // face (DESIGN-SYSTEM §4); `here` is read from the key table, so it holds still.
            if let Some(kbd) = kbd {
                control = control.kbd(kbd).show_kbd();
            }
            match runnable {
                Some(action) => control.on_click(move |_, _, cx| run(&run_state, action, cx)),
                None => control.tooltip(elsewhere(shown)),
            }
            .harness_target(step_target(n, m))
        });
        div()
            .flex()
            .items_start()
            .gap(theme.space.md)
            .p(theme.space.md)
            .rounded(theme.radii.card)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .bg(theme.colors.surface)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(theme.metrics.chip_h)
                    .rounded(theme.radii.full)
                    .bg(theme.colors.accent_subtle)
                    .child(Text::caption((n + 1).to_string()).tone(Tone::Accent)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.space.xxs)
                    .child(Text::ui_strong(step.title))
                    .child(Text::caption(step.body)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_wrap()
                    .justify_end()
                    .max_w(relative(0.5))
                    .gap(theme.space.xs)
                    .children(buttons),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap(theme.space.md)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(theme.space.xs)
                .child(Text::section_title(guide.title))
                .child(Text::ui(guide.intro).muted()),
        )
        .children(steps)
        .children(guide.terminal_callout.then(|| terminal_callout(cx)))
        .into_any_element()
}

/// Why a step's button is not live here: where its key works.
fn elsewhere(action: Option<&'static str>) -> String {
    match action
        .and_then(crate::action_catalogue::info)
        .map(|info| info.place)
    {
        Some(place) => format!(
            "Works in {}: open Help there to run it",
            place_phrase(place)
        ),
        None => "Does not work here".to_owned(),
    }
}

/// A place as it reads inside a sentence.
fn place_phrase(place: Place) -> String {
    match place {
        Place::Everywhere => "every screen".to_owned(),
        Place::Hub => "the hub".to_owned(),
        Place::Terminal => "a worktree's terminals".to_owned(),
        Place::AgentThread => "an agent thread".to_owned(),
        Place::AgentPopup => "the agent window".to_owned(),
        Place::Scroll => "scroll mode".to_owned(),
        Place::Dialog => "that dialog".to_owned(),
        Place::EditingText => "a text field".to_owned(),
        other => format!("the {} screen", other.title().to_lowercase()),
    }
}

/// The harness name of step `n`'s button `m`, prepared once per process.
fn step_target(n: usize, m: usize) -> SharedString {
    static NAMES: std::sync::OnceLock<Vec<Vec<SharedString>>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(|| {
        let most = GUIDES
            .iter()
            .map(|guide| guide.steps.len())
            .max()
            .unwrap_or(0);
        (0..most)
            .map(|n| {
                (0..4)
                    .map(|m| format!("help.step[{n}].action[{m}]").into())
                    .collect()
            })
            .collect()
    });
    names
        .get(n)
        .and_then(|row| row.get(m))
        .cloned()
        .unwrap_or_else(|| format!("help.step[{n}].action[{m}]").into())
}

/// The one idea that confuses everyone: inside a terminal, Fleet's keys need the prefix.
fn terminal_callout(cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .items_start()
        .gap(theme.space.md)
        .p(theme.space.md)
        .mt(theme.space.sm)
        .rounded(theme.radii.card)
        .bg(theme.colors.accent_subtle)
        .child(
            Icon::Info
                .el()
                .size(IconSize::Medium)
                .color(theme.colors.accent),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(theme.space.xxs)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(theme.space.xs)
                        .child(Text::ui_strong(
                            "Inside a terminal, Fleet's keys start with",
                        ))
                        .children(super::model::prefix_kbd().map(|kbd| kbd.size(KbdSize::Small))),
                )
                .child(Text::caption(
                    "Everything else you type goes to the program running there. Hold it a moment to \
                     see every Fleet command, or press it twice to send it to the program.",
                )),
        )
        .into_any_element()
}

/// What a search selected: the action's name, what it does, where it works, and Run.
fn action_detail(
    help: &HelpState,
    shortcut: usize,
    state: &Entity<AppState>,
    run_chip: Option<Kbd>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let Some(row) = shortcuts().get(shortcut) else {
        return div().into_any_element();
    };
    let runnable = row.runnable(&help.here);
    let run_state = state.clone();
    div()
        .flex()
        .flex_col()
        .gap(theme.space.md)
        .child(Text::section_title(row.label.clone()))
        .child(Text::ui(row.entry.info.description).muted())
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.space.sm)
                .child(Text::caption("Works in"))
                .child(Text::ui(row.place_title.clone()))
                .children(row.kbd.clone())
                .children(row.range_end.clone().map(|end| {
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space.xs)
                        .child(Text::caption("\u{2013}"))
                        .child(end)
                })),
        )
        .child(match runnable {
            Some(action) => div()
                .flex()
                .child(
                    Button::new("help-run", "Run")
                        .style(ButtonStyle::Primary)
                        .when_some(run_chip, Button::kbd)
                        .on_click(move |_, _, cx| run(&run_state, action, cx))
                        .harness_target("help.run"),
                )
                .into_any_element(),
            None => Text::caption(format!(
                "It works in {}. Open Help there to run it from here.",
                place_phrase(row.place)
            ))
            .into_any_element(),
        })
        .into_any_element()
}

/// `Fleet 0.1.0 · fleetd up 3h · protocol 8`, and on the table the hint that rows run.
fn footer_line(help: &HelpState, state: &Entity<AppState>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (version, uptime) = state.read(cx).snapshot.as_ref().map_or_else(
        || ("\u{2013}".to_owned(), "\u{2013}".to_owned()),
        |snapshot| {
            let started = age_secs(&snapshot.daemon.started_at, now_unix());
            (
                crate::presentation::bare_version(&snapshot.daemon.version).to_owned(),
                started.map_or_else(|| "\u{2013}".to_owned(), fleet_ui_kit::format_age),
            )
        },
    );
    div()
        .flex()
        .items_center()
        .gap(theme.space.md)
        .min_w_0()
        .child(
            Text::caption(format!(
                "Fleet {version} \u{00b7} fleetd up {uptime} \u{00b7} protocol {}",
                super::protocol()
            ))
            .faint(),
        )
        .children(
            (help.tab == Tab::Shortcuts)
                .then(|| Text::caption("Click a row that works here to run it").ellipsize()),
        )
        .into_any_element()
}
