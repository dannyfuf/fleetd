//! The declarative key table.
//!
//! One macro produces three artifacts from the same rows: the [`gpui::KeyBinding`]s that get
//! registered, the table as data (which the `?` help overlay and the bottom key-hint bar render
//! from, so a binding and its documentation cannot drift), and the context list used by the tests.
//!
//! `>` never appears inside `key_context()`. It exists only in a *binding predicate* and means
//! "some ancestor carries this context", which is why [`crate::state::GitUiState::context_chain`]
//! returns one word per nesting level and the root view emits one `div` per word.

use gpui::{Action, App, KeyBinding};

use crate::actions::{
    branches, commitfiles, commits, conflict, diff, files, global, lg_confirm, lg_help, list, menu,
    prompt, remotes, staging, stash, subcommits, tags,
};

/// One row of the key table, in the order it is registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingSpec {
    /// The keystrokes, space separated for a sequence (`"g g"`).
    pub keys: &'static str,
    /// The gpui key-context predicate this row is scoped to.
    pub context: &'static str,
    /// The fully qualified action name, e.g. `files::ToggleStaged`.
    pub action: &'static str,
}

macro_rules! key_table {
    ($( $keys:literal, $context:literal => $action:expr ; )*) => {
        /// Every binding, ready for [`gpui::App::bind_keys`].
        #[must_use]
        pub fn bindings() -> Vec<KeyBinding> {
            vec![$( KeyBinding::new($keys, $action, Some($context)) ),*]
        }

        /// The same table as data: keystrokes, context and action name.
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

/// The root key context. It is on the outermost div of every frame and binds nothing printable,
/// so a focused text prompt can never trip a global command.
pub const ROOT_CONTEXT: &str = "Lazygit";

key_table! {
    // -------------------------------------------------------------- list navigation (all panels)
    "j",            "Lazygit > Panels" => list::MoveDown;
    "down",         "Lazygit > Panels" => list::MoveDown;
    "k",            "Lazygit > Panels" => list::MoveUp;
    "up",           "Lazygit > Panels" => list::MoveUp;
    "<",            "Lazygit > Panels" => list::GoTop;
    "home",         "Lazygit > Panels" => list::GoTop;
    ">",            "Lazygit > Panels" => list::GoBottom;
    "G",            "Lazygit > Panels" => list::GoBottom;
    "end",          "Lazygit > Panels" => list::GoBottom;
    "ctrl-d",       "Lazygit > Panels" => list::PageDown;
    ".",            "Lazygit > Panels" => list::PageDown;
    "ctrl-u",       "Lazygit > Panels" => list::PageUp;
    ",",            "Lazygit > Panels" => list::PageUp;
    "H",            "Lazygit > Panels" => list::ScrollLeft;
    "L",            "Lazygit > Panels" => list::ScrollRight;

    // -------------------------------------------------------------- diff view
    // `{` / `}` are lazygit's own context keys. `|` is free in lazygit and reads as two
    // columns, which is what it does.
    "{",            "Lazygit > Panels" => diff::LessContext;
    "}",            "Lazygit > Panels" => diff::MoreContext;
    "|",            "Lazygit > Panels" => diff::ToggleSplit;

    // `g g` is registered before any bare `g`, so a panel that binds `g` wins outright instead of
    // leaving the sequence pending. It is therefore only offered where `g` is free.
    "g g",          "Lazygit > Panels > Status" => list::GoTop;
    "g g",          "Lazygit > Panels > Files" => list::GoTop;
    "g g",          "Lazygit > Panels > Branches" => list::GoTop;
    "g g",          "Lazygit > Panels > Remotes" => list::GoTop;
    "g g",          "Lazygit > Panels > Tags" => list::GoTop;
    "g g",          "Lazygit > Panels > Main" => list::GoTop;

    // -------------------------------------------------------------- global
    "1",            "Lazygit > Panels" => global::FocusStatus;
    "2",            "Lazygit > Panels" => global::FocusFiles;
    "3",            "Lazygit > Panels" => global::FocusBranches;
    "4",            "Lazygit > Panels" => global::FocusCommits;
    "5",            "Lazygit > Panels" => global::FocusStash;
    "0",            "Lazygit > Panels" => global::FocusMain;
    "tab",          "Lazygit > Panels" => global::NextPanel;
    "shift-tab",    "Lazygit > Panels" => global::PrevPanel;
    "]",            "Lazygit > Panels" => global::NextTab;
    "[",            "Lazygit > Panels" => global::PrevTab;
    "l",            "Lazygit > Panels" => global::NextTab;
    "h",            "Lazygit > Panels" => global::PrevTab;
    "q",            "Lazygit > Panels" => global::Quit;
    "R",            "Lazygit > Panels" => global::Refresh;
    "?",            "Lazygit > Panels" => global::OpenHelp;
    "+",            "Lazygit > Panels" => global::NextScreenMode;
    "_",            "Lazygit > Panels" => global::PrevScreenMode;
    "@",            "Lazygit > Panels" => global::ToggleCommandLog;
    "escape",       "Lazygit > Panels" => global::Cancel;
    "p",            "Lazygit > Panels" => global::Pull;
    "P",            "Lazygit > Panels" => global::Push;
    "f",            "Lazygit > Panels" => global::Fetch;
    "m",            "Lazygit > Panels" => global::OperationMenu;

    // -------------------------------------------------------------- Files
    "space",        "Lazygit > Panels > Files" => files::ToggleStaged;
    "a",            "Lazygit > Panels > Files" => files::ToggleStagedAll;
    "d",            "Lazygit > Panels > Files" => files::Discard;
    "c",            "Lazygit > Panels > Files" => files::Commit;
    "A",            "Lazygit > Panels > Files" => files::Amend;
    "S",            "Lazygit > Panels > Files" => files::StashMenu;
    // lazygit toggles the tree with `` ` ``; both halves of that key are bound because a
    // platform may report the shifted one as `~`.
    "`",            "Lazygit > Panels > Files" => files::ToggleTree;
    "~",            "Lazygit > Panels > Files" => files::ToggleTree;
    "-",            "Lazygit > Panels > Files" => files::CollapseAll;
    "=",            "Lazygit > Panels > Files" => files::ExpandAll;
    "enter",        "Lazygit > Panels > Files" => files::Enter;

    // -------------------------------------------------------------- Branches, Local tab
    "space",        "Lazygit > Panels > Branches" => branches::Checkout;
    "n",            "Lazygit > Panels > Branches" => branches::New;
    "d",            "Lazygit > Panels > Branches" => branches::Delete;
    "R",            "Lazygit > Panels > Branches" => branches::Rename;
    "M",            "Lazygit > Panels > Branches" => branches::Merge;
    "r",            "Lazygit > Panels > Branches" => branches::Rebase;
    "u",            "Lazygit > Panels > Branches" => branches::UpstreamMenu;
    "T",            "Lazygit > Panels > Branches" => branches::Tag;
    "enter",        "Lazygit > Panels > Branches" => branches::Enter;

    // -------------------------------------------------------------- Branches, Remotes tab
    "enter",        "Lazygit > Panels > Remotes" => remotes::Enter;
    "space",        "Lazygit > Panels > Remotes" => remotes::Checkout;

    // -------------------------------------------------------------- Branches, Tags tab
    "n",            "Lazygit > Panels > Tags" => tags::New;
    "d",            "Lazygit > Panels > Tags" => tags::Delete;
    "space",        "Lazygit > Panels > Tags" => tags::Checkout;

    // -------------------------------------------------------------- Commits
    "enter",        "Lazygit > Panels > Commits" => commits::Enter;
    "space",        "Lazygit > Panels > Commits" => commits::Checkout;
    "r",            "Lazygit > Panels > Commits" => commits::Reword;
    "s",            "Lazygit > Panels > Commits" => commits::Squash;
    "f",            "Lazygit > Panels > Commits" => commits::Fixup;
    "d",            "Lazygit > Panels > Commits" => commits::Drop;
    "e",            "Lazygit > Panels > Commits" => commits::Edit;
    "ctrl-j",       "Lazygit > Panels > Commits" => commits::MoveDown;
    "ctrl-k",       "Lazygit > Panels > Commits" => commits::MoveUp;
    "g",            "Lazygit > Panels > Commits" => commits::ResetMenu;
    "c",            "Lazygit > Panels > Commits" => commits::Copy;
    "v",            "Lazygit > Panels > Commits" => commits::Paste;
    "t",            "Lazygit > Panels > Commits" => commits::Revert;
    "T",            "Lazygit > Panels > Commits" => commits::Tag;
    "A",            "Lazygit > Panels > Commits" => commits::Amend;
    "n",            "Lazygit > Panels > Commits" => commits::NewBranch;

    // -------------------------------------------------------------- Commits, Reflog tab
    "enter",        "Lazygit > Panels > Reflog" => commits::Enter;
    "space",        "Lazygit > Panels > Reflog" => commits::Checkout;
    "c",            "Lazygit > Panels > Reflog" => commits::Copy;
    "v",            "Lazygit > Panels > Reflog" => commits::Paste;
    "g",            "Lazygit > Panels > Reflog" => commits::ResetMenu;

    // -------------------------------------------------------------- Stash
    "space",        "Lazygit > Panels > Stash" => stash::Apply;
    "g",            "Lazygit > Panels > Stash" => stash::Pop;
    "d",            "Lazygit > Panels > Stash" => stash::Drop;
    "n",            "Lazygit > Panels > Stash" => stash::NewBranch;
    "enter",        "Lazygit > Panels > Stash" => stash::Enter;

    // -------------------------------------------------------------- Main panel, staging mode
    "space",        "Lazygit > Panels > Staging" => staging::Apply;
    "d",            "Lazygit > Panels > Staging" => staging::Discard;
    "v",            "Lazygit > Panels > Staging" => staging::ToggleRange;
    "a",            "Lazygit > Panels > Staging" => staging::ToggleLineMode;
    "tab",          "Lazygit > Panels > Staging" => staging::SwitchSide;
    "h",            "Lazygit > Panels > Staging" => staging::PrevHunk;
    "l",            "Lazygit > Panels > Staging" => staging::NextHunk;
    "left",         "Lazygit > Panels > Staging" => staging::PrevHunk;
    "right",        "Lazygit > Panels > Staging" => staging::NextHunk;

    // -------------------------------------------------------------- Main panel, sub-commits
    "enter",        "Lazygit > Panels > Main > SubCommits" => subcommits::ShowDiff;
    "space",        "Lazygit > Panels > Main > SubCommits" => subcommits::ShowDiff;
    "c",            "Lazygit > Panels > Main > SubCommits" => commits::Copy;
    "v",            "Lazygit > Panels > Main > SubCommits" => commits::Paste;

    // -------------------------------------------------------------- Main panel, commit files
    "enter",        "Lazygit > Panels > Main > CommitFiles" => commitfiles::ShowPatch;
    "space",        "Lazygit > Panels > Main > CommitFiles" => commitfiles::ShowPatch;

    // -------------------------------------------------------------- Main panel, conflict mode
    "o",            "Lazygit > Panels > Conflict" => conflict::TakeOurs;
    "t",            "Lazygit > Panels > Conflict" => conflict::TakeTheirs;
    "b",            "Lazygit > Panels > Conflict" => conflict::TakeBoth;
    "l",            "Lazygit > Panels > Conflict" => conflict::NextSection;
    "h",            "Lazygit > Panels > Conflict" => conflict::PrevSection;

    // -------------------------------------------------------------- Confirmation dialog
    "enter",        "Lazygit > LgDialog > LgConfirm" => lg_confirm::Accept;
    "y",            "Lazygit > LgDialog > LgConfirm" => lg_confirm::Accept;
    "escape",       "Lazygit > LgDialog > LgConfirm" => lg_confirm::Cancel;
    "n",            "Lazygit > LgDialog > LgConfirm" => lg_confirm::Cancel;

    // -------------------------------------------------------------- Text prompt (no bare keys)
    "enter",        "Lazygit > LgDialog > Prompt" => prompt::Accept;
    "cmd-enter",    "Lazygit > LgDialog > Prompt" => prompt::Submit;
    "ctrl-enter",   "Lazygit > LgDialog > Prompt" => prompt::Submit;
    "escape",       "Lazygit > LgDialog > Prompt" => prompt::Cancel;
    "backspace",    "Lazygit > LgDialog > Prompt" => prompt::Backspace;
    "ctrl-w",       "Lazygit > LgDialog > Prompt" => prompt::DeleteWord;
    "ctrl-u",       "Lazygit > LgDialog > Prompt" => prompt::DeleteToStart;
    "left",         "Lazygit > LgDialog > Prompt" => prompt::Left;
    "right",        "Lazygit > LgDialog > Prompt" => prompt::Right;
    "ctrl-a",       "Lazygit > LgDialog > Prompt" => prompt::Home;
    "ctrl-e",       "Lazygit > LgDialog > Prompt" => prompt::End;
    "cmd-v",        "Lazygit > LgDialog > Prompt" => prompt::Paste;
    "ctrl-v",       "Lazygit > LgDialog > Prompt" => prompt::Paste;

    // -------------------------------------------------------------- Menu
    "enter",        "Lazygit > LgDialog > Menu" => menu::Accept;
    "escape",       "Lazygit > LgDialog > Menu" => menu::Cancel;
    "j",            "Lazygit > LgDialog > Menu" => menu::Down;
    "down",         "Lazygit > LgDialog > Menu" => menu::Down;
    "k",            "Lazygit > LgDialog > Menu" => menu::Up;
    "up",           "Lazygit > LgDialog > Menu" => menu::Up;
    "/",            "Lazygit > LgDialog > Menu" => menu::StartFilter;

    // -------------------------------------------------------------- Menu, filtering (no bare keys)
    "enter",        "Lazygit > LgDialog > MenuFilter" => menu::Accept;
    "escape",       "Lazygit > LgDialog > MenuFilter" => menu::Cancel;
    "down",         "Lazygit > LgDialog > MenuFilter" => menu::Down;
    "up",           "Lazygit > LgDialog > MenuFilter" => menu::Up;
    "ctrl-n",       "Lazygit > LgDialog > MenuFilter" => menu::Down;
    "ctrl-p",       "Lazygit > LgDialog > MenuFilter" => menu::Up;
    "backspace",    "Lazygit > LgDialog > MenuFilter" => prompt::Backspace;
    "ctrl-u",       "Lazygit > LgDialog > MenuFilter" => prompt::DeleteToStart;
    "ctrl-w",       "Lazygit > LgDialog > MenuFilter" => prompt::DeleteWord;

    // -------------------------------------------------------------- Help
    "escape",       "Lazygit > LgDialog > LgHelp" => lg_help::Close;
    "q",            "Lazygit > LgDialog > LgHelp" => lg_help::Close;
    "?",            "Lazygit > LgDialog > LgHelp" => lg_help::Close;
    "j",            "Lazygit > LgDialog > LgHelp" => lg_help::Down;
    "down",         "Lazygit > LgDialog > LgHelp" => lg_help::Down;
    "k",            "Lazygit > LgDialog > LgHelp" => lg_help::Up;
    "up",           "Lazygit > LgDialog > LgHelp" => lg_help::Up;
}

/// Registers the whole table with the app.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
}

/// Whether a binding predicate is satisfied by a rendered chain.
///
/// `A > B` is gpui's *descendant* operator: it means "a node carrying `B` with `A` somewhere
/// above it", not "the immediate child of `A`". Matching is therefore a subsequence test over
/// the chain, root context included.
#[must_use]
pub fn context_matches(context: &str, chain: &[&str]) -> bool {
    let mut full = vec![ROOT_CONTEXT];
    full.extend_from_slice(chain);
    let mut words = full.into_iter();
    context
        .split('>')
        .map(str::trim)
        .all(|word| words.any(|candidate| candidate == word))
}

/// Every binding reachable from a rendered chain, in table order.
#[must_use]
pub fn bindings_for_chain(chain: &[&str]) -> Vec<BindingSpec> {
    table()
        .into_iter()
        .filter(|spec| context_matches(spec.context, chain))
        .collect()
}

/// `files::ToggleStaged` becomes `toggle staged`.
#[must_use]
pub fn humanize(action: &str) -> String {
    let name = action.rsplit("::").next().unwrap_or(action);
    let mut words = String::new();
    for (index, character) in name.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            words.push(' ');
        }
        words.extend(character.to_lowercase());
    }
    words
}

