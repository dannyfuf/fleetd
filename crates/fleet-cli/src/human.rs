//! Human-readable command output formatting.

use fleet_core::{
    inspection::WorktreeInspection,
    sessions::{SessionState, WorktreeStatus},
};
use fleet_proto::response::{
    DoctorCheck, DoctorStatus, PruneResult, SleepResult, WorktreeDeleteResult,
};

/// Formats the compact list summary.
#[must_use]
pub fn list(repo_count: usize, worktree_count: usize) -> String {
    format!("{repo_count} repos, {worktree_count} worktrees")
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
            let mut line = format!("{} {} {}", inspection.worktree_id, inspection.branch, state);
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

/// Formats board summaries as a column-aligned table.
#[must_use]
pub fn boards(boards: &[fleet_core::board::BoardSummary]) -> String {
    let header = [
        "BOARD",
        "CONTEXT",
        "NAME",
        "BACKEND",
        "CARDS",
        "OPEN",
        "DIRTY",
        "CONFLICTS",
    ]
    .map(str::to_owned)
    .to_vec();
    let mut rows = vec![header];
    rows.extend(boards.iter().map(|board| {
        let mut row = vec![
            board.id.to_string(),
            board.context_id.to_string(),
            crate::envelope::single_line(&board.name),
            crate::envelope::single_line(&board.backend_kind),
            board.card_count.to_string(),
            board.open_count.to_string(),
            board.dirty_count.to_string(),
            board.conflict_count.to_string(),
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
fn columns(rows: &[Vec<String>]) -> Vec<String> {
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
    let mut sections = vec![format!(
        "{} ({})\n{}",
        crate::envelope::single_line(&view.board.name),
        crate::envelope::single_line(&view.board.prefix),
        board_header(view, backend, now)
    )];
    for status in &view.board.statuses {
        let cards = fleet_core::board::column_cards(&view.cards, &status.id);
        let mut lines = vec![format!(
            "{} ({})",
            crate::envelope::single_line(&status.name),
            cards.len()
        )];
        for card in cards {
            let mut row = format!(
                "{}  {}  {}",
                display_key(&view.board, card),
                card.priority.label().to_lowercase(),
                crate::envelope::single_line(&card.title)
            );
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
    if !backend.is_local() {
        let summary = fleet_core::board::summarize(&view.board, &view.cards);
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
    ];
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
