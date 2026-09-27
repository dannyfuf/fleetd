//! Cancellable app-side driver for chunked, resumable media staging.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use async_channel::{Receiver, Sender};
use fleet_core::ids::HostId;
use fleet_proto::{
    error::ProtoError,
    event::ToastLevel,
    media::{UPLOAD_CONCURRENCY, UPLOAD_IDLE_EXPIRY, UPLOAD_WINDOW},
    request::{MediaAnchor, UploadId},
    response::ResponseBody,
    snapshot::{LinkState, Snapshot},
};
use fleet_ui_kit::{Icon, Toast};
use gpui::{App, AsyncApp, Entity, Task, WeakEntity};
use tokio::sync::Semaphore;

use super::Attachment;
use crate::{
    bridge::{Bridge, MediaStageOp},
    state::{AppState, ToastTarget},
};

use self::{
    driver::{DriveFailure, drive_upload},
    prepare::{PreparedItem, PreparedUpload, prepare_attachment},
};

mod driver;
mod prepare;

type StageReply = Receiver<Result<ResponseBody, ProtoError>>;

const PROGRESS_UPDATE_INTERVAL: Duration = Duration::from_millis(250);
const PROGRESS_TOAST_DWELL: Duration = UPLOAD_IDLE_EXPIRY;

trait MediaTransport: Clone + 'static {
    fn stage_media(&self, anchor: MediaAnchor, upload: UploadId, op: MediaStageOp) -> StageReply;
}

impl MediaTransport for Bridge {
    fn stage_media(&self, anchor: MediaAnchor, upload: UploadId, op: MediaStageOp) -> StageReply {
        Bridge::stage_media(self, anchor, upload, op)
    }
}

/// Observable phase of one live upload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadState {
    /// Waiting for a worker slot or a remote link.
    Waiting,
    /// Sending file chunks.
    Sending,
    /// Verifying and publishing the staged data.
    Finishing,
    /// The driver stopped with a terminal failure.
    Failed,
}

/// Read-only progress for a stable upload id returned by [`stage`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadProgress {
    /// Current driver phase.
    pub state: UploadState,
    /// Acknowledged decoded bytes.
    pub sent: u64,
    /// Total decoded bytes.
    pub total: u64,
}

/// Terminal outcome for one source item in a staging gesture.
#[derive(Debug)]
pub struct StageOutcome {
    /// Stable registry id, unchanged if the receiver expires and the driver begins again.
    pub upload: UploadId,
    /// Remote/local staged path, or the user-facing failure for this item.
    pub result: Result<PathBuf, String>,
}

/// State retained by [`AppState`] so dropping a record cancels its GPUI task.
#[derive(Default)]
pub(crate) struct UploadRegistry {
    uploads: HashMap<UploadId, UploadRecord>,
    windows: HashMap<Option<HostId>, Arc<Semaphore>>,
    workers: HashMap<Option<HostId>, Arc<Semaphore>>,
    link_waiters: HashMap<HostId, Vec<Sender<()>>>,
}

struct UploadRecord {
    id: UploadId,
    anchor: MediaAnchor,
    host: Option<HostId>,
    name: String,
    sent: u64,
    total: u64,
    state: UploadState,
    toast: u64,
    last_toast_at: Option<Instant>,
    task: Option<Task<()>>,
}

impl std::fmt::Debug for UploadRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UploadRegistry")
            .field("uploads", &self.uploads.keys())
            .field("windows", &self.windows.keys())
            .field("link_waiters", &self.link_waiters.keys())
            .finish()
    }
}

impl UploadRegistry {
    fn window(&mut self, host: Option<&HostId>) -> Arc<Semaphore> {
        self.windows
            .entry(host.cloned())
            .or_insert_with(|| Arc::new(Semaphore::new(UPLOAD_WINDOW)))
            .clone()
    }

    fn workers(&mut self, host: Option<&HostId>) -> Arc<Semaphore> {
        self.workers
            .entry(host.cloned())
            .or_insert_with(|| Arc::new(Semaphore::new(UPLOAD_CONCURRENCY)))
            .clone()
    }

    fn register_waiter(&mut self, host: &HostId) -> Receiver<()> {
        // One ready edge completes one waiter; a second value would carry no additional state.
        let (sender, receiver) = async_channel::bounded(1);
        let waiters = self.link_waiters.entry(host.clone()).or_default();
        waiters.retain(|waiter| !waiter.is_closed());
        waiters.push(sender);
        receiver
    }