/// `ctrl-n` becomes `^n`, `shift-tab` becomes `S-⇥`; the hint column is narrow.
#[must_use]
pub fn pretty_keys(keys: &str) -> String {
    keys.split(' ')
        .map(|stroke| {
            stroke
                .replace("ctrl-", "^")
                .replace("cmd-", "\u{2318}")
                .replace("shift-", "S-")
                .replace("alt-", "\u{2325}")
                .replace("escape", "esc")
                .replace("enter", "\u{23ce}")
                .replace("tab", "\u{21e5}")
                .replace("space", "\u{2423}")
                .replace("backspace", "\u{232b}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rows the bottom bar shows for a chain: the deepest context first, then the globals,
/// merged so one action never appears twice.
#[must_use]
pub fn hints_for_chain(chain: &[&str], limit: usize) -> Vec<(String, String)> {
    let mut rows: Vec<BindingSpec> = bindings_for_chain(chain);
    rows.reverse();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut hints = Vec::new();
    for spec in rows {
        if seen.contains(&spec.action) {
            continue;
        }
        seen.push(spec.action);
        hints.push((pretty_keys(spec.keys), humanize(spec.action)));
        if hints.len() >= limit {
            break;
        }
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_keystroke_parses() {
        for spec in table() {
            for stroke in spec.keys.split(' ') {
                assert!(
                    gpui::Keystroke::parse(stroke).is_ok(),
                    "invalid keystroke {stroke:?} in {:?}",
                    spec.action
                );
            }
        }
    }

    #[test]
    fn no_context_binds_the_same_keys_twice() {
        let mut seen = HashSet::new();
        for spec in table() {
            assert!(
                seen.insert((spec.context, spec.keys)),
                "`{}` is bound twice in `{}`",
                spec.keys,
                spec.context
            );
        }
    }

    #[test]
    fn text_contexts_never_bind_a_printable_key() {
        for spec in table() {
            if spec.context.ends_with("Prompt") || spec.context.ends_with("MenuFilter") {
                assert!(
                    spec.keys.len() > 1,
                    "`{}` in `{}` would shadow typing",
                    spec.keys,
                    spec.context
                );
            }
        }
    }

    #[test]
    fn the_root_context_binds_nothing() {
        for spec in table() {
            assert_ne!(
                spec.context, ROOT_CONTEXT,
                "the root context must stay free so prompts can type"
            );
        }
    }

    #[test]
    fn hints_come_from_the_table() {
        let hints = hints_for_chain(&["Panels", "Files"], 4);
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(_, label)| label == "enter"));
    }

    #[test]
    fn humanize_and_pretty_keys_format_for_the_hint_bar() {
        assert_eq!(humanize("files::ToggleStaged"), "toggle staged");
        assert_eq!(pretty_keys("ctrl-d"), "^d");
        assert_eq!(pretty_keys("g g"), "g g");
    }
}
