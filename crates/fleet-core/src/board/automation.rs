//! The pure board-automation engine: what a column start, a throttle and a cascade decide.
//!
//! Nothing here touches a daemon, a provider or a clock. Every input is a value the caller
//! already holds and `now` is passed in, so the whole engine is table-testable and the service
//! above it is left with the side effects: reserving a slot, spawning a child, writing a board.

use std::collections::{BTreeSet, VecDeque};

use crate::{
    agents::{AgentKind, PermissionMode},
    board::{
        defaults::render_card_template,
        model::{
            Action, ActionKind, ActivityKind, Board, Card, CardRun, Comment, PendingRun,
            RunOutcome, Status,
        },
        ops::{BoardError, blocks, is_satisfied, latest_run, move_card, push_activity, queued},
    },
    ids::{CardId, StatusId},
};

#[cfg(test)]
mod tests;

/// The cards a board already has a run in flight for.
///
/// Built once per evaluation and consulted per card: the throttle asks both how many runs are
/// live and whether this particular card is one of them, and a linear scan per visited card
/// would make a cascade quadratic on a large board.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveIndex {
    live: BTreeSet<CardId>,
}

impl LiveIndex {
    /// Indexes every card whose newest run has not ended.
    #[must_use]
    pub fn from_runs(cards: &[Card]) -> Self {
        Self {
            live: cards
                .iter()
                .filter(|card| latest_run(card).is_some_and(CardRun::is_live))
                .map(|card| card.id.clone())
                .collect(),
        }
    }

    /// Whether this card already has a live run.
    #[must_use]
    pub fn contains(&self, card: &CardId) -> bool {
        self.live.contains(card)
    }

    /// How many cards have a live run.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Whether no card has a live run.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

/// One run the caller must start outside the gate that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct StartRun {
    /// The card to run.
    pub card: CardId,
    /// The column the card was in when the run was decided.
    pub status_id: StatusId,
    /// What that column runs.
    pub action: Action,
}

/// Everything one evaluation decided, for the caller to apply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Runs to start, in the order they were decided.
    pub starts: Vec<StartRun>,
    /// Cards parked behind the board's live-run ceiling.
    pub queued: Vec<CardId>,
    /// Cards the cascade moved, and where to.
    pub moved: Vec<(CardId, StatusId)>,
}

/// The agent one run asks for, after the card, the column and the action have had their say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPrefs {
    /// `None` leaves the provider to the daemon's configured default.
    pub provider: Option<AgentKind>,
    /// `None` leaves the model to the provider's default.
    pub model: Option<String>,
    /// `None` leaves the reasoning effort to the provider's default.
    pub effort: Option<String>,
    /// Permission mode is a workflow policy, so it comes from the column alone.
    pub mode: PermissionMode,
}

/// Walks the board out from `seeds`, starting what a column asks for and cascading what that
/// unblocks.
///
/// Breadth-first over the derived `blocks` index with a `seen` set, so a diamond is visited once
/// and a cycle a user built by hand terminates. `cards` is mutated in memory — moves, pending
/// runs and activity — and the runs to start come back in the [`Plan`] so the caller can apply
/// them outside the gate that serialises this evaluation. `in_flight` is the reservation set:
/// every [`StartRun`] is inserted into it before it is returned.
///
/// A `pending_run` this evaluation decided to start is *not* cleared here: the run does not
/// exist until the caller has spoken to the delegation service, and the caller clears the
/// marker in the same write that records the [`CardRun`].
///
/// # Errors
///
/// Returns the [`BoardError`] of any in-memory move the cascade performs.
pub fn re_evaluate(
    board: &Board,
    cards: &mut [Card],
    seeds: &[CardId],
    live: &LiveIndex,
    in_flight: &mut BTreeSet<CardId>,
    now: &str,
) -> Result<Plan, BoardError> {
    Walk {
        live,
        in_flight,
        settled: BTreeSet::new(),
        seen: BTreeSet::new(),
        queue: seeds.iter().cloned().collect(),
        plan: Plan::default(),
    }
    .walk(board, cards, now)
}

