//! The one-shot NDJSON import: `$FLEET_HOME/agents/<thread>/events.ndjson` → SQLite, once.
//!
//! It runs at store construction on the writer's own connection **before the writer thread is
//! spawned**, so it has the only read-write handle to itself, no concurrency to reason about, and
//! no `.await` anywhere near a `Transaction<'_>`. By the time [`super::SqliteAgentStore::open`]
//! returns, every legacy transcript is either in the database or explicitly refused.
//!
//! It is not a migration slot. `docs/decisions/0013-sqlite-agent-transcripts.md` sketched it as
//! one, but a migration runs inside a single transaction that also owns the schema change, and
//! this step needs **one transaction per thread** (so one bad log costs one thread) and it moves
//! files, which must happen strictly *after* the commit that made the rows durable. Both are the
//! opposite of what a migration slot gives you.
//!
//! Four properties, in the order they matter:
//!
//! 1. **Nothing is deleted.** A log that has been imported is *moved* to
//!    `$FLEET_HOME/agents/imported/<thread>-<ms>.ndjson`, which is `quarantine_log_tail`'s
//!    discipline applied to the whole file. The unreadable tail of a torn log therefore survives
//!    twice: verbatim in that file, and row by row in `agent_events_quarantine`.
//! 2. **It is idempotent.** The move is what makes a second start find nothing, and a thread that
//!    already has rows in `agent_events` is skipped even if the move failed — so idempotency does
//!    not depend on the filesystem having cooperated.
//! 3. **A torn tail costs the tail, never the thread.** The readable prefix is imported, the rest
//!    is quarantined with its reason, and a `Notice` event records the tear in the transcript
//!    itself so a user reading the thread learns why it stops.
//! 4. **A log this build cannot read is left exactly where it is.** A header naming an unknown
//!    schema version aborts that thread's import and moves nothing, which is
//!    `AgentStore::load`'s rule verbatim and for its reason: destroying another build's
//!    transcript is worse than skipping it.

use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use fleet_core::agents::{AgentEvent, Seq, SeqEvent, ThreadId};
use rusqlite::{Connection, TransactionBehavior, params};

use super::{
    AgentIndex, AgentThreadRecord, index,
    project::{self, OptionalRow as _, StagedEvent, json_text},
};

/// The legacy per-thread log file name.
const LEGACY_LOG: &str = "events.ndjson";

/// The legacy whole-file index.
const LEGACY_INDEX: &str = "index.json";

/// Where imported files are moved. Never deleted; a follow-up release removes the directory.
const IMPORTED_DIR: &str = "imported";

/// The marker that distinguishes a legacy log header line from a [`SeqEvent`] line.
const LOG_MARKER: &str = "fleet-agent-events";

/// The only legacy log schema version this build knows how to read.
const LOG_VERSION: u32 = 1;

/// The only legacy index schema version this build knows how to read.
const INDEX_VERSION: u32 = 1;

/// What one import pass did, for the log line and for the tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ImportReport {
    /// Threads whose events were written to the database by this pass.
    pub(super) imported: usize,
    /// Threads whose log was already in the database, so only the file was moved.
    pub(super) already_present: usize,
    /// Threads left untouched: an unknown header version, or a log that could not be read.
    pub(super) refused: usize,
    /// Events moved to `agent_events_quarantine` because the prefix stopped before them.
    pub(super) quarantined: usize,
}

impl ImportReport {
    /// Whether any log was actually taken into the database by this pass or a previous one.
    fn moved(&self) -> bool {
        self.imported + self.already_present > 0
    }
}

