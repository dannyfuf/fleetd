//! The durable read-through mirror of the threads other hosts own.
//!
//! > The remote daemon owns the transcript and the sequence. The local daemon is a re-framing
//! > proxy that **also** maintains a durable read-through mirror in its own `state.sqlite`. The
//! > app never knows whether a thread is local or remote (`docs/NATIVE-AGENTS.md` §9.3).
//!
//! Everything here is one side of that sentence: the local daemon's *cache* of a remote log. The
//! other side — deciding when to answer from it and when to ask the owner — is the router's
//! (`services/router/agents.rs`), because that is where the link lives.
//!
//! **Four authority rules, and where each is enforced.**
//!
//! | Rule | Enforced by |
//! | --- | --- |
//! | A thread with `owner_host` set may only be appended to from events received on that host's link | [`super::super::store::admits_append`], called on the pre-flight read here and again inside the write transaction |
//! | No harness process is ever started for a mirrored thread | [`AgentSessionManager::refuse_if_mirrored`] on every lifecycle verb, plus the orphan-settlement skip in [`super::hydrate`] |
//! | A mutation routes upstream and is never applied locally on optimism | the router classifies by owner; this module makes the local manager *refuse* rather than apply, for the window in which the router's id map is empty |
//! | The mirror never fabricates a `Synchronized` | [`AgentSessionManager::mirror_open`] answers `synchronized = false`, always, and nothing here publishes [`Event::AgentSynchronized`] |
//!
//! The third rule deserves its own sentence, because "the router already routes it" is not
//! enough. The router's thread→host map is rebuilt from a live link; between a daemon start and
//! the owner's first snapshot it is empty, and a mutation for a mirrored thread classifies as
//! *local* and arrives here. Refusing it with the owner's name is the difference between "this
//! host is unreachable" and a gate answer the owner never agreed to.

use async_trait::async_trait;
use fleet_core::{
    agents::{AgentThreadSummary, Seq, SeqEvent, ThreadId, ThreadProjection},
    ids::HostId,
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    request::RequestBody,
    response::ResponseBody,
};

use crate::{
    DaemonError, DaemonResult,
    services::router::agents::{AgentMirror, MirrorWrite},
};

use super::{
    super::store::{Admission, ThreadOwnership, admits_append, page_cursor},
    AgentSessionManager, storage_error,
    window::{OpenRequest, admits_replay, window_response},
};

/// What became of one owner-sequenced event offered to the mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MirrorIngest {
    /// The event extended the mirrored prefix.
    Applied,
    /// The prefix already held that sequence. Catch-up and live delivery overlap by design.
    Duplicate,
    /// The mirror cannot take the event without leaving a hole, so the window needs refilling.
    ///
    /// Never an error: a cache that has fallen behind is still a valid prefix, and the next open
    /// asks the owner for everything after what it holds.
    NeedsWindow,
    /// The event was refused on authority grounds and nothing was written.
    Refused,
}

impl AgentSessionManager {
    /// The threads another host owns, as the durable mirror recorded them at daemon start.
    ///
    /// The router seeds its id map from this so a mirrored thread classifies upstream from the
    /// first request of a fresh daemon, rather than only after the owner's first snapshot.
    pub(crate) fn mirrored_threads(&self) -> Vec<(ThreadId, HostId)> {
        match self.inner.store() {
            Ok(store) => store.boot_work().mirrored.clone(),
            Err(error) => {
                tracing::warn!(%error, "could not read the mirrored native-agent threads");
                Vec::new()
            }
        }
    }

