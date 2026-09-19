//! The daemon round trips one agent tab makes, and what each answer is allowed to do.
//!
//! Split out of `super` because they share one shape and none of them touches
//! [`WorkspaceScreen`](super::WorkspaceScreen): a request goes out, and the reply either installs
//! state or is dropped with a reason. Five of them are deliberately **not** fire-and-forget —
//! `open_thread`, `load_older_page` and `refresh_checkpoints` all carry an answer the surface
//! reads, `account_login` carries the one thing the user has to act on, and `close_agent_tab` has
//! to release the lease before the view goes.

use gpui::{App, Entity, SharedString};

use fleet_core::{
    agents::{AgentKind, Seq, ThreadId},
    ids::WorktreeId,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

use crate::{
    bridge::{Bridge, BridgeCommand},
    screens::agent_thread::AgentThreadView,
    state::AppState,
};
use fleet_ui_kit::Icon;

use super::super::{SessionKind, record_mutation_failure, workspace_tabs, worktree_of};
use super::leave_agent_tab;

/// `ctrl-s x` on an agent tab drops the client lease; the thread keeps running.
///
/// The daemon answers the close with an ack and goes on listing the thread — §6 keeps every
/// thread browsable — so taking the tab out of the strip is the client's own bookkeeping. Without
/// it the tab is redrawn by the very next summary broadcast and `^s x` looks like it did nothing.
pub(super) fn close_agent_tab(bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
    let Some(thread) = state.read(cx).active_session().and_then(|session| {
        let SessionKind::Worktree(worktree) = &session.kind else {
            return None;
        };
        state.read(cx).agents.active(worktree)
    }) else {
        return;
    };
    bridge.send_agent(BridgeCommand::AgentThreadClose { thread });
    leave_agent_tab(state, cx);
    state.update(cx, |app, cx| {
        if app.agents.close(thread) {
            cx.notify();
        }
    });
}

/// Selects a thread and mirrors a top-level reopen to the daemon.
///
/// Delegated children remain attached only in this window; their caller owns the durable tab.
pub(crate) fn reopen_agent_tab(
    state: &Entity<AppState>,
    thread: ThreadId,
    send: impl FnOnce(BridgeCommand),
    cx: &mut App,
) -> bool {
    let app = state.read(cx);
    let was_closed = app.agents.is_closed(thread);
    let top_level = app
        .agents
        .summary(thread)
        .is_some_and(|summary| summary.parent.is_none());
    let selected = state.update(cx, |app, cx| {
        let selected = app.select_agent_thread(thread);
        if selected {
            cx.notify();
        }
        selected
    });
    if selected && was_closed && top_level {
        send(BridgeCommand::AgentThreadReopen { thread });
    }
    selected
}

/// Asks for a thread's newest window, or for everything after a cursor, and installs it (§8/§10).
///
/// A first paint asks for a bounded window: an unbounded snapshot of a long transcript is not a
/// slow frame but an **undecodable** one, past the frame limit. A resync asks only for the tail
/// after the cursor, which carries zero transcript bytes when nothing happened while the client
/// was away. A gap in either reply is recorded rather than papered over.
pub(super) fn open_thread(
    bridge: &impl super::AgentThreadRequester,
    state: &Entity<AppState>,
    thread: ThreadId,
    from_seq: Option<Seq>,
    cx: &mut App,
) {
    let window = match from_seq {
        Some(seq) => fleet_client::AgentWindowRequest::resume(seq),
        None => fleet_client::AgentWindowRequest::newest(fleet_proto::agents::WINDOW_DEFAULT_TURNS),
    };
    let reply = bridge.request_agent(BridgeCommand::AgentThreadOpen { thread, window });
    // One request is in flight from here: the flag comes back only if the reply is a gap.
    state.update(cx, |app, _| app.agents.clear_resync(thread));
    let bridge = bridge.clone();
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        let Some(state) = state.upgrade() else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(Ok(ResponseBody::AgentThreadSnapshot {
                projection,
                events_after,
            })) => state.update(cx, |app, cx| {
                app.agents.install_snapshot(projection, &events_after);
                cx.notify();
            }),
            // A windowing daemon answers the same open with a bounded window plus the session
            // view the composer's controls are gated on.
            Ok(Ok(ResponseBody::AgentThreadWindow(window))) => state.update(cx, |app, cx| {
                app.agents.install_window(&window);
                cx.notify();
            }),
            // A cursor the daemon no longer holds — a log trimmed on hydrate, another home — is
            // not the user's problem: one fresh bounded open replaces the catch-up (§9.2).
            Ok(Err(error)) if from_seq.is_some() => {
                tracing::debug!(
                    %thread,
                    error = %error.message,
                    "catch-up open refused; re-opening the newest window instead"
                );
                open_thread(&bridge, &state, thread, None, cx);
            }
            Ok(Err(error)) => state.update(cx, |app, cx| {
                record_mutation_failure(app, format!("agent thread: {}", error.message));
                cx.notify();
            }),
            // A lost or unexpected reply leaves the tab on its placeholder projection; the next
            // summary or event re-arms the resync rather than clearing the transcript.
            Ok(Ok(_)) | Err(_) => {}
        });
    })
    .detach();
}

