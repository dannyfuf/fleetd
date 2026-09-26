use std::{
    collections::VecDeque,
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use fleet_core::ids::HostId;
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    media::{CHUNK_BYTES, UPLOAD_IDLE_EXPIRY, UPLOAD_WINDOW},
    request::UploadId,
    response::ResponseBody,
    snapshot::LinkState,
};
use gpui::{AsyncApp, WeakEntity};
use sha2::{Digest as _, Sha256};
use tokio::sync::{OwnedSemaphorePermit, TryAcquireError};

use super::{
    MediaTransport, StageReply, UploadState, UploadTarget, advance_progress,
    prepare::{DataSource, PreparedUpload},
    set_upload_state, set_waiting,
};
use crate::{async_util::before_timeout, bridge::MediaStageOp, state::AppState};

pub(super) const INITIAL_LINK_WAIT: Duration = Duration::from_secs(30);
const RESUME_MARGIN: Duration = Duration::from_secs(15);
const RESUME_LINK_WAIT: Duration = UPLOAD_IDLE_EXPIRY.saturating_sub(RESUME_MARGIN);
pub(super) const LINK_STATE_SETTLE: Duration = Duration::from_millis(100);

struct UploadRun<'a, T> {
    state: &'a WeakEntity<AppState>,
    target: &'a UploadTarget<T>,
    id: UploadId,
}

#[derive(Clone)]
struct ChunkSpec {
    file: u32,
    offset: u64,
    bytes: Vec<u8>,
}

struct PendingChunk {
    spec: ChunkSpec,
    reply: StageReply,
    _permit: OwnedSemaphorePermit,
}

pub(super) enum DriveFailure {
    Restart,
    LinkDown(String),
    NotFound(String),
    Terminal(String),
}

enum FlushFailure {
    Restart,
    Terminal(String),
    LinkDown(Vec<ChunkSpec>),
}

pub(super) async fn drive_upload<T: MediaTransport>(
    state: &WeakEntity<AppState>,
    target: &UploadTarget<T>,
    initial_id: UploadId,
    upload: &PreparedUpload,
    cx: &mut AsyncApp,
) -> (UploadId, Result<PathBuf, DriveFailure>) {
    let id = initial_id;
    let result = async {
        for attempt in 0..=1 {
            wait_for_link(state, target.host.as_ref(), id, INITIAL_LINK_WAIT, cx).await?;
            set_upload_state(state, id, UploadState::Sending, cx);
            let run = UploadRun { state, target, id };
            match upload_once(&run, upload, cx).await {
                Ok(path) => return Ok(path),
                Err(DriveFailure::Restart) if attempt == 0 => continue,
                Err(error) => return Err(error),
            }
        }
        Err(DriveFailure::Restart)
    }
    .await;
    if result.is_err() {
        set_upload_state(state, id, UploadState::Failed, cx);
    }
    (id, result)
}

