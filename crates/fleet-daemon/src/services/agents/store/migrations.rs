//! The forward-only migration ladder for the native-agent database, and its runner.
//!
//! Six rules govern this file, and they are what make the ladder safe to extend:
//!
//! 1. One numbered slot per change, never reused and never renumbered. A superseded migration
//!    stays registered as a no-op that burns its slot rather than disappearing, because a
//!    database that already recorded the slot must stay consistent with this manifest.
//! 2. The manifest is comparable against what a database recorded, which catches the real
//!    failure: two branches both claim a slot, and the second one's `CREATE TABLE` is silently
//!    skipped because the id is already in the ledger.
//! 3. A recorded `sql_sha256` that no longer matches its source is a fatal startup error, never a
//!    repair attempt. That is what makes rule 1 enforceable instead of aspirational.
//! 4. `CREATE INDEX IF NOT EXISTS` always, and `ADD COLUMN` always guarded by a
//!    `PRAGMA table_info` check, so re-running a slot is harmless.
//! 5. [`run`] takes a ceiling purely so a test can build the schema as of slot N-1, seed the rows
//!    that shape had, apply slot N, and assert the upgrade.
//! 6. Every slot carries a doc comment stating *why*, especially a column that is nullable on
//!    purpose or a change that drops something.
//!
//! A database whose ladder is *ahead* of this binary is refused at start with a clear message and
//! is never rebuilt: refusing to run is the same discipline the NDJSON log applied to an unknown
//! header version, and for the same reason — another build's transcript is not ours to rewrite.

use std::collections::BTreeMap;

use anyhow::{Context, bail};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use super::schema;

/// Ledger namespace for this database. A second database would use its own.
const DOMAIN: &str = "agents";

/// One numbered schema change.
pub(super) struct Migration {
    /// The slot. Strictly increasing across [`MIGRATIONS`], never reused, never renumbered.
    pub id: u32,
    /// Stable identifier recorded in the ledger; renaming a shipped slot is a fatal mismatch.
    pub name: &'static str,
    /// Applied inside the runner's single transaction.
    pub run: fn(&Transaction<'_>) -> rusqlite::Result<()>,
    /// The exact text [`Migration::sha256`] covers. A slot that runs Rust rather than a single
    /// batch still names the SQL it is defined by, so an edit to that text is caught.
    pub source: &'static str,
    /// SHA-256 of [`Migration::source`], recorded on first apply and compared on every open.
    pub sha256: &'static str,
}

/// The ladder, in slot order.
pub(super) const MIGRATIONS: &[Migration] = &[Migration {
    id: 1,
    name: "initial_schema",
    run: m001::run,
    source: schema::INITIAL_SCHEMA,
    sha256: "dbd290f683a3916d3c1cde9d8062892230a694c0f94c5e828d3210a3ab57bd62",
}];

/// Slot 001 — create the log and every read model derived from it.
///
/// One slot rather than one per table: nothing has shipped yet, so there is no database at slot
/// zero anywhere to upgrade incrementally, and a single slot keeps the initial shape readable as
/// one document in [`schema::INITIAL_SCHEMA`].
mod m001 {
    use rusqlite::Transaction;

    use super::schema;

