//! §3.8.3 Confirm — *show me exactly what I will lose, in facts, with their age*.
//!
//! One dialog serves delete, prune, kill and close-terminal. What it asks about is published
//! by whoever opens it through [`crate::dialogs::request_confirm`]; the facts, the size, the
//! escalation key and the consequence sentence are all derived from that request.
//!
//! **[D-10] escalation.** `y` confirms only while every decisive fact (`dirty`,
//! `uniqueCommits`, `published`, `session`) is known; one unknown fact — or an inspection
//! error, or a still-running dry run — makes `Y` the only key, and `Enter` stops working.
//! [`FactList::confirm_key`] owns that rule so the footer and the key bindings cannot drift.

use std::time::Instant;

use fleet_core::{
    github::InspectionPrState,
    ids::{ContextId, RepoId, SessionId, TerminalId, WorktreeId},
    inspection::WorktreeInspection,
    sessions::SessionState,
};
use fleet_proto::{request::RequestBody, response::PruneResult, response::ResponseBody};
use fleet_ui_kit::{Icon, prelude::*};
use gpui::{AnyElement, App, Entity, FocusHandle, Window, div, px};

use crate::{
    actions::confirm as confirm_actions,
    bridge::Bridge,
    dialogs::{age_secs, notify, now_epoch, root, with_host},
    state::AppState,
};

/// What the open Confirm dialog asks about.
///
/// The hub, the workspace and the palette all route their destructive keys through this: call
/// [`crate::dialogs::request_confirm`] and then open [`crate::dialogs::Dialogs::Confirm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmRequest {
    /// `d` on a worktree row.
    DeleteWorktree {
        /// The worktree to delete.
        id: WorktreeId,
    },
    /// `d` on a repo row: cascades to every worktree and session.
    DeleteRepo {
        /// The repository to delete.
        repo: RepoId,
        /// How many worktrees go with it.
        worktrees: usize,
    },
    /// `D` in the Hub, or `ctrl-d` in the Edit-context dialog.
    DeleteContext {
        /// The context to delete.
        context: ContextId,
        /// Its display name.
        name: String,
        /// How many repositories go with it.
        repos: usize,
        /// How many worktrees go with it.
        worktrees: usize,
        /// How many sessions go with it.
        sessions: usize,
    },
    /// `x` on a worktree row: a multi-target confirm with its own body.
    Prune {
        /// The repository to prune.
        repo: RepoId,
    },
    /// `K` on a worktree row.
    KillSession {
        /// The session to kill.
        session: SessionId,
        /// How many terminals die with it.
        terminals: usize,
        /// The keep-alive labels running in it.
        running: Vec<String>,
        /// Whether an editor in it has unsaved changes.
        unsaved: bool,
    },
    /// `ctrl-s x` in the Workspace.
    CloseTerminal {
        /// The terminal to close.
        terminal: TerminalId,
        /// Its one-based tab index.
        index: usize,
        /// Its name.
        name: String,
        /// The command running in it, when one is.
        running: Option<String>,
    },
}

