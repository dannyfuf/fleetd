//! Session/link/watch observation owns catch-up work; rendering only reads prepared lines.

use super::*;
use async_channel::{Receiver, RecvError};
use fleet_proto::error::ProtoError;
use gpui::{Context, EntityId, Global, Subscription, Task, WeakEntity};
use std::collections::HashMap;

/// What a daemon reply resolves to: the response, a refusal, or a link that went away.
type Reply = Result<Result<ResponseBody, ProtoError>, RecvError>;

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
    let controller = cx.new(|cx| WatchController::new(state, bridge.clone(), cx));
    cx.default_global::<Controllers>().0.insert(id, controller);
}

struct WatchController {
    state: WeakEntity<AppState>,
    bridge: Bridge,
    _subscriptions: Vec<Subscription>,
    tasks: HashMap<u64, Task<()>>,
    next_task: u64,
    generation: u64,
}

impl WatchController {
    fn new(state: &Entity<AppState>, bridge: Bridge, cx: &mut Context<Self>) -> Self {
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
            bridge,
            _subscriptions: subscriptions,
            tasks: HashMap::new(),
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
        apply: impl 'static + FnOnce(&mut AppState, Reply),
    ) {
        let Some(state) = self.state.upgrade().map(|state| state.downgrade()) else {
            return;
        };
        let id = self.next_task;
        self.next_task += 1;
        let task = cx.spawn(async move |this, cx| {
            let response = reply.recv().await;
            let _ = state.update(cx, |app, cx| {
                if app.link_generation != generation {
                    return;
                }
                apply(app, response);
                cx.notify();
            });
            let _ = this.update(cx, |this, _| {
                this.tasks.remove(&id);
            });
        });
        self.tasks.insert(id, task);
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
            self.generation = generation;
        }
        let session = app.active_session().map(|s| s.id.clone());
        let list = state.update(cx, |app, _| app.watches.enter(session, generation));
        if let Some(session) = list {
            let known = state.read(cx).watches.ids(&session);
            let reply = self.bridge.request(RequestBody::ListWatches {
                session: session.clone(),
            });
            self.apply(cx, reply, generation, move |app, response| {
                if let Ok(Ok(ResponseBody::Watches(watches))) = response {
                    app.watches.listed(&session, known, watches, Instant::now());
                }
            });
        }
        let tails = state.update(cx, |app, _| app.watches.take_tails());
        for (watch, from_seq) in tails {
            let reply = self
                .bridge
                .request(RequestBody::TailWatch { watch, from_seq });
            self.apply(cx, reply, generation, move |app, response| match response {
                Ok(Ok(ResponseBody::WatchTail(tail))) => app.watches.tailed(tail, Instant::now()),
                Ok(Err(error)) if error.kind == ErrorKind::NotFound => app.watches.dismissed(watch),
                _ => app.watches.tail_failed(watch),
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
        let reply = self.bridge.request(RequestBody::DismissWatch { watch });
        self.apply(cx, reply, generation, move |app, response| match response {
            Ok(Ok(ResponseBody::Ack)) => app.watches.dismissed(watch),
            Ok(Err(error)) if error.kind == ErrorKind::NotFound => app.watches.dismissed(watch),
            Ok(Err(error)) => app.toast_short(error.message, Icon::Info, Instant::now()),
            _ => app.toast_short("could not dismiss watch", Icon::Info, Instant::now()),
        });
    }
}
