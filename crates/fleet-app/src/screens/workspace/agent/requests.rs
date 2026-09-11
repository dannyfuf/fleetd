//! The daemon round trips one agent tab makes, and what each answer is allowed to do.
//!
//! Split out of `super` because they share one shape and none of them touches
//! [`WorkspaceScreen`](super::WorkspaceScreen): a request goes out, and the reply either installs
//! state or is dropped with a reason. Four of them are deliberately **not** fire-and-forget —
//! `open_thread`, `load_older_page` and `refresh_checkpoints` all carry an answer the surface
//! reads, and `close_agent_tab` has to release the lease before the view goes.

use gpui::{App, Entity};

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

/// Asks for a thread's newest window, or for everything after a cursor, and installs it (§8/§10).
///
/// A first paint asks for a bounded window: an unbounded snapshot of a long transcript is not a
/// slow frame but an **undecodable** one, past the frame limit. A resync asks only for the tail
/// after the cursor, which carries zero transcript bytes when nothing happened while the client
/// was away. A gap in either reply is recorded rather than papered over.
pub(super) fn open_thread(
    bridge: &Bridge,
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
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
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
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
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
    let view = view.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        // `Unsupported` is the daemon saying it keeps none; every other failure leaves the
        // listing as it was, and the next settled turn asks again.
        cx.update(|cx| {
            if let Ok(Ok(ResponseBody::AgentCheckpoints(checkpoints))) = answer {
                view.update(cx, |view, cx| view.install_checkpoints(&checkpoints, cx));
            }
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
/// The daemon keeps `last_seen_seq` in runtime state only (§3.3), so a restart brings every
/// already-read tab back as amber `needs you`. The local cursor is the surviving truth, and it
/// is sent for every thread rather than only for the tab that is being shown — a tab the user
/// never revisits would otherwise keep its dot forever.
pub(super) fn resend_seen_cursors(bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
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