    pub(crate) fn wake_link(&mut self, host: &HostId, link: LinkState) {
        if link != LinkState::Ready {
            return;
        }
        let Some(waiters) = self.link_waiters.remove(host) else {
            return;
        };
        for waiter in waiters {
            match waiter.try_send(()) {
                Ok(()) | Err(async_channel::TrySendError::Closed(_)) => {}
                Err(async_channel::TrySendError::Full(())) => {
                    tracing::debug!(%host, "media link waiter was already awake");
                }
            }
        }
    }

    pub(crate) fn wake_ready_links(&mut self, snapshot: &Snapshot) {
        for status in &snapshot.hosts {
            self.wake_link(&status.id, status.link);
        }
    }

    #[cfg(test)]
    fn record(&self, upload: UploadId) -> Option<&UploadRecord> {
        self.uploads.get(&upload)
    }
}

struct PreparedBatch {
    uploads: Vec<PreparedItem>,
    failures: Vec<ItemFailure>,
}

struct ItemFailure {
    index: usize,
    upload: UploadId,
    name: String,
    message: String,
}

struct WorkerOutcome {
    index: usize,
    upload: UploadId,
    result: Result<PathBuf, ItemFailure>,
    warning: Option<String>,
}

struct WorkerSpec<T> {
    index: usize,
    id: UploadId,
    upload: PreparedUpload,
    anchor: MediaAnchor,
    host: Option<HostId>,
    transport: T,
    outcomes: Sender<WorkerOutcome>,
}

struct UploadTarget<T> {
    anchor: MediaAnchor,
    host: Option<HostId>,
    transport: T,
    window: Arc<Semaphore>,
}

/// Stages an attachment and reports one terminal outcome per source item in source order.
///
/// Returned ids are immediately usable with [`progress`] and [`cancel`], and remain stable if an
/// expired receiver entry makes the driver begin that item again.
pub fn stage(
    state: &Entity<AppState>,
    bridge: &Bridge,
    anchor: MediaAnchor,
    attachment: Attachment,
    force_copy: bool,
    on_complete: impl FnOnce(Vec<StageOutcome>, &mut App) + 'static,
    cx: &mut App,
) -> Vec<UploadId> {
    stage_with_outcomes(
        state,
        bridge.clone(),
        anchor,
        attachment,
        force_copy,
        on_complete,
        cx,
    )
}

#[cfg(test)]
fn stage_with<T: MediaTransport>(
    state: &Entity<AppState>,
    transport: T,
    anchor: MediaAnchor,
    attachment: Attachment,
    force_copy: bool,
    on_path: impl FnOnce(Vec<PathBuf>, &mut App) + 'static,
    cx: &mut App,
) {
    let weak_state = state.downgrade();
    stage_with_outcomes(
        state,
        transport,
        anchor,
        attachment,
        force_copy,
        move |outcomes, cx| {
            let mut paths = Vec::new();
            let mut failures = Vec::new();
            for outcome in outcomes {
                match outcome.result {
                    Ok(path) => paths.push(path),
                    Err(message) => failures.push(message),
                }
            }
            if !failures.is_empty()
                && let Some(state) = weak_state.upgrade()
            {
                state.update(cx, |app, cx| {
                    app.apply_toast_event(ToastLevel::Error, failures.join("; "), Instant::now());
                    cx.notify();
                });
            }
            if !paths.is_empty() {
                on_path(paths, cx);
            }
        },
        cx,
    );
}