impl ConfirmRequest {
    /// The title §3.8.3 gives this action.
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            Self::DeleteWorktree { id } => format!("Delete {}?", id.as_str()),
            Self::DeleteRepo { repo, .. } => format!("Delete repository {}?", repo.as_str()),
            Self::DeleteContext { name, .. } => format!("Delete context \"{name}\"?"),
            Self::Prune { repo } => format!("Prune {}", repo.as_str()),
            Self::KillSession { session, .. } => format!("Kill session {}?", session.as_str()),
            Self::CloseTerminal { index, name, .. } => {
                format!("Close terminal {index} \"{name}\"?")
            }
        }
    }

    /// The consequence sentence. Users confirm the sentence, not the title (§3.8.3).
    #[must_use]
    pub fn consequence(&self, facts: &Facts) -> String {
        match self {
            Self::DeleteWorktree { .. } => {
                if facts.risky {
                    "Deleting kills the session and moves the copy to trash; commits that exist \
                     only here are lost."
                        .to_owned()
                } else {
                    "Moves the copy to trash, then removes it in the background.".to_owned()
                }
            }
            Self::DeleteRepo { worktrees, .. } => format!(
                "Also deletes {worktrees} worktrees and their sessions. The base clone and every \
                 copy go to trash."
            ),
            Self::DeleteContext {
                repos,
                worktrees,
                sessions,
                ..
            } => format!(
                "Also deletes {repos} repositories, {worktrees} worktrees and every session in \
                 them ({sessions} running)."
            ),
            Self::Prune { .. } => {
                "Deletes the ones listed below. The skipped ones are kept, with the reason shown."
                    .to_owned()
            }
            Self::KillSession {
                terminals,
                running,
                unsaved,
                ..
            } => {
                let mut sentence = format!("Kills {terminals} terminals at once.");
                if *unsaved {
                    sentence.push_str(" An editor has unsaved changes.");
                }
                if !running.is_empty() {
                    sentence.push_str(&format!(" {} are killed too.", running.join(", ")));
                }
                sentence.push_str(" Nothing is saved.");
                sentence
            }
            Self::CloseTerminal { running, .. } => match running {
                Some(command) => format!("{command} is running in it and will be killed."),
                None => "The terminal and its shell are closed.".to_owned(),
            },
        }
    }

    /// The header glyph §3.8 assigns to this confirm.
    #[must_use]
    pub const fn icon(&self, compact: bool) -> Icon {
        if !compact {
            return Icon::TriangleAlert;
        }
        match self {
            Self::DeleteWorktree { .. } | Self::DeleteRepo { .. } | Self::DeleteContext { .. } => {
                Icon::Trash
            }
            Self::Prune { .. } => Icon::Scissors,
            Self::KillSession { .. } => Icon::Power,
            Self::CloseTerminal { .. } => Icon::X,
        }
    }

    /// The verb on the primary action.
    #[must_use]
    pub fn action_label(&self, prune_count: usize) -> String {
        match self {
            Self::DeleteWorktree { .. } | Self::DeleteRepo { .. } | Self::DeleteContext { .. } => {
                "Delete".to_owned()
            }
            Self::Prune { .. } => format!("Prune {prune_count}"),
            Self::KillSession { .. } => "Kill".to_owned(),
            Self::CloseTerminal { .. } => "Close".to_owned(),
        }
    }

    /// Whether §3.8.3 requires `Y` regardless of what the facts say.
    ///
    /// Repo and context delete cascade far beyond the row under the cursor, so [D-10] escalates
    /// them unconditionally.
    #[must_use]
    pub const fn always_strong(&self) -> bool {
        matches!(self, Self::DeleteRepo { .. } | Self::DeleteContext { .. })
    }

    /// Whether `I` re-checks anything here.
    #[must_use]
    pub const fn rechecks(&self) -> bool {
        matches!(self, Self::DeleteWorktree { .. } | Self::Prune { .. })
    }

    /// The full id of the target, shown on its own line in the expanded form.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::DeleteWorktree { id } => id.as_str().to_owned(),
            Self::DeleteRepo { repo, .. } | Self::Prune { repo } => repo.as_str().to_owned(),
            Self::DeleteContext { context, .. } => context.as_str().to_owned(),
            Self::KillSession { session, .. } => session.as_str().to_owned(),
            Self::CloseTerminal { name, .. } => name.clone(),
        }
    }
}

/// The Confirm dialog's draft.
#[derive(Debug, Clone, Default)]
pub struct ConfirmState {
    /// What is being confirmed.
    pub request: Option<ConfirmRequest>,
    /// The inspection behind a delete confirm's facts.
    pub inspection: Option<WorktreeInspection>,
    /// The dry run behind a prune confirm's body.
    pub prune: Option<PruneResult>,
    /// Whether facts are still being gathered.
    pub loading: bool,
    /// The exact warning from a failed inspection.
    pub error: Option<String>,
    /// Whether the prune body shows its KEEP list (`s`).
    pub show_keep: bool,
    /// When the facts last arrived, for the mandatory freshness stamp.
    pub checked_at: Option<Instant>,
    /// Bumps on every seed and re-check.
    pub seq: u64,
}

/// The decisive facts of §3.8.3, already ordered and classified.
#[derive(Debug, Default)]
pub struct Facts {
    /// The kit list, which owns the ordering and the escalation key.
    pub list: FactList,
    /// Whether any risk fact is true.
    pub risky: bool,
    /// The age of the facts in seconds, for the mandatory freshness stamp.
    pub age_secs: Option<i64>,
}

