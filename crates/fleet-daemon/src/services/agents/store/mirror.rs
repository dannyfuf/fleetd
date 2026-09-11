//! The mirror columns of `threads`, and the only statements allowed to write them.
//!
//! A mirrored thread lives in the **same tables** as a local one with one nullable column,
//! `threads.owner_host`, set (`docs/NATIVE-AGENTS.md` §9.3). That is not a storage economy: a
//! mirrored thread *is* a prefix of the owner's log carrying the owner's own `seq` values, so
//! every read in this store — the list `SELECT`, the replay, the window, the cursor codec —
//! serves it with no branch at all. `AgentThreadOpen` runs the same SQL whether the thread is
//! local or remote, which is the whole reason remote can feel local.
//!
//! **The mirror is a read-through cache, never a replica.** Three consequences are enforced here
//! rather than trusted:
//!
//! 1. Only events, never fields. Everything below either appends an owner-sequenced event or
//!    refreshes the denormalized header the owner broadcast. Nothing merges a projection
//!    field-by-field, so the mirror cannot invent a state the owner never had.
//! 2. Only from the owner's link. [`admits_append`] is the single decision, and it is a pure
//!    function so the pre-flight read and the in-transaction tripwire ask exactly the same
//!    question. Two writers on one sequence space is silent corruption, which is the one failure
//!    mode a cache must not have.
//! 3. Any disagreement resolves in favour of the owner. An owner whose head is *behind* our
//!    mirror has had its database replaced; the mirrored rows are discarded whole
//!    ([`discard`]) and refetched, because the mirror is never right against the owner.
//!
//! **Column meanings**, which the schema comments fix and this module implements:
//!
//! | Column | Meaning for a mirrored thread |
//! | --- | --- |
//! | `head_seq` / `projected_seq` | the prefix this daemon actually holds — identical meaning to a local thread, which is what keeps the boot census, the rebuild and every read unbranched |
//! | `mirror_head_seq` | the **owner's** head as last reported, so the admission ladder can compare without a round trip |
//! | `mirror_oldest_seq` | how far back the mirrored prefix reaches; `NULL` means "to the start", which is the only shape [`append_from_owner`] will build |
//! | `mirror_synced_at` | when the owner last confirmed content |

use anyhow::Context;
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{AgentThreadSummary, Seq, SeqEvent, ThreadId},
    ids::HostId,
};
use rusqlite::{Connection, Transaction, params};
use thiserror::Error;

use super::project::{
    self, OptionalRow as _, StagedEvent, discriminant, json_text, session_state_column,
};

/// Where a thread is owned, and how much of it this daemon holds.
///
/// Read in one statement against the `threads` primary key, which is why every authority check
/// below can afford to take it first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadOwnership {
    /// The host that owns the thread's log and sequence, or `None` for a locally owned thread.
    pub(crate) owner: Option<HostId>,
    /// The head of the prefix this daemon holds.
    pub(crate) head_seq: Seq,
    /// The projector cursor over that prefix.
    pub(crate) projected_seq: Seq,
    /// The owner's head as last reported — `threads.mirror_head_seq`.
    pub(crate) owner_head: Option<Seq>,
    /// How far back the mirrored prefix reaches; `None` means to the start of the log.
    pub(crate) oldest_seq: Option<Seq>,
    /// When the owner last confirmed this content.
    pub(crate) synced_at: Option<DateTime<Utc>>,
}

impl ThreadOwnership {
    /// Whether another host owns this thread's sequence space.
    pub(crate) const fn is_mirrored(&self) -> bool {
        self.owner.is_some()
    }

    /// Whether the mirror holds transcript this daemon can paint without the link.
    ///
    /// A row with no events is *cold*: it came from the summary stream and carries a header but
    /// no transcript, so an open has to reach the owner.
    pub(crate) const fn is_warm(&self) -> bool {
        self.head_seq.0 > 0
    }

    /// How far the owner is ahead of the prefix this daemon holds.
    pub(crate) fn pending_events(&self) -> u64 {
        self.owner_head
            .map_or(0, |head| head.0.saturating_sub(self.head_seq.0))
    }
}

