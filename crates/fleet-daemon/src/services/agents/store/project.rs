//! The projector: one event in, the log row and every read model row out, in one transaction.
//!
//! There is exactly one projector, `agents.v1`, and its cursor is `threads.projected_seq`. It runs
//! **synchronously with the append, inside the same transaction**, which is the decision the whole
//! read path is built on (`docs/NATIVE-AGENTS.md` §8,
//! `docs/decisions/0013-sqlite-agent-transcripts.md`): by the time [`append_event`] plus
//! [`project_event`] plus [`advance_head`] have committed, every read model already reflects the
//! event, so a client that reacts to a broadcast by opening the thread cannot observe a transcript
//! older than the event that woke it. An eventually-consistent projector would make that a race
//! with no bound and no signal.
//!
//! The cost is that a projection bug fails a user command. That is the right trade: the reducer's
//! `accepts()` has already refused an inconsistent event *before* the write, so a projection that
//! refuses a sequenced event is a daemon bug, and failing at the moment it happens beats a read
//! model that drifts.
//!
//! **Fidelity.** These statements mirror `fleet_core::agents::ThreadProjection`, which is the
//! authoritative reducer and the one the app holds. Where the two could drift they are pinned by
//! `tests.rs`, which replays the same log through both and compares the counters the thread list
//! paints from. Two things the reducer keeps and the initial schema deliberately does not: the
//! cumulative usage/cost/context observations of `TokenUsage`, which belong to a turn footer and
//! are re-derived from `turns.usage_json`, and `Notice` rows, which ride along on the read path by
//! sequence range rather than as projected rows.

use anyhow::{Context, bail};
use chrono::Utc;
use fleet_core::agents::{
    AgentEvent, Attention, ItemKind, SeqEvent, SessionState, StreamKind, ThreadId, TurnId,
    TurnOutcome, TurnState,
};
use rusqlite::{Transaction, params};
use serde_json::{Value, json};

mod attention;
mod codec;
mod items;

use attention::{changes_attention, clear_retrying, recompute_attention};
pub(super) use codec::{
    OptionalRow, StagedEvent, decode_event, discriminant, json_text, ms, optional_json,
};
use codec::{execute, seq_of};
use items::{append_output, append_text, close_open_items, is_terminal, item_detail};

/// Appends one event to the log.
///
/// A conflict on `(thread_id, seq)` aborts the transaction, which is the point: safety comes from
/// the single writer thread, and the unique index is the tripwire that turns a hypothetical second
/// writer into a refused write instead of silent corruption.
pub(super) fn append_event(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    staged: &StagedEvent,
) -> anyhow::Result<()> {
    let event = &staged.event;
    let bytes = i64::try_from(staged.payload.len()).unwrap_or(i64::MAX);
    transaction
        .execute(
            "INSERT INTO agent_events (thread_id, seq, at, kind, raw, payload, bytes) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                thread.to_string(),
                seq_of(event),
                ms(event.at),
                staged.kind,
                event.raw.as_deref(),
                staged.payload,
                bytes,
            ],
        )
        .with_context(|| format!("append agent event {} of thread {thread}", event.seq))?;
    Ok(())
}