async fn upload_once<T: MediaTransport>(
    run: &UploadRun<'_, T>,
    upload: &PreparedUpload,
    cx: &mut AsyncApp,
) -> Result<PathBuf, DriveFailure> {
    loop {
        let result = expect_ack(
            run.target.transport.stage_media(
                run.target.anchor.clone(),
                run.id,
                MediaStageOp::Begin {
                    entry: upload.entry.clone(),
                },
            ),
            "begin media upload",
            run.target.host.is_some(),
        )
        .await;
        match result {
            Err(DriveFailure::LinkDown(_)) => {
                wait_for_link_after_failure(
                    run.state,
                    run.target.host.as_ref(),
                    run.id,
                    RESUME_LINK_WAIT,
                    cx,
                )
                .await?;
                set_upload_state(run.state, run.id, UploadState::Sending, cx);
            }
            Err(DriveFailure::Terminal(_))
                if link_is_down(run.state, run.target.host.as_ref(), cx) =>
            {
                wait_for_link(
                    run.state,
                    run.target.host.as_ref(),
                    run.id,
                    RESUME_LINK_WAIT,
                    cx,
                )
                .await?;
                set_upload_state(run.state, run.id, UploadState::Sending, cx);
            }
            result => break result?,
        }
    }

    let mut pending = VecDeque::new();
    let mut digests = Vec::with_capacity(upload.files.len());
    for (file_index, file) in upload.files.iter().enumerate() {
        let file_index = u32::try_from(file_index).map_err(|_| {
            DriveFailure::Terminal("media manifest contains too many files".to_owned())
        })?;
        let mut offset = 0_u64;
        let mut hasher = Sha256::new();
        while offset < file.size {
            let source = file.source.clone();
            let remaining = file.size - offset;
            let read_len = usize::try_from(remaining.min(CHUNK_BYTES as u64)).map_err(|_| {
                DriveFailure::Terminal("media chunk length does not fit this platform".to_owned())
            })?;
            let read = cx.background_executor().spawn(async move {
                read_chunk(source, offset, read_len).map(|bytes| {
                    let mut hasher = hasher;
                    hasher.update(&bytes);
                    (bytes, hasher)
                })
            });
            let (bytes, next_hasher) = read
                .await
                .map_err(|error| DriveFailure::Terminal(error.to_string()))?;
            hasher = next_hasher;
            let sent = u64::try_from(bytes.len()).map_err(|_| {
                DriveFailure::Terminal("media chunk length does not fit u64".to_owned())
            })?;
            let spec = ChunkSpec {
                file: file_index,
                offset,
                bytes,
            };
            enqueue_chunk(run, spec, &mut pending, cx).await?;
            offset = offset.saturating_add(sent);
        }
        digests.push(format!("{:x}", hasher.finalize()));
    }
    flush_all(run, &mut pending, cx).await?;

    set_upload_state(run.state, run.id, UploadState::Finishing, cx);
    loop {
        let result = receive(
            run.target.transport.stage_media(
                run.target.anchor.clone(),
                run.id,
                MediaStageOp::Finish {
                    sha256: digests.clone(),
                },
            ),
            "finish media upload",
            run.target.host.is_some(),
        )
        .await;
        match result {
            Ok(ResponseBody::Path { path, .. }) => return Ok(PathBuf::from(path)),
            Ok(_) => {
                return Err(DriveFailure::Terminal(
                    "finish media upload returned an unexpected response".to_owned(),
                ));
            }
            Err(DriveFailure::LinkDown(_)) => {
                wait_for_link_after_failure(
                    run.state,
                    run.target.host.as_ref(),
                    run.id,
                    RESUME_LINK_WAIT,
                    cx,
                )
                .await?;
                set_upload_state(run.state, run.id, UploadState::Finishing, cx);
            }
            Err(DriveFailure::NotFound(_))
                if link_is_down(run.state, run.target.host.as_ref(), cx) =>
            {
                wait_for_link_after_failure(
                    run.state,
                    run.target.host.as_ref(),
                    run.id,
                    RESUME_LINK_WAIT,
                    cx,
                )
                .await?;
                set_upload_state(run.state, run.id, UploadState::Finishing, cx);
            }
            Err(DriveFailure::NotFound(_)) => return Err(DriveFailure::Restart),
            Err(DriveFailure::Terminal(_))
                if link_is_down(run.state, run.target.host.as_ref(), cx) =>
            {
                wait_for_link(
                    run.state,
                    run.target.host.as_ref(),
                    run.id,
                    RESUME_LINK_WAIT,
                    cx,
                )
                .await?;
                set_upload_state(run.state, run.id, UploadState::Finishing, cx);
            }
            Err(error) => return Err(error),
        }
    }
}

async fn enqueue_chunk<T: MediaTransport>(
    run: &UploadRun<'_, T>,
    spec: ChunkSpec,
    pending: &mut VecDeque<PendingChunk>,
    cx: &mut AsyncApp,
) -> Result<(), DriveFailure> {
    let permit = loop {
        match Arc::clone(&run.target.window).try_acquire_owned() {
            Ok(permit) => break permit,
            Err(TryAcquireError::NoPermits) if !pending.is_empty() => {
                flush_front(run, pending, cx).await?;
            }
            Err(TryAcquireError::NoPermits) => {
                break Arc::clone(&run.target.window)
                    .acquire_owned()
                    .await
                    .map_err(|_| {
                        DriveFailure::Terminal("media upload window closed".to_owned())
                    })?;
            }
            Err(TryAcquireError::Closed) => {
                return Err(DriveFailure::Terminal(
                    "media upload window closed".to_owned(),
                ));
            }
        }
    };
    let reply = run.target.transport.stage_media(
        run.target.anchor.clone(),
        run.id,
        MediaStageOp::Chunk {
            file: spec.file,
            offset: spec.offset,
            bytes: spec.bytes.clone(),
        },
    );
    pending.push_back(PendingChunk {
        spec,
        reply,
        _permit: permit,
    });
    if pending.len() >= UPLOAD_WINDOW {
        flush_front(run, pending, cx).await?;
    }
    Ok(())
}