/// Walks the board from cards that did **not** enter their column: rule 1 is skipped for the
/// seeds themselves, and applied to every card the cascade moves.
///
/// A column runs its action *on entry*. A run that ends where it started leaves its card in a
/// column the card entered before the run — it has entered nothing — so seeding it through
/// [`re_evaluate`] would start the same run again the moment the last one ended, and again when
/// that one ended, for as long as the card stood there. What the ending *can* still change is
/// the satisfaction of the cards this one blocks, which is rule 2; those are walked exactly as
/// they are from any other seed, and a card the cascade moves has entered its new column and
/// gets both rules.
///
/// `>` ([`crate`] users: `start_run`) stays the one verb that runs a card standing still, which
/// is what makes a terminal run's outcome something a person reads rather than something the
/// board immediately overwrites.
///
/// # Errors
///
/// Returns the [`BoardError`] of any in-memory move the cascade performs.
pub fn re_evaluate_settled(
    board: &Board,
    cards: &mut [Card],
    seeds: &[CardId],
    live: &LiveIndex,
    in_flight: &mut BTreeSet<CardId>,
    now: &str,
) -> Result<Plan, BoardError> {
    Walk {
        live,
        in_flight,
        settled: seeds.iter().cloned().collect(),
        seen: BTreeSet::new(),
        queue: seeds.iter().cloned().collect(),
        plan: Plan::default(),
    }
    .walk(board, cards, now)
}

/// One breadth-first evaluation: what it may not start twice, and what it has decided so far.
struct Walk<'a> {
    /// The cards the caller already knows are running.
    live: &'a LiveIndex,
    /// The reservation, shared with every other evaluation this service is running.
    in_flight: &'a mut BTreeSet<CardId>,
    /// Seeds that did not enter their column, so rule 1 is not theirs to answer.
    settled: BTreeSet<CardId>,
    /// Cards already visited, so a diamond is visited once and a cycle terminates.
    seen: BTreeSet<CardId>,
    /// Cards still to visit.
    queue: VecDeque<CardId>,
    /// What the caller must apply once the gate is released.
    plan: Plan,
}