    pub(super) fn run(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
        transaction.execute_batch(schema::INITIAL_SCHEMA)
    }
}

/// Configures the connection and applies every pending migration in one transaction.
///
/// Called once at store construction, before the manager exists. A failure here is fatal at
/// startup by design: the daemon must not run with half a truth, exactly as it refuses to run
/// with an unreadable `state.json`.
///
/// `to_inclusive` is a ceiling on the highest slot to apply and exists for the migration tests
/// described in this module's rule 5. Production callers pass `None`.
pub(super) fn run(conn: &mut Connection, to_inclusive: Option<u32>) -> anyhow::Result<()> {
    configure_writer(conn)?;
    validate_manifest()?;

    // IMMEDIATE, so a competing writer is refused at BEGIN rather than after the first statement
    // has already been applied.
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin agent database migration transaction")?;
    transaction
        .execute_batch(schema::MIGRATION_LEDGER)
        .context("create the agent database migration ledger")?;

    let applied = read_applied(&transaction)?;
    validate_applied(&applied)?;

    let ceiling = to_inclusive.unwrap_or(u32::MAX);
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.id <= ceiling && !applied.contains_key(&migration.id))
    {
        tracing::info!(
            slot = migration.id,
            name = migration.name,
            "applying agent database migration"
        );
        (migration.run)(&transaction)
            .with_context(|| format!("apply agent database migration {}", label(migration)))?;
        record(&transaction, migration)?;
    }

    transaction
        .commit()
        .context("commit agent database migrations")
}

/// Applies [`schema::WRITER_PRAGMAS`] to the read-write connection.
///
/// The runner does this itself rather than trusting its caller, because the ordering the pragma
/// list depends on has to hold before the first statement touches the file.
pub(super) fn configure_writer(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(schema::WRITER_PRAGMAS)
        .context("configure the agent database writer connection")
}

/// Applies [`schema::READER_PRAGMAS`] to a connection from the read-only pool.
pub(super) fn configure_reader(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(schema::READER_PRAGMAS)
        .context("configure an agent database reader connection")
}

fn record(transaction: &Transaction<'_>, migration: &Migration) -> anyhow::Result<()> {
    transaction
        .execute(
            "INSERT INTO fleet_migrations (domain, id, name, sql_sha256, applied_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                DOMAIN,
                i64::from(migration.id),
                migration.name,
                migration.sha256,
                chrono::Utc::now().timestamp_millis(),
            ],
        )
        .with_context(|| format!("record agent database migration {}", label(migration)))?;
    Ok(())
}

/// Rejects a manifest this binary could not apply coherently — a duplicated or out-of-order slot,
/// or a slot whose source text was edited after its hash was recorded.
fn validate_manifest() -> anyhow::Result<()> {
    let mut previous = 0;
    for migration in MIGRATIONS {
        if migration.id <= previous {
            bail!(
                "agent database migration slots must strictly increase; slot {} follows {previous}",
                migration.id
            );
        }
        let actual = sha256(migration.source);
        if actual != migration.sha256 {
            bail!(
                "agent database migration {} source hash changed: manifest says {}, source hashes to {actual}. \
                 A shipped slot is never edited; add the next slot instead",
                label(migration),
                migration.sha256
            );
        }
        previous = migration.id;
    }
    Ok(())
}

fn read_applied(transaction: &Transaction<'_>) -> anyhow::Result<BTreeMap<u32, AppliedMigration>> {
    let mut statement = transaction
        .prepare(
            "SELECT id, name, sql_sha256 FROM fleet_migrations WHERE domain = ?1 ORDER BY id ASC",
        )
        .context("prepare the applied agent migrations query")?;
    let rows = statement
        .query_map([DOMAIN], |row| {
            Ok(AppliedMigration {
                id: row.get(0)?,
                name: row.get(1)?,
                sha256: row.get(2)?,
            })
        })
        .context("query the applied agent migrations")?;

    let mut applied = BTreeMap::new();
    for row in rows {
        let migration = row.context("decode an applied agent migration")?;
        applied.insert(migration.id, migration);
    }
    Ok(applied)
}

