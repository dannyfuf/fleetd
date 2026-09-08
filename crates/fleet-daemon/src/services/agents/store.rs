//! Append-only native-agent event log and durable thread index.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, bail};
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, ModelSelection, PermissionMode, Seq, SeqEvent, ThreadId, TurnOutcome,
    },
    ids::WorktreeId,
};
use serde::{Deserialize, Serialize};

/// Persisted metadata index version.
pub const AGENT_INDEX_VERSION: u32 = 1;

/// Persisted transcript-log schema version.
///
/// §6 gives the index "its own file and version" and requires that threads "stay browsable
/// read-only regardless". The transcript had neither: a line written by a build whose
/// [`AgentEvent`] schema differs fails to decode, and [`AgentStore::load`] cannot tell that
/// apart from a crash's half-written tail — so it quarantines the line *and every event behind
/// it*, and the next append writes over a transcript this build simply could not read. The
/// header line stamps the schema the log was written against, so a mismatch is refused whole
/// and the bytes are left exactly where they are.
pub const AGENT_LOG_VERSION: u32 = 1;

/// The marker that distinguishes a log header line from a [`SeqEvent`] line.
const AGENT_LOG_MARKER: &str = "fleet-agent-events";

/// Filesystem-backed native-agent transcript and index store.
///
/// Serialization is per thread rather than per store: two threads write two different files,
/// and one thread's whole-log replay must not hold every other thread's streaming append behind
/// it. The index keeps its own lock, because it is the one file they do share.
#[derive(Debug, Clone)]
pub struct AgentStore {
    root: PathBuf,
    logs: Arc<Mutex<HashMap<ThreadId, Arc<Mutex<()>>>>>,
    index: Arc<Mutex<()>>,
}

