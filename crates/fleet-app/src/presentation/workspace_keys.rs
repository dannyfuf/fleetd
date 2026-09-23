//! The Workspace's chrome chips, read once from the key table.
//!
//! In the Hub a bar button resolves its chip from the live keymap in the focused context, as
//! every kit button does. Over a terminal that finds nothing: Help, Jobs and the agents picker are
//! bound after the prefix, in `Workspace > Prefix`, a context the window is in for one key only.
//! The chip is therefore the whole sequence the key table spells — the prefix, then the key bound
//! after it — so it reads `⌃S ?` all the time rather than only while `⌃S` is held.

use std::sync::OnceLock;

use fleet_ui_kit::Kbd;
use gpui::{Action, Keystroke};

use crate::{
    actions::{fleet, prefix, workspace},
    keymap,
};

/// Where a terminal tab rests; the prefix is bound here.
const TERMINAL_CONTEXT: &str = "Workspace > Terminal";
/// The one-shot context the key after the prefix resolves in.
const PREFIX_CONTEXT: &str = "Workspace > Prefix";

/// The chips the Workspace's bars show.
pub(super) struct WorkspaceKeys {
    /// `⌃S` alone: the "Fleet commands" button.
    pub(super) prefix: Option<Kbd>,
    /// `⌃S s`: back to the Hub.
    pub(super) go_hub: Option<Kbd>,
    /// `⌃S ?`: Help.
    pub(super) help: Option<Kbd>,
    /// `⌃S J`: the Jobs sheet.
    pub(super) jobs: Option<Kbd>,
    /// `⌃S d`: the agents picker.
    pub(super) agents: Option<Kbd>,
}

pub(super) fn workspace_keys() -> &'static WorkspaceKeys {
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
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workspace_chip_spells_the_prefix_then_its_key() {
        let keys = workspace_keys();
        let spelled = |kbd: &Option<Kbd>| {
            kbd.as_ref()
                .map(|kbd| {
                    kbd.strokes()
                        .iter()
                        .map(|stroke| stroke.unparse())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        };
        assert_eq!(spelled(&keys.prefix), "ctrl-s");
        assert_eq!(spelled(&keys.go_hub), "ctrl-s s");
        assert_eq!(spelled(&keys.help), "ctrl-s ?");
        assert_eq!(spelled(&keys.jobs), "ctrl-s shift-j");
        assert_eq!(spelled(&keys.agents), "ctrl-s d");
    }
}
