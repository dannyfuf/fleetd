//! The action catalogue: what a person calls each action, what it does, and where it works.
//!
//! Every action bound in [`keymap::table`] has exactly one [`ActionInfo`] here, written by hand.
//! Help, the command palette, the `^s` command menu, tooltips and buttons all read their words
//! from this module, so a label is written once and nothing user-visible is ever generated from
//! an action's type name (ADR 0023). A test fails when a binding ships without an entry.
//!
//! # Reading it
//!
//! * [`info`] answers "what is this action called" for one action name, e.g. `"hub::MoveDown"`.
//!   A tooltip, a button or a palette row uses this.
//! * [`for_place`] lists the entries whose keys work in one [`Place`], in the catalogue's stable
//!   order, each with the key sequences bound there. Help's "Here in …", the `^s` menu and
//!   the palette's suggestions use this.
//! * [`all`] lists every entry with every key it has, for Help's full, searchable table. Its
//!   "Where" column is [`ActionInfo::place`], the one place an entry is filed under.
//!
//! Both lists are built once from the key table and the entries, on first use, and never per
//! frame.
//!
//! # Ranges
//!
//! `SelectTab1` … `SelectTab9`, `SelectContext1` … `SelectContext9` and `Choose1` … `Choose5`
//! are one entry each, with a range label ("Go to tab 1–9"). [`Placed::key_range`] gives the
//! first and last key so a reader can draw `1`–`9` rather than nine chips.

mod contexts;
mod entries;

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::keymap;

pub use crate::keymap::BindingSpec;
pub use contexts::context_title;

/// Where an action works. Help's "Where" list, the `^s` menu and the palette group by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    /// Bound on the app root: works on every screen.
    Everywhere,
    /// The Hub, its repositories pane, its filter bar and the first-run card.
    Hub,
    /// The Hub's worktrees list.
    Worktrees,
    /// The Hub's pull-requests screen.
    PullRequests,
    /// A board: the Hub's board tab, a worktree's board tab, and the board's filter.
    Board,
    /// An open card, and the new-card and property-picker dialogs opened from a board.
    CardDetail,
    /// A worktree's terminal or native tab, including the keys after `^s`.
    Terminal,
    /// A native agent thread tab, in any of its modes.
    AgentThread,
    /// The floating agent terminal window.
    AgentPopup,
    /// Scroll mode inside a terminal or the agent window.
    Scroll,
    /// The Jobs panel.
    Jobs,
    /// A dialog or the command palette.
    Dialog,
    /// Any text field, while it is being typed in.
    EditingText,
    /// The screens shown when fleetd is down or being checked.
    Daemon,
}

impl Place {
    /// Every place, in the order Help lists them.
    pub const ALL: &'static [Self] = &[
        Self::Everywhere,
        Self::Hub,
        Self::Worktrees,
        Self::PullRequests,
        Self::Board,
        Self::CardDetail,
        Self::Terminal,
        Self::AgentThread,
        Self::AgentPopup,
        Self::Scroll,
        Self::Jobs,
        Self::Dialog,
        Self::EditingText,
        Self::Daemon,
    ];

    /// The heading a person reads for this place.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Everywhere => "Everywhere",
            Self::Hub => "Hub",
            Self::Worktrees => "Worktrees",
            Self::PullRequests => "Pull requests",
            Self::Board => "Board",
            Self::CardDetail => "Card",
            Self::Terminal => "Terminal",
            Self::AgentThread => "Agent thread",
            Self::AgentPopup => "Agent window",
            Self::Scroll => "Scrolling",
            Self::Jobs => "Jobs",
            Self::Dialog => "Dialogs",
            Self::EditingText => "Editing text",
            Self::Daemon => "fleetd",
        }
    }

    /// The place a key context belongs to, e.g. `Hub > Worktrees` → [`Place::Worktrees`].
    ///
    /// `None` only for a context the catalogue does not know; a test keeps every context of
    /// [`keymap::table`] known.
    #[must_use]
    pub fn of_context(context: &str) -> Option<Self> {
        contexts::place_of_context(context)
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The heading an action is listed under in Help and in the `^s` command menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// Quitting, settings, refresh and other app-level commands.
    App,
    /// Moving the cursor, focus and screen.
    Navigation,
    /// Terminal and agent tabs inside a worktree.
    Tabs,
    /// Leaving, sleeping and switching worktree sessions.
    Session,
    /// What a terminal does: paste, scroll back, zoom, restart.
    Terminal,
    /// Starting, finding and working with agents.
    Agents,
    /// Answering an agent's permission request, question or plan.
    Decisions,
    /// Panels and side views: help, jobs, the board tab, the watch pane, details.
    Panels,
    /// Worktrees and their sessions.
    Worktree,
    /// Repositories.
    Repository,
    /// Contexts.
    Context,
    /// Pull requests.
    PullRequests,
    /// The board and its cards.
    Board,
    /// One open card.
    Card,
    /// Scroll mode.
    Scroll,
    /// The Jobs panel.
    Jobs,
    /// Dialogs, confirms and the palette.
    Dialog,
    /// Editing text in a field.
    EditingText,
    /// fleetd being down or checked.
    Daemon,
}

