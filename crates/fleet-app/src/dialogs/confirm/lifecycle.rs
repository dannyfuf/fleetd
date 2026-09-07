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
        let answer = reply.recv().await;
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
                    Ok(Ok(ResponseBody::Inspections(mut inspections))) => {
                        host.confirm.inspection = inspections.pop();
                    }
                    Ok(Ok(_)) => {
                        host.confirm.error = Some("unexpected inspection response".to_owned());
                    }
                    Ok(Err(failure)) => host.confirm.error = Some(failure.message),
                    Err(_) => {
                        host.confirm.error =
                            Some("fleetd disconnected during inspection".to_owned());
                    }
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
        ids: None,
    });
    let weak_state = state.downgrade();
    let task = cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let live = with_host(&state, cx, |host| {
                if host.confirm.seq != seq {
                    return false;
                }
                host.confirm.loading = false;
                host.confirm.checked_at = None;
                match answer {
                    Ok(Ok(ResponseBody::Pruned(result))) => {
                        host.confirm.checked_at = Some(Instant::now());
                        host.confirm.prune = Some(result);
                        host.confirm.update_list();
                    }
                    Ok(Ok(_)) => host.confirm.error = Some("unexpected prune response".to_owned()),
                    Ok(Err(failure)) => host.confirm.error = Some(failure.message),
                    Err(_) => {
                        host.confirm.error =
                            Some("fleetd disconnected during prune re-check".to_owned());
                    }
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
        host.confirm.begin_recheck();
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
pub(super) fn commit(state: &Entity<AppState>, bridge: &Bridge, pressed: ConfirmKey, cx: &mut App) {
    let Some(request) = with_host(state, cx, |host| host.confirm.request.clone()) else {
        return;
    };
    let allowed = with_host(state, cx, |host| {
        admits(confirmation_policy(&host.confirm, now_unix()), pressed)
    });
    if !allowed {
        return;
    }
    match request {
        ConfirmRequest::DeleteWorktree { id } => {
            let seq = with_host(state, cx, |host| {
                host.confirm.loading = true;
                host.confirm.error = None;
                host.confirm.seq
            });
            notify(state, cx);
            let reply = bridge.request(RequestBody::DeleteWorktrees {
                ids: vec![id.clone()],
            });
            crate::dialogs::host::complete_request(state, cx, async move |state, cx| {
                let outcome = match reply.recv().await {
                    Ok(Ok(ResponseBody::WorktreesDeleted(results))) => {
                        delete_outcome(&id, &results)
                    }
                    Ok(Ok(_)) => Err("unexpected delete response".to_owned()),
                    Ok(Err(failure)) => Err(failure.message),
                    Err(_) => Err("fleetd disconnected before deletion completed".to_owned()),
                };
                let Some(state) = state.upgrade() else { return };
                cx.update(|cx| {
                    let live = with_host(&state, cx, |host| host.confirm.seq == seq);
                    match outcome {
                        Ok(trash_entry) => {
                            if let Some(trash_entry) = trash_entry {
                                state.update(cx, |app, _| app.last_trash_entry = Some(trash_entry));
                            }
                            if live {
                                state.update(cx, |app, cx| {
                                    app.close_overlay();
                                    cx.notify();
                                });
                            }
                        }
                        Err(message) if live => {
                            with_host(&state, cx, |host| {
                                host.confirm.loading = false;
                                host.confirm.error = Some(message);
                            });
                            notify(&state, cx);
                        }
                        Err(_) => {}
                    }
                });
            });
            return;
        }
        ConfirmRequest::DeleteRepo { repo, .. } => {
            bridge.send(RequestBody::DeleteRepo { repo });
        }
        ConfirmRequest::DeleteContext { context, .. } => {
            bridge.send(RequestBody::DeleteContext { id: context });
        }
        ConfirmRequest::Prune { repo } => {
            let supported = state
                .read(cx)
                .daemon_capabilities
                .contains(fleet_proto::response::PRUNE_REVIEWED_IDS_CAPABILITY);
            let request = with_host(state, cx, |host| {
                reviewed_prune_request(&host.confirm, repo, supported)
            });
            let request = match request {
                Ok(request) => request,
                Err(message) => {
                    with_host(state, cx, |host| host.confirm.error = Some(message));
                    notify(state, cx);
                    return;
                }
            };
            bridge.send(request);
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

pub(super) fn reviewed_prune_ids(confirm: &ConfirmState) -> Vec<WorktreeId> {
    confirm
        .prune
        .as_ref()
        .map(|result| result.deleted.clone())
        .unwrap_or_default()
}

pub(super) fn reviewed_prune_request(
    confirm: &ConfirmState,
    repo: RepoId,
    capability_supported: bool,
) -> Result<RequestBody, String> {
    if !capability_supported {
        return Err(
            "fleetd cannot commit the reviewed prune set; update or restart fleetd, then re-check"
                .to_owned(),
        );
    }
    Ok(RequestBody::PruneWorktrees {
        dry_run: false,
        fetch: false,
        kill_sessions: false,
        repo: Some(repo),
        ids: Some(reviewed_prune_ids(confirm)),
    })
}

pub(super) fn delete_outcome(
    expected: &WorktreeId,
    results: &[fleet_proto::response::WorktreeDeleteResult],
) -> Result<Option<String>, String> {
    let failures = results
        .iter()
        .filter(|result| !result.ok)
        .map(|result| {
            result
                .reason
                .clone()
                .unwrap_or_else(|| format!("{} could not be deleted", result.worktree_id.as_str()))
        })
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    results
        .iter()
        .find(|result| &result.worktree_id == expected)
        .map(|result| result.trash_entry.clone())
        .ok_or_else(|| {
            format!(
                "fleetd returned no deletion result for {}",
                expected.as_str()
            )
        })
}
