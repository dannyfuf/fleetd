//! SQL text for the native-agent event store: connection settings, the migration ledger, and
//! the statements each numbered migration slot executes.
//!
//! This module holds no queries. The read path builds its statements where it runs them
//! (`docs/NATIVE-AGENTS.md` §8); what lives here is the shape those queries assume.

/// Settings applied to the single read-write connection, at open, in exactly this order.
///
/// The order is the decision, not the values: `busy_timeout` must be set **first** so the
/// `journal_mode` conversion below waits for another connection to release the database rather
/// than failing outright. That is the one mistake in this list which produces a daemon that will
/// not start, and only on the machine where a developer left a `sqlite3` session open — which is
/// also why `busy_timeout` is set at all, given that only `fleetd` ever writes this file.
pub(super) const WRITER_PRAGMAS: &str = r#"
PRAGMA busy_timeout = 5000;      -- FIRST, so the journal_mode conversion waits rather than fails
PRAGMA journal_mode = WAL;       -- persistent; a reader keeps a consistent snapshot while a turn streams
PRAGMA synchronous = NORMAL;     -- loses commits to power loss, never to a process crash; the writer
                                 -- raises it to FULL around a batch that must be durable
PRAGMA foreign_keys = OFF;       -- deliberate: a thread delete spans six tables AND attachment files,
                                 -- and the files must go strictly after the commit. A cascade would
                                 -- remove rows the file sweep never learned about, orphaning bytes
                                 -- on disk with no reference left to find them by
PRAGMA wal_autocheckpoint = 1000;-- fold the WAL back on a schedule instead of for the daemon's life
PRAGMA temp_store = MEMORY;      -- a sort that spills must not touch the disk on the write path
PRAGMA cache_size = -8000;       -- 8 MiB: the thread list plus one open transcript
PRAGMA trusted_schema = OFF;     -- a schema-level expression can never reach a function this
                                 -- process did not intend to expose
"#;

/// Settings applied to each connection in the read-only pool, in this order.
///
/// `query_only` is a second lock on top of `SQLITE_OPEN_READ_ONLY`: the open flag stops the file
/// from being written, this stops a statement that would try, so a misplaced `UPDATE` fails in
/// the test that runs it instead of silently taking the writer's place.
pub(super) const READER_PRAGMAS: &str = r#"
PRAGMA busy_timeout = 5000;
PRAGMA query_only = ON;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -4000;       -- 4 MiB per reader; two readers, so 8 MiB total
"#;

/// The ledger, created by the migration runner before it reads which slots were applied.
///
/// It is not itself a migration slot: a slot is recorded *in* this table, so the table has to
/// exist first. `domain` is reserved for a second database later; `sql_sha256` is the tripwire
/// that makes "a slot is never renumbered or edited after shipping" enforceable rather than
/// aspirational, and it catches a whitespace-only edit that changes nothing semantically but
/// proves the migration was touched after a build shipped it.
pub(super) const MIGRATION_LEDGER: &str = "\
CREATE TABLE IF NOT EXISTS fleet_migrations (
  domain      TEXT    NOT NULL,
  id          INTEGER NOT NULL,
  name        TEXT    NOT NULL,
  sql_sha256  TEXT    NOT NULL,
  applied_at  INTEGER NOT NULL,
  PRIMARY KEY (domain, id)
) WITHOUT ROWID;
";

/// Slot 001 — the append-only log and every read model derived from it.
///
/// Conventions this text follows, and every later slot must too: timestamps are `INTEGER` unix
/// milliseconds UTC and never ISO text, because the keyset anchor is an integer `seq` and nothing
/// needs a lexicographically sortable time; booleans are `INTEGER` 0/1; all JSON is `TEXT` and
/// never `BLOB`, so it stays greppable from `sqlite3` during an incident; and every index is
/// `IF NOT EXISTS` so a slot that adds one is safe to re-run.
pub(super) const INITIAL_SCHEMA: &str = r#"
-- ── the log ─────────────────────────────────────────────────────────────────
-- The only truth. Every table below it is derivable from these rows by replaying one thread.
CREATE TABLE agent_events (
  row_id     INTEGER PRIMARY KEY AUTOINCREMENT, -- global write order; never on the wire, never a cursor
  thread_id  TEXT    NOT NULL,
  seq        INTEGER NOT NULL,                  -- fleet_core::agents::Seq: dense from 1, per thread
  at         INTEGER NOT NULL,                  -- unix ms, stamped by the reducer
  kind       TEXT    NOT NULL,                  -- AgentEvent discriminant, e.g. 'content_delta'
  raw        TEXT,                              -- provider frame type name, diagnostics only
  payload    TEXT    NOT NULL,                  -- serde_json of AgentEvent
  bytes      INTEGER NOT NULL                   -- length(payload), so the resume budget sums a
                                                -- column instead of touching every payload page
);

