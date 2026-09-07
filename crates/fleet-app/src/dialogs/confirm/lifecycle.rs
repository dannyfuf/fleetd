use super::*;

/// Takes the published request and asks the daemon for the facts behind it.
pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let request = with_host(state, cx, |host| host.pending_confirm.take());
    let seq = with_host(state, cx, |host| {
        let seq = host.confirm.seq.wrapping_add(1);
        host.confirm = ConfirmState {
            request: request.clone(),
            loading: matches!(
                request,
                Some(ConfirmRequest::DeleteWorktree { .. } | ConfirmRequest::Prune { .. })
            ),
            seq,
            ..ConfirmState::default()
        };
        seq
    });
    match request {
        Some(ConfirmRequest::DeleteWorktree { id }) => inspect(id, seq, false, state, bridge, cx),
        Some(ConfirmRequest::Prune { repo }) => dry_run(repo, seq, state, bridge, cx),
        _ => {}
    }
}

/// Runs `inspect` and swaps the values in place when it answers.
pub(super) fn inspect(
    id: WorktreeId,
    seq: u64,
    fetch: bool,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::InspectWorktrees {
        ids: vec![id],
        repo: None,
        fetch,
    });
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let live = with_host(&state, cx, |host| {
                if host.confirm.seq != seq {
                    return false;
                }
                host.confirm.loading = false;
                match answer {
                    Ok(ResponseBody::Inspections(mut inspections)) => {
                        host.confirm.inspection = inspections.pop();
                    }
                    Ok(_) => {}
                    Err(failure) => host.confirm.error = Some(failure.message),
                }
                true
            });
            if live {
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "confirm-inspect", task);
}

/// Runs the prune dry run; an empty result closes the dialog with the §3.8.3 toast.
pub(super) fn dry_run(
    repo: RepoId,
    seq: u64,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    let reply = bridge.request(RequestBody::PruneWorktrees {
        dry_run: true,
        fetch: true,
        kill_sessions: false,
        repo: Some(repo.clone()),
    });
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else { return ; };
            let live = with_host(&state, cx, |host| {
                if host.confirm.seq != seq {
                    return false;
                }
                host.confirm.loading = false;
                host.confirm.checked_at = Some(Instant::now());
                match answer {
                    Ok(ResponseBody::Pruned(result)) => { host.confirm.prune = Some(result); host.confirm.update_list(); },
                    Ok(_) => {}
                    Err(failure) => host.confirm.error = Some(failure.message),
                }
                true
            });
            if !live {
                return;
            }
            let empty = with_host(&state, cx, |host| {
                host.confirm
                    .prune
                    .as_ref()
                    .map(|result| (result.deleted.is_empty(), result.skipped.len()))
            });
            if let Some((true, skipped)) = empty {
                // §3.8.3: with nothing eligible the dialog is not a dialog, it is a toast.
                state.update(cx, |app, cx| {
                    app.close_overlay();
                    app.toast_short(
                        format!(
                            "Nothing to prune in {} \u{2014} {skipped} skipped \u{00b7} J for reasons",
                            repo.name()
                        ),
                        Icon::Scissors,
                        Instant::now(),
                    );
                    cx.notify();
                });
            } else {
                notify(&state, cx);
            }
        });
    });
    crate::dialogs::retain_task(state, cx, "confirm-prune", task);
}

/// `I`: re-run the inspection with a fetch, swapping the values in place.
pub(super) fn recheck(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let (request, seq) = with_host(state, cx, |host| {
        host.confirm.seq = host.confirm.seq.wrapping_add(1);
        host.confirm.loading = true;
        host.confirm.error = None;
        (host.confirm.request.clone(), host.confirm.seq)
    });
    notify(state, cx);
    match request {
        Some(ConfirmRequest::DeleteWorktree { id }) => inspect(id, seq, true, state, bridge, cx),
        Some(ConfirmRequest::Prune { repo }) => dry_run(repo, seq, state, bridge, cx),
        _ => {
            with_host(state, cx, |host| host.confirm.loading = false);
        }
    }
}

/// Confirms: send the mutation and close. Everything it starts lives in fleetd.
pub(super) fn commit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some(request) = with_host(state, cx, |host| host.confirm.request.clone()) else {
        return;
    };
    if with_host(state, cx, |host| host.confirm.loading) {
        // §3.8.3: while the dry run is still going, the key simply does nothing.
        return;
    }
    match request {
        ConfirmRequest::DeleteWorktree { id } => {
            let reply = bridge.request(RequestBody::DeleteWorktrees { ids: vec![id] });
            crate::dialogs::host::complete_request(state, cx, async move |state, cx| {
                let Ok(Ok(ResponseBody::WorktreesDeleted(results))) = reply.recv().await else {
                    return;
                };
                let trash_entry = results.into_iter().find_map(|result| result.trash_entry);
                if let Some(trash_entry) = trash_entry
                    && let Some(state) = state.upgrade()
                {
                    state.update(cx, |app, cx| {
                        app.last_trash_entry = Some(trash_entry);
                        cx.notify();
                    });
                }
            });
        }
        ConfirmRequest::DeleteRepo { repo, .. } => {
            bridge.send(RequestBody::DeleteRepo { repo });
        }
        ConfirmRequest::DeleteContext { context, .. } => {
            bridge.send(RequestBody::DeleteContext { id: context });
        }
        ConfirmRequest::Prune { repo } => {
            bridge.send(RequestBody::PruneWorktrees {
                dry_run: false,
                fetch: false,
                kill_sessions: false,
                repo: Some(repo),
            });
        }
        ConfirmRequest::KillSession { session, .. } => {
            bridge.send(RequestBody::KillSession { session });
        }
        ConfirmRequest::CloseTerminal { terminal, .. } => {
            bridge.send(RequestBody::CloseTerminal { terminal });
        }
    }
    state.update(cx, |app, cx| {
        app.close_overlay();
        cx.notify();
    });
}