/// Refuses a database this binary must not touch: one carrying a slot the binary does not know
/// (a newer build wrote it), or a slot whose recorded name or hash disagrees with the manifest.
fn validate_applied(applied: &BTreeMap<u32, AppliedMigration>) -> anyhow::Result<()> {
    for recorded in applied.values() {
        let Some(expected) = MIGRATIONS
            .iter()
            .find(|migration| migration.id == recorded.id)
        else {
            bail!(
                "the agent database records migration slot {} ({}), which this build does not know: \
                 it was written by a newer fleetd. Refusing to open it",
                recorded.id,
                recorded.name
            );
        };
        if recorded.name != expected.name {
            bail!(
                "agent database migration slot {} name mismatch: the database recorded `{}`, this build has `{}`",
                recorded.id,
                recorded.name,
                expected.name
            );
        }
        if recorded.sha256 != expected.sha256 {
            bail!(
                "agent database migration {} hash mismatch: the database recorded {}, this build has {}. \
                 A shipped slot was edited",
                label(expected),
                recorded.sha256,
                expected.sha256
            );
        }
    }
    Ok(())
}

fn label(migration: &Migration) -> String {
    format!("{:03}_{}", migration.id, migration.name)
}

fn sha256(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

/// A row of the `fleet_migrations` ledger.
struct AppliedMigration {
    id: u32,
    name: String,
    sha256: String,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use anyhow::Context;
    use rusqlite::{Connection, params};

    use super::schema::{REQUIRED_INDEXES, REQUIRED_TABLES};
    use super::{DOMAIN, MIGRATIONS, run, sha256};

    #[test]
    fn the_full_ladder_applies_cleanly_from_empty() -> anyhow::Result<()> {
        let mut conn = memory_database()?;

        run(&mut conn, None)?;

        assert_eq!(objects(&conn, "table")?, expected(REQUIRED_TABLES));
        assert_eq!(applied_slots(&conn)?, vec![1]);
        Ok(())
    }

    #[test]
    fn the_ladder_is_idempotent() -> anyhow::Result<()> {
        let mut conn = memory_database()?;
        run(&mut conn, None)?;
        let schema_before = schema_manifest(&conn)?;
        let ledger_before = ledger(&conn)?;

        run(&mut conn, None)?;

        assert_eq!(schema_manifest(&conn)?, schema_before);
        assert_eq!(ledger(&conn)?, ledger_before);
        Ok(())
    }

    #[test]
    fn every_index_the_read_queries_rely_on_exists() -> anyhow::Result<()> {
        let mut conn = memory_database()?;

        run(&mut conn, None)?;

        assert_eq!(objects(&conn, "index")?, expected(REQUIRED_INDEXES));
        Ok(())
    }

    /// Asserts the planner actually reaches each read path through its index. A missing or
    /// mis-ordered index still returns the right rows by scanning a whole thread, which is the
    /// failure the NDJSON store was replaced for and is invisible until a transcript is large.
    #[test]
    fn the_read_paths_are_index_seeks_not_scans() -> anyhow::Result<()> {
        let mut conn = memory_database()?;
        run(&mut conn, None)?;

        // Literal bounds rather than placeholders: `EXPLAIN QUERY PLAN` binds parameters like any
        // other statement, and the plan for a range predicate is the same either way.
        let paths: &[(&str, &str, &str)] = &[
            (
                "the page of turns",
                "SELECT turn_id, start_seq FROM turns \
                 WHERE thread_id = 'thread' AND start_seq < 900 ORDER BY start_seq DESC LIMIT 10",
                "idx_turns_keyset",
            ),
            (
                "items in the window",
                "SELECT item_id, turn_id, kind, status, start_seq FROM items \
                 WHERE thread_id = 'thread' AND start_seq >= 100 AND start_seq < 900 \
                 ORDER BY start_seq DESC, item_id DESC LIMIT 500",
                "idx_items_thread_seq",
            ),
            (
                "pinned open gates",
                "SELECT gate_id, kind_json, opened_seq FROM gates \
                 WHERE thread_id = 'thread' AND status = 'open' ORDER BY opened_seq ASC",
                "idx_gates_open",
            ),
            (
                "the resume window",
                "SELECT seq, at, raw, payload FROM agent_events \
                 WHERE thread_id = 'thread' AND seq > 100 AND seq <= 900 \
                 ORDER BY seq ASC LIMIT 1000",
                "idx_agent_events_thread_seq",
            ),
            (
                "the resume-admission probe",
                "SELECT COUNT(*) FROM agent_events \
                 WHERE thread_id = 'thread' AND kind = 'session_started' AND seq > 100",
                "idx_agent_events_thread_kind_seq",
            ),
            (
                "items under one subagent",
                "SELECT item_id FROM items \
                 WHERE thread_id = 'thread' AND parent_id = 'item' ORDER BY start_seq ASC",
                "idx_items_thread_parent",
            ),
            (
                "threads with an open gate",
                "SELECT thread_id FROM threads WHERE open_gate_count > 0",
                "idx_threads_open_gates",
            ),
            (
                "threads the projector is behind on",
                "SELECT thread_id FROM threads WHERE projected_seq < head_seq",
                "idx_threads_unprojected",
            ),
            (
                "mirrored threads",
                "SELECT thread_id FROM threads WHERE owner_host = 'host'",
                "idx_threads_owner",
            ),
        ];

        for (what, sql, index) in paths {
            let steps = query_plan(&conn, sql)?;
            let plan = steps.join(" | ");
            assert!(
                plan.contains(index),
                "{what} must be served by {index}, plan was: {plan}"
            );
            // A scan of a partial index is the intended plan for the two boot probes; a scan with
            // no index at all is the failure this test exists to catch.
            let unindexed = steps
                .iter()
                .any(|step| step.starts_with("SCAN") && !step.contains("USING"));
            assert!(!unindexed, "{what} must not scan a table, plan was: {plan}");
        }
        Ok(())
    }

    /// Rule 5: build the schema as of slot N-1, then apply slot N. With one slot shipped, N-1 is
    /// the empty database plus the ledger, which is exactly the state a fresh install starts in.
    #[test]
    fn slot_001_applies_onto_the_schema_at_slot_000() -> anyhow::Result<()> {
        let mut conn = memory_database()?;

        run(&mut conn, Some(0))?;
        assert_eq!(objects(&conn, "table")?, expected(&["fleet_migrations"]));
        assert_eq!(applied_slots(&conn)?, Vec::<u32>::new());

        run(&mut conn, Some(1))?;

        assert_eq!(objects(&conn, "table")?, expected(REQUIRED_TABLES));
        assert_eq!(applied_slots(&conn)?, vec![1]);
        Ok(())
    }

    #[test]
    fn every_slot_hash_matches_its_source() {
        for migration in MIGRATIONS {
            assert_eq!(
                sha256(migration.source),
                migration.sha256,
                "slot {} was edited after its hash was recorded",
                migration.id
            );
        }
    }

    #[test]
    fn a_database_ahead_of_this_build_is_refused() -> anyhow::Result<()> {
        let mut conn = memory_database()?;
        run(&mut conn, None)?;
        let unknown_slot = MIGRATIONS
            .iter()
            .map(|migration| migration.id)
            .max()
            .unwrap_or_default()
            + 1;
        conn.execute(
            "INSERT INTO fleet_migrations (domain, id, name, sql_sha256, applied_at) \
             VALUES (?1, ?2, 'from_a_newer_build', 'unknown', 0)",
            params![DOMAIN, unknown_slot],
        )
        .context("record a slot from a newer build")?;

        let refused = run(&mut conn, None);

        let message = format!("{:#}", refused.expect_err("a newer ladder must be refused"));
        assert!(message.contains("newer fleetd"), "message was: {message}");
        Ok(())
    }

    #[test]
    fn an_edited_shipped_slot_is_refused() -> anyhow::Result<()> {
        let mut conn = memory_database()?;
        run(&mut conn, None)?;
        conn.execute(
            "UPDATE fleet_migrations SET sql_sha256 = 'edited' WHERE domain = ?1 AND id = 1",
            params![DOMAIN],
        )
        .context("rewrite a recorded slot hash")?;

        let refused = run(&mut conn, None);

        let message = format!("{:#}", refused.expect_err("an edited slot must be refused"));
        assert!(message.contains("hash mismatch"), "message was: {message}");
        Ok(())
    }

    #[test]
    fn a_renamed_shipped_slot_is_refused() -> anyhow::Result<()> {
        let mut conn = memory_database()?;
        run(&mut conn, None)?;
        conn.execute(
            "UPDATE fleet_migrations SET name = 'renamed' WHERE domain = ?1 AND id = 1",
            params![DOMAIN],
        )
        .context("rename a recorded slot")?;

        let refused = run(&mut conn, None);

        let message = format!("{:#}", refused.expect_err("a renamed slot must be refused"));
        assert!(message.contains("name mismatch"), "message was: {message}");
        Ok(())
    }

    /// A second ladder in the same file must not see this one's slots, which is what `domain`
    /// buys: the ledger is shared, the slot space is not.
    #[test]
    fn the_ledger_is_scoped_by_domain() -> anyhow::Result<()> {
        let mut conn = memory_database()?;
        run(&mut conn, None)?;
        conn.execute(
            "INSERT INTO fleet_migrations (domain, id, name, sql_sha256, applied_at) \
             VALUES ('boards', 1, 'someone_elses_slot', 'other', 0)",
            [],
        )
        .context("record another domain's slot")?;

        run(&mut conn, None)?;

        assert_eq!(applied_slots(&conn)?, vec![1]);
        Ok(())
    }

    fn memory_database() -> anyhow::Result<Connection> {
        Connection::open_in_memory().context("open an in-memory agent database")
    }

    fn expected(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn objects(conn: &Connection, kind: &str) -> anyhow::Result<BTreeSet<String>> {
        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = ?1 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .with_context(|| format!("prepare the {kind} manifest query"))?;
        let rows = statement
            .query_map([kind], |row| row.get::<_, String>(0))
            .with_context(|| format!("query the {kind} manifest"))?;
        let mut names = BTreeSet::new();
        for row in rows {
            names.insert(row.with_context(|| format!("decode a {kind} name"))?);
        }
        Ok(names)
    }

    fn query_plan(conn: &Connection, sql: &str) -> anyhow::Result<Vec<String>> {
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .with_context(|| format!("prepare a query plan for `{sql}`"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(3))
            .with_context(|| format!("query the plan for `{sql}`"))?;
        let mut steps = Vec::new();
        for row in rows {
            steps.push(row.context("decode a query plan step")?);
        }
        Ok(steps)
    }

    fn schema_manifest(conn: &Connection) -> anyhow::Result<Vec<(String, String, Option<String>)>> {
        let mut statement = conn
            .prepare(
                "SELECT type, name, sql FROM sqlite_master \
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .context("prepare the schema manifest query")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .context("query the schema manifest")?;
        let mut manifest = Vec::new();
        for row in rows {
            manifest.push(row.context("decode a schema manifest row")?);
        }
        Ok(manifest)
    }

    fn ledger(conn: &Connection) -> anyhow::Result<Vec<(u32, String, String, i64)>> {
        let mut statement = conn
            .prepare(
                "SELECT id, name, sql_sha256, applied_at FROM fleet_migrations \
                 WHERE domain = ?1 ORDER BY id",
            )
            .context("prepare the migration ledger query")?;
        let rows = statement
            .query_map(params![DOMAIN], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .context("query the migration ledger")?;
        let mut recorded = Vec::new();
        for row in rows {
            recorded.push(row.context("decode a migration ledger row")?);
        }
        Ok(recorded)
    }

    fn applied_slots(conn: &Connection) -> anyhow::Result<Vec<u32>> {
        Ok(ledger(conn)?.into_iter().map(|(id, _, _, _)| id).collect())
    }
}