/// Imports every legacy NDJSON log under `root`, then moves what it imported aside.
///
/// Returns without touching the database on a fresh install, which is the overwhelmingly common
/// case: the trigger is a `<thread>/events.ndjson` file existing at all.
pub(super) fn run(conn: &mut Connection, root: &Path) -> anyhow::Result<ImportReport> {
    let logs = legacy_logs(root)?;
    if logs.is_empty() {
        return Ok(ImportReport::default());
    }
    let records = legacy_records(root);
    let imported_dir = root.join(IMPORTED_DIR);
    fs::create_dir_all(&imported_dir).with_context(|| {
        format!(
            "create the imported-transcript directory `{}`",
            imported_dir.display()
        )
    })?;

    let mut report = ImportReport::default();
    for (thread, log) in logs {
        match import_one(conn, thread, &log, records.get(&thread), &mut report) {
            // The commit landed, so the file is history — and only then.
            Ok(()) => move_aside(&log, &imported_dir, &format!("{thread}"))?,
            Err(error) => {
                // One log this build must not or cannot read costs that thread and nothing else,
                // and its bytes stay exactly where they are so a later build can try again.
                report.refused += 1;
                tracing::warn!(
                    %thread,
                    log = %log.display(),
                    error = %format!("{error:#}"),
                    "refused a native-agent NDJSON transcript; leaving it in place"
                );
            }
        }
    }

    // The index is retired only when nothing is left behind that still needs it: a log this
    // build refused stays on disk for a later one, and moving its metadata away would leave that
    // build importing a transcript it cannot name.
    let legacy_index = root.join(LEGACY_INDEX);
    if legacy_index.is_file() && report.moved() && report.refused == 0 {
        move_aside(&legacy_index, &imported_dir, "index")?;
    }
    tracing::info!(
        imported = report.imported,
        already_present = report.already_present,
        refused = report.refused,
        quarantined = report.quarantined,
        "imported the native-agent NDJSON transcripts into the agent database"
    );
    Ok(report)
}

/// Imports one thread's log inside one transaction.
///
/// `Ok` means the events are durable and the file may be moved aside. `Err` means this build must
/// not, or could not, read them: the transaction rolled back whole and the caller leaves the bytes
/// alone.
fn import_one(
    conn: &mut Connection,
    thread: ThreadId,
    log: &Path,
    record: Option<&AgentThreadRecord>,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin a native-agent transcript import transaction")?;
    let present: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM agent_events WHERE thread_id = ?1 LIMIT 1",
            params![thread.to_string()],
            |row| row.get(0),
        )
        .optional_row()
        .context("probe for an already-imported transcript")?;
    if present.is_some() {
        // The rows are already here, so a repeat pass is a file move and nothing else. This is
        // what makes the import idempotent even when the move failed last time.
        drop(transaction);
        report.already_present += 1;
        tracing::info!(%thread, "a native-agent transcript was already imported; moving its log aside");
        return Ok(());
    }

    let prefix = read_prefix(log)?;
    match record {
        Some(record) => index::upsert(&transaction, record)?,
        // A log the legacy index does not name was already invisible under the old store, whose
        // index was authoritative for visibility. The events are imported so nothing is lost, and
        // the row is soft-deleted rather than half-listed with a worktree nobody can derive — the
        // same "omitted from the index means hidden, not erased" mapping `index.rs` documents.
        None => hide_unregistered(&transaction, thread)?,
    }
    for event in &prefix.events {
        let staged = StagedEvent::prepare(event)?;
        project::append_event(&transaction, thread, &staged)?;
    }
    if let Some(reason) = &prefix.torn {
        report.quarantined += quarantine_tail(&transaction, thread, &prefix, reason)?;
        let notice = SeqEvent {
            seq: prefix
                .events
                .last()
                .map_or(Seq(1), |event| event.seq.next()),
            at: chrono::Utc::now(),
            raw: Some("ndjson_import".to_owned()),
            event: AgentEvent::Notice(format!(
                "the imported transcript stopped early: {reason}. The original log is kept under \
                 agents/imported/ and the events after this point are in the quarantine table."
            )),
        };
        let staged = StagedEvent::prepare(&notice)?;
        project::append_event(&transaction, thread, &staged)?;
    }
    // One projector, one code path: the import writes no projection rows of its own, it replays
    // the log it just wrote through the same reducer every live append goes through. That is also
    // what leaves `head_seq == projected_seq`, so the boot repair pass finds nothing to do.
    project::rebuild_thread(&transaction, thread)?;
    transaction
        .commit()
        .context("commit a native-agent transcript import")?;
    report.imported += 1;
    tracing::info!(
        %thread,
        events = prefix.events.len(),
        torn = prefix.torn.is_some(),
        "imported a native-agent NDJSON transcript"
    );
    Ok(())
}