/// Turns one inspection into the fact list §3.8.3 draws.
#[must_use]
pub fn worktree_facts(inspection: Option<&WorktreeInspection>, loading: bool, now: i64) -> Facts {
    let Some(inspection) = inspection else {
        return Facts {
            list: FactList::new().loading(true),
            risky: false,
            age_secs: None,
        };
    };
    let mut list = FactList::new().loading(loading);
    let mut risky = false;

    if inspection.dirty {
        risky = true;
        list = list.fact(Fact::risk(match inspection.dirty_files {
            Some(count) => format!("{count} uncommitted files"),
            None => "uncommitted changes".to_owned(),
        }));
    } else {
        list = list.fact(Fact::safe("clean"));
    }

    match inspection.unique_commits {
        Some(0) => list = list.fact(Fact::safe("no commits of its own")),
        Some(count) => {
            risky = true;
            list = list.fact(Fact::risk(format!(
                "{count} commits not on {}",
                inspection.target_branch
            )));
        }
        None => {
            list = list.fact(Fact::unknown("unique commit count unavailable"));
        }
    }

    if inspection.merged || inspection.merged_into_target {
        list = list.fact(Fact::safe(format!(
            "merged into {}",
            inspection.target_branch
        )));
    } else if let Some(pull_request) = inspection.pr.as_ref() {
        let label = match pull_request.state {
            InspectionPrState::Open => {
                format!("PR #{} open (not merged)", pull_request.number)
            }
            InspectionPrState::Merged => format!("PR #{} merged", pull_request.number),
            InspectionPrState::Closed => {
                risky = true;
                format!("PR #{} closed without merging", pull_request.number)
            }
        };
        list = list.fact(if matches!(pull_request.state, InspectionPrState::Closed) {
            Fact::risk(label)
        } else {
            Fact::safe(label)
        });
    } else if !inspection.published {
        risky = true;
        list = list.fact(Fact::risk("never pushed"));
    }

    match inspection.session {
        SessionState::None => list = list.fact(Fact::safe("no session")),
        SessionState::Detached | SessionState::Attached => {
            risky = true;
            let running = if inspection.running.is_empty() {
                String::new()
            } else {
                format!(" \u{00b7} {} running", inspection.running.join(", "))
            };
            list = list.fact(Fact::risk(format!("session attached{running}")));
        }
        SessionState::Unknown => list = list.fact(Fact::unknown("session state unknown")),
    }

    for warning in &inspection.warnings {
        list = list.fact(Fact::unknown(warning.clone()));
    }
    if let Some(error) = inspection.error.as_ref() {
        list = list.fact(Fact::unknown(error.clone()));
    }

    Facts {
        list,
        risky,
        age_secs: age_secs(&inspection.inspected_at, now),
    }
}

// ---------------------------------------------------------------------------- seeding