/// Why an append from a host's link was refused.
///
/// Typed rather than prose because each variant has a different remedy: `Unknown` and `Gap` are
/// repaired by refetching a window, while `Local` and `Foreign` are bugs that must never be
/// repaired by writing anyway.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum MirrorRefusal {
    /// No row for this thread, so nothing says the sending host owns it.
    #[error(
        "agent thread {thread} is not in this daemon's mirror; host {host} must send its summary before its events"
    )]
    Unknown {
        /// The thread the event named.
        thread: ThreadId,
        /// The host the event arrived from.
        host: HostId,
    },
    /// The row exists and this daemon owns it.
    #[error(
        "agent thread {thread} is owned by this daemon; host {host} may not append to its sequence"
    )]
    Local {
        /// The thread the event named.
        thread: ThreadId,
        /// The host the event arrived from.
        host: HostId,
    },
    /// The row exists and a *different* host owns it.
    #[error(
        "agent thread {thread} is owned by host {owner}; host {writer} may not append to its sequence"
    )]
    Foreign {
        /// The thread the event named.
        thread: ThreadId,
        /// The host that owns the sequence space.
        owner: HostId,
        /// The host the event arrived from.
        writer: HostId,
    },
    /// The event is not the next one, so appending it would leave a hole in the prefix.
    #[error(
        "agent thread {thread} is mirrored to sequence {head} and host {host} sent {seq}; the mirror needs a refill"
    )]
    Gap {
        /// The thread the event named.
        thread: ThreadId,
        /// The host the event arrived from.
        host: HostId,
        /// The head of the prefix this daemon holds.
        head: Seq,
        /// The sequence the owner sent.
        seq: Seq,
    },
}

/// What [`admits_append`] decided about one owner-sequenced event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// The event extends the prefix by one and may be appended.
    Append,
    /// The prefix already contains this sequence. Overlap between a catch-up replay and live
    /// delivery is expected, so a duplicate is dropped rather than refused.
    Duplicate,
}

/// The one authority decision: may `writer`'s event at `seq` extend this thread's prefix?
///
/// Pure on purpose. The caller reads the row and asks this before it queues a write, and the
/// write transaction re-reads the row and asks it again; a decision made in two places from one
/// function cannot drift, and the second call is the tripwire that catches a racing writer the
/// first call could not see.
pub(crate) fn admits_append(
    thread: ThreadId,
    writer: &HostId,
    ownership: Option<&ThreadOwnership>,
    seq: Seq,
) -> Result<Admission, MirrorRefusal> {
    let Some(ownership) = ownership else {
        return Err(MirrorRefusal::Unknown {
            thread,
            host: writer.clone(),
        });
    };
    let Some(owner) = ownership.owner.as_ref() else {
        return Err(MirrorRefusal::Local {
            thread,
            host: writer.clone(),
        });
    };
    if owner != writer {
        return Err(MirrorRefusal::Foreign {
            thread,
            owner: owner.clone(),
            writer: writer.clone(),
        });
    }
    if seq <= ownership.head_seq {
        return Ok(Admission::Duplicate);
    }
    if seq == ownership.head_seq.next() {
        return Ok(Admission::Append);
    }
    Err(MirrorRefusal::Gap {
        thread,
        host: writer.clone(),
        head: ownership.head_seq,
        seq,
    })
}

/// The ownership row of one thread, or `None` when this daemon has never heard of it.
pub(super) fn read_ownership(
    conn: &Connection,
    thread: ThreadId,
) -> anyhow::Result<Option<ThreadOwnership>> {
    conn.prepare_cached(
        "SELECT owner_host, head_seq, projected_seq, mirror_head_seq, mirror_oldest_seq, \
                mirror_synced_at \
           FROM threads WHERE thread_id = ?1",
    )
    .context("prepare the thread ownership query")?
    .query_row(params![thread.to_string()], |row| {
        Ok(OwnershipRow {
            owner_host: row.get(0)?,
            head_seq: row.get(1)?,
            projected_seq: row.get(2)?,
            mirror_head_seq: row.get(3)?,
            mirror_oldest_seq: row.get(4)?,
            mirror_synced_at: row.get(5)?,
        })
    })
    .optional_row()
    .with_context(|| format!("read the ownership of thread {thread}"))?
    .map(|row| row.decode(thread))
    .transpose()
}

