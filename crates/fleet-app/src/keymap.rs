//! The complete key table of `docs/KEYMAP.md`, expressed as gpui key bindings.
//!
//! # Contexts
//!
//! gpui resolves a binding against the *context stack* of the focused element, and a deeper
//! match beats a shallower one. [`crate::state::AppState::context_chain`] builds that stack,
//! and [`crate::shell`] renders one nested element per entry, so the predicates below read
//! exactly like the table in `docs/KEYMAP.md`:
//!
//! ```text
//! Fleet > Hub > Worktrees          the Hub with the worktrees list focused
//! Fleet > Workspace > Prefix       a terminal, one key after ctrl-s
//! Fleet > Dialog > Confirm         any confirm dialog
//! ```
//!
//! Because `Hub` is an ancestor of `Repos`, `Worktrees` and `Prs`, a binding on `Hub` is
//! inherited by all three panes and a binding on `Hub > Prs` overrides it.
//!
//! # Prefix mechanics
//!
//! `ctrl-s` is the Workspace's only app key: in `Workspace > Terminal` it fires
//! [`crate::actions::workspace::EnterPrefix`] and every other key falls through to the PTY,
//! because gpui dispatches bindings before `on_key_down` listeners. `Workspace > Prefix` is a
//! **one-shot** context: the shell leaves it on the next key whether or not that key matched a
//! binding, so no timeout is needed and no key can leak into the PTY. `ctrl-s ctrl-s` sends a
//! literal `ctrl-s`.
//!
//! # Deliberate deviations
//!
//! * `docs/KEYMAP.md` lists `q` as "close" in Palette mode. A `q` binding there would make the
//!   query untypable, because bindings outrank the text input. Only `Esc` closes the palette;
//!   see `docs/APP-CONTRACTS.md`.

use gpui::{Action, App, KeyBinding};

use crate::actions::fleet::Cancel;
use crate::actions::{
    confirm, context_dialog, create_worktree, daemon, dialog, filter, first_run,
    fleet::{
        FocusStickyError, OpenAgentClaude, OpenAgentOpencode, OpenHelp, OpenJobs, OpenPalette,
        OpenSettings, Quit, QuitAndStopDaemon, Refresh, UpdateFleet,
    },
    help, hub, jobs, palette, prefix, prs, quit_daemon_dialog, quit_dialog, repos, scroll,
    settings, workspace, worktrees,
};

/// One row of the key table, in the order it is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSpec {
    /// The keystrokes, space separated for a sequence (`"g g"`).
    pub keys: &'static str,
    /// The gpui key-context predicate this row is scoped to.
    pub context: &'static str,
    /// The fully qualified action name, e.g. `hub::MoveDown`.
    pub action: &'static str,
}

macro_rules! key_table {
    ($( $keys:literal, $context:literal => $action:expr ; )*) => {
        /// Every binding, ready for [`gpui::App::bind_keys`].
        ///
        /// # Panics
        ///
        /// Panics when a keystroke or context predicate in the table is malformed. The
        /// `key_table_is_well_formed` test builds the whole table, so a malformed row cannot
        /// reach a release build.
        #[must_use]
        pub fn bindings() -> Vec<KeyBinding> {
            vec![$( KeyBinding::new($keys, $action, Some($context)) ),*]
        }

        /// The same table as data: keystrokes, context and action name.
        ///
        /// The help overlay and the command palette render their key hints from this, so a
        /// binding and its documentation can never drift apart.
        #[must_use]
        pub fn table() -> Vec<BindingSpec> {
            vec![$( BindingSpec {
                keys: $keys,
                context: $context,
                action: Action::name(&$action),
            } ),*]
        }
    };
}

/// The root key context. Present on every screen, including the daemon surfaces.
pub const ROOT_CONTEXT: &str = "Fleet";

