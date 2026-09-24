//! The Workspace's key chips, read once from the key table.
//!
//! In the Hub a button resolves its chip from the live keymap in the focused context, as every
//! kit button does. Over a terminal that finds nothing: the Workspace's commands are bound after
//! the prefix, in `Workspace > Prefix`, a context the window is in for one key only. The chip is
//! therefore the whole sequence the key table spells — the prefix, then the key bound after it —
//! so it reads `⌃S ?` all the time rather than only while `⌃S` is held. A native agent tab binds
//! the same keys as `^s` chords, so one spelling serves both.

use std::sync::OnceLock;

use fleet_ui_kit::Kbd;
use gpui::{Action, Keystroke};

use crate::{
    actions::{fleet, native_agent, prefix, workspace},
    keymap,
};

/// Where a terminal tab rests; the prefix is bound here.
const TERMINAL_CONTEXT: &str = "Workspace > Terminal";
/// The one-shot context the key after the prefix resolves in.
const PREFIX_CONTEXT: &str = "Workspace > Prefix";

/// The chips the Workspace's chrome shows: its bars, its tab strip and their menus.
pub(crate) struct WorkspaceKeys {
    /// `⌃S` alone: the "Fleet commands" button.
    pub(crate) prefix: Option<Kbd>,
    /// `⌃S s`: back to the Hub.
    pub(crate) go_hub: Option<Kbd>,
    /// `⌃S ?`: Help.
    pub(crate) help: Option<Kbd>,
    /// `⌃S J`: the Jobs sheet.
    pub(crate) jobs: Option<Kbd>,
    /// `⌃S d`: the agents picker.
    pub(crate) agents: Option<Kbd>,
    /// `⌃S w`: the last session.
    pub(crate) last_session: Option<Kbd>,
    /// `⌃S W`: the session switcher.
    pub(crate) session_switcher: Option<Kbd>,
    /// `⌃S 1`–`⌃S 9`: select a tab, by position.
    pub(crate) select_tab: [Option<Kbd>; 9],
    /// `⌃S c`: a new terminal.
    pub(crate) new_terminal: Option<Kbd>,
    /// `⌃S a`: a new Claude thread.
    pub(crate) new_claude: Option<Kbd>,
    /// `⌃S A`: a new Codex thread.
    pub(crate) new_codex: Option<Kbd>,
    /// `⌃S b`: this worktree's board tab.
    pub(crate) board: Option<Kbd>,
    /// `⌃S F`: the agent's terminal fallback.
    pub(crate) fallback: Option<Kbd>,
    /// `⌃S x`: close the tab.
    pub(crate) close: Option<Kbd>,
    /// `⌃S ,`: rename the tab.
    pub(crate) rename: Option<Kbd>,
    /// `⌃S r`: restart the exited command.
    pub(crate) restart: Option<Kbd>,
    /// `⌃S v`: the watch split.
    pub(crate) watch: Option<Kbd>,
    /// `⌃S g`: the Changes panel.
    pub(crate) changes: Option<Kbd>,
    /// `⌃S z`: zoom.
    pub(crate) zoom: Option<Kbd>,
    /// `⌃S !`: the sticky error, opened in the Jobs panel.
    pub(crate) sticky_error: Option<Kbd>,
}

/// The Hub's `!`, for the status bar, which is drawn outside the focused element and so cannot
/// resolve it from focus.
pub(crate) fn hub_sticky_error_key() -> Option<Kbd> {
    static KEY: OnceLock<Option<Kbd>> = OnceLock::new();
    KEY.get_or_init(|| {
        keymap::keystrokes_in("Hub", &fleet::FocusStickyError).map(|strokes| Kbd::new(&strokes))
    })
    .clone()
}

/// The Hub's `J`, for the toasts that point at the Jobs panel.
pub(crate) fn hub_jobs_key() -> Option<Kbd> {
    static KEY: OnceLock<Option<Kbd>> = OnceLock::new();
    KEY.get_or_init(|| {
        keymap::keystrokes_in("Hub", &fleet::OpenJobs).map(|strokes| Kbd::new(&strokes))
    })
    .clone()
}