/// Registers a thread the legacy index never named, hidden rather than half-listed.
fn hide_unregistered(
    transaction: &rusqlite::Transaction<'_>,
    thread: ThreadId,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    transaction
        .execute(
            "INSERT INTO threads (thread_id, worktree_id, provider, title, created_at, \
             last_activity_at, session_state, attention, deleted_at) \
             VALUES (?1, '', '', '', ?2, ?2, 'starting', ?3, ?2) \
             ON CONFLICT (thread_id) DO NOTHING",
            params![
                thread.to_string(),
                now,
                json_text(&fleet_core::agents::Attention::Idle)?
            ],
        )
        .with_context(|| format!("register the unindexed imported thread {thread}"))?;
    tracing::warn!(
        %thread,
        "the legacy index does not name this imported thread; its events are kept and the thread is hidden"
    );
    Ok(())
}

/// Moves every line the prefix did not accept into `agent_events_quarantine`.
///
/// A half-written line has no sequence and no kind, so it is stored under `seq = 0` and
/// `kind = 'import_torn_tail'` with the raw line as its payload. Nothing reads this table on any
/// hot path; it exists so the reason a transcript stops is recoverable, and losing the bytes is
/// not acceptable.
fn quarantine_tail(
    transaction: &rusqlite::Transaction<'_>,
    thread: ThreadId,
    prefix: &Prefix,
    reason: &str,
) -> anyhow::Result<usize> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut moved = 0;
    for line in &prefix.tail {
        let seq = serde_json::from_str::<SeqEvent>(line.trim_end())
            .ok()
            .map_or(0, |event| i64::try_from(event.seq.0).unwrap_or(i64::MAX));
        transaction
            .execute(
                "INSERT INTO agent_events_quarantine \
                 (thread_id, seq, at, kind, raw, payload, bytes, quarantined_at, reason) \
                 VALUES (?1, ?2, ?3, 'import_torn_tail', 'ndjson_import', ?4, ?5, ?3, ?6)",
                params![
                    thread.to_string(),
                    seq,
                    now,
                    line,
                    i64::try_from(line.len()).unwrap_or(i64::MAX),
                    reason,
                ],
            )
            .with_context(|| format!("quarantine an unreadable log line of thread {thread}"))?;
        moved += 1;
    }
    Ok(moved)
}

/// The readable prefix of one legacy log, and whatever came after it.
struct Prefix {
    /// Events that decoded and were dense from sequence 1.
    events: Vec<SeqEvent>,
    /// Why the prefix stopped, or `None` when the whole file replayed.
    torn: Option<String>,
    /// Every line after the prefix, verbatim, including the one that broke it.
    tail: Vec<String>,
}

/// Reads one legacy log the way `AgentStore::load` did, prefix and tail kept apart.
///
/// The three rules it reproduces exactly: the first line may be a version header, an unknown
/// header version fails the whole file rather than tearing it, and a final line with no newline
/// is a write a crash cut in half rather than an event.
fn read_prefix(log: &Path) -> anyhow::Result<Prefix> {
    let file = fs::File::open(log)
        .with_context(|| format!("open the legacy agent event log `{}`", log.display()))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut tail = Vec::new();
    let mut expected = Seq(1);
    let mut number = 0_usize;
    let mut torn = None;
    loop {
        let mut line = String::new();
        number += 1;
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_read) => {}
            Err(error) => {
                torn = Some(format!("line {number} could not be read: {error}"));
                break;
            }
        }
        if line.trim().is_empty() {
            continue;
        }
        if number == 1
            && let Ok(header) = serde_json::from_str::<LogHeader>(line.trim_end())
            && header.log == LOG_MARKER
        {
            if header.version != LOG_VERSION {
                bail!(
                    "unsupported native-agent log version {} in `{}`; expected {LOG_VERSION}",
                    header.version,
                    log.display()
                );
            }
            continue;
        }
        if !line.ends_with('\n') {
            torn = Some(format!("line {number} ends without a newline"));
            tail.push(line);
            break;
        }
        let event: SeqEvent = match serde_json::from_str(line.trim_end()) {
            Ok(event) => event,
            Err(error) => {
                torn = Some(format!("line {number} could not be decoded: {error}"));
                tail.push(line);
                break;
            }
        };
        if event.seq != expected {
            torn = Some(format!(
                "line {number} is out of order: expected {expected}, got {}",
                event.seq
            ));
            tail.push(line);
            break;
        }
        expected = expected.next();
        events.push(event);
    }
    if torn.is_some() {
        // Everything behind the break is kept too: it is the rest of the evidence.
        for line in reader.lines() {
            match line {
                Ok(line) => tail.push(line),
                Err(error) => {
                    tracing::warn!(
                        log = %log.display(),
                        %error,
                        "could not read the whole torn tail of a legacy agent log"
                    );
                    break;
                }
            }
        }
    }
    Ok(Prefix { events, torn, tail })
}