fn stage_with_outcomes<T: MediaTransport>(
    state: &Entity<AppState>,
    transport: T,
    anchor: MediaAnchor,
    attachment: Attachment,
    force_copy: bool,
    on_complete: impl FnOnce(Vec<StageOutcome>, &mut App) + 'static,
    cx: &mut App,
) -> Vec<UploadId> {
    let item_count = match &attachment {
        Attachment::Paths(paths) => paths.len(),
        Attachment::Blob { .. } => 1,
    };
    let uploads = (0..item_count).map(|_| UploadId::new()).collect::<Vec<_>>();
    let resolved = state.read_with(cx, |app, _| resolve_target(app, anchor));
    let ResolvedTarget {
        anchor,
        host: target_host,
        local,
    } = match resolved {
        Ok(target) => target,
        Err(message) => {
            on_complete(
                uploads
                    .iter()
                    .copied()
                    .map(|upload| StageOutcome {
                        upload,
                        result: Err(message.clone()),
                    })
                    .collect(),
                cx,
            );
            return uploads;
        }
    };
    if !force_copy
        && local
        && let Attachment::Paths(paths) = &attachment
    {
        on_complete(
            uploads
                .iter()
                .copied()
                .zip(paths.iter().cloned())
                .map(|(upload, path)| StageOutcome {
                    upload,
                    result: Ok(path),
                })
                .collect(),
            cx,
        );
        return uploads;
    }

    let prepared_uploads = uploads.clone();
    let prepare = cx
        .background_executor()
        .spawn(async move { prepare_attachment(attachment, prepared_uploads) });
    let weak_state = state.downgrade();
    let callback_uploads = uploads.clone();
    cx.spawn(async move |cx| {
        let prepared = match prepare.await {
            Ok(prepared) => prepared,
            Err(error) => {
                let message = error.to_string();
                cx.update(|cx| {
                    on_complete(
                        callback_uploads
                            .iter()
                            .copied()
                            .map(|upload| StageOutcome {
                                upload,
                                result: Err(message.clone()),
                            })
                            .collect(),
                        cx,
                    );
                });
                return;
            }
        };
        let PreparedBatch { uploads, failures } = prepared;
        let worker_count = uploads.len();
        // Every worker sends at most one terminal outcome, so this bound cannot back-pressure
        // uploads and cannot grow with chunk count.
        let (outcomes, outcome_rx) = async_channel::bounded(worker_count.max(1));
        let started = cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return false;
            };
            for item in uploads {
                spawn_upload(
                    &state,
                    WorkerSpec {
                        index: item.index,
                        id: item.id,
                        upload: item.upload,
                        anchor: anchor.clone(),
                        host: target_host.clone(),
                        transport: transport.clone(),
                        outcomes: outcomes.clone(),
                    },
                    cx,
                );
            }
            true
        });
        drop(outcomes);
        if !started {
            return;
        }

        let mut outcomes = Vec::with_capacity(worker_count + failures.len());
        outcomes.extend(failures.into_iter().map(|failure| WorkerOutcome {
            index: failure.index,
            upload: failure.upload,
            result: Err(failure),
            warning: None,
        }));
        while let Ok(outcome) = outcome_rx.recv().await {
            outcomes.push(outcome);
        }
        if outcomes.is_empty() {
            return;
        }
        outcomes.sort_by_key(|outcome| outcome.index);
        let mut completed = Vec::with_capacity(outcomes.len());
        let mut warnings = Vec::new();
        for outcome in outcomes {
            if let Some(warning) = outcome.warning {
                warnings.push(warning);
            }
            completed.push(StageOutcome {
                upload: outcome.upload,
                result: outcome
                    .result
                    .map_err(|failure| format!("{}: {}", failure.name, failure.message)),
            });
        }
        if !warnings.is_empty() {
            show_warning(&weak_state, warnings.join("; "), cx);
        }
        cx.update(|cx| on_complete(completed, cx));
    })
    // Preparation and aggregation convert every failure into user-visible state.
    .detach();
    uploads
}

struct ResolvedTarget {
    anchor: MediaAnchor,
    host: Option<HostId>,
    local: bool,
}

fn resolve_target(app: &AppState, anchor: MediaAnchor) -> Result<ResolvedTarget, String> {
    match anchor {
        MediaAnchor::Local => Ok(ResolvedTarget {
            anchor: MediaAnchor::Local,
            host: None,
            local: true,
        }),
        MediaAnchor::Terminal { terminal } => match app.terminal_host(terminal) {
            Some(Some(host)) => Ok(ResolvedTarget {
                anchor: MediaAnchor::Host { host: host.clone() },
                host: Some(host.clone()),
                local: false,
            }),
            Some(None) => Ok(ResolvedTarget {
                anchor: MediaAnchor::Terminal { terminal },
                host: None,
                local: true,
            }),
            None => Err(format!(
                "Terminal {terminal} is not available; the file was not copied"
            )),
        },
        MediaAnchor::Host { host } => Ok(ResolvedTarget {
            anchor: MediaAnchor::Host { host: host.clone() },
            host: Some(host),
            local: false,
        }),
        MediaAnchor::Thread { thread } => {
            let Some(summary) = app.agents.summary(thread) else {
                return Err(format!(
                    "Agent thread {thread} is not available; the file was not copied"
                ));
            };
            Ok(ResolvedTarget {
                anchor: match &summary.host {
                    Some(host) => MediaAnchor::Host { host: host.clone() },
                    None => MediaAnchor::Thread { thread },
                },
                host: summary.host.clone(),
                local: summary.host.is_none(),
            })
        }
    }
}

