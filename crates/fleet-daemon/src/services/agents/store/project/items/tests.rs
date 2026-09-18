use chrono::{DateTime, Utc};
use fleet_core::agents::{
    AgentEvent, AgentKind, Attention, DelegationId, DelegationStatus, ItemId, ItemKind, ItemPatch,
    ItemPayloadPatch, ItemStatus, Seq, SeqEvent, ThreadId, TurnId, TurnOutcome, Usage,
};
use rusqlite::{Connection, Transaction, params};

use super::super::{StagedEvent, append_event, rebuild_thread};

fn timestamp(seq: u64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(1_700_000_000_000 + i64::try_from(seq).unwrap_or_default())
        .unwrap_or_else(Utc::now)
}

fn event(seq: u64, event: AgentEvent) -> SeqEvent {
    SeqEvent {
        seq: Seq(seq),
        at: timestamp(seq),
        raw: None,
        event,
    }
}

fn append_log(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    event: SeqEvent,
) -> anyhow::Result<()> {
    let staged = StagedEvent::prepare(&event)?;
    append_event(transaction, thread, &staged)
}

fn migrated_database() -> anyhow::Result<Connection> {
    let mut connection = Connection::open_in_memory()?;
    super::super::super::migrations::run(&mut connection, None)?;
    Ok(connection)
}

fn item_status(
    transaction: &Transaction<'_>,
    thread: ThreadId,
    item: ItemId,
) -> anyhow::Result<String> {
    Ok(transaction.query_row(
        "SELECT status FROM items WHERE thread_id = ?1 AND item_id = ?2",
        params![thread.to_string(), item.to_string()],
        |row| row.get(0),
    )?)
}

fn attention(transaction: &Transaction<'_>, thread: ThreadId) -> anyhow::Result<Attention> {
    let encoded: String = transaction.query_row(
        "SELECT attention FROM threads WHERE thread_id = ?1",
        [thread.to_string()],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&encoded)?)
}

#[test]
fn rebuild_preserves_background_work_until_its_terminal_update() -> anyhow::Result<()> {
    let mut connection = migrated_database()?;
    let transaction = connection.transaction()?;
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let subagent = ItemId::new();

    append_log(
        &transaction,
        thread,
        event(
            1,
            AgentEvent::TurnStarted {
                turn,
                user_item: ItemId::new(),
            },
        ),
    )?;
    append_log(
        &transaction,
        thread,
        event(
            2,
            AgentEvent::ItemStarted {
                turn,
                item: subagent,
                kind: ItemKind::Subagent {
                    name: "explore".to_owned(),
                    description: "Find callers".to_owned(),
                    result: None,
                },
                parent: None,
            },
        ),
    )?;
    append_log(
        &transaction,
        thread,
        event(
            3,
            AgentEvent::TurnSettled {
                turn,
                outcome: TurnOutcome::Completed,
                usage: Usage::default(),
                duration_ms: 10,
                files_changed: Vec::new(),
            },
        ),
    )?;

    rebuild_thread(&transaction, thread)?;

    assert_eq!(item_status(&transaction, thread, subagent)?, "in_progress");
    assert_eq!(attention(&transaction, thread)?, Attention::Working);

    append_log(
        &transaction,
        thread,
        event(
            4,
            AgentEvent::ItemUpdated {
                item: subagent,
                patch: ItemPatch {
                    status: Some(ItemStatus::Completed),
                    ..ItemPatch::default()
                },
            },
        ),
    )?;
    rebuild_thread(&transaction, thread)?;

    assert_eq!(item_status(&transaction, thread, subagent)?, "completed");
    assert_ne!(attention(&transaction, thread)?, Attention::Working);
    Ok(())
}

#[test]
fn rebuild_preserves_delegations_without_counting_them_as_background_work() -> anyhow::Result<()> {
    let mut connection = migrated_database()?;
    let transaction = connection.transaction()?;
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let delegation = ItemId::new();

    append_log(
        &transaction,
        thread,
        event(
            1,
            AgentEvent::TurnStarted {
                turn,
                user_item: ItemId::new(),
            },
        ),
    )?;
    append_log(
        &transaction,
        thread,
        event(
            2,
            AgentEvent::ItemStarted {
                turn,
                item: delegation,
                kind: ItemKind::Delegation {
                    id: DelegationId::new(),
                    provider: AgentKind::Codex,
                    child: ThreadId::new(),
                    status: DelegationStatus::Starting,
                },
                parent: None,
            },
        ),
    )?;
    append_log(
        &transaction,
        thread,
        event(
            3,
            AgentEvent::TurnSettled {
                turn,
                outcome: TurnOutcome::Completed,
                usage: Usage::default(),
                duration_ms: 10,
                files_changed: Vec::new(),
            },
        ),
    )?;

    rebuild_thread(&transaction, thread)?;

    assert_eq!(
        item_status(&transaction, thread, delegation)?,
        "in_progress"
    );
    assert_ne!(attention(&transaction, thread)?, Attention::Working);

    append_log(
        &transaction,
        thread,
        event(
            4,
            AgentEvent::ItemUpdated {
                item: delegation,
                patch: ItemPatch {
                    payload: Some(ItemPayloadPatch::Delegation {
                        status: DelegationStatus::Succeeded,
                    }),
                    status: Some(ItemStatus::Completed),
                },
            },
        ),
    )?;
    rebuild_thread(&transaction, thread)?;

    let (status, delegation_status): (String, String) = transaction.query_row(
        "SELECT status, json_extract(detail_json, '$.payload.data.status') FROM items \
         WHERE thread_id = ?1 AND item_id = ?2",
        params![thread.to_string(), delegation.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(status, "completed");
    assert_eq!(delegation_status, "succeeded");
    Ok(())
}