impl Walk<'_> {
    /// Visits every reachable card, applying rules 1 and 0 and then rule 2 to each.
    fn walk(mut self, board: &Board, cards: &mut [Card], now: &str) -> Result<Plan, BoardError> {
        while let Some(card_id) = self.queue.pop_front() {
            // A diamond reaches the same card down two arms and a hand-built cycle reaches it
            // down its own; both stop here rather than starting a second run or walking forever.
            if !self.seen.insert(card_id.clone()) {
                continue;
            }
            let Some(index) = cards.iter().position(|card| card.id == card_id) else {
                continue;
            };
            // A settled seed is standing where it already was, and a column runs on entry: only
            // what it blocks can have changed. Anything the cascade moves is not settled, so the
            // column it lands in still gets its say.
            if !self.settled.contains(&card_id) {
                self.start_on_entry(board, cards, index, now);
                // Rule 0 moves the card this visit is already standing on, and `seen` would stop
                // the queue from visiting it again, so the column it lands in answers here. The
                // bound is the number of columns: a hand-built loop of routing columns stops
                // after one lap instead of spinning.
                let mut hops = 0;
                while hops < board.statuses.len()
                    && self.advance_on_entry(board, cards, index, now)?
                {
                    self.start_on_entry(board, cards, index, now);
                    hops += 1;
                }
            }
            self.advance_dependants(board, cards, &card_id, now)?;
        }
        Ok(self.plan)
    }

    /// Rule 1: what this card's own column asks for, if a slot is free.
    fn start_on_entry(&mut self, board: &Board, cards: &mut [Card], index: usize, now: &str) {
        let card = &cards[index];
        if card.archived {
            return;
        }
        let Some(action) = on_enter(board, &card.status_id) else {
            return;
        };
        // A card already working is not started again, however it re-entered the column: `live`
        // is the join the caller holds, `runs` is what the document itself remembers, and
        // `in_flight` is the run this evaluation — or an earlier one — has already promised.
        if self.live.contains(&card.id)
            || self.in_flight.contains(&card.id)
            || latest_run(card).is_some_and(CardRun::is_live)
        {
            return;
        }
        let id = card.id.clone();
        let status_id = card.status_id.clone();
        let action = action.clone();
        if self.live.len() + self.in_flight.len() >= board.settings.max_live_runs() as usize {
            self.park(cards, index, status_id, now);
            return;
        }
        let message = run_started(&resolve_prefs(card, &action));
        self.in_flight.insert(id.clone());
        self.plan.starts.push(StartRun {
            card: id,
            status_id,
            action,
        });
        push_activity(
            &mut cards[index],
            ActivityKind::RunStarted,
            None,
            message,
            now,
        );
    }

    /// Parks a card behind the board's ceiling, owing it the run its column asked for.
    ///
    /// Nothing is announced: a card waiting for a slot has had nothing happen to it yet, and an
    /// activity entry per throttled move would bury the history the runs themselves write.
    fn park(&mut self, cards: &mut [Card], index: usize, status_id: StatusId, now: &str) {
        let card = &mut cards[index];
        // A card already waiting for this same column keeps the `since` it has waited from:
        // `next_pending` hands a freed slot to the oldest wait, and restamping it here would
        // send a card to the back of the queue every time anything else on the board moved.
        if card
            .pending_run
            .as_ref()
            .is_none_or(|pending| pending.status_id != status_id)
        {
            card.pending_run = Some(PendingRun {
                status_id,
                since: now.into(),
            });
            card.updated_at = now.into();
        }
        if !self.plan.queued.contains(&card.id) {
            self.plan.queued.push(card.id.clone());
        }
    }

    /// Rule 0: a card standing in a routing column with nothing blocking it moves on at once, or
    /// is queued there for the slot the column it is bound for needs.
    ///
    /// Returns whether the card moved, so the walk can let the column it landed in answer.
    fn advance_on_entry(
        &mut self,
        board: &Board,
        cards: &mut [Card],
        index: usize,
        now: &str,
    ) -> Result<bool, BoardError> {
        let card = &cards[index];
        if card.archived {
            return Ok(false);
        }
        let Some(target) = advance_target(board, &card.status_id) else {
            return Ok(false);
        };
        if self.live.contains(&card.id)
            || self.in_flight.contains(&card.id)
            || latest_run(card).is_some_and(CardRun::is_live)
        {
            return Ok(false);
        }
        // A card owed a run by the column it stands in has already arrived: advancing it would
        // strand that run in a column the card has left, exactly as rule 2 refuses to.
        if card
            .pending_run
            .as_ref()
            .is_some_and(|pending| pending.status_id == card.status_id)
        {
            return Ok(false);
        }
        if card
            .blocked_by
            .iter()
            .any(|blocker| !is_satisfied(board, cards, blocker))
        {
            return Ok(false);
        }
        let reason = if queued(card) {
            "a run slot freed"
        } else {
            "nothing blocks it"
        };
        let message = format!("Moved to {}: {reason}", column_name(board, &target));
        self.try_advance(board, cards, index, &target, message, now)
    }

    /// Moves a card out of its routing column into `target`, or queues it there when `target`
    /// runs something and the board has no slot left.
    ///
    /// A queued card keeps its place in line: [`Walk::park`] leaves an older `since` for the same
    /// column alone. Returns whether the card moved.
    fn try_advance(
        &mut self,
        board: &Board,
        cards: &mut [Card],
        index: usize,
        target: &StatusId,
        message: String,
        now: &str,
    ) -> Result<bool, BoardError> {
        if on_enter(board, target).is_some()
            && self.live.len() + self.in_flight.len() >= board.settings.max_live_runs() as usize
        {
            self.park(cards, index, target.clone(), now);
            return Ok(false);
        }
        let id = cards[index].id.clone();
        if !move_card(board, cards, &id, target, None, now)? {
            return Ok(false);
        }
        record_auto_move(&mut cards[index], message, now);
        self.plan.moved.push((id.clone(), target.clone()));
        self.queue.push_back(id);
        Ok(true)
    }

    /// Rule 2: every card this one blocks whose column releases it once nothing blocks it.
    fn advance_dependants(
        &mut self,
        board: &Board,
        cards: &mut [Card],
        blocker_id: &CardId,
        now: &str,
    ) -> Result<(), BoardError> {
        let Some(blocker) = cards.iter().find(|card| card.id == *blocker_id) else {
            return Ok(());
        };
        let key = blocker.display_key(board);
        let reached = column_name(board, &blocker.status_id);
        let dependants: Vec<CardId> = blocks(cards, blocker_id)
            .into_iter()
            .map(|card| card.id.clone())
            .collect();
        for dependant in dependants {
            let Some(index) = cards.iter().position(|card| card.id == dependant) else {
                continue;
            };
            let card = &cards[index];
            if card.archived {
                continue;
            }
            let Some(target) = advance_target(board, &card.status_id) else {
                continue;
            };
            // A card that is working, or owed a run, stays where its run is: advancing it would
            // strand the run in a column the card has left.
            if card.pending_run.is_some()
                || self.live.contains(&card.id)
                || self.in_flight.contains(&card.id)
                || latest_run(card).is_some_and(CardRun::is_live)
            {
                continue;
            }
            if card
                .blocked_by
                .iter()
                .any(|blocker| !is_satisfied(board, cards, blocker))
            {
                continue;
            }
            let message = format!(
                "Moved to {}: unblocked by {key} reaching {reached}",
                column_name(board, &target)
            );
            self.try_advance(board, cards, index, &target, message, now)?;
        }
        Ok(())
    }
}