/// Every `<thread>/events.ndjson` under the store root, keyed by thread.
fn legacy_logs(root: &Path) -> anyhow::Result<Vec<(ThreadId, PathBuf)>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("list the agent store root `{}`", root.display()));
        }
    };
    let mut logs = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("read an entry of the agent store `{}`", root.display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(thread) = name.parse::<ThreadId>() else {
            continue;
        };
        let log = entry.path().join(LEGACY_LOG);
        if log.is_file() {
            logs.push((thread, log));
        }
    }
    logs.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(logs)
}

/// The legacy `index.json`, or an empty map when there is none to read.
///
/// A missing or unreadable index is not fatal: the logs are the truth, and a thread the index
/// cannot describe is imported hidden rather than dropped.
fn legacy_records(root: &Path) -> HashMap<ThreadId, AgentThreadRecord> {
    let path = root.join(LEGACY_INDEX);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(error) => {
            tracing::warn!(index = %path.display(), %error, "could not read the legacy agent index");
            return HashMap::new();
        }
    };
    let index: AgentIndex = match serde_json::from_str(&text) {
        Ok(index) => index,
        Err(error) => {
            tracing::warn!(index = %path.display(), %error, "could not decode the legacy agent index");
            return HashMap::new();
        }
    };
    if index.version != INDEX_VERSION {
        tracing::warn!(
            index = %path.display(),
            version = index.version,
            expected = INDEX_VERSION,
            "the legacy agent index names a schema version this build does not read"
        );
        return HashMap::new();
    }
    index
        .threads
        .into_iter()
        .map(|record| (record.thread, record))
        .collect()
}

/// Moves one file into `imported/`, stamped so a repeat never overwrites the previous copy.
fn move_aside(path: &Path, imported: &Path, stem: &str) -> anyhow::Result<()> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("ndjson");
    let target = imported.join(format!(
        "{stem}-{}.{extension}",
        chrono::Utc::now().timestamp_millis()
    ));
    fs::rename(path, &target).with_context(|| {
        format!(
            "move the imported log `{}` to `{}`",
            path.display(),
            target.display()
        )
    })
}

/// The first line of a legacy transcript log: the schema it was written against.
///
/// `log` is what tells a header line apart from a [`SeqEvent`] line, so a log written before the
/// header existed still replays: its first line has no `log` field and decodes as the event it is.
#[derive(serde::Deserialize)]
struct LogHeader {
    log: String,
    version: u32,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::DateTime;
    use fleet_core::{
        agents::{
            AgentKind, ItemKind, ItemStatus, PermissionMode, Seq, ThreadId, TurnId, TurnOutcome,
            Usage,
        },
        ids::WorktreeId,
        paths::FleetHome,
    };

    use super::super::{
        AGENT_INDEX_VERSION, AgentIndex, AgentThreadRecord, SqliteAgentStore,
        tests::{database, dump, probe},
    };
    use super::*;

    /// Millisecond stamps, because the schema stores milliseconds: comparing a live append to an
    /// imported one would otherwise compare nanoseconds nothing keeps.
    fn stamp(offset: i64) -> chrono::DateTime<chrono::Utc> {
        DateTime::from_timestamp_millis(1_700_000_000_000 + offset).unwrap_or_else(chrono::Utc::now)
    }

    fn event(seq: u64, event: AgentEvent) -> SeqEvent {
        SeqEvent {
            seq: Seq(seq),
            at: stamp(i64::try_from(seq).unwrap_or_default()),
            raw: Some("fixture".to_owned()),
            event,
        }
    }

