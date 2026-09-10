use super::*;

/// Whether a card survives the board filter, against an already trimmed, lowercased needle.
///
/// The match is a case-insensitive **substring** over the four things a card is looked up by —
/// title, key, labels and assignee — for the reason §3.10 gives: a filter that hides rows whose
/// letters you can see is worse than one that ignores the order you typed them in.
///
/// The fold is [`crate::presentation::contains_folded`], the same one the Hub's lists use: it
/// walks the haystack a character at a time instead of allocating a lowercased copy of every
/// title, key, assignee and label of every card on every keystroke. It stays Unicode-aware,
/// which an `eq_ignore_ascii_case` scan would not — a board is allowed to be in Spanish.
#[must_use]
fn matches_needle(board: &Board, card: &Card, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let contains = |text: &str| crate::presentation::contains_folded(text, needle);
    contains(&card.title)
        || card.remote.as_ref().is_some_and(|link| contains(&link.key))
        || contains(&card.local_key(board))
        || card.assignee.as_deref().is_some_and(contains)
        || card.labels.iter().any(|id| {
            board
                .labels
                .iter()
                .find(|label| &label.id == id)
                .is_some_and(|label| contains(&label.name))
                || contains(id.as_str())
        })
}

/// The cards of one column, in contract order, after the filter.
#[must_use]
pub fn visible_cards<'a>(view: &'a BoardView, status: &StatusId, query: &str) -> Vec<&'a Card> {
    let needle = query.trim().to_lowercase();
    ops_column_cards(&view.cards, status)
        .into_iter()
        .filter(|card| matches_needle(&view.board, card, &needle))
        .collect()
}

/// How many cards the whole board shows, and how many it holds (`8/12`).
///
/// Both halves count the same population: a card whose status no column names can never be
/// shown, so counting it in the total made a board with two orphans read `3/5` with no filter
/// set and no filter chip — indistinguishable from a filter hiding two cards. The orphans are
/// named by their own error row, which is where they can actually be acted on.
#[must_use]
pub fn counts(view: &BoardView, query: &str) -> (usize, usize) {
    let columns = grouped_cards(view, query);
    (columns.iter().map(Vec::len).sum(), placed(view))
}

/// The live cards this board has a column for.
#[must_use]
pub(super) fn placed(view: &BoardView) -> usize {
    view.cards
        .iter()
        .filter(|card| {
            !card.archived
                && view
                    .board
                    .statuses
                    .iter()
                    .any(|status| status.id == card.status_id)
        })
        .count()
}

/// Every column's cards, filtered and sorted, indexed by `board.statuses`.
#[must_use]
pub(super) fn grouped_cards<'a>(view: &'a BoardView, query: &str) -> Vec<Vec<&'a Card>> {
    let needle = query.trim().to_lowercase();
    let mut columns = vec![Vec::new(); view.board.statuses.len()];
    for card in view.cards.iter().filter(|card| !card.archived) {
        if let Some(column) = view
            .board
            .statuses
            .iter()
            .position(|status| status.id == card.status_id)
            && matches_needle(&view.board, card, &needle)
        {
            columns[column].push(card);
        }
    }
    for cards in &mut columns {
        cards.sort_by(|a, b| {
            (a.position, &a.created_at, a.number).cmp(&(b.position, &b.created_at, b.number))
        });
    }
    columns
}

/// The kit's priority level for a domain priority.
#[must_use]
pub(crate) const fn priority_level(priority: Priority) -> PriorityLevel {
    match priority {
        Priority::Urgent => PriorityLevel::Urgent,
        Priority::High => PriorityLevel::High,
        Priority::Medium => PriorityLevel::Medium,
        Priority::Low => PriorityLevel::Low,
        Priority::None => PriorityLevel::None,
    }
}

/// The accent a status column wears.
///
/// The kit takes a resolved color because the mapping from a workflow category to a token is
/// domain knowledge: `Started` is the amber "in flight" of §1.4, `Completed` the green, a
/// `Canceled` column the muted grey that says "this is not a failure, it is a dead end".
#[must_use]
pub(super) fn category_accent(status: &Status, theme: &Theme) -> Hsla {
    if let Some(token) = status.color.as_deref()
        && let Some(color) = token_color(token, theme)
    {
        return color;
    }
    match status.category {
        StatusCategory::Backlog => theme.colors.text_muted,
        StatusCategory::Unstarted => theme.colors.text_secondary,
        StatusCategory::Started => theme.colors.warning,
        StatusCategory::Completed => theme.colors.success,
        StatusCategory::Canceled => theme.colors.border_strong,
    }
}

