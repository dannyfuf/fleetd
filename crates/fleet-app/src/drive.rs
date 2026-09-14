//! Harness mode: a deterministic window and the socket that drives it.
//!
//! Everything here is off unless the environment asks for it. `FLEET_HARNESS=1` pins the window
//! geometry, the title and motion so two runs of the same scenario lay out identically, and
//! `FLEET_HARNESS_SOCK` opens the request/response socket described by `docs/TESTING-HARNESS.md`
//! §1. In a normal launch [`harness`] returns `None` at startup and nothing below costs a frame.
//!
//! A command is answered only once it has been applied *and* the frame that shows it has been
//! painted, so the dump or screenshot that follows it observes the effect rather than racing it.

use crate::state::{AppState, BoundsSnapshot, TargetSnapshot};
use fleet_drive::{
    input,
    predicate::{self, Clause, Evaluation, Predicate},
    protocol::{Command, HoverArgs, Request, Response},
    server,
};
use gpui::{
    App, AppContext as _, AsyncWindowContext, Entity, Pixels, SharedString, Size, Task, Window, px,
    size,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::BTreeMap, path::PathBuf, time::Duration, time::Instant};

/// Logical window size in harness mode, overridable by `FLEET_HARNESS_SIZE=WxH`.
const DEFAULT_HARNESS_SIZE: (f32, f32) = (1440.0, 900.0);

/// Smallest logical window edge the harness will accept from `FLEET_HARNESS_SIZE`.
const MIN_HARNESS_EDGE: f32 = 1.0;

/// How long a command waits for the frame showing its effect before answering anyway.
///
/// A window the compositor has stopped painting — minimised, or on an output that went away —
/// must not wedge the runner behind a frame that is never coming.
const FRAME_DEADLINE: Duration = Duration::from_millis(500);

/// Whether this process has a compositor frame loop to deliver `Window::on_next_frame`.
///
/// GPUI's headless platform accepts a next-frame callback and never calls it, so any app path
/// that parks work behind one stalls forever in the `headless` lane. Exactly one path does —
/// the stale-key queue in `shell::root::focus` — and it consults this flag for a second way to
/// make progress. False in every normal launch and in both lanes that have a compositor.
static NO_FRAME_LOOP: AtomicBool = AtomicBool::new(false);

/// Whether `Window::on_next_frame` can be relied on in this process. See [`NO_FRAME_LOOP`].
pub(crate) fn no_frame_loop() -> bool {
    NO_FRAME_LOOP.load(Ordering::Relaxed)
}

/// The longest `hover` dwell the app will honour, so a mistyped scenario cannot wedge a run.
const MAX_DWELL_MS: u64 = 5_000;

/// How long `resize` waits for the compositor to grant the size it asked for.
const RESIZE_DEADLINE: Duration = Duration::from_millis(1_000);

/// The floor under one `resize` poll, so a granted-instantly platform cannot spin.
const RESIZE_POLL: Duration = Duration::from_millis(16);

/// Harness-mode configuration, read once at startup.
pub(crate) struct Harness {
    /// Identifies this run to the compositor, through the window title, and to the runner.
    run_id: String,
    /// Whether `FLEET_HARNESS=1` asked for a pinned, animation-free window.
    deterministic: bool,
    /// The pinned logical window size.
    window_size: Size<Pixels>,
    /// The pinned window title, which the lane matches its output rule against.
    title: SharedString,
    /// The lane reported by `meta`, as the runner labelled it or as the app can prove it.
    lane: String,
    /// Whether this process really has no compositor, which decides how a command settles.
    headless: bool,
    /// Where the runner connects, when it asked for a socket.
    socket: Option<PathBuf>,
}

/// Reads harness mode from the environment. `None` in every normal launch.
pub(crate) fn harness() -> Option<Harness> {
    let deterministic = std::env::var_os("FLEET_HARNESS").is_some_and(|value| value == "1");
    let socket = non_empty_var_os("FLEET_HARNESS_SOCK").map(PathBuf::from);
    if !deterministic && socket.is_none() {
        return None;
    }
    let run_id = run_id(socket.as_deref());
    Some(Harness {
        title: SharedString::from(format!("Fleet [harness:{run_id}]")),
        run_id,
        deterministic,
        window_size: parse_size(non_empty_var("FLEET_HARNESS_SIZE").as_deref()),
        lane: lane(),
        headless: is_headless(),
        socket,
    })
}

impl Harness {
    /// The logical window size the harness pins, or `None` when it only opened a socket.
    pub(crate) fn window_size(&self) -> Option<Size<Pixels>> {
        self.deterministic.then_some(self.window_size)
    }

    /// The window title the harness pins, or `None` when it only opened a socket.
    pub(crate) fn window_title(&self) -> Option<SharedString> {
        self.deterministic.then(|| self.title.clone())
    }

