//! Which [`Place`] each key context of the key table belongs to, and what a person calls it.

use super::Place;

/// `(context predicate, place, heading)` for every context [`crate::keymap::table`] binds.
///
/// The heading is what Help prints above the keys of one context. It names the surface the
/// way the screen does, never the context word itself (`Hub > Prs` reads "Pull requests").
const CONTEXTS: &[(&str, Place, &str)] = &[
    ("Fleet", Place::Everywhere, "Everywhere"),
    ("Hub", Place::Hub, "Hub"),
    ("Hub > Repos", Place::Hub, "Repositories"),
    ("Hub > Worktrees", Place::Worktrees, "Worktrees"),
    ("Hub > Prs", Place::PullRequests, "Pull requests"),
    ("Hub > Board", Place::Board, "Board"),
    ("Filter", Place::Hub, "Filter"),
    ("FirstRun", Place::Hub, "First run"),
    ("Workspace > Native > Board", Place::Board, "Board tab"),
    ("Filter > BoardFilter", Place::Board, "Board filter"),
    ("Dialog > CardDetail", Place::CardDetail, "Card"),
    (
        "Dialog > CardDetailEditing",
        Place::CardDetail,
        "Card, typing",
    ),
    ("Dialog > CardCreate", Place::CardDetail, "New card"),
    (
        "Dialog > CardPicker",
        Place::CardDetail,
        "Card field picker",
    ),
    ("Workspace > Terminal", Place::Terminal, "Terminal"),
    ("Workspace > Native", Place::Terminal, "Git and board tabs"),
    ("Workspace > Prefix", Place::Terminal, "After ^s"),
    ("Workspace > Scroll", Place::Scroll, "Scrolling"),
    ("Agent > Terminal", Place::AgentPopup, "Agent window"),
    (
        "Agent > Prefix",
        Place::AgentPopup,
        "Agent window, after ^s",
    ),
    ("Agent > Scroll", Place::Scroll, "Agent window, scrolling"),
    ("Agent > AgentIdle", Place::AgentThread, "Idle"),
    ("Agent > AgentWorking", Place::AgentThread, "Working"),
    (
        "Agent > AgentDecision > AgentPermission",
        Place::AgentThread,
        "Permission request",
    ),
    (
        "Agent > AgentDecision > AgentQuestion",
        Place::AgentThread,
        "Question",
    ),
    (
        "Agent > AgentDecision > AgentPlan",
        Place::AgentThread,
        "Plan",
    ),
    ("Agent > AgentNativeScroll", Place::AgentThread, "Scrolling"),
    (
        "Agent > AgentNativeScroll > AgentRow",
        Place::AgentThread,
        "Focused row",
    ),
    ("Jobs", Place::Jobs, "Jobs"),
    ("Jobs > Log", Place::Jobs, "Open log"),
    ("Palette", Place::Dialog, "Palette"),
    ("Dialog", Place::Dialog, "Any dialog"),
    ("Dialog > Create", Place::Dialog, "New worktree"),
    (
        "Dialog > CreateEditing",
        Place::Dialog,
        "New worktree, typing",
    ),
    ("Dialog > Confirm", Place::Dialog, "Confirm"),
    ("Dialog > Context", Place::Dialog, "Context"),
    ("Dialog > Assign", Place::Dialog, "Move repository"),
    ("Dialog > Settings", Place::Dialog, "Settings"),
    (
        "Dialog > SettingsEditing",
        Place::Dialog,
        "Settings, typing",
    ),
    ("Dialog > BoardSettings", Place::Dialog, "Board settings"),
    (
        "Dialog > BoardSettingsEditing",
        Place::Dialog,
        "Board settings, typing",
    ),
    ("Dialog > Help", Place::Dialog, "Help"),
    ("Dialog > Quit", Place::Dialog, "Quit"),
    ("Dialog > QuitDaemon", Place::Dialog, "Quit and stop fleetd"),
    ("FleetTextInput", Place::EditingText, "Text fields"),
    (
        "FleetTextInput && mode == multiline",
        Place::EditingText,
        "Multi-line text",
    ),
    (
        "FleetTextInput && mode == multiline && enter == newline",
        Place::EditingText,
        "Multi-line text where Enter adds a line",
    ),
    ("Daemon > Down", Place::Daemon, "fleetd is not running"),
    (
        "Daemon > Banner",
        Place::Daemon,
        "Lost connection to fleetd",
    ),
    ("Daemon > Doctor", Place::Daemon, "Health report"),
];

/// The place a key context belongs to.
pub(super) fn place_of_context(context: &str) -> Option<Place> {
    CONTEXTS
        .iter()
        .find(|(known, _, _)| *known == context)
        .map(|(_, place, _)| *place)
}

/// The heading Help prints above one key context's keys, e.g. `Hub > Prs` → "Pull requests".
#[must_use]
pub fn context_title(context: &str) -> Option<&'static str> {
    CONTEXTS
        .iter()
        .find(|(known, _, _)| *known == context)
        .map(|(_, _, title)| *title)
}