/// One ownership row, still encoded.
struct OwnershipRow {
    owner_host: Option<String>,
    head_seq: i64,
    projected_seq: i64,
    mirror_head_seq: Option<i64>,
    mirror_oldest_seq: Option<i64>,
    mirror_synced_at: Option<i64>,
}

impl OwnershipRow {
    fn decode(self, thread: ThreadId) -> anyhow::Result<ThreadOwnership> {
        let owner = self
            .owner_host
            .as_deref()
            .map(|host| HostId::try_from(host.to_owned()))
            .transpose()
            .with_context(|| format!("thread {thread} names an unusable owner host"))?;
        Ok(ThreadOwnership {
            owner,
            head_seq: seq(self.head_seq),
            projected_seq: seq(self.projected_seq),
            owner_head: self.mirror_head_seq.map(seq),
            oldest_seq: self.mirror_oldest_seq.map(seq),
            synced_at: self
                .mirror_synced_at
                .and_then(DateTime::from_timestamp_millis),
        })
    }
}

/// Records — or refreshes — the header of one thread another host owns.
///
/// This is the *only* way a row with `owner_host` set is created, and the only field-shaped write
/// the mirror performs: the columns it touches are exactly the ones the owner broadcasts in
/// [`AgentThreadSummary`], and it never touches `head_seq`, `projected_seq` or any projected row.
/// A header older than the prefix this daemon already holds is dropped rather than applied, so a
/// summary that arrives out of order cannot walk the list row backwards.
pub(super) fn claim(
    transaction: &Transaction<'_>,
    host: &HostId,
    summary: &AgentThreadSummary,
) -> anyhow::Result<()> {
    let thread = summary.thread;
    let current = read_ownership(transaction, thread)?;
    if let Some(current) = &current {
        match current.owner.as_ref() {
            None => {
                anyhow::bail!(
                    "{}",
                    MirrorRefusal::Local {
                        thread,
                        host: host.clone(),
                    }
                );
            }
            Some(owner) if owner != host => {
                anyhow::bail!(
                    "{}",
                    MirrorRefusal::Foreign {
                        thread,
                        owner: owner.clone(),
                        writer: host.clone(),
                    }
                );
            }
            Some(_) => {}
        }
        // The mirror is never right against the owner: a head behind our prefix means the
        // owner's database was restored or replaced, so the prefix is discarded whole and
        // refetched rather than reconciled.
        if summary.last_seq < current.head_seq {
            tracing::warn!(
                %thread,
                %host,
                mirrored = %current.head_seq,
                owner = %summary.last_seq,
                "the owner's head is behind this mirror; discarding the mirrored transcript"
            );
            discard(transaction, thread)?;
        }
    }

    let now = Utc::now().timestamp_millis();
    let last_activity = summary
        .last_activity
        .map_or(now, |at| at.timestamp_millis());
    let owner_head = i64::try_from(summary.last_seq.0).unwrap_or(i64::MAX);
    project::execute(
        transaction,
        "INSERT INTO threads (thread_id, worktree_id, owner_host, provider, title, created_at, \
         last_activity_at, session_state, turn_json, attention, last_completed_seq, \
         last_nonterminal_seq, exit_code, mirror_head_seq, mirror_synced_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
         ON CONFLICT (thread_id) DO UPDATE SET \
         worktree_id = excluded.worktree_id, provider = excluded.provider, \
         title = excluded.title, \
         last_activity_at = MAX(threads.last_activity_at, excluded.last_activity_at), \
         session_state = excluded.session_state, turn_json = excluded.turn_json, \
         attention = excluded.attention, last_completed_seq = excluded.last_completed_seq, \
         last_nonterminal_seq = excluded.last_nonterminal_seq, exit_code = excluded.exit_code, \
         mirror_head_seq = MAX(COALESCE(threads.mirror_head_seq, 0), excluded.mirror_head_seq), \
         mirror_synced_at = excluded.mirror_synced_at, deleted_at = NULL \
         WHERE excluded.mirror_head_seq >= COALESCE(threads.mirror_head_seq, 0)",
        params![
            thread.to_string(),
            summary.worktree.to_string(),
            host.to_string(),
            discriminant(&summary.provider, "AgentKind")?,
            summary.title,
            now,
            last_activity,
            session_state_column(&summary.session)?,
            json_text(&summary.turn)?,
            json_text(&summary.attention)?,
            summary.last_completed_seq.map(|seq| seq.0),
            summary.last_nonterminal_seq.map(|seq| seq.0),
            summary.exit_code,
            owner_head,
            now,
        ],
        "record a mirrored thread header",
    )
}