/// Applies one event to every read model it can change.
pub(super) fn project_event(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    event: &SeqEvent,
) -> anyhow::Result<()> {
    ensure_thread_row(transaction, thread, event)?;
    let at = ms(event.at);
    let seq = seq_of(event);
    let id = thread.to_string();

    match &event.event {
        AgentEvent::SessionStarted {
            provider,
            resume_cursor,
            model,
            mode,
            tools,
            commands,
            skills,
        } => {
            let provider = discriminant(provider, "AgentKind")?;
            execute(
                transaction,
                "UPDATE threads SET provider = ?2, session_state = 'ready', exit_code = NULL, \
                 retrying_json = NULL WHERE thread_id = ?1",
                params![id, provider],
                "record a started session on its thread",
            )?;
            upsert_session(
                transaction,
                thread,
                &provider,
                resume_cursor.as_deref(),
                &optional_json(model.as_ref())?,
                &discriminant(mode, "PermissionMode")?,
                "ready",
                Some(&json_text(tools)?),
                Some(&json_text(commands)?),
                Some(&json_text(skills)?),
            )?;
        }
        AgentEvent::MetadataChanged { title, mode, model } => {
            if let Some(title) = title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                execute(
                    transaction,
                    "UPDATE threads SET title = ?2 WHERE thread_id = ?1",
                    params![id, title],
                    "record a retitled thread",
                )?;
            }
            if let Some(mode) = mode {
                execute(
                    transaction,
                    "UPDATE sessions SET mode = ?2 WHERE thread_id = ?1",
                    params![id, discriminant(mode, "PermissionMode")?],
                    "record a changed permission mode",
                )?;
            }
            if let Some(model) = model {
                execute(
                    transaction,
                    "UPDATE sessions SET model_json = ?2 WHERE thread_id = ?1",
                    params![id, json_text(model)?],
                    "record a changed model selection",
                )?;
            }
        }
        AgentEvent::SessionStateChanged(state) => {
            let state = discriminant(state, "SessionState")?;
            execute(
                transaction,
                "UPDATE threads SET session_state = ?2, retrying_json = NULL WHERE thread_id = ?1",
                params![id, state],
                "record a session state change",
            )?;
            execute(
                transaction,
                "UPDATE sessions SET state = ?2 WHERE thread_id = ?1",
                params![id, state],
                "record a session state change on its session",
            )?;
        }
        AgentEvent::SessionExited { code, expected } => {
            let state = if *expected {
                discriminant(&SessionState::Stopped, "SessionState")?
            } else {
                discriminant(&SessionState::Error, "SessionState")?
            };
            execute(
                transaction,
                "UPDATE threads SET session_state = ?2, exit_code = ?3, retrying_json = NULL \
                 WHERE thread_id = ?1",
                params![id, state, code],
                "record a provider exit",
            )?;
            execute(
                transaction,
                "UPDATE sessions SET state = ?2 WHERE thread_id = ?1",
                params![id, state],
                "record a provider exit on its session",
            )?;
            if !expected {
                fail_active_turn(transaction, thread, "provider exited unexpectedly", seq, at)?;
            }
        }
        AgentEvent::TurnStarted { turn, user_item } => {
            execute(
                transaction,
                "INSERT INTO turns (thread_id, turn_id, user_item_id, start_seq, state, started_at) \
                 VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
                params![id, turn.to_string(), user_item.to_string(), seq, at],
                "open a turn",
            )?;
            execute(
                transaction,
                "UPDATE threads SET running_turn_id = ?2, turn_json = ?3, \
                 retrying_json = NULL WHERE thread_id = ?1",
                params![id, turn.to_string(), json_text(&TurnState::Running(*turn))?],
                "record the running turn",
            )?;
        }
        AgentEvent::TurnCompleted {
            turn,
            outcome,
            usage,
            duration_ms,
            files_changed,
        } => {
            close_open_items(transaction, thread, *turn, seq, at)?;
            // §3.3: an authoritative error result is a failed turn, not a completed one. The
            // outcome stays on the row either way, because that is what the footer reads.
            let (state, settled) = if matches!(outcome, TurnOutcome::Error { .. }) {
                ("failed", TurnState::Failed(*turn))
            } else {
                ("completed", TurnState::Completed(*turn, outcome.clone()))
            };
            let outcome_json = json_text(outcome)?;
            execute(
                transaction,
                "UPDATE turns SET end_seq = ?3, state = ?4, outcome = ?5, completed_at = ?6, \
                 duration_ms = ?7, usage_json = ?8, files_changed_json = ?9 \
                 WHERE thread_id = ?1 AND turn_id = ?2",
                params![
                    id,
                    turn.to_string(),
                    seq,
                    state,
                    outcome_json,
                    at,
                    i64::try_from(*duration_ms).unwrap_or(i64::MAX),
                    json_text(usage)?,
                    json_text(files_changed)?,
                ],
                "settle a completed turn",
            )?;
            execute(
                transaction,
                "UPDATE threads SET running_turn_id = NULL, last_outcome = ?2, \
                 last_completed_seq = ?3, turn_json = ?4, retrying_json = NULL \
                 WHERE thread_id = ?1",
                params![id, outcome_json, seq, json_text(&settled)?],
                "record a completed turn on its thread",
            )?;
        }
        AgentEvent::TurnAborted { turn, reason: _ } => {
            close_open_items(transaction, thread, *turn, seq, at)?;
            let outcome_json = json_text(&TurnOutcome::Interrupted)?;
            execute(
                transaction,
                "UPDATE turns SET end_seq = ?3, state = 'aborted', outcome = ?4, \
                 completed_at = ?5, duration_ms = MAX(0, ?5 - started_at) \
                 WHERE thread_id = ?1 AND turn_id = ?2",
                params![id, turn.to_string(), seq, outcome_json, at],
                "settle an aborted turn",
            )?;
            execute(
                transaction,
                "UPDATE threads SET running_turn_id = NULL, last_outcome = ?2, \
                 turn_json = ?3, retrying_json = NULL WHERE thread_id = ?1",
                params![id, outcome_json, json_text(&TurnState::Interrupted(*turn))?],
                "record an aborted turn on its thread",
            )?;
        }
        AgentEvent::ItemStarted {
            turn,
            item,
            kind,
            parent,
        } => {
            let (tool_kind, tool_name) = match kind {
                ItemKind::Tool {
                    kind: tool, name, ..
                } => (Some(discriminant(tool, "ToolKind")?), Some(name.clone())),
                _ => (None, None),
            };
            execute(
                transaction,
                "INSERT INTO items (thread_id, item_id, turn_id, parent_id, kind, tool_kind, \
                 tool_name, status, start_seq, created_at, updated_at, detail_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9, ?9, ?10) \
                 ON CONFLICT (thread_id, item_id) DO UPDATE SET \
                 turn_id = excluded.turn_id, parent_id = excluded.parent_id, \
                 kind = excluded.kind, tool_kind = excluded.tool_kind, \
                 tool_name = excluded.tool_name, status = 'running', end_seq = NULL, \
                 updated_at = excluded.updated_at, detail_json = excluded.detail_json",
                params![
                    id,
                    item.to_string(),
                    turn.to_string(),
                    parent.map(|parent| parent.to_string()),
                    discriminant(kind, "ItemKind")?,
                    tool_kind,
                    tool_name,
                    seq,
                    at,
                    json_text(&json!({ "kind": serde_json::to_value(kind)? }))?,
                ],
                "open a transcript item",
            )?;
            if matches!(kind, ItemKind::UserMessage { .. }) {
                execute(
                    transaction,
                    "UPDATE threads SET last_user_msg_seq = ?2, retrying_json = NULL \
                     WHERE thread_id = ?1",
                    params![id, seq],
                    "record the newest user message",
                )?;
            } else {
                clear_retrying(transaction, thread)?;
            }
        }
        AgentEvent::ContentDelta {
            item,
            stream,
            delta,
        } => {
            match stream {
                StreamKind::AssistantText => {
                    append_text(transaction, thread, &item.to_string(), "text", delta, at)?;
                }
                StreamKind::Reasoning => {
                    append_text(
                        transaction,
                        thread,
                        &item.to_string(),
                        "reasoning",
                        delta,
                        at,
                    )?;
                }
                StreamKind::ToolOutput => {
                    append_output(transaction, thread, &item.to_string(), delta, at)?;
                }
            }
            clear_retrying(transaction, thread)?;
        }
        AgentEvent::ItemUpdated { item, patch } => {
            let mut detail = item_detail(transaction, thread, *item)?;
            for (key, value) in [
                ("input", patch.input.clone()),
                ("summary", patch.summary.clone().map(Value::String)),
                ("result", patch.result.clone().map(Value::String)),
                (
                    "diff",
                    patch.diff.as_ref().map(serde_json::to_value).transpose()?,
                ),
            ] {
                if let Some(value) = value {
                    detail.insert(key.to_owned(), value);
                }
            }
            let status = patch
                .status
                .map(|status| discriminant(&status, "ItemStatus"));
            let status = status.transpose()?;
            let terminal = patch.status.is_some_and(is_terminal);
            execute(
                transaction,
                "UPDATE items SET \
                 text = CASE WHEN ?3 IS NULL THEN text ELSE ?3 END, \
                 output = CASE WHEN ?4 IS NULL THEN output ELSE ?4 END, \
                 output_bytes = CASE WHEN ?4 IS NULL THEN output_bytes ELSE LENGTH(CAST(?4 AS BLOB)) END, \
                 output_elided = CASE WHEN ?4 IS NULL THEN output_elided ELSE 0 END, \
                 status = COALESCE(?5, status), \
                 end_seq = CASE WHEN ?6 = 1 THEN ?7 ELSE end_seq END, \
                 detail_json = ?8, updated_at = ?9 \
                 WHERE thread_id = ?1 AND item_id = ?2",
                params![
                    id,
                    item.to_string(),
                    patch.text.as_deref(),
                    patch.output.as_deref(),
                    status,
                    i64::from(terminal),
                    seq,
                    json_text(&Value::Object(detail))?,
                    at,
                ],
                "patch a transcript item",
            )?;
            clear_retrying(transaction, thread)?;
        }
        AgentEvent::ItemCompleted { item, status } => {
            let terminal = is_terminal(*status);
            execute(
                transaction,
                "UPDATE items SET status = ?3, \
                 end_seq = CASE WHEN ?4 = 1 THEN ?5 ELSE end_seq END, updated_at = ?6 \
                 WHERE thread_id = ?1 AND item_id = ?2",
                params![
                    id,
                    item.to_string(),
                    discriminant(status, "ItemStatus")?,
                    i64::from(terminal),
                    seq,
                    at,
                ],
                "settle a transcript item",
            )?;
            clear_retrying(transaction, thread)?;
        }
        AgentEvent::GateOpened { gate, turn, kind } => {
            execute(
                transaction,
                "INSERT INTO gates (thread_id, gate_id, turn_id, kind, kind_json, status, \
                 opened_seq, opened_at, blocked_since) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?7, ?7) \
                 ON CONFLICT (thread_id, gate_id) DO UPDATE SET \
                 turn_id = excluded.turn_id, kind = excluded.kind, kind_json = excluded.kind_json, \
                 status = 'open', answer_json = NULL, resolved_by = NULL, resolved_seq = NULL, \
                 resolved_at = NULL, opened_seq = excluded.opened_seq, \
                 opened_at = excluded.opened_at, blocked_since = excluded.blocked_since",
                params![
                    id,
                    gate.to_string(),
                    turn.map(|turn| turn.to_string()),
                    discriminant(kind, "GateKind")?,
                    json_text(kind)?,
                    seq,
                    at,
                ],
                "open a gate",
            )?;
        }
        AgentEvent::GateResolved { gate, answer, by } => {
            resolve_gate(transaction, thread, gate, answer, by, seq, at)?;
        }
        AgentEvent::TokenUsage {
            turn,
            usage,
            context_pct: _,
            cost_usd: _,
        } => {
            // §5: turn metadata is withheld until the turn completes, so a late observation on a
            // settled turn must not move the footer the user has already read.
            execute(
                transaction,
                "UPDATE turns SET usage_json = ?3 \
                 WHERE thread_id = ?1 AND turn_id = ?2 AND end_seq IS NULL",
                params![id, turn.to_string(), json_text(usage)?],
                "record token usage on a running turn",
            )?;
            clear_retrying(transaction, thread)?;
        }
        AgentEvent::Checkpoint(kind) => {
            // A checkpoint's identity is its turn, so a second boundary inside one turn replaces
            // the first: the row records that the turn crossed a boundary, and the log keeps both.
            // A boundary before the first turn is keyed on the empty turn id, of which there can
            // only ever be one window.
            execute(
                transaction,
                "INSERT INTO checkpoints (thread_id, turn_id, ordinal, kind, seq, at) \
                 SELECT ?1, \
                        COALESCE((SELECT turn_id FROM turns WHERE thread_id = ?1 \
                                   ORDER BY start_seq DESC LIMIT 1), ''), \
                        (SELECT COUNT(*) + 1 FROM checkpoints WHERE thread_id = ?1), ?2, ?3, ?4 \
                 ON CONFLICT (thread_id, turn_id) DO UPDATE SET \
                 kind = excluded.kind, seq = excluded.seq, at = excluded.at",
                params![id, discriminant(kind, "CheckpointKind")?, seq, at],
                "record a checkpoint",
            )?;
        }
        AgentEvent::Retrying {
            attempt,
            retry_in_ms,
            reason,
        } => {
            execute(
                transaction,
                "UPDATE threads SET retrying_json = ?2 WHERE thread_id = ?1",
                params![
                    id,
                    json_text(&json!({
                        "attempt": attempt,
                        "retryInMs": retry_in_ms,
                        "reason": reason,
                    }))?
                ],
                "record a provider retry",
            )?;
        }
        AgentEvent::RuntimeError { fatal, message } => {
            execute(
                transaction,
                "UPDATE sessions SET last_error = ?2 WHERE thread_id = ?1",
                params![id, message],
                "record a runtime error on its session",
            )?;
            if *fatal {
                let state = discriminant(&SessionState::Error, "SessionState")?;
                execute(
                    transaction,
                    "UPDATE threads SET session_state = ?2, retrying_json = NULL \
                     WHERE thread_id = ?1",
                    params![id, state],
                    "record a fatal runtime error",
                )?;
                execute(
                    transaction,
                    "UPDATE sessions SET state = ?2 WHERE thread_id = ?1",
                    params![id, state],
                    "record a fatal runtime error on its session",
                )?;
                fail_active_turn(transaction, thread, message, seq, at)?;
            }
        }
        // A notice is a transcript row the read path picks up from the log by sequence range,
        // exactly as imported history is, so it projects to nothing of its own.
        AgentEvent::Notice(_) => {}
    }

    if changes_attention(&event.event) {
        recompute_attention(transaction, thread)?;
    }
    Ok(())
}