/// Rewrites the entry [`move_card`] just appended as the automation's own sentence.
///
/// `Moved` is reserved for a human's or the CLI's move: `ops::attention` reads a `Moved` newer
/// than a run's end as "a person has seen this", so an automatic move that left one behind
/// would clear the very attention it should be raising.
fn record_auto_move(card: &mut Card, message: String, now: &str) {
    if card
        .activity
        .last()
        .is_some_and(|entry| entry.kind == ActivityKind::Moved)
    {
        card.activity.pop();
    }
    push_activity(card, ActivityKind::AutoMoved, None, message, now);
}

/// `Run started · {provider} · {model} · {effort}`, dropping each part nobody has chosen.
///
/// The provider drops with its separator exactly as the model and the effort do: what the
/// column and the card left open is the daemon's configured default, which this engine does
/// not know and must not guess into a sentence a person reads as fact.
fn run_started(prefs: &ResolvedPrefs) -> String {
    let mut message = String::from("Run started");
    let parts = [
        prefs
            .provider
            .map(|provider| provider.executable().to_owned()),
        prefs.model.clone(),
        prefs.effort.clone(),
    ];
    for part in parts.into_iter().flatten() {
        message.push_str(" · ");
        message.push_str(&part);
    }
    message
}

/// The column with this id.
fn column<'a>(board: &'a Board, status_id: &StatusId) -> Option<&'a Status> {
    board.statuses.iter().find(|status| status.id == *status_id)
}

/// What a column runs on a card that enters it, if anything.
fn on_enter<'a>(board: &'a Board, status_id: &StatusId) -> Option<&'a Action> {
    column(board, status_id)
        .and_then(|status| status.automation.as_ref())
        .and_then(|automation| automation.on_enter.as_ref())
}

/// Where a routing column sends a card nothing blocks, if it is one.
fn advance_target(board: &Board, status_id: &StatusId) -> Option<StatusId> {
    column(board, status_id)
        .and_then(|status| status.automation.as_ref())
        .and_then(|automation| automation.advance_when_unblocked.clone())
}

/// A column's name, falling back to its id on a board that no longer carries it.
fn column_name(board: &Board, status_id: &StatusId) -> String {
    column(board, status_id).map_or_else(|| status_id.to_string(), |status| status.name.clone())
}