/// Appends one owner-sequenced event to a mirrored thread's prefix.
///
/// Returns whether the event extended the prefix; a duplicate answers `false` and writes nothing.
/// The append, its projection rows and the head bump are the same three calls a locally owned
/// event makes, at the owner's own sequence — which is what makes one read path serve both.
pub(super) fn append_from_owner(
    transaction: &Transaction<'_>,
    host: &HostId,
    thread: ThreadId,
    staged: &StagedEvent,
) -> anyhow::Result<bool> {
    let ownership = read_ownership(transaction, thread)?;
    match admits_append(thread, host, ownership.as_ref(), staged.event.seq) {
        Ok(Admission::Append) => {}
        Ok(Admission::Duplicate) => return Ok(false),
        Err(refusal) => return Err(anyhow::Error::new(refusal)),
    }
    project::append_event(transaction, thread, staged)?;
    project::project_event(transaction, thread, &staged.event)?;
    project::advance_head(transaction, thread, &staged.event)?;
    note_owner_head(transaction, thread, staged.event.seq)?;
    Ok(true)
}

/// Raises the owner's last-reported head and stamps the sync time.
///
/// Monotone by construction: a delta that arrives after a newer summary must not walk the ladder's
/// comparison backwards.
pub(super) fn note_owner_head(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    owner_head: Seq,
) -> anyhow::Result<()> {
    project::execute(
        transaction,
        "UPDATE threads SET \
         mirror_head_seq = MAX(COALESCE(mirror_head_seq, 0), ?2), mirror_synced_at = ?3 \
         WHERE thread_id = ?1 AND owner_host IS NOT NULL",
        params![
            thread.to_string(),
            i64::try_from(owner_head.0).unwrap_or(i64::MAX),
            Utc::now().timestamp_millis(),
        ],
        "record a mirrored thread's owner head",
    )
}

/// Deletes every mirrored row of one thread, leaving a cold header behind.
///
/// Only ever called for a thread another host owns: the mirror may throw its own cache away, and
/// may never throw away a transcript this daemon is the owner of. The header row itself survives
/// with its mirror columns cleared, so the thread stays in the list and the next open refetches
/// it.
pub(super) fn discard(transaction: &Transaction<'_>, thread: ThreadId) -> anyhow::Result<()> {
    let id = thread.to_string();
    let owned: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM threads WHERE thread_id = ?1 AND owner_host IS NULL",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("check whether thread {thread} is locally owned"))?;
    if owned.is_some() {
        anyhow::bail!("agent thread {thread} is owned by this daemon; its log is not a cache");
    }
    for statement in [
        "DELETE FROM agent_events WHERE thread_id = ?1",
        "DELETE FROM turns WHERE thread_id = ?1",
        "DELETE FROM items WHERE thread_id = ?1",
        "DELETE FROM gates WHERE thread_id = ?1",
        "DELETE FROM checkpoints WHERE thread_id = ?1",
        "DELETE FROM sessions WHERE thread_id = ?1",
        "DELETE FROM item_attachments WHERE thread_id = ?1",
    ] {
        transaction
            .execute(statement, params![id])
            .with_context(|| format!("discard the mirrored rows of thread {thread}"))?;
    }
    project::execute(
        transaction,
        "UPDATE threads SET head_seq = 0, projected_seq = 0, running_turn_id = NULL, \
         open_gate_count = 0, last_user_msg_seq = NULL, retrying_json = NULL, \
         mirror_head_seq = NULL, mirror_oldest_seq = NULL, mirror_synced_at = NULL \
         WHERE thread_id = ?1 AND owner_host IS NOT NULL",
        params![id],
        "cool a mirrored thread down to its header",
    )
}