/// Advances the log head and the projector cursor together, in the append's transaction.
///
/// They move as one statement because `head_seq == projected_seq` is the healthy state and the
/// only thing that makes the "who is behind?" boot query meaningful: a head that could outrun the
/// cursor inside a committed transaction would make every thread look mid-rebuild.
pub(super) fn advance_head(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    event: &SeqEvent,
) -> anyhow::Result<()> {
    execute(
        transaction,
        "UPDATE threads SET head_seq = ?2, projected_seq = ?2, last_activity_at = ?3, \
         last_nonterminal_seq = CASE WHEN ?4 = 1 THEN ?2 ELSE last_nonterminal_seq END \
         WHERE thread_id = ?1",
        params![
            thread.to_string(),
            seq_of(event),
            ms(event.at),
            i64::from(is_nonterminal(&event.event)),
        ],
        "advance a thread's log head",
    )
}

/// Moves every event after `last` out of the log and rebuilds the thread's read model.
///
/// This is the reducer's half of the old store's quarantine, and it keeps that discipline: an
/// event that decodes and is in sequence but that the projection refuses on replay must leave the
/// log — otherwise the next append, written at the sequence the replay actually reached, collides
/// with it — and it must not be *lost*, because it is the only evidence of why the replay stopped.
/// `None` empties the thread's log.
///
/// The rebuild is in the same transaction: a read model that still carried the quarantined events
/// would be a read model of a log that no longer exists.
pub(super) fn quarantine_after(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    last: Option<fleet_core::agents::Seq>,
    reason: &str,
) -> anyhow::Result<usize> {
    let id = thread.to_string();
    let boundary = last.map_or(0, |last| i64::try_from(last.0).unwrap_or(i64::MAX));
    let moved = transaction
        .execute(
            "INSERT INTO agent_events_quarantine \
             (thread_id, seq, at, kind, raw, payload, bytes, quarantined_at, reason) \
             SELECT thread_id, seq, at, kind, raw, payload, bytes, ?3, ?4 FROM agent_events \
             WHERE thread_id = ?1 AND seq > ?2",
            params![id, boundary, Utc::now().timestamp_millis(), reason],
        )
        .with_context(|| format!("quarantine the tail of thread {thread}"))?;
    if moved == 0 {
        return Ok(0);
    }
    transaction
        .execute(
            "DELETE FROM agent_events WHERE thread_id = ?1 AND seq > ?2",
            params![id, boundary],
        )
        .with_context(|| format!("drop the quarantined tail of thread {thread}"))?;
    rebuild_thread(transaction, thread)?;
    tracing::warn!(
        thread = %thread,
        quarantined = moved,
        reason,
        "moved the unprojectable tail of a native-agent log to the quarantine table"
    );
    Ok(moved)
}

