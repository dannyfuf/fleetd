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
//! `Workspace > Terminal` reserves `ctrl-s` plus the standard macOS clipboard keys. Every other
//! key falls through to the PTY because gpui dispatches bindings before `on_key_down` listeners.
//! `Workspace > Prefix` is a
//! **one-shot** context: the shell leaves it on the next key whether or not that key matched a
//! binding, so no timeout is needed and no key can leak into the PTY. `ctrl-s ctrl-s` sends a
//! literal `ctrl-s`.
//!
//! # Deliberate deviations
//!
//! * `docs/KEYMAP.md` lists `q` as "close" in Palette mode. A `q` binding there would make the
//!   query untypable, because bindings outrank the text input. Only `Esc` closes the palette;
//!   see `docs/APP-CONTRACTS.md`.

use gpui::{Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Keystroke};

use crate::actions::fleet::Cancel;
use crate::actions::{
    agent, board, card_detail, confirm, context_dialog, create_worktree, daemon, dialog, filter,
    first_run,
    fleet::{
        FocusStickyError, OpenAgentClaude, OpenAgentCodex, OpenHelp, OpenJobs, OpenPalette,
        OpenSettings, Quit, QuitAndStopDaemon, Refresh, UpdateFleet,
    },
    help, hub, jobs, native_agent, palette, prefix, prs, quit_daemon_dialog, quit_dialog, repos,
    scroll, settings, workspace, worktrees,
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
        thread_local! {
            static PARSED_BINDINGS: Vec<(BindingSpec, KeyBinding)> = {
                let mut bindings = Vec::new();
                $(
                    let spec = BindingSpec { keys: $keys, context: $context, action: Action::name(&$action) };
                    match parse_binding(spec, Action::boxed_clone(&$action)) {
                        Ok(binding) => bindings.push((spec, binding)),
                        Err(error) => tracing::error!(keys = $keys, context = $context, %error, "invalid built-in key binding"),
                    }
                )*
                bindings
            };
        }

        /// Validated bindings, parsed once on the UI thread.
        #[must_use]
        pub fn bindings() -> Vec<KeyBinding> {
            PARSED_BINDINGS.with(|bindings| bindings.iter().map(|(_, binding)| binding.clone()).collect())
        }

        /// The same table as data: keystrokes, context and action name.
        ///
        /// The help overlay and the command palette render their key hints from this, so a
        /// binding and its documentation can never drift apart.
        #[must_use]
        pub fn table() -> Vec<BindingSpec> {
            cached_table().to_vec()
        }

        fn cached_table() -> &'static [BindingSpec] {
            static TABLE: std::sync::OnceLock<Vec<BindingSpec>> = std::sync::OnceLock::new();
            TABLE.get_or_init(|| vec![$( BindingSpec {
                keys: $keys,
                context: $context,
                action: Action::name(&$action),
            } ),*])
        }

        /// Resolves one keystroke against one exact key context.
        ///
        /// This is used by the Workspace's live prefix interceptor. GPUI's rendered context
        /// tree is one frame behind a state change, so the second key of a fast `ctrl-s s`
        /// cannot safely wait for `Workspace > Prefix` to be painted. Generating the resolver
        /// from this macro keeps that fallback on the same table as ordinary key dispatch.
        #[must_use]
        pub fn action_for_keystroke(
            context: &str,
            keystroke: &Keystroke,
        ) -> Option<Box<dyn Action>> {
            PARSED_BINDINGS.with(|bindings| {
                bindings.iter().find_map(|(spec, binding)| {
                    if spec.context == context && binding.keystrokes().len() == 1
                        && keystroke.should_match(&binding.keystrokes()[0])
                    {
                        Some(binding.action().boxed_clone())
                    } else {
                        None
                    }
                })
            })
        }
    };
}

fn parse_binding(spec: BindingSpec, action: Box<dyn Action>) -> Result<KeyBinding, String> {
    let predicate =
        KeyBindingContextPredicate::parse(spec.context).map_err(|error| error.to_string())?;
    KeyBinding::load(
        spec.keys,
        action,
        Some(predicate.into()),
        false,
        None,
        &DummyKeyboardMapper,
    )
    .map_err(|error| error.to_string())
}

/// The root key context. Present on every screen, including the daemon surfaces.
pub const ROOT_CONTEXT: &str = "Fleet";

