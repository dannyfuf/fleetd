//! Human-readable command output formatting.

use fleet_core::{
    agents::{Delegation, DelegationStatus, DelegationUsage},
    inspection::WorktreeInspection,
    model::{Repo, Worktree},
    sessions::{SessionState, WorktreeStatus},
};
use fleet_proto::response::{
    DoctorCheck, DoctorStatus, PruneResult, SleepResult, WorktreeDeleteResult,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Formats the compact list summary.
#[must_use]
pub fn list(repos: &[Repo], worktrees: &[Worktree]) -> String {
    let mut lines = vec![format!(
        "{} repos, {} worktrees",
        repos.len(),
        worktrees.len()
    )];
    if !worktrees.is_empty() {
        lines.push("WORKTREE\tHOST\tSESSION".to_owned());
        lines.extend(worktrees.iter().map(|worktree| {
            format!(
                "{}\t{}\t{}",
                worktree.id,
                worktree.host.as_ref().map_or("local", |host| host.as_str()),
                worktree.session
            )
        }));
    }
    lines.join("\n")
}

/// Formats one row per watch: ID, source, label, status, start time, and terminal ID.
#[must_use]
pub fn watches(watches: &[fleet_core::watches::Watch]) -> String {
    use fleet_core::watches::WatchStatus;
    watches
        .iter()
        .map(|watch| {
            let status = match watch.status {
                WatchStatus::Running => "running".to_owned(),
                WatchStatus::Exited {
                    code: Some(code), ..
                } => format!("exited {code}"),
                WatchStatus::Exited { .. } => "interrupted".to_owned(),
            };
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                watch.id,
                watch.source,
                crate::envelope::single_line(&watch.label),
                status,
                watch.started_at,
                watch.terminal
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats independent deletion results, one worktree per line.
#[must_use]
pub fn delete(results: &[WorktreeDeleteResult]) -> String {
    results
        .iter()
        .map(|result| {
            if result.ok {
                format!("Deleted {}", result.worktree_id)
            } else {
                format!(
                    "Failed {}: {}",
                    result.worktree_id,
                    result.reason.as_deref().unwrap_or("unknown error")
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats inspection facts as a readable per-worktree report.
#[must_use]
pub fn inspect(inspections: &[WorktreeInspection]) -> String {
    inspections
        .iter()
        .map(|inspection| {
            let state = if inspection.error.is_some() {
                "error"
            } else if inspection.dirty {
                "dirty"
            } else if inspection.merged {
                "merged"
            } else {
                "active"
            };
            let mut line = format!(
                "{} {} {} {}",
                inspection.worktree_id, inspection.host, inspection.branch, state
            );
            if let Some(error) = &inspection.error {
                line.push_str(": ");
                line.push_str(error);
            }
            if !inspection.warnings.is_empty() {
                line.push_str(" [");
                line.push_str(&inspection.warnings.join(", "));
                line.push(']');
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats safe-prune results.
#[must_use]
pub fn prune(result: &PruneResult) -> String {
    let deleted = result.deleted.iter().map(|id| {
        if result.dry_run {
            format!("Would delete {id}")
        } else {
            format!("Deleted {id}")
        }
    });
    let skipped = result
        .skipped
        .iter()
        .map(|entry| format!("Skipped {}: {}", entry.worktree_id, entry.reason));
    deleted.chain(skipped).collect::<Vec<_>>().join("\n")
}

/// Formats worktree status in swarm's `<id> <session>` line form.
#[must_use]
pub fn status(statuses: &[WorktreeStatus]) -> String {
    statuses
        .iter()
        .map(|status| {
            let session = match status.session {
                SessionState::None => "none",
                SessionState::Detached => "detached",
                SessionState::Attached => "attached",
                SessionState::Unknown => "unknown",
            };
            format!("{} {session}", status.worktree_id)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats a sleep-policy report.
#[must_use]
pub fn sleep(result: &SleepResult) -> String {
    let kept = result
        .kept
        .iter()
        .map(|entry| format!("Kept {}: {}", entry.window, entry.reason));
    let closed = result
        .closed
        .iter()
        .map(|window| format!("Closed {window}"));
    let mut lines = kept.chain(closed).collect::<Vec<_>>();
    if result.session_killed {
        lines.push("Session killed".to_owned());
    }
    if lines.is_empty() {
        lines.push("Nothing to sleep".to_owned());
    }
    lines.join("\n")
}

/// Formats diagnostics as a plain fixed-column report.
#[must_use]
pub fn doctor(checks: &[DoctorCheck]) -> String {
    let mut lines = vec!["CHECK STATUS DETAIL".to_owned()];
    lines.extend(checks.iter().map(|check| {
        format!(
            "{} {} {}",
            check.check,
            match check.status {
                DoctorStatus::Ok => "ok",
                DoctorStatus::Warn => "warn",
                DoctorStatus::Fail => "fail",
            },
            check.detail
        )
    }));
    lines.join("\n")
}

/// The display key of every card caller in a listing, resolved by the command that read them.
///
/// A delegation records the board and card that started it, not the key a person types: turning
/// one into `FLT-7` costs a `GetBoard`, which is a request a renderer may not make. The command
/// resolves what it can and hands it over; a caller missing from the map prints its raw card id,
/// which is still a selector every board verb accepts.
pub type CallerKeys = std::collections::BTreeMap<fleet_core::ids::CardId, String>;

/// Formats one fixed-field line per delegation.
///
/// Nine tab-separated fields: id, status, provider, child thread, duration, total tokens, cost,
/// delivery, caller. The two spend fields print `-` rather than `0` when the daemon has no usage
/// for the child — a delegation that has not reported a number yet and one that genuinely spent
/// nothing are different facts, and a zero would claim the second. The caller is `thread {id}`
/// or `card {KEY}`: a run started by a column is not a run any thread is waiting on, and a
/// listing that could not say which was which sent readers to the wrong transcript.
#[must_use]
pub fn subagents_with_keys(
    delegations: &[Delegation],
    now: SystemTime,
    keys: &CallerKeys,
) -> String {
    delegations
        .iter()
        .map(|delegation| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                delegation.id,
                delegation_status_word(delegation.status),
                delegation.provider.executable(),
                delegation.child,
                duration(elapsed_seconds(delegation, now)),
                total_tokens_field(delegation),
                cost_field(delegation),
                delegation.delivery.word(),
                caller_field(delegation, keys)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The list line's caller field: `thread {id}`, or `card {KEY}` for a column's own run.
fn caller_field(delegation: &Delegation, keys: &CallerKeys) -> String {
    match &delegation.caller {
        fleet_core::agents::DelegationCaller::Thread(thread) => format!("thread {thread}"),
        fleet_core::agents::DelegationCaller::Card { card, .. } => format!(
            "card {}",
            keys.get(card).map_or_else(
                || crate::envelope::single_line(card.as_str()),
                |key| crate::envelope::single_line(key)
            )
        ),
    }
}

/// Formats the whole of `fleet subagent status` for one delegation.
///
/// The order is `NATIVE-AGENTS.md` §15.4's: the fixed-field line `list` prints, the brief whole,
/// the child's spend when there is one, and — once the delegation is terminal —
/// [`delivered_message`] verbatim. That last block is the point of this renderer: it is the same
/// text `fleet subagent wait` prints for the same record, so an orchestrator that greps one can
/// grep the other, and a report whose `wait` was missed is still reachable without going and
/// reading files out of a temporary directory.
#[must_use]
pub fn subagent_status_with_keys(
    delegation: &Delegation,
    now: SystemTime,
    keys: &CallerKeys,
) -> String {
    let mut sections = vec![
        subagents_with_keys(std::slice::from_ref(delegation), now, keys),
        format!("brief:\n{}", crate::envelope::safe_block(&delegation.brief)),
    ];
    // A child that has reported nothing prints no line at all: a row of zeros would claim it
    // spent nothing, which is a different fact from not knowing yet.
    if let Some(usage) = &delegation.usage {
        sections.push(usage_line(usage));
    }
    if delegation.status.is_terminal() {
        sections.push(delivered_message(delegation, now));
    }
    sections.join("\n\n")
}

/// The child's spend on one line: totals, the breakdown, context, and a cost when one is known.
fn usage_line(usage: &DelegationUsage) -> String {
    let tokens = &usage.usage;
    let mut line = format!(
        "usage: {} tokens, {} in, {} out, {} cache read, {} cache write, context {:.0}%",
        tokens.total_tokens,
        tokens.input_tokens,
        tokens.output_tokens,
        tokens.cache_read_tokens,
        tokens.cache_write_tokens,
        usage.context_pct,
    );
    if let Some(cost) = usage.cost_usd {
        line.push_str(&format!(", ${cost:.2}"));
    }
    line
}

/// The list line's total-token field, or `-` when the daemon reported no usage.
fn total_tokens_field(delegation: &Delegation) -> String {
    delegation.usage.as_ref().map_or_else(
        || "-".to_owned(),
        |usage| usage.usage.total_tokens.to_string(),
    )
}

/// The list line's cost field, or `-` when there is no usage or the provider reported no cost.
fn cost_field(delegation: &Delegation) -> String {
    delegation
        .usage
        .as_ref()
        .and_then(|usage| usage.cost_usd)
        .map_or_else(|| "-".to_owned(), |cost| format!("${cost:.2}"))
}

/// Formats the message a caller receives when a delegation finishes.
#[must_use]
pub fn delivered_message(delegation: &Delegation, now: SystemTime) -> String {
    let result = delegation.result.as_ref();
    let text = result.map_or("", |result| result.text.as_str());
    let files_changed = result.map_or(0, |result| result.files_changed.len());
    let mut message = format!(
        "[fleet subagent {} finished: {}]\nprovider: {}, thread: {}, duration: {}, files changed: {}\n\n{}",
        delegation.id,
        delegation_status_word(delegation.status),
        delegation.provider.executable(),
        delegation.child,
        duration(elapsed_seconds(delegation, now)),
        files_changed,
        crate::envelope::safe_block(text)
    );
    if result.is_some_and(|result| result.elided) {
        message.push_str(&format!("\n\n(report elided at {} bytes)", text.len()));
    }
    message
}

/// Formats the message a caller receives when a wait timed out with the child still live.
///
/// One line, and deliberately sharing no phrase with [`delivered_message`]: a caller that
/// greps for `finished:` must not match this, and a caller reading it must not be told a
/// duration and a file count that only a finished child has. The duration is the delegation's
/// age, the same figure the delivered message reports, not the length of this one wait.
#[must_use]
pub fn still_running_message(delegation: &Delegation, now: SystemTime) -> String {
    format!(
        "[fleet subagent {} still running after {}, status: {}, thread: {}]",
        delegation.id,
        duration(elapsed_seconds(delegation, now)),
        delegation_status_word(delegation.status),
        delegation.child,
    )
}

/// The wire word for a status, which is deliberately not the transcript's.
///
/// `DelegationStatus::word` is written for the app's rows, where a finished child reads `done`
/// and a live one reads `working`. These two reports are a contract other programs parse — the
/// delivered message's first line says `finished: succeeded` — so they print the enum's own
/// names instead of the row vocabulary.
fn delegation_status_word(status: DelegationStatus) -> &'static str {
    match status {
        DelegationStatus::Starting => "starting",
        DelegationStatus::Running => "running",
        DelegationStatus::Blocked => "blocked",
        DelegationStatus::Settling => "settling",
        DelegationStatus::Succeeded => "succeeded",
        DelegationStatus::Incomplete => "incomplete",
        DelegationStatus::Failed => "failed",
        DelegationStatus::Cancelled => "cancelled",
    }
}

fn elapsed_seconds(delegation: &Delegation, now: SystemTime) -> u64 {
    let end = delegation.finished.map_or_else(
        || {
            now.duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs())
        },
        |finished| u64::try_from(finished.timestamp()).unwrap_or_default(),
    );
    let start = u64::try_from(delegation.created.timestamp()).unwrap_or_default();
    end.saturating_sub(start)
}

fn duration(seconds: u64) -> String {
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

/// Formats board summaries as a column-aligned table.
#[must_use]
pub fn boards(boards: &[fleet_core::board::BoardSummary]) -> String {
    let header = [
        "BOARD",
        "CONTEXT",
        "SCOPE",
        "NAME",
        "BACKEND",
        "CARDS",
        "OPEN",
        "DIRTY",
        "CONFLICTS",
        "WORKING",
        "NEEDS YOU",
    ]
    .map(str::to_owned)
    .to_vec();
    let mut rows = vec![header];
    rows.extend(boards.iter().map(|board| {
        let mut row = vec![
            board.id.to_string(),
            board.context_id.to_string(),
            // A context has two boards, and the scope is what tells them apart.
            match (&board.worktree_id, board.kind) {
                (Some(worktree), _) => worktree.to_string(),
                (None, fleet_core::board::BoardKind::Reviews) => "reviews".to_owned(),
                (None, _) => "context".to_owned(),
            },
            crate::envelope::single_line(&board.name),
            crate::envelope::single_line(&board.backend_kind),
            board.card_count.to_string(),
            board.open_count.to_string(),
            board.dirty_count.to_string(),
            board.conflict_count.to_string(),
            board.working_count.to_string(),
            board.attention_count.to_string(),
        ];
        // A failing sync is a per-board fact, not a column every clean board pays a dash for, so
        // it rides along as an extra trailing cell that only an unhealthy board grows.
        if let Some(error) = &board.last_error {
            row.push(format!("error: {}", crate::envelope::single_line(error)));
        }
        row
    }));
    columns(&rows).join("\n")
}

/// The gutter between two columns of a `human` table.
const GUTTER: &str = "  ";

/// Pads `rows` into columns two spaces apart, so a header and its values line up.
///
/// A width is the widest cell that column carries, counted in `char`s: the names these tables
/// print are already single-lined, and the alternative is a display-width dependency. Each row's
/// own last cell is left unpadded, so a table never ends in trailing blanks and a row that grows
/// an extra trailing cell still finds it at the column the padded row above put it.
pub(crate) fn columns(rows: &[Vec<String>]) -> Vec<String> {
    let mut widths: Vec<usize> = Vec::new();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            let width = cell.chars().count();
            match widths.get_mut(index) {
                Some(current) => *current = (*current).max(width),
                None => widths.push(width),
            }
        }
    }
    rows.iter()
        .map(|row| {
            let last = row.len().saturating_sub(1);
            row.iter()
                .enumerate()
                .map(|(index, cell)| {
                    if index == last {
                        return cell.clone();
                    }
                    let width = widths.get(index).copied().unwrap_or_default();
                    let padding = width.saturating_sub(cell.chars().count());
                    format!("{cell}{:padding$}", "")
                })
                .collect::<Vec<_>>()
                .join(GUTTER)
        })
        .collect()
}

/// Formats columns in board order and cards in their stored column order.
///
/// `backend_label` is the descriptor label from `ListBoardBackends`; without it the header
/// names the raw registry kind, which is what a daemon that cannot list backends leaves us.
/// `now` is Unix-epoch seconds, so the `synced 3m ago` stamp is a pure function of its inputs.
#[must_use]
pub fn board(
    view: &fleet_core::board::BoardView,
    backend: Option<&fleet_core::board::BackendDescriptor>,
    now: i64,
) -> String {
    // A Reviews board belongs to its context the way a worktree board belongs to its worktree,
    // and a reader who sees two boards with the same name needs to know which one this is.
    let scope = match (&view.board.worktree_id, view.board.kind) {
        (Some(id), _) => format!(" · worktree {id}"),
        (None, fleet_core::board::BoardKind::Reviews) => format!(
            " · reviews of {}",
            crate::envelope::single_line(view.board.context_id.as_str())
        ),
        (None, fleet_core::board::BoardKind::Tasks) => String::new(),
    };
    let mut sections = vec![format!(
        "{} ({}){}\n{}",
        crate::envelope::single_line(&view.board.name),
        crate::envelope::single_line(&view.board.prefix),
        scope,
        board_header(view, backend, now)
    )];
    let stamp = rfc3339_utc(now);
    for status in &view.board.statuses {
        let cards = fleet_core::board::column_cards(&view.cards, &status.id);
        let mut lines = vec![format!(
            "{} ({}){}",
            crate::envelope::single_line(&status.name),
            cards.len(),
            // The bolt says this column runs something, which is the one column fact a reader
            // needs before moving a card into it: the move starts work.
            if column_runs_an_action(status) {
                " \u{26a1}"
            } else {
                ""
            }
        )];
        for card in cards {
            let mut row = display_key(&view.board, card);
            // The run mark sits before the priority word because it is the more urgent fact:
            // a row a reader scans says "this one is moving" before it says how much it matters.
            if let Some(mark) = run_mark(view, card, &stamp) {
                row.push_str(&format!("  {mark}"));
            }
            row.push_str(&format!(
                "  {}  {}",
                card.priority.label().to_lowercase(),
                crate::envelope::single_line(&card.title)
            ));
            if let Some(blocked) = fleet_core::board::blocked(&view.board, &view.cards, card) {
                row.push_str(&format!("  \u{2298} {}", blocked.unsatisfied));
            }
            // `card_labels` answers an em dash for a card with none — it is written for the
            // report's one-per-line fields — so the emptiness test is the card's, not the
            // string's. Testing the string printed `[—]` beside every unlabeled card.
            if !card.labels.is_empty() {
                row.push_str(&format!("  [{}]", card_labels(&view.board, card)));
            }
            if let Some(assignee) = &card.assignee {
                row.push_str(&format!("  @{}", crate::envelope::single_line(assignee)));
            }
            lines.push(row);
        }
        sections.push(lines.join("\n"));
    }
    // A card whose status is no longer a column belongs to no section above, and `board list`
    // still counts it: printing the columns alone makes it invisible on the one surface that
    // was asked to show the board. The app draws exactly these as error rows.
    let orphans: Vec<&fleet_core::board::Card> = view
        .cards
        .iter()
        .filter(|card| !card.archived)
        .filter(|card| {
            !view
                .board
                .statuses
                .iter()
                .any(|status| status.id == card.status_id)
        })
        .collect();
    if !orphans.is_empty() {
        let mut lines = vec![format!("No column ({})", orphans.len())];
        lines.extend(orphans.iter().map(|card| {
            format!(
                "{}  {}  {}  [status {}]",
                display_key(&view.board, card),
                card.priority.label().to_lowercase(),
                crate::envelope::single_line(&card.title),
                crate::envelope::single_line(card.status_id.as_str())
            )
        }));
        sections.push(lines.join("\n"));
    }
    sections.join("\n\n")
}

/// Whether a column starts something when a card enters it.
fn column_runs_an_action(status: &fleet_core::board::Status) -> bool {
    status
        .automation
        .as_ref()
        .is_some_and(|automation| automation.on_enter.is_some())
}

/// Whether a card has work in flight: a joined live delegation, or a run it has not ended.
///
/// Both are asked because they answer different questions. The join is the daemon's live view
/// and is absent from a board read by a client that never asked for one; the card's own last
/// run is what the document remembers, and a run whose daemon has gone is still unfinished.
fn is_working(view: &fleet_core::board::BoardView, card: &fleet_core::board::Card) -> bool {
    view.live_runs.iter().any(|run| run.card_id == card.id)
        || fleet_core::board::latest_run(card).is_some_and(fleet_core::board::CardRun::is_live)
}

/// The mark a board row carries before its priority word, when it carries one.
///
/// One mark, in this order: a card that is running says so, a card waiting on a person says so
/// next, and a card owed a run it could not start says so last. A row with two marks would be a
/// row nobody can scan, and these three are ordered by what the reader has to do about them.
fn run_mark(
    view: &fleet_core::board::BoardView,
    card: &fleet_core::board::Card,
    now: &str,
) -> Option<&'static str> {
    if is_working(view, card) {
        return Some("\u{25cf} working");
    }
    if fleet_core::board::attention(card, now) {
        return Some("! needs you");
    }
    card.pending_run.as_ref().map(|_| "\u{2026} pending")
}

/// How many of the backend's own settings the board header names.
const HEADER_SETTINGS: usize = 2;

/// The backend line under the board title: `backend: Jira (acli) · project SP · synced 3m ago
/// · 2 dirty · 1 conflict`.
///
/// A local board has no remote to be behind, so it prints its backend and nothing else: a
/// `never synced · 0 dirty · 0 conflict` tail would report a problem that cannot exist.
fn board_header(
    view: &fleet_core::board::BoardView,
    descriptor: Option<&fleet_core::board::BackendDescriptor>,
    now: i64,
) -> String {
    let backend = &view.board.backend;
    // One summary for the whole line, over the view's own run join and a real stamp: `attention`
    // and the working count compare RFC 3339 timestamps, and the epoch seconds this renderer is
    // handed are the only clock it has. Passing `""` — which is what the CLI did before this
    // line counted runs — made every card's attention test fail to parse and answer no.
    let summary =
        fleet_core::board::summarize(&view.board, &view.cards, &view.live_runs, &rfc3339_utc(now));
    let mut parts = vec![format!(
        "backend: {}",
        crate::envelope::single_line(descriptor.map_or(backend.kind.as_str(), |descriptor| {
            descriptor.label.as_str()
        }))
    )];
    // Which settings identify a board is the backend's own answer, read from the schema it
    // publishes, in the order it declares them. Naming keys here put one backend's vocabulary
    // in a printer every backend shares, so a second backend's identity settings — whatever it
    // calls them — never reached this line (BOARD §10).
    for schema in descriptor
        .map(|descriptor| descriptor.settings_schema.as_slice())
        .unwrap_or_default()
        .iter()
        .take(HEADER_SETTINGS)
    {
        if let Some(value) = backend
            .settings
            .get(&schema.key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parts.push(format!(
                "{} {}",
                crate::envelope::single_line(&schema.key),
                crate::envelope::single_line(value)
            ));
        }
    }
    // What the board is doing right now comes before what its remote is doing: a run in flight
    // is this minute's fact, and a sync stamp is not. Both counts are dropped while they are
    // zero, so a board nobody has automated prints exactly the header it always printed.
    // The live runs and the owed ones are stated apart, as the app's header states them: a
    // queue behind the limit is the throttle working, and one count over the limit would read
    // as the limit broken (`4/2 working`).
    let open = || view.cards.iter().filter(|card| !card.archived);
    let live = open().filter(|card| is_working(view, card)).count();
    let owed = open()
        .filter(|card| card.pending_run.is_some() && !is_working(view, card))
        .count();
    if owed > 0 {
        parts.push(format!("{live} working \u{b7} {owed} waiting"));
    } else if live > 0 {
        parts.push(format!(
            "{live}/{} working",
            view.board.settings.max_live_runs()
        ));
    }
    if summary.attention_count > 0 {
        parts.push(format!("{} needs you", summary.attention_count));
    }
    if !backend.is_local() {
        parts.push(synced_age(view.board.sync.last_synced_at.as_deref(), now));
        parts.push(format!("{} dirty", summary.dirty_count));
        parts.push(format!("{} conflict", summary.conflict_count));
        // `board list` says this and `board show` did not: a sync that has been failing for
        // days printed a stamp from the last one that worked and nothing else.
        if let Some(error) = &view.board.sync.last_error {
            parts.push(format!("error: {}", crate::envelope::single_line(error)));
        }
    }
    parts.join(" \u{b7} ")
}

/// `synced 3m ago`, `never synced`, or the raw stamp when it is not a timestamp we can read.
#[must_use]
pub fn synced_age(last_synced_at: Option<&str>, now: i64) -> String {
    let Some(stamp) = last_synced_at else {
        return "never synced".to_owned();
    };
    epoch_secs(stamp).map_or_else(
        || format!("synced {}", crate::envelope::single_line(stamp)),
        |then| format!("synced {} ago", format_age((now - then).max(0))),
    )
}

/// A coarse relative age in the app's unit letters (`45s`, `3m`, `2h`, `5d`, `3w`, `1y`).
fn format_age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        86_400..=1_209_599 => format!("{}d", seconds / 86_400),
        1_209_600..=31_535_999 => format!("{}w", seconds / 604_800),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// Unix-epoch seconds for an RFC3339 stamp, honoring its offset; `None` when it is not one.
fn epoch_secs(iso: &str) -> Option<i64> {
    let bytes = iso.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return None;
    }
    let number = |from: usize, to: usize| iso.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    let tail = iso.get(19..).unwrap_or_default();
    let tail = tail.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(offset) = tail.strip_prefix('+').or_else(|| tail.strip_prefix('-')) {
        let sign = if tail.starts_with('-') { -1 } else { 1 };
        let hours = offset.get(0..2)?.parse::<i64>().ok()?;
        let minutes = offset
            .get(3..5)
            .or_else(|| offset.get(2..4))
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        seconds -= sign * (hours * 3_600 + minutes * 60);
    }
    Some(seconds)
}

/// The RFC 3339 UTC stamp for `epoch` seconds: the format every board clock writes.
///
/// `attention`, `summarize` and the pending-run age compare stamps, not integers, and the board
/// renderers are handed the same epoch clock `synced 3m ago` uses. Converting here keeps them
/// pure functions of their arguments rather than giving the run counts a second, hidden clock
/// that a test could not move. Public so a command holding the same epoch clock can stamp its
/// own `summarize` call rather than passing the `""` that counts nothing.
#[must_use]
pub fn rfc3339_utc(epoch: i64) -> String {
    let (year, month, day) = civil_from_days(epoch.div_euclid(86_400));
    let seconds = epoch.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

/// The civil date `days` after 1970-01-01, by Howard Hinnant's algorithm — [`days_from_civil`]
/// read backwards.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// Days from 1970-01-01 to `y-m-d`, by Howard Hinnant's civil-calendar algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Formats the registered backend kinds as a compact table.
///
/// The settings column lists the keys `fleet board set --setting` accepts for that kind; the
/// capability column lists only what the backend can do, so an empty cell is an em dash.
#[must_use]
pub fn board_backends(backends: &[fleet_core::board::BackendDescriptor]) -> String {
    let mut lines = vec!["KIND  LABEL  CAPABILITIES  SETTINGS".to_owned()];
    lines.extend(backends.iter().map(|backend| {
        let capabilities = capability_names(&backend.capabilities);
        // The key alone is what `--setting k=v` takes, but it is also all the CLI ever said:
        // the one signal a backend has for "this one is mandatory" lives in the schema's name,
        // and a user learned about it from a daemon refusal instead.
        let settings = backend
            .settings_schema
            .iter()
            .map(|schema| {
                format!(
                    "{} ({})",
                    crate::envelope::single_line(&schema.key),
                    crate::envelope::single_line(&schema.name)
                )
            })
            .collect::<Vec<_>>();
        format!(
            "{}  {}  {}  {}",
            crate::envelope::single_line(&backend.kind),
            crate::envelope::single_line(&backend.label),
            list_or_dash(&capabilities),
            list_or_dash(&settings),
        )
    }));
    lines.join("\n")
}

/// The names of the capabilities a backend declares, in the struct's declaration order.
fn capability_names(capabilities: &fleet_core::board::BackendCapabilities) -> Vec<String> {
    [
        ("pull", capabilities.pull),
        ("push-updates", capabilities.push_updates),
        ("push-create", capabilities.push_create),
        ("transitions", capabilities.transitions),
        ("comments", capabilities.comments),
        ("custom-properties", capabilities.custom_properties),
        ("incremental", capabilities.incremental),
    ]
    .into_iter()
    .filter(|&(_, supported)| supported)
    .map(|(name, _)| name.to_owned())
    .collect()
}

/// Formats what the backend reports about one board: its statuses, labels, properties, the
/// fields it refuses to write back, and the people it knows.
#[must_use]
pub fn board_backend_schema(schema: &fleet_core::board::BackendSchema) -> String {
    let mut lines = vec![format!(
        "Key prefix: {}",
        schema
            .key_prefix
            .as_deref()
            .map_or_else(|| "\u{2014}".to_owned(), crate::envelope::single_line)
    )];
    lines.push(format!(
        "Read-only fields: {}",
        list_or_dash(
            &schema
                .readonly_fields
                .iter()
                .map(|field| crate::envelope::single_line(field))
                .collect::<Vec<_>>()
        )
    ));
    lines.push("\nStatuses".to_owned());
    for status in &schema.statuses {
        lines.push(format!(
            "{}  {}  {}",
            crate::envelope::single_line(&status.id),
            crate::envelope::single_line(&status.name),
            status.category.map_or("\u{2014}", category_name),
        ));
    }
    lines.push("\nProperties".to_owned());
    for property in &schema.properties {
        lines.push(format!(
            "{}  {}  {}  {}",
            crate::envelope::single_line(&property.key),
            crate::envelope::single_line(&property.name),
            property_kind_name(property.kind),
            if property.editable {
                "editable"
            } else {
                "read-only"
            },
        ));
    }
    let labels = schema
        .labels
        .iter()
        .map(|label| crate::envelope::single_line(label))
        .collect::<Vec<_>>();
    let assignees = schema
        .assignees
        .iter()
        .map(|assignee| crate::envelope::single_line(assignee))
        .collect::<Vec<_>>();
    lines.push(format!("\nLabels: {}", list_or_dash(&labels)));
    lines.push(format!("Assignees: {}", list_or_dash(&assignees)));
    lines.join("\n")
}

/// A comma-separated list, or the em dash every other empty field in these reports prints.
fn list_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        return "\u{2014}".to_owned();
    }
    values.join(", ")
}

/// The wire name of a status category.
const fn category_name(category: fleet_core::board::StatusCategory) -> &'static str {
    use fleet_core::board::StatusCategory;
    match category {
        StatusCategory::Backlog => "backlog",
        StatusCategory::Unstarted => "unstarted",
        StatusCategory::Started => "started",
        StatusCategory::Completed => "completed",
        StatusCategory::Canceled => "canceled",
    }
}

/// The wire name of a property kind.
const fn property_kind_name(kind: fleet_core::board::PropertyKind) -> &'static str {
    use fleet_core::board::PropertyKind;
    match kind {
        PropertyKind::Text => "text",
        PropertyKind::Number => "number",
        PropertyKind::Bool => "bool",
        PropertyKind::Date => "date",
        PropertyKind::Select => "select",
        PropertyKind::MultiSelect => "multi_select",
        PropertyKind::User => "user",
        PropertyKind::Url => "url",
    }
}

/// The key a reader can type back, sanitised: on a mirrored card it is the remote's own.
///
/// `Card::display_key` answers `remote.key` for a linked card, so the identifier at the head of
/// every board row and card report is a string the backend chose, not one Fleet validated.
fn display_key(board: &fleet_core::board::Board, card: &fleet_core::board::Card) -> String {
    crate::envelope::single_line(&card.display_key(board))
}

/// The card's label names, or the em dash every other empty field in this report prints.
fn card_labels(board: &fleet_core::board::Board, card: &fleet_core::board::Card) -> String {
    if card.labels.is_empty() {
        return "\u{2014}".to_owned();
    }
    card.labels
        .iter()
        .map(|id| {
            crate::envelope::single_line(
                board
                    .labels
                    .iter()
                    .find(|label| &label.id == id)
                    .map_or(id.as_str(), |label| label.name.as_str()),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// One tab-separated line per run on a card: `id  column  state  provider  model  effort
/// duration  tokens  cost  thread`, with an em dash for anything unknown.
///
/// The same line `card runs` prints and `card show` lists, in the shape of [`subagents`]: these
/// are the two reports an orchestrator parses, and a run is the same row wherever it is read.
/// `live` is the view's delegation join, which is what turns a run the card has not ended into
/// the word the daemon would use for it. `now` is epoch seconds, and `None` says the surface
/// has no clock — a live run then prints no duration rather than counting from 1970.
#[must_use]
pub fn card_runs(
    board: &fleet_core::board::Board,
    card: &fleet_core::board::Card,
    live: &[fleet_core::board::LiveRun],
    now: Option<i64>,
) -> String {
    card.runs
        .iter()
        .map(|run| {
            let column = board
                .statuses
                .iter()
                .find(|status| status.id == run.status_id)
                .map_or(run.status_id.as_str(), |status| status.name.as_str());
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                run.id,
                crate::envelope::single_line(column),
                run_state(run, live),
                run.provider.executable(),
                or_dash(run.model.as_deref()),
                or_dash(run.effort.as_deref()),
                run_duration(run, now),
                run.tokens
                    .map_or_else(|| "\u{2014}".to_owned(), |tokens| tokens.to_string()),
                run.cost_usd
                    .map_or_else(|| "\u{2014}".to_owned(), |cost| format!("${cost:.2}")),
                run.thread_id
                    .as_ref()
                    .map_or_else(|| "\u{2014}".to_owned(), ToString::to_string),
                run_refusal(run).unwrap_or_else(|| "\u{2014}".to_owned()),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The sentence a run that never started was refused with, as one line.
///
/// The daemon records the refusal under its error kind (`validation failed: invalid automation:
/// …`, `conflict: …`); the sentence after those prefixes is the one every surface prints
/// verbatim (`docs/BOARD.md` §11.2).
#[must_use]
pub fn run_refusal(run: &fleet_core::board::CardRun) -> Option<String> {
    if !run.failed_to_start() || run.outcome != Some(fleet_core::board::RunOutcome::Failed) {
        return None;
    }
    let mut sentence = run.detail.as_deref()?;
    for prefix in ["validation failed: ", "invalid automation: ", "conflict: "] {
        sentence = sentence.strip_prefix(prefix).unwrap_or(sentence);
    }
    Some(crate::envelope::single_line(sentence))
}

/// A run's state word: its outcome once it has one, else what the daemon says it is doing.
///
/// A run the card has not ended and the join does not carry is still `running`: the card is the
/// durable record, and a daemon that has forgotten a delegation has not finished it.
fn run_state(
    run: &fleet_core::board::CardRun,
    live: &[fleet_core::board::LiveRun],
) -> &'static str {
    run.outcome.map_or_else(
        || {
            live.iter()
                .find(|joined| joined.run == run.id)
                .map_or("running", |joined| delegation_status_word(joined.status))
        },
        fleet_core::board::RunOutcome::word,
    )
}

/// How long a run took, or has been going; an em dash when neither can be told.
fn run_duration(run: &fleet_core::board::CardRun, now: Option<i64>) -> String {
    let Some(started) = epoch_secs(&run.started_at) else {
        return "\u{2014}".to_owned();
    };
    let end = match run.ended_at.as_deref() {
        Some(ended) => epoch_secs(ended),
        None => now,
    };
    end.filter(|end| *end >= started)
        .map_or_else(|| "\u{2014}".to_owned(), |end| duration_secs(end - started))
}

/// A duration in the `14m 02s` shape, from a signed count of seconds.
fn duration_secs(seconds: i64) -> String {
    duration(u64::try_from(seconds).unwrap_or_default())
}

/// A value, or the em dash every empty field in these reports prints.
fn or_dash(value: Option<&str>) -> String {
    value.map_or_else(|| "\u{2014}".to_owned(), crate::envelope::single_line)
}

/// `Agent: codex, model gpt-5 (column default), effort high`, or nothing to say.
///
/// A field the card did not set is marked as the column's, because that is the difference
/// between an agent this card asks for and one the workflow hands it: moving the card to
/// another column changes the second and not the first.
fn card_agent_line(
    board: &fleet_core::board::Board,
    card: &fleet_core::board::Card,
) -> Option<String> {
    use fleet_core::board::ActionKind;

    let action = board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .and_then(|status| status.automation.as_ref())
        .and_then(|automation| automation.on_enter.as_ref());
    let prefs = card.agent.as_ref();
    let (provider, model, effort) = match action {
        Some(action) => {
            let resolved = fleet_core::board::resolve_prefs(card, action);
            (resolved.provider, resolved.model, resolved.effort)
        }
        None => (
            prefs.and_then(|prefs| prefs.provider),
            prefs.and_then(|prefs| prefs.model.clone()),
            prefs.and_then(|prefs| prefs.effort.clone()),
        ),
    };
    // A skill column runs on Claude whatever the card asked for, so the provider it prints is
    // the column's even on a card that named one.
    let provider_is_the_cards = prefs.and_then(|prefs| prefs.provider).is_some()
        && !matches!(
            action.map(|action| &action.kind),
            Some(ActionKind::Skill { .. })
        );
    let mut parts = Vec::new();
    if let Some(provider) = provider {
        parts.push(format!(
            "{}{}",
            provider.executable(),
            inherited(provider_is_the_cards)
        ));
    }
    if let Some(model) = model {
        parts.push(format!(
            "model {}{}",
            crate::envelope::single_line(&model),
            inherited(prefs.is_some_and(|prefs| prefs.model.is_some()))
        ));
    }
    if let Some(effort) = effort {
        parts.push(format!(
            "effort {}{}",
            crate::envelope::single_line(&effort),
            inherited(prefs.is_some_and(|prefs| prefs.effort.is_some()))
        ));
    }
    (!parts.is_empty()).then(|| format!("Agent: {}", parts.join(", ")))
}

/// The tail that says a field came from the column rather than the card.
const fn inherited(from_the_card: bool) -> &'static str {
    if from_the_card {
        ""
    } else {
        " (column default)"
    }
}

/// The display keys of some cards, in board order, or the em dash when there are none.
fn card_keys<'a>(
    board: &fleet_core::board::Board,
    cards: impl Iterator<Item = &'a fleet_core::board::Card>,
) -> String {
    let keys: Vec<String> = cards.map(|card| display_key(board, card)).collect();
    list_or_dash(&keys)
}

/// Formats a card's identity, properties, Markdown description, and comments.
///
/// `cards` is the board's card set, used to name the parent by the key the user typed rather
/// than by the one identifier no command ever prints.
#[must_use]
pub fn board_card(
    board: &fleet_core::board::Board,
    cards: &[fleet_core::board::Card],
    card: &fleet_core::board::Card,
) -> String {
    let status = board
        .statuses
        .iter()
        .find(|status| status.id == card.status_id)
        .map_or(card.status_id.as_str(), |status| status.name.as_str());
    let status = crate::envelope::single_line(status);
    let mut lines = vec![
        // The header is one row of a line-oriented report: a multi-line title cannot break it.
        format!(
            "{}  {}",
            display_key(board, card),
            crate::envelope::single_line(&card.title)
        ),
        // `CardId` admits any non-whitespace bytes, and a mirrored card takes its id from the
        // remote: the one identifier this report promises is exact still cannot be trusted raw.
        format!("ID: {}", crate::envelope::single_line(card.id.as_str())),
        format!("Status: {status}"),
        format!("Priority: {}", card.priority.label()),
        format!("Labels: {}", card_labels(board, card)),
        format!(
            "Assignee: {}",
            card.assignee
                .as_deref()
                .map_or_else(|| "\u{2014}".to_owned(), crate::envelope::single_line)
        ),
        format!(
            "Estimate: {}",
            card.estimate
                .map_or_else(|| "—".to_owned(), |value| value.to_string())
        ),
        format!(
            "Due: {}",
            card.due_date
                .as_deref()
                .map_or_else(|| "\u{2014}".to_owned(), crate::envelope::single_line)
        ),
        format!(
            "Parent: {}",
            card.parent_id.as_ref().map_or_else(
                || "\u{2014}".to_owned(),
                |id| cards.iter().find(|parent| parent.id == *id).map_or_else(
                    || crate::envelope::single_line(id.as_str()),
                    |parent| display_key(board, parent)
                )
            )
        ),
        format!(
            "Repo: {}",
            card.repo_id.as_ref().map_or("—", |id| id.as_str())
        ),
        format!(
            "Worktree: {}",
            card.worktree_id.as_ref().map_or("—", |id| id.as_str())
        ),
    ];
    // Only a review card has a pull request, so only a review card is told one: a task card
    // prints exactly the report it printed before Reviews boards existed.
    if let Some(pull_request) = &card.pull_request {
        lines.push(format!(
            "Pull request: {}  {}",
            crate::envelope::single_line(&pull_request.key()),
            crate::envelope::single_line(&pull_request.url)
        ));
    }
    lines.extend([
        format!("Archived: {}", card.archived),
        format!("Dirty: {}", card.dirty),
        format!(
            "Created: {}",
            crate::envelope::single_line(&card.created_at)
        ),
        format!(
            "Updated: {}",
            crate::envelope::single_line(&card.updated_at)
        ),
    ]);
    // The workflow rows appear only when the card has something to say through them: a board
    // nobody has automated prints exactly the report it printed before automation existed, and
    // three em dashes on every card is not a feature anybody asked for.
    if let Some(agent) = card_agent_line(board, card) {
        lines.push(agent);
    }
    if !card.blocked_by.is_empty() {
        // A blocker the view no longer carries still says something the reader can look up,
        // exactly as the parent row does for a card outside the set it was handed.
        let keys: Vec<String> = card
            .blocked_by
            .iter()
            .map(|blocker| {
                cards.iter().find(|other| other.id == *blocker).map_or_else(
                    || crate::envelope::single_line(blocker.as_str()),
                    |other| display_key(board, other),
                )
            })
            .collect();
        let mut line = format!("Blocked by: {}", list_or_dash(&keys));
        if let Some(blocked) = fleet_core::board::blocked(board, cards, card) {
            line.push_str(&format!("  \u{2298} {} waiting", blocked.unsatisfied));
            // A blocker nobody can finish will never release this card on its own, and the row
            // that only counts says nothing about the one thing the reader has to go and fix.
            if blocked.tone == fleet_core::board::BlockedTone::Warning {
                line.push_str(" (a blocker was canceled or archived)");
            }
        }
        lines.push(line);
    }
    let dependants = fleet_core::board::blocks(cards, &card.id);
    if !dependants.is_empty() {
        lines.push(format!(
            "Blocks: {}",
            card_keys(board, dependants.into_iter())
        ));
    }
    // Only an unlinked card answers to its local key, so only an unlinked card is told one:
    // `resolve_card` refuses `FLT-7` on a mirrored card, and printing it here handed the reader
    // a selector the very next command rejected.
    if card.remote.is_none() {
        lines.push(format!(
            "Local key: {}",
            crate::envelope::single_line(&card.local_key(board))
        ));
    }
    if let Some(remote) = &card.remote {
        lines.push(format!(
            "Remote: {} ({})",
            crate::envelope::single_line(&remote.key),
            crate::envelope::single_line(&remote.backend)
        ));
        if let Some(url) = &remote.url {
            lines.push(format!("URL: {}", crate::envelope::single_line(url)));
        }
        lines.push(format!(
            "Synced: {}",
            crate::envelope::single_line(&remote.synced_at)
        ));
    }
    if let Some(conflict) = &card.conflict {
        // The same table the app's conflict banner reads: `status_id, due_date` is a sentence
        // the reader has to translate, on this surface as much as on that one.
        // A conflict with no named field still has to say what happened; the app's banner
        // covers exactly that case with the same sentence.
        let fields = conflict
            .fields
            .iter()
            .map(|field| crate::envelope::single_line(fleet_core::board::field_label(field)))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(if fields.is_empty() {
            "Conflict: the remote changed".to_owned()
        } else {
            format!("Conflict: {fields}")
        });
    }
    // A backend is free to call a property whatever it likes, and Jira calls one of them
    // `Created`. Printed under that name it became a second `Created:` row holding a different
    // date than the card's own, with nothing to tell the two apart: a property whose name
    // collides with a row already written answers to its key instead.
    let taken: Vec<String> = lines
        .iter()
        .filter_map(|line| line.split(':').next())
        .map(str::to_owned)
        .collect();
    for (key, value) in &card.properties {
        let name = board
            .properties
            .iter()
            .find(|schema| &schema.key == key)
            .map(|schema| schema.name.as_str())
            .filter(|name| !taken.iter().any(|used| used == name))
            .unwrap_or(key.as_str());
        lines.push(format!(
            "{}: {}",
            crate::envelope::single_line(name),
            crate::envelope::single_line(&value.display())
        ));
    }
    lines.push(format!(
        "\nDescription\n{}",
        crate::envelope::safe_block(&card.description)
    ));
    // The run history, in the line `card runs` prints. This report has no delegation join and no
    // clock, so a run still going prints its state and no duration: `card runs` is the verb with
    // both, and inventing a duration from a clock this renderer does not have is worse than a
    // dash.
    if !card.runs.is_empty() {
        lines.push(format!("\nRuns\n{}", card_runs(board, card, &[], None)));
    }
    lines.push("\nComments".to_owned());
    for comment in &card.comments {
        // Locally authored comments carry no author; "@unknown" would name a person.
        let author = comment
            .author
            .as_deref()
            .map(|author| format!("  @{}", crate::envelope::single_line(author)))
            .unwrap_or_default();
        lines.push(format!(
            "{}{author}\n{}",
            crate::envelope::single_line(&comment.created_at),
            crate::envelope::safe_block(&comment.body)
        ));
    }
    lines.join("\n")
}

/// Formats the completed job's progress and refreshed board counts.
#[must_use]
pub fn board_sync(
    job: &fleet_proto::job::JobRecord,
    summary: &fleet_core::board::BoardSummary,
) -> String {
    let mut lines = vec![format!("Synced {} ({})", summary.id, job.id)];
    if let Some(progress) = &job.progress {
        lines.push(crate::envelope::single_line(progress));
    }
    lines.push(format!(
        "{} cards, {} open, {} dirty, {} conflicts",
        summary.card_count, summary.open_count, summary.dirty_count, summary.conflict_count
    ));
    if let Some(error) = &summary.last_error {
        lines.push(format!(
            "Last error: {}",
            crate::envelope::single_line(error)
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::board::BoardView;
    use serde_json::json;

    /// 2026-09-06T12:00:00Z, the stamp every fixture below is written against.
    const NOW: i64 = 1_788_696_000;

    /// A board whose Ready column runs the card, holding one card in each state the marks name.
    fn view() -> BoardView {
        serde_json::from_value(json!({
            "board": {
                "id": "work", "contextId": "work", "name": "Fleet", "prefix": "FLT",
                "backend": {"kind": "local", "settings": {}},
                "statuses": [
                    {"id": "todo", "name": "Todo", "category": "unstarted"},
                    {"id": "ready", "name": "Ready", "category": "unstarted", "automation": {
                        "onEnter": {
                            "kind": {"kind": "prompt"},
                            "agent": {"provider": "codex", "model": "gpt-5", "effort": "high"}
                        }
                    }},
                    {"id": "done", "name": "Done", "category": "completed"}
                ],
                "labels": [], "properties": [], "settings": {"maxLiveRuns": 2},
                "nextNumber": 5, "sync": {}, "createdAt": "now", "updatedAt": "now"
            },
            "cards": [
                {
                    "id": "card-1", "boardId": "work", "number": 1, "title": "Design",
                    "statusId": "done", "priority": "medium", "position": 0,
                    "createdAt": "now", "updatedAt": "now"
                },
                {
                    "id": "card-2", "boardId": "work", "number": 2, "title": "Build",
                    "statusId": "ready", "priority": "high", "position": 1,
                    "blockedBy": ["card-1"],
                    "agent": {"effort": "low"},
                    "runs": [{
                        "id": "00000000-0000-4000-8000-000000000001",
                        "threadId": "00000000-0000-4000-8000-0000000000a1",
                        "statusId": "ready", "action": {"kind": "prompt"}, "provider": "codex",
                        "model": "gpt-5", "startedAt": "2026-09-06T11:58:00Z"
                    }],
                    "createdAt": "now", "updatedAt": "now"
                },
                {
                    "id": "card-3", "boardId": "work", "number": 3, "title": "Test",
                    "statusId": "todo", "priority": "low", "position": 2,
                    "blockedBy": ["card-2"],
                    "runs": [{
                        "id": "00000000-0000-4000-8000-000000000002",
                        "threadId": "00000000-0000-4000-8000-0000000000a2",
                        "statusId": "ready", "action": {"kind": "prompt"}, "provider": "claude",
                        "startedAt": "2026-09-06T11:50:00Z", "endedAt": "2026-09-06T11:52:30Z",
                        "outcome": "needs_you", "tokens": 1200, "costUsd": 0.5
                    }],
                    "createdAt": "now", "updatedAt": "now"
                },
                {
                    "id": "card-4", "boardId": "work", "number": 4, "title": "Ship",
                    "statusId": "ready", "priority": "none", "position": 3,
                    "pendingRun": {"statusId": "ready", "since": "2026-09-06T11:59:50Z"},
                    "createdAt": "now", "updatedAt": "now"
                }
            ],
            "liveRuns": [{
                "cardId": "card-2", "run": "00000000-0000-4000-8000-000000000001",
                "status": "blocked", "started": "2026-09-06T11:58:00Z"
            }]
        }))
        .expect("the fixture is a board view")
    }

    /// Each mark says a different thing a reader has to do, and none of them moves the fields a
    /// board row already printed.
    #[test]
    fn a_board_row_marks_work_attention_and_a_run_it_is_owed() {
        let text = board(&view(), None, NOW);
        assert!(
            text.contains("FLT-2  \u{25cf} working  high  Build"),
            "{text}"
        );
        assert!(text.contains("FLT-3  ! needs you  low  Test"), "{text}");
        assert!(
            text.contains("FLT-4  \u{2026} pending  none  Ship"),
            "{text}"
        );
        // A card nothing is happening to keeps exactly the row it had before runs existed.
        assert!(text.contains("FLT-1  medium  Design"), "{text}");
    }

    /// The block count follows the title, and a satisfied blocker is not one.
    #[test]
    fn a_blocked_card_counts_only_the_blockers_that_have_not_finished() {
        let text = board(&view(), None, NOW);
        assert!(text.contains("Test  \u{2298} 1"), "{text}");
        // FLT-2's only blocker sits in a Completed column, so it is not waiting on anything.
        assert!(!text.contains("Build  \u{2298}"), "{text}");
    }

    /// The bolt is the warning a move into this column starts work, and the header says how much
    /// of the board's budget is already spent.
    #[test]
    fn the_columns_and_the_header_say_what_the_board_is_running() {
        let text = board(&view(), None, NOW);
        assert!(text.contains("Ready (2) \u{26a1}"), "{text}");
        assert!(text.contains("Todo (1)\n"), "{text}");
        let header = text.lines().nth(1).expect("the header is the second line");
        // One run is live and one card is owed a slot: stated apart, never as `2/2 working`,
        // so a queue behind the limit cannot read as the limit broken.
        assert_eq!(
            header, "backend: local \u{b7} 1 working \u{b7} 1 waiting \u{b7} 1 needs you",
            "{text}"
        );
        let mut running = view();
        for card in &mut running.cards {
            card.pending_run = None;
        }
        let text = board(&running, None, NOW);
        assert_eq!(
            text.lines().nth(1),
            Some("backend: local \u{b7} 1/2 working \u{b7} 1 needs you"),
            "{text}"
        );
    }

    /// A board running nothing prints the header it always printed.
    #[test]
    fn a_board_with_no_runs_keeps_its_header_and_its_rows() {
        let mut view = view();
        view.live_runs.clear();
        for card in &mut view.cards {
            card.runs.clear();
            card.pending_run = None;
        }
        let text = board(&view, None, NOW);
        assert_eq!(text.lines().nth(1), Some("backend: local"), "{text}");
        assert!(
            !text.contains('\u{25cf}') && !text.contains('\u{2026}'),
            "{text}"
        );
    }

    /// `board list` carries the same two counts the daemon summarises.
    #[test]
    fn the_board_table_carries_the_working_and_attention_counts() {
        let view = view();
        let summary = fleet_core::board::summarize(
            &view.board,
            &view.cards,
            &view.live_runs,
            &rfc3339_utc(NOW),
        );
        let text = boards(&[summary]);
        let header = text.lines().next().expect("a header");
        assert!(header.ends_with("CONFLICTS  WORKING  NEEDS YOU"), "{text}");
        let row = text.lines().nth(1).expect("one board");
        let counts: Vec<&str> = row.split_whitespace().rev().take(2).collect();
        assert_eq!(counts, ["1", "2"], "{text}");
    }

    /// The run line is the same eleven fields wherever it is printed, and the live join names the
    /// state the daemon would use.
    #[test]
    fn a_run_line_names_its_column_state_spend_and_thread() {
        let view = view();
        let live = card_runs(&view.board, &view.cards[1], &view.live_runs, Some(NOW));
        let fields: Vec<&str> = live.split('\t').collect();
        assert_eq!(fields.len(), 11, "{live}");
        assert_eq!(fields[10], "\u{2014}", "{live}");
        assert_eq!(fields[1], "Ready", "{live}");
        assert_eq!(fields[2], "blocked", "{live}");
        assert_eq!(fields[3], "codex", "{live}");
        assert_eq!(fields[5], "\u{2014}", "{live}");
        assert_eq!(fields[6], "2m 00s", "{live}");
        // A surface with no clock prints no duration for a run that has not ended, rather than
        // counting from the epoch.
        let clockless = card_runs(&view.board, &view.cards[1], &[], None);
        assert_eq!(
            clockless.split('\t').nth(6),
            Some("\u{2014}"),
            "{clockless}"
        );
        assert_eq!(clockless.split('\t').nth(2), Some("running"), "{clockless}");

        let ended = card_runs(&view.board, &view.cards[2], &[], Some(NOW));
        let fields: Vec<&str> = ended.split('\t').collect();
        assert_eq!(fields[2], "needs you", "{ended}");
        assert_eq!(fields[6], "2m 30s", "{ended}");
        assert_eq!(fields[7], "1200", "{ended}");
        assert_eq!(fields[8], "$0.50", "{ended}");
    }

    /// The card report gains the workflow sections, and a field the card did not set is named as
    /// the column's.
    #[test]
    fn a_card_report_names_its_agent_its_links_and_its_runs() {
        let view = view();
        let text = board_card(&view.board, &view.cards, &view.cards[1]);
        assert!(
            text.contains(
                "Agent: codex (column default), model gpt-5 (column default), effort low"
            ),
            "{text}"
        );
        assert!(text.contains("Blocked by: FLT-1"), "{text}");
        assert!(!text.contains("\u{2298}"), "{text}");
        assert!(text.contains("Blocks: FLT-3"), "{text}");
        assert!(
            text.contains("\nRuns\n00000000-0000-4000-8000-000000000001\tReady"),
            "{text}"
        );

        // The card waiting on this one says what it is waiting for, and how loudly.
        let blocked = board_card(&view.board, &view.cards, &view.cards[2]);
        assert!(
            blocked.contains("Blocked by: FLT-2  \u{2298} 1 waiting"),
            "{blocked}"
        );
    }

    /// A board nobody has automated prints the report it printed before automation existed.
    #[test]
    fn a_card_with_no_workflow_grows_no_sections() {
        let view = view();
        let plain = view.cards[0].clone();
        let text = board_card(&view.board, std::slice::from_ref(&plain), &plain);
        for absent in ["Agent:", "Blocked by:", "Blocks:", "\nRuns\n"] {
            assert!(!text.contains(absent), "{absent:?} in {text}");
        }
    }

    /// The ninth field says which surface is waiting on the child.
    #[test]
    fn a_delegation_line_names_its_caller() {
        let thread: Delegation = serde_json::from_value(json!({
            "id": "00000000-0000-4000-8000-000000000003",
            "caller": "00000000-0000-4000-8000-000000000001",
            "callerTurn": "00000000-0000-4000-8000-000000000004",
            "callerItem": "00000000-0000-4000-8000-000000000005",
            "child": "00000000-0000-4000-8000-000000000002",
            "provider": "codex", "depth": 1, "brief": "b", "expectation": "e",
            "status": "running", "delivery": {"type": "pending"},
            "created": "2026-09-18T12:00:00Z"
        }))
        .expect("the fixture is a delegation");
        let line = subagents_with_keys(
            std::slice::from_ref(&thread),
            SystemTime::UNIX_EPOCH,
            &CallerKeys::new(),
        );
        assert!(
            line.ends_with("\tpending\tthread 00000000-0000-4000-8000-000000000001"),
            "{line}"
        );

        let card: Delegation = serde_json::from_value(json!({
            "id": "00000000-0000-4000-8000-000000000003",
            "caller": {"board": "work", "card": "card-2"},
            "child": "00000000-0000-4000-8000-000000000002",
            "provider": "codex", "depth": 1, "brief": "b", "expectation": "e",
            "status": "running", "delivery": {"type": "pending"},
            "created": "2026-09-18T12:00:00Z"
        }))
        .expect("the fixture is a delegation");
        // With no key resolved the raw card id is still a selector every board verb accepts.
        let line = subagents_with_keys(
            std::slice::from_ref(&card),
            SystemTime::UNIX_EPOCH,
            &CallerKeys::new(),
        );
        assert!(line.ends_with("\tcard card-2"), "{line}");

        let keys = CallerKeys::from([("card-2".parse().expect("a card id"), "FLT-2".to_owned())]);
        let line = subagents_with_keys(std::slice::from_ref(&card), SystemTime::UNIX_EPOCH, &keys);
        assert!(line.ends_with("\tcard FLT-2"), "{line}");
        assert!(
            subagent_status_with_keys(&card, SystemTime::UNIX_EPOCH, &keys)
                .lines()
                .next()
                .is_some_and(|first| first.ends_with("\tcard FLT-2")),
            "the status report opens with the list line"
        );
    }

    /// The stamp the run counts are measured against is the epoch clock the header already had.
    #[test]
    fn the_board_clock_round_trips_through_rfc_3339() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(NOW), "2026-09-06T12:00:00Z");
        for epoch in [0, 1, 86_399, 951_782_400, NOW, 4_102_444_800] {
            assert_eq!(epoch_secs(&rfc3339_utc(epoch)), Some(epoch), "{epoch}");
        }
    }
}
