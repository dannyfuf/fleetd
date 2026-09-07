use super::*;

impl HubCtx {
    pub(super) fn open_repo(&self, cx: &mut App) {
        let Some(row) = self.selected_rail_row(cx) else {
            return;
        };
        if row.kind == RailKind::CloneFailed {
            // §3.2: `Enter` on a failed clone opens the Jobs panel focused on that job.
            self.state.update(cx, |state, cx| {
                state.open_overlay(Overlay::Jobs);
                cx.notify();
            });
            return;
        }
        self.state.update(cx, |state, cx| {
            state.scope = match &row.repo {
                Some(repo) if row.kind != RailKind::All => RepoScope::Repo(repo.clone()),
                _ => RepoScope::All,
            };
            state.cursors.worktrees = 0;
            state.hub_pane = HubPane::List;
            cx.notify();
        });
        self.schedule_inspection(cx);
    }

    pub(super) fn delete_repo(&self, cx: &mut App) {
        let Some(repo) = self.selected_rail_row(cx).and_then(|row| row.repo) else {
            return;
        };
        let worktrees = self
            .state
            .read(cx)
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .worktrees
                    .iter()
                    .filter(|worktree| worktree.repo_id == repo)
                    .count()
            })
            .unwrap_or_default();
        self.prepare_confirm(dialogs::ConfirmRequest::DeleteRepo { repo, worktrees }, cx);
    }

    /// `x` is bound only on a clone-failed row; on every other row it is a no-op (KEYMAP §Repos).
    pub(super) fn dismiss_clone(&self, cx: &mut App) {
        let Some(row) = self.selected_rail_row(cx) else {
            return;
        };
        if row.kind != RailKind::CloneFailed {
            return;
        }
        let Some(repo) = row.repo else { return };
        if self.refuses(cx) {
            return;
        }
        self.bridge.send(RequestBody::DismissClone { repo });
    }

    pub(super) fn edit_hooks(&self, cx: &mut App) {
        let Some(repo) = self.selected_rail_row(cx).and_then(|row| row.repo) else {
            return;
        };
        dialogs::request_edit_hooks(cx, repo);
        self.open_dialog(Dialogs::EditHooks, cx);
    }

    pub(super) fn open_worktree(&self, sleep_previous: bool, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        if self.refuses(cx) {
            return;
        }
        self.open_session(row.id, sleep_previous, cx);
    }

    /// Marks a worktree as opened and enters its session once the daemon confirms it exists.
    fn open_session(&self, id: WorktreeId, sleep_previous: bool, cx: &mut App) {
        self.bridge
            .send(RequestBody::TouchWorktreeOpened { id: id.clone() });
        self.ask(
            RequestBody::EnsureSession {
                worktree: Some(id),
                agent: None,
                sleep_previous,
            },
            cx,
            |result, ctx, cx| ctx.enter_session(result, cx),
        );
    }

    /// Moves to the Workspace once the daemon confirms the session exists.
    pub(super) fn enter_session(
        &self,
        result: Result<ResponseBody, ProtoError>,
        cx: &mut gpui::AsyncApp,
    ) {
        match result {
            Ok(ResponseBody::Session(session)) => {
                self.state.update(cx, |state, cx| {
                    state.touch_session(session.id.clone());
                    state.screen = Screen::Workspace {
                        session: session.id,
                    };
                    cx.notify();
                });
            }
            Ok(_) => {}
            Err(error) => self.report(error, cx),
        }
    }

    /// A failed request is sticky, never a toast (§1.8).
    pub(super) fn report(&self, error: ProtoError, cx: &mut gpui::AsyncApp) {
        self.state.update(cx, |state, cx| {
            state.sticky_error = Some(crate::state::StickyError {
                text: error.message,
                job: None,
                retryable: false,
            });
            cx.notify();
        });
    }

    pub(super) fn delete_worktree(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        self.prepare_confirm(dialogs::ConfirmRequest::DeleteWorktree { id: row.id }, cx);
    }

    pub(super) fn undo_delete(&self, cx: &mut App) {
        let entry = self.state.read(cx).last_trash_entry.clone();
        let Some(entry) = entry else {
            self.toast("Nothing to undo", Icon::Info, true, cx);
            return;
        };
        if self.refuses(cx) {
            return;
        }
        self.bridge.send(RequestBody::RestoreTrash { entry });
        self.state
            .update(cx, |state, _| state.last_trash_entry = None);
    }

    /// `x` — the dry run first, then either a refusal toast or the confirm (§3.8.3).
    pub(super) fn prune(&self, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        if let Some(repo) = self.scoped_repo(cx) {
            self.prepare_confirm(dialogs::ConfirmRequest::Prune { repo }, cx);
        }
    }

    pub(super) fn sleep(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        if self.refuses(cx) {
            return;
        }
        self.ask(
            RequestBody::SleepWorktree { id: row.id },
            cx,
            |result, ctx, cx| match result {
                Ok(ResponseBody::Slept(report)) if !report.kept.is_empty() => {
                    let kept = report
                        .kept
                        .iter()
                        .map(|kept| format!("{} ({})", kept.window, kept.reason))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let now = Instant::now();
                    ctx.state.update(cx, |state, cx| {
                        state.toast(
                            Toast::new(format!("Slept · kept {kept}")).icon(Icon::Moon),
                            now,
                            dwell_for(ToastDuration::Normal),
                        );
                        cx.notify();
                    });
                }
                Ok(_) => {}
                Err(error) => ctx.report(error, cx),
            },
        );
    }

    pub(super) fn kill(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        let Some(session) = self.state.read(cx).snapshot.as_ref().and_then(|snapshot| {
            snapshot.sessions.iter().find(|session| {
                matches!(
                    &session.kind,
                    fleet_core::sessions::SessionKind::Worktree(id) if id == &row.id
                )
            })
        }) else {
            return;
        };
        self.prepare_confirm(
            dialogs::ConfirmRequest::KillSession {
                session: session.id.clone(),
                terminals: session.terminals.len(),
                running: row.keep_alive.iter().map(ToString::to_string).collect(),
                unsaved: false,
            },
            cx,
        );
    }

    pub(super) fn inspect_selected(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        self.inspect(row.id, true, cx);
    }

    pub(super) fn copy_path(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        self.ask(
            RequestBody::WorktreePath { id: row.id },
            cx,
            |result, ctx, cx| {
                if let Ok(ResponseBody::Path(path)) = result {
                    cx.update(|cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path));
                    });
                    let now = Instant::now();
                    ctx.state.update(cx, |state, cx| {
                        state.toast_short("Path copied", Icon::ClipboardCheck, now);
                        cx.notify();
                    });
                }
            },
        );
    }

    pub(super) fn copy_branch(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(row.branch.to_string()));
        self.toast("Branch copied", Icon::ClipboardCheck, true, cx);
    }

    pub(super) fn switch_tab(&self, tab: PrTab, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            if state.pr_tab != tab {
                state.pr_tab = tab;
                cx.notify();
            }
        });
    }

    pub(super) fn back_to_worktrees(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            state.filter = crate::state::FilterState::default();
            cx.notify();
        });
    }

    /// `Enter` / `O` / `c` on the PR screen: open the local worktree, or create it first.
    pub(super) fn open_pr(&self, open: bool, sleep_previous: bool, cx: &mut App) {
        let Some(row) = self.selected_pr(cx) else {
            return;
        };
        if self.refuses(cx) {
            return;
        }
        if let Some(id) = row.local.clone() {
            if open {
                self.open_session(id, sleep_previous, cx);
            }
            return;
        }
        let key = (row.repo.clone(), row.number);
        self.hub.update(cx, |hub, _| {
            hub.invalidate();
            hub.creating.push(key.clone());
        });
        // `HubState` is its own entity and the shell only observes `AppState`, so a mutation
        // here repaints nothing on its own — and `creating` is what makes the §3.5 glyph spin
        // for the length of the `gh` round trip.
        self.state.update(cx, |_, cx| cx.notify());
        self.ask(
            RequestBody::CreateWorktreeFromPr {
                repo: row.repo.clone(),
                number: row.number,
            },
            cx,
            move |result, ctx, cx| {
                ctx.hub.update(cx, |hub, _| {
                    hub.invalidate();
                    hub.creating.retain(|candidate| candidate != &key);
                });
                // Same reason as above: the spinner has to stop even when the branch below
                // repaints nothing of its own.
                ctx.state.update(cx, |_, cx| cx.notify());
                match result {
                    Ok(ResponseBody::Worktree { worktree, .. }) if open => {
                        ctx.ask_from_async(worktree.id, sleep_previous, cx);
                    }
                    Ok(_) => {}
                    Err(error) => ctx.report(error, cx),
                }
            },
        );
    }

    /// The second half of "create then open", already on the async context.
    pub(super) fn ask_from_async(
        &self,
        id: WorktreeId,
        sleep_previous: bool,
        cx: &mut gpui::AsyncApp,
    ) {
        let reply = self.bridge.request(RequestBody::EnsureSession {
            worktree: Some(id),
            agent: None,
            sleep_previous,
        });
        let ctx = self.clone();
        cx.spawn(async move |cx| {
            if let Ok(result) = reply.recv().await {
                ctx.enter_session(result, cx);
            }
        })
        .detach();
    }

    pub(super) fn inspect_pr(&self, cx: &mut App) {
        let Some(id) = self.selected_pr(cx).and_then(|row| row.local) else {
            return;
        };
        self.inspect(id, true, cx);
    }

    pub(super) fn copy_pr_url(&self, cx: &mut App) {
        let Some(row) = self.selected_pr(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(row.url.to_string()));
        self.toast("PR URL copied", Icon::ClipboardCheck, true, cx);
    }

    pub(super) fn refresh(&self, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        let repo = match &self.state.read(cx).scope {
            RepoScope::Repo(repo) => Some(repo.clone()),
            RepoScope::All => None,
        };
        self.bridge.send(RequestBody::RefreshStatuses { repo });
        self.fetch_pull_requests(true, cx);
    }

    /// `b` — the PR's URL on the PR screen, the inspected PR or the repo elsewhere.
    pub(super) fn open_in_browser(&self, cx: &mut App) {
        let url = if matches!(self.state.read(cx).screen, Screen::Hub { tab: HubTab::Prs }) {
            self.selected_pr(cx).map(|row| row.url.to_string())
        } else if self.state.read(cx).hub_pane == HubPane::Repos {
            self.selected_rail_row(cx)
                .and_then(|row| row.repo)
                .map(|repo| format!("https://github.com/{repo}"))
        } else {
            self.selected_worktree(cx).and_then(|row| {
                let hub = self.hub.read(cx);
                hub.inspections
                    .get(&row.id)
                    .and_then(|slot| slot.data.as_ref())
                    .and_then(|data| data.pr.as_ref())
                    .map(|pr| pr.url.clone())
                    .or_else(|| Some(format!("https://github.com/{}", row.repo)))
            })
        };
        if let Some(url) = url {
            cx.open_url(&url);
        }
    }

    pub(super) fn open_dialog(&self, dialog: Dialogs, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.open_overlay(Overlay::Dialog(dialog));
            cx.notify();
        });
    }

    /// Stores a prepared confirm and opens the shared §3.8 dialog frame.
    pub(super) fn prepare_confirm(&self, request: dialogs::ConfirmRequest, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        dialogs::request_confirm(cx, request);
        self.state.update(cx, |state, cx| {
            state.open_overlay(Overlay::Dialog(Dialogs::Confirm));
            cx.notify();
        });
    }
}
