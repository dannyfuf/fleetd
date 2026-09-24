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
pub(crate) fn category_accent(status: &Status, theme: &Theme) -> Hsla {
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

/// What one tile says about its card's run and its blockers.
///
/// The kit's vocabulary and nothing else. The fold from a card's runs, its owed run, the
/// delegation mirror and its links happens once per change in `AppState::refresh_card_marks`
/// (contracts §5.2); this is the copy of its answer the view layer draws, so the board model
/// depends on the kit alone and [`build`] stays callable from a fixture with no `AppState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileMark {
    /// The run mark the tile draws, when the card has one.
    pub run: Option<RunMark>,
    /// How many cards still block it, and how loudly to say so.
    pub blocked: Option<(u32, BlockedTone)>,
}

/// Every mark one board model draws, handed to [`build`] once per rebuild.
///
/// The projection hands this over rather than the model reaching for state: *render prepares
/// nothing*, and neither does the model — both halves are already derived when they arrive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardMarks {
    /// What each marked card says; a card with nothing to say is absent.
    pub by_card: HashMap<CardId, TileMark>,
    /// Cards holding or owed a run slot — the header's numerator.
    pub working: u32,
    /// Of those, the cards only owed one: waiting for a slot to free.
    pub waiting: u32,
    /// Cards waiting on a person — the header's amber count.
    pub needs_you: u32,
    /// The branch, and its pull request, of every worktree a card on this board links.
    ///
    /// The worktree list and the PR badges live in the app state, not in the board view, so the
    /// projection reads them once and hands them over with the marks.
    pub links: HashMap<WorktreeId, LinkedBranch>,
}

/// The branch a card's worktree is on, as its tile names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedBranch {
    /// The branch name.
    pub branch: SharedString,
    /// Its open pull request, when the app knows of one.
    pub pr: Option<(u64, PrBadgeState)>,
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
    /// Cards holding or owed a run slot.
    pub working: u32,
    /// Of those, the cards only owed one.
    pub waiting: u32,
    /// How many runs this board lets itself hold at once (`BoardSettings::max_live_runs`).
    pub live_limit: u32,
    /// Cards waiting on a person.
    pub needs_you: u32,
    /// `1 of 2 runs working`, or `1 working · 1 waiting` while a card is owed a run the limit
    /// has no slot for; already composed, or `None` while nothing is running or owed.
    ///
    /// The string is built here rather than in the header body for the reason every other
    /// string on this screen is: `docs/APP-CONTRACTS.md:101` — *render prepares nothing*.
    pub working_label: Option<String>,
    /// `1 needs you`, already composed, or `None` while no card is waiting on anybody.
    pub needs_you_label: Option<String>,
}

impl HeaderFacts {
    /// Reads the facts out of a loaded board.
    ///
    /// `backend_label` is the registry's name for this board's backend kind; `None` falls back
    /// to the kind itself, which is what the first frames of a connection have. `marks` is the
    /// fold the app state already did; the two counts are only stated while they are non-zero,
    /// so a board with no automation keeps exactly the header it has today.
    #[must_use]
    pub fn of(view: &BoardView, backend_label: Option<&str>, now: i64, marks: &BoardMarks) -> Self {
        let live_limit = view.board.settings.max_live_runs();
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
            working: marks.working,
            waiting: marks.waiting,
            live_limit,
            needs_you: marks.needs_you,
            working_label: working_label(marks.working, marks.waiting, live_limit),
            needs_you_label: (marks.needs_you > 0)
                .then(|| format!("{} needs you", marks.needs_you)),
        }
    }
}

/// The header's run count, or `None` while no card holds or is owed a slot.
///
/// While every counted card holds its slot the count is read against the limit, `1 of 2 runs
/// working`. Once one is only owed a run, `N of M` would put more over the limit than it
/// allows (`2 of 1 run working`), so the two are stated apart — `1 working · 1 waiting` — and
/// either half is left out at zero.
fn working_label(working: u32, waiting: u32, live_limit: u32) -> Option<String> {
    if working == 0 {
        return None;
    }
    let live = working.saturating_sub(waiting);
    if waiting == 0 {
        let runs = if live_limit == 1 { "run" } else { "runs" };
        return Some(format!("{live} of {live_limit} {runs} working"));
    }
    let waiting = format!("{waiting} waiting");
    Some(if live == 0 {
        waiting
    } else {
        format!("{live} working \u{b7} {waiting}")
    })
}

