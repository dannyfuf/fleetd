use super::{CommandOutput, parse_id, unknown, validation, wait_event};
use crate::{
    args::{WatchListArgs, WatchTailArgs},
    envelope::{PROTOCOL, WatchesEnvelope, to_json},
    human,
};
use fleet_client::Client;
use fleet_core::{
    ids::SessionId,
    watches::{WatchStatus, WatchStream},
};
use fleet_proto::{error::ProtoError, event::Event};
use std::io::Write;

pub(super) fn watch_session(
    explicit: Option<SessionId>,
    environment: Option<&str>,
) -> Result<SessionId, ProtoError> {
    if let Some(session) = explicit {
        return Ok(session);
    }
    let value = environment
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| validation("fleet watch list requires --session <id> or FLEET_SESSION"))?;
    parse_id(value).map_err(|error| validation(format!("invalid FLEET_SESSION: {}", error.message)))
}

pub(super) async fn watch_list(
    client: &Client,
    arguments: WatchListArgs,
) -> Result<CommandOutput, ProtoError> {
    let session = watch_session(arguments.session, None)?;
    let watches = client.list_watches(session).await?;
    let text = if arguments.json {
        to_json(&WatchesEnvelope {
            protocol: PROTOCOL,
            watches: &watches,
        })?
    } else {
        human::watches(&watches)
    };
    Ok(CommandOutput::success(text))
}

pub(super) async fn watch_tail(
    client: &Client,
    arguments: WatchTailArgs,
) -> Result<CommandOutput, ProtoError> {
    watch_tail_to(
        client,
        arguments,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
    .await?;
    Ok(CommandOutput::success(String::new()))
}

pub(super) async fn watch_tail_to(
    client: &Client,
    arguments: WatchTailArgs,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), ProtoError> {
    let mut events = client.events();
    let mut next_seq = None;
    loop {
        let tail = client.tail_watch(arguments.id, next_seq).await?;
        for chunk in tail.chunks {
            write_chunk(chunk, stdout, stderr)?;
        }
        if !arguments.follow || matches!(tail.watch.status, WatchStatus::Exited { .. }) {
            return Ok(());
        }
        let mut cursor = tail.next_seq;
        while let Some(Event::WatchOutput { chunks, .. }) =
            wait_event(&mut events, |event| match event {
                Event::WatchOutput { watch, .. } => *watch == arguments.id,
                Event::WatchExited(watch) => watch.id == arguments.id,
                Event::WatchDismissed(watch) => *watch == arguments.id,
                _ => false,
            })
            .await?
        {
            let mut gap = false;
            for chunk in chunks {
                if chunk.seq < cursor {
                    continue;
                }
                if chunk.seq > cursor {
                    gap = true;
                    break;
                }
                cursor = chunk.seq.saturating_add(1);
                write_chunk(chunk, stdout, stderr)?;
            }
            if gap {
                break;
            }
        }
        // Read retained output on gaps and before accepting completion.
        next_seq = Some(cursor);
    }
}

fn write_chunk(
    chunk: fleet_core::watches::WatchChunk,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<(), ProtoError> {
    let output: &mut dyn Write = match chunk.stream {
        WatchStream::Stdout => stdout,
        WatchStream::Stderr => stderr,
    };
    output
        .write_all(chunk.text.as_bytes())
        .and_then(|()| output.flush())
        .map_err(|error| unknown(format!("could not write watch output: {error}")))
}