    /// Refuses a verb that would act locally on a thread another host owns.
    ///
    /// The error is [`ErrorKind::Remote`] and names the owner, which is what the surface needs:
    /// a mutation on a mirrored thread is unreachable, not invalid, and the composer says so
    /// while keeping the draft.
    pub(crate) async fn refuse_if_mirrored(
        &self,
        thread: ThreadId,
        verb: &str,
    ) -> Result<(), ProtoError> {
        let Some(owner) = self.owner_of(thread).await? else {
            return Ok(());
        };
        Err(ProtoError {
            kind: ErrorKind::Remote,
            message: format!(
                "agent thread {thread} is owned by host {owner}; `{verb}` must be answered by its owner"
            ),
        })
    }

    /// The host that owns one thread, without hydrating it.
    ///
    /// One primary-key lookup. A hydrated thread answers from memory instead, which is what keeps
    /// the mutation path free of a database round trip.
    pub(crate) async fn owner_of(&self, thread: ThreadId) -> Result<Option<HostId>, ProtoError> {
        if let Some(runtime) = self.hydrated(thread) {
            return Ok(runtime.owner());
        }
        Ok(self
            .ownership(thread)
            .await?
            .and_then(|ownership| ownership.owner))
    }

    /// Where a thread is owned and how much of its log this daemon holds.
    pub(crate) async fn ownership(
        &self,
        thread: ThreadId,
    ) -> Result<Option<ThreadOwnership>, ProtoError> {
        self.inner
            .store()
            .map_err(storage_error)?
            .ownership(thread)
            .await
            .map_err(storage_error)
    }

    /// Records — or refreshes — the headers of the threads one host owns.
    ///
    /// Header-shaped and nothing more: the summaries a remote daemon broadcasts are what make the
    /// thread list survive a disconnection, and they start no provider and create no transcript.
    pub(crate) async fn mirror_adopt(&self, host: &HostId, summaries: &[AgentThreadSummary]) {
        let Ok(store) = self.inner.store() else {
            return;
        };
        for summary in summaries {
            if let Err(error) = store.mirror_claim(host.clone(), summary).await {
                tracing::warn!(
                    thread = %summary.thread,
                    %host,
                    %error,
                    "could not record a mirrored native-agent thread header"
                );
            }
        }
    }

    /// Appends one event received on `host`'s link to that host's mirrored thread.
    pub(crate) async fn mirror_ingest(
        &self,
        host: &HostId,
        thread: ThreadId,
        event: &SeqEvent,
    ) -> MirrorIngest {
        self.mirror_append(host, thread, std::slice::from_ref(event), None)
            .await
    }

    /// Appends a run of owner-sequenced events, reporting what the mirror did with them.
    ///
    /// The pre-flight read and the write transaction ask the same authority question; this one
    /// exists so a gap becomes a refill instead of a failed write, and so a refusal is logged
    /// once with the reason rather than once per event.
    pub(crate) async fn mirror_append(
        &self,
        host: &HostId,
        thread: ThreadId,
        events: &[SeqEvent],
        owner_head: Option<Seq>,
    ) -> MirrorIngest {
        let Ok(store) = self.inner.store() else {
            return MirrorIngest::Refused;
        };
        let ownership = match store.ownership(thread).await {
            Ok(ownership) => ownership,
            Err(error) => {
                tracing::warn!(%thread, %host, %error, "could not read a mirrored thread's ownership");
                return MirrorIngest::Refused;
            }
        };
        let Some(first) = events.first() else {
            return match owner_head {
                Some(head) => self.mirror_note_head(host, thread, head, &ownership).await,
                None => MirrorIngest::Duplicate,
            };
        };
        match admits_append(thread, host, ownership.as_ref(), first.seq) {
            Ok(Admission::Append) => {}
            Ok(Admission::Duplicate) => {
                // The whole run may still carry events past the prefix, so only a run that ends
                // inside it is a pure duplicate.
                let newest = events.last().map_or(first.seq, |event| event.seq);
                let head = ownership
                    .as_ref()
                    .map_or(Seq::default(), |own| own.head_seq);
                if newest <= head {
                    return MirrorIngest::Duplicate;
                }
            }
            Err(refusal) => {
                let needs_window = matches!(
                    refusal,
                    super::super::store::MirrorRefusal::Gap { .. }
                        | super::super::store::MirrorRefusal::Unknown { .. }
                );
                if needs_window {
                    tracing::debug!(%thread, %host, %refusal, "the mirror needs a refill");
                    return MirrorIngest::NeedsWindow;
                }
                tracing::warn!(%thread, %host, %refusal, "refused an append to a mirrored thread");
                return MirrorIngest::Refused;
            }
        }
        match store
            .mirror_append(host.clone(), thread, events, owner_head)
            .await
        {
            Ok(()) => {
                // The hydrated projection was built from a shorter prefix, so it is now behind
                // the log it came from. Dropping it is what keeps a mirrored read honest: the
                // database is the truth and the next open rebuilds from it, rather than this
                // daemon re-implementing the reducer over an owner's stream.
                self.forget(thread);
                MirrorIngest::Applied
            }
            Err(error) => {
                tracing::warn!(%thread, %host, %error, "could not extend a mirrored thread");
                MirrorIngest::NeedsWindow
            }
        }
    }