/// One card, prepared for its tile.
///
/// Every string the tile is built from is allocated here, once per board revision, rather than
/// in the render body: `format!`ing an element id, cloning a title and rebuilding the label and
/// extra chips for 200 cards is a per-frame cost that grows with the board (`gpui-performance`
/// rules 2, 7 and 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRow {
    /// The card, which a drag carries to the column it is dropped on.
    pub id: CardId,
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
    /// What the card's run says, when it has one.
    pub run: Option<RunMark>,
    /// The run pill's words: `working 4m · codex`, `review passed`.
    pub run_label: Option<SharedString>,
    /// How many cards still block it, and how loudly to say so.
    pub blocked: Option<(u32, BlockedTone)>,
    /// The blocked pill's words: `blocked by FLT-5`.
    pub blocked_label: Option<SharedString>,
    /// The linked worktree's branch and pull request.
    pub link: Option<LinkedBranch>,
    /// The card's own pull request, `owner/name#123`, when it has one (a Reviews board's card):
    /// the tile's reference line.
    pub reference: Option<SharedString>,
    /// Which of the card's menu entries can do something for it.
    pub menu: CardMenu,
}

/// Which card actions can do something for one card, for its `⋯` and right-click menu.
///
/// The same rules the keys refuse by, answered once per rebuild so the menu lists only what
/// works (DESIGN-SYSTEM §4: an entry that could only refuse is left out, not greyed). The key
/// stays bound either way and still says why when it is pressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CardMenu {
    /// `o`: the card links a worktree.
    pub open_worktree: bool,
    /// `x`: the card's remote issue has an address.
    pub open_remote: bool,
    /// `B` and `y`: the card is a review card with a pull request (BOARD §11.9).
    pub pull_request: bool,
    /// `d`: the card is Fleet's own — a mirrored card is deleted in its backend.
    pub delete: bool,
    /// `A`: the card has a run a thread can be attached from.
    pub attach: bool,
    /// `X`: the card has a live run, or is owed one.
    pub cancel: bool,
    /// `>`: the card's column runs an action.
    pub run_now: bool,
}

impl CardMenu {
    /// What a card's menu may offer, from the card and the run mark its tile draws.
    ///
    /// The board's tile menu and the card detail's controls both ask this, so a verb the one
    /// hides the other hides too.
    #[must_use]
    pub fn of(board: &Board, card: &Card, run: Option<RunMark>) -> Self {
        let has_action = |status: &StatusId| {
            board
                .statuses
                .iter()
                .find(|column| &column.id == status)
                .and_then(|column| column.automation.as_ref())
                .is_some_and(|automation| automation.on_enter.is_some())
        };
        let live = card.pending_run.is_some()
            || card.runs.last().is_some_and(CardRun::is_live)
            || matches!(
                run,
                Some(RunMark::Pending | RunMark::Stalled | RunMark::Working)
            );
        Self {
            open_worktree: card.worktree_id.is_some(),
            open_remote: crate::screens::board::remote_url(card).is_some(),
            pull_request: card.pull_request.is_some(),
            delete: card.remote.is_none(),
            attach: run.is_some() || card.runs.iter().any(|run| run.thread_id.is_some()),
            cancel: live,
            run_now: has_action(&card.status_id),
        }
    }
}

/// The standard card fields a board's backend owns, for the pickers the menu leaves out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadonlyFields {
    /// `status_id`: `s`, `[` and `]`.
    pub status: bool,
    /// `priority`: `p`.
    pub priority: bool,
    /// `assignee`: `a`.
    pub assignee: bool,
    /// `labels`: `t`.
    pub labels: bool,
    /// `estimate`: `e`.
    pub estimate: bool,
}

impl ReadonlyFields {
    /// The fields `board`'s backend cannot write back; none on a local board.
    #[must_use]
    pub fn of(board: &Board) -> Self {
        let fields: &[String] = if board.backend.is_local() {
            &[]
        } else {
            &board.sync.readonly_fields
        };
        let has = |name: &str| fields.iter().any(|field| field == name);
        Self {
            status: has("status_id"),
            priority: has("priority"),
            assignee: has("assignee"),
            labels: has("labels"),
            estimate: has("estimate"),
        }
    }
}

impl CardRow {
    /// Prepares one card of `board` for its tile.
    ///
    /// The marks are read out of the map the projection was handed, not derived here: a tile
    /// mark is a fold over the delegation mirror, and doing it per card per rebuild would
    /// answer the same question once per card instead of once per change.
    #[must_use]
    fn of(view: &BoardView, card: &Card, marks: &BoardMarks, now: i64) -> Self {
        let board = &view.board;
        let mark = marks.by_card.get(&card.id).copied().unwrap_or_default();
        let link = card
            .worktree_id
            .as_ref()
            .and_then(|worktree| marks.links.get(worktree))
            .cloned();
        let menu = CardMenu::of(board, card, mark.run);
        Self {
            id: card.id.clone(),
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
            run: mark.run,
            run_label: mark.run.map(|run| run_label(board, card, run, now)),
            blocked: mark.blocked,
            blocked_label: mark
                .blocked
                .map(|(count, _)| blocked_label(view, card, count)),
            link,
            reference: card
                .pull_request
                .as_ref()
                .map(|pull_request| SharedString::from(pull_request.key())),
            menu,
        }
    }
}

