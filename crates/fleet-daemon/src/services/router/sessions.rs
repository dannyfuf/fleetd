//! Remote terminal-frame attachment forwarding.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use fleet_core::ids::{HostId, TerminalId};
use fleet_proto::{event::Event, request::RequestBody};
use tokio_util::sync::CancellationToken;

use crate::{DaemonError, DaemonResult, machines::RemoteEndpoint, server::BroadcastBus};

use super::RemoteIds;

#[derive(Default)]
pub(crate) struct Attached {
    by_host: BTreeMap<HostId, BTreeMap<TerminalId, usize>>,
}

struct Pump {
    cancel: CancellationToken,
}

/// Per-router subscriptions that relay frames only while a local terminal is attached.
#[derive(Default)]
pub(crate) struct RemoteTerminalFrames {
    attached: Arc<Mutex<Attached>>,
    pumps: Mutex<BTreeMap<HostId, Pump>>,
}

impl RemoteTerminalFrames {
    pub(crate) fn shared_attachments(&self) -> Arc<Mutex<Attached>> {
        Arc::clone(&self.attached)
    }

    pub(crate) fn attach(
        &self,
        endpoint: Arc<dyn RemoteEndpoint>,
        host: &HostId,
        local: TerminalId,
        ids: RemoteIds,
        events: BroadcastBus,
    ) -> DaemonResult<()> {
        let Some((owner, _)) = ids.remote_terminal(local) else {
            return Err(DaemonError::NotFound(format!("terminal {local}")));
        };
        if &owner != host {
            return Err(DaemonError::Validation(format!(
                "terminal {local} belongs to host {owner}, not {host}"
            )));
        }

        let mut pumps = lock(&self.pumps);
        let mut attached = lock(&self.attached);
        *attached
            .by_host
            .entry(host.clone())
            .or_default()
            .entry(local)
            .or_default() += 1;
        if pumps.contains_key(host) {
            return Ok(());
        }

        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            remove_attachment(&mut attached, host, local);
            DaemonError::Process(format!("remote terminal frame pump: {error}"))
        })?;
        let mut remote_events = endpoint.events();
        let cancel = CancellationToken::new();
        pumps.insert(
            host.clone(),
            Pump {
                cancel: cancel.clone(),
            },
        );
        let attached = Arc::clone(&self.attached);
        let host = host.clone();
        runtime.spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    event = remote_events.recv() => match event {
                        Ok(Event::TerminalFrame(mut frame)) => {
                            let Some(local) = ids.existing_local_terminal(&host, frame.terminal) else {
                                tracing::debug!(
                                    %host,
                                    terminal = %frame.terminal,
                                    "dropping frame for unknown remote terminal"
                                );
                                continue;
                            };
                            if !is_attached(&attached, &host, local) {
                                continue;
                            }
                            frame.terminal = local;
                            events.publish(Event::TerminalFrame(frame));
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(%host, skipped, "remote terminal frame pump lagged");
                            request_full_frames(&endpoint, &ids, &attached, &host).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        Ok(())
    }

    pub(crate) fn detach(&self, host: &HostId, local: TerminalId) {
        let empty = remove_attachment(&mut lock(&self.attached), host, local);
        if empty && let Some(pump) = lock(&self.pumps).remove(host) {
            pump.cancel.cancel();
        }
    }
}

pub(crate) fn attachments_from_shared(
    attached: &Arc<Mutex<Attached>>,
    host: &HostId,
) -> Vec<(TerminalId, usize)> {
    lock(attached)
        .by_host
        .get(host)
        .into_iter()
        .flat_map(|terminals| terminals.iter())
        .map(|(terminal, count)| (*terminal, *count))
        .collect()
}

pub(crate) fn on_attach(
    subscriptions: &RemoteTerminalFrames,
    endpoint: Arc<dyn RemoteEndpoint>,
    host: &HostId,
    local: TerminalId,
    ids: RemoteIds,
    events: BroadcastBus,
) -> DaemonResult<()> {
    subscriptions.attach(endpoint, host, local, ids, events)
}