    /// Installs the application-wide determinism switches, once, before the window opens.
    ///
    /// GPUI's own reduce-motion flag is the whole mechanism: `with_animation` renders its static
    /// state and schedules no animation frames, so a screenshot is reproducible and an idle
    /// window really is idle. Reading it costs nothing when the flag is false.
    pub(crate) fn prepare(&self, cx: &mut App) {
        if self.deterministic {
            cx.set_reduce_motion(true);
        }
        // Read once, before the window opens, so the frame-starved lane is known to every path
        // that would otherwise wait for a frame that is never coming.
        NO_FRAME_LOOP.store(self.headless, Ordering::Relaxed);
        // The target recorder is a thread-local flag `fleet_ui_kit::harness_target` reads once
        // per painted element; it stays false in every launch this function is not reached from,
        // which is every launch that did not ask for harness mode.
        fleet_ui_kit::harness::set_recording(true);
        // `fleet-drive` resolves `click worktrees.row[2]` by *pulling* the painted table through
        // this reader rather than depending on the kit, so a rect is never cached across a frame.
        // Without it every named location fails with "this Fleet installed no harness target
        // reader"; raw coordinates would still work, which is exactly the silent half-working
        // state worth avoiding.
        input::set_target_source(|window| input::TargetFrame {
            frame: fleet_ui_kit::harness::frame(window),
            targets: fleet_ui_kit::harness::painted(window)
                .into_iter()
                .map(|target| {
                    (
                        target.name,
                        input::TargetRect {
                            x: target.rect.x,
                            y: target.rect.y,
                            w: target.rect.w,
                            h: target.rect.h,
                            frame: target.rect.frame,
                        },
                    )
                })
                .collect(),
        });
    }
}

/// Starts the socket driver, cancelled on window close. `None` when no socket was asked for.
///
/// `state` is the one `Entity<AppState>` the window owns; `dump`, `await` and `assert` read their
/// projection from it in an update path, never from a render body.
pub(crate) fn spawn(
    harness: Harness,
    state: Entity<AppState>,
    window: &Window,
    cx: &App,
) -> Option<Task<()>> {
    let socket = harness.socket.clone()?;
    let window_id = window.window_handle().window_id();
    let (closed_tx, closed_rx) = async_channel::bounded(1);
    let subscription = cx.on_window_closed(move |_, closed| {
        if closed == window_id {
            // The receiver is gone only after the window-scoped driver has shut down.
            let _ = closed_tx.try_send(());
        }
    });
    Some(window.spawn(cx, async move |cx| {
        let _subscription = subscription;
        tokio::select! {
            _ = closed_rx.recv() => {},
            _ = run(socket, harness, state, cx) => {},
        }
    }))
}

/// Serves commands until the runner quits the app, the socket closes, or the task is cancelled.
async fn run(
    socket: PathBuf,
    harness: Harness,
    state: Entity<AppState>,
    cx: &mut AsyncWindowContext,
) {
    let (server, channel) = match server::listen(&socket) {
        Ok(parts) => parts,
        Err(error) => {
            tracing::error!(%error, socket = %socket.display(), "harness: could not open the command socket");
            return;
        }
    };
    tracing::info!(
        socket = %server.path.display(),
        run_id = %harness.run_id,
        lane = %harness.lane,
        "harness: listening"
    );
    // Nothing asks a headless window for its first frame, so the driver does: until one is
    // painted the window has no dispatch tree and the scenario's first `key` line would be
    // swallowed. Every later command gets its frame from `Harness::settle`.
    if harness.headless
        && let Err(error) = harness.paint(cx)
    {
        tracing::warn!(%error, "harness: the first headless frame was not painted");
    }
    while let Ok(incoming) = channel.requests.recv().await {
        let (response, quit) = answer(&harness, &state, &incoming.request, cx).await;
        if incoming.reply.send(response).await.is_err() {
            tracing::warn!("harness: the client went away before its response was written");
        }
        if quit {
            if let Err(error) = cx.update(|_, cx| cx.quit()) {
                tracing::warn!(%error, "harness: the window was already gone at quit");
            }
            break;
        }
    }
    // The listener thread can be parked handing over a request or waiting for its reply, so the
    // channel that releases it must close before `Server::drop` joins the thread.
    drop(channel);
    drop(server);
}

/// Applies one request and reports its outcome, plus whether the app was asked to exit.
///
/// An unknown command is a failed response, never a closed connection: the envelope is
/// additive-only and an older Fleet must be able to say so to a newer runner.
async fn answer(
    harness: &Harness,
    state: &Entity<AppState>,
    request: &Request,
    cx: &mut AsyncWindowContext,
) -> (Response, bool) {
    let id = request.id;
    let command = match request.command() {
        Ok(command) => command,
        Err(error) => {
            return (
                Response::err(id, format!("unknown command {:?}: {error}", request.cmd)),
                false,
            );
        }
    };
    let quit = matches!(command, Command::Quit(_));
    match apply(harness, state, &command, cx).await {
        Ok(Outcome {
            data,
            failure: None,
        }) => (Response::ok(id, data), quit),
        Ok(Outcome {
            data,
            failure: Some(message),
        }) => (
            Response {
                id,
                ok: false,
                data,
                error: Some(message),
            },
            false,
        ),
        Err(error) => (Response::err(id, format!("{error:#}")), false),
    }
}

/// What one command answers with: its `data` object, and the failure it reports, if any.
///
/// `assert` and a timed-out `await` are *reported* failures rather than errors: the command ran
/// exactly as asked and its answer is "no", so the response carries the snapshot and the clause
/// values that explain it. `docs/TESTING-HARNESS.md` §1 allows a failure to carry diagnostic data.
struct Outcome {
    data: Value,
    failure: Option<String>,
}

impl Outcome {
    /// A command that succeeded, with the data it answers.
    fn ok(data: Value) -> Self {
        Self {
            data,
            failure: None,
        }
    }