impl AgentStore {
    /// Creates a store rooted at `$FLEET_HOME/agents`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            logs: Arc::new(Mutex::new(HashMap::new())),
            index: Arc::new(Mutex::new(())),
        }
    }

    /// The mutation lock of one thread's event log.
    fn log_lock(&self, thread: ThreadId) -> Arc<Mutex<()>> {
        Arc::clone(
            self.logs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(thread)
                .or_default(),
        )
    }

    /// Returns the store root.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Appends one sequenced event durably before broadcast.
    pub fn append(&self, thread: ThreadId, event: &SeqEvent) -> anyhow::Result<()> {
        let log = self.log_lock(thread);
        let _guard = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = self.root.join(thread.to_string());
        fs::create_dir_all(&directory)
            .with_context(|| format!("create agent thread directory `{}`", directory.display()))?;
        let path = directory.join("events.ndjson");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open agent event log `{}`", path.display()))?;
        // A new log declares the schema it is written against on its first line, so a later
        // build reading it can tell "I do not understand this" from "a crash cut this in half".
        if file
            .metadata()
            .with_context(|| format!("stat agent event log `{}`", path.display()))?
            .len()
            == 0
        {
            serde_json::to_writer(&mut file, &LogHeader::default())
                .with_context(|| format!("encode agent event log header for thread {thread}"))?;
            file.write_all(b"\n")
                .with_context(|| format!("append agent event log `{}`", path.display()))?;
        }
        serde_json::to_writer(&mut file, event)
            .with_context(|| format!("encode agent event for thread {thread}"))?;
        file.write_all(b"\n")
            .with_context(|| format!("append agent event log `{}`", path.display()))?;
        file.flush()
            .with_context(|| format!("flush agent event log `{}`", path.display()))?;
        if sync_required(&event.event) {
            file.sync_data()
                .with_context(|| format!("sync agent event log `{}`", path.display()))?;
        }
        Ok(())
    }

    /// Loads every retained event for a thread in sequence order.
    ///
    /// A tail that cannot be replayed — a half-written final line from a crash, a decode
    /// failure, a sequence break — is quarantined rather than failing the thread. §6 replays
    /// every thread's whole log at start and keeps threads "browsable read-only regardless", so
    /// one torn line must not take a whole transcript out of `summaries()` and answer
    /// `NotFound` for the life of the daemon. The log is truncated at the last line that did
    /// replay, and the discarded bytes are kept beside it, exactly as a broken index is.
    ///
    /// A log whose header names a schema version this build does not implement is the one case
    /// that is *not* torn: it fails whole, with the log left exactly as it is on disk.
    pub fn load(&self, thread: ThreadId) -> anyhow::Result<Vec<SeqEvent>> {
        let log = self.log_lock(thread);
        let _guard = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.root.join(thread.to_string()).join("events.ndjson");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("open agent event log `{}`", path.display()));
            }
        };
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut expected = Seq(1);
        let mut replayed_bytes = 0_u64;
        let mut offset = 0_u64;
        let mut number = 0_usize;
        let torn = loop {
            let mut line = String::new();
            number += 1;
            let read = match reader.read_line(&mut line) {
                Ok(0) => break None,
                Ok(read) => read,
                Err(error) => break Some(format!("line {number} could not be read: {error}")),
            };
            offset = offset.saturating_add(read as u64);
            if line.trim().is_empty() {
                replayed_bytes = offset;
                continue;
            }
            // The header is the first line of every log this build writes. A log stamped with a
            // schema this build does not implement is not torn: quarantining it would move a
            // transcript another build wrote out of the way on every open, so it is refused
            // whole and left untouched — the same answer `read_index` gives an index version it
            // does not know.
            if number == 1
                && let Ok(header) = serde_json::from_str::<LogHeader>(line.trim_end())
                && header.log == AGENT_LOG_MARKER
            {
                if header.version != AGENT_LOG_VERSION {
                    bail!(
                        "unsupported native-agent log version {} in `{}`; expected {}",
                        header.version,
                        path.display(),
                        AGENT_LOG_VERSION
                    );
                }
                replayed_bytes = offset;
                continue;
            }
            // A last line with no newline is a write the crash cut in half, never an event.
            if !line.ends_with('\n') {
                break Some(format!("line {number} ends without a newline"));
            }
            let event: SeqEvent = match serde_json::from_str(line.trim_end()) {
                Ok(event) => event,
                Err(error) => break Some(format!("line {number} could not be decoded: {error}")),
            };
            if event.seq != expected {
                break Some(format!(
                    "line {number} is out of order: expected {expected}, got {}",
                    event.seq
                ));
            }
            expected = expected.next();
            events.push(event);
            replayed_bytes = offset;
        };
        if let Some(reason) = torn {
            tracing::warn!(
                thread = %thread,
                log = %path.display(),
                reason,
                replayed = events.len(),
                "quarantining the unreplayable tail of a native-agent event log"
            );
            if let Err(error) = quarantine_log_tail(&path, replayed_bytes) {
                tracing::warn!(%error, log = %path.display(), "could not quarantine the log tail");
            }
        }
        Ok(events)
    }

    /// Drops every logged event after `last`, keeping the discarded bytes beside the log.
    ///
    /// This is the reducer's half of the same quarantine [`AgentStore::load`] performs: an event
    /// that decodes and is in sequence but that the projection rejects on replay would
    /// otherwise stay on disk forever, and the next append — written at the sequence replay
    /// actually reached — would collide with it. `None` empties the log.
    pub fn truncate_after(&self, thread: ThreadId, last: Option<Seq>) -> anyhow::Result<()> {
        let log = self.log_lock(thread);
        let _guard = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.root.join(thread.to_string()).join("events.ndjson");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("open agent event log `{}`", path.display()));
            }
        };
        let mut reader = BufReader::new(file);
        let mut keep = 0_u64;
        let mut offset = 0_u64;
        loop {
            let mut line = String::new();
            let read = reader
                .read_line(&mut line)
                .with_context(|| format!("read agent event log `{}`", path.display()))?;
            if read == 0 {
                break;
            }
            offset = offset.saturating_add(read as u64);
            let Some(seq) = serde_json::from_str::<SeqEvent>(line.trim_end())
                .ok()
                .map(|event| event.seq)
            else {
                continue;
            };
            if last.is_some_and(|last| seq <= last) {
                keep = offset;
            } else {
                break;
            }
        }
        quarantine_log_tail(&path, keep)
    }

    /// Reads and validates the native-agent thread index.
    pub fn read_index(&self) -> anyhow::Result<AgentIndex> {
        let _guard = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.root.join("index.json");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(broken) = broken_index(&self.root)? {
                    bail!(
                        "native-agent index is quarantined at `{}`",
                        broken.display()
                    );
                }
                return Ok(AgentIndex::default());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read agent index `{}`", path.display()));
            }
        };
        let parsed = serde_json::from_str::<AgentIndex>(&text)
            .context("decode native-agent index")
            .and_then(validate_index);
        match parsed {
            Ok(index) => Ok(index),
            Err(error) => {
                let quarantine = quarantine_path(&path);
                fs::rename(&path, &quarantine).with_context(|| {
                    format!(
                        "quarantine corrupt agent index `{}` as `{}`",
                        path.display(),
                        quarantine.display()
                    )
                })?;
                Err(error.context(format!(
                    "agent index quarantined at `{}`",
                    quarantine.display()
                )))
            }
        }
    }

    /// Atomically replaces the native-agent thread index.
    pub fn write_index(&self, index: &AgentIndex) -> anyhow::Result<()> {
        let _guard = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_index(index.clone())?;
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create agent store `{}`", self.root.display()))?;
        let path = self.root.join("index.json");
        let temporary = self
            .root
            .join(format!(".index.json.{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("create agent index temporary `{}`", temporary.display()))?;
        let write_result = (|| -> anyhow::Result<()> {
            serde_json::to_writer_pretty(&mut file, index).context("encode native-agent index")?;
            file.write_all(b"\n")
                .context("terminate native-agent index")?;
            file.flush().context("flush native-agent index")?;
            file.sync_all().context("sync native-agent index")?;
            fs::rename(&temporary, &path).with_context(|| {
                format!(
                    "replace agent index `{}` with `{}`",
                    path.display(),
                    temporary.display()
                )
            })?;
            sync_directory(&self.root)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ignored = fs::remove_file(&temporary);
        }
        write_result
    }
}

/// Whether this event has to reach the disk before the daemon says it happened.
///
/// `GateResolved` is here for the same reason `GateOpened` is: §3 rule 4 closes a gate only on
/// the resolution event, so a durable open with a lost close replays a card no adapter can ever
/// answer — and restart recovery only settles gates on threads it considers orphaned. Both ends
/// of a gate are rare enough that the fsync costs nothing on the streaming path.
fn sync_required(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TurnCompleted { .. }
            | AgentEvent::GateOpened { .. }
            | AgentEvent::GateResolved { .. }
            | AgentEvent::SessionExited { .. }
    )
}

fn validate_index(index: AgentIndex) -> anyhow::Result<AgentIndex> {
    if index.version != AGENT_INDEX_VERSION {
        bail!(
            "unsupported native-agent index version {}; expected {}",
            index.version,
            AGENT_INDEX_VERSION
        );
    }
    let mut threads = std::collections::HashSet::new();
    for record in &index.threads {
        if !threads.insert(record.thread) {
            bail!("duplicate native-agent thread {} in index", record.thread);
        }
    }
    Ok(index)
}

/// Moves everything after `keep` bytes of a log into a sibling file and truncates the log.
///
/// The bytes are preserved before they are dropped, so a torn transcript can still be inspected
/// — the same discipline `StateStore` applies to a broken index.
fn quarantine_log_tail(path: &Path, keep: u64) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open agent event log `{}`", path.display()))?;
    let length = file
        .metadata()
        .with_context(|| format!("stat agent event log `{}`", path.display()))?
        .len();
    if length <= keep {
        return Ok(());
    }
    file.seek(std::io::SeekFrom::Start(keep))
        .with_context(|| format!("seek agent event log `{}`", path.display()))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)
        .with_context(|| format!("read the tail of `{}`", path.display()))?;
    let quarantined = path.with_file_name(format!(
        "events.ndjson.broken-{}-{}",
        Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4()
    ));
    fs::write(&quarantined, &tail)
        .with_context(|| format!("write quarantined log tail `{}`", quarantined.display()))?;
    file.set_len(keep)
        .with_context(|| format!("truncate agent event log `{}`", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync agent event log `{}`", path.display()))?;
    Ok(())
}

fn quarantine_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "index.json.broken-{}-{}",
        Utc::now().timestamp_millis(),
        uuid::Uuid::new_v4()
    ))
}