    /// Drops one thread's hydrated reducer state, so the next use rebuilds it from the database.
    fn forget(&self, thread: ThreadId) {
        self.inner
            .threads
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&thread);
    }

    /// Records the owner's head without any transcript, so the ladder can compare.
    async fn mirror_note_head(
        &self,
        host: &HostId,
        thread: ThreadId,
        head: Seq,
        ownership: &Option<ThreadOwnership>,
    ) -> MirrorIngest {
        let Some(ownership) = ownership else {
            return MirrorIngest::NeedsWindow;
        };
        if ownership.owner.as_ref() != Some(host) {
            return MirrorIngest::Refused;
        }
        // The mirror is never right against the owner: a head behind the prefix means the owner's
        // database was replaced, so the cache goes rather than being reconciled.
        if head < ownership.head_seq {
            return self.mirror_reset(thread).await;
        }
        MirrorIngest::Duplicate
    }

    /// Throws one mirrored thread's cached transcript away and drops its hydrated state.
    pub(crate) async fn mirror_reset(&self, thread: ThreadId) -> MirrorIngest {
        let Ok(store) = self.inner.store() else {
            return MirrorIngest::Refused;
        };
        if let Err(error) = store.mirror_discard(thread).await {
            tracing::warn!(%thread, %error, "could not discard a mirrored native-agent transcript");
            return MirrorIngest::Refused;
        }
        self.forget(thread);
        MirrorIngest::NeedsWindow
    }

    /// Absorbs the owner's answer to an open into the mirror.
    ///
    /// Only the event tail is absorbed. A projection is never merged field-by-field — that is the
    /// difference between a cache and a replica, and it is why a disagreement can only ever be
    /// resolved by taking the owner's events.
    pub(crate) async fn mirror_absorb(
        &self,
        host: &HostId,
        thread: ThreadId,
        summary: Option<&AgentThreadSummary>,
        events: &[SeqEvent],
        owner_head: Option<Seq>,
    ) -> MirrorIngest {
        if let Some(summary) = summary {
            self.mirror_adopt(host, std::slice::from_ref(summary)).await;
        }
        self.mirror_append(host, thread, events, owner_head).await
    }

    /// Answers one open entirely from the mirror, or `None` when it cannot.
    ///
    /// `None` means "ask the owner", and it is the answer in four cases: the thread is local; the
    /// mirror holds a header with no transcript; the client sent a page cursor, and the mirror
    /// holds a prefix from the start of the log so it has nothing older to give; or the owner is
    /// so far ahead that the delta would trip the admission ladder, in which case answering from
    /// the cache would paint a stale transcript that could never converge.
    ///
    /// A warm mirror answers from local SQLite with `synchronized = false`, and the caller asks
    /// the owner for `(head, …]` in parallel — zero transcript bytes cross the link and the app
    /// paints in one frame.
    pub(crate) async fn mirror_open(
        &self,
        request: &OpenRequest,
    ) -> Option<Result<ResponseBody, ProtoError>> {
        // Only a client that asked for a window can be *told* the answer is cached, and a cached
        // answer a client reads as live is exactly what §9.3's fourth authority rule forbids. A
        // version-6 open of a mirrored thread is therefore proxied, not served.
        if !request.windowed || request.before_cursor.is_some() {
            return None;
        }
        let ownership = match self.ownership(request.thread).await {
            Ok(ownership) => ownership?,
            Err(error) => return Some(Err(error)),
        };
        if !ownership.is_mirrored() || !ownership.is_warm() {
            return None;
        }
        if !admits_replay(ownership.pending_events(), 0) {
            tracing::debug!(
                thread = %request.thread,
                pending = ownership.pending_events(),
                "the mirror is too far behind its owner to answer; proxying the window instead"
            );
            return None;
        }
        let mut answer = self.answer_open(request, false).await;
        // A mirrored answer never reaches the router's id translation, because it never left this
        // machine. The owner is the one field that has to be filled in by hand, or the list would
        // say a thread is remote and its own window would say it is local.
        if let Ok(ResponseBody::AgentThreadWindow(window)) = &mut answer {
            window.summary.host = ownership.owner.clone();
        }
        Some(answer)
    }

    /// The request that brings this daemon's mirror up to the owner's head.
    ///
    /// Snapshot-then-delta, and `after_seq` is the head of the prefix already held: a caught-up
    /// mirror transfers nothing at all, and a cold one asks from zero. Whether the whole range is
    /// admissible is the **owner's** decision, because only the owner knows what those events
    /// weigh — past the ladder it answers with a bounded window and an empty tail, which leaves
    /// this mirror cold rather than half-filled.
    pub(crate) fn delta_request(ownership: &ThreadOwnership, request: &OpenRequest) -> RequestBody {
        RequestBody::AgentThreadOpen {
            thread: request.thread,
            from_seq: None,
            after_seq: Some(ownership.head_seq),
            turn_limit: Some(request.turn_limit),
            before_cursor: None,
            request_sync_marker: true,
        }
    }

    /// Builds the answer to one open from this daemon's own database.
    ///
    /// The same six statements serve a local thread and a mirrored one — that is the property the
    /// single `owner_host` column buys, and the reason `synchronized` is a parameter rather than
    /// a branch inside.
    pub(crate) async fn answer_open(
        &self,
        request: &OpenRequest,
        synchronized: bool,
    ) -> Result<ResponseBody, ProtoError> {
        let runtime = self.runtime(request.thread).await?;
        let projection = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.projection.clone()
        };
        self.window_for(request, &projection, synchronized).await
    }

    /// Windows one projection, replaying the resume tail when the ladder admits it.
    pub(crate) async fn window_for(
        &self,
        request: &OpenRequest,
        projection: &ThreadProjection,
        synchronized: bool,
    ) -> Result<ResponseBody, ProtoError> {
        let store = self.inner.store().map_err(storage_error)?;
        let before = request
            .before_cursor
            .as_deref()
            .and_then(|token| page_cursor(token, request.thread));
        let events_after = match request.resume {
            // A page read is history, never a live tail: replaying the tail under a cursor would
            // hand the client events it already applied.
            Some(_) if before.is_some() => Vec::new(),
            Some(resume) => self.admitted_tail(request.thread, resume).await?,
            None => Vec::new(),
        };
        // A resume replays **or** windows, never both. A client that sent a cursor already holds
        // everything before it, so shipping the newest turns alongside the tail that produced
        // them would double the frame to tell it what it knows. The window is what the ladder
        // falls back to, and what a cursorless open asks for.
        let keyset = if events_after.is_empty() {
            store
                .turn_keyset(
                    request.thread,
                    before,
                    usize::try_from(request.turn_limit).unwrap_or(usize::MAX),
                )
                .await
                .map_err(storage_error)?
        } else {
            super::super::store::TurnKeyset::default()
        };
        let session = store
            .session_runtime(request.thread)
            .await
            .map_err(storage_error)?;
        let mut response = window_response(
            projection,
            session.as_ref(),
            request,
            &keyset,
            events_after,
            synchronized,
        );
        response.session.capabilities = self
            .harness_capabilities(request.thread)
            .await
            .map(Box::new);
        // The budget is respected by narrowing the window, never by enlarging the frame: over
        // budget, the largest item bodies become a head plus an `elided` entry the client reads
        // back with `AgentItemBody`, and only then are the oldest turns dropped.
        let narrowed = super::bodies::narrow_to_budget(&mut response)
            .map_err(|error| storage_error(anyhow::anyhow!(error)))?;
        if narrowed > 0 {
            tracing::debug!(
                target: "fleet::agents",
                thread = %request.thread,
                turns = keyset.turns.len(),
                narrowed,
                elided = response.window.elided.len(),
                "a windowed agent transcript was narrowed to its wire budget",
            );
        }
        Ok(ResponseBody::AgentThreadWindow(Box::new(response)))
    }

    /// The replay tail of `(resume, head]`, or nothing when the ladder trips.
    ///
    /// Measured before it is read: `COUNT(*)` and `SUM(bytes)` over an indexed range, so deciding
    /// not to replay costs one statement instead of the replay it refused.
    async fn admitted_tail(
        &self,
        thread: ThreadId,
        resume: Seq,
    ) -> Result<Vec<SeqEvent>, ProtoError> {
        let store = self.inner.store().map_err(storage_error)?;
        let (events, bytes) = store
            .resume_admission(thread, resume)
            .await
            .map_err(storage_error)?;
        if !admits_replay(events, bytes) {
            tracing::info!(
                %thread,
                events,
                bytes,
                "the resume ladder tripped; answering with a window instead of a replay"
            );
            return Ok(Vec::new());
        }
        Ok(store
            .load(thread)
            .await
            .map_err(storage_error)?
            .into_iter()
            .filter(|event| event.seq > resume)
            .collect())
    }

    /// Publishes the marker that says catch-up is complete for one thread.
    ///
    /// Only ever called for a thread **this** daemon owns. A mirrored thread's marker is the
    /// owner's to emit and this daemon's to forward: the app's `Live` status must mean the owner
    /// said so (`docs/NATIVE-AGENTS.md` §9.3).
    pub(crate) fn publish_synchronized(&self, thread: ThreadId) {
        self.inner
            .events
            .publish(Event::AgentSynchronized { thread });
    }

    /// Tells subscribers a mirrored thread's stored window changed, so they re-read it.
    pub(crate) fn publish_window(&self, thread: ThreadId) {
        self.inner.events.publish(Event::AgentWindow { thread });
    }
}