    /// A command that ran and reports a negative answer, with the evidence for it.
    fn failed(data: Value, message: impl Into<String>) -> Self {
        Self {
            data,
            failure: Some(message.into()),
        }
    }
}

impl From<Value> for Outcome {
    fn from(data: Value) -> Self {
        Self::ok(data)
    }
}

/// Runs one decoded command, returning the `data` object its response carries.
async fn apply(
    harness: &Harness,
    state: &Entity<AppState>,
    command: &Command,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<Outcome> {
    match command {
        Command::Meta(_) => {
            harness.settle(cx).await?;
            Ok(harness.geometry(cx)?.into())
        }
        // The response is written before `cx.quit()` runs, so the runner sees the acknowledgement
        // rather than a closed socket.
        Command::Quit(_) => Ok(json!({}).into()),
        Command::Wait(args) => {
            cx.background_executor()
                .timer(Duration::from_millis(args.millis))
                .await;
            Ok(json!({}).into())
        }
        Command::Key(args) => {
            let handled = cx.update(|window, cx| input::dispatch_key(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({ "handled": handled }).into())
        }
        Command::Type(args) => {
            cx.update(|window, cx| input::dispatch_type(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::Shot(args) => {
            anyhow::ensure!(
                !harness.headless,
                "no pixels in the headless lane: this Fleet has no compositor surface"
            );
            cx.update(|window, cx| {
                window.activate_window();
                cx.activate(true);
            })?;
            harness.settle(cx).await?;
            let mut geometry = harness.geometry(cx)?;
            if let Some(object) = geometry.as_object_mut() {
                object.insert("name".to_owned(), Value::String(args.name.clone()));
            }
            Ok(geometry.into())
        }
        Command::Scroll(args) => {
            cx.update(|window, cx| input::dispatch_scroll(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }

        Command::Dump(args) => {
            harness.settle(cx).await?;
            let projection = harness.project(state, cx)?;
            Ok(json!({
                "name": args.name,
                "revision": projection.revision,
                "summary": projection.snapshot.summary(),
                "snapshot": serde_json::to_value(projection.snapshot.as_ref())?,
            })
            .into())
        }
        Command::Await(args) => {
            let predicate = predicate::parse(&args.predicate)?;
            harness.settle(cx).await?;
            harness
                .wait_for(state, &predicate, args.timeout_ms, cx)
                .await
        }
        Command::Assert(args) => {
            let predicate = predicate::parse(&args.predicate)?;
            harness.settle(cx).await?;
            let projection = harness.project(state, cx)?;
            let snapshot = serde_json::to_value(projection.snapshot.as_ref())?;
            let evaluation = predicate::eval(&predicate, &snapshot)?;
            Ok(verdict(&predicate, &evaluation, &snapshot, None))
        }
        Command::Move(args) => {
            cx.update(|window, cx| input::dispatch_move(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::Click(args) => {
            cx.update(|window, cx| input::dispatch_click(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::Press(args) => {
            cx.update(|window, cx| input::dispatch_press(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::Release(args) => {
            cx.update(|window, cx| input::dispatch_release(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::Drag(args) => {
            cx.update(|window, cx| input::dispatch_drag(window, cx, args))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        // The dwell is awaited here rather than slept through in `fleet-drive`: a synchronous
        // dispatcher can only hold the foreground thread, and a held foreground thread is one
        // that runs none of the app timers a hover is usually waiting on.
        Command::Hover(args) => {
            cx.update(|window, cx| {
                input::dispatch_hover(
                    window,
                    cx,
                    &HoverArgs {
                        at: args.at.clone(),
                        dwell_ms: 0,
                    },
                )
            })??;
            if args.dwell_ms > 0 {
                cx.background_executor()
                    .timer(Duration::from_millis(args.dwell_ms.min(MAX_DWELL_MS)))
                    .await;
            }
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::ClipboardSet(args) => {
            cx.update(|window, cx| input::clipboard_set(window, cx, &args.text))??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::ClipboardGet(_) => {
            let text = cx.update(input::clipboard_get)??;
            Ok(json!({ "text": text }).into())
        }
        // `Window::resize` is a *request*: on Wayland the granted size arrives on a later
        // platform callback, so answering after one settle can report the size the window still
        // has. The answer is the geometry, so it waits for the bounds to actually move.
        Command::Resize(args) => {
            cx.update(|window, cx| input::resize(window, cx, args.width, args.height))??;
            harness.settle(cx).await?;
            harness.await_size(args.width, args.height, cx).await?;
            Ok(harness.geometry(cx)?.into())
        }
        Command::Blur(_) => {
            cx.update(input::blur)??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        Command::Focus(_) => {
            cx.update(input::focus)??;
            harness.settle(cx).await?;
            Ok(json!({}).into())
        }
        // The clock the app owns, moved in an update path rather than slept through. What this
        // does and does not reach is spelled out on `AppState::advance_clock`; the short version
        // is that it moves Fleet's own stored dwells and nothing inside `fleetd`.
        Command::Advance(args) => {
            let by = Duration::from_millis(args.millis);
            let changed = cx.update(|_, cx| {
                state.update(cx, |state, cx| {
                    let changed = state.advance_clock(by, Instant::now());
                    if changed {
                        cx.notify();
                    }
                    changed
                })
            })?;
            harness.settle(cx).await?;
            Ok(json!({ "millis": args.millis, "changed": changed }).into())
        }
    }
}

impl Harness {
    /// Waits for the frame that shows whatever the command just did.
    ///
    /// GPUI's headless platform drops the frame callback outright — its own comment is "anything
    /// that awaits a frame will never resolve headlessly" — and it never asks for a frame of its
    /// own either, so that lane paints one itself with [`Harness::paint`]. Everywhere else a
    /// frame is requested explicitly, which wakes a window the compositor had let go idle.
    async fn settle(&self, cx: &mut AsyncWindowContext) -> anyhow::Result<()> {
        if self.headless {
            // The update's effects are already flushed; letting the other ready foreground tasks
            // run first is what gives an entity updated from a spawned task a chance to land
            // before the frame that has to show it.
            cx.background_spawn(async {}).await;
            return self.paint(cx);
        }
        let (painted_tx, painted_rx) = async_channel::bounded(1);
        cx.update(|window, _| {
            window.on_next_frame(move |_, _| {
                // The receiver is gone only when the driver task was cancelled mid-command.
                let _ = painted_tx.try_send(());
            });
        })?;
        tokio::select! {
            _ = painted_rx.recv() => {},
            _ = cx.background_executor().timer(FRAME_DEADLINE) => {
                tracing::warn!("harness: no frame arrived within the settle deadline");
            },
        }
        Ok(())
    }

    /// Paints one frame synchronously, which only the headless lane needs.
    ///
    /// A window with no compositor surface is never asked for a frame, so without this it never
    /// lays out and never paints: `window.frame` stays `0`, `targets` stays empty, and — the
    /// reason this exists — the dispatch tree that `key` resolves its bindings against is never
    /// built, so every keystroke falls through to the IME handler and the app ignores it while
    /// `dispatch_keystroke` still answers `true`. `Window::draw` is the same call GPUI makes to
    /// give a new window its first frame; `fleet_drive::input` uses it between the phases of a
    /// click for the same reason.
    fn paint(&self, cx: &mut AsyncWindowContext) -> anyhow::Result<()> {
        cx.update(|window, cx| window.draw(cx).clear(cx))
    }

    /// Waits until the window really is `width`×`height`, or the deadline says it never will be.
    ///
    /// Missing the size is not an error: `attach` runs inside a tiling compositor that may refuse
    /// the request outright, and a scenario that cares asserts on `window.bounds` itself. What
    /// this prevents is the *silent* version — a `resize` answering with the old geometry because
    /// the granted size had not arrived yet.
    async fn await_size(
        &self,
        width: u32,
        height: u32,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<()> {
        let deadline = Instant::now() + RESIZE_DEADLINE;
        loop {
            let bounds = cx.update(|window, _| window.bounds())?;
            let granted = f32::from(bounds.size.width).round() as u32 == width
                && f32::from(bounds.size.height).round() as u32 == height;
            if granted {
                return Ok(());
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    width,
                    height,
                    actual_w = f32::from(bounds.size.width),
                    actual_h = f32::from(bounds.size.height),
                    "harness: the window never took the size resize asked for"
                );
                return Ok(());
            }
            // A frame apiece, with a floor under the poll so a platform that paints instantly
            // cannot turn a refused resize into a spin.
            self.settle(cx).await?;
            cx.background_executor().timer(RESIZE_POLL).await;
        }
    }

    /// The window geometry `meta` and `shot` report, in logical coordinates.
    ///
    /// `frame` comes from `fleet_ui_kit::harness`, which is the single frame authority: the
    /// target recorder stamps every rectangle with the same number, so the contract's "a
    /// target's frame must equal `window.frame`" rule compares one counter with itself.
    fn geometry(&self, cx: &mut AsyncWindowContext) -> anyhow::Result<Value> {
        let (bounds, scale_factor, frame) = cx.update(|window, _| {
            (
                window.bounds(),
                window.scale_factor(),
                fleet_ui_kit::harness::frame(window),
            )
        })?;
        Ok(json!({
            "run_id": self.run_id,
            "lane": self.lane,
            "bounds": {
                "x": f32::from(bounds.origin.x),
                "y": f32::from(bounds.origin.y),
                "w": f32::from(bounds.size.width),
                "h": f32::from(bounds.size.height),
            },
            "scale_factor": scale_factor,
            "title": self.title.as_ref(),
            "frame": frame,
        }))
    }

    /// Copies the painted frame's window metrics and target table into `AppState`, then projects.
    ///
    /// The builder holds `&AppState` and cannot reach either — the window belongs to gpui and the
    /// target table to `fleet_ui_kit`'s paint-time recorder — so the command that is about to
    /// answer brings them across here, in an update path. Both writers compare before they store,
    /// so an identical frame does not invalidate the memo and does not wake a waiting `await`.
    fn project(
        &self,
        state: &Entity<AppState>,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<crate::state::HarnessProjection> {
        cx.update(|window, cx| {
            let frame = fleet_ui_kit::harness::frame(window);
            // Paint order is back to front, so a name painted twice keeps its topmost rectangle.
            let targets: BTreeMap<String, TargetSnapshot> = fleet_ui_kit::harness::painted(window)
                .into_iter()
                .map(|target| {
                    (
                        target.name.to_string(),
                        TargetSnapshot {
                            x: target.rect.x,
                            y: target.rect.y,
                            w: target.rect.w,
                            h: target.rect.h,
                            frame: target.rect.frame,
                        },
                    )
                })
                .collect();
            let bounds = window.bounds();
            let bounds = BoundsSnapshot {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                w: f32::from(bounds.size.width),
                h: f32::from(bounds.size.height),
            };
            let scale_factor = window.scale_factor();
            let title = self.title.to_string();
            state.update(cx, |state, _| {
                state.harness.set_targets(targets);
                state.harness.set_window(bounds, scale_factor, title, frame);
                state.harness_projection()
            })
        })
    }

    /// Re-evaluates `predicate` whenever the application state changes, until it holds or
    /// `timeout_ms` elapses.
    ///
    /// The wake-up is `cx.observe` on the one `Entity<AppState>`: that is the app's own
    /// invalidation signal, which every update path already raises with `cx.notify()`, so the
    /// waiter needs no poll timer and an idle app costs nothing while a scenario waits on it.
    /// Only a moved revision can change the answer, so a notification that projects identically
    /// is skipped without re-running the predicate.
    ///
    /// Two snapshot fields are not woken by this signal, and a scenario should reach for them
    /// with `dump` or `assert`, which project on demand:
    ///
    /// * `window` and `targets` are copied in by [`Harness::project`] rather than by an update
    ///   path, so they are read as of the moment this call projects them.
    /// * `terminal` moves on a terminal-only bridge batch, which
    ///   `crate::shell::root::events::apply_batch` applies *without* notifying `AppState` in a
    ///   normal launch — plain PTY output must not wake every chrome observer. In harness mode
    ///   that same path raises the signal explicitly, so `await terminal.text ~= …` waits for
    ///   the output itself rather than for the next unrelated notification.
    ///
    /// A timeout is a *reported* failure carrying the last snapshot and every clause's actual
    /// value, which is what `docs/TESTING-HARNESS.md` §1 promises.
    async fn wait_for(
        &self,
        state: &Entity<AppState>,
        predicate: &Predicate,
        timeout_ms: u64,
        cx: &mut AsyncWindowContext,
    ) -> anyhow::Result<Outcome> {
        // One slot is the whole queue: a full channel already says "re-project", and one
        // re-projection answers for any number of notifications the waiter slept through.
        let (changed_tx, changed_rx) = async_channel::bounded(1);
        // Armed before the first evaluation, so a change that lands between the two is not lost.
        // The subscription is held by this command and unregisters when it answers.
        let _observation = cx.update(|_, cx| {
            cx.observe(state, move |_, _| {
                // The receiver is gone only once this command has answered.
                let _ = changed_tx.try_send(());
            })
        })?;
        let mut expiry = cx
            .background_executor()
            .timer(Duration::from_millis(timeout_ms));
        let mut evaluated: Option<u64> = None;
        let mut last = None;
        loop {
            let projection = self.project(state, cx)?;
            if evaluated != Some(projection.revision) {
                evaluated = Some(projection.revision);
                let snapshot = serde_json::to_value(projection.snapshot.as_ref())?;
                let evaluation = predicate::eval(predicate, &snapshot)?;
                if evaluation.satisfied {
                    return Ok(verdict(predicate, &evaluation, &snapshot, Some(timeout_ms)));
                }
                last = Some((evaluation, snapshot));
            }
            tokio::select! {
                _ = &mut expiry => break,
                _ = changed_rx.recv() => {},
            }
        }
        // The first pass always evaluates, because no revision can equal `None`.
        let Some((evaluation, snapshot)) = last else {
            anyhow::bail!("await evaluated no revision before its {timeout_ms} ms timeout")
        };
        Ok(verdict(predicate, &evaluation, &snapshot, Some(timeout_ms)))
    }
}

/// Builds the answer `assert` and `await` share: the verdict, the clauses, and the snapshot.
///
/// The snapshot rides along on failure only. A satisfied predicate has nothing to explain, and a
/// passing `await idle` between every scenario step must not write a full dump into `run.jsonl`.
fn verdict(
    predicate: &Predicate,
    evaluation: &Evaluation,
    snapshot: &Value,
    timeout_ms: Option<u64>,
) -> Outcome {
    let mut data = json!({
        "predicate": predicate.to_string(),
        "satisfied": evaluation.satisfied,
        "clauses": evaluation
            .clauses
            .iter()
            .map(|outcome| json!({
                "clause": outcome.clause.to_string(),
                "satisfied": outcome.satisfied,
                "actual": outcome.actual,
            }))
            .collect::<Vec<_>>(),
    });
    let Some(summary) = evaluation.failure_summary() else {
        return Outcome::ok(data);
    };
    // `idle` reports one JSON object of five counters, so the clause's own account of itself is
    // that whole object. A scenario that waited on the bare word is owed the shorter answer:
    // which constituent is still pending.
    let summary = if evaluation
        .failures()
        .any(|outcome| matches!(outcome.clause, Clause::Idle))
    {
        format!(
            "{summary}; still busy: {}",
            busy_counters(&snapshot["idle"])
        )
    } else {
        summary
    };
    if let Some(object) = data.as_object_mut() {
        object.insert("idle".to_owned(), snapshot["idle"].clone());
        object.insert("snapshot".to_owned(), snapshot.clone());
    }
    match timeout_ms {
        Some(timeout_ms) => {
            Outcome::failed(data, format!("timed out after {timeout_ms} ms: {summary}"))
        }
        None => Outcome::failed(data, summary),
    }
}

/// The five constituents of `idle`, in the order `docs/TESTING-HARNESS.md` §3 lists them.
const IDLE_COUNTERS: [&str; 5] = [
    "in_flight_requests",
    "running_jobs",
    "pending_frame",
    "live_toast_timers",
    "armed_debounces",
];

/// Names the constituents of `idle` that still report pending work, as `name=value` pairs.
fn busy_counters(idle: &Value) -> String {
    let busy: Vec<String> = IDLE_COUNTERS
        .iter()
        .filter(|counter| is_pending(&idle[**counter]))
        .map(|counter| format!("{counter}={}", idle[*counter]))
        .collect();
    if busy.is_empty() {
        // `idle` is derived from the five counters, never reported beside them, so this pair
        // cannot disagree unless the projection the predicate read was stale.
        return "nothing, which means the projection is stale".to_owned();
    }
    busy.join(", ")
}

/// Whether one constituent of `idle` reports pending work: a true flag or a nonzero count.
fn is_pending(counter: &Value) -> bool {
    match counter {
        Value::Bool(pending) => *pending,
        Value::Number(count) => count.as_u64().is_some_and(|count| count > 0),
        _ => false,
    }
}

/// `FLEET_HARNESS_RUN_ID`, else the socket's file stem, else this process id.
///
/// The runner owns the identity; the fallbacks only keep a hand-driven session nameable.
fn run_id(socket: Option<&std::path::Path>) -> String {
    non_empty_var("FLEET_HARNESS_RUN_ID")
        .or_else(|| {
            socket
                .and_then(std::path::Path::file_stem)
                .map(|stem| stem.to_string_lossy().into_owned())
                .filter(|stem| !stem.is_empty())
        })
        .unwrap_or_else(|| std::process::id().to_string())
}

/// Parses `WIDTHxHEIGHT`, falling back to the fixed default so geometry never varies by accident.
fn parse_size(raw: Option<&str>) -> Size<Pixels> {
    let parsed = raw.and_then(|value| {
        let (width, height) = value.split_once(['x', 'X'])?;
        let width: f32 = width.trim().parse().ok()?;
        let height: f32 = height.trim().parse().ok()?;
        (width.is_finite()
            && height.is_finite()
            && width >= MIN_HARNESS_EDGE
            && height >= MIN_HARNESS_EDGE)
            .then_some((width, height))
    });
    let (width, height) = parsed.unwrap_or(DEFAULT_HARNESS_SIZE);
    size(px(width), px(height))
}

/// Whether this process has no compositor to paint into.
fn is_headless() -> bool {
    std::env::var_os("ZED_HEADLESS").is_some()
        || (non_empty_var_os("WAYLAND_DISPLAY").is_none() && non_empty_var_os("DISPLAY").is_none())
}

/// The lane `meta` reports.
///
/// Only the runner can tell a dedicated virtual output from the developer's own session, so it
/// labels the run through `FLEET_HARNESS_LANE`; without that label the app reports what it can
/// prove for itself.
fn lane() -> String {
    non_empty_var("FLEET_HARNESS_LANE").unwrap_or_else(|| {
        if is_headless() {
            "headless".to_owned()
        } else {
            "attach".to_owned()
        }
    })
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn non_empty_var_os(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_drive::protocol::{AdvanceArgs, AssertArgs, AwaitArgs, DumpArgs, EmptyArgs, KeyArgs};
    use fleet_ui_kit::{Icon, Toast};
    use gpui::{Context, IntoElement, Render, TestAppContext, div};
    use std::os::unix::net::UnixStream;

    /// The toast dwell these tests advance past, chosen so real waiting would be obvious.
    const TEST_DWELL_MS: u64 = 4_000;
    const TEST_DWELL: Duration = Duration::from_millis(TEST_DWELL_MS);

    struct DriverWindow;
    impl Render for DriverWindow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// A window plus the one application state its driver projects, as `spawn` pairs them.
    fn driver(cx: &mut TestAppContext) -> (Entity<AppState>, AsyncWindowContext) {
        let window = cx.add_window(|_, _| DriverWindow);
        let state = cx.update(|cx| {
            cx.new(|_| {
                AppState::new(
                    std::path::PathBuf::from("/tmp/fleet-harness-test"),
                    Instant::now(),
                )
            })
        });
        let async_cx = window
            .update(cx, |_, window, cx| window.to_async(cx))
            .expect("window context");
        (state, async_cx)
    }

    fn harness_for(socket: Option<PathBuf>) -> Harness {
        Harness {
            run_id: "test".to_owned(),
            deterministic: true,
            window_size: parse_size(None),
            title: SharedString::new_static("Fleet [harness:test]"),
            lane: "headless".to_owned(),
            headless: true,
            socket,
        }
    }

    fn socket_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-app-drive-{name}-{}-{:?}.sock",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn harness_geometry_is_identical_across_launches() {
        // Two launches of the same run read the same environment, so the pinned size must be a
        // pure function of it; a malformed value falls back rather than varying.
        assert_eq!(parse_size(None), parse_size(None));
        assert_eq!(parse_size(None), size(px(1440.0), px(900.0)));
        assert_eq!(parse_size(Some("1600x1000")), size(px(1600.0), px(1000.0)));
        assert_eq!(parse_size(Some("1600X1000")), size(px(1600.0), px(1000.0)));
        for bad in [
            "",
            "1600",
            "widexhigh",
            "0x0",
            "-4x-4",
            "NaNxNaN",
            "infxinf",
        ] {
            assert_eq!(parse_size(Some(bad)), parse_size(None), "{bad:?}");
        }
    }

    #[test]
    fn the_run_id_names_the_window_for_the_compositor() {
        let id = run_id(Some(std::path::Path::new("/tmp/fleet-harness/abc123.sock")));
        assert_eq!(id, "abc123");
        assert_eq!(format!("Fleet [harness:{id}]"), "Fleet [harness:abc123]");
        assert!(!run_id(None).is_empty());
    }

    #[gpui::test]
    async fn an_unknown_command_fails_without_closing_the_connection(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        let request = Request {
            id: 9,
            cmd: "future".to_owned(),
            args: json!({}),
        };
        let (response, quit) = answer(&harness, &state, &request, &mut async_cx).await;
        assert_eq!(response.id, 9);
        assert!(!response.ok && !quit);
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unknown command \"future\"")),
            "{:?}",
            response.error
        );
    }

    #[gpui::test]
    async fn dump_answers_the_whole_snapshot_and_its_summary(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        let dump = Request::new(
            1,
            &Command::Dump(DumpArgs {
                name: "hub".to_owned(),
            }),
        )
        .expect("encode dump");
        let (response, _) = answer(&harness, &state, &dump, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["name"], json!("hub"));
        assert_eq!(
            response.data["snapshot"]["version"],
            json!(crate::state::SNAPSHOT_VERSION)
        );
        assert_eq!(response.data["snapshot"]["screen"], json!("Hub"));
        // The pinned title and geometry reach the snapshot through the same update path.
        assert_eq!(
            response.data["snapshot"]["window"]["title"],
            json!("Fleet [harness:test]")
        );
        assert!(
            response.data["summary"]
                .as_str()
                .is_some_and(|summary| summary.starts_with("screen=Hub ")),
            "{:?}",
            response.data["summary"]
        );
        // Two dumps with nothing between them describe the same revision.
        let (again, _) = answer(&harness, &state, &dump, &mut async_cx).await;
        assert_eq!(response.data["revision"], again.data["revision"]);
    }

    #[gpui::test]
    async fn assert_reports_the_actual_value_it_saw(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        let held = Request::new(
            1,
            &Command::Assert(AssertArgs {
                predicate: "screen == Hub".to_owned(),
            }),
        )
        .expect("encode assert");
        let (response, _) = answer(&harness, &state, &held, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["satisfied"], json!(true));
        assert!(
            response.data["snapshot"].is_null(),
            "a held assert carries no dump"
        );

        let failed = Request::new(
            2,
            &Command::Assert(AssertArgs {
                predicate: "screen == Workspace".to_owned(),
            }),
        )
        .expect("encode assert");
        let (response, _) = answer(&harness, &state, &failed, &mut async_cx).await;
        assert!(!response.ok);
        assert_eq!(response.data["clauses"][0]["actual"], json!("Hub"));
        assert_eq!(response.data["snapshot"]["screen"], json!("Hub"));
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("screen is \"Hub\"")),
            "{:?}",
            response.error
        );

        // A malformed predicate is a command error, not a verdict.
        let malformed = Request::new(
            3,
            &Command::Assert(AssertArgs {
                predicate: "screen ==".to_owned(),
            }),
        )
        .expect("encode assert");
        let (response, _) = answer(&harness, &state, &malformed, &mut async_cx).await;
        assert!(!response.ok);
        assert!(response.data["clauses"].is_null());
    }

    #[gpui::test]
    async fn await_returns_as_soon_as_its_predicate_holds_and_times_out_otherwise(
        cx: &mut TestAppContext,
    ) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        let satisfied = Request::new(
            1,
            &Command::Await(AwaitArgs {
                predicate: "screen == Hub && overlay absent".to_owned(),
                timeout_ms: 5_000,
            }),
        )
        .expect("encode await");
        let (response, _) = answer(&harness, &state, &satisfied, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["satisfied"], json!(true));

        let never = Request::new(
            2,
            &Command::Await(AwaitArgs {
                predicate: "screen == Workspace".to_owned(),
                timeout_ms: 20,
            }),
        )
        .expect("encode await");
        let (response, _) = answer(&harness, &state, &never, &mut async_cx).await;
        assert!(!response.ok);
        // The timeout names the clause, the value it saw, and all six idle fields.
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("timed out after 20 ms")),
            "{:?}",
            response.error
        );
        assert_eq!(response.data["clauses"][0]["actual"], json!("Hub"));
        assert_eq!(response.data["idle"]["running_jobs"], json!(0));
        assert!(response.data["idle"]["idle"].is_boolean());
    }

    /// Pushes one toast onto the real state, exactly as an update path would.
    fn toast(cx: &mut TestAppContext, state: &Entity<AppState>, text: &'static str) {
        cx.update(|cx| {
            state.update(cx, |state, cx| {
                state.toast(
                    Toast::new(text).icon(Icon::Info),
                    Instant::now(),
                    TEST_DWELL,
                );
                cx.notify();
            });
        });
    }

    #[gpui::test]
    async fn await_answers_on_the_state_change_that_satisfies_it(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        // The change lands on a timer, so the waiter is already parked on its observation when
        // the state moves. Nothing polls: the only wake-up is the `cx.notify()` below.
        let updater = cx.update(|cx| {
            let state = state.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(5))
                    .await;
                state.update(cx, |state, cx| {
                    state.toast(
                        Toast::new("harness-wake").icon(Icon::Info),
                        Instant::now(),
                        TEST_DWELL,
                    );
                    cx.notify();
                });
            })
        });

        let started = Instant::now();
        let request = Request::new(
            1,
            &Command::Await(AwaitArgs {
                predicate: "toasts[0].text == \"harness-wake\"".to_owned(),
                timeout_ms: 60_000,
            }),
        )
        .expect("encode await");
        let (response, _) = answer(&harness, &state, &request, &mut async_cx).await;
        updater.await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["satisfied"], json!(true));
        assert!(
            response.data["snapshot"].is_null(),
            "a satisfied await carries no dump"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the waiter answered on the change, not on its 60 s timeout"
        );
    }

    #[gpui::test]
    async fn an_await_timeout_names_the_idle_counter_that_is_still_busy(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);
        toast(cx, &state, "harness-busy");

        let request = Request::new(
            1,
            &Command::Await(AwaitArgs {
                predicate: "idle".to_owned(),
                timeout_ms: 20,
            }),
        )
        .expect("encode await");
        let (response, _) = answer(&harness, &state, &request, &mut async_cx).await;
        assert!(!response.ok);
        let error = response.error.unwrap_or_default();
        assert!(
            error.contains("still busy: live_toast_timers=1"),
            "an `idle` timeout names the constituent, not just the object: {error}"
        );
        assert_eq!(response.data["idle"]["live_toast_timers"], json!(1));
        assert_eq!(response.data["idle"]["idle"], json!(false));
        // The last snapshot rides along so the failure explains itself.
        assert_eq!(
            response.data["snapshot"]["toasts"][0]["text"],
            json!("harness-busy")
        );
    }

    #[gpui::test]
    async fn advance_expires_a_toast_without_waiting_out_its_dwell(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);
        toast(cx, &state, "harness-dwell");

        let present = Request::new(
            1,
            &Command::Assert(AssertArgs {
                predicate: "toasts[0].text == \"harness-dwell\"".to_owned(),
            }),
        )
        .expect("encode assert");
        let (response, _) = answer(&harness, &state, &present, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);

        let started = Instant::now();
        let advance = Request::new(
            2,
            &Command::Advance(AdvanceArgs {
                millis: TEST_DWELL_MS + 1,
            }),
        )
        .expect("encode advance");
        let (response, _) = answer(&harness, &state, &advance, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["changed"], json!(true));

        let gone = Request::new(
            3,
            &Command::Assert(AssertArgs {
                predicate: "toasts[0] absent".to_owned(),
            }),
        )
        .expect("encode assert");
        let (response, _) = answer(&harness, &state, &gone, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);
        assert!(
            started.elapsed() < TEST_DWELL,
            "the dwell was advanced through, not waited out"
        );
    }

    #[gpui::test]
    async fn meta_reports_the_pinned_window_identity(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        let meta = Request::new(1, &Command::Meta(EmptyArgs {})).expect("encode meta");
        let (first, _) = answer(&harness, &state, &meta, &mut async_cx).await;
        assert!(first.ok, "{:?}", first.error);
        assert_eq!(first.data["run_id"], json!("test"));
        assert_eq!(first.data["lane"], json!("headless"));
        assert_eq!(first.data["title"], json!("Fleet [harness:test]"));

        let (second, _) = answer(&harness, &state, &meta, &mut async_cx).await;
        assert_eq!(first.data["bounds"], second.data["bounds"]);
        assert_eq!(first.data["scale_factor"], second.data["scale_factor"]);
    }

    #[gpui::test]
    async fn a_key_is_applied_and_its_handling_reported(cx: &mut TestAppContext) {
        let harness = harness_for(None);
        let (state, mut async_cx) = driver(cx);

        let key = Request::new(
            1,
            &Command::Key(KeyArgs {
                keys: vec!["?".to_owned()],
            }),
        )
        .expect("encode key");
        let (response, _) = answer(&harness, &state, &key, &mut async_cx).await;
        assert!(response.ok, "{:?}", response.error);
        // Nothing in this bare window binds `?`, which is exactly what the runner is told.
        assert_eq!(response.data["handled"], json!([false]));

        let bad = Request::new(
            2,
            &Command::Key(KeyArgs {
                keys: vec!["not-a-key".to_owned()],
            }),
        )
        .expect("encode key");
        let (response, _) = answer(&harness, &state, &bad, &mut async_cx).await;
        assert!(!response.ok);
    }

    #[gpui::test]
    async fn closing_the_window_stops_the_driver_and_unlinks_its_socket(cx: &mut TestAppContext) {
        let path = socket_path("close");
        let window = cx.add_window(|_, _| DriverWindow);
        let state = cx.update(|cx| {
            cx.new(|_| {
                AppState::new(
                    std::path::PathBuf::from("/tmp/fleet-harness-test"),
                    Instant::now(),
                )
            })
        });
        let task = window
            .update(cx, |_, window, cx| {
                spawn(harness_for(Some(path.clone())), state.clone(), window, cx)
            })
            .expect("update the window")
            .expect("a socket was configured");
        cx.run_until_parked();
        assert!(path.exists(), "the driver binds before it serves");

        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("close the window");
        task.await;
        assert!(!path.exists(), "the driver unlinks its socket on shutdown");
        assert!(
            UnixStream::connect(&path).is_err(),
            "a closed window leaves nothing listening"
        );
    }
}