key_table! {
    // ---------------------------------------------------------------- global (§KEYMAP global)
    "ctrl-q",       "Fleet" => Quit;
    "ctrl-shift-q", "Fleet" => QuitAndStopDaemon;

    // ---------------------------------------------------------------- Hub, all panes
    "j",            "Hub" => hub::MoveDown;
    "down",         "Hub" => hub::MoveDown;
    "k",            "Hub" => hub::MoveUp;
    "up",           "Hub" => hub::MoveUp;
    "g g",          "Hub" => hub::GoTop;
    "G",            "Hub" => hub::GoBottom;
    "ctrl-d",       "Hub" => hub::HalfPageDown;
    "ctrl-u",       "Hub" => hub::HalfPageUp;
    "h",            "Hub" => hub::FocusPrevPane;
    "left",         "Hub" => hub::FocusPrevPane;
    "shift-tab",    "Hub" => hub::FocusPrevPane;
    "l",            "Hub" => hub::FocusNextPane;
    "right",        "Hub" => hub::FocusNextPane;
    "tab",          "Hub" => hub::FocusNextPane;
    "g r",          "Hub" => hub::GoRepos;
    "g w",          "Hub" => hub::GoWorktrees;
    "g p",          "Hub" => hub::GoPrs;
    "g j",          "Hub" => hub::GoJobs;
    "g a",          "Hub" => hub::GoAllRepos;
    "g t",          "Hub" => hub::NextContext;
    "g shift-t",    "Hub" => hub::PrevContext;
    "1",            "Hub" => hub::SelectContext1;
    "2",            "Hub" => hub::SelectContext2;
    "3",            "Hub" => hub::SelectContext3;
    "4",            "Hub" => hub::SelectContext4;
    "5",            "Hub" => hub::SelectContext5;
    "6",            "Hub" => hub::SelectContext6;
    "7",            "Hub" => hub::SelectContext7;
    "8",            "Hub" => hub::SelectContext8;
    "9",            "Hub" => hub::SelectContext9;
    "p",            "Hub" => hub::TogglePrScreen;
    "/",            "Hub" => hub::OpenFilter;
    ":",            "Hub" => OpenPalette;
    ",",            "Hub" => OpenSettings;
    "?",            "Hub" => OpenHelp;
    "J",            "Hub" => OpenJobs;
    "!",            "Hub" => FocusStickyError;
    "i",            "Hub" => hub::ToggleDetail;
    "H",            "Hub" => hub::ToggleRepoRail;
    "a",            "Hub" => OpenAgentClaude;
    "A",            "Hub" => OpenAgentOpencode;
    "r",            "Hub" => Refresh;
    "U",            "Hub" => UpdateFleet;
    "N",            "Hub" => hub::NewContext;
    "E",            "Hub" => hub::EditContext;
    "D",            "Hub" => hub::DeleteContext;
    "b",            "Hub" => hub::OpenInBrowser;
    "escape",       "Hub" => Cancel;

    // ---------------------------------------------------------------- Hub › Repos
    "enter",        "Hub > Repos" => repos::Open;
    "o",            "Hub > Repos" => repos::Open;
    "l",            "Hub > Repos" => repos::Open;
    "n",            "Hub > Repos" => repos::Clone;
    "d",            "Hub > Repos" => repos::Delete;
    "x",            "Hub > Repos" => repos::DismissClone;
    "e",            "Hub > Repos" => repos::EditHooks;
    "m",            "Hub > Repos" => repos::MoveToContext;

    // ---------------------------------------------------------------- Hub › Worktrees
    "enter",        "Hub > Worktrees" => worktrees::Open;
    "o",            "Hub > Worktrees" => worktrees::Open;
    "O",            "Hub > Worktrees" => worktrees::OpenKeepAwake;
    "n",            "Hub > Worktrees" => worktrees::Create;
    "d",            "Hub > Worktrees" => worktrees::Delete;
    "u",            "Hub > Worktrees" => worktrees::UndoDelete;
    "x",            "Hub > Worktrees" => worktrees::Prune;
    "s",            "Hub > Worktrees" => worktrees::Sleep;
    "K",            "Hub > Worktrees" => worktrees::Kill;
    "I",            "Hub > Worktrees" => worktrees::Inspect;
    "y",            "Hub > Worktrees" => worktrees::CopyPath;
    "Y",            "Hub > Worktrees" => worktrees::CopyBranch;

    // ---------------------------------------------------------------- Hub › Pull requests
    "tab",          "Hub > Prs" => prs::NextTab;
    "l",            "Hub > Prs" => prs::NextTab;
    "shift-tab",    "Hub > Prs" => prs::PrevTab;
    "h",            "Hub > Prs" => prs::PrevTab;
    "enter",        "Hub > Prs" => prs::Open;
    "o",            "Hub > Prs" => prs::Open;
    "O",            "Hub > Prs" => prs::OpenKeepAwake;
    "c",            "Hub > Prs" => prs::CreateWithoutOpening;
    "I",            "Hub > Prs" => prs::Inspect;
    "y",            "Hub > Prs" => prs::CopyUrl;
    "r",            "Hub > Prs" => prs::Refresh;
    "p",            "Hub > Prs" => prs::Back;
    "q",            "Hub > Prs" => prs::Back;

    // ---------------------------------------------------------------- Workspace › Terminal
    "ctrl-s",       "Workspace > Terminal" => workspace::EnterPrefix;

    // ---------------------------------------------------------------- Workspace › Prefix
    "ctrl-s",       "Workspace > Prefix" => prefix::SendLiteral;
    "s",            "Workspace > Prefix" => prefix::GoHub;
    "S",            "Workspace > Prefix" => prefix::SleepAndGoHub;
    "1",            "Workspace > Prefix" => prefix::SelectTab1;
    "2",            "Workspace > Prefix" => prefix::SelectTab2;
    "3",            "Workspace > Prefix" => prefix::SelectTab3;
    "4",            "Workspace > Prefix" => prefix::SelectTab4;
    "5",            "Workspace > Prefix" => prefix::SelectTab5;
    "6",            "Workspace > Prefix" => prefix::SelectTab6;
    "7",            "Workspace > Prefix" => prefix::SelectTab7;
    "8",            "Workspace > Prefix" => prefix::SelectTab8;
    "9",            "Workspace > Prefix" => prefix::SelectTab9;
    "h",            "Workspace > Prefix" => prefix::PrevTab;
    "p",            "Workspace > Prefix" => prefix::PrevTab;
    "l",            "Workspace > Prefix" => prefix::NextTab;
    "n",            "Workspace > Prefix" => prefix::NextTab;
    "tab",          "Workspace > Prefix" => prefix::LastTab;
    "w",            "Workspace > Prefix" => prefix::LastSession;
    "W",            "Workspace > Prefix" => prefix::SessionSwitcher;
    "c",            "Workspace > Prefix" => prefix::NewTerminal;
    "x",            "Workspace > Prefix" => prefix::CloseTerminal;
    "r",            "Workspace > Prefix" => prefix::RestartCommand;
    "y",            "Workspace > Prefix" => prefix::CopyWorktreePath;
    ",",            "Workspace > Prefix" => prefix::RenameTerminal;
    "[",            "Workspace > Prefix" => prefix::EnterScroll;
    "]",            "Workspace > Prefix" => prefix::Paste;
    "a",            "Workspace > Prefix" => OpenAgentClaude;
    "A",            "Workspace > Prefix" => OpenAgentOpencode;
    "z",            "Workspace > Prefix" => prefix::ToggleZoom;
    "!",            "Workspace > Prefix" => FocusStickyError;
    "J",            "Workspace > Prefix" => OpenJobs;
    "?",            "Workspace > Prefix" => OpenHelp;
    "escape",       "Workspace > Prefix" => prefix::Cancel;

    // ---------------------------------------------------------------- Workspace › Scroll
    "j",            "Workspace > Scroll" => scroll::LineDown;
    "k",            "Workspace > Scroll" => scroll::LineUp;
    "ctrl-d",       "Workspace > Scroll" => scroll::HalfPageDown;
    "ctrl-u",       "Workspace > Scroll" => scroll::HalfPageUp;
    "ctrl-f",       "Workspace > Scroll" => scroll::PageDown;
    "ctrl-b",       "Workspace > Scroll" => scroll::PageUp;
    "g g",          "Workspace > Scroll" => scroll::Top;
    "G",            "Workspace > Scroll" => scroll::Bottom;
    "v",            "Workspace > Scroll" => scroll::StartSelection;
    "y",            "Workspace > Scroll" => scroll::Yank;
    "/",            "Workspace > Scroll" => scroll::Search;
    "n",            "Workspace > Scroll" => scroll::SearchNext;
    "N",            "Workspace > Scroll" => scroll::SearchPrev;
    "q",            "Workspace > Scroll" => scroll::Exit;
    "i",            "Workspace > Scroll" => scroll::Exit;
    "escape",       "Workspace > Scroll" => scroll::Escape;

    // ---------------------------------------------------------------- Filter
    "enter",        "Filter" => filter::Accept;
    "escape",       "Filter" => filter::Escape;
    "ctrl-n",       "Filter" => filter::CursorDown;
    "down",         "Filter" => filter::CursorDown;
    "ctrl-p",       "Filter" => filter::CursorUp;
    "up",           "Filter" => filter::CursorUp;
    "backspace",    "Filter" => filter::Backspace;
    "ctrl-w",       "Filter" => filter::DeleteWord;
    "ctrl-u",       "Filter" => filter::Clear;

    // ---------------------------------------------------------------- Palette
    "enter",        "Palette" => palette::Run;
    "escape",       "Palette" => palette::Close;
    "ctrl-n",       "Palette" => palette::CursorDown;
    "down",         "Palette" => palette::CursorDown;
    "ctrl-p",       "Palette" => palette::CursorUp;
    "up",           "Palette" => palette::CursorUp;
    "backspace",    "Palette" => palette::Backspace;
    "ctrl-w",       "Palette" => palette::DeleteWord;
    "ctrl-u",       "Palette" => palette::Clear;

    // ---------------------------------------------------------------- Jobs panel
    "J",            "Jobs" => jobs::Close;
    "escape",       "Jobs" => jobs::Close;
    "q",            "Jobs" => jobs::Close;
    "j",            "Jobs" => jobs::MoveDown;
    "down",         "Jobs" => jobs::MoveDown;
    "k",            "Jobs" => jobs::MoveUp;
    "up",           "Jobs" => jobs::MoveUp;
    "g g",          "Jobs" => jobs::Top;
    "G",            "Jobs" => jobs::Bottom;
    "enter",        "Jobs" => jobs::ToggleLog;
    "c",            "Jobs" => jobs::CancelJob;
    "X",            "Jobs" => jobs::CancelAll;
    "R",            "Jobs" => jobs::Retry;
    "y",            "Jobs" => jobs::CopyLogPath;
    "D",            "Jobs" => jobs::DismissFinished;
    "f",            "Jobs" => jobs::CycleFilter;
    "escape",       "Jobs > Log" => jobs::CollapseLog;

    // ---------------------------------------------------------------- Dialogs, shared frame
    "enter",        "Dialog" => dialog::Confirm;
    "escape",       "Dialog" => dialog::Cancel;
    "tab",          "Dialog" => dialog::NextField;
    "shift-tab",    "Dialog" => dialog::PrevField;
    "ctrl-n",       "Dialog" => dialog::CursorDown;
    "down",         "Dialog" => dialog::CursorDown;
    "ctrl-p",       "Dialog" => dialog::CursorUp;
    "up",           "Dialog" => dialog::CursorUp;
    "backspace",    "Dialog" => dialog::Backspace;
    "ctrl-w",       "Dialog" => dialog::DeleteWord;
    "ctrl-u",       "Dialog" => dialog::ClearInput;
    "ctrl-a",       "Dialog" => dialog::LineStart;
    "ctrl-e",       "Dialog" => dialog::LineEnd;
    "left",         "Dialog" => dialog::CursorLeft;
    "right",        "Dialog" => dialog::CursorRight;

    // ---------------------------------------------------------------- Dialog › Create worktree
    "left",         "Dialog > Create" => create_worktree::HostPrev;
    "right",        "Dialog > Create" => create_worktree::HostNext;
    "alt-enter",    "Dialog > Create" => create_worktree::CreateWithoutOpening;

    // ---------------------------------------------------------------- Dialog › Confirm
    "y",            "Dialog > Confirm" => confirm::Accept;
    "enter",        "Dialog > Confirm" => confirm::Accept;
    "Y",            "Dialog > Confirm" => confirm::AcceptStrong;
    "n",            "Dialog > Confirm" => confirm::Reject;
    "q",            "Dialog > Confirm" => confirm::Reject;
    "escape",       "Dialog > Confirm" => confirm::Reject;
    "I",            "Dialog > Confirm" => confirm::Recheck;
    "s",            "Dialog > Confirm" => confirm::ToggleKeep;

    // ---------------------------------------------------------------- Dialog › New / Edit context
    "ctrl-d",       "Dialog > Context" => context_dialog::Delete;

    // ---------------------------------------------------------------- Dialog › Assign repo
    "j",            "Dialog > Assign" => dialog::CursorDown;
    "k",            "Dialog > Assign" => dialog::CursorUp;

    // ---------------------------------------------------------------- Dialog › Settings
    "space",        "Dialog > Settings" => settings::Toggle;
    "h",            "Dialog > Settings" => settings::CyclePrev;
    "l",            "Dialog > Settings" => settings::CycleNext;
    // `left` / `right` stay on the shared `dialog::CursorLeft` / `CursorRight` of the `Dialog`
    // context: the settings dialog moves the caret when a text row has the keyboard and cycles
    // the choice otherwise, so one binding serves both halves of §3.8.6.
    "j",            "Dialog > Settings" => settings::MoveDown;
    "k",            "Dialog > Settings" => settings::MoveUp;
    "E",            "Dialog > Settings" => settings::OpenConfigFile;
    "D",            "Dialog > Settings" => settings::RunDoctor;

    // ---------------------------------------------------------------- Dialog › Help
    "escape",       "Dialog > Help" => help::Close;
    "?",            "Dialog > Help" => help::Close;

    // ---------------------------------------------------------------- Dialog › Quit
    "y",            "Dialog > Quit" => quit_dialog::Accept;
    "n",            "Dialog > Quit" => quit_dialog::Reject;
    "escape",       "Dialog > Quit" => quit_dialog::Reject;
    "J",            "Dialog > Quit" => quit_dialog::OpenJobs;
    "W",            "Dialog > Quit" => quit_dialog::NeverWarn;

    // ---------------------------------------------------------------- Dialog › Quit and stop daemon
    "Y",            "Dialog > QuitDaemon" => quit_daemon_dialog::Accept;
    "n",            "Dialog > QuitDaemon" => quit_daemon_dialog::Reject;
    "escape",       "Dialog > QuitDaemon" => quit_daemon_dialog::Reject;

    // ---------------------------------------------------------------- Daemon › Down (§3.12 B)
    "r",            "Daemon > Down" => daemon::Retry;
    "L",            "Daemon > Down" => daemon::OpenLog;
    "D",            "Daemon > Down" => daemon::RunDoctor;
    "D",            "Daemon > Doctor" => daemon::RunDoctor;
    "L",            "Daemon > Doctor" => daemon::OpenLog;
    "escape",       "Daemon > Doctor" => daemon::DismissBanner;

    // ---------------------------------------------------------------- Daemon › Banner (§3.12 C)
    "r",            "Daemon > Banner" => daemon::Reconnect;
    "l",            "Daemon > Banner" => daemon::OpenLog;
    "escape",       "Daemon > Banner" => daemon::DismissBanner;

    // ---------------------------------------------------------------- First run
    "i",            "FirstRun" => first_run::Import;
    "N",            "FirstRun" => hub::NewContext;
    "n",            "FirstRun" => repos::Clone;
    "?",            "FirstRun" => OpenHelp;
    ",",            "FirstRun" => OpenSettings;
}

