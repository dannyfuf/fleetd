use super::*;

/// Renders the confirm (§3.8.3).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let draft = &host.read(cx).confirm;
    let Some(request) = draft.request.clone() else {
        // Nothing was published: say so instead of confirming an unknown action.
        return root(focus)
            .child(
                Dialog::new("Nothing to confirm")
                    .icon(Icon::CircleQuestionMark)
                    .width(crate::dialogs::Dialogs::Confirm.width(cx))
                    .body(Text::ui("This confirm was opened without a target.").muted())
                    .hint_row(KeyHintRow::new().key("esc", "cancel")),
            )
            .into_any_element();
    };

    let card = if let ConfirmRequest::Prune { .. } = &request {
        prune_card(&request, draft, host.clone(), cx)
    } else {
        facts_card(&request, draft)
    };

    let accept_state = state.clone();
    let accept_bridge = bridge.clone();
    let strong_state = state.clone();
    let strong_bridge = bridge.clone();
    let recheck_state = state.clone();
    let recheck_bridge = bridge.clone();

    root(focus)
        .on_action(move |_: &confirm_actions::Accept, _window, cx| {
            if !strong_required(&accept_state, cx) {
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
                with_host(&state, cx, |host| {
                    host.confirm.show_keep = !host.confirm.show_keep;
                    host.confirm.update_list();
                });
                notify(&state, cx);
            }
        })
        .child(card)
        .into_any_element()
}

/// The compact / expanded facts confirm.
pub(super) fn facts_card(request: &ConfirmRequest, draft: &ConfirmState) -> AnyElement {
    let now = now_unix();
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
    let title = request.title(compact);
    let target = request.target();
    let show_target = !title.contains(&target);
    let consequence = request.consequence(&facts);
    let mut card = ConfirmDialog::new(title, facts.list)
        .consequence(consequence)
        .icon(request.icon(compact))
        .hints(hints)
        .action_label(request.action_label(0));
    // §3.8.3 puts the full id in exactly one place: the title, or row 1 of the expanded body.
    if show_target {
        card = card.target(target);
    }
    if request.always_strong() {
        card = card.force_confirm_key(ConfirmKey::Upper);
    }
    if let Some(age) = facts.age_secs {
        card = card.stamp(FreshnessStamp::new("checked", age).action("I", "re-check"));
    }
    card.into_any_element()
}

/// The 720 px multi-target prune body (§3.8.3).
pub(super) fn prune_card(
    request: &ConfirmRequest,
    draft: &ConfirmState,
    host: Entity<crate::dialogs::host::DialogHost>,
    cx: &App,
) -> AnyElement {
    let gap = cx.theme().space.xs;
    let (deleted, skipped) = draft.prune.as_ref().map_or((0, 0), |result| {
        (result.deleted.len(), result.skipped.len())
    });
    let total = deleted + skipped;
    let mut body = div().flex().flex_col().gap(gap);
    if draft.loading {
        body = body.child(SpinnerWithLabel::new(
            "prune-dry-run",
            "checking worktrees…",
        ));
    } else {
        if let Some(list) = &draft.list {
            body = body.child(
                gpui::list(list.clone(), move |index, _, cx| {
                    let draft = &host.read(cx).confirm;
                    let Some(result) = &draft.prune else {
                        return div().into_any_element();
                    };
                    if index == 0 {
                        return SectionHeader::new("DELETE").into_any_element();
                    }
                    if let Some(id) = result.deleted.get(index - 1) {
                        return FactList::new()
                            .fact(Fact::safe(id.slug().to_owned()))
                            .into_any_element();
                    }
                    if index == result.deleted.len() + 1 {
                        return SectionHeader::new("KEEP").into_any_element();
                    }
                    let Some(entry) = result.skipped.get(index - result.deleted.len() - 2) else {
                        return div().into_any_element();
                    };
                    let label = format!("{}   {}", entry.worktree_id.slug(), entry.reason);
                    FactList::new()
                        .fact(if entry.reason.contains("unknown") {
                            Fact::unknown(label)
                        } else {
                            Fact::risk(label)
                        })
                        .into_any_element()
                })
                .h(cx.theme().metrics.row_h * draft.list_len().clamp(1, 10) as f32),
            );
        }
        if !draft.show_keep {
            body = body
                .child(KeyHintRow::new().key("s", format!("show the {skipped} kept worktrees")));
        }
    }
    let age = draft
        .checked_at
        .map_or(0, |at| i64::try_from(at.elapsed().as_secs()).unwrap_or(0));
    let body = body.child(FreshnessStamp::new("dry run · fetched", age));
    Dialog::new(format!("{} — {deleted} of {total}", request.title(true)))
        .icon(Icon::Scissors)
        .width(crate::dialogs::Dialogs::Settings.width(cx))
        .tone(Tone::Warning)
        .body(body)
        .hint_row(KeyHintRow::new().key("s", "keep list").key("n", "cancel"))
        .primary(format!("y  Prune {deleted}"))
        .into_any_element()
}