/// The run pill's words for one card: what its run is doing, for how long, and with whom.
///
/// `working 4m · codex` while a run is live — the age from the run's own start stamp, the
/// provider from its row — and `review passed` when the run succeeded and the card stayed, named
/// after what the column that ran it does. A mark the card's own rows cannot date yet (a child
/// the delegation mirror announced before the board reloaded) says only its word.
#[must_use]
pub(super) fn run_label(board: &Board, card: &Card, mark: RunMark, now: i64) -> SharedString {
    let newest = card.runs.last();
    let with_provider = |word: String| match newest {
        Some(run) => format!("{word} \u{b7} {}", provider_word(run.provider)),
        None => word,
    };
    SharedString::from(match mark {
        RunMark::Working => match newest.filter(|run| run.is_live()) {
            Some(run) => with_provider(format!(
                "working {}",
                crate::presentation::age_label(&run.started_at, now)
            )),
            None => "working".to_owned(),
        },
        RunMark::Pending => "waiting".to_owned(),
        RunMark::Stalled => card.pending_run.as_ref().map_or_else(
            || "waiting".to_owned(),
            |pending| {
                format!(
                    "waiting {}",
                    crate::presentation::age_label(&pending.since, now)
                )
            },
        ),
        RunMark::NeedsYou => "needs you".to_owned(),
        RunMark::Succeeded => newest
            .and_then(|run| {
                board
                    .statuses
                    .iter()
                    .find(|status| status.id == run.status_id)
            })
            .and_then(|status| status.automation.as_ref()?.on_enter.as_ref())
            .map_or_else(|| "run passed".to_owned(), passed_word),
    })
}

/// The blocked pill's words: the one blocker by key, or how many there are.
#[must_use]
pub(super) fn blocked_label(view: &BoardView, card: &Card, count: u32) -> SharedString {
    if count == 1
        && let Some(blocker) = card
            .blocked_by
            .iter()
            .find(|blocker| !is_satisfied(&view.board, &view.cards, blocker))
            .and_then(|blocker| view.cards.iter().find(|other| &other.id == blocker))
    {
        return SharedString::from(format!("blocked by {}", blocker.display_key(&view.board)));
    }
    SharedString::from(format!("blocked by {count} cards"))
}

/// How a provider is named in a sentence.
#[must_use]
const fn provider_word(provider: AgentKind) -> &'static str {
    match provider {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
}

/// The verb a column's action is read as, in the third person: `implements`, `reviews`.
///
/// A skill is named by what its name says it does; a prompt by the first word of its
/// instructions (`Implement this card…`). Anything this table does not know reads `runs the
/// card` or `runs <skill>`, which is true of every action.
#[must_use]
fn action_verb(action: &ColumnAction) -> String {
    const VERBS: [(&str, &str); 8] = [
        ("implement", "implements"),
        ("review", "reviews"),
        ("fix", "fixes"),
        ("test", "tests"),
        ("plan", "plans"),
        ("write", "writes"),
        ("document", "documents"),
        ("refactor", "refactors"),
    ];
    let known = |word: &str| {
        let word = word.to_lowercase();
        VERBS
            .iter()
            .find(|(stem, _)| word.contains(stem))
            .map(|(_, verb)| (*verb).to_owned())
    };
    match &action.kind {
        ActionKind::Skill { name, .. } => known(name).unwrap_or_else(|| format!("runs {name}")),
        ActionKind::Prompt => action
            .instructions
            .split_whitespace()
            .next()
            .and_then(known)
            .unwrap_or_else(|| "runs the card".to_owned()),
    }
}

/// What a succeeded run of `action` is called: `review passed`, `implements passed` would not
/// read, so a verb gets its noun.
#[must_use]
fn passed_word(action: &ColumnAction) -> String {
    match action_verb(action).as_str() {
        "reviews" => "review passed".to_owned(),
        "tests" => "tests passed".to_owned(),
        "implements" => "implemented".to_owned(),
        "fixes" => "fixed".to_owned(),
        _ => "run passed".to_owned(),
    }
}

/// The pill a column with an `on_enter` action wears: `On enter: codex implements`.
///
/// The provider is the column's own; a column that leaves it to the card or the daemon's default
/// reads the verb alone rather than guess which agent will answer.
#[must_use]
pub(super) fn automation_label(status: &Status) -> Option<SharedString> {
    let action = status.automation.as_ref()?.on_enter.as_ref()?;
    let verb = action_verb(action);
    Some(SharedString::from(match action.agent.provider {
        Some(provider) => format!("On enter: {} {verb}", provider_word(provider)),
        None => format!("On enter: agent {verb}"),
    }))
}

