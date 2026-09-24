//! The 28 px status bar (UX-SPEC §2.2): the daemon, the breadcrumb, the job ticker or the sticky
//! error, and the buttons that teach the two keys everything else hangs off.

use fleet_ui_kit::{Button, ButtonSize, ButtonStyle, DaemonState, HarnessTargetExt, StatusBar};
use gpui::{AnyElement, App, IntoElement, SharedString};

use crate::{
    actions::{fleet, workspace},
    dialogs::Dialogs,
    presentation::{hub_sticky_error_key, terminal_label, workspace_keys},
    screens::hub::effective_context,
    shell::daemon::{dot_label, dot_state},
    state::{AppState, HubTab, Mode, Overlay, RepoScope, Screen, StickyError, breadcrumb},
    views::{job_ticker, sticky_error},
};

/// Which buttons the right end of the bar carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Place {
    /// `Shortcuts ?`.
    #[default]
    Hub,
    /// `Fleet commands ⌃S` and `Shortcuts ⌃S ?`.
    Terminal,
    /// `Shortcuts ⌃S ?` only. A native agent thread takes its prefix as a chord the shell's
    /// interceptor resolves, so there is no prefix *mode* for a button to enter; the ⌃S menu
    /// appears when the chord is held (§3.6.0).
    AgentThread,
}

/// Everything the status bar draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::shell) struct StatusModel {
    daemon: DaemonState,
    daemon_word: Option<&'static str>,
    breadcrumb: SharedString,
    slot: job_ticker::StatusSlot,
    /// The screen the sticky error's focus key is spelled for.
    screen: Option<Screen>,
    place: Place,
    mode: Option<Mode>,
}

impl StatusModel {
    pub(super) fn build(state: &AppState) -> Self {
        let jobs = state
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.jobs.as_slice())
            .unwrap_or_default();
        let place = match state.screen {
            Screen::Hub { .. } => Place::Hub,
            Screen::Workspace { .. } if state.active_agent_thread().is_some() => Place::AgentThread,
            Screen::Workspace { .. } => Place::Terminal,
        };
        // Over a terminal the bar says which process the keys go to, how big it is, and that
        // closing the window does not end it (§3.6); everywhere else, where you are.
        let breadcrumb = match place {
            Place::Terminal => terminal_line(state).unwrap_or_else(|| breadcrumb_text(state)),
            Place::Hub | Place::AgentThread => breadcrumb_text(state),
        };
        Self {
            daemon: dot_state(&state.daemon),
            daemon_word: dot_label(&state.daemon),
            breadcrumb: SharedString::from(breadcrumb),
            slot: job_ticker::status_slot(jobs, state.sticky_error.as_ref()),
            screen: Some(state.screen.clone()),
            place,
            mode: Some(state.mode()),
        }
    }
}

pub(super) fn render(model: &StatusModel, _cx: &App) -> AnyElement {
    let mut bar = StatusBar::new()
        .daemon(
            model.daemon,
            model.daemon_word.map(SharedString::new_static),
        )
        .breadcrumb(model.breadcrumb.clone());
    match &model.slot {
        job_ticker::StatusSlot::Error(error) => {
            bar = bar.error(error_slot(error, model.screen.as_ref()));
        }
        job_ticker::StatusSlot::Ticker(content) => {
            bar = bar.ticker(job_ticker::render(content));
        }
        job_ticker::StatusSlot::Idle => {}
    }
    let keys = workspace_keys();
    let shortcuts = Button::new("statusbar-shortcuts", "Shortcuts")
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact)
        .action(Box::new(fleet::OpenHelp));
    match model.place {
        Place::Hub => {
            bar = bar.trailing(shortcuts.harness_target("statusbar.shortcuts"));
        }
        Place::Terminal | Place::AgentThread => {
            if model.place == Place::Terminal {
                let mut commands = Button::new("statusbar-commands", "Fleet commands")
                    .style(ButtonStyle::Ghost)
                    .size(ButtonSize::Compact)
                    .action(Box::new(workspace::EnterPrefix));
                if let Some(kbd) = keys.prefix.clone() {
                    commands = commands.kbd(kbd);
                }
                bar = bar.trailing(commands.harness_target("statusbar.commands"));
            }
            let shortcuts = match keys.help.clone() {
                Some(kbd) => shortcuts.kbd(kbd),
                None => shortcuts,
            };
            bar = bar.trailing(shortcuts.harness_target("statusbar.shortcuts"));
        }
    }
    bar.into_any_element()
}

fn error_slot(error: &StickyError, screen: Option<&Screen>) -> AnyElement {
    let kbd = match screen {
        Some(Screen::Workspace { .. }) => workspace_keys().sticky_error.clone(),
        Some(Screen::Hub { .. }) | None => hub_sticky_error_key(),
    };
    sticky_error::render(error, kbd)
}

/// `zsh · 164×42 · kept alive by fleetd`: the active PTY, its size, and who owns it. A
/// Fleet-drawn tab has no process to describe, so it keeps the breadcrumb.
fn terminal_line(state: &AppState) -> Option<String> {
    let terminal = state.active_terminal_record()?;
    if terminal.is_native() {
        return None;
    }
    let name = terminal_label(terminal, state.renamed_terminals.contains(&terminal.id));
    Some(match state.grids.get(&terminal.id) {
        Some(grid) => format!(
            "{name} \u{b7} {}\u{d7}{} \u{b7} kept alive by fleetd",
            grid.cols, grid.rows
        ),
        None => format!("{name} \u{b7} kept alive by fleetd"),
    })
}

/// The status-bar breadcrumb `context › repo › row` (§2.2).
#[must_use]
fn breadcrumb_text(state: &AppState) -> String {
    let context = effective_context(state)
        .map(|context| context.name.as_str())
        .unwrap_or_default();
    let repo = match &state.scope {
        RepoScope::All => "",
        RepoScope::Repo(repo) => repo.name(),
    };
    // The board's row is derived here, not cached by its render: the bars are prepared before
    // the body renders, so a row written during the body's render names the previously selected
    // card.
    let row = if matches!(state.screen, Screen::Hub { tab: HubTab::Board }) {
        board_row(state)
    } else {
        state.breadcrumb_row.clone().unwrap_or_default()
    };
    breadcrumb(&[context, repo, &row])
}

/// The board's breadcrumb row: the focused card, unless the surface is not about a card.
///
/// Board settings edits the *board*, so naming a card there points the reader at the one thing
/// the dialog cannot change. The board's name would be no better — it defaults to the context's,
/// which the breadcrumb already says — so the row is simply dropped and `breadcrumb` suppresses
/// it.
fn board_row(state: &AppState) -> String {
    if matches!(state.overlay, Some(Overlay::Dialog(Dialogs::BoardSettings))) {
        return String::new();
    }
    crate::screens::board::selected_card(state)
        .map(|card| {
            state
                .board()
                .map_or_else(|| card.title.clone(), |view| card.display_key(&view.board))
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
