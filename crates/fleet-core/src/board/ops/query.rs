use super::*;

/// Nonarchived cards in stable column order.
#[must_use]
pub fn column_cards<'a>(cards: &'a [Card], status_id: &StatusId) -> Vec<&'a Card> {
    let mut column: Vec<_> = cards
        .iter()
        .filter(|c| !c.archived && c.status_id == *status_id)
        .collect();
    column.sort_by(|a, b| {
        (a.position, &a.created_at, a.number).cmp(&(b.position, &b.created_at, b.number))
    });
    column
}
/// First configured status in the requested category.
#[must_use]
pub fn first_status_in(board: &Board, category: StatusCategory) -> Option<&Status> {
    board.statuses.iter().find(|s| s.category == category)
}
/// Renders the branch template as a nonempty slug of at most 48 ASCII characters.
///
/// The result also names a Git branch, so it never carries a `.`: a trailing period, a `..`
/// pair or a `.lock` component are all rejected by `validate_branch`, and an ordinary title
/// ("Fix the login bug.") must not be able to fail the whole card → worktree flow.
#[must_use]
pub fn worktree_slug(board: &Board, card: &Card) -> String {
    let rendered = board
        .settings
        .branch_template
        .replace("{key}", &card.display_key(board))
        .replace("{slug}", &crate::slug::slugify(&card.title));
    let mut slug = branch_safe_slug(&rendered);
    if slug.is_empty() {
        slug = branch_safe_slug(&card.local_key(board));
    }
    if slug.is_empty() {
        slug = "card".into();
    }
    slug.truncate(48);
    let trimmed = slug.trim_matches(['-', '_']);
    if trimmed.is_empty() {
        "card".into()
    } else {
        trimmed.to_owned()
    }
}

/// Slugifies `value` and folds the dots a slug allows but a branch name cannot carry.
fn branch_safe_slug(value: &str) -> String {
    crate::slug::slugify(&crate::slug::slugify(value).replace('.', "-"))
}
/// The newest run on this card, live or ended.
#[must_use]
pub fn latest_run(card: &Card) -> Option<&CardRun> {
    card.runs.last()
}

/// The cards `card` blocks: the reverse of everyone's `blocked_by`, in stable card order.
///
/// A card stores only what blocks it, because that is the direction a person edits. Every
/// surface that has to answer "what happens when this finishes" — the detail's Blocks row, the
/// engine's re-evaluation seeds — derives the other direction here rather than persisting it,
/// so the two can never disagree.
#[must_use]
pub fn blocks<'a>(cards: &'a [Card], card: &CardId) -> Vec<&'a Card> {
    cards
        .iter()
        .filter(|other| other.blocked_by.iter().any(|blocker| blocker == card))
        .collect()
}

/// Whether a blocker has reached a column that counts as finished.
///
/// Only `Completed` satisfies. A canceled card is a decision not to do the work, not a report
/// that it is done, and letting it release its dependants would start runs on top of work
/// somebody deliberately stopped.
#[must_use]
pub fn is_satisfied(board: &Board, cards: &[Card], blocker: &CardId) -> bool {
    cards
        .iter()
        .find(|card| card.id == *blocker)
        .and_then(|card| board.statuses.iter().find(|s| s.id == card.status_id))
        .is_some_and(|status| status.category == StatusCategory::Completed)
}

/// How many blockers a card is still waiting for, and how loudly to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocked {
    /// Blockers that have not reached a `Completed` column.
    pub unsatisfied: u32,
    /// How a surface should colour the mark.
    pub tone: BlockedTone,
}

/// Whether a block is ordinary waiting or something a person has to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedTone {
    /// Every blocker is still live work; the card is simply waiting its turn.
    Muted,
    /// A blocker was canceled or archived, so this card will never be released on its own.
    Warning,
}

