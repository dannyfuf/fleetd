# 0013 — SQLite for agent transcripts

**Adopted** for `fleet-daemon`'s `services/agents/store/`. One SQLite database per daemon at
`$FLEET_HOME/agents/state.sqlite` holds the agent event log and every read model, replacing the
append-only NDJSON log per thread plus the whole-file `agents/index.json`.
`docs/NATIVE-AGENTS.md` §8 and §9 are the specification; this file records why.

**Not built.** This ADR fixes the decision so the implementation does not relitigate it.

## What the NDJSON design could not do

It was not slow because it was unoptimised; it was slow because a flat log has no index. Five
failures, each visible as latency:

- **`load()` is O(whole thread).** Opening a 4 000-turn thread parses every byte of every delta.
  There is no "last N", and a 20k-token assistant message is thousands of `ContentDelta` lines
  re-parsed on every open.
- **A cursored open is also O(whole thread).** Answering "give me events 4700..4711" replays the
  entire file — over SSH, after every reconnect, for every open thread.
- **Daemon start replays every thread's whole log**, so `fleetd` startup grows without bound with
  transcript history.
- **Four syscalls per event of synchronous `std::fs` on a tokio worker, under a
  `std::sync::Mutex`.** A streaming turn blocks a worker thread on `open`+`stat`+`write`+`flush`
  per token batch — a direct violation of `rust-async-background-work` Rule 10.
- **No queryable projection.** "Which threads have an open gate?" requires replaying every
  thread, so the thread list is a `Vec` rescanned linearly on every persisted event.

None of these is fixable without reimplementing an index, a page cache and a query planner.
t3code (`apps/server/src/persistence/`, 50 migrations) answers all five with one database, and the
two properties Fleet is buying are **bounded reads** — every query carries an explicit `LIMIT`,
every replay a row *and* byte budget summed in SQL without decoding — and **denormalized list
state**, so the thread list never touches the message or activity tables.

## The load-bearing choices

- **`rusqlite` with `bundled`, in `fleet-daemon` only, and no new crate.** `sqlx` was rejected on
  one decisive point: its `Transaction` is `async` and compiles fine when held across an `.await`,
  which under a single owned writer is a deadlock the type system does not catch, whereas
  `rusqlite`'s `Transaction<'_>` is not `Send` across that boundary and the borrow checker refuses
  it outright. Its compile-time SQL checking — the one thing it would add — needs a build-time
  database or a checked-in offline cache, and `make test` builds `fleetd` first on machines that
  have neither; for ~20 long-lived queries each already exercised against a `:memory:` database
  built *as of migration N-1*, that is a burden without a payoff. `sqlez` was rejected as
  vendoring Zed's wrapper for a single consumer. `bundled` costs ~30 s of first build and a `cc`
  dependency, and buys a guaranteed feature floor — window functions, `json_*`, partial indexes,
  `RETURNING` — on every machine; linking the host's `libsqlite3` would silently vary the
  available SQL.
- **SQLite lives in `fleet-daemon`, never in `fleet-core`.** Putting it in `fleet-core` would give
  `fleet-app`, `fleet-cli` and `fleet-ui-kit` a transitive C dependency and break the
  "`fleet-core` and `fleet-proto` are I/O-free" rule. `fleet-core` keeps `AgentEvent`, `SeqEvent`,
  `Seq` and the reducer, storage-agnostic, because the app holds the same types. No new crate: a
  crate costs a manifest, a `lib.rs`, a layering test entry and a docs row, and the only consumer
  that could justify one is a CLI reading transcripts without a daemon — but `fleet agent tail`
  goes over the socket.
- **One owned writer thread, named `fleet-agent-db`, owning one read-write connection for the
  process lifetime**, fed by an `mpsc` inbox where each command carries a `oneshot` reply. This is
  t3code's `Queue.unbounded` + `Deferred` (`OrchestrationEngine.ts:96`) transliterated, and it is
  the single most copyable idea in that codebase: one writer, FIFO, no interleaving, a
  promise-shaped API. `spawn_blocking` was rejected because a task that never returns occupies a
  slot for the daemon's life in a pool shared with PTY and git work; a named `std::thread` is
  honest about what it is. `Arc<Mutex<Connection>>` shared across tokio tasks was rejected as the
  same serialization with none of the clarity.
- **A separate 2-connection read-only pool** behind a semaphore, used from `spawn_blocking`. Under
  WAL a reader sees a consistent snapshot for the life of its transaction while the writer
  commits, which is what makes an atomic `(window, watermark)` read possible with no lock. t3code
  cannot exploit this — `node:sqlite` is synchronous and holds one handle behind a semaphore of 1 —
  and Fleet should.
- **Projections are synchronous with the append, in the same transaction, committed before the
  broadcast.** An eventually-consistent catch-up projector on the write path was rejected: by the
  time the append returns, every read model must already reflect it, which is precisely what makes
  reads instant and what lets the thread list repaint from denormalized counters in one frame. The
  background projector exists only at start, and only for threads whose cursor fell behind.
- **The projector cursor is per thread, not global.** t3code carries nine global cursors because
  its sequence is global; Fleet's `Seq` is already per-thread, which is the better fit for a
  per-thread subscription and makes lazy per-thread replay at start trivial.
- **The pagination cursor is `(thread_id, before_seq)` — one content-derived integer.** t3code's
  `(anchor_timestamp, turn_id)` keyset exists only because its sequence is global and its
  projection rows get rewritten. Fleet's log is append-only and its sequence is per thread, so one
  integer is sufficient and is cheap to compare, cheap to store, and cheap to resume from.
- **`PRAGMA busy_timeout` is set before `journal_mode = WAL`,** so the journal conversion waits
  rather than fails. `synchronous = NORMAL`, raised to `FULL` only around a batch that requires
  durability. `foreign_keys = OFF` deliberately: deletes are explicit multi-table statements in
  the projector, which is what you want when the same delete must also remove files.
- **Large payloads are never spilled to a blob table.** The item row carries a bounded head+tail
  window, the log carries the full bytes, and the client asks for the rest with `AgentItemBody`
  paged in 256 KiB chunks. t3code built `checkpoint_diff_blobs` in migration 003 and abandoned it;
  Fleet skips that step. Attachments go on disk, referenced from SQL, swept at start and GC'd
  after commit — never base64 in a row.
- **No compaction, no `VACUUM`, no prefix delete.** Deletion happens per *thread*, never per
  sequence prefix, because a prefix delete invalidates the meaning of a projector cursor. t3code's
  log has no retention anywhere and that is fine, because nothing scans it; the same holds here
  once every read is bounded.
- **`state.sqlite` is not part of `PersistedState`.** It gets its own migration ladder and its own
  failure mode: a database the daemon cannot open or migrate is fatal at start, exactly as an
  unreadable `state.json` is, and for the same reason — the daemon must not run with half a truth.

## What survives from the old store

Four disciplines, reproduced rather than discarded: a header-versioned log; torn-tail quarantine
rather than data loss; selective durability rather than an fsync per event; and
restart-recovery-as-appended-events, so a thread the log leaves `Starting` or `Running` is settled
explicitly by appending events and never by a silent state edit.

## Cost

A one-shot import moves existing NDJSON logs aside into `$FLEET_HOME/agents/imported/` after
replaying them, so no transcript is lost and the migration is inspectable. The `AgentStore` seam
is exactly seven public methods, so the replacement is a drop-in for `AgentSessionManager` and
nothing above the store needs to know.
