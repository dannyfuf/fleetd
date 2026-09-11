//! The single live activity row: which entries it subsumes, and what tense it speaks in.
//!
//! `spec-B` §B1.2: the live row is emitted at the position of the **last visible work entry of
//! the active turn** and subsumes every entry in that contiguous tail. The tail walk **breaks**
//! — it does not skip — on anything that is not a plain work entry of the active turn: an error
//! row, a compaction, a subagent spawn, an entry of another turn. Skipping instead of breaking
//! is how a live row swallows an error nobody then sees.

use fleet_core::agents::{Item, ItemId, ItemKind, ItemStatus};
use fleet_ui_kit::{Icon, WorkLiveRow};
use gpui::SharedString;

use super::item::{kind_word, summary_text, tool_icon};

/// The live tail of the active turn: the entries the live row stands in for.
///
/// The first element is the position the live row is emitted at. An empty result means the turn
/// has no work entry yet, and the working row is the live row instead.
pub(crate) fn tail(items: &[&Item]) -> Vec<usize> {
    let mut tail = Vec::new();
    for (index, item) in items.iter().enumerate().rev() {
        if !subsumable(item) {
            break;
        }
        tail.push(index);
    }
    tail.reverse();
    tail
}

/// Whether one entry may disappear behind the live row.
fn subsumable(item: &Item) -> bool {
    match &item.kind {
        // A plain tool call is the only thing the live row speaks for. A subagent spawn is a
        // roster the reader is watching, an error is news, and prose is the answer.
        ItemKind::Tool(_) => true,
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantText { .. }
        | ItemKind::Reasoning { .. }
        | ItemKind::Subagent { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Error { .. } => false,
    }
}

/// The status the live row speaks in, which is a rule rather than a lookup.
///
/// A *completed* command inside the live row still reads `running cargo`, never `ran cargo`, so
/// the row never flickers between tenses while the turn is alive. Only the three states that
/// are news in their own right survive the collapse.
pub(crate) const fn live_status(status: ItemStatus, turn_active: bool) -> ItemStatus {
    match status {
        ItemStatus::Failed | ItemStatus::Denied | ItemStatus::Stopped => status,
        _ if turn_active => ItemStatus::InProgress,
        ItemStatus::InProgress => ItemStatus::InProgress,
        ItemStatus::Completed => ItemStatus::Completed,
    }
}

/// The live row for one subsumed tail.
pub(crate) fn row(items: &[&Item], tail: &[usize], turn_active: bool) -> Option<WorkLiveRow> {
    let last = items.get(*tail.last()?).copied()?;
    let status = live_status(last.status, turn_active);
    let label = label(last, status, tail.len());
    Some(WorkLiveRow {
        label,
        icon: match &last.kind {
            ItemKind::Tool(call) => tool_icon(&call.kind),
            _ => Icon::Wrench,
        },
        shimmer: status == ItemStatus::InProgress,
    })
}

/// The present-tense label: `running cargo`, `reading src/main.rs`, `editing 3 files`.
fn label(item: &Item, status: ItemStatus, subsumed: usize) -> SharedString {
    let ItemKind::Tool(call) = &item.kind else {
        return SharedString::new_static("working");
    };
    let (participle, infinitive) = verb(&call.kind);
    // A run of edits reads as a count rather than as whichever file happens to be last: the
    // reader is watching progress, and one of five file names is the least useful of the five.
    let subject = if subsumed > 1 && super::group::action_of(item) == super::group::Action::Edit {
        format!("{subsumed} files")
    } else {
        let summary = summary_text(item);
        if summary.is_empty() {
            kind_word(&call.kind)
        } else {
            summary
        }
    };
    match status {
        ItemStatus::Failed => SharedString::from(format!("failed to {infinitive} {subject}")),
        ItemStatus::Denied => SharedString::from(format!("declined to {infinitive} {subject}")),
        ItemStatus::Stopped => SharedString::from(format!("stopped {participle} {subject}")),
        // `InProgress` and `Completed` are the same sentence on purpose: a completed command
        // inside a live row still reads `running cargo`, so the row never changes tense while
        // the turn is alive.
        ItemStatus::InProgress | ItemStatus::Completed => {
            SharedString::from(format!("{participle} {subject}"))
        }
    }
}

/// What one kind is doing, as a present participle and as a bare infinitive.
///
/// Both spellings come from one place so `running cargo` and `failed to run cargo` can never
/// drift into two different vocabularies.
const fn verb(kind: &fleet_core::agents::ToolKind) -> (&'static str, &'static str) {
    use fleet_core::agents::ToolKind;

    match kind {
        ToolKind::Read => ("reading", "read"),
        ToolKind::Edit | ToolKind::Write => ("editing", "edit"),
        ToolKind::Bash => ("running", "run"),
        ToolKind::Search | ToolKind::Grep => ("searching", "search"),
        ToolKind::Fetch => ("fetching", "fetch"),
        ToolKind::Agent => ("delegating", "delegate"),
        ToolKind::Todo => ("updating", "update"),
        ToolKind::Skill => ("loading", "load"),
        ToolKind::Mcp { .. } | ToolKind::Unknown { .. } => ("calling", "call"),
    }
}

/// Which items a live tail hides, as ids, for the emission loop to skip.
pub(crate) fn subsumed(items: &[&Item], tail: &[usize]) -> Vec<ItemId> {
    tail.iter()
        .filter_map(|index| items.get(*index).map(|item| item.id))
        .collect()
}
