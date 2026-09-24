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
                    .dismiss_action(crate::dialogs::Dialogs::Confirm.dismiss_action())
                    .icon(Icon::CircleQuestionMark)
                    .width(crate::dialogs::Dialogs::Confirm.width(cx))
                    .body(Text::ui("This confirm was opened without a target.").muted()),
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
                commit(&accept_state, &accept_bridge, ConfirmKey::Lower, cx);
            }
        })
        .on_action(move |_: &confirm_actions::AcceptStrong, _window, cx| {
            commit(&strong_state, &strong_bridge, ConfirmKey::Upper, cx);
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
    let facts = facts_for(request, draft, now);
    let compact = facts.list.is_compact();
    let policy = confirmation_policy(draft, now);

    let title = request.title();
    // §3.8.3 names the target in exactly one place: the subtitle, unless the title already says
    // it in full.
    let subtitle = draft.subtitle.clone().or_else(|| {
        let target = request.target();
        (!title.contains(&target)).then_some(target)
    });
    let consequence = request.consequence(&facts);
    let mut card = ConfirmDialog::new(title, facts.list)
        .dismiss_action(crate::dialogs::Dialogs::Confirm.dismiss_action())
        .accept_actions(
            Box::new(confirm_actions::Accept),
            Box::new(confirm_actions::AcceptStrong),
        )
        .consequence(consequence)
        .icon(request.icon(compact))
        .action_label(request.action_label(0))
        .strong_label(request.strong_label())
        .force_confirm_key(policy.key)
        .accept_disabled(!policy.authorized);
    if let Some(subtitle) = subtitle {
        card = card.target(subtitle);
    }
    if let Some(age) = facts.age_secs {
        card = card.stamp(FreshnessStamp::new("Checked", age));
    }
    if request.rechecks() {
        card = card.recheck_action(Box::new(confirm_actions::Recheck));
    }
    card.into_any_element()
}

/// The decisive facts this request states, and the risk flag its consequence sentence reads.
///
/// Split out of [`facts_card`] so the sentence the harness reports back is built by the same
/// code that draws it: a second formatting of the same request would drift a word at a time
/// (`docs/TESTING-HARNESS.md` §3, `dialog.message`).
pub(super) fn facts_for(request: &ConfirmRequest, draft: &ConfirmState, now: i64) -> Facts {
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
        ConfirmRequest::DeleteRepo { worktrees, .. } => {
            let lead = plural(*worktrees as u64, "worktree");
            Facts {
                list: FactList::new()
                    .fact(Fact::risk(format!("{lead} deleted with it")).strong(&lead))
                    .fact(Fact::risk("the pristine clone is moved to trash")),
                risky: true,
                ..Facts::default()
            }
        }
        ConfirmRequest::DeleteContext {
            repos,
            worktrees,
            sessions,
            ..
        } => {
            let strong = |count: usize, one: &str, many: &str| {
                let lead = format!("{count} {}", if count == 1 { one } else { many });
                Fact::risk(lead.clone()).strong(&lead)
            };
            Facts {
                list: FactList::new()
                    .fact(strong(*repos, "repository", "repositories"))
                    .fact(strong(*worktrees, "worktree", "worktrees"))
                    .fact(strong(*sessions, "running session", "running sessions")),
                risky: true,
                ..Facts::default()
            }
        }
        _ => Facts::default(),
    };
    if let Some(error) = draft.error.as_ref() {
        facts.list = facts.list.fact(Fact::unknown(error.clone()));
    }
    facts
}

/// One row of a prune section: the worktree, and why it is kept when it is.
fn prune_row(slug: &str, reason: Option<(&str, bool)>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (icon, tone) = match reason {
        Some(_) => (Icon::TriangleAlert, Tone::Warning),
        None => (Icon::Check, Tone::Success),
    };
    div()
        .flex()
        .items_center()
        .gap(theme.space.sm)
        .h(theme.metrics.row_h)
        .w_full()
        .child(icon.el().size(IconSize::Medium).color(tone.color(theme)))
        .child(Text::data(slug.to_owned()).flex_none())
        .children(reason.map(|(reason, unknown)| {
            Text::ui(reason.to_owned())
                .tone(if unknown { Tone::Warning } else { Tone::Muted })
                .ellipsize()
        }))
        .into_any_element()
}

/// The 720 px multi-target prune confirm (§3.8.3): a `Delete` and a `Keep` section, each row
/// with its reason, on the same alert frame and buttons as every other confirm.
pub(super) fn prune_card(
    request: &ConfirmRequest,
    draft: &ConfirmState,
    host: Entity<crate::dialogs::host::DialogHost>,
    cx: &App,
) -> AnyElement {
    let (deleted, skipped) = draft.prune.as_ref().map_or((0, 0), |result| {
        (result.deleted.len(), result.skipped.len())
    });
    let total = deleted + skipped;
    let mut body = div().flex().flex_col().gap(cx.theme().space.xs);
    if draft.loading {
        body = body.child(SpinnerWithLabel::new(
            "prune-dry-run",
            "checking worktrees…",
        ));
    } else if let Some(list) = &draft.list {
        body = body.child(
            gpui::list(list.clone(), move |index, _, cx| {
                let draft = &host.read(cx).confirm;
                let Some(result) = &draft.prune else {
                    return div().into_any_element();
                };
                if index == 0 {
                    return SectionHeader::new("Delete")
                        .trailing(Text::hint(format!("{} worktrees", result.deleted.len())))
                        .into_any_element();
                }
                if let Some(id) = result.deleted.get(index - 1) {
                    return prune_row(id.slug(), None, cx);
                }
                if index == result.deleted.len() + 1 {
                    return SectionHeader::new("Keep")
                        .trailing(Text::hint(format!("{} worktrees", result.skipped.len())))
                        .into_any_element();
                }
                let Some(entry) = result.skipped.get(index - result.deleted.len() - 2) else {
                    return div().into_any_element();
                };
                prune_row(
                    entry.worktree_id.slug(),
                    Some((&entry.reason, entry.reason.contains("unknown"))),
                    cx,
                )
            })
            .h(cx.theme().metrics.row_h * draft.list_len().clamp(1, 10) as f32),
        );
    }
    let age = draft
        .checked_at
        .map_or(0, |at| i64::try_from(at.elapsed().as_secs()).unwrap_or(0));
    let policy = confirmation_policy(draft, now_unix());
    let mut card = ConfirmDialog::new(
        format!("{} \u{2014} {deleted} of {total}", request.title()),
        FactList::new(),
    )
    .dismiss_action(crate::dialogs::Dialogs::Confirm.dismiss_action())
    .accept_actions(
        Box::new(confirm_actions::Accept),
        Box::new(confirm_actions::AcceptStrong),
    )
    .icon(Icon::Scissors)
    .width(crate::dialogs::Dialogs::Settings.width(cx))
    .body(body)
    .stamp(FreshnessStamp::new("dry run \u{00b7} fetched", age))
    .recheck_action(Box::new(confirm_actions::Recheck))
    .consequence(request.consequence(&Facts::default()))
    .action_label(request.action_label(deleted))
    .force_confirm_key(policy.key)
    .accept_disabled(!policy.authorized);
    if skipped > 0 && !draft.loading {
        card = card.footer_start(
            Button::new(
                "prune-show-kept",
                if draft.show_keep {
                    "Hide kept"
                } else {
                    "Show kept"
                },
            )
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact)
            .selected(draft.show_keep)
            .action(Box::new(confirm_actions::ToggleKeep)),
        );
    }
    if let Some(error) = draft.error.as_ref() {
        card = card.error(error.clone());
    }
    card.into_any_element()
}