#[async_trait]
impl AgentMirror for AgentSessionManager {
    async fn adopt(&self, host: &HostId, summaries: &[AgentThreadSummary]) {
        self.mirror_adopt(host, summaries).await;
    }

    async fn ingest(&self, host: &HostId, thread: ThreadId, event: &SeqEvent) -> MirrorWrite {
        match self.mirror_ingest(host, thread, event).await {
            MirrorIngest::Applied => MirrorWrite::Stored,
            MirrorIngest::NeedsWindow => MirrorWrite::NeedsWindow,
            MirrorIngest::Duplicate | MirrorIngest::Refused => MirrorWrite::Ignored,
        }
    }

    async fn open(&self, host: &HostId, body: &RequestBody) -> Option<DaemonResult<ResponseBody>> {
        let request = OpenRequest::from_body(body)?;
        let answer = self.mirror_open(&request).await?;
        // The owner has to be the host the request was routed to. A thread this daemon mirrors
        // from another host is not this link's to answer.
        if self.owner_of(request.thread).await.ok()?.as_ref() != Some(host) {
            return None;
        }
        Some(daemon_result(answer))
    }

    async fn delta(&self, host: &HostId, body: &RequestBody) -> Option<RequestBody> {
        let request = OpenRequest::from_body(body)?;
        let ownership = self.ownership(request.thread).await.ok()??;
        if ownership.owner.as_ref() != Some(host) {
            return None;
        }
        tracing::debug!(
            thread = %request.thread,
            %host,
            held = %ownership.head_seq,
            pending = ownership.pending_events(),
            "asking a thread's owner for the delta this mirror is missing"
        );
        Some(Self::delta_request(&ownership, &request))
    }

