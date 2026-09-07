//! The declarative key table.
//!
//! One macro produces three artifacts from the same rows: the [`gpui::KeyBinding`]s that get
//! registered, the table as data (which the `?` help overlay and the bottom key-hint bar render
//! from, so a binding and its documentation cannot drift), and the context list used by the tests.
//!
//! `>` never appears inside `key_context()`. It exists only in a *binding predicate* and means
//! "some ancestor carries this context", which is why the state's context chain returns one word
//! per nesting level and the root view emits one `div` per word.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Action, App, KeyBinding, SharedString};

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
        pub fn table() -> Vec<BindingSpec> { cached_table().to_vec() }

        fn cached_table() -> &'static [BindingSpec] {
            static TABLE: std::sync::OnceLock<Vec<BindingSpec>> = std::sync::OnceLock::new();
            TABLE.get_or_init(|| vec![$( BindingSpec {
                keys: $keys,
                context: $context,
                action: Action::name(&$action),
            } ),*])
        }
    };
}

/// The root key context. It is on the outermost div of every frame and binds nothing printable,
/// so a focused text prompt can never trip a global command.
pub const ROOT_CONTEXT: &str = "Lazygit";

key_table! {

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

    "space",        "Lazygit > Panels > Branches" => branches::Checkout;
    "n",            "Lazygit > Panels > Branches" => branches::New;
    "d",            "Lazygit > Panels > Branches" => branches::Delete;
    "R",            "Lazygit > Panels > Branches" => branches::Rename;
    "M",            "Lazygit > Panels > Branches" => branches::Merge;
    "r",            "Lazygit > Panels > Branches" => branches::Rebase;
    "u",            "Lazygit > Panels > Branches" => branches::UpstreamMenu;
    "T",            "Lazygit > Panels > Branches" => branches::Tag;
    "enter",        "Lazygit > Panels > Branches" => branches::Enter;

    "enter",        "Lazygit > Panels > Remotes" => remotes::Enter;
    "space",        "Lazygit > Panels > Remotes" => remotes::Checkout;

    "n",            "Lazygit > Panels > Tags" => tags::New;
    "d",            "Lazygit > Panels > Tags" => tags::Delete;
    "space",        "Lazygit > Panels > Tags" => tags::Checkout;

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

    "enter",        "Lazygit > Panels > Reflog" => commits::Enter;
    "space",        "Lazygit > Panels > Reflog" => commits::Checkout;
    "c",            "Lazygit > Panels > Reflog" => commits::Copy;
    "v",            "Lazygit > Panels > Reflog" => commits::Paste;
    "g",            "Lazygit > Panels > Reflog" => commits::ResetMenu;

    "space",        "Lazygit > Panels > Stash" => stash::Apply;
    "g",            "Lazygit > Panels > Stash" => stash::Pop;
    "d",            "Lazygit > Panels > Stash" => stash::Drop;
    "n",            "Lazygit > Panels > Stash" => stash::NewBranch;
    "enter",        "Lazygit > Panels > Stash" => stash::Enter;

    "space",        "Lazygit > Panels > Staging" => staging::Apply;
    "d",            "Lazygit > Panels > Staging" => staging::Discard;
    "v",            "Lazygit > Panels > Staging" => staging::ToggleRange;
    "a",            "Lazygit > Panels > Staging" => staging::ToggleLineMode;
    "tab",          "Lazygit > Panels > Staging" => staging::SwitchSide;
    "h",            "Lazygit > Panels > Staging" => staging::PrevHunk;
    "l",            "Lazygit > Panels > Staging" => staging::NextHunk;
    "left",         "Lazygit > Panels > Staging" => staging::PrevHunk;
    "right",        "Lazygit > Panels > Staging" => staging::NextHunk;

    "enter",        "Lazygit > Panels > Main > SubCommits" => subcommits::ShowDiff;
    "space",        "Lazygit > Panels > Main > SubCommits" => subcommits::ShowDiff;
    "c",            "Lazygit > Panels > Main > SubCommits" => commits::Copy;
    "v",            "Lazygit > Panels > Main > SubCommits" => commits::Paste;

    "enter",        "Lazygit > Panels > Main > CommitFiles" => commitfiles::ShowPatch;
    "space",        "Lazygit > Panels > Main > CommitFiles" => commitfiles::ShowPatch;

    "o",            "Lazygit > Panels > Conflict" => conflict::TakeOurs;
    "t",            "Lazygit > Panels > Conflict" => conflict::TakeTheirs;
    "b",            "Lazygit > Panels > Conflict" => conflict::TakeBoth;
    "l",            "Lazygit > Panels > Conflict" => conflict::NextSection;
    "h",            "Lazygit > Panels > Conflict" => conflict::PrevSection;

    "enter",        "Lazygit > LgDialog > LgConfirm" => lg_confirm::Accept;
    "y",            "Lazygit > LgDialog > LgConfirm" => lg_confirm::Accept;
    "escape",       "Lazygit > LgDialog > LgConfirm" => lg_confirm::Cancel;
    "n",            "Lazygit > LgDialog > LgConfirm" => lg_confirm::Cancel;

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

    "enter",        "Lazygit > LgDialog > Menu" => menu::Accept;
    "escape",       "Lazygit > LgDialog > Menu" => menu::Cancel;
    "j",            "Lazygit > LgDialog > Menu" => menu::Down;
    "down",         "Lazygit > LgDialog > Menu" => menu::Down;
    "k",            "Lazygit > LgDialog > Menu" => menu::Up;
    "up",           "Lazygit > LgDialog > Menu" => menu::Up;
    "/",            "Lazygit > LgDialog > Menu" => menu::StartFilter;

    "enter",        "Lazygit > LgDialog > MenuFilter" => menu::Accept;
    "escape",       "Lazygit > LgDialog > MenuFilter" => menu::Cancel;
    "down",         "Lazygit > LgDialog > MenuFilter" => menu::Down;
    "up",           "Lazygit > LgDialog > MenuFilter" => menu::Up;
    "ctrl-n",       "Lazygit > LgDialog > MenuFilter" => menu::Down;
    "ctrl-p",       "Lazygit > LgDialog > MenuFilter" => menu::Up;
    "backspace",    "Lazygit > LgDialog > MenuFilter" => prompt::Backspace;
    "ctrl-u",       "Lazygit > LgDialog > MenuFilter" => prompt::DeleteToStart;
    "ctrl-w",       "Lazygit > LgDialog > MenuFilter" => prompt::DeleteWord;

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
pub(crate) fn context_matches(context: &str, chain: &[&str]) -> bool {
    let mut words = std::iter::once(ROOT_CONTEXT).chain(chain.iter().copied());
    context
        .split('>')
        .map(str::trim)
        .all(|word| words.any(|candidate| candidate == word))
}

/// Every binding reachable from a rendered chain, in table order.
///
/// Memoized on the chain: the help overlay resolves the same ~120-row answer on every frame it
/// is open, and the hint bar resolves it again behind [`hints_for_chain`].
#[must_use]
pub(crate) fn bindings_for_chain(chain: &[&'static str]) -> Rc<[BindingSpec]> {
    thread_local! {
        static LAST: RefCell<Option<ResolvedBindings>> = const { RefCell::new(None) };
    }
    LAST.with_borrow_mut(|last| {
        if let Some(resolved) = last
            && resolved.chain == chain
        {
            return Rc::clone(&resolved.bindings);
        }
        let bindings: Rc<[BindingSpec]> = cached_table()
            .iter()
            .copied()
            .filter(|spec| context_matches(spec.context, chain))
            .collect();
        *last = Some(ResolvedBindings {
            chain: chain.to_vec(),
            bindings: Rc::clone(&bindings),
        });
        bindings
    })
}

/// The memoized bindings for one chain.
struct ResolvedBindings {
    chain: Vec<&'static str>,
    bindings: Rc<[BindingSpec]>,
}

/// `files::ToggleStaged` becomes `toggle staged`.
///
/// Public because it is the canonical spelling of an action name: an embedder rendering this
/// crate's key table in its own help surface must produce the same labels.
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
///
/// Public alongside [`humanize`] for the same reason: one spelling of a keystroke everywhere.
#[must_use]
pub fn pretty_keys(keys: &str) -> String {
    let mut pretty = String::with_capacity(keys.len());
    for (index, stroke) in keys.split(' ').enumerate() {
        if index > 0 {
            pretty.push(' ');
        }
        push_stroke(&mut pretty, stroke);
    }
    pretty
}

/// Modifiers are a `-`-separated prefix, so they are peeled one at a time rather than replaced as
/// substrings: `backspace` contains `space`, and would otherwise print as `back\u{2423}`.
fn push_stroke(out: &mut String, stroke: &str) {
    let mut rest = stroke;
    while let Some((modifier, tail)) = rest.split_once('-') {
        let symbol = match modifier {
            "ctrl" => "^",
            "cmd" => "\u{2318}",
            "shift" => "S-",
            "alt" => "\u{2325}",
            _ => break,
        };
        out.push_str(symbol);
        rest = tail;
    }
    out.push_str(match rest {
        "escape" => "esc",
        "enter" => "\u{23ce}",
        "tab" => "\u{21e5}",
        "space" => "\u{2423}",
        "backspace" => "\u{232b}",
        key => key,
    });
}

/// The rows the bottom bar shows for a chain: the deepest context first, then the globals,
/// merged so one action never appears twice.
///
/// The bar re-renders every frame while the chain changes only on navigation, so the last
/// resolution is memoized: a repeat frame clones one `Rc` instead of reformatting every row.
#[must_use]
pub(crate) fn hints_for_chain(chain: &[&'static str], limit: usize) -> Rc<[Hint]> {
    thread_local! {
        static LAST: RefCell<Option<ResolvedHints>> = const { RefCell::new(None) };
    }
    LAST.with_borrow_mut(|last| {
        if let Some(resolved) = last
            && resolved.chain == chain
            && resolved.limit == limit
        {
            return Rc::clone(&resolved.hints);
        }
        let hints = resolve_hints(chain, limit);
        *last = Some(ResolvedHints {
            chain: chain.to_vec(),
            limit,
            hints: Rc::clone(&hints),
        });
        hints
    })
}

/// The memoized answer for one chain.
struct ResolvedHints {
    chain: Vec<&'static str>,
    limit: usize,
    hints: Rc<[Hint]>,
}

/// One rendered row of the bottom key-hint bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hint {
    /// The keystrokes as [`pretty_keys`] renders them.
    pub(crate) keys: SharedString,
    /// The action as [`humanize`] renders it.
    pub(crate) label: SharedString,
}

fn resolve_hints(chain: &[&'static str], limit: usize) -> Rc<[Hint]> {
    let mut seen: Vec<&'static str> = Vec::new();
    let mut hints = Vec::new();
    for spec in bindings_for_chain(chain).iter().rev() {
        if seen.contains(&spec.action) {
            continue;
        }
        seen.push(spec.action);
        hints.push(Hint {
            keys: pretty_keys(spec.keys).into(),
            label: humanize(spec.action).into(),
        });
        if hints.len() >= limit {
            break;
        }
    }
    hints.into()
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
        assert!(hints.iter().any(|hint| hint.label == "enter"));
    }

    #[test]
    fn repeating_a_chain_reuses_the_resolved_hints_and_bindings() {
        let first = hints_for_chain(&["Panels", "Files"], 8);
        assert!(Rc::ptr_eq(
            &first,
            &hints_for_chain(&["Panels", "Files"], 8)
        ));
        assert!(!Rc::ptr_eq(
            &first,
            &hints_for_chain(&["Panels", "Commits"], 8)
        ));

        let bindings = bindings_for_chain(&["Panels", "Files"]);
        assert!(Rc::ptr_eq(
            &bindings,
            &bindings_for_chain(&["Panels", "Files"])
        ));
        assert!(!Rc::ptr_eq(
            &bindings,
            &bindings_for_chain(&["Panels", "Commits"])
        ));
    }

    #[test]
    fn humanize_and_pretty_keys_format_for_the_hint_bar() {
        assert_eq!(humanize("files::ToggleStaged"), "toggle staged");
        assert_eq!(pretty_keys("ctrl-d"), "^d");
        assert_eq!(pretty_keys("g g"), "g g");
        assert_eq!(pretty_keys("shift-tab"), "S-\u{21e5}");
        assert_eq!(pretty_keys("backspace"), "\u{232b}");
        assert_eq!(pretty_keys("space"), "\u{2423}");
        assert_eq!(pretty_keys("-"), "-");
    }
}