/// Replays one thread's log from the beginning, rewriting every projection row.
///
/// Boot repair and an explicit rebuild are the same code path. Two properties make it safe to run
/// on a live thread: it is idempotent, because every upsert is keyed on a content-derived id; and
/// it rewrites only the projector-owned columns of `sessions`, leaving `started_at`,
/// `last_seen_at` and `restart_count` byte-identical — a rebuild that cleared those would make
/// restart recovery believe a running provider had never started.
pub(super) fn rebuild_thread(
    transaction: &Transaction<'_>,
    thread: ThreadId,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    for statement in [
        "DELETE FROM turns WHERE thread_id = ?1",
        "DELETE FROM items WHERE thread_id = ?1",
        "DELETE FROM gates WHERE thread_id = ?1",
        "DELETE FROM checkpoints WHERE thread_id = ?1",
    ] {
        transaction
            .execute(statement, params![id])
            .with_context(|| format!("clear the read model of thread {thread}"))?;
    }
    execute(
        transaction,
        "UPDATE threads SET head_seq = 0, projected_seq = 0, session_state = 'starting', \
         running_turn_id = NULL, last_outcome = NULL, open_gate_count = 0, \
         last_user_msg_seq = NULL, last_completed_seq = NULL, last_nonterminal_seq = NULL, \
         exit_code = NULL, retrying_json = NULL, attention = ?2, turn_json = ?3 \
         WHERE thread_id = ?1",
        params![
            id,
            json_text(&Attention::Idle)?,
            json_text(&TurnState::None)?
        ],
        "reset a thread's projected state",
    )?;

    // Bounded, resumable and constant-memory: one thread is the unit of replay and one chunk is
    // the unit of work, so a rebuild of a 4 000-turn transcript never materialises it.
    const REPLAY_CHUNK: i64 = 512;
    let mut after = 0_i64;
    loop {
        let mut statement = transaction
            .prepare_cached(
                "SELECT seq, at, raw, payload FROM agent_events \
                 WHERE thread_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            )
            .context("prepare the projection replay query")?;
        let chunk = statement
            .query_map(params![id, after, REPLAY_CHUNK], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .context("query the projection replay chunk")?
            .collect::<Result<Vec<_>, _>>()
            .context("decode the projection replay chunk")?;
        if chunk.is_empty() {
            return Ok(());
        }
        for (seq, at, raw, payload) in chunk {
            let event = decode_event(seq, at, raw, &payload)
                .with_context(|| format!("decode event {seq} of thread {thread} for a rebuild"))?;
            project_event(transaction, thread, &event)?;
            advance_head(transaction, thread, &event)?;
            after = seq;
        }
    }
}

/// Creates the thread row an append needs, without touching one that already exists.
///
/// An append for a thread the index has not registered yet still lands: the log is the truth, and
/// `write_index` fills the metadata in afterwards. The placeholder is honest about what it does
/// not know rather than guessing a worktree.
fn ensure_thread_row(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    event: &SeqEvent,
) -> anyhow::Result<()> {
    let at = ms(event.at);
    transaction
        .execute(
            "INSERT OR IGNORE INTO threads \
             (thread_id, worktree_id, provider, title, created_at, last_activity_at, \
              session_state, attention) \
             VALUES (?1, '', '', '', ?2, ?2, 'starting', ?3)",
            params![thread.to_string(), at, json_text(&Attention::Idle)?],
        )
        .with_context(|| format!("register thread {thread} in the agent database"))?;
    Ok(())
}

/// Upserts the projector-owned half of a session row, leaving the daemon-owned half alone.
#[allow(clippy::too_many_arguments)]
fn upsert_session(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    provider: &str,
    resume_cursor: Option<&str>,
    model_json: &Option<String>,
    mode: &str,
    state: &str,
    tools_json: Option<&str>,
    commands_json: Option<&str>,
    skills_json: Option<&str>,
) -> anyhow::Result<()> {
    execute(
        transaction,
        "INSERT INTO sessions (thread_id, provider, resume_cursor, model_json, mode, state, \
         tools_json, commands_json, skills_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT (thread_id) DO UPDATE SET \
         provider = excluded.provider, resume_cursor = excluded.resume_cursor, \
         model_json = excluded.model_json, mode = excluded.mode, state = excluded.state, \
         last_error = NULL, tools_json = excluded.tools_json, \
         commands_json = excluded.commands_json, skills_json = excluded.skills_json",
        params![
            thread.to_string(),
            provider,
            resume_cursor,
            model_json.as_deref(),
            mode,
            state,
            tools_json,
            commands_json,
            skills_json,
        ],
        "record a session",
    )
}

/// Marks one thread's session failed, with the reason, and touches nothing else.
///
/// This is the answer to a replay that cannot be repaired, and it is deliberately *not* an
/// appended event: there is no `AgentEvent` for "this daemon could not read your transcript", and
/// inventing one would put a lie in the log. The thread stays browsable read-only up to
/// `projected_seq`, the failure is visible in the list and in `sessions.last_error`, and every
/// other thread is unaffected — which is what lets the daemon start anyway.
pub(super) fn fail_session(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    message: &str,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    let state = discriminant(&SessionState::Error, "SessionState")?;
    execute(
        transaction,
        "UPDATE threads SET session_state = ?2, attention = ?3 WHERE thread_id = ?1",
        params![id, state, json_text(&Attention::Failed)?],
        "record an unrepairable thread",
    )?;
    execute(
        transaction,
        "UPDATE sessions SET state = ?2, last_error = ?3 WHERE thread_id = ?1",
        params![id, state, message],
        "record an unrepairable session",
    )
}

/// Fails the running turn, if there is one, as the reducer does on an unexpected exit or a fatal
/// runtime error.
fn fail_active_turn(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    message: &str,
    seq: i64,
    at: i64,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    let running: Option<String> = transaction
        .query_row(
            "SELECT running_turn_id FROM threads WHERE thread_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read the running turn of thread {thread}"))?
        .flatten();
    let Some(running) = running else {
        return Ok(());
    };
    let Ok(turn) = running.parse::<TurnId>() else {
        bail!("thread {thread} records an unparsable running turn `{running}`");
    };
    close_open_items(transaction, thread, turn, seq, at)?;
    let outcome_json = json_text(&TurnOutcome::Error {
        message: Some(message.to_owned()),
    })?;
    execute(
        transaction,
        "UPDATE turns SET end_seq = ?3, state = 'failed', outcome = ?4, completed_at = ?5, \
         duration_ms = MAX(0, ?5 - started_at) \
         WHERE thread_id = ?1 AND turn_id = ?2 AND end_seq IS NULL",
        params![id, running, seq, outcome_json, at],
        "fail the running turn",
    )?;
    execute(
        transaction,
        "UPDATE threads SET running_turn_id = NULL, last_outcome = ?2, turn_json = ?3 \
         WHERE thread_id = ?1",
        params![id, outcome_json, json_text(&TurnState::Failed(turn))?],
        "clear the failed running turn",
    )
}

/// Closes a gate and charges the wait it ends to the turn that was blocked on it.
fn resolve_gate(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    gate: &fleet_core::agents::GateId,
    answer: &fleet_core::agents::GateAnswer,
    by: &fleet_core::agents::GateResolver,
    seq: i64,
    at: i64,
) -> anyhow::Result<()> {
    let id = thread.to_string();
    // The thread has been blocked since the earliest gate of this window opened, so the window
    // start is read *before* this gate leaves the open set.
    let since: Option<i64> = transaction
        .query_row(
            "SELECT MIN(blocked_since) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read the blocked window of thread {thread}"))?
        .flatten();
    let owner: Option<Option<String>> = transaction
        .query_row(
            "SELECT turn_id FROM gates WHERE thread_id = ?1 AND gate_id = ?2 AND status = 'open'",
            params![id, gate.to_string()],
            |row| row.get(0),
        )
        .optional_row()
        .with_context(|| format!("read gate {gate} of thread {thread}"))?;
    let Some(owner) = owner else {
        bail!("resolution of unknown or already resolved gate {gate} of thread {thread}");
    };
    execute(
        transaction,
        "UPDATE gates SET status = 'resolved', answer_json = ?3, resolved_by = ?4, \
         resolved_seq = ?5, resolved_at = ?6, blocked_since = NULL \
         WHERE thread_id = ?1 AND gate_id = ?2",
        params![
            id,
            gate.to_string(),
            json_text(answer)?,
            discriminant(by, "GateResolver")?,
            seq,
            at,
        ],
        "resolve a gate",
    )?;

    let still_open: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM gates WHERE thread_id = ?1 AND status = 'open'",
            params![id],
            |row| row.get(0),
        )
        .with_context(|| format!("count the open gates of thread {thread}"))?;
    let Some(since) = since else {
        return Ok(());
    };
    if still_open > 0 {
        // Answering the first gate of an overlapping run must not restart the clock for the
        // second, so whatever stays open inherits the window's start.
        execute(
            transaction,
            "UPDATE gates SET blocked_since = MIN(COALESCE(blocked_since, ?2), ?2) \
             WHERE thread_id = ?1 AND status = 'open'",
            params![id, since],
            "carry a blocked window over to the gates still open",
        )?;
        return Ok(());
    }
    // The gate names its own turn when the provider supplied one; otherwise the turn that is
    // still running owns the wait, because that is the footer the user is about to read.
    let blocked = at.saturating_sub(since).max(0);
    execute(
        transaction,
        "UPDATE turns SET gate_blocked_ms = gate_blocked_ms + ?3 \
         WHERE thread_id = ?1 AND end_seq IS NULL AND turn_id = COALESCE( \
             ?2, (SELECT running_turn_id FROM threads WHERE thread_id = ?1))",
        params![id, owner, blocked],
        "charge a blocked window to its turn",
    )
}

/// Whether the event leaves the thread non-terminal, as `ThreadProjection` decides it.
fn is_nonterminal(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::SessionExited { .. }
            | AgentEvent::TurnCompleted { .. }
            | AgentEvent::TurnAborted { .. }
            | AgentEvent::RuntimeError { fatal: true, .. }
    )
}