fn spawn_upload<T: MediaTransport>(state: &Entity<AppState>, spec: WorkerSpec<T>, cx: &mut App) {
    let WorkerSpec {
        index,
        id,
        upload,
        anchor,
        host,
        transport,
        outcomes,
    } = spec;
    let (window, workers) = state.update(cx, |app, _| {
        (
            app.media_uploads.window(host.as_ref()),
            app.media_uploads.workers(host.as_ref()),
        )
    });
    let weak_state = state.downgrade();
    let task_state = weak_state.clone();
    let task_anchor = anchor.clone();
    let task_host = host.clone();
    let upload_name = upload.name.clone();
    let upload_total = upload.total_bytes;
    let task = cx.spawn(async move |cx| {
        let worker = match workers.acquire_owned().await {
            Ok(worker) => worker,
            Err(_) => {
                let result = Err(ItemFailure {
                    index,
                    upload: id,
                    name: upload.name.clone(),
                    message: "media upload worker queue closed".to_owned(),
                });
                if let Err(error) = outcomes
                    .send(WorkerOutcome {
                        index,
                        upload: id,
                        result,
                        warning: None,
                    })
                    .await
                {
                    tracing::debug!(%error, "media upload result receiver was dropped");
                }
                let cleanup_state = task_state.clone();
                cx.update(|cx| {
                    // The task cannot drop its own retained handle while it is still being polled.
                    cx.defer(move |cx| {
                        cleanup_state
                            .update(cx, |app, cx| {
                                if remove_upload(app, id) {
                                    cx.notify();
                                }
                            })
                            .ok();
                    });
                });
                return;
            }
        };
        let target = UploadTarget {
            anchor: task_anchor,
            host: task_host,
            transport,
            window,
        };
        let (finished_id, result) = drive_upload(&task_state, &target, id, &upload, cx).await;
        drop(worker);
        let warning = (result.is_ok() && upload.skipped > 0).then(|| {
            format!(
                "Skipped {} symbolic link or special file entr{} while copying {}",
                upload.skipped,
                if upload.skipped == 1 { "y" } else { "ies" },
                upload.name
            )
        });
        let result = match result {
            Ok(path) => Ok(path),
            Err(DriveFailure::Terminal(message)) => Err(ItemFailure {
                index,
                upload: id,
                name: upload.name.clone(),
                message,
            }),
            Err(DriveFailure::Restart) => Err(ItemFailure {
                index,
                upload: id,
                name: upload.name.clone(),
                message: "expired twice while it was being copied".to_owned(),
            }),
            Err(DriveFailure::LinkDown(message)) => Err(ItemFailure {
                index,
                upload: id,
                name: upload.name.clone(),
                message,
            }),
            Err(DriveFailure::NotFound(message)) => Err(ItemFailure {
                index,
                upload: id,
                name: upload.name.clone(),
                message,
            }),
        };
        if let Err(error) = outcomes
            .send(WorkerOutcome {
                index,
                upload: id,
                result,
                warning,
            })
            .await
        {
            tracing::debug!(%error, "media upload result receiver was dropped");
        }
        let cleanup_state = task_state.clone();
        cx.update(|cx| {
            // The task cannot drop its own retained handle while it is still being polled.
            cx.defer(move |cx| {
                cleanup_state
                    .update(cx, |app, cx| {
                        if remove_upload(app, finished_id) {
                            cx.notify();
                        }
                    })
                    .ok();
            });
        });
    });
    state.update(cx, |app, cx| {
        insert_upload(app, id, anchor, host, upload_name, upload_total, task);
        cx.notify();
    });
}

fn insert_upload(
    app: &mut AppState,
    id: UploadId,
    anchor: MediaAnchor,
    host: Option<HostId>,
    name: String,
    total: u64,
    task: Task<()>,
) {
    let target = ToastTarget::MediaUpload(id);
    let now = Instant::now();
    let destination = host.as_ref().map_or("this machine", HostId::as_str);
    app.toast_to(
        Toast::new(format!("Copying {} to {destination}… 0%", name)).icon(Icon::CloudUpload),
        target,
        now,
        PROGRESS_TOAST_DWELL,
    );
    let toast = app
        .toasts
        .iter()
        .rev()
        .find(|toast| toast.target == Some(target))
        .map_or(u64::MAX, |toast| toast.id);
    app.media_uploads.uploads.insert(
        id,
        UploadRecord {
            id,
            anchor,
            host,
            name,
            sent: 0,
            total,
            state: UploadState::Waiting,
            toast,
            last_toast_at: None,
            task: Some(task),
        },
    );
}