async fn flush_front<T: MediaTransport>(
    run: &UploadRun<'_, T>,
    pending: &mut VecDeque<PendingChunk>,
    cx: &mut AsyncApp,
) -> Result<(), DriveFailure> {
    let Some(chunk) = pending.pop_front() else {
        return Ok(());
    };
    match classify_chunk_answer(
        chunk.reply.recv().await,
        &chunk.spec,
        pending,
        run.state,
        run.id,
        cx,
    ) {
        Ok(bytes) => {
            advance_progress(run.state, run.id, bytes, cx);
            Ok(())
        }
        Err(FlushFailure::Restart) => Err(DriveFailure::Restart),
        Err(FlushFailure::Terminal(message)) => Err(DriveFailure::Terminal(message)),
        Err(FlushFailure::LinkDown(mut resend)) => {
            pending.clear();
            loop {
                wait_for_link_after_failure(
                    run.state,
                    run.target.host.as_ref(),
                    run.id,
                    RESUME_LINK_WAIT,
                    cx,
                )
                .await?;
                set_upload_state(run.state, run.id, UploadState::Sending, cx);
                let mut next = Vec::new();
                for spec in std::mem::take(&mut resend) {
                    let permit = Arc::clone(&run.target.window)
                        .acquire_owned()
                        .await
                        .map_err(|_| {
                            DriveFailure::Terminal("media upload window closed".to_owned())
                        })?;
                    let reply = run.target.transport.stage_media(
                        run.target.anchor.clone(),
                        run.id,
                        MediaStageOp::Chunk {
                            file: spec.file,
                            offset: spec.offset,
                            bytes: spec.bytes.clone(),
                        },
                    );
                    let mut none_pending = VecDeque::new();
                    match classify_chunk_answer(
                        reply.recv().await,
                        &spec,
                        &mut none_pending,
                        run.state,
                        run.id,
                        cx,
                    ) {
                        Ok(bytes) => advance_progress(run.state, run.id, bytes, cx),
                        Err(FlushFailure::LinkDown(mut retry)) => next.append(&mut retry),
                        Err(FlushFailure::Restart) => return Err(DriveFailure::Restart),
                        Err(FlushFailure::Terminal(message)) => {
                            return Err(DriveFailure::Terminal(message));
                        }
                    }
                    drop(permit);
                }
                if next.is_empty() {
                    break;
                }
                resend = next;
            }
            Ok(())
        }
    }
}

fn classify_chunk_answer(
    answer: Result<Result<ResponseBody, ProtoError>, async_channel::RecvError>,
    failed: &ChunkSpec,
    pending: &mut VecDeque<PendingChunk>,
    state: &WeakEntity<AppState>,
    id: UploadId,
    cx: &mut AsyncApp,
) -> Result<u64, FlushFailure> {
    match answer {
        Ok(Ok(ResponseBody::Ack)) => u64::try_from(failed.bytes.len())
            .map_err(|_| FlushFailure::Terminal("media chunk length does not fit u64".to_owned())),
        Ok(Ok(_)) => Err(FlushFailure::Terminal(
            "media chunk returned an unexpected response".to_owned(),
        )),
        Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
            if link_is_down_for_upload(state, id, cx) {
                classify_chunk_error(error.message, true, failed, pending, state, id, cx)
            } else {
                Err(FlushFailure::Restart)
            }
        }
        Ok(Err(error)) => classify_chunk_error(
            error.message,
            error.kind == ErrorKind::Remote,
            failed,
            pending,
            state,
            id,
            cx,
        ),
        Err(error) => classify_chunk_error(
            format!("media chunk reply channel closed: {error}"),
            true,
            failed,
            pending,
            state,
            id,
            cx,
        ),
    }
}

fn classify_chunk_error(
    message: String,
    remote_failure: bool,
    failed: &ChunkSpec,
    pending: &mut VecDeque<PendingChunk>,
    state: &WeakEntity<AppState>,
    id: UploadId,
    cx: &mut AsyncApp,
) -> Result<u64, FlushFailure> {
    let host = state
        .read_with(cx, |app, _| {
            app.media_uploads
                .uploads
                .get(&id)
                .and_then(|record| record.host.clone())
        })
        .ok()
        .flatten();
    if host.is_none() || (!remote_failure && !link_is_down(state, host.as_ref(), cx)) {
        return Err(FlushFailure::Terminal(message));
    }

    let mut resend = vec![failed.clone()];
    while let Some(chunk) = pending.pop_front() {
        match chunk.reply.try_recv() {
            Ok(Ok(ResponseBody::Ack)) => {
                let bytes = u64::try_from(chunk.spec.bytes.len()).map_err(|_| {
                    FlushFailure::Terminal("media chunk length does not fit u64".to_owned())
                })?;
                advance_progress(state, id, bytes, cx);
            }
            Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
                return Err(FlushFailure::Restart);
            }
            Ok(Ok(_)) => {
                return Err(FlushFailure::Terminal(
                    "media chunk returned an unexpected response".to_owned(),
                ));
            }
            Ok(Err(_)) | Err(async_channel::TryRecvError::Empty) => resend.push(chunk.spec),
            Err(async_channel::TryRecvError::Closed) => resend.push(chunk.spec),
        }
    }
    Err(FlushFailure::LinkDown(resend))
}