    async fn absorb(
        &self,
        host: &HostId,
        body: &RequestBody,
        response: &ResponseBody,
        announce: bool,
    ) -> bool {
        match response {
            ResponseBody::AgentThreadWindow(window) => {
                let thread = window.summary.thread;
                let applied = self
                    .mirror_absorb(
                        host,
                        thread,
                        Some(&window.summary),
                        &window.events_after,
                        Some(window.head_seq),
                    )
                    .await;
                if applied == MirrorIngest::Applied && announce {
                    self.publish_window(thread);
                }
                applied == MirrorIngest::Applied
            }
            ResponseBody::AgentThreadSnapshot {
                projection,
                events_after,
            } => {
                let thread = projection.thread;
                let summary = projection.summary(Seq::default());
                let owner_head = events_after
                    .last()
                    .map_or(projection.last_seq, |event| event.seq);
                let applied = self
                    .mirror_absorb(host, thread, Some(&summary), events_after, Some(owner_head))
                    .await;
                if applied == MirrorIngest::Applied && announce {
                    self.publish_window(thread);
                }
                applied == MirrorIngest::Applied
            }
            // A create and a listing carry headers and no transcript, which is exactly what the
            // offline list needs and all a mirror may take from them.
            ResponseBody::AgentThreadCreated(summary) => {
                self.mirror_adopt(host, std::slice::from_ref(summary)).await;
                false
            }
            ResponseBody::AgentThreads(summaries) => {
                self.mirror_adopt(host, summaries).await;
                false
            }
            _ => {
                debug_assert!(
                    !matches!(body, RequestBody::AgentThreadOpen { .. }),
                    "an open must answer with a snapshot or a window"
                );
                false
            }
        }
    }

