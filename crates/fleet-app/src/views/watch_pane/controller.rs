//! Session/link/watch observation owns catch-up work; rendering only reads prepared lines.

use super::*;
use async_channel::{Receiver, RecvError};
use fleet_proto::error::ProtoError;
use gpui::{Context, EntityId, Global, Subscription, Task, WeakEntity};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// What a daemon reply resolves to: the response, a refusal, or a link that went away.
type Reply = Result<Result<ResponseBody, ProtoError>, RecvError>;
const RECOVERY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
const WATCH_RECOVERY_ERROR: &str = "could not recover watch output";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RecoveryKey {
    List { session: SessionId, generation: u64 },
    Tail(WatchId),
}

struct RetryPlan {
    key: RecoveryKey,
    failed_attempt: u8,
}

fn retry_delay(failed_attempt: u8) -> Duration {
    let exponent = u32::from(failed_attempt.saturating_sub(1));
    RECOVERY_RETRY_DELAY
        .checked_mul(1_u32.checked_shl(exponent).unwrap_or(u32::MAX))
        .unwrap_or(Duration::MAX)
}

fn watch_recovery_error_prefix(watch: WatchId) -> String {
    format!("{WATCH_RECOVERY_ERROR} (watch {}): ", watch.0)
}

fn clear_watch_recovery_error(app: &mut AppState, watch: WatchId) {
    let prefix = watch_recovery_error_prefix(watch);
    if app
        .sticky_error
        .as_ref()
        .is_some_and(|error| error.job.is_none() && error.text.starts_with(prefix.as_str()))
    {
        app.sticky_error = None;
    }
}

#[derive(Clone)]
struct WatchRequests(
    Arc<dyn Fn(RequestBody) -> Receiver<Result<ResponseBody, ProtoError>> + Send + Sync>,
);

impl WatchRequests {
    fn bridge(bridge: Bridge) -> Self {
        Self(Arc::new(move |body| bridge.request(body)))
    }

    fn request(&self, body: RequestBody) -> Receiver<Result<ResponseBody, ProtoError>> {
        (self.0)(body)
    }
}

pub(super) fn apply_dismiss_reply(app: &mut AppState, watch: WatchId, response: Reply) {
    let failure = match response {
        Ok(Ok(ResponseBody::Ack)) => {
            app.watches.dismissed(watch);
            return;
        }
        Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
            app.watches.dismissed(watch);
            return;
        }
        Ok(Err(error)) => error.message,
        Ok(Ok(_)) => "could not dismiss watch: daemon returned an unexpected response".into(),
        Err(_) => "could not dismiss watch: daemon reply was lost".into(),
    };
    app.sticky_error = Some(crate::state::StickyError {
        text: failure,
        job: None,
        retryable: false,
    });
}

/// One controller per observed [`AppState`].
///
/// The Shell reaches watch observation through free functions, so the controller entity has
/// nowhere on the Shell to live; it is keyed by the state it observes and dropped with it.
#[derive(Default)]
struct Controllers(HashMap<EntityId, Entity<WatchController>>);
impl Global for Controllers {}

/// Mounts the state observer once. Initial synchronization is deferred beyond rendering; later
/// work follows state notifications, and the release observer drops the owner with its state.
pub(crate) fn sync(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let id = state.entity_id();
    if cx.default_global::<Controllers>().0.contains_key(&id) {
        return;
    }
    let controller =
        cx.new(|cx| WatchController::new(state, WatchRequests::bridge(bridge.clone()), cx));
    cx.default_global::<Controllers>().0.insert(id, controller);
}

struct WatchController {
    state: WeakEntity<AppState>,
    requests: WatchRequests,
    _subscriptions: Vec<Subscription>,
    tasks: HashMap<u64, Task<()>>,
    retries: HashMap<RecoveryKey, Task<()>>,
    next_task: u64,
    generation: u64,
}