/// What still blocks this card, or `None` when nothing does.
#[must_use]
pub fn blocked(board: &Board, cards: &[Card], card: &Card) -> Option<Blocked> {
    let unsatisfied: Vec<_> = card
        .blocked_by
        .iter()
        .filter(|blocker| !is_satisfied(board, cards, blocker))
        .collect();
    if unsatisfied.is_empty() {
        return None;
    }
    // A blocker nobody can finish — canceled, archived, or no longer on the board — will never
    // release this card on its own, so the mark says so rather than reading as ordinary waiting.
    let stuck = unsatisfied.iter().any(|blocker| {
        cards
            .iter()
            .find(|other| other.id == **blocker)
            .is_none_or(|blocker| {
                blocker.archived
                    || board.statuses.iter().any(|s| {
                        s.id == blocker.status_id && s.category == StatusCategory::Canceled
                    })
            })
    });
    Some(Blocked {
        unsatisfied: count(unsatisfied.iter()),
        tone: if stuck {
            BlockedTone::Warning
        } else {
            BlockedTone::Muted
        },
    })
}

/// Whether this card is waiting on a human.
///
/// Two situations qualify, and both are things a person must decide: the last run ended
/// needing one — and nobody has moved the card by hand since, which is how a human says "seen"
/// — or a run has been owed for at least [`PENDING_AMBER_AFTER_SECS`] without a slot freeing.
/// An outcome move is `AutoMoved`, so only a human's or the CLI's `Moved` clears the first.
#[must_use]
pub fn attention(card: &Card, now: &str) -> bool {
    if card
        .pending_run
        .as_ref()
        .and_then(|pending| elapsed_secs(&pending.since, now))
        .is_some_and(|secs| secs >= PENDING_AMBER_AFTER_SECS)
    {
        return true;
    }
    let Some(run) = latest_run(card) else {
        return false;
    };
    if !run.outcome.is_some_and(RunOutcome::needs_attention) {
        return false;
    }
    let Some(ended) = run.ended_at.as_deref() else {
        return false;
    };
    !card
        .activity
        .iter()
        .any(|entry| entry.kind == ActivityKind::Moved && is_after(&entry.at, ended))
}

/// Seconds between two RFC 3339 stamps, or `None` when either is not one.
fn elapsed_secs(since: &str, now: &str) -> Option<u64> {
    let since = chrono::DateTime::parse_from_rfc3339(since).ok()?;
    let now = chrono::DateTime::parse_from_rfc3339(now).ok()?;
    u64::try_from((now - since).num_seconds()).ok()
}

/// Whether `later` is strictly after `earlier`.
///
/// Board stamps come from one daemon clock in one format, so the text order is the time order;
/// parsing first keeps that true for a document written with a different UTC offset.
fn is_after(later: &str, earlier: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(later),
        chrono::DateTime::parse_from_rfc3339(earlier),
    ) {
        (Ok(later), Ok(earlier)) => later > earlier,
        _ => later > earlier,
    }
}

/// Summarizes nonarchived cards for snapshots.
///
/// `live` is the delegation join a board view carries; a caller with nothing joined passes
/// `&[]` and still counts the runs the cards themselves record.
#[must_use]
pub fn summarize(board: &Board, cards: &[Card], live: &[LiveRun], now: &str) -> BoardSummary {
    let cards: Vec<_> = cards.iter().filter(|c| !c.archived).collect();
    BoardSummary {
        id: board.id.clone(),
        context_id: board.context_id.clone(),
        worktree_id: board.worktree_id.clone(),
        name: board.name.clone(),
        prefix: board.prefix.clone(),
        backend_kind: board.backend.kind.clone(),
        card_count: cards.len(),
        open_count: cards
            .iter()
            .filter(|c| {
                board.statuses.iter().any(|s| {
                    s.id == c.status_id
                        && !matches!(
                            s.category,
                            StatusCategory::Completed | StatusCategory::Canceled
                        )
                })
            })
            .count(),
        dirty_count: cards.iter().filter(|c| c.dirty).count(),
        conflict_count: cards.iter().filter(|c| c.conflict.is_some()).count(),
        working_count: count(cards.iter().filter(|card| {
            card.pending_run.is_some()
                || latest_run(card).is_some_and(CardRun::is_live)
                || live.iter().any(|run| run.card_id == card.id)
        })),
        attention_count: count(cards.iter().filter(|card| attention(card, now))),
        last_synced_at: board.sync.last_synced_at.clone(),
        last_error: board.sync.last_error.clone(),
    }
}

/// A count as every summary field spells it, saturating rather than wrapping on a huge board.
fn count<T>(cards: impl Iterator<Item = T>) -> u32 {
    u32::try_from(cards.count()).unwrap_or(u32::MAX)
}