    /// One realistic turn: a session, a prompt, an answer, a settlement.
    fn transcript(thread: ThreadId) -> (AgentThreadRecord, Vec<SeqEvent>) {
        let turn = TurnId::new();
        let user = fleet_core::agents::ItemId::new();
        let assistant = fleet_core::agents::ItemId::new();
        let record = AgentThreadRecord {
            thread,
            worktree: WorktreeId::try_from("acme/api#feature").expect("worktree"),
            provider: AgentKind::Claude,
            title: "Claude".to_owned(),
            created: stamp(0),
            last_activity: stamp(0),
            resume_cursor: Some("session-42".to_owned()),
            model: None,
            mode: PermissionMode::Ask,
            last_outcome: None,
        };
        let events = vec![
            event(
                1,
                AgentEvent::SessionStarted {
                    provider: AgentKind::Claude,
                    resume_cursor: Some("session-42".to_owned()),
                    model: None,
                    mode: PermissionMode::Ask,
                    tools: Vec::new(),
                    commands: Vec::new(),
                    skills: Vec::new(),
                },
            ),
            event(
                2,
                AgentEvent::TurnStarted {
                    turn,
                    user_item: user,
                },
            ),
            event(
                3,
                AgentEvent::ItemStarted {
                    turn,
                    item: user,
                    kind: ItemKind::UserMessage {
                        text: "ship it".to_owned(),
                        attachments: Vec::new(),
                    },
                    parent: None,
                },
            ),
            event(
                4,
                AgentEvent::ItemCompleted {
                    item: user,
                    status: ItemStatus::Done,
                },
            ),
            event(
                5,
                AgentEvent::ItemStarted {
                    turn,
                    item: assistant,
                    kind: ItemKind::AssistantText,
                    parent: None,
                },
            ),
            event(
                6,
                AgentEvent::ContentDelta {
                    item: assistant,
                    stream: fleet_core::agents::StreamKind::AssistantText,
                    delta: "shipping".to_owned(),
                },
            ),
            event(
                7,
                AgentEvent::ItemCompleted {
                    item: assistant,
                    status: ItemStatus::Done,
                },
            ),
            event(
                8,
                AgentEvent::TurnCompleted {
                    turn,
                    outcome: TurnOutcome::Completed,
                    usage: Usage::default(),
                    duration_ms: 12,
                    files_changed: Vec::new(),
                },
            ),
        ];
        (record, events)
    }

    /// Writes the legacy layout the old store produced: a versioned log plus `index.json`.
    fn write_legacy(
        root: &Path,
        record: &AgentThreadRecord,
        lines: &[String],
    ) -> anyhow::Result<()> {
        let directory = root.join(record.thread.to_string());
        fs::create_dir_all(&directory).context("create a legacy thread directory")?;
        let mut text = format!("{{\"log\":\"{LOG_MARKER}\",\"version\":{LOG_VERSION}}}\n");
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        fs::write(directory.join(LEGACY_LOG), text).context("write a legacy log")?;
        let index = AgentIndex {
            version: AGENT_INDEX_VERSION,
            threads: vec![record.clone()],
        };
        fs::write(
            root.join(LEGACY_INDEX),
            serde_json::to_string_pretty(&index).context("encode a legacy index")?,
        )
        .context("write a legacy index")?;
        Ok(())
    }

    fn lines(events: &[SeqEvent]) -> anyhow::Result<Vec<String>> {
        events
            .iter()
            .map(|event| serde_json::to_string(event).context("encode a legacy log line"))
            .collect()
    }