/// Loads the page of transcript behind the window a thread already holds (§8).
///
/// Does nothing without a page cursor, which is how "there is no more history" is expressed: the
/// daemon omits `before_cursor` when the window reached the start of the thread. A `Parked`
/// outcome is dropped rather than retried in a loop — the mirror is behind the head the page was
/// read at, the live events that close the gap are already on their way, and the next approach
/// to the top asks again.
pub(super) fn load_older_page(
    bridge: &Bridge,
    state: &Entity<AppState>,
    thread: ThreadId,
    cx: &mut App,
) {
    let Some(cursor) = state.read(cx).agents.older_cursor(thread) else {
        return;
    };
    let reply = bridge.request_agent(BridgeCommand::AgentThreadOpen {
        thread,
        window: fleet_client::AgentWindowRequest::older_than(
            cursor,
            fleet_proto::agents::WINDOW_DEFAULT_TURNS,
        ),
    });
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        let Some(state) = state.upgrade() else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(Ok(ResponseBody::AgentThreadWindow(window))) => {
                state.update(cx, |app, cx| match app.agents.merge_older_page(&window) {
                    fleet_client::PageOutcome::Merged { turns, items } => {
                        if turns > 0 || items > 0 {
                            cx.notify();
                        }
                    }
                    fleet_client::PageOutcome::Parked { .. }
                    | fleet_client::PageOutcome::Unknown => {}
                })
            }
            Ok(Err(error)) => state.update(cx, |app, cx| {
                record_mutation_failure(app, format!("agent history: {}", error.message));
                cx.notify();
            }),
            // A daemon without the window capability answers the unbounded snapshot, which is
            // not a page and must never be merged as one: it would replace the whole transcript.
            Ok(Ok(_)) | Err(_) => {}
        });
    })
    .detach();
}

/// Re-reads one thread's checkpoint listing into its view.
///
/// A daemon that keeps no checkpoints refuses with `Unsupported` and the view keeps an empty
/// listing, which is what stops `[u]` being drawn there — the refusal is not a user-facing
/// failure, so it is not toasted. Anything else is: a revert the user is about to press should
/// not be offered from a listing that silently failed to load.
pub(super) fn refresh_checkpoints(
    bridge: &Bridge,
    view: &Entity<AgentThreadView>,
    thread: ThreadId,
    cx: &mut App,
) {
    let reply = bridge.request_agent(BridgeCommand::AgentCheckpoints { thread });
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        // `Unsupported` is the daemon saying it keeps none; every other failure leaves the
        // listing as it was, and the next settled turn asks again.
        cx.update(|cx| {
            if let Ok(Ok(ResponseBody::AgentCheckpoints(checkpoints))) = answer {
                view.update(cx, |view, cx| view.install_checkpoints(&checkpoints, cx))
                    .ok();
            }
        });
    })
    .detach();
}

/// `⏎`: sends the turn, and tells the view when the daemon refused it.
///
/// An ack changes nothing here — the bubble is reconciled by the projection, by id — but a
/// refusal (`not live`, a harness that would not take the prompt, the transport deadline) is
/// the one answer the view cannot learn any other way, and a bubble stuck at `sending` is the
/// thread reading `working` for as long as the tab stays open.
pub(super) fn send_turn(
    bridge: &Bridge,
    view: &Entity<AgentThreadView>,
    command: BridgeCommand,
    item: Option<fleet_core::agents::ItemId>,
    cx: &mut App,
) {
    let Some(item) = item else {
        // Nothing to mark failed: only the composer's own sends carry an id.
        bridge.send_agent(command);
        return;
    };
    let reply = bridge.request_agent(command);
    let view = view.downgrade();
    cx.spawn(async move |cx| {
        let reason = match reply.recv().await {
            Ok(Ok(_)) => return,
            Ok(Err(error)) => error.message,
            Err(_) => "the Fleet daemon bridge closed before answering".to_owned(),
        };
        tracing::warn!(%item, reason, "the daemon refused an agent send");
        // A closed tab has no bubble left to mark, which is the one way this update fails.
        view.update(cx, |view, cx| view.send_failed(item, &reason, cx))
            .ok();
    })
    .detach();
}