impl WatchController {
    fn new(state: &Entity<AppState>, requests: WatchRequests, cx: &mut Context<Self>) -> Self {
        let id = state.entity_id();
        let subscriptions = vec![
            cx.observe(state, |this, _, cx| this.synchronize(cx)),
            cx.observe_release(state, move |_, _, cx| {
                cx.default_global::<Controllers>().0.remove(&id);
            }),
        ];
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| this.synchronize(cx));
        });
        Self {
            state: state.downgrade(),
            requests,
            _subscriptions: subscriptions,
            tasks: HashMap::new(),
            retries: HashMap::new(),
            next_task: 0,
            generation: 0,
        }
    }

    /// Applies one reply under `generation`, holding the task until it settles so that dropping
    /// the controller — or relinking — cancels catch-up work instead of writing a stale answer.
    fn apply(
        &mut self,
        cx: &mut Context<Self>,
        reply: Receiver<Result<ResponseBody, ProtoError>>,
        generation: u64,
        apply: impl 'static + FnOnce(&mut AppState, Reply) -> Option<RetryPlan>,
    ) {
        let Some(state) = self.state.upgrade().map(|state| state.downgrade()) else {
            return;
        };
        let id = self.next_task;
        self.next_task += 1;
        let task = cx.spawn(async move |this, cx| {
            let response = reply.recv().await;
            let retry = state.update(cx, |app, _| {
                if app.link_generation != generation {
                    return None;
                }
                Some(apply(app, response))
            });
            let _ = this.update(cx, |this, _| {
                this.tasks.remove(&id);
            });
            let applied = matches!(&retry, Ok(Some(_)));
            if let Ok(Some(Some(retry))) = retry {
                let _ = this.update(cx, |this, cx| this.schedule_retry(retry, cx));
            }
            if applied {
                let _ = state.update(cx, |_, cx| cx.notify());
            }
        });
        self.tasks.insert(id, task);
    }

    fn schedule_retry(&mut self, retry: RetryPlan, cx: &mut Context<Self>) {
        if self.retries.contains_key(&retry.key) {
            return;
        }
        let key = retry.key.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(retry_delay(retry.failed_attempt))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.retries.remove(&retry.key);
                let Some(state) = this.state.upgrade() else {
                    return;
                };
                let ready = state.update(cx, |app, _| match &retry.key {
                    RecoveryKey::List {
                        session,
                        generation,
                    } => app.watches.list_retry_ready(session, *generation),
                    RecoveryKey::Tail(watch) => app.watches.tail_retry_ready(*watch),
                });
                if ready {
                    this.synchronize(cx);
                }
            });
        });
        self.retries.insert(key, task);
    }

    fn synchronize(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let app = state.read(cx);
        if app.drops_terminal_keys() {
            return;
        }
        let generation = app.link_generation;
        if self.generation != generation {
            self.tasks.clear();
            self.retries.clear();
            self.generation = generation;
        }
        let session = app.active_session().map(|s| s.id.clone());
        let list = state.update(cx, |app, _| app.watches.enter(session, generation));
        if let Some(session) = list {
            self.retries.remove(&RecoveryKey::List {
                session: session.clone(),
                generation,
            });
            let known = state.read(cx).watches.ids(&session);
            let reply = self.requests.request(RequestBody::ListWatches {
                session: session.clone(),
            });
            self.apply(cx, reply, generation, move |app, response| match response {
                Ok(Ok(ResponseBody::Watches(watches))) => {
                    app.watches
                        .listed(&session, generation, known, watches, Instant::now());
                    None
                }
                _ => app
                    .watches
                    .list_failed(&session, generation)
                    .map(|failed_attempt| RetryPlan {
                        key: RecoveryKey::List {
                            session,
                            generation,
                        },
                        failed_attempt,
                    }),
            });
        }
        let tails = state.update(cx, |app, _| app.watches.take_tails());
        for (watch, from_seq) in tails {
            self.retries.remove(&RecoveryKey::Tail(watch));
            let reply = self
                .requests
                .request(RequestBody::TailWatch { watch, from_seq });
            self.apply(cx, reply, generation, move |app, response| match response {
                Ok(Ok(ResponseBody::WatchTail(tail))) => {
                    app.watches.tailed(tail, Instant::now());
                    clear_watch_recovery_error(app, watch);
                    None
                }
                Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
                    app.watches.dismissed(watch);
                    clear_watch_recovery_error(app, watch);
                    None
                }
                failure => {
                    let message = match &failure {
                        Ok(Err(error)) => error.message.as_str(),
                        Ok(Ok(_)) => "daemon returned an unexpected response",
                        Err(_) => "daemon reply was lost",
                    };
                    let retry = app
                        .watches
                        .tail_failed(watch, from_seq)
                        .map(|failed_attempt| RetryPlan {
                            key: RecoveryKey::Tail(watch),
                            failed_attempt,
                        });
                    if retry
                        .as_ref()
                        .is_some_and(|retry| retry.failed_attempt == 1)
                    {
                        app.sticky_error = Some(crate::state::StickyError {
                            text: format!("{}{message}", watch_recovery_error_prefix(watch)),
                            job: None,
                            retryable: false,
                        });
                    }
                    retry
                }
            });
        }
    }
}

/// Dismiss a finished watch or locally hide a running one, preserving the existing error policy.
pub(crate) fn dismiss_selected(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    sync(state, bridge, cx);
    let owner = cx
        .global::<Controllers>()
        .0
        .get(&state.entity_id())
        .cloned();
    if let Some(owner) = owner {
        owner.update(cx, |controller, cx| controller.dismiss(cx));
    }
}

impl WatchController {
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let selected = state.update(cx, |app, cx| {
            let selected = app.close_selected_watch(Instant::now());
            cx.notify();
            selected
        });
        let Some(watch) = selected else {
            return;
        };
        let generation = state.read(cx).link_generation;
        let reply = self.requests.request(RequestBody::DismissWatch { watch });
        self.apply(cx, reply, generation, move |app, response| {
            apply_dismiss_reply(app, watch, response);
            None
        });
    }
}

#[cfg(test)]
mod tests {
    include!("tests.rs");
}