impl Group {
    /// The heading a person reads.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::App => "Fleet",
            Self::Navigation => "Navigation",
            Self::Tabs => "Tabs",
            Self::Session => "Session",
            Self::Terminal => "Terminal",
            Self::Agents => "Agents",
            Self::Decisions => "Agent requests",
            Self::Panels => "Panels",
            Self::Worktree => "Worktree",
            Self::Repository => "Repository",
            Self::Context => "Contexts",
            Self::PullRequests => "Pull requests",
            Self::Board => "Board",
            Self::Card => "Card",
            Self::Scroll => "Scrolling",
            Self::Jobs => "Jobs",
            Self::Dialog => "Dialogs",
            Self::EditingText => "Editing text",
            Self::Daemon => "fleetd",
        }
    }
}

/// What a person needs to know about one action (or one range of actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionInfo {
    /// A sentence-case verb phrase: "Go to tab 1–9", "Delete the worktree safely".
    pub label: &'static str,
    /// The label where space is tight — the `^s` menu's columns, a compact button. Equal to
    /// [`Self::label`] unless an entry says otherwise.
    pub short_label: &'static str,
    /// One line saying what happens, including the safety a person should know about
    /// ("asks first", "keeps running in fleetd").
    pub description: &'static str,
    /// The one place the action is filed under — Help's "Where" column. Its keys may work in
    /// other places too; [`for_place`] answers that.
    pub place: Place,
    /// The heading it is listed under.
    pub group: Group,
    /// Deletes or stops something that cannot be brought back from here, so it always goes
    /// through a confirm (or is that confirm's strong answer). The palette marks the row and a
    /// button uses the danger style.
    pub destructive: bool,
    /// The command palette offers it as a command row.
    pub palette: bool,
    /// Where it ranks among the most useful things to do in the places its keys work: Help's
    /// "Here in …" lists the six lowest ranks reachable from where it was opened. `None` for
    /// an entry that is only ever found by search or in the full table.
    pub rank: Option<u8>,
}

/// One catalogue entry: the action names it covers and what to call them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The fully qualified action names, e.g. `["hub::MoveDown"]`. More than one only for a
    /// range, in order (`SelectTab1` … `SelectTab9`).
    pub actions: &'static [&'static str],
    /// What to call them.
    pub info: ActionInfo,
}

impl Entry {
    /// An entry with the short label equal to the label, not destructive, not in the palette.
    const fn new(
        actions: &'static [&'static str],
        place: Place,
        group: Group,
        label: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            actions,
            info: ActionInfo {
                label,
                short_label: label,
                description,
                place,
                group,
                destructive: false,
                palette: false,
                rank: None,
            },
        }
    }

    /// Sets the label used where space is tight.
    const fn short(mut self, short_label: &'static str) -> Self {
        self.info.short_label = short_label;
        self
    }

    /// Marks the entry as destructive.
    const fn destructive(mut self) -> Self {
        self.info.destructive = true;
        self
    }

    /// Offers the entry as a palette command.
    const fn palette(mut self) -> Self {
        self.info.palette = true;
        self
    }

    /// Features the entry in Help's "Here in …" list at `rank`, lower first.
    const fn featured(mut self, rank: u8) -> Self {
        self.info.rank = Some(rank);
        self
    }

    /// The action a control dispatches for this entry: the only one, or a range's first.
    #[must_use]
    pub fn action(&self) -> &'static str {
        self.actions[0]
    }

    /// Whether the entry stands for a numbered range of actions.
    #[must_use]
    pub fn is_range(&self) -> bool {
        self.actions.len() > 1
    }
}