pub(crate) fn workspace_keys() -> &'static WorkspaceKeys {
    static KEYS: OnceLock<WorkspaceKeys> = OnceLock::new();
    KEYS.get_or_init(|| {
        let prefix = keymap::keystrokes_in(TERMINAL_CONTEXT, &workspace::EnterPrefix);
        let prefixed = |action: &dyn Action| -> Option<Kbd> {
            let mut strokes: Vec<Keystroke> = prefix.clone()?;
            strokes.extend(keymap::keystrokes_in(PREFIX_CONTEXT, action)?);
            Some(Kbd::new(&strokes))
        };
        WorkspaceKeys {
            prefix: prefix.as_deref().map(Kbd::new),
            go_hub: prefixed(&prefix::GoHub),
            help: prefixed(&fleet::OpenHelp),
            jobs: prefixed(&fleet::OpenJobs),
            agents: prefixed(&prefix::AgentsPicker),
            last_session: prefixed(&prefix::LastSession),
            session_switcher: prefixed(&prefix::SessionSwitcher),
            select_tab: [
                prefixed(&prefix::SelectTab1),
                prefixed(&prefix::SelectTab2),
                prefixed(&prefix::SelectTab3),
                prefixed(&prefix::SelectTab4),
                prefixed(&prefix::SelectTab5),
                prefixed(&prefix::SelectTab6),
                prefixed(&prefix::SelectTab7),
                prefixed(&prefix::SelectTab8),
                prefixed(&prefix::SelectTab9),
            ],
            new_terminal: prefixed(&prefix::NewTerminal),
            new_claude: prefixed(&native_agent::NewClaude),
            new_codex: prefixed(&native_agent::NewCodex),
            board: prefixed(&prefix::OpenBoard),
            fallback: prefixed(&native_agent::TerminalFallback),
            close: prefixed(&prefix::CloseTerminal),
            rename: prefixed(&prefix::RenameTerminal),
            restart: prefixed(&prefix::RestartCommand),
            watch: prefixed(&prefix::ToggleWatchPane),
            changes: prefixed(&prefix::ToggleChanges),
            zoom: prefixed(&prefix::ToggleZoom),
            sticky_error: prefixed(&fleet::FocusStickyError),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spelled(kbd: &Option<Kbd>) -> String {
        kbd.as_ref()
            .map(|kbd| {
                kbd.strokes()
                    .iter()
                    .map(|stroke| stroke.unparse())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }

    #[test]
    fn every_workspace_chip_spells_the_prefix_then_its_key() {
        let keys = workspace_keys();
        assert_eq!(spelled(&keys.prefix), "ctrl-s");
        assert_eq!(spelled(&keys.go_hub), "ctrl-s s");
        assert_eq!(spelled(&keys.help), "ctrl-s ?");
        assert_eq!(spelled(&keys.jobs), "ctrl-s shift-j");
        assert_eq!(spelled(&keys.agents), "ctrl-s d");
        assert_eq!(spelled(&keys.last_session), "ctrl-s w");
        assert_eq!(spelled(&keys.session_switcher), "ctrl-s shift-w");
        assert_eq!(spelled(&keys.select_tab[0]), "ctrl-s 1");
        assert_eq!(spelled(&keys.select_tab[8]), "ctrl-s 9");
        assert_eq!(spelled(&keys.new_terminal), "ctrl-s c");
        assert_eq!(spelled(&keys.new_claude), "ctrl-s a");
        assert_eq!(spelled(&keys.new_codex), "ctrl-s shift-a");
        assert_eq!(spelled(&keys.board), "ctrl-s b");
        assert_eq!(spelled(&keys.fallback), "ctrl-s shift-f");
        assert_eq!(spelled(&keys.close), "ctrl-s x");
        assert_eq!(spelled(&keys.rename), "ctrl-s ,");
        assert_eq!(spelled(&keys.restart), "ctrl-s r");
        assert_eq!(spelled(&keys.watch), "ctrl-s v");
        assert_eq!(spelled(&keys.changes), "ctrl-s g");
        assert_eq!(spelled(&keys.zoom), "ctrl-s z");
        assert_eq!(spelled(&keys.sticky_error), "ctrl-s !");
        assert_eq!(spelled(&hub_sticky_error_key()), "!");
        assert_eq!(spelled(&hub_jobs_key()), "shift-j");
    }
}