fn set_waiting(app: &mut AppState, id: UploadId, host: &HostId) {
    let Some(record) = app.media_uploads.uploads.get_mut(&id) else {
        return;
    };
    record.state = UploadState::Waiting;
    update_record_toast(
        &mut app.toasts,
        record,
        format!("Waiting for {host} to reconnect…"),
    );
}

fn set_upload_state(
    state: &WeakEntity<AppState>,
    id: UploadId,
    upload_state: UploadState,
    cx: &mut AsyncApp,
) {
    state
        .update(cx, |app, cx| {
            let Some(record) = app.media_uploads.uploads.get_mut(&id) else {
                return;
            };
            if record.state == upload_state {
                return;
            }
            record.state = upload_state;
            if upload_state == UploadState::Finishing {
                update_record_toast(
                    &mut app.toasts,
                    record,
                    format!("Finishing {}…", record.name),
                );
            }
            cx.notify();
        })
        .ok();
}

fn advance_progress(state: &WeakEntity<AppState>, id: UploadId, bytes: u64, cx: &mut AsyncApp) {
    state
        .update(cx, |app, cx| {
            let Some(record) = app.media_uploads.uploads.get_mut(&id) else {
                return;
            };
            record.sent = record.sent.saturating_add(bytes).min(record.total);
            let now = Instant::now();
            let should_present = record.last_toast_at.is_none()
                || record.sent == record.total
                || record.last_toast_at.is_some_and(|last| {
                    now.saturating_duration_since(last) >= PROGRESS_UPDATE_INTERVAL
                });
            if !should_present {
                return;
            }
            record.last_toast_at = Some(now);
            let percent = record
                .sent
                .saturating_mul(100)
                .checked_div(record.total)
                .unwrap_or(100);
            let destination = record.host.as_ref().map_or("this machine", HostId::as_str);
            update_record_toast(
                &mut app.toasts,
                record,
                format!("Copying {} to {destination}… {percent}%", record.name),
            );
            cx.notify();
        })
        .ok();
}

fn update_record_toast(
    toasts: &mut [crate::state::LiveToast],
    record: &UploadRecord,
    text: String,
) {
    let Some(toast) = toasts.iter_mut().find(|toast| toast.id == record.toast) else {
        return;
    };
    toast.toast = Toast::new(text).icon(Icon::CloudUpload).action("Cancel");
    toast.expires_at = Instant::now() + PROGRESS_TOAST_DWELL;
}

fn remove_upload(app: &mut AppState, id: UploadId) -> bool {
    let Some(record) = app.media_uploads.uploads.remove(&id) else {
        return false;
    };
    app.dismiss_toast(record.toast);
    true
}

fn show_warning(state: &WeakEntity<AppState>, message: String, cx: &mut AsyncApp) {
    state
        .update(cx, |app, cx| {
            app.apply_toast_event(ToastLevel::Warning, message, Instant::now());
            cx.notify();
        })
        .ok();
}

/// Reads the latest progress for one live upload returned by [`stage`].
#[must_use]
pub fn progress(app: &AppState, upload: UploadId) -> Option<UploadProgress> {
    app.media_uploads
        .uploads
        .get(&upload)
        .map(|record| UploadProgress {
            state: record.state,
            sent: record.sent,
            total: record.total,
        })
}

/// Cancels one live upload, drops its task, and asks the daemon to remove its partial data.
pub fn cancel(state: &Entity<AppState>, bridge: &Bridge, upload: UploadId, cx: &mut App) {
    cancel_with(state, bridge.clone(), upload, cx);
}

fn cancel_with<T: MediaTransport>(
    state: &Entity<AppState>,
    transport: T,
    upload: UploadId,
    cx: &mut App,
) {
    let cancelled = state.update(cx, |app, cx| {
        let record = app.media_uploads.uploads.remove(&upload)?;
        app.dismiss_toast(record.toast);
        cx.notify();
        Some((record.anchor, record.id, record.task))
    });
    let Some((anchor, id, task)) = cancelled else {
        return;
    };
    drop(task);
    let reply = transport.stage_media(anchor, id, MediaStageOp::Cancel);
    cx.spawn(async move |_| match reply.recv().await {
        Ok(Ok(ResponseBody::Ack)) => {}
        Ok(Ok(_)) => tracing::debug!(%id, "cancel media upload returned an unexpected response"),
        Ok(Err(error)) => tracing::debug!(%id, %error, "cancel media upload was not acknowledged"),
        Err(error) => tracing::debug!(%id, %error, "cancel media upload reply channel closed"),
    })
    // Cancel is best effort; the remote idle expiry cleans up when it cannot be delivered.
    .detach();
}

#[cfg(test)]
mod tests;