/// A board-supplied token **name** resolved against the theme; unknown names fall back.
fn token_color(token: &str, theme: &Theme) -> Option<Hsla> {
    Some(match token {
        "accent" => theme.colors.accent,
        "success" => theme.colors.success,
        "warning" => theme.colors.warning,
        "danger" => theme.colors.danger,
        "info" => theme.colors.info,
        "muted" => theme.colors.text_muted,
        "secondary" => theme.colors.text_secondary,
        _ => return None,
    })
}

/// The label chips of a card, as the kit wants them: name plus an optional token **name**.
#[must_use]
pub(super) fn label_chips(board: &Board, card: &Card) -> Vec<(SharedString, Option<SharedString>)> {
    card.labels
        .iter()
        .filter_map(|id| board.labels.iter().find(|label| &label.id == id))
        .map(|label| {
            (
                SharedString::from(label.name.clone()),
                label.color.clone().map(SharedString::from),
            )
        })
        .collect()
}

/// The `show_on_card` custom property values of a card, in schema order.
#[must_use]
pub(super) fn card_extras(board: &Board, card: &Card) -> Vec<SharedString> {
    board
        .properties
        .iter()
        .filter(|schema| schema.show_on_card)
        .filter_map(|schema| {
            card.properties
                .get(&schema.key)
                .map(|value| value.display())
                .filter(|value| !value.is_empty())
                .map(|value| SharedString::from(format!("{}: {value}", schema.name)))
        })
        .collect()
}

/// The facts the board header states, derived once and drawn once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderFacts {
    /// Board name.
    pub name: String,
    /// Identifier prefix, e.g. `FLT`.
    pub prefix: String,
    /// What the backend chip reads: the registry's human label for `board.backend.kind`.
    ///
    /// The chip states which system the board mirrors, and `jira` is the key a config file
    /// uses, not the name the product has. The raw kind stands in only until the registry
    /// answers, because a chip with nothing in it reads as a broken header.
    pub backend: String,
    /// Whether that backend is the local no-remote one.
    pub local: bool,
    /// Relative age of `sync.last_synced_at`, when the board was ever synced.
    pub synced: Option<String>,
    /// Cards with unpushed local edits.
    pub dirty: usize,
    /// Cards with an unresolved conflict.
    pub conflicts: usize,
    /// The board's last sync error, verbatim.
    pub error: Option<String>,
}

impl HeaderFacts {
    /// Reads the facts out of a loaded board.
    ///
    /// `backend_label` is the registry's name for this board's backend kind; `None` falls back
    /// to the kind itself, which is what the first frames of a connection have.
    #[must_use]
    pub fn of(view: &BoardView, backend_label: Option<&str>, now: i64) -> Self {
        Self {
            name: view.board.name.clone(),
            prefix: view.board.prefix.clone(),
            backend: backend_label
                .map_or_else(|| view.board.backend.kind.clone(), ToOwned::to_owned),
            local: view.board.backend.is_local(),
            synced: view
                .board
                .sync
                .last_synced_at
                .as_deref()
                .map(|at| crate::presentation::age_label(at, now)),
            // Archived cards are in no column and reachable by no key: counting them would
            // put a chip in the header for rows the board never shows.
            dirty: view
                .cards
                .iter()
                .filter(|card| !card.archived && card.dirty)
                .count(),
            conflicts: view
                .cards
                .iter()
                .filter(|card| !card.archived && card.conflict.is_some())
                .count(),
            error: view.board.sync.last_error.clone(),
        }
    }
}

/// One card, prepared for its tile.
///
/// Every string the tile is built from is allocated here, once per board revision, rather than
/// in the render body: `format!`ing an element id, cloning a title and rebuilding the label and
/// extra chips for 200 cards is a per-frame cost that grows with the board (`gpui-performance`
/// rules 2, 7 and 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRow {
    /// The tile's element id, stable across reorders because it is keyed on the card.
    pub element_id: SharedString,
    /// `FLT-12`.
    pub key: SharedString,
    /// The card title.
    pub title: SharedString,
    /// The priority mark.
    pub priority: PriorityLevel,
    /// Label chips as `(name, color token)`.
    pub labels: Vec<(SharedString, Option<SharedString>)>,
    /// The assignee, when the card has one.
    pub assignee: Option<SharedString>,
    /// The estimate, when the card has one.
    pub estimate: Option<u32>,
    /// The due date, when the card has one.
    pub due: Option<SharedString>,
    /// Whether the card owns a worktree.
    pub worktree: bool,
    /// Whether the card has unpushed local edits.
    pub dirty: bool,
    /// Whether the card has an unresolved conflict.
    pub conflict: bool,
    /// The `show_on_card` custom properties, in schema order.
    pub extras: Vec<SharedString>,
}