/// Takes the published request and asks the daemon for the facts behind it.
pub(crate) fn seed(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let request = with_host(cx, |host| host.pending_confirm.take());
    let seq = with_host(cx, |host| {
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
fn inspect(
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
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let live = with_host(cx, |host| {
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
    })
    .detach();
}

/// Runs the prune dry run; an empty result closes the dialog with the §3.8.3 toast.
fn dry_run(repo: RepoId, seq: u64, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let reply = bridge.request(RequestBody::PruneWorktrees {
        dry_run: true,
        fetch: true,
        kill_sessions: false,
        repo: Some(repo.clone()),
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(answer) = reply.recv().await else {
            return;
        };
        cx.update(|cx| {
            let live = with_host(cx, |host| {
                if host.confirm.seq != seq {
                    return false;
                }
                host.confirm.loading = false;
                host.confirm.checked_at = Some(Instant::now());
                match answer {
                    Ok(ResponseBody::Pruned(result)) => host.confirm.prune = Some(result),
                    Ok(_) => {}
                    Err(failure) => host.confirm.error = Some(failure.message),
                }
                true
            });
            if !live {
                return;
            }
            let empty = with_host(cx, |host| {
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
    })
    .detach();
}

// ---------------------------------------------------------------------------- rendering

/// Renders the confirm (§3.8.3).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = with_host(cx, |host| host.confirm.clone());
    let Some(request) = draft.request.clone() else {
        // Nothing was published: say so instead of confirming an unknown action.
        return root(focus)
            .child(
                Dialog::new("Nothing to confirm")
                    .icon(Icon::CircleQuestionMark)
                    .width(super::Dialogs::Confirm.width())
                    .body(Text::ui("This confirm was opened without a target.").muted())
                    .hint_row(KeyHintRow::new().key("esc", "cancel")),
            )
            .into_any_element();
    };

    let card = if let ConfirmRequest::Prune { .. } = &request {
        prune_card(&request, &draft, cx)
    } else {
        facts_card(&request, &draft)
    };

    let accept_state = state.clone();
    let accept_bridge = bridge.clone();
    let strong_state = state.clone();
    let strong_bridge = bridge.clone();
    let recheck_state = state.clone();
    let recheck_bridge = bridge.clone();

    root(focus)
        .on_action(move |_: &confirm_actions::Accept, _window, cx| {
            if !strong_required(cx) {
                commit(&accept_state, &accept_bridge, cx);
            }
        })
        .on_action(move |_: &confirm_actions::AcceptStrong, _window, cx| {
            commit(&strong_state, &strong_bridge, cx);
        })
        .on_action(move |_: &confirm_actions::Recheck, _window, cx| {
            recheck(&recheck_state, &recheck_bridge, cx);
        })
        .on_action({
            let state = state.clone();
            move |_: &confirm_actions::ToggleKeep, _window, cx| {
                with_host(cx, |host| {
                    host.confirm.show_keep = !host.confirm.show_keep;
                });
                notify(&state, cx);
            }
        })
        .child(card)
        .into_any_element()
}

/// The compact / expanded facts confirm.
fn facts_card(request: &ConfirmRequest, draft: &ConfirmState) -> AnyElement {
    let now = now_epoch();
    let mut facts = match request {
        ConfirmRequest::DeleteWorktree { .. } => {
            worktree_facts(draft.inspection.as_ref(), draft.loading, now)
        }
        ConfirmRequest::KillSession {
            terminals,
            running,
            unsaved,
            ..
        } => kill_facts(*terminals, running, *unsaved),
        ConfirmRequest::CloseTerminal { running, .. } => close_terminal_facts(running.as_deref()),
        ConfirmRequest::DeleteRepo { worktrees, .. } => Facts {
            list: FactList::new()
                .fact(Fact::risk(format!(
                    "{worktrees} worktrees are deleted with it"
                )))
                .fact(Fact::risk("the pristine clone is moved to trash")),
            risky: true,
            age_secs: None,
        },
        ConfirmRequest::DeleteContext {
            repos,
            worktrees,
            sessions,
            ..
        } => Facts {
            list: FactList::new()
                .fact(Fact::risk(format!("{repos} repositories")))
                .fact(Fact::risk(format!("{worktrees} worktrees")))
                .fact(Fact::risk(format!("{sessions} running sessions"))),
            risky: true,
            age_secs: None,
        },
        _ => Facts::default(),
    };
    if let Some(error) = draft.error.as_ref() {
        facts.list = facts.list.fact(Fact::unknown(error.clone()));
    }
    let compact = facts.list.is_compact();

    let mut hints = KeyHintRow::new();
    if request.rechecks() {
        hints = hints.key("I", "re-check");
    }
    let mut card = ConfirmDialog::new(request.title(), facts.list.clone())
        .target(request.target())
        .consequence(request.consequence(&facts))
        .icon(request.icon(compact))
        .hints(hints)
        .action_label(request.action_label(0));
    if request.always_strong() {
        card = card.force_confirm_key(ConfirmKey::Upper);
    }
    if let Some(age) = facts.age_secs {
        card = card.stamp(FreshnessStamp::new("checked", age).action("I", "re-check"));
    }
    card.into_any_element()
}

/// `K` on a worktree: the facts are what the session is running (§3.8.3).
fn kill_facts(terminals: usize, running: &[String], unsaved: bool) -> Facts {
    let mut list = FactList::new().fact(Fact::risk(format!("{terminals} terminals")));
    if unsaved {
        list = list.fact(Fact::risk("an editor has unsaved changes"));
    }
    for label in running {
        list = list.fact(Fact::risk(format!("{label} running")));
    }
    Facts {
        list,
        risky: true,
        age_secs: None,
    }
}

/// `ctrl-s x`: one fact, the command that dies with the terminal.
fn close_terminal_facts(running: Option<&str>) -> Facts {
    match running {
        Some(command) => Facts {
            list: FactList::new().fact(Fact::risk(format!("{command} is running in it"))),
            risky: true,
            age_secs: None,
        },
        None => Facts {
            list: FactList::new().fact(Fact::safe("nothing is running in it")),
            risky: false,
            age_secs: None,
        },
    }
}

/// The 720 px multi-target prune body (§3.8.3).
fn prune_card(request: &ConfirmRequest, draft: &ConfirmState, cx: &mut App) -> AnyElement {
    let gap = cx.theme().space.xs;
    let deleted: Vec<String> = draft.prune.as_ref().map_or_else(Vec::new, |result| {
        result
            .deleted
            .iter()
            .map(|id| id.slug().to_owned())
            .collect()
    });
    let skipped = draft
        .prune
        .as_ref()
        .map_or_else(Vec::new, |result| result.skipped.clone());
    let total = deleted.len() + skipped.len();

    let mut body = div().flex().flex_col().gap(gap);
    if draft.loading {
        body = body.child(SpinnerWithLabel::new(
            "prune-dry-run",
            "checking worktrees\u{2026}",
        ));
    } else {
        body = body.child(SectionHeader::new("DELETE")).children(
            deleted
                .iter()
                .map(|slug| FactList::new().fact(Fact::safe(slug.clone()))),
        );
        if draft.show_keep {
            body = body.child(SectionHeader::new("KEEP")).children(
                skipped
                    .iter()
                    .map(|entry| {
                        FactList::new().fact(if entry.reason.contains("unknown") {
                            Fact::unknown(format!(
                                "{}   {}",
                                entry.worktree_id.slug(),
                                entry.reason
                            ))
                        } else {
                            Fact::risk(format!("{}   {}", entry.worktree_id.slug(), entry.reason))
                        })
                    })
                    .collect::<Vec<_>>(),
            );
        } else {
            body = body.child(
                KeyHintRow::new().key("s", format!("show the {} kept worktrees", skipped.len())),
            );
        }
    }

    let age = draft
        .checked_at
        .map_or(0, |at| i64::try_from(at.elapsed().as_secs()).unwrap_or(0));
    let body = body.child(FreshnessStamp::new("dry run \u{00b7} fetched", age));

    Dialog::new(format!(
        "{} \u{2014} {} of {total}",
        request.title(),
        deleted.len()
    ))
    .icon(Icon::Scissors)
    .width(px(720.0))
    .tone(Tone::Warning)
    .body(body)
    .hint_row(KeyHintRow::new().key("s", "keep list").key("n", "cancel"))
    .primary(format!("y  Prune {}", deleted.len()))
    .into_any_element()
}

/// Whether the open confirm needs `Y` rather than `y`.
fn strong_required(cx: &mut App) -> bool {
    with_host(cx, |host| {
        let draft = &host.confirm;
        let Some(request) = draft.request.as_ref() else {
            return true;
        };
        if request.always_strong() {
            return true;
        }
        match request {
            ConfirmRequest::DeleteWorktree { .. } => {
                let facts = worktree_facts(draft.inspection.as_ref(), draft.loading, now_epoch());
                facts.list.confirm_key() == ConfirmKey::Upper || draft.error.is_some()
            }
            ConfirmRequest::Prune { .. } => draft.loading || draft.prune.is_none(),
            ConfirmRequest::KillSession {
                running, unsaved, ..
            } => *unsaved || !running.is_empty(),
            _ => false,
        }
    })
}

/// `I`: re-run the inspection with a fetch, swapping the values in place.
fn recheck(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let (request, seq) = with_host(cx, |host| {
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
            with_host(cx, |host| host.confirm.loading = false);
        }
    }
}

/// Confirms: send the mutation and close. Everything it starts lives in fleetd.
fn commit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let Some(request) = with_host(cx, |host| host.confirm.request.clone()) else {
        return;
    };
    if with_host(cx, |host| host.confirm.loading) {
        // §3.8.3: while the dry run is still going, the key simply does nothing.
        return;
    }
    match request {
        ConfirmRequest::DeleteWorktree { id } => {
            let reply = bridge.request(RequestBody::DeleteWorktrees { ids: vec![id] });
            let state = state.clone();
            cx.spawn(async move |cx| {
                let Ok(Ok(ResponseBody::WorktreesDeleted(results))) = reply.recv().await else {
                    return;
                };
                let trash_entry = results.into_iter().find_map(|result| result.trash_entry);
                if let Some(trash_entry) = trash_entry {
                    state.update(cx, |app, cx| {
                        app.last_trash_entry = Some(trash_entry);
                        cx.notify();
                    });
                }
            })
            .detach();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn inspection() -> WorktreeInspection {
        WorktreeInspection {
            worktree_id: WorktreeId::try_from("buk/payroll#fix-rut")
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            host: "local".to_owned(),
            path: "/tmp/wt".to_owned(),
            branch: "fix-rut".to_owned(),
            base_ref: "origin/main".to_owned(),
            head: Some("abc".to_owned()),
            target_branch: "origin/main".to_owned(),
            upstream: Some("origin/fix-rut".to_owned()),
            ahead: Some(0),
            behind: Some(0),
            upstream_gone: false,
            dirty: false,
            dirty_files: Some(0),
            merged_into_target: true,
            unique_commits: Some(0),
            published: true,
            merged: true,
            pr: None,
            session: SessionState::None,
            running: Vec::new(),
            inspected_at: "2026-09-04T12:00:00Z".to_owned(),
            warnings: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn a_clean_merged_unattached_worktree_is_a_compact_lowercase_confirm() {
        let facts = worktree_facts(Some(&inspection()), false, 1_788_523_200);
        assert!(facts.list.is_compact());
        assert_eq!(facts.list.confirm_key(), ConfirmKey::Lower);
        assert!(!facts.risky);
        assert_eq!(facts.age_secs, Some(0));
    }

    #[test]
    fn an_unknown_unique_commit_count_escalates_to_upper() {
        let mut probe = inspection();
        probe.unique_commits = None;
        let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
        assert_eq!(facts.list.confirm_key(), ConfirmKey::Upper);
        assert!(!facts.list.is_compact());
    }

    #[test]
    fn dirt_and_a_session_are_risks_that_expand_the_dialog() {
        let mut probe = inspection();
        probe.dirty = true;
        probe.dirty_files = Some(12);
        probe.session = SessionState::Attached;
        probe.running = vec!["claude".to_owned()];
        let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
        assert!(facts.risky);
        assert!(!facts.list.is_compact());
        assert_eq!(
            facts.list.confirm_key(),
            ConfirmKey::Lower,
            "known risks still confirm with `y`; only unknowns escalate"
        );
    }

    #[test]
    fn warnings_arrive_verbatim_as_unknown_facts() {
        let mut probe = inspection();
        probe.warnings = vec![fleet_core::inspection::WARNING_GH_UNAVAILABLE.to_owned()];
        let facts = worktree_facts(Some(&probe), false, 1_788_523_200);
        assert_eq!(facts.list.confirm_key(), ConfirmKey::Upper);
    }

    #[test]
    fn facts_that_have_not_arrived_yet_are_loading_and_strong() {
        let facts = worktree_facts(None, true, 0);
        assert_eq!(facts.list.confirm_key(), ConfirmKey::Upper);
        assert_eq!(facts.age_secs, None);
    }

    #[test]
    fn repo_and_context_deletes_are_always_strong() {
        let repo = RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}"));
        assert!(
            ConfirmRequest::DeleteRepo {
                repo: repo.clone(),
                worktrees: 8
            }
            .always_strong()
        );
        assert!(!ConfirmRequest::Prune { repo }.always_strong());
    }

    #[test]
    fn every_action_has_the_wording_the_spec_fixes() {
        let id = WorktreeId::try_from("buk/payroll#fix-rut-validator")
            .unwrap_or_else(|error| panic!("{error}"));
        let request = ConfirmRequest::DeleteWorktree { id };
        assert_eq!(request.title(), "Delete buk/payroll#fix-rut-validator?");
        let benign = Facts::default();
        assert_eq!(
            request.consequence(&benign),
            "Moves the copy to trash, then removes it in the background."
        );
        let risky = Facts {
            risky: true,
            ..Facts::default()
        };
        assert!(request.consequence(&risky).contains("kills the session"));
    }
}