/// The card that should take a slot the moment one frees: the later column first, then the
/// oldest wait.
///
/// Later first because a card already deep in the pipeline is closer to being finished, and a
/// board that always started the newest arrival would leave its own review column starved.
#[must_use]
pub fn next_pending<'a>(board: &Board, cards: &'a [Card]) -> Option<&'a Card> {
    next_pending_except(board, cards, &BTreeSet::new())
}

/// [`next_pending`], passing over the cards in `reserved`: those an earlier freed slot already
/// went to, whose start has not cleared their marker yet. Handing a second freed slot to one of
/// them would start nothing and lose the slot.
#[must_use]
pub fn next_pending_except<'a>(
    board: &Board,
    cards: &'a [Card],
    reserved: &BTreeSet<CardId>,
) -> Option<&'a Card> {
    cards
        .iter()
        .filter(|card| !card.archived && !reserved.contains(&card.id))
        .filter_map(|card| {
            let pending = card.pending_run.as_ref()?;
            // A column whose action was taken away owes nothing, even to a card still marked
            // as waiting for it; the trigger site clears those markers, and this read must not
            // hand a freed slot to one it has not reached yet.
            on_enter(board, &pending.status_id)?;
            let rank = board
                .statuses
                .iter()
                .position(|status| status.id == pending.status_id)?;
            Some((rank, pending, card))
        })
        .max_by(|left, right| {
            let (left_rank, left_pending, left_card) = left;
            let (right_rank, right_pending, right_card) = right;
            left_rank
                .cmp(right_rank)
                .then_with(|| right_pending.since.cmp(&left_pending.since))
                .then_with(|| right_card.number.cmp(&left_card.number))
        })
        .map(|(_, _, card)| card)
}

/// Clears the marker of every queued card that can no longer take the slot it waits for, and
/// answers whether it cleared any.
///
/// A card queued in a routing column ([`queued`]) is moved on by rule 0 when a slot frees, and
/// rule 0 refuses a card something now blocks, or one whose column no longer routes to the
/// column it waits for. [`next_pending`] would still hand such a card every freed slot — the
/// oldest wait wins — and each would start nothing, so every newer card behind it would wait
/// forever. Clearing the marker takes it out of line; rule 2 moves it on, and it queues again,
/// once nothing blocks it. Nothing is announced: the card never moved, and the wait was never
/// an event either.
pub fn clear_stale_queues(board: &Board, cards: &mut [Card]) -> bool {
    let stale: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, card)| queued(card))
        .filter(|(_, card)| {
            let bound = card.pending_run.as_ref().map(|pending| &pending.status_id);
            card.archived
                || advance_target(board, &card.status_id).as_ref() != bound
                || card
                    .blocked_by
                    .iter()
                    .any(|blocker| !is_satisfied(board, cards, blocker))
        })
        .map(|(index, _)| index)
        .collect();
    for &index in &stale {
        cards[index].pending_run = None;
    }
    !stale.is_empty()
}

/// The agent one run asks for: the card first, then the column, then the daemon's own default.
///
/// A skill always runs on Claude — a card asking for Codex is ignored rather than refused,
/// because a card move must not fail over a preference — while the model and the effort still
/// come from the card, so a card's own reasoning budget survives a skill column.
#[must_use]
pub fn resolve_prefs(card: &Card, action: &Action) -> ResolvedPrefs {
    let card_prefs = card.agent.as_ref();
    let provider = match action.kind {
        ActionKind::Skill { .. } => Some(AgentKind::Claude),
        ActionKind::Prompt => card_prefs
            .and_then(|prefs| prefs.provider)
            .or(action.agent.provider),
    };
    ResolvedPrefs {
        provider,
        model: card_prefs
            .and_then(|prefs| prefs.model.clone())
            .or_else(|| action.agent.model.clone()),
        effort: card_prefs
            .and_then(|prefs| prefs.effort.clone())
            .or_else(|| action.agent.effort.clone()),
        mode: action.agent.mode.unwrap_or(PermissionMode::FullAccess),
    }
}

