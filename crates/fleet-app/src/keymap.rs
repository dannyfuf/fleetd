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
//! A native agent tab spells its prefix as two-keystroke rows (`ctrl-s s`) rather than as a
//! context, because its chain is derived from daemon state and has no room for a one-shot context. Those
//! rows are still never matched by GPUI: the shell's keystroke interceptor takes `ctrl-s` and the
//! key after it, and resolves the pair here through [`chord_action_for_chain`]. GPUI replays the
//! keystrokes of a sequence that matched nothing as *input*, so leaving the chord to it typed a
//! stray character into the composer on every unbound `ctrl-s <key>`. The rows remain in the table
//! because the table is what the Help overlay, the palette hints and `docs/KEYMAP.md` are read
//! from — and what the resolver itself walks.
//!
//! # Deliberate deviations
//!
//! * `docs/KEYMAP.md` lists `q` as "close" in Palette mode. A `q` binding there would make the
//!   query untypable, because bindings outrank the text input. Only `Esc` closes the palette;
//!   see `docs/APP-CONTRACTS.md`.

use fleet_ui_kit::{menu_actions, text_input};
use gpui::{Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Keystroke};

use crate::actions::fleet::Cancel;
use crate::actions::{
    agent, board, board_settings, card_detail, confirm, context_dialog, create_worktree, daemon,
    dialog, filter, first_run,
    fleet::{
        DismissStickyError, FocusStickyError, OpenAgentClaude, OpenAgentCodex, OpenHelp, OpenJobs,
        OpenPalette, OpenSettings, Quit, QuitAndStopDaemon, Refresh, UpdateFleet,
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
                for (spec, action) in shared_rows() {
                    match parse_binding(spec, action) {
                        Ok(binding) => bindings.push((spec, binding)),
                        Err(error) => tracing::error!(keys = spec.keys, context = spec.context, %error, "invalid built-in key binding"),
                    }
                }
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
            TABLE.get_or_init(|| {
                let mut table = vec![$( BindingSpec {
                    keys: $keys,
                    context: $context,
                    action: Action::name(&$action),
                } ),*];
                table.extend(shared_rows().into_iter().map(|(spec, _)| spec));
                table
            })
        }

        /// Resolves a `^s <key>` chord against one **live** key-context chain.
        ///
        /// A native agent tab draws a text composer, and GPUI replays the keystrokes of a
        /// sequence that matched nothing as *input* (`Window::replay_pending_input`), so an
        /// unbound `^s <key>` used to type a stray character into the draft. The shell's
        /// keystroke interceptor therefore takes the whole chord before GPUI's own two-key
        /// matcher sees it, and resolves the second key here — against the chain the state
        /// says is live, not the one the last frame painted.
        ///
        /// Context matching follows gpui's own rule: `>` is a **subsequence** of the chain, and
        /// the deepest match wins, with the later row winning a tie exactly as `cx.bind_keys`
        /// would. The walk is O(rows) and allocates only for the answer.
        #[must_use]
        pub fn chord_action_for_chain(
            chain: &[&str],
            keystroke: &Keystroke,
        ) -> Option<Box<dyn Action>> {
            PARSED_BINDINGS.with(|bindings| {
                let mut best: Option<(usize, &KeyBinding)> = None;
                for (spec, binding) in bindings {
                    let keystrokes = binding.keystrokes();
                    if keystrokes.len() != 2
                        || !is_prefix_key(keystrokes[0].inner())
                        || !keystroke.should_match(&keystrokes[1])
                    {
                        continue;
                    }
                    let Some(depth) = context_depth(spec.context, chain) else {
                        continue;
                    };
                    if best.is_none_or(|(deepest, _)| depth >= deepest) {
                        best = Some((depth, binding));
                    }
                }
                best.map(|(_, binding)| binding.action().boxed_clone())
            })
        }

        /// Resolves one keystroke against a live context chain using gpui's depth rule.
        #[cfg(test)]
        #[must_use]
        pub fn action_for_chain(
            chain: &[&str],
            keystroke: &Keystroke,
        ) -> Option<Box<dyn Action>> {
            PARSED_BINDINGS.with(|bindings| {
                let mut best: Option<(usize, &KeyBinding)> = None;
                for (spec, binding) in bindings {
                    let keystrokes = binding.keystrokes();
                    if keystrokes.len() != 1 || !keystroke.should_match(&keystrokes[0]) {
                        continue;
                    }
                    let Some(depth) = context_depth(spec.context, chain) else {
                        continue;
                    };
                    if best.is_none_or(|(deepest, _)| depth >= deepest) {
                        best = Some((depth, binding));
                    }
                }
                best.map(|(_, binding)| binding.action().boxed_clone())
            })
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

/// One row of a table that is registered against more than one key context.
///
/// The action is a constructor rather than a value because a `Box<dyn Action>` cannot live in a
/// `const`; the table stays readable data either way.
type SharedRow = (&'static str, fn() -> Box<dyn Action>);

/// The six native agent-thread key contexts of `docs/KEYMAP.md` § *Native agent thread*.
///
/// gpui matches `>` as a subsequence of the rendered chain rather than as a parent test, and
/// none of these six is on another's chain, so a row that must fire in an agent tab whatever the
/// thread is doing has to name every one of them.
const AGENT_THREAD_CONTEXTS: &[&str] = &[
    "Agent > AgentIdle",
    "Agent > AgentWorking",
    "Agent > AgentNativeScroll",
    "Agent > AgentDecision > AgentPermission",
    "Agent > AgentDecision > AgentQuestion",
    "Agent > AgentDecision > AgentPlan",
];

/// The contexts published while the live composer is actively editing.
///
/// Decision contexts still draw the composer, but publish their browsing word only while the
/// gate owns bare answer keys. Once a gate draft is being composed, `agent_context_chain`
/// publishes `AgentIdle` or `AgentWorking` instead.
#[cfg(test)]
const AGENT_COMPOSER_CONTEXTS: &[&str] = &["Agent > AgentIdle", "Agent > AgentWorking"];

/// Every context that keeps the composer's `^s` controls available.
///
/// `AgentNativeScroll` keeps only the three escapes its own block binds: a frozen tail has no
/// model to change, no traits menu and no access mode, and §12 gives it `q`/`i`/`esc` to leave.
const AGENT_CONTROL_CONTEXTS: &[&str] = &[
    "Agent > AgentIdle",
    "Agent > AgentWorking",
    "Agent > AgentDecision > AgentPermission",
    "Agent > AgentDecision > AgentQuestion",
    "Agent > AgentDecision > AgentPlan",
];

/// Workspace tab selection and the two MRU jumps, as `^s` chords.
const AGENT_SELECTION_ROWS: &[SharedRow] = &[
    ("ctrl-s 1", || Box::new(native_agent::SelectTab1)),
    ("ctrl-s 2", || Box::new(native_agent::SelectTab2)),
    ("ctrl-s 3", || Box::new(native_agent::SelectTab3)),
    ("ctrl-s 4", || Box::new(native_agent::SelectTab4)),
    ("ctrl-s 5", || Box::new(native_agent::SelectTab5)),
    ("ctrl-s 6", || Box::new(native_agent::SelectTab6)),
    ("ctrl-s 7", || Box::new(native_agent::SelectTab7)),
    ("ctrl-s 8", || Box::new(native_agent::SelectTab8)),
    ("ctrl-s 9", || Box::new(native_agent::SelectTab9)),
    ("ctrl-s tab", || Box::new(native_agent::LastTab)),
    ("ctrl-s w", || Box::new(native_agent::LastSession)),
];

/// The session-level rows of the Workspace prefix table, repeated inside an agent tab.
///
/// An agent thread is the Workspace's selected tab, so its context chain *replaces*
/// `Workspace > …` rather than covering it; without these rows the session commands were dead
/// keys in an agent tab and `^s s` typed an `s` into the composer. Every handler already lives
/// on an ancestor of the agent view — `prefix::GoHub` on the `Fleet` root, the rest on the
/// Workspace root — so the rows dispatch with no handler moved.
///
/// `r` (restart), `,` (rename), `]` (paste) and `^s ^s` (send a literal) stay out: each one
/// addresses a PTY, and there is none behind a Fleet-drawn tab.
const AGENT_SESSION_ROWS: &[SharedRow] = &[
    ("ctrl-s s", || Box::new(prefix::GoHub)),
    ("ctrl-s S", || Box::new(prefix::SleepAndGoHub)),
    ("ctrl-s h", || Box::new(prefix::PrevTab)),
    ("ctrl-s p", || Box::new(prefix::PrevTab)),
    ("ctrl-s l", || Box::new(prefix::NextTab)),
    ("ctrl-s n", || Box::new(prefix::NextTab)),
    ("ctrl-s W", || Box::new(prefix::SessionSwitcher)),
    ("ctrl-s u", || Box::new(prefix::UpToCaller)),
    ("ctrl-s d", || Box::new(prefix::AgentsPicker)),
    ("ctrl-s c", || Box::new(prefix::NewTerminal)),
    ("ctrl-s b", || Box::new(prefix::OpenBoard)),
    ("ctrl-s y", || Box::new(prefix::CopyWorktreePath)),
    ("ctrl-s z", || Box::new(prefix::ToggleZoom)),
    ("ctrl-s v", || Box::new(prefix::ToggleWatchPane)),
    ("ctrl-s g", || Box::new(prefix::ToggleChanges)),
    ("ctrl-s V", || Box::new(prefix::DismissWatch)),
    ("ctrl-s N", || Box::new(prefix::NextWatch)),
    ("ctrl-s P", || Box::new(prefix::PrevWatch)),
    ("ctrl-s !", || Box::new(FocusStickyError)),
    ("ctrl-s J", || Box::new(OpenJobs)),
    ("ctrl-s ?", || Box::new(OpenHelp)),
    ("ctrl-s k", || Box::new(OpenPalette)),
    ("ctrl-s escape", || Box::new(prefix::Cancel)),
];

/// The thread controls §12 gives every mode that still owns a composer.
///
/// §1 keeps the terminal path "as an explicit fallback, now on `^s F`" and §3.3 rule 4 lets an
/// unanswered gate outlive its turn: without these rows on the decision contexts the escape
/// hatches and every control are dead keys for as long as a card is open.
const AGENT_CONTROL_ROWS: &[SharedRow] = &[
    ("ctrl-s m", || Box::new(native_agent::Model)),
    ("ctrl-s e", || Box::new(native_agent::Traits)),
    ("ctrl-s t", || Box::new(native_agent::AccessMode)),
    ("ctrl-s [", || Box::new(native_agent::Scroll)),
    ("ctrl-s a", || Box::new(native_agent::NewClaude)),
    ("ctrl-s A", || Box::new(native_agent::NewCodex)),
    ("ctrl-s x", || Box::new(native_agent::CloseTab)),
    ("ctrl-s F", || Box::new(native_agent::TerminalFallback)),
];

/// The palette's second key beside `:`: ⌘K on macOS, `ctrl-k` everywhere else (KEYMAP.md
/// § Palette mode). It is bound only where `:` is, so a focused text field keeps its own
/// `ctrl-k` (delete to the line end): `FleetTextInput` sits deeper on the chain and wins.
const PALETTE_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-k"
} else {
    "ctrl-k"
};

/// Every context where `:` opens the palette, and so where [`PALETTE_KEY`] does too.
const PALETTE_KEY_CONTEXTS: &[&str] = &["Hub", "Dialog > CardDetail"];

/// [`PALETTE_KEY`] as a row.
const PALETTE_KEY_ROWS: &[SharedRow] = &[(PALETTE_KEY, || Box::new(OpenPalette))];

/// The Workspace's terminal and native tabs and the agent thread: `ctrl-k` belongs to the
/// shell and the composer there, so only macOS, whose ⌘K no terminal program sees, binds it.
/// Every platform also has `ctrl-s k` (the prefix table and [`AGENT_SESSION_ROWS`]).
const WORKSPACE_PALETTE_CONTEXTS: &[&str] = &[
    "Workspace > Terminal",
    "Workspace > Native",
    "Agent > AgentIdle",
    "Agent > AgentWorking",
    "Agent > AgentNativeScroll",
    "Agent > AgentDecision > AgentPermission",
    "Agent > AgentDecision > AgentQuestion",
    "Agent > AgentDecision > AgentPlan",
];

/// ⌘K on macOS; nothing elsewhere.
const WORKSPACE_PALETTE_ROWS: &[SharedRow] = if cfg!(target_os = "macos") {
    PALETTE_KEY_ROWS
} else {
    &[]
};

/// The context × row products the table registers after its literal rows, in that order: every
/// row of a family is registered under each context of that family.
const SHARED_TABLES: &[(&[&str], &[SharedRow])] = &[
    (AGENT_THREAD_CONTEXTS, AGENT_SELECTION_ROWS),
    (AGENT_THREAD_CONTEXTS, AGENT_SESSION_ROWS),
    (AGENT_CONTROL_CONTEXTS, AGENT_CONTROL_ROWS),
    (PALETTE_KEY_CONTEXTS, PALETTE_KEY_ROWS),
    (WORKSPACE_PALETTE_CONTEXTS, WORKSPACE_PALETTE_ROWS),
];

/// Every `(spec, action)` pair [`SHARED_TABLES`] stands for, built once per consumer.
fn shared_rows() -> Vec<(BindingSpec, Box<dyn Action>)> {
    let mut rows = Vec::new();
    for (contexts, table) in SHARED_TABLES {
        for context in *contexts {
            for (keys, action) in *table {
                let action = action();
                rows.push((
                    BindingSpec {
                        keys,
                        context,
                        action: action.name(),
                    },
                    action,
                ));
            }
        }
    }
    rows
}

/// Whether a keystroke is the `^s` prefix itself.
///
/// Recognised by shape rather than by a table lookup: the agent-thread contexts bind no bare
/// `ctrl-s` row for a lookup to find, and the shell's interceptor has to know the prefix before
/// it can decide whether a chord is starting.
#[must_use]
pub fn is_prefix_key(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    keystroke.key == "s"
        && modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.platform
        && !modifiers.function
}

/// The keystrokes of the first row binding `action` in exactly `context`.
///
/// For a control whose chip must name a key of a context the focus is not in right now: the
/// Workspace's chrome shows `⌃S ?` for Help, and `?` is bound in `Workspace > Prefix`, a context
/// the window is in for one key only. [`fleet_ui_kit::Kbd::for_action`] resolves against the
/// focused context and would find nothing there.
#[must_use]
pub fn keystrokes_in(context: &str, action: &dyn Action) -> Option<Vec<Keystroke>> {
    let name = action.name();
    let spec = cached_table()
        .iter()
        .find(|spec| spec.context == context && spec.action == name)?;
    parse_keys(spec)
}

/// The keystrokes of the **last** row binding `action` in exactly `context`: the one gpui ranks
/// highest there, since a later row wins a tie. For a chip that must name the key a live lookup
/// would have named, without depending on focus — the command field's `⌘K` beside the Hub's `:`.
#[must_use]
pub fn highest_keystrokes_in(context: &str, action: &dyn Action) -> Option<Vec<Keystroke>> {
    let name = action.name();
    let spec = cached_table()
        .iter()
        .rev()
        .find(|spec| spec.context == context && spec.action == name)?;
    parse_keys(spec)
}

/// One row's keys as keystrokes; an unparsable row is logged and gives `None`.
fn parse_keys(spec: &BindingSpec) -> Option<Vec<Keystroke>> {
    spec.keys
        .split_whitespace()
        .map(Keystroke::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| tracing::error!(keys = spec.keys, %error, "unparsable key table row"))
        .ok()
}

/// How deep a context predicate matches a live chain, or `None` if it does not match.
///
/// gpui reads `A > B` as "B somewhere below A", not "B's parent is A", so the words are matched
/// as a subsequence and the answer is the chain index the last word landed on — which is the
/// depth `cx.bind_keys` ranks competing rows by.
pub(crate) fn context_depth(context: &str, chain: &[&str]) -> Option<usize> {
    let mut index = 0;
    let mut depth = 0;
    // The live chain carries identifiers but not attributes. Attribute predicates can never be
    // prefix rows (they bind one key), and for subsequence resolution their identifier is the
    // satisfiable part: `FleetTextInput && mode == multiline` matches `FleetTextInput`.
    let identifiers = context.split_once(" && ").map_or(context, |(head, _)| head);
    for word in identifiers.split(" > ") {
        let found = chain[index..].iter().position(|link| *link == word)?;
        depth = index + found;
        index = depth + 1;
    }
    Some(depth)
}

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
    // `A`, `X` and `>` are deliberately absent here: the Hub draws a context board, which can
    // never hold a run, and a key whose only answer is a refusal is an affordance that does
    // nothing (contracts §5.5). `b` and `m` edit card fields every board has, so they stay —
    // and while the board owns the keyboard they shadow the Hub's own `b` (§6 arbitration).
    "b", "Hub > Board" => board::PickBlockedBy;
    "m", "Hub > Board" => board::PickAgent;
    "C", "Hub > Board" => board::Columns;
    // Not `S` and `F`, which the schedules strip would suggest: those are sync and full sync.
    "T", "Hub > Board" => board::Schedules;
    "R", "Hub > Board" => board::RunSchedules;

    // BOARD §8: the `fleet://board` tab draws the very same board with the very same keys, so
    // every `Hub > Board` row above is repeated here against the same action. The word sits
    // under `Workspace > Native`, which keeps `ctrl-s` — the prefix is never a board key.
    "h", "Workspace > Native > Board" => board::PrevColumn;
    "left", "Workspace > Native > Board" => board::PrevColumn;
    "l", "Workspace > Native > Board" => board::NextColumn;
    "right", "Workspace > Native > Board" => board::NextColumn;
    "j", "Workspace > Native > Board" => board::NextCard;
    "down", "Workspace > Native > Board" => board::NextCard;
    "k", "Workspace > Native > Board" => board::PrevCard;
    "up", "Workspace > Native > Board" => board::PrevCard;
    "enter", "Workspace > Native > Board" => board::OpenCard;
    "c", "Workspace > Native > Board" => board::NewCard;
    "s", "Workspace > Native > Board" => board::PickStatus;
    "p", "Workspace > Native > Board" => board::PickPriority;
    "a", "Workspace > Native > Board" => board::PickAssignee;
    "t", "Workspace > Native > Board" => board::PickLabels;
    "e", "Workspace > Native > Board" => board::PickEstimate;
    "[", "Workspace > Native > Board" => board::MovePrevColumn;
    "]", "Workspace > Native > Board" => board::MoveNextColumn;
    "w", "Workspace > Native > Board" => board::CreateWorktree;
    "o", "Workspace > Native > Board" => board::OpenWorktree;
    "S", "Workspace > Native > Board" => board::Sync;
    "F", "Workspace > Native > Board" => board::FullSync;
    "x", "Workspace > Native > Board" => board::OpenRemote;
    "d", "Workspace > Native > Board" => board::DeleteCard;
    ",", "Workspace > Native > Board" => board::Settings;
    "r", "Workspace > Native > Board" => board::Reload;
    "/", "Workspace > Native > Board" => board::Filter;
    "A", "Workspace > Native > Board" => board::AttachRun;
    "X", "Workspace > Native > Board" => board::CancelRun;
    ">", "Workspace > Native > Board" => board::RunNow;
    "b", "Workspace > Native > Board" => board::PickBlockedBy;
    "m", "Workspace > Native > Board" => board::PickAgent;
    "C", "Workspace > Native > Board" => board::Columns;
    "T", "Workspace > Native > Board" => board::Schedules;
    "R", "Workspace > Native > Board" => board::RunSchedules;

    // The Review tab of the PR screen draws the context's Reviews board (KEYMAP § Hub › Pull
    // requests › Review board). The word nests under `Hub > Prs`, so `Hub > Board`'s rows match
    // it too at the same depth; these rows are registered after them and win every tie. The
    // differences: `tab` / `shift-tab` stay the screen's tab keys, `p` / `q` stay back, the run
    // keys are bound because a review card runs in its own pull-request worktree, and `B` / `y`
    // act on the card's pull request.
    "tab", "Hub > Prs > Board" => prs::NextTab;
    "shift-tab", "Hub > Prs > Board" => prs::PrevTab;
    "p", "Hub > Prs > Board" => prs::Back;
    "q", "Hub > Prs > Board" => prs::Back;
    "h", "Hub > Prs > Board" => board::PrevColumn;
    "left", "Hub > Prs > Board" => board::PrevColumn;
    "l", "Hub > Prs > Board" => board::NextColumn;
    "right", "Hub > Prs > Board" => board::NextColumn;
    "j", "Hub > Prs > Board" => board::NextCard;
    "down", "Hub > Prs > Board" => board::NextCard;
    "k", "Hub > Prs > Board" => board::PrevCard;
    "up", "Hub > Prs > Board" => board::PrevCard;
    "enter", "Hub > Prs > Board" => board::OpenCard;
    "c", "Hub > Prs > Board" => board::NewCard;
    "s", "Hub > Prs > Board" => board::PickStatus;
    "a", "Hub > Prs > Board" => board::PickAssignee;
    "t", "Hub > Prs > Board" => board::PickLabels;
    "e", "Hub > Prs > Board" => board::PickEstimate;
    "[", "Hub > Prs > Board" => board::MovePrevColumn;
    "]", "Hub > Prs > Board" => board::MoveNextColumn;
    "w", "Hub > Prs > Board" => board::CreateWorktree;
    "o", "Hub > Prs > Board" => board::OpenWorktree;
    "S", "Hub > Prs > Board" => board::Sync;
    "F", "Hub > Prs > Board" => board::FullSync;
    "x", "Hub > Prs > Board" => board::OpenRemote;
    "d", "Hub > Prs > Board" => board::DeleteCard;
    ",", "Hub > Prs > Board" => board::Settings;
    "r", "Hub > Prs > Board" => board::Reload;
    "/", "Hub > Prs > Board" => board::Filter;
    "A", "Hub > Prs > Board" => board::AttachRun;
    "X", "Hub > Prs > Board" => board::CancelRun;
    ">", "Hub > Prs > Board" => board::RunNow;
    "b", "Hub > Prs > Board" => board::PickBlockedBy;
    "m", "Hub > Prs > Board" => board::PickAgent;
    "C", "Hub > Prs > Board" => board::Columns;
    "B", "Hub > Prs > Board" => board::OpenPullRequest;
    "y", "Hub > Prs > Board" => board::CopyPullRequestUrl;
    "T", "Hub > Prs > Board" => board::Schedules;
    "R", "Hub > Prs > Board" => board::RunSchedules;
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
    "A", "Dialog > CardDetail" => board::AttachRun;
    "X", "Dialog > CardDetail" => board::CancelRun;
    ">", "Dialog > CardDetail" => board::RunNow;
    "b", "Dialog > CardDetail" => board::PickBlockedBy;
    "m", "Dialog > CardDetail" => board::PickAgent;
    // The board's field keys, answered on the card on show: each selects the property row it
    // edits and opens it, exactly as a click on that row does (UX-SPEC § Card detail).
    "s", "Dialog > CardDetail" => board::PickStatus;
    "p", "Dialog > CardDetail" => board::PickPriority;
    "a", "Dialog > CardDetail" => board::PickAssignee;
    "t", "Dialog > CardDetail" => board::PickLabels;
    "e", "Dialog > CardDetail" => board::PickEstimate;
    "o", "Dialog > CardDetail" => board::OpenWorktree;
    // `B`, not `b`: `b` is *blocked by* on every board surface.
    "B", "Dialog > CardDetail" => board::OpenPullRequest;
    "y", "Dialog > CardDetail" => board::CopyPullRequestUrl;
    // The palette replaces the dialog it is opened over and remembers which one it was, so the
    // `Card detail:` rows can save or cancel an edit already typed instead of reseeding one over
    // it. Without a way in from the detail those rows can never be listed and that path is dead.
    ":",      "Dialog > CardDetail" => OpenPalette;
    "escape", "Dialog > CardDetailEditing" => card_detail::Close;
    "enter", "Dialog > CardDetailEditing" => card_detail::EditProperty;
    "ctrl-s", "Dialog > CardDetailEditing" => card_detail::Save;
    "ctrl-enter", "Dialog > CardDetailEditing" => card_detail::Save;
    "ctrl-enter", "Dialog > CardCreate" => board::CreateAndOpen;
    "space", "Dialog > CardPicker" => settings::Toggle;
    "j", "Dialog > BoardSettings" => settings::MoveDown;
    "k", "Dialog > BoardSettings" => settings::MoveUp;
    "h", "Dialog > BoardSettings" => settings::CyclePrev;
    "l", "Dialog > BoardSettings" => settings::CycleNext;
    // The arrows cycle only while browsing, exactly as they do in the global Settings dialog:
    // a materialized row publishes `BoardSettingsEditing`, this row leaves the chain, and
    // `FleetTextInput` moves the caret instead.
    "left", "Dialog > BoardSettings" => settings::CyclePrev;
    "right", "Dialog > BoardSettings" => settings::CycleNext;
    "space", "Dialog > BoardSettings" => settings::Toggle;
    // The Columns list's five verbs. They are plain letters because the pane they act on has
    // no editor open: a row being typed into publishes `BoardSettingsEditing`, where these
    // rows are not bound and the letter is the letter.
    "n", "Dialog > BoardSettings" => board_settings::NewColumn;
    "d", "Dialog > BoardSettings" => board_settings::DeleteColumn;
    "J", "Dialog > BoardSettings" => board_settings::MoveColumnDown;
    "K", "Dialog > BoardSettings" => board_settings::MoveColumnUp;
    "P", "Dialog > BoardSettings" => board_settings::ApplyPreset;
    "r", "Dialog > BoardSettings" => board_settings::RunScheduleNow;
    // §5.4 moved the save off `enter`, which the Columns pane needs for drilling in, so it is
    // bound in both contexts: a column row being edited saves without being left first.
    "ctrl-s", "Dialog > BoardSettings" => board_settings::Save;
    "ctrl-s", "Dialog > BoardSettingsEditing" => board_settings::Save;
    "enter", "Dialog > BoardSettingsEditing" => dialog::Confirm;

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
    "X",            "Hub" => DismissStickyError;
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
    "u",            "Workspace > Prefix" => prefix::UpToCaller;
    "d",            "Workspace > Prefix" => prefix::AgentsPicker;
    "c",            "Workspace > Prefix" => prefix::NewTerminal;
    // The board tab is created on demand rather than by `windows[]` (BOARD §8), so this one
    // key both makes it and returns to it; pressing it twice is one tab, selected twice.
    "b",            "Workspace > Prefix" => prefix::OpenBoard;
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
    "g",            "Workspace > Prefix" => prefix::ToggleChanges;
    "V",            "Workspace > Prefix" => prefix::DismissWatch;
    "N",            "Workspace > Prefix" => prefix::NextWatch;
    "P",            "Workspace > Prefix" => prefix::PrevWatch;
    "!",            "Workspace > Prefix" => FocusStickyError;
    "J",            "Workspace > Prefix" => OpenJobs;
    "?",            "Workspace > Prefix" => OpenHelp;
    // The palette's Workspace key: `ctrl-k` is the shell's here (KEYMAP.md § Palette mode).
    "k",            "Workspace > Prefix" => OpenPalette;
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
    // every `^s` row has to be registered against each sub-mode by name — `AgentIdle`/
    // `AgentWorking` are not on the `AgentDecision > *` chain, and none of them is on another's.
    // That product lives in [`SHARED_TABLES`] below rather than as a hand-copied block per
    // context. **Row focus lives inside scroll mode**, which is why `AgentRow` is only ever
    // entered under `AgentNativeScroll`. And `/`, `@`, `$`, `⇧⏎` and `esc`-on-idle are
    // deliberately **unbound**: the composer inserts the character and reports it, which is what
    // keeps all three triggers typable and lets an IME preedit and a selection cancel before the
    // `esc` cascade runs.
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

    "escape",        "Agent > AgentWorking" => native_agent::Stop;
    "enter",         "Agent > AgentWorking" => native_agent::Steer;
    "cmd-enter",     "Agent > AgentWorking" => native_agent::Steer;
    "shift-tab",     "Agent > AgentWorking" => native_agent::PlanMode;
    "up",            "Agent > AgentWorking" => native_agent::History;
    "ctrl-p",        "Agent > AgentWorking" => native_agent::History;
    "down",          "Agent > AgentWorking" => native_agent::HistoryNext;
    "ctrl-n",        "Agent > AgentWorking" => native_agent::HistoryNext;

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

    // Row focus: entered only under `AgentNativeScroll`, which is what finally makes these six
    // fire and retires the long-standing caveat that `Agent > AgentRow` was bound, handled and
    // never entered.
    "enter",         "Agent > AgentNativeScroll > AgentRow" => native_agent::ExpandRow;
    "u",             "Agent > AgentNativeScroll > AgentRow" => native_agent::Revert;
    "o",             "Agent > AgentNativeScroll > AgentRow" => native_agent::OpenInEditor;
    "y",             "Agent > AgentNativeScroll > AgentRow" => native_agent::CopyRow;
    "d",             "Agent > AgentNativeScroll > AgentRow" => native_agent::DiffRow;
    "x",             "Agent > AgentNativeScroll > AgentRow" => native_agent::CancelDelegation;

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

    // An open kit menu (ADR 0023): navigated like a list under a text field, so never `j`/`k`.
    "down",   "FleetMenu" => menu_actions::SelectNext;
    "ctrl-n", "FleetMenu" => menu_actions::SelectNext;
    "up",     "FleetMenu" => menu_actions::SelectPrevious;
    "ctrl-p", "FleetMenu" => menu_actions::SelectPrevious;
    "enter",  "FleetMenu" => menu_actions::Confirm;
    "escape", "FleetMenu" => menu_actions::Cancel;

    "left", "FleetTextInput" => text_input::MoveLeft;
    "right", "FleetTextInput" => text_input::MoveRight;
    "alt-left", "FleetTextInput" => text_input::MoveWordLeft;
    "alt-right", "FleetTextInput" => text_input::MoveWordRight;
    "home", "FleetTextInput" => text_input::MoveToRowStart;
    "end", "FleetTextInput" => text_input::MoveToRowEnd;
    "cmd-left", "FleetTextInput" => text_input::MoveToLineStart;
    "cmd-right", "FleetTextInput" => text_input::MoveToLineEnd;
    "up", "FleetTextInput" => text_input::MoveUp;
    "down", "FleetTextInput" => text_input::MoveDown;
    "cmd-up", "FleetTextInput" => text_input::MoveToStart;
    "cmd-down", "FleetTextInput" => text_input::MoveToEnd;
    "shift-left", "FleetTextInput" => text_input::SelectLeft;
    "shift-right", "FleetTextInput" => text_input::SelectRight;
    "alt-shift-left", "FleetTextInput" => text_input::SelectWordLeft;
    "alt-shift-right", "FleetTextInput" => text_input::SelectWordRight;
    "shift-home", "FleetTextInput" => text_input::SelectToRowStart;
    "shift-end", "FleetTextInput" => text_input::SelectToRowEnd;
    "cmd-shift-left", "FleetTextInput" => text_input::SelectToLineStart;
    "cmd-shift-right", "FleetTextInput" => text_input::SelectToLineEnd;
    "shift-up", "FleetTextInput" => text_input::SelectUp;
    "shift-down", "FleetTextInput" => text_input::SelectDown;
    "cmd-shift-up", "FleetTextInput" => text_input::SelectToStart;
    "cmd-shift-down", "FleetTextInput" => text_input::SelectToEnd;
    "ctrl-a", "FleetTextInput" => text_input::MoveToLineStart;
    "ctrl-e", "FleetTextInput" => text_input::MoveToLineEnd;
    "ctrl-shift-a", "FleetTextInput" => text_input::SelectToLineStart;
    "ctrl-shift-e", "FleetTextInput" => text_input::SelectToLineEnd;
    "ctrl-b", "FleetTextInput" => text_input::MoveLeft;
    "ctrl-f", "FleetTextInput" => text_input::MoveRight;
    "backspace", "FleetTextInput" => text_input::Backspace;
    "delete", "FleetTextInput" => text_input::Delete;
    "alt-backspace", "FleetTextInput" => text_input::DeleteWordBackward;
    "alt-delete", "FleetTextInput" => text_input::DeleteWordForward;
    "cmd-backspace", "FleetTextInput" => text_input::DeleteToLineStart;
    "cmd-delete", "FleetTextInput" => text_input::DeleteToLineEnd;
    "ctrl-w", "FleetTextInput" => text_input::DeleteWordBackward;
    "ctrl-u", "FleetTextInput" => text_input::DeleteToLineStart;
    "ctrl-k", "FleetTextInput" => text_input::DeleteToLineEnd;
    "ctrl-h", "FleetTextInput" => text_input::Backspace;
    "ctrl-d", "FleetTextInput" => text_input::Delete;
    "cmd-a", "FleetTextInput" => text_input::SelectAll;
    "cmd-c", "FleetTextInput" => text_input::Copy;
    "cmd-x", "FleetTextInput" => text_input::Cut;
    "cmd-v", "FleetTextInput" => text_input::Paste;
    "cmd-z", "FleetTextInput" => text_input::Undo;
    "cmd-shift-z", "FleetTextInput" => text_input::Redo;
    "enter", "FleetTextInput && mode == multiline && enter == newline" => text_input::Newline;
    "shift-enter", "FleetTextInput && mode == multiline" => text_input::Newline;

    "enter",        "Filter" => filter::Accept;
    "escape",       "Filter" => filter::Escape;
    "ctrl-n",       "Filter" => filter::CursorDown;
    "down",         "Filter" => filter::CursorDown;
    "ctrl-p",       "Filter" => filter::CursorUp;
    "up",           "Filter" => filter::CursorUp;

    "shift-tab", "Filter > BoardFilter" => board::PrevColumn;
    "tab", "Filter > BoardFilter" => board::NextColumn;

    "enter",        "Palette" => palette::Run;
    "escape",       "Palette" => palette::Close;
    "ctrl-n",       "Palette" => palette::CursorDown;
    "down",         "Palette" => palette::CursorDown;
    "ctrl-p",       "Palette" => palette::CursorUp;
    "up",           "Palette" => palette::CursorUp;

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

    "left",         "Dialog > Create" => create_worktree::HostPrev;
    "right",        "Dialog > Create" => create_worktree::HostNext;
    "alt-enter",    "Dialog > Create" => create_worktree::CreateWithoutOpening;
    "alt-enter",    "Dialog > CreateEditing" => create_worktree::CreateWithoutOpening;

    // `enter` first: the later row is the higher-precedence binding, and that is the one the
    // action button's key chip shows — the confirm teaches `y`, and `enter` still works.
    "enter",        "Dialog > Confirm" => confirm::Accept;
    "y",            "Dialog > Confirm" => confirm::Accept;
    "Y",            "Dialog > Confirm" => confirm::AcceptStrong;
    "n",            "Dialog > Confirm" => confirm::Reject;
    "q",            "Dialog > Confirm" => confirm::Reject;
    "escape",       "Dialog > Confirm" => confirm::Reject;
    "I",            "Dialog > Confirm" => confirm::Recheck;
    "s",            "Dialog > Confirm" => confirm::ToggleKeep;

    // `ctrl-d` is `FleetTextInput`'s delete-forward, and this dialog is two live editors with
    // no browsing state between them: the deeper input would win every time. The stronger
    // shift variant collides with nothing the editor owns, and matches the Hub, where the
    // same delete is the uppercase `D`.
    "ctrl-shift-d", "Dialog > Context" => context_dialog::Delete;

    "j",            "Dialog > Assign" => dialog::CursorDown;
    "k",            "Dialog > Assign" => dialog::CursorUp;

    "space",        "Dialog > Settings" => settings::Toggle;
    "h",            "Dialog > Settings" => settings::CyclePrev;
    "l",            "Dialog > Settings" => settings::CycleNext;
    // The arrows cycle only while browsing: a materialized row publishes `SettingsEditing`,
    // this row leaves the chain with `Settings`, and `FleetTextInput` moves the caret instead.
    "left",         "Dialog > Settings" => settings::CyclePrev;
    "right",        "Dialog > Settings" => settings::CycleNext;
    "j",            "Dialog > Settings" => settings::MoveDown;
    "k",            "Dialog > Settings" => settings::MoveUp;
    "E",            "Dialog > Settings" => settings::OpenConfigFile;
    "D",            "Dialog > Settings" => settings::RunDoctor;
    "/",            "Dialog > Settings" => settings::Search;
    "enter",        "Dialog > SettingsEditing" => dialog::Confirm;
    // The header's search field owns the printable keys; `↓`/`↑` move over its hits through
    // the `Dialog` rows, `Enter` jumps to the selected one and `Esc` clears it rather than
    // discarding the dialog.
    "enter",        "Dialog > SettingsSearch" => dialog::Confirm;
    "escape",       "Dialog > SettingsSearch" => settings::EndSearch;

    "escape",       "Dialog > Help" => help::Close;
    "?",            "Dialog > Help" => help::Close;
    "ctrl-tab",     "Dialog > Help" => help::SwitchTab;
    "ctrl-shift-tab", "Dialog > Help" => help::SwitchTab;

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
        "Hub > Prs > Board",
        "Dialog > CardDetail",
        "Dialog > CardDetailEditing",
        "Dialog > CardCreate",
        "Dialog > CardPicker",
        "Dialog > BoardSettings",
        "Dialog > BoardSettingsEditing",
        "Workspace > Terminal",
        "Workspace > Native",
        "Workspace > Native > Board",
        "Workspace > Prefix",
        "Workspace > Scroll",
        // `Agent` itself binds nothing: gpui evaluates a bare identifier against every node of
        // the dispatch path, so an `Agent` binding also fires inside a native agent tab, where
        // the popup is not mounted. Every popup key lives on `Agent > Terminal` / `> Prefix`.
        "Agent > Terminal",
        "Agent > Prefix",
        "Agent > Scroll",
        AGENT_COMPOSER_CONTEXTS[0],
        AGENT_COMPOSER_CONTEXTS[1],
        "Agent > AgentDecision > AgentPermission",
        "Agent > AgentDecision > AgentQuestion",
        "Agent > AgentDecision > AgentPlan",
        // §12: row focus lives **inside** scroll mode, so the row context is only ever on the
        // chain under `AgentNativeScroll`. Binding it as a sibling was the shape that made
        // `⏎`/`u`/`o` bound, handled and never entered.
        "Agent > AgentNativeScroll",
        "Agent > AgentNativeScroll > AgentRow",
        "Filter",
        "Filter > BoardFilter",
        "Palette",
        "FleetTextInput",
        "FleetTextInput && mode == multiline",
        "FleetTextInput && mode == multiline && enter == newline",
        "Jobs",
        "Dialog",
        "Dialog > Create",
        "Dialog > CreateEditing",
        "Dialog > Confirm",
        "Dialog > Context",
        "Dialog > Assign",
        "Dialog > Settings",
        "Dialog > SettingsEditing",
        "Dialog > SettingsSearch",
        "Dialog > Help",
        "Dialog > Quit",
        "Dialog > QuitDaemon",
        "Daemon > Down",
        "Daemon > Banner",
        "Daemon > Doctor",
        "FirstRun",
    ];

    /// Contexts that may remain in the live chain while a `FleetTextInput` owns editing.
    ///
    /// Every entry is an *ancestor* of a live editor, so a bare printable key here is a key the
    /// user cannot type (`docs/APP-CONTRACTS.md` §3). `Fleet` wraps every surface there is;
    /// `Workspace > Native` wraps the embedded pane whose prompt is an editor too.
    const TEXT_INPUT_HOST_CONTEXTS: &[&str] = &[
        "Fleet",
        "Workspace > Native",
        "Dialog",
        "Dialog > CardDetailEditing",
        "Dialog > BoardSettingsEditing",
        "Dialog > CardPicker",
        "Dialog > SettingsEditing",
        "Dialog > SettingsSearch",
        "Dialog > CreateEditing",
        "Dialog > CardCreate",
        "Dialog > Clone",
        "Dialog > Context",
        "Dialog > Rename",
        "Dialog > Hooks",
        "Filter",
        "Filter > BoardFilter",
        "Palette",
        AGENT_COMPOSER_CONTEXTS[0],
        AGENT_COMPOSER_CONTEXTS[1],
    ];

    /// Printable owner keys intentionally retained by text-input host contexts.
    const TEXT_INPUT_HOST_EXCEPTIONS: &[(&str, &str)] = &[
        // The picker's query is a filter: `space` toggles the highlighted card, so the query
        // never contains a space. Its `TextInput` filters `' '` out of typing and paste, so the
        // model and this binding agree.
        ("Dialog > CardPicker", "space"),
    ];

    /// Contexts that *would* wrap a live editor, but leave the chain before one takes over.
    ///
    /// The second half of the ownership rule: a container may keep bare printable keys only if
    /// `AppState::context_chain` drops its whole word while an editor owns the keyboard. §3.12
    /// C's banner is the one such container — it is appended innermost to a base chain, which
    /// would otherwise put its `r` and `l` between the user and the agent composer — and
    /// `state::navigation` and `state::agents` own the tests that keep that drop true.
    const EDITOR_YIELDING_CONTEXTS: &[&str] = &["Daemon > Banner"];

    /// Contexts that can never be an ancestor of a live `FleetTextInput`, so bare printable
    /// keys are theirs to bind.
    ///
    /// Browsing words are here because their dialogs publish a distinct `*Editing` word the
    /// moment a field owns typing; full-window surfaces are here because they draw no editor;
    /// the `FleetTextInput` rows are the editor itself rather than a host around it.
    const NO_LIVE_EDITOR_CONTEXTS: &[&str] = &[
        // An open kit menu holds only items today; a filter field in one would move it to the
        // host list, which its non-printable keys already satisfy.
        "FleetMenu",
        "FleetTextInput",
        "FleetTextInput && mode == multiline",
        "FleetTextInput && mode == multiline && enter == newline",
        "Hub",
        "Hub > Repos",
        "Hub > Worktrees",
        "Hub > Prs",
        "Hub > Board",
        // The Review tab's board: its filter publishes `Filter > BoardFilter` like the Hub's.
        "Hub > Prs > Board",
        // The board pane's own word, for the same reason the Hub's is here: the board's filter
        // input never wraps in it, because `AppState::context_chain` publishes
        // `Filter > BoardFilter` and nothing else while that editor owns the keyboard.
        "Workspace > Native > Board",
        "Workspace > Terminal",
        "Workspace > Prefix",
        "Workspace > Scroll",
        "Agent > Terminal",
        "Agent > Prefix",
        "Agent > Scroll",
        "Agent > AgentDecision > AgentPermission",
        "Agent > AgentDecision > AgentQuestion",
        "Agent > AgentDecision > AgentPlan",
        "Agent > AgentNativeScroll",
        "Agent > AgentNativeScroll > AgentRow",
        "Jobs",
        "Jobs > Log",
        "Dialog > CardDetail",
        "Dialog > BoardSettings",
        "Dialog > Settings",
        "Dialog > Create",
        "Dialog > Confirm",
        "Dialog > Assign",
        "Dialog > Help",
        "Dialog > Quit",
        "Dialog > QuitDaemon",
        "Daemon > Down",
        "Daemon > Doctor",
        "FirstRun",
    ];

    fn is_bare_printable_owner_key(keys: &str) -> bool {
        let key = keys.strip_prefix("shift-").unwrap_or(keys);
        key == "space" || key.chars().count() == 1
    }

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
                        "Dialog > CardCreate"
                            | "Dialog > CardPicker"
                            | "Dialog > BoardSettings"
                            | "Dialog > BoardSettingsEditing"
                    ))
                .then_some((keys, context, action))
            })
            .collect();
        // `Hub > Prs > Board` is documented as "the `Hub > Board` table plus these
        // differences" and is checked against that sentence by its own test below.
        let registered: HashSet<_> = table()
            .into_iter()
            .filter(|spec| spec.context != REVIEW_BOARD)
            .filter(|spec| {
                spec.action.starts_with("board::")
                    || spec.action.starts_with("card_detail::")
                    || matches!(
                        spec.context,
                        "Dialog > CardCreate"
                            | "Dialog > CardPicker"
                            | "Dialog > BoardSettings"
                            | "Dialog > BoardSettingsEditing"
                    )
            })
            .map(|spec| (spec.keys, spec.context, spec.action))
            .collect();
        assert!(!documented.is_empty());
        assert_eq!(documented, registered);
    }

    /// The Review tab's board context (KEYMAP § Hub › Pull requests › Review board).
    const REVIEW_BOARD: &str = "Hub > Prs > Board";

    /// KEYMAP says the Review board binds the whole `Hub > Board` table against the same
    /// actions, with a short list of differences. This is that sentence as a test.
    #[test]
    fn the_review_board_binds_the_hub_board_table_with_its_documented_differences() {
        let table = table();
        let rows = |context: &str| -> HashMap<&'static str, &'static str> {
            table
                .iter()
                .filter(|spec| spec.context == context)
                .map(|spec| (spec.keys, spec.action))
                .collect()
        };
        let mut expected = rows("Hub > Board");
        for (keys, action) in [
            ("tab", "prs::NextTab"),
            ("shift-tab", "prs::PrevTab"),
            ("p", "prs::Back"),
            ("q", "prs::Back"),
            ("A", "board::AttachRun"),
            ("X", "board::CancelRun"),
            (">", "board::RunNow"),
            ("B", "board::OpenPullRequest"),
            ("y", "board::CopyPullRequestUrl"),
        ] {
            expected.insert(keys, action);
        }
        assert_eq!(rows(REVIEW_BOARD), expected);

        // Every tie with `Hub > Board` (same depth on the live chain) must go to the review
        // rows, which gpui settles by registration order.
        let chain = ["Fleet", "Hub", "Prs", "Board"];
        for (keys, action) in rows(REVIEW_BOARD) {
            let resolved = action_for_chain(&chain, &Keystroke::parse(keys).unwrap())
                .map(|action| action.name());
            assert_eq!(resolved, Some(action), "`{keys}` on the Review board");
        }
    }

    /// The floor sits within ten rows of the live table, so a silent loss is caught and a
    /// deliberate retirement is a one-line edit here. The seven per-dialog editing rows
    /// (`backspace`, `ctrl-w`, `ctrl-u`, `ctrl-a`, `ctrl-e`, `left`, `right`) moved to the
    /// `FleetTextInput` table, and the two `Dialog > Settings` arrows that used to ride on
    /// them were added back.
    #[test]
    fn key_table_is_well_formed() {
        let bindings = bindings();
        assert_eq!(bindings.len(), table().len());
        assert!(
            bindings.len() > 640,
            "the table lost rows: {}",
            bindings.len()
        );
    }

    /// One editing vocabulary, stated twice: `key_table!` feeds the Help overlay and the
    /// documentation-drift test, and `text_input::default_bindings` serves every app that has
    /// no key table of its own. They must not drift.
    #[test]
    fn the_kit_editor_table_matches_this_one() {
        fn shape(bindings: Vec<gpui::KeyBinding>) -> Vec<String> {
            let mut rows: Vec<String> = bindings
                .iter()
                .filter(|binding| {
                    binding.predicate().is_some_and(|predicate| {
                        format!("{predicate:?}").contains(fleet_ui_kit::TEXT_INPUT_KEY_CONTEXT)
                    })
                })
                .map(|binding| {
                    format!(
                        "{:?} {} {:?}",
                        binding.keystrokes(),
                        binding.action().name(),
                        binding
                            .predicate()
                            .map(|predicate| format!("{predicate:?}"))
                    )
                })
                .collect();
            rows.sort();
            rows
        }
        let app = shape(bindings());
        assert!(!app.is_empty());
        assert_eq!(app, shape(fleet_ui_kit::text_input::default_bindings()));
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

    /// Contracts §5.5: a context board can never hold a run, so `A`, `X` and `>` are bound on
    /// the worktree board pane and the detail only — a key whose one answer is a refusal is an
    /// affordance that does nothing. `b`, `m` and `C` edit things every board has.
    #[test]
    fn the_run_keys_are_absent_from_the_hubs_board() {
        let table = table();
        let bound = |context: &str, action: &str| {
            table
                .iter()
                .any(|spec| spec.context == context && spec.action == action)
        };
        for action in ["board::AttachRun", "board::CancelRun", "board::RunNow"] {
            assert!(
                !bound("Hub > Board", action),
                "`{action}` is bound on the Hub's context board"
            );
            assert!(bound("Workspace > Native > Board", action), "{action}");
            assert!(bound("Dialog > CardDetail", action), "{action}");
        }
        for action in ["board::PickBlockedBy", "board::PickAgent"] {
            for context in [
                "Hub > Board",
                "Workspace > Native > Board",
                "Dialog > CardDetail",
            ] {
                assert!(bound(context, action), "{action} in {context}");
            }
        }
        assert!(bound("Hub > Board", "board::Columns"));
        assert!(bound("Workspace > Native > Board", "board::Columns"));
        assert!(!bound("Dialog > CardDetail", "board::Columns"));
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

    /// Every dialog edits through a live input now, so `backspace` belongs to the component
    /// wherever one is mounted and to nothing at all where none is.
    #[test]
    fn text_input_owns_backspace_below_every_dialog() {
        let backspace = Keystroke::parse("backspace").unwrap_or_else(|error| panic!("{error}"));
        for host in ["CardDetailEditing", "Clone", "Context", "Rename", "Hooks"] {
            assert_eq!(
                action_for_chain(&["Dialog", host, "FleetTextInput"], &backspace)
                    .map(|action| action.name()),
                Some("text_input::Backspace"),
                "`Dialog > {host}` must leave backspace to its input"
            );
        }
        assert_eq!(
            action_for_chain(&["Dialog", "Clone"], &backspace).map(|action| action.name()),
            None,
            "the `Dialog` container binds no editing key of its own"
        );
    }

    #[test]
    fn predicate_contexts_match_their_identifier_in_chain_resolution() {
        assert_eq!(
            context_depth(
                "FleetTextInput && mode == multiline",
                &["Dialog", "CardCreate", "FleetTextInput"]
            ),
            Some(2)
        );
        assert_eq!(bindings().len(), table().len(), "every built-in row parses");
    }

    #[test]
    fn text_input_leaves_container_keys_and_ctrl_v_unbound() {
        let input_rows: Vec<_> = table()
            .into_iter()
            .filter(|spec| spec.context.starts_with("FleetTextInput"))
            .collect();
        assert_eq!(
            input_rows.len(),
            49,
            "visual-row actions, logical ctrl-shift selection and owner-aware Enter must stay in the gallery table"
        );
        for keys in ["tab", "shift-tab", "ctrl-n", "ctrl-p", "escape", "ctrl-v"] {
            assert!(
                input_rows.iter().all(|spec| spec.keys != keys),
                "`{keys}` belongs to the input's container"
            );
        }
        assert!(input_rows.iter().any(|spec| {
            spec.keys == "enter"
                && spec.context == "FleetTextInput && mode == multiline && enter == newline"
        }));
        assert!(input_rows.iter().all(|spec| {
            spec.keys != "enter"
                || spec.context == "FleetTextInput && mode == multiline && enter == newline"
        }));
        assert!(input_rows.iter().any(|spec| {
            spec.keys == "shift-enter" && spec.context == "FleetTextInput && mode == multiline"
        }));
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
        // `Workspace > Native > Board` is a different word, nested one deeper: the board tab
        // draws the board itself instead of handing its keys to an embedded view, and it still
        // reserves nothing beyond the prefix its parent binds.
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

    /// The binding and its row in `docs/KEYMAP.md` are one change.
    ///
    /// The prefix table is written `| key | action |`, so the board-wide drift test above
    /// cannot see it; this is the same check for the one row `prefix::OpenBoard` owns. A key
    /// missing from the table is a key nobody finds, and a row with nothing bound is a lie.
    #[test]
    fn the_board_prefix_key_is_bound_and_documented() {
        let keystroke = Keystroke::parse("b").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            action_for_keystroke("Workspace > Prefix", &keystroke).map(|action| action.name()),
            Some("prefix::OpenBoard")
        );
        let docs = include_str!("../../../docs/KEYMAP.md");
        assert!(
            docs.lines()
                .any(|line| line.starts_with("| `b` |") && line.contains("board tab")),
            "the `ctrl-s` table in docs/KEYMAP.md names no board row"
        );
    }

    #[test]
    fn live_prefix_resolution_uses_the_authoritative_table() {
        for keys in [
            "s", "S", "ctrl-s", "1", "tab", "W", "u", "d", "b", "[", "]", "escape",
        ] {
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

        // `o` is refused on purpose (KEYMAP "Recorded so they are not re-proposed"), which
        // makes it the stable example of a key the live prefix consumes and resolves to
        // nothing.
        assert!(
            action_for_keystroke(
                "Workspace > Prefix",
                &Keystroke::parse("o").unwrap_or_else(|error| panic!("{error}"))
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

    /// `docs/KEYMAP.md` § *Native agent thread*: an agent tab repeats the Workspace selection,
    /// MRU **and** session rows, because its chain replaces `Workspace > …` instead of covering
    /// it. Every one of the six sub-modes has to name them: gpui's `>` is a subsequence test and
    /// none of the six is on another's chain.
    #[test]
    fn native_agent_modes_bind_workspace_selection_mru_and_session_rows() {
        let expected = [
            ("ctrl-s 1", "native_agent::SelectTab1"),
            ("ctrl-s 2", "native_agent::SelectTab2"),
            ("ctrl-s 3", "native_agent::SelectTab3"),
            ("ctrl-s 4", "native_agent::SelectTab4"),
            ("ctrl-s 5", "native_agent::SelectTab5"),
            ("ctrl-s 6", "native_agent::SelectTab6"),
            ("ctrl-s 7", "native_agent::SelectTab7"),
            ("ctrl-s 8", "native_agent::SelectTab8"),
            ("ctrl-s 9", "native_agent::SelectTab9"),
            ("ctrl-s tab", "native_agent::LastTab"),
            ("ctrl-s w", "native_agent::LastSession"),
            ("ctrl-s s", "prefix::GoHub"),
            ("ctrl-s S", "prefix::SleepAndGoHub"),
            ("ctrl-s h", "prefix::PrevTab"),
            ("ctrl-s p", "prefix::PrevTab"),
            ("ctrl-s l", "prefix::NextTab"),
            ("ctrl-s n", "prefix::NextTab"),
            ("ctrl-s W", "prefix::SessionSwitcher"),
            ("ctrl-s u", "prefix::UpToCaller"),
            ("ctrl-s d", "prefix::AgentsPicker"),
            ("ctrl-s c", "prefix::NewTerminal"),
            ("ctrl-s b", "prefix::OpenBoard"),
            ("ctrl-s y", "prefix::CopyWorktreePath"),
            ("ctrl-s z", "prefix::ToggleZoom"),
            ("ctrl-s v", "prefix::ToggleWatchPane"),
            ("ctrl-s g", "prefix::ToggleChanges"),
            ("ctrl-s V", "prefix::DismissWatch"),
            ("ctrl-s N", "prefix::NextWatch"),
            ("ctrl-s P", "prefix::PrevWatch"),
            ("ctrl-s !", "fleet::FocusStickyError"),
            ("ctrl-s J", "fleet::OpenJobs"),
            ("ctrl-s ?", "fleet::OpenHelp"),
            ("ctrl-s escape", "prefix::Cancel"),
        ];
        let table = table();

        for context in AGENT_THREAD_CONTEXTS {
            for (keys, action) in expected {
                assert!(
                    table.iter().any(|spec| {
                        spec.context == *context && spec.keys == keys && spec.action == action
                    }),
                    "missing `{keys}` in `{context}`"
                );
            }
        }
    }

    /// The four PTY-only rows of the Workspace prefix table stay out of an agent tab.
    ///
    /// `r` restarts the exited command, `,` renames the terminal, `]` pastes into it and
    /// `^s ^s` sends a literal `ctrl-s`: every one of them addresses a PTY, and a Fleet-drawn
    /// tab has none. Binding them would be a key that silently does nothing.
    #[test]
    fn the_pty_only_prefix_rows_are_absent_from_every_agent_thread_context() {
        let table = table();
        for context in AGENT_THREAD_CONTEXTS {
            for keys in ["ctrl-s r", "ctrl-s ,", "ctrl-s ]", "ctrl-s ctrl-s"] {
                assert!(
                    !table
                        .iter()
                        .any(|spec| spec.context == *context && spec.keys == keys),
                    "`{keys}` addresses a PTY and must stay out of `{context}`"
                );
            }
        }
    }

    /// The chord resolver answers from the live chain, the way the shell's interceptor asks it.
    #[test]
    fn the_chord_resolver_follows_the_live_chain_and_gpui_subsequence_rule() {
        let chord = |chain: &[&str], keys: &str| {
            let keystroke = Keystroke::parse(keys)
                .unwrap_or_else(|error| panic!("invalid key {keys:?}: {error}"));
            chord_action_for_chain(chain, &keystroke).map(|action| action.name())
        };

        assert_eq!(chord(&["Agent", "AgentIdle"], "s"), Some("prefix::GoHub"));
        assert_eq!(
            chord(&["Agent", "AgentIdle"], "u"),
            Some("prefix::UpToCaller")
        );
        assert_eq!(
            chord(&["Agent", "AgentWorking"], "d"),
            Some("prefix::AgentsPicker")
        );
        assert_eq!(
            chord(&["Agent", "AgentWorking", "Daemon", "Banner"], "s"),
            Some("prefix::GoHub"),
            "the daemon banner is appended innermost and owns no `^s` row"
        );
        // Row focus is a word *under* `AgentNativeScroll`, so the scroll rows still have to
        // resolve through the subsequence match rather than an exact context compare.
        assert_eq!(
            chord(&["Agent", "AgentNativeScroll", "AgentRow"], "["),
            Some("native_agent::Scroll")
        );
        assert_eq!(
            chord(&["Agent", "AgentDecision", "AgentPlan"], "x"),
            Some("native_agent::CloseTab")
        );
        // A frozen tail has no model to change, and no chord is a chord outside an agent tab.
        assert_eq!(chord(&["Agent", "AgentNativeScroll"], "m"), None);
        assert_eq!(chord(&["Workspace", "Terminal"], "s"), None);
        assert_eq!(chord(&["Agent", "AgentIdle"], "q"), None);
    }

    #[test]
    fn delegation_row_keeps_expand_and_copy_and_adds_cancel() {
        let context = "Agent > AgentNativeScroll > AgentRow";
        for (keys, expected) in [
            ("enter", "native_agent::ExpandRow"),
            ("y", "native_agent::CopyRow"),
            ("x", "native_agent::CancelDelegation"),
        ] {
            let stroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                action_for_keystroke(context, &stroke).map(|action| action.name()),
                Some(expected),
                "{keys}"
            );
        }
    }

    #[test]
    fn only_a_bare_control_s_is_the_prefix() {
        let prefix = Keystroke::parse("ctrl-s").unwrap_or_else(|error| panic!("{error}"));
        assert!(is_prefix_key(&prefix));
        for keys in ["s", "ctrl-shift-s", "ctrl-alt-s", "cmd-s", "ctrl-a"] {
            let keystroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
            assert!(!is_prefix_key(&keystroke), "{keys}");
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

    /// The confirm's action button shows the highest-precedence binding of `confirm::Accept`,
    /// which is the later row: it must teach `y`, the key §3.8.3 names, not `enter`.
    #[test]
    fn the_confirm_teaches_y_and_still_accepts_enter() {
        let accept: Vec<&str> = table()
            .iter()
            .filter(|spec| spec.context == "Dialog > Confirm" && spec.action == "confirm::Accept")
            .map(|spec| spec.keys)
            .collect();
        assert_eq!(accept, ["enter", "y"]);
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
    fn text_input_hosts_never_bind_printable_owner_keys() {
        for keys in [
            "a",
            "7",
            ":",
            "/",
            "?",
            ",",
            ".",
            "-",
            "=",
            "[",
            "]",
            "'",
            "`",
            "\\",
            "shift-a",
            "shift-?",
            "space",
            "shift-space",
        ] {
            assert!(is_bare_printable_owner_key(keys), "`{keys}` is printable");
        }
        for keys in [
            "enter",
            "escape",
            "tab",
            "backspace",
            "delete",
            "left",
            "right",
            "home",
            "end",
            "f1",
            "ctrl-a",
            "alt-a",
            "cmd-a",
            "platform-a",
        ] {
            assert!(
                !is_bare_printable_owner_key(keys),
                "`{keys}` is not a bare printable key"
            );
        }

        for spec in table() {
            if TEXT_INPUT_HOST_CONTEXTS.contains(&spec.context) {
                assert!(
                    !is_bare_printable_owner_key(spec.keys)
                        || TEXT_INPUT_HOST_EXCEPTIONS.contains(&(spec.context, spec.keys)),
                    "`{}` in `{}` would shadow typing",
                    spec.keys,
                    spec.context
                );
            }
        }
    }

    /// The list above is only as good as its coverage: the §3.12 C banner was missing from it,
    /// and its bare `r` / `l` sat innermost on every base chain — including the agent
    /// composer's. Forcing every context to be classified is what makes the next one land in
    /// one of the three answers instead of in the silent default.
    #[test]
    fn every_key_context_is_classified_against_a_live_text_input() {
        let mut contexts: Vec<&str> = table().into_iter().map(|spec| spec.context).collect();
        contexts.sort_unstable();
        contexts.dedup();
        for context in contexts {
            let answers = [
                TEXT_INPUT_HOST_CONTEXTS,
                EDITOR_YIELDING_CONTEXTS,
                NO_LIVE_EDITOR_CONTEXTS,
            ]
            .into_iter()
            .filter(|list| list.contains(&context))
            .count();
            assert_eq!(
                answers, 1,
                "`{context}` must say exactly once whether it can wrap a live `TextInput`: \
                 a host binds no bare printable key, a yielding context leaves the chain while \
                 an editor edits, and anything else declares that it never wraps one"
            );
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