/// How much log sits in `(after, head]`, for the admission ladder.
///
/// Sums the `bytes` column rather than reading payloads, which is the reason that column exists:
/// deciding whether a replay is admissible must not cost the replay.
pub(super) fn resume_admission(
    conn: &Connection,
    thread: ThreadId,
    after: Seq,
) -> anyhow::Result<(u64, u64)> {
    let (events, bytes): (i64, i64) = conn
        .prepare_cached(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM agent_events \
              WHERE thread_id = ?1 AND seq > ?2",
        )
        .context("prepare the resume admission query")?
        .query_row(
            params![
                thread.to_string(),
                i64::try_from(after.0).unwrap_or(i64::MAX)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .with_context(|| format!("measure the resumable tail of thread {thread}"))?;
    Ok((
        u64::try_from(events).unwrap_or_default(),
        u64::try_from(bytes).unwrap_or_default(),
    ))
}

/// The events one mirrored append batch will write, prepared off the writer thread.
pub(super) fn stage(events: &[SeqEvent]) -> anyhow::Result<Vec<StagedEvent>> {
    events.iter().map(StagedEvent::prepare).collect()
}

fn seq(value: i64) -> Seq {
    Seq(u64::try_from(value).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::agents::AgentEvent;

    /// A migrated in-memory database, for the two statements that are worth asserting directly.
    fn database() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap_or_else(|error| panic!("{error}"));
        super::super::migrations::run(&mut conn, None).unwrap_or_else(|error| panic!("{error}"));
        conn
    }

    fn ownership(owner: Option<&str>, head: u64) -> ThreadOwnership {
        ThreadOwnership {
            owner: owner.map(|host| host.parse().unwrap_or_else(|error| panic!("{error}"))),
            head_seq: Seq(head),
            projected_seq: Seq(head),
            owner_head: Some(Seq(head)),
            oldest_seq: None,
            synced_at: None,
        }
    }

    fn host(value: &str) -> HostId {
        value.parse().unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn only_the_owning_host_may_extend_a_mirrored_sequence() {
        let thread = ThreadId::new();
        let owner = host("dev-box");
        let other = host("laptop");
        let mirrored = ownership(Some("dev-box"), 4);

        assert_eq!(
            admits_append(thread, &owner, Some(&mirrored), Seq(5)),
            Ok(Admission::Append)
        );
        assert_eq!(
            admits_append(thread, &other, Some(&mirrored), Seq(5)),
            Err(MirrorRefusal::Foreign {
                thread,
                owner,
                writer: other.clone(),
            })
        );
        assert_eq!(
            admits_append(thread, &other, Some(&ownership(None, 4)), Seq(5)),
            Err(MirrorRefusal::Local {
                thread,
                host: other.clone(),
            })
        );
        assert_eq!(
            admits_append(thread, &other, None, Seq(1)),
            Err(MirrorRefusal::Unknown {
                thread,
                host: other,
            })
        );
    }

    #[test]
    fn a_replayed_event_is_a_duplicate_and_a_skipped_one_is_a_gap() {
        let thread = ThreadId::new();
        let owner = host("dev-box");
        let mirrored = ownership(Some("dev-box"), 4);

        // Catch-up and live delivery overlap by design, so a sequence we hold is dropped.
        assert_eq!(
            admits_append(thread, &owner, Some(&mirrored), Seq(4)),
            Ok(Admission::Duplicate)
        );
        assert_eq!(
            admits_append(thread, &owner, Some(&mirrored), Seq(1)),
            Ok(Admission::Duplicate)
        );
        // A hole would make every later read a lie, so it is refused and refilled instead.
        assert_eq!(
            admits_append(thread, &owner, Some(&mirrored), Seq(6)),
            Err(MirrorRefusal::Gap {
                thread,
                host: owner,
                head: Seq(4),
                seq: Seq(6),
            })
        );
    }

    #[test]
    fn the_ladder_measures_the_tail_without_reading_a_payload() {
        let mut conn = database();
        let thread = ThreadId::new();
        let owner = host("dev-box");
        let summary = fleet_core::agents::ThreadProjection::new(
            thread,
            fleet_core::ids::WorktreeId::try_from("acme/api#feature")
                .unwrap_or_else(|error| panic!("{error}")),
            fleet_core::agents::AgentKind::Claude,
        )
        .summary(Seq::default());
        let transaction = conn.transaction().unwrap_or_else(|error| panic!("{error}"));
        claim(&transaction, &owner, &summary).unwrap_or_else(|error| panic!("{error}"));
        for seq in 1..=3_u64 {
            let event = SeqEvent {
                seq: Seq(seq),
                at: Utc::now(),
                raw: None,
                event: AgentEvent::Notice("x".repeat(10)),
            };
            let staged = StagedEvent::prepare(&event).unwrap_or_else(|error| panic!("{error}"));
            assert!(
                append_from_owner(&transaction, &owner, thread, &staged)
                    .unwrap_or_else(|error| panic!("{error}"))
            );
        }
        transaction
            .commit()
            .unwrap_or_else(|error| panic!("{error}"));

        let (events, bytes) =
            resume_admission(&conn, thread, Seq(1)).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(events, 2);
        assert!(bytes > 0, "the byte budget is summed, not guessed");
        // A mirrored prefix carries the owner's own sequences and nothing else.
        let ownership = read_ownership(&conn, thread)
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("the mirrored row exists"));
        assert_eq!(ownership.head_seq, Seq(3));
        assert_eq!(ownership.projected_seq, Seq(3));
        assert_eq!(ownership.owner, Some(owner));
    }

    #[test]
    fn a_locally_owned_log_is_never_discarded_as_a_cache() {
        let mut conn = database();
        let thread = ThreadId::new();
        conn.execute(
            "INSERT INTO threads (thread_id, worktree_id, provider, title, created_at, \
             last_activity_at, session_state, attention) \
             VALUES (?1, 'acme/api#feature', 'claude', 'local', 0, 0, 'ready', '\"idle\"')",
            params![thread.to_string()],
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let transaction = conn.transaction().unwrap_or_else(|error| panic!("{error}"));

        let refused = discard(&transaction, thread);

        assert!(
            refused.is_err(),
            "the mirror may throw its own cache away and nothing else"
        );
    }

    #[test]
    fn a_cold_header_is_mirrored_but_not_warm() {
        let cold = ThreadOwnership {
            head_seq: Seq(0),
            projected_seq: Seq(0),
            ..ownership(Some("dev-box"), 0)
        };
        assert!(cold.is_mirrored());
        assert!(!cold.is_warm());
        assert!(ownership(Some("dev-box"), 3).is_warm());
        assert!(!ownership(None, 3).is_mirrored());
    }

    #[test]
    fn pending_events_measure_how_far_the_owner_is_ahead() {
        let mut behind = ownership(Some("dev-box"), 10);
        behind.owner_head = Some(Seq(42));
        assert_eq!(behind.pending_events(), 32);

        let mut ahead = ownership(Some("dev-box"), 10);
        ahead.owner_head = Some(Seq(4));
        assert_eq!(ahead.pending_events(), 0);

        let mut unknown = ownership(Some("dev-box"), 10);
        unknown.owner_head = None;
        assert_eq!(unknown.pending_events(), 0);
    }
}