pub(crate) fn on_detach(subscriptions: &RemoteTerminalFrames, host: &HostId, local: TerminalId) {
    subscriptions.detach(host, local);
}

async fn request_full_frames(
    endpoint: &Arc<dyn RemoteEndpoint>,
    ids: &RemoteIds,
    attached: &Arc<Mutex<Attached>>,
    host: &HostId,
) {
    let terminals = lock(attached)
        .by_host
        .get(host)
        .into_iter()
        .flat_map(|terminals| terminals.keys())
        .filter_map(|local| ids.remote_terminal(*local))
        .filter_map(|(owner, remote)| (owner == *host).then_some(remote))
        .collect::<Vec<_>>();
    for terminal in terminals {
        if let Err(error) = endpoint
            .request(RequestBody::RequestFullFrame { terminal })
            .await
        {
            tracing::debug!(%host, %terminal, %error, "failed to request remote full frame");
        }
    }
}

fn is_attached(attached: &Arc<Mutex<Attached>>, host: &HostId, terminal: TerminalId) -> bool {
    lock(attached)
        .by_host
        .get(host)
        .and_then(|terminals| terminals.get(&terminal))
        .is_some_and(|count| *count > 0)
}

/// Returns whether the host has no remaining terminal attachments.
fn remove_attachment(attached: &mut Attached, host: &HostId, terminal: TerminalId) -> bool {
    let Some(terminals) = attached.by_host.get_mut(host) else {
        return true;
    };
    if let Some(count) = terminals.get_mut(&terminal) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            terminals.remove(&terminal);
        }
    }
    let empty = terminals.is_empty();
    if empty {
        attached.by_host.remove(host);
    }
    empty
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fleet_proto::terminal::{
        CursorShape, CursorState, FrameUpdate, TerminalModes, ViewportInfo,
    };

    use super::*;
    use crate::testing::FakeRemote;

    #[tokio::test]
    async fn attach_relays_known_frames_and_detach_unsubscribes() {
        let host = HostId::try_from("devbox").expect("host");
        let remote = Arc::new(FakeRemote::new(host.clone()));
        let endpoint: Arc<dyn RemoteEndpoint> = remote.clone();
        let ids = RemoteIds::default();
        let local = ids.local_terminal(&host, TerminalId(7));
        let subscriptions = RemoteTerminalFrames::default();
        let events = BroadcastBus::default();
        let mut receiver = events.subscribe();

        on_attach(
            &subscriptions,
            endpoint,
            &host,
            local,
            ids.clone(),
            events.clone(),
        )
        .expect("attach");
        remote.emit(Event::TerminalFrame(frame(TerminalId(99), 1)));
        remote.emit(Event::TerminalFrame(frame(TerminalId(7), 2)));

        let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("translated frame")
            .expect("event");
        assert!(matches!(
            event,
            Event::TerminalFrame(FrameUpdate { terminal, seq: 2, .. }) if terminal == local
        ));

        on_detach(&subscriptions, &host, local);
        remote.emit(Event::TerminalFrame(frame(TerminalId(7), 3)));
        match tokio::time::timeout(Duration::from_millis(50), receiver.recv()).await {
            Err(_) => {}
            Ok(Ok(event)) => panic!("received frame after detach: {event:?}"),
            Ok(Err(error)) => panic!("event stream closed after detach: {error}"),
        }
    }

    fn frame(terminal: TerminalId, seq: u64) -> FrameUpdate {
        FrameUpdate {
            terminal,
            seq,
            cols: 80,
            rows: 24,
            full: true,
            shift: None,
            rows_changed: Vec::new(),
            cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len: 0,
                offset: 0,
                history_epoch: 0,
            },
            modes: TerminalModes::default(),
            title: None,
        }
    }
}