/// The child's brief without its footer: the column's instructions, the card, the pull request it
/// is about, the notes its person left since the last run, and the reports its previous runs left.
///
/// The instructions go through [`render_card_template`], so `{pr_url}`, `{pr_repo}` and
/// `{pr_number}` resolve on a card about a pull request and stay as written on any other.
///
/// A skill action opens the brief with its own invocation, because [`Action`] reaches the child
/// as a first message and nothing else carries the skill: that is the same reason
/// `validate_automation` refuses a skill action on Codex, which does not read one.
#[must_use]
pub fn brief(action: &Action, key: &str, card: &Card, reports: &[&Comment]) -> String {
    let mut brief = String::new();
    if let ActionKind::Skill { name, args } = &action.kind {
        brief.push('/');
        brief.push_str(name);
        if !args.trim().is_empty() {
            brief.push(' ');
            brief.push_str(args.trim());
        }
        brief.push_str("\n\n");
    }
    let instructions = render_card_template(&action.instructions, key, card);
    if !instructions.trim().is_empty() {
        brief.push_str(instructions.trim_end());
        brief.push_str("\n\n");
    }
    brief.push_str(&format!("# {key} — {}\n", card.title));
    if !card.description.trim().is_empty() {
        brief.push('\n');
        brief.push_str(card.description.trim_end());
        brief.push('\n');
    }
    if let Some(pull_request) = &card.pull_request {
        brief.push_str(&format!(
            "\n## Pull request\n\n{}#{} · {}\n",
            pull_request.repo, pull_request.number, pull_request.url
        ));
    }
    let notes = notes_since_last_run(card);
    if !notes.is_empty() {
        brief.push_str("\n## Notes from you\n\n");
        for note in notes {
            let author = note
                .author
                .as_deref()
                .map(str::trim)
                .filter(|author| !author.is_empty())
                .unwrap_or("you");
            brief.push_str(&format!("- {author}: {}\n", note.body.trim_end()));
        }
    }
    if !reports.is_empty() {
        brief.push_str("\n## Previous run reports\n");
        // Newest first, whatever order the caller kept them in: the newest report is the one
        // describing the tree this run is about to open, and a child that reads only the top of
        // a long brief must meet it first.
        let mut newest: Vec<&&Comment> = reports.iter().collect();
        newest.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        for report in newest {
            // Labelled with how its run ended, so "the newest succeeded report" names one report
            // even after a later run failed and left a report of its own.
            let outcome = report
                .run_id
                .and_then(|run| card.runs.iter().find(|row| row.id == run))
                .and_then(|row| row.outcome)
                .map_or_else(String::new, |outcome| format!(" · {}", outcome.word()));
            brief.push_str(&format!(
                "\n### Report from {}{outcome}\n\n{}\n",
                report.created_at,
                report.body.trim_end()
            ));
        }
    }
    brief
}

/// The comments a person wrote on this card since its newest *succeeded* run started, oldest
/// first.
///
/// A comment carrying a `run_id` is a run's own report, which reaches the brief as a report, not
/// as a note. A card that has never finished a run successfully hands over every note it has:
/// a run that failed may have read them, but it did nothing with them, and the retry must apply
/// them too — a note on a *Reviewed* card still reaches the *Review published* run that retries
/// a failed publish. The cut is the run's start, not its end, because its brief was written
/// when it started: a note made while it worked never reached it, and must reach the next run.
fn notes_since_last_run(card: &Card) -> Vec<&Comment> {
    let started = card
        .runs
        .iter()
        .rev()
        .find(|run| run.outcome == Some(RunOutcome::Succeeded))
        .map(|run| run.started_at.as_str());
    let mut notes: Vec<&Comment> = card
        .comments
        .iter()
        .filter(|comment| comment.run_id.is_none())
        .filter(|comment| started.is_none_or(|started| is_after(&comment.created_at, started)))
        .collect();
    notes.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    notes
}

/// Whether `later` is strictly after `earlier`, reading both as RFC 3339 when they parse and as
/// text otherwise: board stamps come from one daemon clock in one format, so the two agree.
fn is_after(later: &str, earlier: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(later),
        chrono::DateTime::parse_from_rfc3339(earlier),
    ) {
        (Ok(later), Ok(earlier)) => later > earlier,
        _ => later > earlier,
    }
}