impl CardRow {
    /// Prepares one card of `board` for its tile.
    #[must_use]
    fn of(board: &Board, card: &Card) -> Self {
        Self {
            element_id: SharedString::from(format!("board-card-{}", card.id.as_str())),
            key: SharedString::from(card.display_key(board)),
            title: SharedString::from(card.title.clone()),
            priority: priority_level(card.priority),
            labels: label_chips(board, card),
            assignee: card.assignee.clone().map(SharedString::from),
            estimate: card.estimate,
            due: card.due_date.clone().map(SharedString::from),
            worktree: card.worktree_id.is_some(),
            dirty: card.dirty,
            conflict: card.conflict.is_some(),
            extras: card_extras(board, card),
        }
    }
}

/// One status column, prepared for its [`KanbanColumn`].
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnRows {
    /// The column's element id.
    pub element_id: SharedString,
    /// The element id of the click target wrapped around it.
    pub hit_id: SharedString,
    /// The status this column draws, kept whole so the accent stays a theme lookup.
    pub status: Status,
    /// The cards of the column, filtered and in contract order.
    pub rows: Rc<[CardRow]>,
}

/// Everything the board screen draws, derived once per board revision.
///
/// `docs/APP-CONTRACTS.md:101` — *render prepares nothing*. The grouping, the filter, the
/// per-column sort, the orphan scan and every tile string live here, behind
/// `crate::screens::board::projection`'s revision key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardModel {
    /// One entry per `board.statuses`, in contract order.
    pub columns: Rc<[ColumnRows]>,
    /// How many cards the filter leaves showing.
    pub shown: usize,
    /// How many live cards the board has a column for.
    pub total: usize,
    /// Whether the board declares no columns at all.
    pub no_columns: bool,
    /// The orphan row's sentence, when cards reference statuses this board no longer has.
    pub orphans: Option<SharedString>,
    /// The facts the header states.
    pub facts: HeaderFacts,
}

/// Builds the whole board model from a loaded view.
///
/// `backend_label` is the registry's name for the board's backend kind and `now` the epoch
/// second the synced stamp is relative to, exactly as [`HeaderFacts::of`] takes them.
#[must_use]
pub fn build(view: &BoardView, query: &str, backend_label: Option<&str>, now: i64) -> BoardModel {
    let grouped = grouped_cards(view, query);
    let shown = grouped.iter().map(Vec::len).sum();
    let columns: Rc<[ColumnRows]> = view
        .board
        .statuses
        .iter()
        .enumerate()
        .map(|(index, status)| ColumnRows {
            element_id: SharedString::from(format!("board-column-{}", status.id.as_str())),
            hit_id: SharedString::from(format!("board-column-hit-{index}")),
            status: status.clone(),
            rows: grouped[index]
                .iter()
                .map(|card| CardRow::of(&view.board, card))
                .collect(),
        })
        .collect();
    BoardModel {
        columns,
        shown,
        total: placed(view),
        no_columns: view.board.statuses.is_empty(),
        orphans: orphan_sentence(view),
        facts: HeaderFacts::of(view, backend_label, now),
    }
}

/// The sentence the orphan row states, or `None` when every card has a column.
///
/// Reloading returns the same view: only a status this board still has, or a sync that restores
/// the missing one, can place them — which is why the row names the two keys that can.
#[must_use]
fn orphan_sentence(view: &BoardView) -> Option<SharedString> {
    let orphans: Vec<&Card> = view
        .cards
        .iter()
        .filter(|card| {
            !card.archived
                && !view
                    .board
                    .statuses
                    .iter()
                    .any(|status| status.id == card.status_id)
        })
        .collect();
    if orphans.is_empty() {
        return None;
    }
    Some(SharedString::from(format!(
        "{} card(s) reference statuses this board no longer has \u{2014} ,  board settings or \
         S  sync: {}",
        orphans.len(),
        orphans
            .iter()
            .map(|card| format!("{} {}", card.display_key(&view.board), card.title))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}