/// Who picks up a card dropped into a column with an `on_enter` action: `codex will pick it
/// up`, the second half of the drop slot's `Drop to start FLT-3 · codex will pick it up`.
#[must_use]
pub(super) fn pickup_phrase(status: &Status) -> Option<SharedString> {
    let action = status.automation.as_ref()?.on_enter.as_ref()?;
    Some(SharedString::from(match action.agent.provider {
        Some(provider) => format!("{} will pick it up", provider_word(provider)),
        None => "an agent will pick it up".to_owned(),
    }))
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
    /// Whether entering this column starts something — the header's muted `⚡`.
    ///
    /// Only `on_enter` counts: `on_success` and `advance_when_unblocked` move a card the column
    /// is already done with, and a glyph promising a run for one of those would lie.
    pub has_action: bool,
    /// The pill naming that action in words: `On enter: codex implements`.
    pub automation: Option<SharedString>,
    /// Who picks up a card dropped here, when entering the column starts a run.
    pub pickup: Option<SharedString>,
    /// The `+` button's name, its tooltip: `Add a card to Todo`.
    pub add_label: SharedString,
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
    /// The subtitle under the board's name: `8 cards · 1 of 2 runs working`, or `3 of 8 cards`
    /// while a filter hides some.
    pub summary: SharedString,
    /// The first card waiting on a person, as `(column, row)`: where `1 needs you` goes.
    pub needs_you_at: Option<(usize, usize)>,
    /// The card fields this board's backend owns.
    pub readonly: ReadonlyFields,
    /// The header's schedules strip, when the board has schedules and the daemon runs them.
    pub schedules: Option<ScheduleStrip>,
}

impl BoardModel {
    /// The model with its schedules strip, which the projection folds from the schedules
    /// mirror beside [`build`]: the mirror is app state, and the model is built from the view.
    #[must_use]
    pub fn with_schedules(mut self, schedules: Option<ScheduleStrip>) -> Self {
        self.schedules = schedules;
        self
    }
}

/// Builds the whole board model from a loaded view.
///
/// `backend_label` is the registry's name for the board's backend kind and `now` the epoch
/// second the synced stamp is relative to, exactly as [`HeaderFacts::of`] takes them. `marks`
/// is the app state's already-folded [`BoardMarks`], keyed into the projection by its own
/// revision so a mark that moves rebuilds the model and a headline that does not never will.
#[must_use]
pub fn build(
    view: &BoardView,
    query: &str,
    backend_label: Option<&str>,
    now: i64,
    marks: &BoardMarks,
) -> BoardModel {
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
                .map(|card| CardRow::of(view, card, marks, now))
                .collect(),
            has_action: status
                .automation
                .as_ref()
                .is_some_and(|automation| automation.on_enter.is_some()),
            automation: automation_label(status),
            pickup: pickup_phrase(status),
            add_label: SharedString::from(format!("Add a card to {}", status.name)),
        })
        .collect();
    // The first card, in board order, that waits on a person: where the header's amber count
    // takes the cursor.
    let needs_you_at = columns.iter().enumerate().find_map(|(column, rows)| {
        rows.rows
            .iter()
            .position(|row| matches!(row.run, Some(RunMark::NeedsYou | RunMark::Stalled)))
            .map(|row| (column, row))
    });
    let total = placed(view);
    let facts = HeaderFacts::of(view, backend_label, now, marks);
    let cards = if shown == total {
        format!("{total} {}", if total == 1 { "card" } else { "cards" })
    } else {
        format!("{shown} of {total} cards")
    };
    let summary = SharedString::from(match &facts.working_label {
        Some(working) => format!("{cards} \u{b7} {working}"),
        None => cards,
    });
    BoardModel {
        needs_you_at,
        summary,
        readonly: ReadonlyFields::of(&view.board),
        columns,
        shown,
        total,
        no_columns: view.board.statuses.is_empty(),
        orphans: orphan_sentence(view),
        facts,
        schedules: None,
    }
}

/// The sentence the orphan row states, or `None` when every card has a column.
///
/// Reloading returns the same view: only a status this board still has, or a sync that restores
/// the missing one, can place them — which is why the callout carries the two buttons that can.
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
    // The callout carries the two buttons that can place them — Board settings and Sync — so
    // the sentence states the fact and names the cards, and spells no key.
    Some(SharedString::from(format!(
        "{} card(s) reference statuses this board no longer has: {}",
        orphans.len(),
        orphans
            .iter()
            .map(|card| format!("{} {}", card.display_key(&view.board), card.title))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}