    async fn cached_threads(&self, host: &HostId) -> Vec<AgentThreadSummary> {
        self.summaries()
            .await
            .into_iter()
            .filter(|summary| summary.host.as_ref() == Some(host))
            .collect()
    }
}

/// The daemon-shaped form of an agent refusal.
///
/// `services::dispatch` has the richer mapping — it can name the database path for a filesystem
/// failure — and this is the same shape without it, because the mirror answers inside the
/// router's result type rather than the dispatcher's.
fn daemon_result(result: Result<ResponseBody, ProtoError>) -> DaemonResult<ResponseBody> {
    result.map_err(|error| match error.kind {
        ErrorKind::NotFound => DaemonError::NotFound(error.message),
        ErrorKind::Conflict => DaemonError::Conflict(error.message),
        ErrorKind::Validation => DaemonError::Validation(error.message),
        ErrorKind::Cancelled => DaemonError::Cancelled,
        ErrorKind::Unsupported => DaemonError::Unsupported(error.message),
        ErrorKind::Remote => DaemonError::Remote(error.message),
        ErrorKind::Git => DaemonError::Git(error.message),
        ErrorKind::Github => DaemonError::Github(error.message),
        ErrorKind::Fs | ErrorKind::Tmux | ErrorKind::Unknown => {
            DaemonError::Protocol(error.message)
        }
    })
}