-- `UNIQUE (thread_id, seq)` is a tripwire, not a concurrency mechanism: safety comes from the
-- single writer thread, and the constraint turns a hypothetical second writer into an aborted
-- transaction instead of silent corruption. It is declared as a named unique index rather than a
-- table-level constraint so the tripwire and the index the read path needs are one B-tree.
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_events_thread_seq
  ON agent_events(thread_id, seq);
-- Serves the "was this thread restarted inside the gap" probe on the resume path.
CREATE INDEX IF NOT EXISTS idx_agent_events_thread_kind_seq
  ON agent_events(thread_id, kind, seq);

-- ── quarantined events ──────────────────────────────────────────────────────
-- Where `truncate_after` puts the events it drops. A transaction has no torn tail, so the NDJSON
-- store's `events.ndjson.broken-*` machinery is gone — but the *other* reason it existed is not:
-- an event that decodes, is in sequence, and is then refused by the projection on replay must
-- leave the log so the next append does not collide with it, and losing it silently would destroy
-- the only evidence of why the replay stopped. It is moved here instead, with the reason, and
-- nothing reads this table on any hot path.
CREATE TABLE agent_events_quarantine (
  row_id         INTEGER PRIMARY KEY AUTOINCREMENT,
  thread_id      TEXT    NOT NULL,
  seq            INTEGER NOT NULL,
  at             INTEGER NOT NULL,
  kind           TEXT    NOT NULL,
  raw            TEXT,
  payload        TEXT    NOT NULL,
  bytes          INTEGER NOT NULL,
  quarantined_at INTEGER NOT NULL,
  reason         TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_events_quarantine_thread
  ON agent_events_quarantine(thread_id, seq);

-- ── threads: the list row and the projector cursor ──────────────────────────
-- Replaces index.json wholesale. No WITHOUT ROWID: this table is wide and updated constantly,
-- which is the shape a clustered key is worst at.
CREATE TABLE threads (
  thread_id            TEXT    PRIMARY KEY,
  worktree_id          TEXT    NOT NULL,
  owner_host           TEXT,                    -- NULL = this daemon owns it, else the remote HostId
  provider             TEXT    NOT NULL,        -- open string: the database must never be the thing
                                                -- that cannot store a value the wire can carry
  title                TEXT    NOT NULL,
  created_at           INTEGER NOT NULL,
  last_activity_at     INTEGER NOT NULL,

  -- head of the log and the projector cursor; head_seq == projected_seq is the healthy state
  head_seq             INTEGER NOT NULL DEFAULT 0,
  projected_seq        INTEGER NOT NULL DEFAULT 0,

  -- denormalized so a list read never touches items, turns or gates
  session_state        TEXT    NOT NULL,        -- SessionState discriminant
  running_turn_id      TEXT,
  last_outcome         TEXT,                    -- TurnOutcome discriminant
  -- The reducer's `TurnState` verbatim, so the list row carries the turn *and its identity*
  -- without a second query. `running_turn_id` alone cannot: `Completed(turn, outcome)`,
  -- `Interrupted(turn)` and `Failed(turn)` all clear it, and re-deriving which of the three a
  -- settled turn was from `last_outcome` is ambiguous — an authoritative `Interrupted` result and
  -- an aborted turn write the same outcome. Storing the enum removes the ambiguity instead of
  -- joining `turns` on every row of every list read.
  turn_json            TEXT    NOT NULL DEFAULT '{"type":"none"}',
  open_gate_count      INTEGER NOT NULL DEFAULT 0,
  attention            TEXT    NOT NULL,        -- AttentionKind, as the reducer derives it
  last_user_msg_seq    INTEGER,
  last_completed_seq   INTEGER,
  last_nonterminal_seq INTEGER,
  exit_code            INTEGER,
  -- The provider's current retry, as `AgentEvent::Retrying` reported it, or NULL when none is in
  -- flight. It is a column rather than a derivation because a retry is what tells the difference
  -- between a thread that is working and a thread that is idle, and the reducer clears it on the
  -- next event of any other kind: without it the denormalized `attention` this table exists to
  -- serve would disagree with `ThreadProjection::attention` for exactly the window in which a
  -- user is most likely to be looking at the list.
  retrying_json        TEXT,

  -- mirror bookkeeping; all NULL for a locally owned thread
  mirror_head_seq      INTEGER,                 -- the owner's head as last reported
  mirror_oldest_seq    INTEGER,                 -- how far back the mirrored window reaches; NULL = to the start
  mirror_synced_at     INTEGER,

  archived_at          INTEGER,
  deleted_at           INTEGER
);

CREATE INDEX IF NOT EXISTS idx_threads_list
  ON threads(deleted_at, archived_at, worktree_id, last_activity_at DESC, thread_id);
CREATE INDEX IF NOT EXISTS idx_threads_owner
  ON threads(owner_host) WHERE owner_host IS NOT NULL;
-- Partial, so "which threads have an open gate?" costs nothing on a database where none do.
CREATE INDEX IF NOT EXISTS idx_threads_open_gates
  ON threads(open_gate_count) WHERE open_gate_count > 0;
-- Partial, so "who is behind?" at start is free rather than a full table scan.
CREATE INDEX IF NOT EXISTS idx_threads_unprojected
  ON threads(thread_id) WHERE projected_seq < head_seq;

-- ── turns ───────────────────────────────────────────────────────────────────
-- One row per user prompt. There is deliberately no pending-turn placeholder and no nullable
-- turn id: the adapter emits TurnStarted at submission, so it owns the pending window and this
-- table never sees one.
CREATE TABLE turns (
  thread_id          TEXT    NOT NULL,
  turn_id            TEXT    NOT NULL,
  user_item_id       TEXT,
  start_seq          INTEGER NOT NULL,          -- seq of TurnStarted; the keyset anchor
  end_seq            INTEGER,                   -- seq of TurnCompleted / TurnAborted
  state              TEXT    NOT NULL,          -- running | completed | aborted | failed
  outcome            TEXT,                      -- TurnOutcome discriminant
  started_at         INTEGER NOT NULL,
  completed_at       INTEGER,
  duration_ms        INTEGER,                   -- provider duration minus gate-blocked time
  gate_blocked_ms    INTEGER NOT NULL DEFAULT 0,
  usage_json         TEXT,
  files_changed_json TEXT,
  PRIMARY KEY (thread_id, turn_id)
) WITHOUT ROWID;

-- Carries both the range predicate and the order of the page query, so its LIMIT genuinely
-- bounds the scan instead of sorting every turn in the thread into a temp B-tree first.
CREATE UNIQUE INDEX IF NOT EXISTS idx_turns_keyset
  ON turns(thread_id, start_seq);

-- ── items: the transcript rows ──────────────────────────────────────────────
CREATE TABLE items (
  thread_id     TEXT    NOT NULL,
  item_id       TEXT    NOT NULL,
  turn_id       TEXT,                            -- NULL for turnless items (imported history, notices)
  parent_id     TEXT,                            -- subagent nesting
  kind          TEXT    NOT NULL,                -- ItemKind discriminant
  tool_kind     TEXT,                            -- ToolKind discriminant when kind = 'tool'
  tool_name     TEXT,                            -- provider-native name, for the unknown-tool row
  status        TEXT    NOT NULL,                -- ItemStatus discriminant
  start_seq     INTEGER NOT NULL,                -- seq of ItemStarted; the row's stable sort key
  end_seq       INTEGER,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,

  -- Append-only text channels, concatenated in SQL by ContentDelta, which is what collapses
  -- thousands of delta rows into one row for reads while the log keeps every delta for replay.
  text          TEXT    NOT NULL DEFAULT '',     -- StreamKind::AssistantText
  reasoning     TEXT    NOT NULL DEFAULT '',     -- StreamKind::Reasoning
  output        TEXT    NOT NULL DEFAULT '',     -- StreamKind::ToolOutput, a bounded head+tail window
  output_bytes  INTEGER NOT NULL DEFAULT 0,      -- the true byte count, even when `output` is windowed
  output_elided INTEGER NOT NULL DEFAULT 0,      -- 1 when `output` is a window; the only lossy column

  detail_json   TEXT,                            -- input, summary, result, diff ref, provider extras
  PRIMARY KEY (thread_id, item_id)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_items_thread_seq
  ON items(thread_id, start_seq, item_id);
CREATE INDEX IF NOT EXISTS idx_items_thread_turn
  ON items(thread_id, turn_id, start_seq);
CREATE INDEX IF NOT EXISTS idx_items_thread_parent
  ON items(thread_id, parent_id, start_seq) WHERE parent_id IS NOT NULL;

-- ── gates: permission, question, plan ───────────────────────────────────────
CREATE TABLE gates (
  thread_id    TEXT    NOT NULL,
  gate_id      TEXT    NOT NULL,
  turn_id      TEXT,
  kind         TEXT    NOT NULL,                 -- 'permission' | 'question' | 'plan'
  kind_json    TEXT    NOT NULL,                 -- GateKind verbatim, provider-native option ids included
  status       TEXT    NOT NULL,                 -- 'open' | 'resolved'
  answer_json  TEXT,                             -- GateAnswer
  resolved_by  TEXT,                             -- GateResolver
  opened_seq   INTEGER NOT NULL,
  opened_at    INTEGER NOT NULL,
  -- The moment the thread began waiting on the *window* this gate belongs to, which is the
  -- moment the earliest overlapping gate opened rather than this one's own `opened_at`. A run of
  -- overlapping gates is one wait, so answering the first of them must not restart the clock for
  -- the second, and the blocked time the turn footer subtracts must not be charged twice.
  blocked_since INTEGER,
  resolved_seq INTEGER,
  resolved_at  INTEGER,
  PRIMARY KEY (thread_id, gate_id)
) WITHOUT ROWID;

-- Partial: an open gate is pinned into every page of every read, so this lookup happens on every
-- thread open and must cost nothing on a thread that has none.
CREATE INDEX IF NOT EXISTS idx_gates_open
  ON gates(thread_id, opened_seq) WHERE status = 'open';

-- ── checkpoints ─────────────────────────────────────────────────────────────
-- git_ref is a ref or an oid. The diff itself is never stored: DiffView asks fleet-git for it.
CREATE TABLE checkpoints (
  thread_id  TEXT    NOT NULL,
  turn_id    TEXT    NOT NULL,
  ordinal    INTEGER NOT NULL,                   -- turn count at capture; monotone per thread
  kind       TEXT    NOT NULL,                   -- CheckpointKind discriminant
  git_ref    TEXT,
  seq        INTEGER NOT NULL,
  at         INTEGER NOT NULL,
  files_json TEXT    NOT NULL DEFAULT '[]',
  PRIMARY KEY (thread_id, turn_id)
) WITHOUT ROWID;

CREATE UNIQUE INDEX IF NOT EXISTS idx_checkpoints_ordinal
  ON checkpoints(thread_id, ordinal);

-- ── session runtime ─────────────────────────────────────────────────────────
-- 1:1 with a thread, but not a pure projection: the daemon writes the runtime columns directly.
-- Column ownership is enforced by convention and by a test, never by SQL — a projection rebuild
-- rewrites the projector-owned columns and must leave the daemon-owned ones exactly as they were.
CREATE TABLE sessions (
  thread_id     TEXT PRIMARY KEY,
  -- projector-owned: derived from the log, rewritten by a rebuild
  provider      TEXT    NOT NULL,
  resume_cursor TEXT,
  model_json    TEXT,                            -- ModelSelection
  mode          TEXT    NOT NULL,                -- PermissionMode
  state         TEXT    NOT NULL,                -- SessionState
  last_error    TEXT,
  tools_json    TEXT,
  commands_json TEXT,
  skills_json   TEXT,
  -- daemon-owned: runtime bookkeeping a rebuild must not touch
  started_at    INTEGER,
  last_seen_at  INTEGER,
  restart_count INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

-- ── attachments: references only, bytes live on disk ────────────────────────
CREATE TABLE item_attachments (
  thread_id     TEXT    NOT NULL,
  item_id       TEXT    NOT NULL,
  attachment_id TEXT    NOT NULL,                -- <thread-segment>-<uuid>[-<ext>].<ext>
  relative_path TEXT    NOT NULL,                -- under $FLEET_HOME/agents/attachments
  mime          TEXT,
  bytes         INTEGER NOT NULL,
  PRIMARY KEY (thread_id, item_id, attachment_id)
) WITHOUT ROWID;

-- The sweep at start walks the attachments directory and asks this index whether a file on disk
-- is still referenced, so it is a lookup by path, not by owner.
CREATE INDEX IF NOT EXISTS idx_item_attachments_path
  ON item_attachments(relative_path);

-- ── per-client seen cursors ─────────────────────────────────────────────────
-- Gives AgentMarkSeen somewhere to land: today it is a wire round trip that validates its input
-- and returns an ack without recording anything.
CREATE TABLE seen (
  client_id TEXT    NOT NULL,                    -- stable per install, sent in Hello
  thread_id TEXT    NOT NULL,
  seq       INTEGER NOT NULL,
  at        INTEGER NOT NULL,
  PRIMARY KEY (client_id, thread_id)
) WITHOUT ROWID;
"#;

/// Every table the schema declares at head, as `sqlite_master` names them.
#[cfg(test)]
pub(super) const REQUIRED_TABLES: &[&str] = &[
    "agent_events",
    "agent_events_quarantine",
    "checkpoints",
    "fleet_migrations",
    "gates",
    "item_attachments",
    "items",
    "seen",
    "sessions",
    "threads",
    "turns",
];

/// Every index the read paths of `docs/NATIVE-AGENTS.md` §8 rely on.
///
/// A read query whose index is missing still returns the right rows, just by scanning a whole
/// thread — which is the exact failure the NDJSON store was replaced for, and it is invisible
/// until a transcript is large. So the set is asserted by name here and by query plan in
/// `migrations::tests`.
#[cfg(test)]
pub(super) const REQUIRED_INDEXES: &[&str] = &[
    "idx_agent_events_quarantine_thread",
    "idx_agent_events_thread_kind_seq",
    "idx_agent_events_thread_seq",
    "idx_checkpoints_ordinal",
    "idx_gates_open",
    "idx_item_attachments_path",
    "idx_items_thread_parent",
    "idx_items_thread_seq",
    "idx_items_thread_turn",
    "idx_threads_list",
    "idx_threads_open_gates",
    "idx_threads_owner",
    "idx_threads_unprojected",
    "idx_turns_keyset",
];

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use rusqlite::Connection;

    use super::{READER_PRAGMAS, WRITER_PRAGMAS};

    /// The ordering claim in [`WRITER_PRAGMAS`] is only worth making if the conversion it protects
    /// actually happens, and `journal_mode` is persistent only for a file-backed database — a
    /// `:memory:` connection reports `memory` and would pass a weaker assertion vacuously.
    #[test]
    fn writer_pragmas_convert_a_file_database_to_wal() -> anyhow::Result<()> {
        let directory = tempfile::tempdir().context("create pragma test directory")?;
        let conn = Connection::open(directory.path().join("state.sqlite"))
            .context("open file-backed agent database")?;

        conn.execute_batch(WRITER_PRAGMAS)
            .context("apply writer pragmas")?;

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .context("read journal_mode")?;
        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .context("read foreign_keys")?;
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .context("read busy_timeout")?;
        assert_eq!(journal_mode, "wal");
        assert_eq!(foreign_keys, 0);
        assert_eq!(busy_timeout, 5000);
        Ok(())
    }

    #[test]
    fn reader_pragmas_forbid_writes() -> anyhow::Result<()> {
        let conn = Connection::open_in_memory().context("open in-memory agent database")?;
        conn.execute_batch("CREATE TABLE probe (id INTEGER PRIMARY KEY)")
            .context("create probe table")?;

        conn.execute_batch(READER_PRAGMAS)
            .context("apply reader pragmas")?;

        let refused = conn.execute_batch("INSERT INTO probe (id) VALUES (1)");
        assert!(
            refused.is_err(),
            "a reader connection must refuse a write: {refused:?}"
        );
        Ok(())
    }
}