/// Registers the whole table with the app.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Every key context named in `docs/KEYMAP.md`.
    const REQUIRED_CONTEXTS: &[&str] = &[
        "Fleet",
        "Hub",
        "Hub > Repos",
        "Hub > Worktrees",
        "Hub > Prs",
        "Workspace > Terminal",
        "Workspace > Prefix",
        "Workspace > Scroll",
        "Filter",
        "Palette",
        "Jobs",
        "Dialog",
        "Dialog > Create",
        "Dialog > Confirm",
        "Dialog > Context",
        "Dialog > Assign",
        "Dialog > Settings",
        "Dialog > Help",
        "Dialog > Quit",
        "Dialog > QuitDaemon",
        "Daemon > Down",
        "Daemon > Banner",
        "FirstRun",
    ];

    #[test]
    fn key_table_is_well_formed() {
        let bindings = bindings();
        assert_eq!(bindings.len(), table().len());
        assert!(
            bindings.len() > 200,
            "the table lost rows: {}",
            bindings.len()
        );
    }

    #[test]
    fn every_documented_context_is_bound() {
        let table = table();
        for context in REQUIRED_CONTEXTS {
            assert!(
                table.iter().any(|spec| spec.context == *context),
                "no binding for key context `{context}`"
            );
        }
    }

    #[test]
    fn no_context_binds_one_keystroke_twice() {
        let mut seen: HashMap<(&str, &str), &str> = HashMap::new();
        for spec in table() {
            let previous = seen.insert((spec.context, spec.keys), spec.action);
            assert_eq!(
                previous, None,
                "`{}` is bound twice in `{}`",
                spec.keys, spec.context
            );
        }
    }

    #[test]
    fn g_prefix_sequences_are_complete() {
        let table = table();
        for keys in ["g g", "g t", "g shift-t", "g r", "g w", "g p", "g j", "g a"] {
            assert!(
                table.iter().any(|spec| spec.keys == keys),
                "missing the `{keys}` sequence"
            );
        }
    }

    #[test]
    fn prefix_is_the_only_app_key_over_a_terminal() {
        let terminal: Vec<_> = table()
            .into_iter()
            .filter(|spec| spec.context == "Workspace > Terminal")
            .collect();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].keys, "ctrl-s");
    }

    #[test]
    fn prefix_sends_a_literal_control_s() {
        assert!(table().iter().any(|spec| {
            spec.context == "Workspace > Prefix"
                && spec.keys == "ctrl-s"
                && spec.action == "prefix::SendLiteral"
        }));
    }

    #[test]
    fn quitting_is_global_and_escape_never_quits() {
        let table = table();
        assert!(
            table
                .iter()
                .any(|spec| spec.keys == "ctrl-q" && spec.context == ROOT_CONTEXT)
        );
        assert!(
            table
                .iter()
                .any(|spec| spec.keys == "ctrl-shift-q" && spec.context == ROOT_CONTEXT)
        );
        assert!(
            !table
                .iter()
                .any(|spec| spec.keys == "escape" && spec.action.ends_with("Quit")),
            "Esc must never be bound to a quit action"
        );
        assert!(
            !table
                .iter()
                .any(|spec| spec.keys == "q" && spec.context == "Hub"),
            "`q` is unbound in the Hub lists"
        );
    }

    #[test]
    fn palette_and_filter_never_bind_printable_keys() {
        for spec in table() {
            if spec.context == "Palette" || spec.context == "Filter" {
                assert!(
                    spec.keys.len() > 1,
                    "`{}` in `{}` would shadow typing",
                    spec.keys,
                    spec.context
                );
            }
        }
    }
}