fn link_is_down_for_upload(state: &WeakEntity<AppState>, id: UploadId, cx: &mut AsyncApp) -> bool {
    let host = state
        .read_with(cx, |app, _| {
            app.media_uploads
                .uploads
                .get(&id)
                .and_then(|record| record.host.clone())
        })
        .ok()
        .flatten();
    link_is_down(state, host.as_ref(), cx)
}

fn link_is_down(state: &WeakEntity<AppState>, host: Option<&HostId>, cx: &mut AsyncApp) -> bool {
    host.is_some_and(|host| {
        state
            .read_with(cx, |app, _| app.host_link_state(host))
            .ok()
            .flatten()
            != Some(LinkState::Ready)
    })
}

async fn flush_all<T: MediaTransport>(
    run: &UploadRun<'_, T>,
    pending: &mut VecDeque<PendingChunk>,
    cx: &mut AsyncApp,
) -> Result<(), DriveFailure> {
    while !pending.is_empty() {
        flush_front(run, pending, cx).await?;
    }
    Ok(())
}

async fn expect_ack(reply: StageReply, operation: &str, remote: bool) -> Result<(), DriveFailure> {
    match receive(reply, operation, remote).await {
        Err(DriveFailure::NotFound(message)) => Err(DriveFailure::Terminal(message)),
        Err(error) => Err(error),
        Ok(ResponseBody::Ack) => Ok(()),
        Ok(_) => Err(DriveFailure::Terminal(format!(
            "{operation} returned an unexpected response"
        ))),
    }
}

async fn receive(
    reply: StageReply,
    operation: &str,
    remote: bool,
) -> Result<ResponseBody, DriveFailure> {
    match reply.recv().await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
            Err(DriveFailure::NotFound(error.message))
        }
        Ok(Err(error)) if error.kind == ErrorKind::Remote && remote => {
            Err(DriveFailure::LinkDown(error.message))
        }
        Ok(Err(error)) => Err(DriveFailure::Terminal(error.message)),
        Err(error) if remote => Err(DriveFailure::LinkDown(format!(
            "{operation} reply channel closed: {error}"
        ))),
        Err(error) => Err(DriveFailure::Terminal(format!(
            "{operation} reply channel closed: {error}"
        ))),
    }
}

async fn wait_for_link(
    state: &WeakEntity<AppState>,
    host: Option<&HostId>,
    id: UploadId,
    timeout: Duration,
    cx: &mut AsyncApp,
) -> Result<(), DriveFailure> {
    let Some(host) = host else {
        return Ok(());
    };
    let waiter = state
        .update(cx, |app, cx| {
            if app.host_link_state(host) == Some(LinkState::Ready) {
                return None;
            }
            set_waiting(app, id, host);
            cx.notify();
            Some(app.media_uploads.register_waiter(host))
        })
        .map_err(|error| DriveFailure::Terminal(error.to_string()))?;
    let Some(waiter) = waiter else {
        return Ok(());
    };
    match before_timeout(waiter.recv(), cx.background_executor().timer(timeout)).await {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(DriveFailure::Terminal(format!(
            "stopped waiting for {host} to reconnect: {error}"
        ))),
        None => Err(DriveFailure::Terminal(format!(
            "Timed out waiting for {host} to reconnect"
        ))),
    }
}

async fn wait_for_link_after_failure(
    state: &WeakEntity<AppState>,
    host: Option<&HostId>,
    id: UploadId,
    timeout: Duration,
    cx: &mut AsyncApp,
) -> Result<(), DriveFailure> {
    cx.background_executor().timer(LINK_STATE_SETTLE).await;
    wait_for_link(state, host, id, timeout, cx).await
}

fn read_chunk(source: DataSource, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = vec![0_u8; length];
    match source {
        DataSource::Path(path) => {
            let mut file = File::open(&path)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("open media file {}", path.display()))?;
            file.seek(SeekFrom::Start(offset))
                .map_err(anyhow::Error::from)
                .with_context(|| format!("seek media file {}", path.display()))?;
            file.read_exact(&mut bytes)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("read media file {}", path.display()))?;
        }
        DataSource::Bytes(source) => {
            let start = usize::try_from(offset)
                .map_err(|_| anyhow::anyhow!("media byte offset does not fit this platform"))?;
            let end = start
                .checked_add(length)
                .ok_or_else(|| anyhow::anyhow!("media byte range overflowed"))?;
            let source = source.get(start..end).ok_or_else(|| {
                anyhow::anyhow!("media byte range is outside the clipboard image")
            })?;
            bytes.copy_from_slice(source);
        }
    }
    Ok(bytes)
}