fn broken_index(root: &Path) -> anyhow::Result<Option<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("list agent store `{}`", root.display()));
        }
    };
    for entry in entries {
        let path = entry
            .with_context(|| format!("read agent store entry in `{}`", root.display()))?
            .path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("index.json.broken-"))
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    File::open(path)
        .with_context(|| format!("open agent store directory `{}`", path.display()))?
        .sync_all()
        .with_context(|| format!("sync agent store directory `{}`", path.display()))
}

/// The first line of a native-agent transcript log: the schema it was written against.
///
/// `log` is what tells a header line apart from a [`SeqEvent`] line, so a log written before
/// this header existed still replays: its first line has no `log` field and is decoded as the
/// event it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LogHeader {
    /// Format marker, always [`AGENT_LOG_MARKER`].
    log: String,
    /// Schema version the events below were written against.
    version: u32,
}

impl Default for LogHeader {
    fn default() -> Self {
        Self {
            log: AGENT_LOG_MARKER.to_owned(),
            version: AGENT_LOG_VERSION,
        }
    }
}

/// Versioned native-agent thread metadata index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIndex {
    /// Persisted schema version.
    pub version: u32,
    /// Threads in creation order.
    pub threads: Vec<AgentThreadRecord>,
}

impl Default for AgentIndex {
    fn default() -> Self {
        Self {
            version: AGENT_INDEX_VERSION,
            threads: Vec::new(),
        }
    }
}