    /// The property the whole import rests on: it produces the projection the reducer would.
    #[tokio::test]
    async fn an_imported_log_projects_exactly_as_a_live_append_sequence() -> anyhow::Result<()> {
        let thread = ThreadId::new();
        let (record, events) = transcript(thread);

        let live_dir = tempfile::tempdir().context("live store directory")?;
        let live = SqliteAgentStore::open(FleetHome::new(live_dir.path()).agents_db_path())?;
        live.write_record(&record).await?;
        for event in &events {
            live.append(thread, event).await?;
        }
        let expected = dump(&probe(&live)?, thread)?;

        let imported_dir = tempfile::tempdir().context("imported store directory")?;
        let home = FleetHome::new(imported_dir.path());
        fs::create_dir_all(home.agents_path()).context("create the legacy root")?;
        write_legacy(&home.agents_path(), &record, &lines(&events)?)?;

        let imported = SqliteAgentStore::open(home.agents_db_path())?;

        assert_eq!(
            dump(&probe(&imported)?, thread)?,
            expected,
            "an imported transcript must project row for row like a live one"
        );
        assert_eq!(imported.load(thread).await?, events, "every event survives");
        assert_eq!(
            imported.summaries().await?,
            live.summaries().await?,
            "and the list row is identical"
        );
        // Moved, never deleted, and the thread directory no longer holds a log to re-import.
        assert!(
            !home
                .agents_path()
                .join(thread.to_string())
                .join(LEGACY_LOG)
                .exists(),
            "the imported log was left where a second start would find it"
        );
        let moved = fs::read_dir(home.agents_path().join(IMPORTED_DIR))
            .context("list the imported directory")?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            moved
                .iter()
                .any(|name| name.starts_with(&thread.to_string())),
            "the log itself was not preserved: {moved:?}"
        );
        assert!(
            moved.iter().any(|name| name.starts_with("index-")),
            "the legacy index was not preserved: {moved:?}"
        );
        Ok(())
    }

    /// A second start must not double-import, whatever the filesystem did.
    #[tokio::test]
    async fn a_second_start_never_imports_the_same_log_twice() -> anyhow::Result<()> {
        let thread = ThreadId::new();
        let (record, events) = transcript(thread);
        let directory = tempfile::tempdir().context("store directory")?;
        let home = FleetHome::new(directory.path());
        fs::create_dir_all(home.agents_path()).context("create the legacy root")?;
        write_legacy(&home.agents_path(), &record, &lines(&events)?)?;

        let first = SqliteAgentStore::open(home.agents_db_path())?;
        let after_first = dump(&probe(&first)?, thread)?;
        drop(first);

        // The ordinary second start: the log is gone, so there is nothing to import.
        let second = SqliteAgentStore::open(home.agents_db_path())?;
        assert_eq!(dump(&probe(&second)?, thread)?, after_first);
        assert_eq!(second.load(thread).await?.len(), events.len());
        drop(second);

        // The hostile second start: the move failed last time, so the log is still there. The
        // rows are what makes the import idempotent, not the filesystem.
        write_legacy(&home.agents_path(), &record, &lines(&events)?)?;
        let third = SqliteAgentStore::open(home.agents_db_path())?;
        assert_eq!(
            dump(&probe(&third)?, thread)?,
            after_first,
            "a re-presented log was imported a second time"
        );
        assert_eq!(third.load(thread).await?, events);
        assert!(
            !home
                .agents_path()
                .join(thread.to_string())
                .join(LEGACY_LOG)
                .exists(),
            "the re-presented log was not moved aside"
        );
        Ok(())
    }

    /// A crash routinely leaves the last line half written; the thread must survive it.
    #[tokio::test]
    async fn a_torn_tail_imports_the_readable_prefix_and_quarantines_the_rest() -> anyhow::Result<()>
    {
        let thread = ThreadId::new();
        let (record, events) = transcript(thread);
        let directory = tempfile::tempdir().context("store directory")?;
        let home = FleetHome::new(directory.path());
        fs::create_dir_all(home.agents_path()).context("create the legacy root")?;

        // Everything through seq 4 is intact; seq 5 was cut in half by the crash.
        let mut written = lines(&events[..4])?;
        write_legacy(&home.agents_path(), &record, &written)?;
        let log = home.agents_path().join(thread.to_string()).join(LEGACY_LOG);
        let torn = format!(
            "{}{}",
            fs::read_to_string(&log).context("read the log")?,
            r#"{"seq":5,"at":"2026-09-07T12:00:00Z","eve"#
        );
        fs::write(&log, &torn).context("tear the log")?;
        written.clear();

        let store = SqliteAgentStore::open(home.agents_db_path())?;

        let imported = store.load(thread).await?;
        assert_eq!(
            imported[..4],
            events[..4],
            "the readable prefix must import verbatim"
        );
        // The tear is recorded in the transcript itself, so a reader learns why it stops.
        assert_eq!(imported.len(), 5, "the prefix plus one notice");
        let AgentEvent::Notice(notice) = &imported[4].event else {
            panic!("the import did not record the tear: {:?}", imported[4]);
        };
        assert!(notice.contains("stopped early"), "{notice}");

        // The bytes survive twice: row by row, and verbatim in the moved file.
        let quarantined: Vec<String> = probe(&store)?
            .prepare("SELECT payload FROM agent_events_quarantine WHERE thread_id = ?1")?
            .query_map(params![thread.to_string()], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(quarantined.len(), 1, "{quarantined:?}");
        assert!(quarantined[0].contains(r#""eve"#), "{quarantined:?}");
        let preserved = fs::read_dir(home.agents_path().join(IMPORTED_DIR))
            .context("list the imported directory")?
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&thread.to_string())
            })
            .map(|entry| fs::read_to_string(entry.path()))
            .transpose()
            .context("read the preserved log")?;
        assert_eq!(preserved.as_deref(), Some(torn.as_str()));

        // And the thread is live: the next append continues at the sequence the prefix reached.
        let next = event(6, AgentEvent::Notice("after the repair".to_owned()));
        store.append(thread, &next).await?;
        assert_eq!(store.load(thread).await?.len(), 6);
        Ok(())
    }

    /// A transcript another build wrote is not ours to rewrite.
    #[tokio::test]
    async fn a_log_from_a_newer_schema_is_refused_whole_and_left_in_place() -> anyhow::Result<()> {
        let thread = ThreadId::new();
        let (record, events) = transcript(thread);
        let directory = tempfile::tempdir().context("store directory")?;
        let home = FleetHome::new(directory.path());
        fs::create_dir_all(home.agents_path()).context("create the legacy root")?;
        write_legacy(&home.agents_path(), &record, &lines(&events)?)?;
        let log = home.agents_path().join(thread.to_string()).join(LEGACY_LOG);
        let text = fs::read_to_string(&log).context("read the log")?;
        let newer = text.replacen(
            &format!("\"version\":{LOG_VERSION}"),
            &format!("\"version\":{}", LOG_VERSION + 1),
            1,
        );
        fs::write(&log, &newer).context("write a newer log")?;

        let store = SqliteAgentStore::open(home.agents_db_path())?;

        assert!(
            store.load(thread).await?.is_empty(),
            "a log this build cannot read must not be imported"
        );
        assert_eq!(
            fs::read_to_string(&log).context("reread the log")?,
            newer,
            "a log this build cannot read must be left exactly as it is"
        );
        Ok(())
    }

    /// Logs written before the version header existed still import every event.
    #[tokio::test]
    async fn a_log_without_a_header_still_imports_every_event() -> anyhow::Result<()> {
        let thread = ThreadId::new();
        let (record, events) = transcript(thread);
        let directory = tempfile::tempdir().context("store directory")?;
        let home = FleetHome::new(directory.path());
        let thread_dir = home.agents_path().join(thread.to_string());
        fs::create_dir_all(&thread_dir).context("create the legacy thread directory")?;
        let mut text = String::new();
        for line in lines(&events)? {
            text.push_str(&line);
            text.push('\n');
        }
        fs::write(thread_dir.join(LEGACY_LOG), text).context("write a header-less log")?;
        let index = AgentIndex {
            version: AGENT_INDEX_VERSION,
            threads: vec![record],
        };
        fs::write(
            home.agents_path().join(LEGACY_INDEX),
            serde_json::to_string(&index).context("encode a legacy index")?,
        )
        .context("write a legacy index")?;

        let store = SqliteAgentStore::open(home.agents_db_path())?;

        assert_eq!(
            store.load(thread).await?,
            events,
            "a log with no header line must not lose its first event"
        );
        assert_eq!(store.summaries().await?.len(), 1);
        Ok(())
    }

    /// A log the legacy index never named keeps its events and stays hidden.
    #[tokio::test]
    async fn a_log_the_legacy_index_never_named_is_imported_hidden() -> anyhow::Result<()> {
        let thread = ThreadId::new();
        let (_record, events) = transcript(thread);
        let directory = tempfile::tempdir().context("store directory")?;
        let home = FleetHome::new(directory.path());
        let thread_dir = home.agents_path().join(thread.to_string());
        fs::create_dir_all(&thread_dir).context("create the legacy thread directory")?;
        let mut text = format!("{{\"log\":\"{LOG_MARKER}\",\"version\":{LOG_VERSION}}}\n");
        for line in lines(&events)? {
            text.push_str(&line);
            text.push('\n');
        }
        fs::write(thread_dir.join(LEGACY_LOG), text).context("write an unindexed log")?;

        let store = SqliteAgentStore::open(home.agents_db_path())?;

        assert_eq!(
            store.load(thread).await?,
            events,
            "an unindexed transcript must still be imported"
        );
        assert!(
            store.summaries().await?.is_empty(),
            "a thread with no metadata must be hidden, not half-listed"
        );
        let deleted: Option<i64> = probe(&store)?.query_row(
            "SELECT deleted_at FROM threads WHERE thread_id = ?1",
            params![thread.to_string()],
            |row| row.get(0),
        )?;
        assert!(deleted.is_some(), "the thread was not soft-deleted");
        assert!(
            database(&store).exists(),
            "the database the events landed in is gone"
        );
        Ok(())
    }
}