/// An entry together with the keys bound to it — in one place for [`for_place`], or anywhere
/// for [`all`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// The entry.
    pub entry: &'static Entry,
    /// Every binding of the entry's actions in scope, in key-table order.
    pub bindings: Vec<BindingSpec>,
    /// The distinct key sequences of [`Self::bindings`], in key-table order (`"ctrl-s 1"`,
    /// `"g g"`). A range lists one per member.
    pub keys: Vec<&'static str>,
}

impl Placed {
    /// What to call it.
    #[must_use]
    pub fn info(&self) -> &'static ActionInfo {
        &self.entry.info
    }

    /// For a range, its first and last key (`"1"`, `"9"`), so a reader draws `1`–`9`.
    #[must_use]
    pub fn key_range(&self) -> Option<(&'static str, &'static str)> {
        if !self.entry.is_range() {
            return None;
        }
        Some((*self.keys.first()?, *self.keys.last()?))
    }
}

/// Every entry, in catalogue order: grouped by where it is filed, then by heading.
#[must_use]
pub fn entries() -> &'static [Entry] {
    entries::ENTRIES
}

/// The entry covering an action name, e.g. `"prefix::SelectTab3"` → the "Go to tab 1–9" entry.
#[must_use]
pub fn entry(action: &str) -> Option<&'static Entry> {
    static BY_ACTION: OnceLock<HashMap<&'static str, &'static Entry>> = OnceLock::new();
    BY_ACTION
        .get_or_init(|| {
            let mut map = HashMap::new();
            for entry in entries::ENTRIES {
                for action in entry.actions {
                    map.insert(*action, entry);
                }
            }
            map
        })
        .get(action)
        .copied()
}

/// What to call an action, e.g. `info("worktrees::Delete")`.
#[must_use]
pub fn info(action: &str) -> Option<&'static ActionInfo> {
    entry(action).map(|entry| &entry.info)
}

/// The entries whose keys work in `place`, in catalogue order, with the keys bound there.
///
/// An entry appears under every place one of its bindings belongs to: `?` (Help) is listed for
/// the Hub, the terminal and the agent tabs alike, each with its own key.
pub fn for_place(place: Place) -> impl Iterator<Item = &'static Placed> {
    static BY_PLACE: OnceLock<Vec<Vec<Placed>>> = OnceLock::new();
    BY_PLACE
        .get_or_init(|| {
            let table = keymap::table();
            Place::ALL
                .iter()
                .map(|place| {
                    entries::ENTRIES
                        .iter()
                        .filter_map(|entry| {
                            placed(entry, &table, |spec| {
                                Place::of_context(spec.context) == Some(*place)
                            })
                        })
                        .collect()
                })
                .collect()
        })
        .get(place.index())
        .into_iter()
        .flatten()
}

/// Every entry with every key it has anywhere, in catalogue order. An entry with no binding at
/// all is still listed, with no keys.
pub fn all() -> impl Iterator<Item = &'static Placed> {
    static ALL: OnceLock<Vec<Placed>> = OnceLock::new();
    ALL.get_or_init(|| {
        let table = keymap::table();
        entries::ENTRIES
            .iter()
            .map(|entry| {
                placed(entry, &table, |_| true).unwrap_or(Placed {
                    entry,
                    bindings: Vec::new(),
                    keys: Vec::new(),
                })
            })
            .collect()
    })
    .iter()
}

/// The entry's bindings that pass `keep`, or `None` when there are none.
fn placed(
    entry: &'static Entry,
    table: &[BindingSpec],
    keep: impl Fn(&BindingSpec) -> bool,
) -> Option<Placed> {
    // Members in range order first, then table order within a member, so a range's keys read
    // `1 … 9` whatever order the table registered them in.
    let bindings: Vec<BindingSpec> = entry
        .actions
        .iter()
        .flat_map(|action| {
            table
                .iter()
                .filter(move |spec| spec.action == *action)
                .copied()
        })
        .filter(|spec| keep(spec))
        .collect();
    if bindings.is_empty() {
        return None;
    }
    let mut keys: Vec<&'static str> = Vec::new();
    for spec in &bindings {
        if !keys.contains(&spec.keys) {
            keys.push(spec.keys);
        }
    }
    Some(Placed {
        entry,
        bindings,
        keys,
    })
}

#[cfg(test)]
mod tests;