/// Persisted metadata needed to list and resume one thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadRecord {
    /// Thread identity.
    pub thread: ThreadId,
    /// Owning worktree.
    pub worktree: WorktreeId,
    /// Provider kind.
    pub provider: AgentKind,
    /// Display title.
    pub title: String,
    /// Creation time.
    pub created: DateTime<Utc>,
    /// Most recent event time.
    pub last_activity: DateTime<Utc>,
    /// Provider-native resume cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cursor: Option<String>,
    /// Last active model selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// Last active permission mode.
    pub mode: PermissionMode,
    /// Last settled turn outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<TurnOutcome>,
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{
        AgentEvent, GateAnswer, GateId, GateKind, GateResolver, PlanAnswer, Seq,
    };

    use super::*;

    #[test]
    fn both_ends_of_a_gate_are_made_durable() {
        // §3 rule 4 closes a gate only on `GateResolved`, and restart recovery settles gates
        // only for threads it considers orphaned: a durable open with a lost close leaves a
        // card no adapter can answer.
        assert!(sync_required(&AgentEvent::GateOpened {
            gate: GateId::new(),
            turn: None,
            kind: GateKind::Plan {
                markdown: String::new(),
                steps: Vec::new(),
            },
        }));
        assert!(sync_required(&AgentEvent::GateResolved {
            gate: GateId::new(),
            answer: GateAnswer::Plan(PlanAnswer::Approve),
            by: GateResolver::User,
        }));
        assert!(!sync_required(&AgentEvent::Notice("chatter".to_owned())));
    }

    #[test]
    fn append_and_load_round_trip_in_sequence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let thread = ThreadId::new();
        let first = event(1, AgentEvent::Notice("first".to_owned()));
        let second = event(2, AgentEvent::Notice("second".to_owned()));

        store.append(thread, &first).expect("append first");
        store.append(thread, &second).expect("append second");

        assert_eq!(store.load(thread).expect("load"), vec![first, second]);
    }

    #[test]
    fn index_write_is_atomic_and_corruption_is_quarantined() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let index = AgentIndex {
            threads: vec![record()],
            ..AgentIndex::default()
        };
        store.write_index(&index).expect("write index");
        assert_eq!(store.read_index().expect("read index"), index);
        assert!(
            std::fs::read_dir(store.root())
                .expect("list store")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );

        std::fs::write(store.root().join("index.json"), "not json").expect("corrupt index");
        let error = store.read_index().expect_err("corruption must fail");
        assert!(error.to_string().contains("quarantined"));
        assert!(!store.root().join("index.json").exists());
        assert!(
            broken_index(store.root())
                .expect("find quarantine")
                .is_some()
        );
        assert!(store.read_index().is_err());
    }

    /// C2: a crash routinely leaves the last line half written; the thread must survive it.
    #[test]
    fn a_torn_tail_is_quarantined_and_everything_before_it_still_replays() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let thread = ThreadId::new();
        let first = event(1, AgentEvent::Notice("first".to_owned()));
        let second = event(2, AgentEvent::Notice("second".to_owned()));
        store.append(thread, &first).expect("append first");
        store.append(thread, &second).expect("append second");
        let path = store.root().join(thread.to_string()).join("events.ndjson");
        let mut torn = fs::read_to_string(&path).expect("read log");
        torn.push_str("{\"seq\":3,\"at\":\"2026-09-07T12:00:00Z\",\"eve");
        fs::write(&path, &torn).expect("tear the log");

        assert_eq!(
            store.load(thread).expect("load"),
            vec![first, second.clone()]
        );

        // The tail is gone from the log and kept beside it, so the next append continues at 3.
        let repaired = fs::read_to_string(&path).expect("read repaired log");
        assert!(!repaired.contains("\"seq\":3"), "{repaired}");
        let quarantined = fs::read_dir(path.parent().expect("thread directory"))
            .expect("list thread directory")
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("events.ndjson.broken-")
            });
        assert!(quarantined, "the torn tail was not preserved");

        let third = event(3, AgentEvent::Notice("third".to_owned()));
        store.append(thread, &third).expect("append third");
        assert_eq!(
            store.load(thread).expect("reload"),
            vec![
                event(1, AgentEvent::Notice("first".to_owned())),
                second,
                third
            ]
        );
    }

    #[test]
    fn truncate_after_drops_exactly_the_events_replay_did_not_reach() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let thread = ThreadId::new();
        for seq in 1..=3 {
            store
                .append(thread, &event(seq, AgentEvent::Notice(format!("n{seq}"))))
                .expect("append");
        }
        store
            .truncate_after(thread, Some(Seq(2)))
            .expect("truncate the log");
        let loaded = store.load(thread).expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].seq, Seq(2));
    }

    /// R7: the log now says which schema it was written against, like the index does.
    #[test]
    fn a_new_log_stamps_its_schema_version_on_its_first_line() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let thread = ThreadId::new();
        let first = event(1, AgentEvent::Notice("first".to_owned()));
        store.append(thread, &first).expect("append first");

        let path = store.root().join(thread.to_string()).join("events.ndjson");
        let text = fs::read_to_string(&path).expect("read log");
        let header = text.lines().next().expect("header line");
        assert_eq!(
            serde_json::from_str::<LogHeader>(header).expect("decode header"),
            LogHeader::default()
        );
        // The header costs no event: the replay is exactly what was appended, and appending
        // again writes only the event.
        assert_eq!(store.load(thread).expect("load"), vec![first.clone()]);
        let second = event(2, AgentEvent::Notice("second".to_owned()));
        store.append(thread, &second).expect("append second");
        assert_eq!(store.load(thread).expect("reload"), vec![first, second]);
        assert_eq!(
            fs::read_to_string(&path).expect("read log").lines().count(),
            3
        );
    }

    /// R7: a transcript another build wrote must not be chopped every time this build opens it.
    #[test]
    fn a_log_from_another_schema_version_is_refused_whole_instead_of_quarantined() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let thread = ThreadId::new();
        store
            .append(thread, &event(1, AgentEvent::Notice("first".to_owned())))
            .expect("append first");
        let path = store.root().join(thread.to_string()).join("events.ndjson");
        let text = fs::read_to_string(&path).expect("read log");
        let mut lines = text.lines();
        lines.next().expect("header line");
        let future = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&LogHeader {
                log: AGENT_LOG_MARKER.to_owned(),
                version: AGENT_LOG_VERSION + 1,
            })
            .expect("encode header"),
            lines.next().expect("event line"),
            r#"{"seq":2,"at":"2026-09-07T12:00:00Z","event":{"type":"invented_later"}}"#
        );
        fs::write(&path, &future).expect("write a newer log");

        let error = store.load(thread).expect_err("an unknown schema must fail");
        assert!(error.to_string().contains("unsupported"), "{error}");
        assert_eq!(fs::read_to_string(&path).expect("reread log"), future);
        assert!(
            !fs::read_dir(path.parent().expect("thread directory"))
                .expect("list thread directory")
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("events.ndjson.broken-")),
            "a log this build cannot read must not be quarantined"
        );
    }

    /// R7: logs written before the header existed still replay.
    #[test]
    fn a_log_without_a_header_still_replays_every_event() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AgentStore::new(temp.path().join("agents"));
        let thread = ThreadId::new();
        let first = event(1, AgentEvent::Notice("first".to_owned()));
        let second = event(2, AgentEvent::Notice("second".to_owned()));
        let directory = store.root().join(thread.to_string());
        fs::create_dir_all(&directory).expect("thread directory");
        let legacy = format!(
            "{}\n{}\n",
            serde_json::to_string(&first).expect("encode first"),
            serde_json::to_string(&second).expect("encode second")
        );
        fs::write(directory.join("events.ndjson"), &legacy).expect("write a legacy log");

        assert_eq!(store.load(thread).expect("load"), vec![first, second]);
        assert_eq!(
            fs::read_to_string(directory.join("events.ndjson")).expect("reread log"),
            legacy
        );
    }

    fn event(seq: u64, event: AgentEvent) -> SeqEvent {
        SeqEvent {
            seq: Seq(seq),
            at: "2026-09-07T12:00:00Z".parse().expect("timestamp"),
            raw: None,
            event,
        }
    }

    fn record() -> AgentThreadRecord {
        AgentThreadRecord {
            thread: ThreadId::new(),
            worktree: WorktreeId::try_from("acme/api#feature").expect("worktree"),
            provider: AgentKind::Claude,
            title: "Thread".to_owned(),
            created: "2026-09-07T12:00:00Z".parse().expect("created"),
            last_activity: "2026-09-07T12:00:00Z".parse().expect("activity"),
            resume_cursor: Some("cursor".to_owned()),
            model: None,
            mode: PermissionMode::Ask,
            last_outcome: None,
        }
    }
}