/// `/login`: starts the harness sign-in and opens the URL it answers with.
///
/// The answer is acted on rather than stored: a sign-in URL is a one-shot the user has to follow
/// now, so it is opened in their browser and named in a toast — and it is *also* a `Notice` row
/// the daemon appended, which is where it stays reachable if the browser did not open.
pub(super) fn account_login(
    bridge: &Bridge,
    state: &Entity<AppState>,
    thread: ThreadId,
    cx: &mut App,
) {
    let reply = bridge.request_agent(BridgeCommand::AgentAccountLogin { thread });
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        let Some(state) = state.upgrade() else {
            return;
        };
        cx.update(|cx| match answer {
            Ok(Ok(ResponseBody::AgentAccountLogin { auth_url })) => {
                cx.open_url(&auth_url);
                state.update(cx, |app, cx| {
                    app.toast_short(
                        SharedString::new_static("opening the sign-in page"),
                        Icon::Info,
                        std::time::Instant::now(),
                    );
                    cx.notify();
                });
            }
            // A harness that signed in without a browser acks instead, and there is nothing to
            // open: the transcript's own `AccountChanged` says what happened.
            Ok(Ok(_)) => {}
            Ok(Err(error)) => state.update(cx, |app, cx| {
                record_mutation_failure(app, format!("agent sign-in: {}", error.message));
                cx.notify();
            }),
            // The bridge is gone, which the connection banner already says.
            Err(_) => {}
        });
    })
    .detach();
}

/// `o` on a focused row: the same `$EDITOR` terminal tab Settings' `E` opens.
pub(super) fn open_in_editor(bridge: &Bridge, state: &Entity<AppState>, path: &str, cx: &mut App) {
    let (session, cwd) = {
        let app = state.read(cx);
        let Some(session) = app.active_session().cloned() else {
            return;
        };
        let cwd = worktree_of(app, &session)
            .map(|worktree| worktree.path.clone())
            .unwrap_or_else(|| session.cwd.clone());
        (session, cwd)
    };
    bridge.send(RequestBody::NewTerminal {
        session: session.id.clone(),
        name: workspace_tabs::unique_terminal_name(&session, "edit"),
        command: format!("{} {}", crate::dialogs::editor_command(), shell_quote(path)),
        cwd,
    });
}

/// Single-quotes a path so a name with spaces survives the daemon's shell.
pub(super) fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// The explicit migration escape hatch: the PTY agent popup, on demand (§10).
///
/// §1 makes the terminal the fallback *for the thread you are in*, so the popup runs that
/// thread's provider rather than always Claude.
pub(super) fn open_terminal_fallback(
    state: &Entity<AppState>,
    provider: Option<AgentKind>,
    worktree: Option<WorktreeId>,
    cx: &mut App,
) {
    let agent = match provider {
        Some(AgentKind::Codex) => fleet_core::config::Agent::Codex,
        _ => fleet_core::config::Agent::Claude,
    };
    state.update(cx, |app, cx| {
        app.toggle_agent_popup(agent, worktree);
        cx.notify();
    });
}

/// Tells the daemon what this window has actually shown, clearing `finished` and `unread`.
pub(super) fn mark_seen(
    bridge: &Bridge,
    state: &Entity<AppState>,
    seq: Seq,
    thread: ThreadId,
    cx: &mut App,
) {
    let already = state.read(cx).agents.seen(thread);
    if already >= seq {
        return;
    }
    state.update(cx, |app, cx| {
        app.agents.mark_seen(thread, seq);
        cx.notify();
    });
    bridge.send_agent(BridgeCommand::AgentMarkSeen { thread, seq });
}

/// Re-reports what this window has read for every thread the daemon has forgotten.
///
/// Identified clients persist a monotonic cursor in the daemon (§9.3). This process can still be
/// ahead of the cursor returned by the latest connection, so it re-sends only those newer local
/// overrides; a tab the user never revisits must not regain an amber dot after reconnect.
pub(super) fn resend_seen_cursors(bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
    let persisted = bridge.agent_seen_cursors();
    state.update(cx, |app, _| app.agents.seed_seen(&persisted));
    let stale = state.read(cx).agents.stale_seen();
    if stale.is_empty() {
        return;
    }
    state.update(cx, |app, _| {
        for (thread, seq) in &stale {
            app.agents.mark_reported(*thread, *seq);
        }
    });
    for (thread, seq) in stale {
        bridge.send_agent(BridgeCommand::AgentMarkSeen { thread, seq });
    }
}