key_table! {
    "g b", "Hub" => board::GoBoard;
    "h", "Hub > Board" => board::PrevColumn;
    "left", "Hub > Board" => board::PrevColumn;
    "l", "Hub > Board" => board::NextColumn;
    "right", "Hub > Board" => board::NextColumn;
    "j", "Hub > Board" => board::NextCard;
    "down", "Hub > Board" => board::NextCard;
    "k", "Hub > Board" => board::PrevCard;
    "up", "Hub > Board" => board::PrevCard;
    "enter", "Hub > Board" => board::OpenCard;
    "c", "Hub > Board" => board::NewCard;
    "s", "Hub > Board" => board::PickStatus;
    "p", "Hub > Board" => board::PickPriority;
    "a", "Hub > Board" => board::PickAssignee;
    "t", "Hub > Board" => board::PickLabels;
    "e", "Hub > Board" => board::PickEstimate;
    "[", "Hub > Board" => board::MovePrevColumn;
    "]", "Hub > Board" => board::MoveNextColumn;
    "w", "Hub > Board" => board::CreateWorktree;
    "o", "Hub > Board" => board::OpenWorktree;
    "S", "Hub > Board" => board::Sync;
    "F", "Hub > Board" => board::FullSync;
    "x", "Hub > Board" => board::OpenRemote;
    "d", "Hub > Board" => board::DeleteCard;
    ",", "Hub > Board" => board::Settings;
    "r", "Hub > Board" => board::Reload;
    "/", "Hub > Board" => board::Filter;
    "escape", "Dialog > CardDetail" => card_detail::Close;
    "i", "Dialog > CardDetail" => card_detail::EditTitle;
    "d", "Dialog > CardDetail" => card_detail::EditDescription;
    "c", "Dialog > CardDetail" => card_detail::AddComment;
    "j", "Dialog > CardDetail" => card_detail::NextProperty;
    "k", "Dialog > CardDetail" => card_detail::PrevProperty;
    "enter", "Dialog > CardDetail" => card_detail::EditProperty;
    "w", "Dialog > CardDetail" => card_detail::CreateWorktree;
    "x", "Dialog > CardDetail" => card_detail::OpenRemote;
    "K", "Dialog > CardDetail" => card_detail::KeepLocal;
    "R", "Dialog > CardDetail" => card_detail::TakeRemote;
    "ctrl-s", "Dialog > CardDetail" => card_detail::Save;
    // The palette replaces the dialog it is opened over and remembers which one it was, so the
    // `Card detail:` rows can save or cancel an edit already typed instead of reseeding one over
    // it. Without a way in from the detail those rows can never be listed and that path is dead.
    ":",      "Dialog > CardDetail" => OpenPalette;
    "ctrl-enter", "Dialog > CardCreate" => board::CreateAndOpen;
    "space", "Dialog > CardPicker" => settings::Toggle;
    "j", "Dialog > BoardSettings" => settings::MoveDown;
    "k", "Dialog > BoardSettings" => settings::MoveUp;
    "h", "Dialog > BoardSettings" => settings::CyclePrev;
    "l", "Dialog > BoardSettings" => settings::CycleNext;
    "space", "Dialog > BoardSettings" => settings::Toggle;

    "ctrl-q",       "Fleet" => Quit;
    "ctrl-shift-q", "Fleet" => QuitAndStopDaemon;

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
    "A",            "Hub" => OpenAgentCodex;
    "r",            "Hub" => Refresh;
    "U",            "Hub" => UpdateFleet;
    "N",            "Hub" => hub::NewContext;
    "E",            "Hub" => hub::EditContext;
    "D",            "Hub" => hub::DeleteContext;
    "b",            "Hub" => hub::OpenInBrowser;
    "escape",       "Hub" => Cancel;

    "enter",        "Hub > Repos" => repos::Open;
    "o",            "Hub > Repos" => repos::Open;
    "l",            "Hub > Repos" => repos::Open;
    "n",            "Hub > Repos" => repos::Clone;
    "d",            "Hub > Repos" => repos::Delete;
    "x",            "Hub > Repos" => repos::DismissClone;
    "e",            "Hub > Repos" => repos::EditHooks;
    "m",            "Hub > Repos" => repos::MoveToContext;

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

    "ctrl-s",       "Workspace > Terminal" => workspace::EnterPrefix;
    "cmd-c",        "Workspace > Terminal" => workspace::CopySelection;
    "cmd-v",        "Workspace > Terminal" => workspace::PasteClipboard;

    // A `fleet://` tab is a terminal as far as this table is concerned: exactly one app key,
    // and every other keystroke belongs to whatever is inside the tab. The consumer is a gpui
    // view rather than a PTY, so the keys fall through to *its* bindings — which live under
    // its own root context, nested inside this one — instead of through `on_key_down`.
    "ctrl-s",       "Workspace > Native" => workspace::EnterPrefix;

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
    // §10 phase 6: `^s a` / `^s A` default to a native thread; the PTY popup stays reachable
    // as an explicit fallback on `^s F`, and `a` / `A` on the Hub still open the popup.
    "a",            "Workspace > Prefix" => native_agent::NewClaude;
    "A",            "Workspace > Prefix" => native_agent::NewCodex;
    "F",            "Workspace > Prefix" => native_agent::TerminalFallback;
    "z",            "Workspace > Prefix" => prefix::ToggleZoom;
    "v",            "Workspace > Prefix" => prefix::ToggleWatchPane;
    "V",            "Workspace > Prefix" => prefix::DismissWatch;
    "N",            "Workspace > Prefix" => prefix::NextWatch;
    "P",            "Workspace > Prefix" => prefix::PrevWatch;
    "!",            "Workspace > Prefix" => FocusStickyError;
    "J",            "Workspace > Prefix" => OpenJobs;
    "?",            "Workspace > Prefix" => OpenHelp;
    "escape",       "Workspace > Prefix" => prefix::Cancel;

    "shift-pageup",   "Workspace > Terminal" => scroll::TerminalPageUp;
    "shift-pagedown", "Workspace > Terminal" => scroll::TerminalPageDown;
    "cmd-home",       "Workspace > Terminal" => scroll::TerminalTop;
    "cmd-end",        "Workspace > Terminal" => scroll::TerminalBottom;

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

    // Agent is the persistent popup context; its Terminal / Prefix / Scroll children mirror
    // Workspace terminal mechanics without changing the Workspace underneath.
    //
    // `agent::Hide` is bound on the popup's own children, never on `Agent` itself: gpui matches
    // `>` as a subsequence, so an `Agent` binding also matches `Fleet > Agent > AgentIdle` — a
    // native tab, where the popup is not mounted and no handler exists. It shadowed the global
    // `ctrl-q → Quit` there and the key did nothing at all (APP-CONTRACTS §3 scopes the gesture
    // to the popup).
    "ctrl-q",       "Agent > Terminal" => agent::Hide;

    "ctrl-s",       "Agent > Terminal" => agent::EnterPrefix;
    "cmd-c",        "Agent > Terminal" => agent::CopySelection;
    "cmd-v",        "Agent > Terminal" => agent::PasteClipboard;

    "ctrl-s",       "Agent > Prefix" => prefix::SendLiteral;
    "ctrl-q",       "Agent > Prefix" => agent::Hide;
    "ctrl-shift-q", "Agent > Prefix" => QuitAndStopDaemon;
    "cmd-c",        "Agent > Prefix" => agent::CopySelection;
    "cmd-v",        "Agent > Prefix" => agent::PasteClipboard;
    "q",            "Agent > Prefix" => agent::Hide;
    "a",            "Agent > Prefix" => OpenAgentClaude;
    "A",            "Agent > Prefix" => OpenAgentCodex;
    "[",            "Agent > Prefix" => prefix::EnterScroll;
    "]",            "Agent > Prefix" => prefix::Paste;
    "r",            "Agent > Prefix" => prefix::RestartCommand;
    "?",            "Agent > Prefix" => OpenHelp;
    "escape",       "Agent > Prefix" => prefix::Cancel;

    "ctrl-s",       "Agent > Scroll" => agent::EnterPrefix;
    "cmd-c",        "Agent > Scroll" => agent::CopySelection;
    "cmd-v",        "Agent > Scroll" => agent::PasteClipboard;
    "j",            "Agent > Scroll" => scroll::LineDown;
    "k",            "Agent > Scroll" => scroll::LineUp;
    "ctrl-d",       "Agent > Scroll" => scroll::HalfPageDown;
    "ctrl-u",       "Agent > Scroll" => scroll::HalfPageUp;
    "ctrl-f",       "Agent > Scroll" => scroll::PageDown;
    "ctrl-b",       "Agent > Scroll" => scroll::PageUp;
    "g g",          "Agent > Scroll" => scroll::Top;
    "G",            "Agent > Scroll" => scroll::Bottom;
    "v",            "Agent > Scroll" => scroll::StartSelection;
    "y",            "Agent > Scroll" => scroll::Yank;
    "/",            "Agent > Scroll" => scroll::Search;
    "n",            "Agent > Scroll" => scroll::SearchNext;
    "N",            "Agent > Scroll" => scroll::SearchPrev;
    "q",            "Agent > Scroll" => scroll::Exit;
    "i",            "Agent > Scroll" => scroll::Exit;
    "escape",       "Agent > Scroll" => scroll::Escape;

    // The native structured agent contexts coexist with the legacy Terminal/Prefix/Scroll popup
    // during migration. `docs/NATIVE-AGENTS.md` §12 is the authoritative table and
    // `docs/KEYMAP.md` § *Native agent thread* mirrors it.
    //
    // Three rules come with it. **gpui matches `>` as a subsequence, not as a parent test**, so
    // the `^s` escape rows have to be repeated on every `AgentDecision > *` context —
    // `AgentIdle`/`AgentWorking` are not on that chain. **Row focus lives inside scroll mode**,
    // which is why `AgentRow` is only ever entered under `AgentNativeScroll`. And `/`, `@`, `$`,
    // `⇧⏎` and `esc`-on-idle are deliberately **unbound**: the composer inserts the character
    // and reports it, which is what keeps all three triggers typable and lets an IME preedit and
    // a selection cancel before the `esc` cascade runs.
    "enter",         "Agent > AgentIdle" => native_agent::Send;
    "cmd-enter",     "Agent > AgentIdle" => native_agent::SendBackground;
    "shift-tab",     "Agent > AgentIdle" => native_agent::PlanMode;
    // DESIGN-SYSTEM §4: "a list under a text field moves with `ctrl-n`/`ctrl-p` or `↓`/`↑`".
    // With no picker open both keys are handed straight back to the composer's own caret
    // motion, so binding them costs a draft nothing.
    "up",            "Agent > AgentIdle" => native_agent::History;
    "ctrl-p",        "Agent > AgentIdle" => native_agent::History;
    "down",          "Agent > AgentIdle" => native_agent::HistoryNext;
    "ctrl-n",        "Agent > AgentIdle" => native_agent::HistoryNext;
    "ctrl-s m",      "Agent > AgentIdle" => native_agent::Model;
    "ctrl-s e",      "Agent > AgentIdle" => native_agent::Traits;
    "ctrl-s t",      "Agent > AgentIdle" => native_agent::AccessMode;
    "ctrl-s [",      "Agent > AgentIdle" => native_agent::Scroll;
    "ctrl-s a",      "Agent > AgentIdle" => native_agent::NewClaude;
    "ctrl-s A",      "Agent > AgentIdle" => native_agent::NewCodex;
    "ctrl-s x",      "Agent > AgentIdle" => native_agent::CloseTab;
    "ctrl-s F",      "Agent > AgentIdle" => native_agent::TerminalFallback;

    "escape",        "Agent > AgentWorking" => native_agent::Stop;
    "enter",         "Agent > AgentWorking" => native_agent::Steer;
    "cmd-enter",     "Agent > AgentWorking" => native_agent::Steer;
    "shift-tab",     "Agent > AgentWorking" => native_agent::PlanMode;
    "up",            "Agent > AgentWorking" => native_agent::History;
    "ctrl-p",        "Agent > AgentWorking" => native_agent::History;
    "down",          "Agent > AgentWorking" => native_agent::HistoryNext;
    "ctrl-n",        "Agent > AgentWorking" => native_agent::HistoryNext;
    "ctrl-s m",      "Agent > AgentWorking" => native_agent::Model;
    "ctrl-s e",      "Agent > AgentWorking" => native_agent::Traits;
    "ctrl-s t",      "Agent > AgentWorking" => native_agent::AccessMode;
    "ctrl-s [",      "Agent > AgentWorking" => native_agent::Scroll;
    "ctrl-s a",      "Agent > AgentWorking" => native_agent::NewClaude;
    "ctrl-s A",      "Agent > AgentWorking" => native_agent::NewCodex;
    "ctrl-s x",      "Agent > AgentWorking" => native_agent::CloseTab;
    "ctrl-s F",      "Agent > AgentWorking" => native_agent::TerminalFallback;

    // §12: `^s [` is a real mode — the transcript takes the same vocabulary the terminal scroll
    // mode has for as long as it is on, and `G` is "newest", not a way out of the mode.
    "j",             "Agent > AgentNativeScroll" => native_agent::ScrollLineDown;
    "k",             "Agent > AgentNativeScroll" => native_agent::ScrollLineUp;
    "ctrl-d",        "Agent > AgentNativeScroll" => native_agent::ScrollHalfPageDown;
    "ctrl-u",        "Agent > AgentNativeScroll" => native_agent::ScrollHalfPageUp;
    "ctrl-f",        "Agent > AgentNativeScroll" => native_agent::ScrollPageDown;
    "ctrl-b",        "Agent > AgentNativeScroll" => native_agent::ScrollPageUp;
    "g g",           "Agent > AgentNativeScroll" => native_agent::ScrollTop;
    "G",             "Agent > AgentNativeScroll" => native_agent::ScrollBottom;
    "q",             "Agent > AgentNativeScroll" => native_agent::ScrollExit;
    "i",             "Agent > AgentNativeScroll" => native_agent::ScrollExit;
    "escape",        "Agent > AgentNativeScroll" => native_agent::Stop;
    "ctrl-s [",      "Agent > AgentNativeScroll" => native_agent::Scroll;
    "ctrl-s x",      "Agent > AgentNativeScroll" => native_agent::CloseTab;
    "ctrl-s F",      "Agent > AgentNativeScroll" => native_agent::TerminalFallback;

    // Row focus: entered only under `AgentNativeScroll`, which is what finally makes these five
    // fire and retires the long-standing caveat that `Agent > AgentRow` was bound, handled and
    // never entered.
    "enter",         "Agent > AgentNativeScroll > AgentRow" => native_agent::ExpandRow;
    "u",             "Agent > AgentNativeScroll > AgentRow" => native_agent::Revert;
    "o",             "Agent > AgentNativeScroll > AgentRow" => native_agent::OpenInEditor;
    "y",             "Agent > AgentNativeScroll > AgentRow" => native_agent::CopyRow;
    "d",             "Agent > AgentNativeScroll > AgentRow" => native_agent::DiffRow;

    // §1 keeps the terminal path "as an explicit fallback, now on `^s F`" and §3.3 rule 4 lets
    // an unanswered gate outlive its turn: without these rows the escape hatches and every
    // control are dead keys for as long as a decision is open. gpui's subsequence match in the
    // other direction is why they cannot be inherited from `AgentIdle`.
    "ctrl-s [",      "Agent > AgentDecision > AgentPermission" => native_agent::Scroll;
    "ctrl-s x",      "Agent > AgentDecision > AgentPermission" => native_agent::CloseTab;
    "ctrl-s F",      "Agent > AgentDecision > AgentPermission" => native_agent::TerminalFallback;
    "ctrl-s m",      "Agent > AgentDecision > AgentPermission" => native_agent::Model;
    "ctrl-s e",      "Agent > AgentDecision > AgentPermission" => native_agent::Traits;
    "ctrl-s t",      "Agent > AgentDecision > AgentPermission" => native_agent::AccessMode;
    "ctrl-s a",      "Agent > AgentDecision > AgentPermission" => native_agent::NewClaude;
    "ctrl-s A",      "Agent > AgentDecision > AgentPermission" => native_agent::NewCodex;
    "ctrl-s [",      "Agent > AgentDecision > AgentQuestion" => native_agent::Scroll;
    "ctrl-s x",      "Agent > AgentDecision > AgentQuestion" => native_agent::CloseTab;
    "ctrl-s F",      "Agent > AgentDecision > AgentQuestion" => native_agent::TerminalFallback;
    "ctrl-s m",      "Agent > AgentDecision > AgentQuestion" => native_agent::Model;
    "ctrl-s e",      "Agent > AgentDecision > AgentQuestion" => native_agent::Traits;
    "ctrl-s t",      "Agent > AgentDecision > AgentQuestion" => native_agent::AccessMode;
    "ctrl-s a",      "Agent > AgentDecision > AgentQuestion" => native_agent::NewClaude;
    "ctrl-s A",      "Agent > AgentDecision > AgentQuestion" => native_agent::NewCodex;
    "ctrl-s [",      "Agent > AgentDecision > AgentPlan" => native_agent::Scroll;
    "ctrl-s x",      "Agent > AgentDecision > AgentPlan" => native_agent::CloseTab;
    "ctrl-s F",      "Agent > AgentDecision > AgentPlan" => native_agent::TerminalFallback;
    "ctrl-s m",      "Agent > AgentDecision > AgentPlan" => native_agent::Model;
    "ctrl-s e",      "Agent > AgentDecision > AgentPlan" => native_agent::Traits;
    "ctrl-s t",      "Agent > AgentDecision > AgentPlan" => native_agent::AccessMode;
    "ctrl-s a",      "Agent > AgentDecision > AgentPlan" => native_agent::NewClaude;
    "ctrl-s A",      "Agent > AgentDecision > AgentPlan" => native_agent::NewCodex;

    // §6.2: the keys are bare letters in a derived context so they cannot fire anywhere else,
    // and **`⏎` is not bound on an approval** — a queued Return keystroke must never approve a
    // shell command. That is the one property worth keeping exactly.
    "y",             "Agent > AgentDecision > AgentPermission" => native_agent::AllowOnce;
    "a",             "Agent > AgentDecision > AgentPermission" => native_agent::AllowSession;
    "n",             "Agent > AgentDecision > AgentPermission" => native_agent::Deny;
    "e",             "Agent > AgentDecision > AgentPermission" => native_agent::EditCommand;
    "escape",        "Agent > AgentDecision > AgentPermission" => native_agent::DenyAndStop;

    "1",             "Agent > AgentDecision > AgentQuestion" => native_agent::Choose1;
    "2",             "Agent > AgentDecision > AgentQuestion" => native_agent::Choose2;
    "3",             "Agent > AgentDecision > AgentQuestion" => native_agent::Choose3;
    "4",             "Agent > AgentDecision > AgentQuestion" => native_agent::Choose4;
    "5",             "Agent > AgentDecision > AgentQuestion" => native_agent::Choose5;
    "space",         "Agent > AgentDecision > AgentQuestion" => native_agent::Toggle;
    "enter",         "Agent > AgentDecision > AgentQuestion" => native_agent::Answer;
    "p",             "Agent > AgentDecision > AgentQuestion" => native_agent::Previous;

    // §6.4: the plan card carries no buttons; its verbs are these two, and which one an empty
    // `⏎` means is decided by whether the composer holds anything.
    "y",             "Agent > AgentDecision > AgentPlan" => native_agent::Implement;
    "n",             "Agent > AgentDecision > AgentPlan" => native_agent::Refine;
    "enter",         "Agent > AgentDecision > AgentPlan" => native_agent::Send;

    "enter",        "Filter" => filter::Accept;
    "escape",       "Filter" => filter::Escape;
    "ctrl-n",       "Filter" => filter::CursorDown;
    "down",         "Filter" => filter::CursorDown;
    "ctrl-p",       "Filter" => filter::CursorUp;
    "up",           "Filter" => filter::CursorUp;
    "backspace",    "Filter" => filter::Backspace;
    "ctrl-w",       "Filter" => filter::DeleteWord;
    "ctrl-u",       "Filter" => filter::Clear;

    "left", "Filter > BoardFilter" => board::PrevColumn;
    "ctrl-b", "Filter > BoardFilter" => board::PrevColumn;
    "right", "Filter > BoardFilter" => board::NextColumn;
    "ctrl-f", "Filter > BoardFilter" => board::NextColumn;

    "enter",        "Palette" => palette::Run;
    "escape",       "Palette" => palette::Close;
    "ctrl-n",       "Palette" => palette::CursorDown;
    "down",         "Palette" => palette::CursorDown;
    "ctrl-p",       "Palette" => palette::CursorUp;
    "up",           "Palette" => palette::CursorUp;
    "backspace",    "Palette" => palette::Backspace;
    "ctrl-w",       "Palette" => palette::DeleteWord;
    "ctrl-u",       "Palette" => palette::Clear;

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

    "left",         "Dialog > Create" => create_worktree::HostPrev;
    "right",        "Dialog > Create" => create_worktree::HostNext;
    "alt-enter",    "Dialog > Create" => create_worktree::CreateWithoutOpening;

    "y",            "Dialog > Confirm" => confirm::Accept;
    "enter",        "Dialog > Confirm" => confirm::Accept;
    "Y",            "Dialog > Confirm" => confirm::AcceptStrong;
    "n",            "Dialog > Confirm" => confirm::Reject;
    "q",            "Dialog > Confirm" => confirm::Reject;
    "escape",       "Dialog > Confirm" => confirm::Reject;
    "I",            "Dialog > Confirm" => confirm::Recheck;
    "s",            "Dialog > Confirm" => confirm::ToggleKeep;

    "ctrl-d",       "Dialog > Context" => context_dialog::Delete;

    "j",            "Dialog > Assign" => dialog::CursorDown;
    "k",            "Dialog > Assign" => dialog::CursorUp;

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

    "escape",       "Dialog > Help" => help::Close;
    "?",            "Dialog > Help" => help::Close;

    "y",            "Dialog > Quit" => quit_dialog::Accept;
    "n",            "Dialog > Quit" => quit_dialog::Reject;
    "escape",       "Dialog > Quit" => quit_dialog::Reject;
    "J",            "Dialog > Quit" => quit_dialog::OpenJobs;
    "W",            "Dialog > Quit" => quit_dialog::NeverWarn;

    "Y",            "Dialog > QuitDaemon" => quit_daemon_dialog::Accept;
    "n",            "Dialog > QuitDaemon" => quit_daemon_dialog::Reject;
    "escape",       "Dialog > QuitDaemon" => quit_daemon_dialog::Reject;

    "r",            "Daemon > Down" => daemon::Retry;
    "L",            "Daemon > Down" => daemon::OpenLog;
    "D",            "Daemon > Down" => daemon::RunDoctor;
    "r",            "Daemon > Doctor" => daemon::Retry;
    "D",            "Daemon > Doctor" => daemon::RunDoctor;
    "L",            "Daemon > Doctor" => daemon::OpenLog;
    "escape",       "Daemon > Doctor" => daemon::DismissBanner;

    "r",            "Daemon > Banner" => daemon::Reconnect;
    "l",            "Daemon > Banner" => daemon::OpenLog;
    "escape",       "Daemon > Banner" => daemon::DismissBanner;

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
    use std::collections::{HashMap, HashSet};

    use super::*;

    /// Every key context named in `docs/KEYMAP.md`.
    const REQUIRED_CONTEXTS: &[&str] = &[
        "Fleet",
        "Hub",
        "Hub > Repos",
        "Hub > Worktrees",
        "Hub > Prs",
        "Hub > Board",
        "Dialog > CardDetail",
        "Dialog > CardCreate",
        "Dialog > CardPicker",
        "Dialog > BoardSettings",
        "Workspace > Terminal",
        "Workspace > Native",
        "Workspace > Prefix",
        "Workspace > Scroll",
        // `Agent` itself binds nothing: gpui evaluates a bare identifier against every node of
        // the dispatch path, so an `Agent` binding also fires inside a native agent tab, where
        // the popup is not mounted. Every popup key lives on `Agent > Terminal` / `> Prefix`.
        "Agent > Terminal",
        "Agent > Prefix",
        "Agent > Scroll",
        "Agent > AgentIdle",
        "Agent > AgentWorking",
        "Agent > AgentDecision > AgentPermission",
        "Agent > AgentDecision > AgentQuestion",
        "Agent > AgentDecision > AgentPlan",
        // §12: row focus lives **inside** scroll mode, so the row context is only ever on the
        // chain under `AgentNativeScroll`. Binding it as a sibling was the shape that made
        // `⏎`/`u`/`o` bound, handled and never entered.
        "Agent > AgentNativeScroll",
        "Agent > AgentNativeScroll > AgentRow",
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
        "Daemon > Doctor",
        "FirstRun",
    ];

    #[test]
    fn board_documentation_and_bindings_match_in_both_directions() {
        let docs = include_str!("../../../docs/KEYMAP.md");
        let documented: HashSet<_> = docs
            .lines()
            .filter_map(|line| {
                let fields: Vec<_> = line.split('`').collect();
                if fields.len() < 7 {
                    return None;
                }
                let (keys, context, action) = (fields[1], fields[3], fields[5]);
                (action.starts_with("board::")
                    || action.starts_with("card_detail::")
                    || matches!(
                        context,
                        "Dialog > CardCreate" | "Dialog > CardPicker" | "Dialog > BoardSettings"
                    ))
                .then_some((keys, context, action))
            })
            .collect();
        let registered: HashSet<_> = table()
            .into_iter()
            .filter(|spec| {
                spec.action.starts_with("board::")
                    || spec.action.starts_with("card_detail::")
                    || matches!(
                        spec.context,
                        "Dialog > CardCreate" | "Dialog > CardPicker" | "Dialog > BoardSettings"
                    )
            })
            .map(|spec| (spec.keys, spec.context, spec.action))
            .collect();
        assert!(!documented.is_empty());
        assert_eq!(documented, registered);
    }

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

    /// The palette replaces the dialog it is opened over and keeps its draft, and the two
    /// `Card detail:` rows act on exactly that. Without a way in from the detail they are rows
    /// no state can ever list, and `behind_palette` is dead machinery.
    #[test]
    fn the_palette_can_be_opened_over_the_card_detail() {
        assert!(
            table()
                .iter()
                .any(|spec| spec.context == "Dialog > CardDetail"
                    && spec.action.ends_with("OpenPalette")),
            "nothing opens the palette from the card detail"
        );
    }

    /// The doctor report replaces the whole context chain (`shell/root/focus.rs`), so every
    /// recovery key the surface it was raised from documented has to live on it too.
    #[test]
    fn the_doctor_report_keeps_the_case_b_retry_key() {
        assert_eq!(
            action_for_keystroke("Daemon > Doctor", &Keystroke::parse("r").unwrap())
                .map(|action| action.name()),
            Some("daemon::Retry")
        );
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
        for keys in [
            "g b",
            "g g",
            "g t",
            "g shift-t",
            "g r",
            "g w",
            "g p",
            "g j",
            "g a",
        ] {
            assert!(
                table.iter().any(|spec| spec.keys == keys),
                "missing the `{keys}` sequence"
            );
        }
    }

    #[test]
    fn terminal_bindings_preserve_clipboard_and_viewport_shortcuts() {
        let terminal: Vec<_> = table()
            .into_iter()
            .filter(|spec| spec.context == "Workspace > Terminal")
            .collect();
        assert_eq!(
            terminal.iter().map(|spec| spec.keys).collect::<Vec<_>>(),
            [
                "ctrl-s",
                "cmd-c",
                "cmd-v",
                "shift-pageup",
                "shift-pagedown",
                "cmd-home",
                "cmd-end"
            ]
        );
        for keys in ["pageup", "pagedown", "home", "end"] {
            assert!(
                action_for_keystroke("Workspace > Terminal", &Keystroke::parse(keys).unwrap())
                    .is_none()
            );
        }
        assert!(
            terminal.iter().all(|spec| spec.keys != "ctrl-v"),
            "ctrl-v belongs to shells and terminal applications"
        );
    }

    /// The resting context of a `fleet://` tab reserves `ctrl-s` and nothing else.
    ///
    /// A native tab hands every other key to the embedded gpui view, which resolves it against
    /// its own bindings. `Workspace > Terminal` may reserve the macOS clipboard and viewport
    /// keys because a PTY has no use for them; the pane does, so it keeps them.
    #[test]
    fn prefix_is_the_only_app_key_over_a_native_pane() {
        let bound: Vec<_> = table()
            .into_iter()
            .filter(|spec| spec.context == "Workspace > Native")
            .collect();
        assert_eq!(bound.len(), 1, "Workspace > Native binds more than one key");
        assert_eq!(bound[0].keys, "ctrl-s");
        assert_eq!(bound[0].action, "workspace::EnterPrefix");
    }

    /// Every fleet-lazygit action name must differ from every fleet-app one.
    ///
    /// gpui registers actions process-wide under `namespace::Name` through `inventory`, and
    /// `App::load_actions` **panics** on a duplicate — at startup, before any window exists.
    /// Linking the two crates is what makes that a real risk, so the check runs wherever they
    /// are linked, which is here.
    #[test]
    fn no_action_name_is_registered_twice() {
        let mut seen: HashMap<&'static str, usize> = HashMap::new();
        let mut names = Vec::new();
        for builder in gpui::private::inventory::iter::<gpui::MacroActionBuilder> {
            let action = (builder.0)();
            *seen.entry(action.name).or_default() += 1;
            names.push(action.name);
        }
        let duplicates: Vec<_> = seen
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(name, _)| *name)
            .collect();
        assert!(
            duplicates.is_empty(),
            "these action names are registered more than once: {duplicates:?}"
        );
        // A sanity check that both crates really are linked into this test binary: without it
        // the assertion above would pass on an empty inventory.
        assert!(
            names.contains(&"workspace::EnterPrefix"),
            "fleet-app's actions are missing from the inventory"
        );
        assert!(
            names.contains(&"lg_confirm::Accept"),
            "fleet-lazygit's actions are missing from the inventory"
        );
    }

    /// fleet-lazygit's own contexts must not satisfy any fleet-app binding predicate.
    ///
    /// gpui's `>` is a *subsequence* test over the rendered chain, not a parent test, so a
    /// pane rendering `... > Dialog > Confirm` inside the Workspace would answer fleet-app's
    /// `Dialog > Confirm` bindings as well as its own.
    #[test]
    fn the_embedded_pane_shares_no_context_word_with_the_app() {
        fn context_words(contexts: impl Iterator<Item = &'static str>) -> HashSet<&'static str> {
            contexts
                .flat_map(|context| context.split('>').map(str::trim))
                .collect()
        }

        let pane_words = context_words(
            fleet_lazygit::keymap::table()
                .into_iter()
                .map(|spec| spec.context),
        );
        let app_words = context_words(table().into_iter().map(|spec| spec.context));
        let shared: Vec<_> = pane_words.intersection(&app_words).copied().collect();
        assert!(
            shared.is_empty(),
            "these key-context words mean two things at once: {shared:?}"
        );
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
    fn watch_keys_are_prefix_only() {
        for (key, name) in [
            ("v", "prefix::ToggleWatchPane"),
            ("V", "prefix::DismissWatch"),
            ("N", "prefix::NextWatch"),
            ("P", "prefix::PrevWatch"),
        ] {
            let stroke = Keystroke::parse(key).unwrap();
            assert_eq!(
                action_for_keystroke("Workspace > Prefix", &stroke)
                    .unwrap()
                    .name(),
                name
            );
            assert!(action_for_keystroke("Workspace > Terminal", &stroke).is_none());
        }
    }

    #[test]
    fn live_prefix_resolution_uses_the_authoritative_table() {
        for keys in ["s", "S", "ctrl-s", "1", "tab", "W", "[", "]", "escape"] {
            let keystroke = Keystroke::parse(keys)
                .unwrap_or_else(|error| panic!("invalid test key {keys:?}: {error}"));
            let action = action_for_keystroke("Workspace > Prefix", &keystroke)
                .unwrap_or_else(|| panic!("prefix key {keys:?} did not resolve"));
            let spec = table()
                .into_iter()
                .find(|spec| spec.context == "Workspace > Prefix" && spec.keys == keys)
                .unwrap_or_else(|| panic!("prefix key {keys:?} is absent from the table"));
            assert_eq!(action.name(), spec.action, "{keys}");
        }

        assert!(
            action_for_keystroke(
                "Workspace > Prefix",
                &Keystroke::parse("d").unwrap_or_else(|error| panic!("{error}"))
            )
            .is_none(),
            "an unknown prefix key is consumed without inventing an action"
        );
    }

    #[test]
    fn agent_popup_prefix_resolution_uses_the_authoritative_table() {
        for (keys, expected) in [
            ("q", "agent::Hide"),
            ("a", "fleet::OpenAgentClaude"),
            ("A", "fleet::OpenAgentCodex"),
            ("[", "prefix::EnterScroll"),
            ("]", "prefix::Paste"),
            ("r", "prefix::RestartCommand"),
            ("?", "fleet::OpenHelp"),
            ("ctrl-s", "prefix::SendLiteral"),
            ("ctrl-q", "agent::Hide"),
            ("ctrl-shift-q", "fleet::QuitAndStopDaemon"),
            ("cmd-c", "agent::CopySelection"),
            ("cmd-v", "agent::PasteClipboard"),
            ("escape", "prefix::Cancel"),
        ] {
            let stroke = Keystroke::parse(keys)
                .unwrap_or_else(|error| panic!("invalid test key {keys:?}: {error}"));
            let action = action_for_keystroke("Agent > Prefix", &stroke)
                .unwrap_or_else(|| panic!("agent prefix key {keys:?} did not resolve"));
            assert_eq!(action.name(), expected, "{keys}");
            assert!(table().iter().any(|spec| {
                spec.context == "Agent > Prefix" && spec.keys == keys && spec.action == expected
            }));
        }
    }

    #[test]
    fn agent_popup_ctrl_q_hides_instead_of_quitting() {
        let stroke = Keystroke::parse("ctrl-q").unwrap_or_else(|error| panic!("{error}"));
        for context in ["Agent > Terminal", "Agent > Prefix"] {
            assert_eq!(
                action_for_keystroke(context, &stroke)
                    .unwrap_or_else(|| panic!("{context} must override global ctrl-q"))
                    .name(),
                "agent::Hide"
            );
        }
        // …and nowhere above them. gpui matches a bare identifier at every node of the dispatch
        // path, so a binding on `Agent` also fires in a native agent tab — where the popup is
        // not mounted, nothing handles `agent::Hide`, and the global quit would be shadowed by
        // a key that does nothing at all.
        assert!(
            action_for_keystroke("Agent", &stroke).is_none(),
            "ctrl-q on the Agent root shadows the global quit in a native agent tab"
        );
        assert!(table().iter().any(|spec| {
            spec.context == ROOT_CONTEXT
                && spec.keys == "ctrl-shift-q"
                && spec.action == "fleet::QuitAndStopDaemon"
        }));
        for (keys, expected) in [
            ("ctrl-s", "agent::EnterPrefix"),
            ("cmd-c", "agent::CopySelection"),
            ("cmd-v", "agent::PasteClipboard"),
        ] {
            let stroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                action_for_keystroke("Agent > Terminal", &stroke)
                    .unwrap_or_else(|| panic!("missing Agent terminal reservation for {keys}"))
                    .name(),
                expected
            );
        }
        for keys in ["shift-pageup", "shift-pagedown", "cmd-home", "cmd-end"] {
            let stroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
            assert!(
                action_for_keystroke("Agent > Terminal", &stroke).is_none(),
                "{keys} must reach the agent PTY"
            );
        }
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

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn malformed_bindings_return_errors_without_panicking() {
        for spec in [
            BindingSpec {
                keys: "not-a-valid-key-modifier-x",
                context: "Fleet",
                action: "fleet::Cancel",
            },
            BindingSpec {
                keys: "escape",
                context: "(",
                action: "fleet::Cancel",
            },
        ] {
            assert!(parse_binding(spec, Box::new(Cancel)).is_err());
        }
    }

    #[test]
    fn repeated_lookup_reuses_the_validated_bindings() {
        let storage = PARSED_BINDINGS.with(|bindings| bindings.as_ptr());
        for spec in cached_table()
            .iter()
            .filter(|spec| !spec.keys.contains(' '))
        {
            let keystroke = Keystroke::parse(spec.keys).unwrap();
            let action = action_for_keystroke(spec.context, &keystroke).unwrap();
            assert_eq!(action.name(), spec.action);
        }
        assert_eq!(storage, PARSED_BINDINGS.with(|bindings| bindings.as_ptr()));
    }
}
